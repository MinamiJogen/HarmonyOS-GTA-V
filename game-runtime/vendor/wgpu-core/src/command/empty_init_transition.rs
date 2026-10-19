//! Narrow OHOS experiment: an empty init-range list has no clear command.
//! Observation cost is identical with the skip option on or off. No settings
//! change any nonempty initialization or actual resource validation.
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;

const REPORT_EVERY: u64 = 65_536;
const MAX_REPORTS: u64 = 64;
static ENABLED: OnceLock<bool> = OnceLock::new();
static ENTRIES: AtomicU64 = AtomicU64::new(0);
static EMPTY: AtomicU64 = AtomicU64::new(0);
static DECISIONS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
static FIRST_EMPTY: AtomicBool = AtomicBool::new(false);
static REPORTS: AtomicU64 = AtomicU64::new(0);

/// Return a decision only. The caller must still try_raw before skipping.
/// Counts describe decisions, including an attempt which later fails try_raw.
pub(super) fn observe(empty: bool) -> bool {
    let enabled = *ENABLED.get_or_init(|| {
        std::env::var_os("MUGUET_SKIP_EMPTY_INIT_TRANSITION")
            .is_some_and(|value| value == "1")
    });
    let skip = enabled && empty;
    let sequence = ENTRIES.fetch_add(1, Ordering::Relaxed) + 1;
    if empty {
        EMPTY.fetch_add(1, Ordering::Relaxed);
    }
    // One matching update on both paths makes instrumentation symmetric.
    DECISIONS[usize::from(skip)].fetch_add(1, Ordering::Relaxed);
    let first_empty = empty && !FIRST_EMPTY.swap(true, Ordering::Relaxed);
    if (sequence == 1 || first_empty || sequence % REPORT_EVERY == 0)
        && REPORTS.fetch_add(1, Ordering::Relaxed) < MAX_REPORTS
    {
        std::eprintln!(
            "OHOS empty-init-transition processId={} enabled={enabled} entries={sequence} emptyEntries={} nonemptyEntries={} emptySkipDecisions={} reportEvery={REPORT_EVERY} maxReports={MAX_REPORTS}",
            std::process::id(),
            EMPTY.load(Ordering::Relaxed),
            sequence.saturating_sub(EMPTY.load(Ordering::Relaxed)),
            DECISIONS[1].load(Ordering::Relaxed),
        );
    }
    skip
}
