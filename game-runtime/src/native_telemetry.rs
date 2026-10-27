//! Bounded native observations. TIDs come from the current OS thread; source
//! samples mark successful present calls, never GPU completion or scanout.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role { Logic, Submission, EngineRender }
impl Role {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "game-main" => Some(Self::Logic), "gpu" => Some(Self::Submission),
            name if name.contains("RenderThread") => Some(Self::EngineRender), _ => None,
        }
    }
    fn index(self) -> usize { match self { Self::Logic => 0, Self::Submission => 1, Self::EngineRender => 2 } }
}
#[derive(Clone, Copy)]
struct Entry { tid: u32, token: u64 }
#[derive(Default)]
struct Registry { entries: [Option<Entry>; 3], next_token: u64 }
impl Registry {
    fn insert(&mut self, role: Role, tid: u32) -> Option<u64> {
        if tid == 0 { return None; }
        self.next_token = self.next_token.checked_add(1)?;
        self.entries[role.index()] = Some(Entry { tid, token: self.next_token });
        Some(self.next_token)
    }
    fn remove(&mut self, role: Role, token: u64) {
        let entry = &mut self.entries[role.index()];
        if entry.is_some_and(|entry| entry.token == token) { *entry = None; }
    }
    fn tid(&self, role: Role) -> Option<u32> { self.entries[role.index()].map(|entry| entry.tid) }
}
static THREADS: OnceLock<Mutex<Registry>> = OnceLock::new();
fn threads() -> &'static Mutex<Registry> { THREADS.get_or_init(|| Mutex::new(Registry::default())) }
struct Registration { role: Role, token: u64 }
impl Drop for Registration {
    fn drop(&mut self) {
        threads().lock().unwrap_or_else(|e| e.into_inner()).remove(self.role, self.token);
    }
}
thread_local! { static REGISTRATION: RefCell<Option<Registration>> = const { RefCell::new(None) }; }
fn current_tid() -> Option<u32> {
    #[cfg(target_env = "ohos")]
    { let tid = unsafe { libc::syscall(libc::SYS_gettid) }; u32::try_from(tid).ok().filter(|tid| *tid != 0) }
    #[cfg(not(target_env = "ohos"))]
    { None } // Never substitute pthread handles or invented host IDs.
}
pub(crate) fn register_current_thread(name: &str) {
    let Some(role) = Role::from_name(name) else { return; };
    let Some(tid) = current_tid() else { return; };
    register_observed_thread(role, tid);
}
fn register_observed_thread(role: Role, tid: u32) {
    REGISTRATION.with(|registration| {
        if registration.borrow().as_ref().is_some_and(|entry| entry.role == role) { return; }
        registration.borrow_mut().take();
        let token = threads().lock().unwrap_or_else(|e| e.into_inner()).insert(role, tid);
        *registration.borrow_mut() = token.map(|token| Registration { role, token });
    });
}

const MAX_INTERVALS: usize = 240;
const FRESH_NS: u64 = 5_000_000_000;
#[derive(Default)]
struct Sources {
    count: u64, last: Option<u64>, intervals: VecDeque<u64>,
    identity: Option<(u64, u64, u64)>, duplicate_sources: u64,
    target_fps: Option<f64>,
}
impl Sources {
    fn presented(&mut self, now: u64, identity: Option<(u64, u64, u64)>) {
        if let (Some((generation, epoch, sequence)), Some((old_generation, old_epoch, old_sequence))) = (identity, self.identity) {
            if generation == old_generation && epoch == old_epoch && sequence <= old_sequence {
                self.duplicate_sources = self.duplicate_sources.saturating_add(1); return;
            }
        }
        if identity.map(|(generation, epoch, _)| (generation, epoch)) != self.identity.map(|(generation, epoch, _)| (generation, epoch)) {
            self.last = None; self.intervals.clear();
        }
        self.identity = identity;
        self.count = self.count.saturating_add(1);
        if let Some(last) = self.last {
            let interval = now.saturating_sub(last);
            // Background gaps cannot become a misleading foreground FPS sample.
            if interval > FRESH_NS { self.intervals.clear(); }
            else if interval > 0 { self.intervals.push_back(interval); }
        }
        self.last = Some(now);
        while self.intervals.len() > MAX_INTERVALS { self.intervals.pop_front(); }
    }
    fn fps(&self, now: u64) -> Option<f64> {
        if self.last.is_none_or(|last| now < last || now - last > FRESH_NS)
            || self.intervals.is_empty() { return None; }
        let nanoseconds: u128 = self.intervals.iter().map(|n| *n as u128).sum();
        (nanoseconds > 0).then_some(self.intervals.len() as f64 * 1e9 / nanoseconds as f64)
    }
    fn percentile_ms(&self, now: u64, percentile: usize) -> Option<f64> {
        self.fps(now)?;
        let mut intervals: Vec<_> = self.intervals.iter().copied().collect(); intervals.sort_unstable();
        let index = (intervals.len() * percentile).div_ceil(100).saturating_sub(1).min(intervals.len() - 1);
        Some(intervals[index] as f64 / 1e6)
    }
}
static SOURCES: OnceLock<Mutex<Sources>> = OnceLock::new();
fn sources() -> &'static Mutex<Sources> { SOURCES.get_or_init(|| Mutex::new(Sources::default())) }
fn monotonic_ns() -> Option<u64> {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0
        || time.tv_sec < 0 || !(0..1_000_000_000).contains(&time.tv_nsec) { return None; }
    (time.tv_sec as u64).checked_mul(1_000_000_000)?.checked_add(time.tv_nsec as u64)
}
pub(crate) fn source_presented() {
    if let Some(now) = monotonic_ns() { sources().lock().unwrap_or_else(|e| e.into_inner()).presented(now, None); }
}
pub(crate) fn source_presented_key(generation: u64, epoch: u64, sequence: u64) {
    if let Some(now) = monotonic_ns() {
        sources().lock().unwrap_or_else(|e| e.into_inner()).presented(now, Some((generation, epoch, sequence)));
    }
}
pub(crate) fn reset_source_window() {
    let mut sources = sources().lock().unwrap_or_else(|e| e.into_inner());
    sources.last = None; sources.intervals.clear(); sources.identity = None;
}
pub(crate) fn set_source_target(target: Option<f64>) {
    let target = target.filter(|value| value.is_finite() && *value > 0.0 && *value <= 240.0);
    let mut sources = sources().lock().unwrap_or_else(|e| e.into_inner());
    if sources.target_fps != target {
        sources.last = None; sources.intervals.clear(); sources.identity = None;
    }
    sources.target_fps = target;
}
pub(crate) fn snapshot() -> serde_json::Value {
    let threads = threads().lock().unwrap_or_else(|e| e.into_inner());
    let sources = sources().lock().unwrap_or_else(|e| e.into_inner());
    let now = monotonic_ns();
    serde_json::json!({
        "processId": std::process::id(),
        "monotonicNs": now,
        "logicTid": threads.tid(Role::Logic), "submissionTid": threads.tid(Role::Submission),
        "engineRenderTid": threads.tid(Role::EngineRender),
        "sourcePresents": sources.count, "currentSourceFps": now.and_then(|now| sources.fps(now)),
        "sourceTargetFps": sources.target_fps,
        "sourceIntervalSamples": sources.intervals.len(),
        "sourceSampleAgeMs": now.zip(sources.last).filter(|(now,last)| now >= last).map(|(now,last)| (now-last) as f64 / 1e6),
        "sourceFrameTimeP50Ms": now.and_then(|now| sources.percentile_ms(now, 50)),
        "sourceFrameTimeP95Ms": now.and_then(|now| sources.percentile_ms(now, 95)),
        "sourceFrameTimeP99Ms": now.and_then(|now| sources.percentile_ms(now, 99)),
        "duplicateSourcePresentsExcluded": sources.duplicate_sources,
        "clock": "CLOCK_MONOTONIC_ns", "scope": "live OS TIDs; bounded deduplicated source present-call intervals; excludes AI and repeats; not GPU completion or display timing"
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replaced_thread_exit_cannot_erase_a_live_replacement() {
        let mut registry = Registry::default();
        let first = registry.insert(Role::Submission, 51).unwrap();
        let second = registry.insert(Role::Submission, 52).unwrap();
        registry.remove(Role::Submission, first);
        assert_eq!(registry.tid(Role::Submission), Some(52));
        registry.remove(Role::Submission, second);
        assert_eq!(registry.tid(Role::Submission), None);
        assert!(registry.insert(Role::Logic, 0).is_none());
    }
    #[test]
    fn roles_are_explicit_and_host_ids_are_never_fabricated() {
        assert_eq!(Role::from_name("gpu"), Some(Role::Submission));
        assert_eq!(Role::from_name("game-main"), Some(Role::Logic));
        assert_eq!(Role::from_name("[RAGE] RenderThread"), Some(Role::EngineRender));
        assert_eq!(Role::from_name("[RAGE Audio] Mixer"), None);
        #[cfg(not(target_env = "ohos"))]
        assert!(current_tid().is_none());
    }
    #[test]
    fn exiting_thread_retires_its_registration() {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (exit_tx, exit_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            register_observed_thread(Role::EngineRender, 921);
            ready_tx.send(()).unwrap(); exit_rx.recv().unwrap();
        });
        ready_rx.recv().unwrap();
        assert_eq!(threads().lock().unwrap().tid(Role::EngineRender), Some(921));
        exit_tx.send(()).unwrap(); worker.join().unwrap();
        assert_eq!(threads().lock().unwrap().tid(Role::EngineRender), None);
    }
    #[test]
    fn intervals_are_bounded_and_stale_or_background_windows_are_unknown() {
        let mut sources = Sources::default(); let start = 1_000_000_000;
        assert!(sources.fps(start).is_none());
        for n in 0..400 { sources.presented(start + n * 50_000_000, None); }
        let end = start + 399 * 50_000_000;
        assert_eq!(sources.intervals.len(), MAX_INTERVALS);
        assert!((sources.fps(end).unwrap() - 20.0).abs() < 0.00001);
        assert_eq!(sources.percentile_ms(end, 50), Some(50.0));
        assert_eq!(sources.percentile_ms(end, 95), Some(50.0));
        assert_eq!(sources.percentile_ms(end, 99), Some(50.0));
        assert!(sources.fps(end + 6_000_000_000).is_none());
        assert!(sources.percentile_ms(end + 6_000_000_000, 99).is_none());
        sources.presented(end + 7_000_000_000, None);
        assert!(sources.intervals.is_empty());
        assert_eq!(sources.count, 401);
    }
    #[test]
    fn duplicate_and_old_sequences_are_excluded_and_new_generations_start_a_fresh_window() {
        let mut sources = Sources::default();
        sources.presented(1_000_000_000, Some((1, 1, 100)));
        sources.presented(1_050_000_000, Some((1, 1, 101)));
        sources.presented(1_075_000_000, Some((1, 1, 101)));
        sources.presented(1_100_000_000, Some((1, 1, 99)));
        assert_eq!(sources.count, 2); assert_eq!(sources.duplicate_sources, 2);
        assert_eq!(sources.fps(1_100_000_000), Some(20.0));
        sources.presented(1_150_000_000, Some((2, 1, 1)));
        assert_eq!(sources.count, 3); assert!(sources.fps(1_150_000_000).is_none());
        sources.presented(1_200_000_000, Some((2, 1, 2)));
        assert_eq!(sources.fps(1_200_000_000), Some(20.0));
    }
}
