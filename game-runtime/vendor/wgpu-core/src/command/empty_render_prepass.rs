//! Skip only proven-empty render prepasses. Read-only proofs never modify a
//! tracker or make GPU calls.
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    pub enabled: bool,
    pub diagnostics: bool,
}
impl Settings {
    pub fn requested() -> Self {
        static SETTINGS: OnceLock<Settings> = OnceLock::new();
        *SETTINGS.get_or_init(|| Self {
            enabled: std::env::var_os("MUGUET_SKIP_EMPTY_RENDER_PREPASS").is_some_and(|v| v == "1"),
            diagnostics: std::env::var_os("MUGUET_RENDER_PREPASS_DIAGNOSTICS").is_some_and(|v| v == "1"),
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Facts {
    pub ohos_vulkan: bool,
    pub discard_fixups_empty: bool,
    pub query_resets_empty: bool,
    pub indirect_batches_empty: bool,
    pub pending_buffers_empty: bool,
    pub pending_textures_empty: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Evaluation {
    pub observed: bool,
    pub eligible: bool,
    pub skip: bool,
}

/// `None` is complex/unknown state. Deliberately reject it, even for a new
/// texture, rather than duplicating the subresource interval algorithm.
/// For known simple new resources, the command-buffer tracker only inserts
/// start/end states; actual initial device transitions still occur at submit.
pub(crate) fn resource_proves_no_transition(
    owned: bool,
    previous: Option<u64>,
    next: Option<u64>,
    ordered_mask: u64,
) -> bool {
    let Some(next) = next else { return false; };
    if !owned { return true; }
    previous.is_some_and(|previous| previous == next && previous & ordered_mask == previous)
}

/// With diagnostics ON, OFF and ON perform the same read-only proof and count
/// eligibility identically. With both options OFF, even the proof is bypassed.
pub(crate) fn evaluate(
    settings: Settings,
    facts: Facts,
    prove_resource_states: impl FnOnce() -> bool,
) -> Evaluation {
    let observed = (settings.enabled || settings.diagnostics) && facts.ohos_vulkan;
    let eligible = observed
        && facts.discard_fixups_empty
        && facts.query_resets_empty
        && facts.indirect_batches_empty
        && facts.pending_buffers_empty
        && facts.pending_textures_empty
        && prove_resource_states();
    Evaluation { observed, eligible, skip: settings.enabled && eligible }
}

static OBSERVED: AtomicU64 = AtomicU64::new(0);
static ELIGIBLE: AtomicU64 = AtomicU64::new(0);
static SKIPPED: AtomicU64 = AtomicU64::new(0);
static PREPARED_FALLBACK: AtomicU64 = AtomicU64::new(0);
static REPORTS: AtomicU64 = AtomicU64::new(0);
const REPORT_EVERY: u64 = 4096;
const MAX_REPORTS: u64 = 64;

pub(crate) fn note(settings: Settings, evaluation: Evaluation) {
    if !evaluation.observed { return; }
    let sequence = OBSERVED.fetch_add(1, Ordering::Relaxed) + 1;
    if evaluation.eligible { ELIGIBLE.fetch_add(1, Ordering::Relaxed); }
    if (sequence == 1 || sequence % REPORT_EVERY == 0)
        && REPORTS.fetch_add(1, Ordering::Relaxed) < MAX_REPORTS
    {
        std::eprintln!("OHOS empty-render-prepass pid={} enabled={} diagnostics={} observed={} eligible={} skipped={} preparedFallback={} reportEvery={} maxReports={} scope=CPU_decisions_not_GPU_busy_or_frame_count",
            std::process::id(), settings.enabled, settings.diagnostics, sequence,
            ELIGIBLE.load(Ordering::Relaxed), SKIPPED.load(Ordering::Relaxed),
            PREPARED_FALLBACK.load(Ordering::Relaxed), REPORT_EVERY, MAX_REPORTS);
    }
}

pub(crate) fn prepared(pending_is_empty: bool) {
    if pending_is_empty { SKIPPED.fetch_add(1, Ordering::Relaxed); }
    else { PREPARED_FALLBACK.fetch_add(1, Ordering::Relaxed); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    fn facts() -> Facts {
        Facts { ohos_vulkan:true, discard_fixups_empty:true, query_resets_empty:true,
            indirect_batches_empty:true, pending_buffers_empty:true, pending_textures_empty:true }
    }
    #[test]
    fn default_off_never_invokes_proof_or_skips() {
        assert_eq!(evaluate(Settings {enabled:false,diagnostics:false}, facts(), || panic!("not observed")),
            Evaluation {observed:false,eligible:false,skip:false});
    }
    #[test]
    fn diagnostics_make_off_on_proof_intensity_symmetric() {
        for proof in [false,true] {
            let calls=Cell::new(0);
            let a=evaluate(Settings {enabled:false,diagnostics:true},facts(),||{calls.set(calls.get()+1);proof});
            let b=evaluate(Settings {enabled:true,diagnostics:true},facts(),||{calls.set(calls.get()+1);proof});
            assert_eq!(calls.get(),2);assert_eq!(a.observed,b.observed);assert_eq!(a.eligible,b.eligible);
            assert!(!a.skip);assert_eq!(b.skip,proof);
        }
    }
    #[test]
    fn every_external_work_or_platform_gate_retains_legacy_without_proof() {
        for blocker in 0..6 {
            let mut f=facts();
            match blocker {0=>f.ohos_vulkan=false,1=>f.discard_fixups_empty=false,
                2=>f.query_resets_empty=false,3=>f.indirect_batches_empty=false,
                4=>f.pending_buffers_empty=false,_=>f.pending_textures_empty=false}
            for enabled in [false,true] {
                let e=evaluate(Settings {enabled,diagnostics:true},f,||panic!("blocked before resource proof"));
                assert!(!e.eligible && !e.skip);
            }
        }
    }
    #[test]
    fn fresh_simple_write_resources_only_set_command_buffer_start_end() {
        for usage in [0,1,2,4,8,u64::MAX] {
            assert!(resource_proves_no_transition(false,None,Some(usage),0));
        }
    }
    #[test]
    fn equal_ordered_reads_pass_and_equal_nonordered_writes_do_not() {
        assert!(resource_proves_no_transition(true,Some(3),Some(3),3));
        assert!(!resource_proves_no_transition(true,Some(4),Some(4),3));
        assert!(!resource_proves_no_transition(true,Some(1),Some(3),3));
        assert!(!resource_proves_no_transition(true,Some(3),Some(1),3));
    }
    #[test]
    fn complex_or_unknown_subresource_states_keep_legacy() {
        for owned in [false,true] {
            assert!(!resource_proves_no_transition(owned,Some(1),None,u64::MAX));
            assert!(!resource_proves_no_transition(owned,None,None,u64::MAX));
        }
        assert!(!resource_proves_no_transition(true,None,Some(1),u64::MAX));
    }
    #[test]
    fn exhaustive_simple_state_proof_preserves_original_barrier_equation() {
        for previous in 0..16 {
            for next in 0..16 {
                for ordered in 0..16 {
                    // Actual tracker skip_barrier compares identical flag bits
                    // and ordered_mask.contains(previous); independent bit scan
                    // here checks that no previous-use bit is outside the mask.
                    let original_skip = previous == next && (0..4).all(|bit|
                        previous & (1 << bit) == 0 || ordered & (1 << bit) != 0);
                    assert_eq!(resource_proves_no_transition(true,Some(previous),Some(next),ordered), original_skip);
                }
            }
        }
    }
    #[test]
    fn one_unproven_resource_prevents_aggregate_skip() {
        let cases=[(false,None,Some(8),0),(true,Some(1),Some(1),1),(true,Some(8),Some(8),1)];
        let e=evaluate(Settings {enabled:true,diagnostics:false},facts(),||cases.into_iter().all(
            |(owned,previous,next,mask)|resource_proves_no_transition(owned,previous,next,mask)));
        assert!(!e.skip);
    }
}
