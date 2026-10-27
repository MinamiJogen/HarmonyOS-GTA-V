//! Low frequency diagnostics owned by game-main; status readers never sample or write.
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub(crate) const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
const EXPORT_INTERVAL: Duration = Duration::from_secs(1);
const STALE_AFTER: Duration = Duration::from_secs(15);

#[derive(Clone, Copy)]
pub(crate) struct Mode { pub foreground: bool, pub generation: u64, pub terminal: bool }

#[derive(Default)]
struct Cache {
    values: Value,
    sampled_at: Option<Instant>,
    sample_duration_ms: u64,
    sample_duration_ns: u64,
    sequence: u64,
    sample_generation: Option<u64>,
    sample_failures: u64,
    sample_error: String,
    last_sample_error: String,
    export_failures: u64,
    last_export_error: String,
    export_duration_ms: u64,
    export_duration_ns: u64,
    running: bool,
    paused: bool,
}
impl Cache {
    fn snapshot(&self, now: Instant) -> Value {
        self.snapshot_for_mode(now, None)
    }
    fn snapshot_for_mode(&self, now: Instant, mode: Option<Mode>) -> Value {
        let age = self.sampled_at.map(|at| millis(now.saturating_duration_since(at)));
        let paused = mode.map(|mode| !mode.foreground || mode.terminal).unwrap_or(self.paused);
        let generation_matches = mode.is_none_or(|mode| self.sample_generation == Some(mode.generation));
        json!({"values":self.values,"workerRunning":self.running,"samplingPaused":paused,
            "sampleSequence":self.sequence,"cacheAgeMs":age,"sampleDurationMs":self.sample_duration_ms,
            "sampleDurationNs":self.sample_duration_ns,
            "sampleGeneration":self.sample_generation,"currentGeneration":mode.map(|mode|mode.generation),
            "sampleMatchesCurrentGeneration":generation_matches,
            "fresh":self.running && !paused && generation_matches && self.sample_error.is_empty() && age.is_some_and(|ms| ms <= millis(STALE_AFTER)),
            "sampleIntervalMs":millis(SAMPLE_INTERVAL),"staleAfterMs":millis(STALE_AFTER),
            "sampleFailures":self.sample_failures,"sampleError":self.sample_error,"lastSampleError":self.last_sample_error,
            "exportFailures":self.export_failures,"lastExportError":self.last_export_error,
            "lastExportDurationMs":self.export_duration_ms,"lastExportDurationNs":self.export_duration_ns,"exportFile":"game-native-status.json",
            "clock":"std::time::Instant; age and duration only"})
    }
    fn sampled(&mut self, result: Result<Value, String>, at: Instant, duration: Duration, generation: u64) {
        self.sample_duration_ms = millis(duration);
        self.sample_duration_ns = duration.as_nanos().min(u64::MAX as u128) as u64;
        match result {
            Ok(values) => { self.values = values; self.sampled_at = Some(at); self.sample_generation = Some(generation); self.sequence += 1; self.sample_error.clear(); }
            Err(error) => { self.sample_failures += 1; self.sample_error = error.clone(); self.last_sample_error = error; }
        }
    }
}
fn millis(duration: Duration) -> u64 { duration.as_millis().min(u64::MAX as u128) as u64 }

#[derive(Default)]
struct Control { stop: bool, revision: u64 }
#[derive(Default)]
struct Shared { cache: Mutex<Cache>, control: Mutex<Control>, wake: Condvar }
impl Shared {
    fn notify(&self, stop: bool) {
        let mut control = self.control.lock().unwrap();
        control.stop |= stop;
        control.revision = control.revision.wrapping_add(1);
        self.wake.notify_one();
    }
    fn snapshot(&self) -> Value { self.cache.lock().unwrap().snapshot(Instant::now()) }
}
static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

pub(crate) fn snapshot(mode: Mode) -> Value {
    // Lifecycle comes from the status reader itself: a worker blocked in a
    // sample/write cannot make old-generation diagnostics appear fresh.
    SHARED.get().map(|shared| shared.cache.lock().unwrap().snapshot_for_mode(Instant::now(), Some(mode)))
        .unwrap_or_else(|| Cache::default().snapshot_for_mode(Instant::now(), Some(mode)))
}
/// Wakes an existing worker only. UI notifications never create a thread.
pub(crate) fn notify() { if let Some(shared) = SHARED.get() { shared.notify(false); } }

pub(crate) struct Worker { shared: Arc<Shared>, thread: Option<JoinHandle<()>> }
impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.notify(true);
        // This guard lives on game-main, never on the ArkUI/input caller.
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
}
pub(crate) fn start(
    root: PathBuf,
    sample: impl FnMut() -> Result<Value, String> + Send + 'static,
    mode: impl Fn() -> Mode + Send + 'static,
    poll: impl FnMut() + Send + 'static,
    status: impl Fn() -> Value + Send + 'static,
) -> std::io::Result<Worker> {
    let shared = SHARED.get_or_init(|| Arc::new(Shared::default())).clone();
    spawn(shared, SAMPLE_INTERVAL, EXPORT_INTERVAL, sample, mode, poll, move || publish(&root, &status()))
}
fn spawn(
    shared: Arc<Shared>, sample_interval: Duration, export_interval: Duration,
    mut sample: impl FnMut() -> Result<Value, String> + Send + 'static,
    mode: impl Fn() -> Mode + Send + 'static,
    mut poll: impl FnMut() + Send + 'static,
    mut export: impl FnMut() -> std::io::Result<()> + Send + 'static,
) -> std::io::Result<Worker> {
    {
        let mut cache = shared.cache.lock().unwrap();
        if cache.running { return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "status sampler already running")); }
        cache.running = true;
        shared.control.lock().unwrap().stop = false;
    }
    let worker_shared = shared.clone();
    let thread = std::thread::Builder::new().name("game-status".into()).spawn(move || {
        let shared = worker_shared;
        let mut next_sample = Instant::now();
        let mut sample_generation = None;
        loop {
            let (stop, revision) = { let control = shared.control.lock().unwrap(); (control.stop, control.revision) };
            let mode = mode();
            let terminal = stop || mode.terminal;
            {
                let mut cache = shared.cache.lock().unwrap();
                cache.paused = !mode.foreground || terminal;
                if terminal { cache.running = false; }
            }
            // Save timeout handling can touch files too; it belongs here rather than in gameStatus.
            poll();
            if !terminal && mode.foreground && (Instant::now() >= next_sample || sample_generation != Some(mode.generation)) {
                let began = Instant::now();
                let result = sample();
                let completed = Instant::now();
                shared.cache.lock().unwrap().sampled(result, completed, completed.duration_since(began), mode.generation);
                sample_generation = Some(mode.generation);
                next_sample = completed + sample_interval;
            }
            let began = Instant::now();
            let exported = export();
            {
                let mut cache = shared.cache.lock().unwrap();
                let elapsed = began.elapsed();
                cache.export_duration_ms = millis(elapsed);
                cache.export_duration_ns = elapsed.as_nanos().min(u64::MAX as u128) as u64;
                if let Err(error) = exported { cache.export_failures += 1; cache.last_export_error = error.to_string(); }
            }
            if terminal { break; }
            let control = shared.control.lock().unwrap();
            // Revision predicate prevents losing a lifecycle/stop notification during a sample or disk write.
            let _ = shared.wake.wait_timeout_while(control, export_interval, |control| !control.stop && control.revision == revision).unwrap();
        }
    });
    match thread {
        Ok(thread) => Ok(Worker { shared, thread: Some(thread) }),
        Err(error) => {
            let mut cache = shared.cache.lock().unwrap();
            cache.running = false;
            cache.sample_error = format!("status sampler start: {error}");
            cache.last_sample_error = cache.sample_error.clone();
            Err(error)
        }
    }
}
fn publish(root: &Path, value: &Value) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let temporary = root.join(".game-native-status.json.tmp");
    let result = (|| {
        let mut file = std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(&temporary)?;
        serde_json::to_writer(&mut file, value).map_err(std::io::Error::other)?;
        file.write_all(b"\n")?;
        file.flush()?;
        std::fs::rename(&temporary, root.join("game-native-status.json"))
    })();
    if result.is_err() { let _ = std::fs::remove_file(temporary); }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::{AtomicBool, AtomicU64, Ordering}, mpsc};
    fn wait(condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !condition() { assert!(Instant::now() < deadline, "worker progress timeout"); std::thread::sleep(Duration::from_millis(1)); }
    }
    #[test]
    fn freshness_and_sampling_failure_keep_last_good_values_and_error_history() {
        let start = Instant::now();
        let mut cache = Cache { running: true, ..Cache::default() };
        assert_eq!(cache.snapshot(start)["fresh"], false);
        cache.sampled(Ok(json!({"heapResidentMB":44})), start, Duration::from_millis(27), 0);
        assert_eq!(cache.snapshot(start)["sampleDurationMs"], 27);
        assert_eq!(cache.snapshot(start + STALE_AFTER)["fresh"], true);
        assert_eq!(cache.snapshot(start + STALE_AFTER + Duration::from_millis(1))["fresh"], false);
        cache.sampled(Err("mincore denied".into()), start + Duration::from_secs(1), Duration::ZERO, 0);
        let status = cache.snapshot(start + Duration::from_secs(2));
        assert_eq!(status["values"]["heapResidentMB"], 44);
        assert_eq!(status["cacheAgeMs"], 2000);
        assert_eq!(status["fresh"], false);
        cache.sampled(Ok(json!({"heapResidentMB":45})), start + Duration::from_secs(3), Duration::ZERO, 0);
        let status = cache.snapshot(start + Duration::from_secs(3));
        assert_eq!(status["fresh"], true); assert_eq!(status["sampleError"], "");
        assert_eq!(status["lastSampleError"], "mincore denied"); assert_eq!(status["sampleFailures"], 1);
        cache.paused = true; assert_eq!(cache.snapshot(start + Duration::from_secs(3))["fresh"], false);
        cache.paused = false; cache.running = false; assert_eq!(cache.snapshot(start + Duration::from_secs(3))["fresh"], false);
    }
    #[test]
    fn live_lifecycle_masks_cached_freshness_before_the_worker_can_observe_a_transition() {
        let now = Instant::now();
        let mut cache = Cache { running: true, ..Cache::default() };
        cache.sampled(Ok(json!({"heapResidentMB":44})), now, Duration::ZERO, 0);
        let mode = |foreground, generation, terminal| Some(Mode { foreground, generation, terminal });
        assert_eq!(cache.snapshot_for_mode(now, mode(true,0,false))["fresh"], true);
        let paused = cache.snapshot_for_mode(now, mode(false,1,false));
        assert_eq!(paused["samplingPaused"], true); assert_eq!(paused["fresh"], false);
        // A quick background/foreground roundtrip cannot refresh old measurements.
        let resumed = cache.snapshot_for_mode(now, mode(true,2,false));
        assert_eq!(resumed["samplingPaused"], false); assert_eq!(resumed["fresh"], false);
        assert_eq!(resumed["sampleGeneration"], 0); assert_eq!(resumed["currentGeneration"], 2);
        assert_eq!(resumed["values"]["heapResidentMB"], 44);
        cache.sampled(Ok(json!({"heapResidentMB":45})), now, Duration::ZERO, 2);
        assert_eq!(cache.snapshot_for_mode(now, mode(true,2,false))["fresh"], true);
        assert_eq!(cache.snapshot_for_mode(now, mode(true,2,true))["fresh"], false);
    }
    #[test]
    fn slow_sample_does_not_block_readers_and_drop_joins_with_a_final_export() {
        let shared = Arc::new(Shared::default());
        let (entered_tx, entered_rx) = mpsc::channel(); let (release_tx, release_rx) = mpsc::channel();
        let exports = Arc::new(AtomicU64::new(0)); let exported = exports.clone();
        let mut first = true;
        let worker = spawn(shared.clone(), Duration::from_secs(60), Duration::from_secs(60), move || {
            if first { first = false; entered_tx.send(()).unwrap(); release_rx.recv().unwrap(); }
            Ok(json!({"heapResidentMB":77}))
        }, || Mode { foreground: true, generation: 0, terminal: false }, || {}, move || { exported.fetch_add(1,Ordering::Relaxed); Ok(()) }).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let reader = shared.clone(); let (read_tx, read_rx) = mpsc::channel();
        let reader = std::thread::spawn(move || read_tx.send(reader.snapshot()).unwrap());
        assert_eq!(read_rx.recv_timeout(Duration::from_secs(1)).expect("cache read waited for heavy sampler")["sampleSequence"], 0);
        reader.join().unwrap(); release_tx.send(()).unwrap();
        wait(|| shared.snapshot()["sampleSequence"] == 1);
        drop(worker);
        assert_eq!(shared.snapshot()["workerRunning"], false);
        assert!(exports.load(Ordering::Relaxed) >= 2, "shutdown must export final cached state");
    }
    #[test]
    fn background_pauses_heavy_sampling_and_foreground_generation_forces_refresh() {
        let shared = Arc::new(Shared::default()); let foreground = Arc::new(AtomicBool::new(false));
        let generation = Arc::new(AtomicU64::new(1)); let mode_generation = generation.clone();
        let mode_foreground = foreground.clone(); let samples = Arc::new(AtomicU64::new(0)); let sampled = samples.clone();
        let worker = spawn(shared.clone(), Duration::from_secs(60), Duration::from_millis(10), move || {
            Ok(json!({"count":sampled.fetch_add(1, Ordering::Relaxed) + 1}))
        }, move || Mode { foreground:mode_foreground.load(Ordering::Acquire), generation:mode_generation.load(Ordering::Acquire), terminal:false }, || {}, || Ok(())).unwrap();
        wait(|| shared.snapshot()["samplingPaused"] == true);
        assert_eq!(samples.load(Ordering::Relaxed), 0);
        foreground.store(true, Ordering::Release); shared.notify(false);
        wait(|| samples.load(Ordering::Relaxed) == 1);
        foreground.store(false, Ordering::Release); shared.notify(false);
        wait(|| shared.snapshot()["samplingPaused"] == true);
        assert_eq!(samples.load(Ordering::Relaxed), 1);
        generation.store(3, Ordering::Release); foreground.store(true, Ordering::Release); shared.notify(false);
        wait(|| samples.load(Ordering::Relaxed) == 2);
        drop(worker); assert_eq!(shared.snapshot()["workerRunning"], false);
    }
    #[test]
    fn terminal_mode_wakes_exits_and_preserves_export_errors() {
        let shared = Arc::new(Shared::default()); let terminal = Arc::new(AtomicBool::new(false)); let mode_terminal = terminal.clone();
        let worker = spawn(shared.clone(), Duration::from_secs(60), Duration::from_secs(60), || Ok(json!({"resident":5})),
            move || Mode { foreground:true, generation:0, terminal:mode_terminal.load(Ordering::Acquire) }, || {},
            || Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "disk denied"))).unwrap();
        wait(|| shared.snapshot()["exportFailures"] == 1);
        terminal.store(true, Ordering::Release); shared.notify(false);
        wait(|| shared.snapshot()["workerRunning"] == false);
        drop(worker);
        let status = shared.snapshot();
        assert_eq!(status["values"]["resident"], 5); assert_eq!(status["exportFailures"], 2);
        assert_eq!(status["lastExportError"], "disk denied");
    }
    #[test]
    fn export_is_atomic_private_and_failure_retains_the_previous_file() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("game-native-status-test-{}",std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        publish(&root, &json!({"error":"original error","n":1})).unwrap();
        assert_eq!(std::fs::metadata(root.join("game-native-status.json")).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::create_dir(root.join(".game-native-status.json.tmp")).unwrap();
        assert!(publish(&root, &json!({"n":2})).is_err());
        let retained: Value = serde_json::from_slice(&std::fs::read(root.join("game-native-status.json")).unwrap()).unwrap();
        assert_eq!(retained["error"], "original error"); assert_eq!(retained["n"], 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}
