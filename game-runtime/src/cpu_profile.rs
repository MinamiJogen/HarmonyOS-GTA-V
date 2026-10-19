//! Opt-in CPU clocks and bounded wall-span aggregates; no GPU work or trace files.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static ENABLED: AtomicBool = AtomicBool::new(false);
const MAX_THREADS: usize = 128;
const N: usize = 20;

#[derive(Clone, Copy)]
#[repr(usize)]
pub(crate) enum Metric {
    Commands, DrawSample, ScopeCreate, ScopePop, Submit, Acquire, Present,
    UboWait, Op8Wait, MapPoll, LoopPoll, PipelineCreate, RingWait, PresentTotal, EncoderFinish,
    StageBindGroup, BindingResolve, PackCopy, BindGroupCreate, BindingProbe,
}
const NAMES: [&str; N] = ["commands", "drawSample64", "scopedCreate", "scopedPop", "queueSubmit",
    "surfaceAcquire", "queuePresent", "uboWait", "guestOp8Wait", "mapPoll", "loopPoll", "pipelineCreate", "ringWait", "presentTotal", "encoderFinish",
    "stageBindGroupSample64", "bindingResolveSample", "packCopySample", "bindGroupCreate", "bindingProbeOverheadSample"];
#[derive(Clone, Copy)]
#[repr(usize)]
enum Phase { LoopOther, CommandsOther, RingWait, PresentOther, Submit, Acquire, PresentCall, UboWait, Op8Wait, Poll, ResourceCreate, ScopePop, PipelineCreate, EncoderFinish }
const PHASE_NAMES: [&str; 14] = ["loopOther", "commandsOther", "ringWait", "presentOther", "submit", "acquire", "presentCall", "uboWait", "op8Wait", "poll", "resourceCreate", "scopePop", "pipelineCreate", "encoderFinish"];
static EXCLUSIVE: [AtomicU64; 14] = [const { AtomicU64::new(0) }; 14];
struct PhaseState { phase: Phase, since: Instant }
std::thread_local! { static GPU_PHASE: std::cell::RefCell<Option<PhaseState>> = const { std::cell::RefCell::new(None) }; }
fn change_phase(next: Option<Phase>, now: Instant) -> Option<Phase> {
    GPU_PHASE.with(|state| {
        let mut state = state.borrow_mut(); let state = state.as_mut()?;
        let old = state.phase;
        EXCLUSIVE[old as usize].fetch_add(now.duration_since(state.since).as_nanos().min(u64::MAX as u128) as u64, Ordering::Relaxed);
        state.since = now;
        if let Some(next) = next { state.phase = next; }
        Some(old)
    })
}
impl Metric {
    fn phase(self) -> Option<Phase> { Some(match self {
        Self::DrawSample | Self::StageBindGroup | Self::BindingResolve | Self::PackCopy | Self::BindGroupCreate | Self::BindingProbe => return None,
        Self::Commands => Phase::CommandsOther,
        Self::ScopeCreate => Phase::ResourceCreate, Self::ScopePop => Phase::ScopePop,
        Self::Submit => Phase::Submit, Self::Acquire => Phase::Acquire, Self::Present => Phase::PresentCall,
        Self::UboWait => Phase::UboWait, Self::Op8Wait => Phase::Op8Wait,
        Self::MapPoll | Self::LoopPoll => Phase::Poll, Self::PipelineCreate => Phase::PipelineCreate,
        Self::RingWait => Phase::RingWait, Self::PresentTotal => Phase::PresentOther,
        Self::EncoderFinish => Phase::EncoderFinish,
    }) }
}
struct Stat { calls: AtomicU64, samples: AtomicU64, ns: AtomicU64, max_ns: AtomicU64 }
impl Stat {
    const fn new() -> Self { Self { calls: AtomicU64::new(0), samples: AtomicU64::new(0), ns: AtomicU64::new(0), max_ns: AtomicU64::new(0) } }
    fn record(&self, ns: u64) {
        self.samples.fetch_add(1, Ordering::Relaxed);
        self.ns.fetch_add(ns, Ordering::Relaxed);
        self.max_ns.fetch_max(ns, Ordering::Relaxed);
    }
}
static STATS: [Stat; N] = [const { Stat::new() }; N];
pub(crate) struct Span(Option<(usize, Instant, Option<Phase>)>);
impl Span { pub(crate) fn is_sampled(&self) -> bool { self.0.is_some() } }
impl Drop for Span {
    fn drop(&mut self) {
        if let Some((i, start, old)) = self.0 {
            let now = Instant::now();
            if old.is_some() { change_phase(old, now); }
            STATS[i].record(now.duration_since(start).as_nanos().min(u64::MAX as u128) as u64);
        }
    }
}
#[inline]
pub(crate) fn span(metric: Metric) -> Span { sampled(metric, 1) }
#[inline]
pub(crate) fn span_if(metric: Metric, active: bool) -> Span { if active { span(metric) } else { Span(None) } }
#[inline]
pub(crate) fn sampled(metric: Metric, every: u64) -> Span {
    if !ENABLED.load(Ordering::Relaxed) { return Span(None); }
    let i = metric as usize;
    let n = STATS[i].calls.fetch_add(1, Ordering::Relaxed);
    Span((n % every == 0).then(|| { let now=Instant::now(); let old=metric.phase().and_then(|phase|change_phase(Some(phase),now)); (i,now,old) }))
}

pub(crate) fn enabled() -> bool { ENABLED.load(Ordering::Relaxed) }

// Observes conservative *sampled* packing input reuse only. No packed bytes or
// GPU objects are cached, and this state cannot select any rendering fast path.
const MAX_BINDING_SIGNATURES: usize = 256;
pub(crate) const MAX_BINDING_SIGNATURE_WORDS: usize = 256;
static RESOURCE_GENERATION: AtomicU64 = AtomicU64::new(0);
#[derive(Default)]
struct BindingObservations {
    generation: u64,
    keys: std::collections::HashSet<Vec<u64>>,
    hits: u64, misses: u64, pack_samples: u64, pack_bytes: u64,
    reused_samples: u64, reused_bytes: u64, invalidations: u64, capacity_resets: u64, oversized: u64,
}
std::thread_local! { static BINDING_OBSERVATIONS: std::cell::RefCell<BindingObservations> = std::cell::RefCell::new(BindingObservations::default()); }
#[inline]
pub(crate) fn invalidate_binding_observations() {
    if enabled() { RESOURCE_GENERATION.fetch_add(1, Ordering::Relaxed); }
}
#[inline]
pub(crate) fn binding_cache_lookup(hit: bool) {
    if !enabled() { return; }
    BINDING_OBSERVATIONS.with(|state| {
        let mut state = state.borrow_mut();
        if hit { state.hits += 1; } else { state.misses += 1; }
    });
}
pub(crate) fn observe_pack_signature(key: Vec<u64>, bytes: u64) {
    if !enabled() { return; }
    BINDING_OBSERVATIONS.with(|state| {
        let mut state = state.borrow_mut();
        if key.len() > MAX_BINDING_SIGNATURE_WORDS { state.oversized += 1; return; }
        let generation = RESOURCE_GENERATION.load(Ordering::Relaxed);
        if state.generation != generation {
            state.invalidations += generation.wrapping_sub(state.generation);
            state.keys.clear();
            state.generation = generation;
        }
        state.pack_samples += 1; state.pack_bytes += bytes;
        if state.keys.contains(&key) {
            state.reused_samples += 1; state.reused_bytes += bytes;
        } else {
            if state.keys.len() >= MAX_BINDING_SIGNATURES { state.keys.clear(); state.capacity_resets += 1; }
            state.keys.insert(key);
        }
    });
}
pub(crate) fn skip_oversized_pack_signature() {
    if enabled() { BINDING_OBSERVATIONS.with(|state| state.borrow_mut().oversized += 1); }
}
fn binding_report() -> serde_json::Value {
    BINDING_OBSERVATIONS.with(|state| {
        let mut state = state.borrow_mut();
        let report = serde_json::json!({
            "cacheHits":state.hits,"cacheMisses":state.misses,
            "sampledPackCalls":state.pack_samples,"sampledPackBytes":state.pack_bytes,
            "sampledMetadataReuseCalls":state.reused_samples,"sampledMetadataReuseBytes":state.reused_bytes,
            "observationObjectInvalidations":state.invalidations,"observationCapacityResets":state.capacity_resets,
            "observationKeys":state.keys.len(),"maximumObservationKeys":MAX_BINDING_SIGNATURES,
            "maximumSignatureWords":MAX_BINDING_SIGNATURE_WORDS,"oversizedSignaturesSkipped":state.oversized,
            "scope":"cache lookups counted exactly; pack reuse is conservative metadata identity observed only inside every64th stage sample, with any object set/destroy invalidating all observed keys; no production cache or byte reuse"
        });
        state.hits=0;state.misses=0;state.pack_samples=0;state.pack_bytes=0;
        state.reused_samples=0;state.reused_bytes=0;state.invalidations=0;state.capacity_resets=0;state.oversized=0;
        report
    })
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Role { GuestMain, GuestWorker, Gpu }
impl Role { fn name(self) -> &'static str { match self { Self::GuestMain => "guestMain", Self::GuestWorker => "guestWorker", Self::Gpu => "gpu" } } }
struct Thread {
    role: Role, name: String, handle: usize, clock: Option<libc::clockid_t>,
    last: Option<u64>, final_cpu: Option<u64>, active: bool,
}
static THREADS: OnceLock<Mutex<Vec<Thread>>> = OnceLock::new();
static DROPPED_THREADS: AtomicU64 = AtomicU64::new(0);
fn threads() -> &'static Mutex<Vec<Thread>> { THREADS.get_or_init(|| Mutex::new(Vec::new())) }

#[cfg(target_env = "ohos")]
unsafe extern "C" {
    fn pthread_getcpuclockid(thread: libc::pthread_t, clock: *mut libc::clockid_t) -> libc::c_int;
    fn pthread_getname_np(thread: libc::pthread_t, name: *mut libc::c_char, size: usize) -> libc::c_int;
}
fn clock_ns(clock: libc::clockid_t) -> Option<u64> {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(clock, &mut time) } != 0 || time.tv_sec < 0 || !(0..1_000_000_000).contains(&time.tv_nsec) { return None; }
    Some((time.tv_sec as u64).saturating_mul(1_000_000_000).saturating_add(time.tv_nsec as u64))
}
fn thread_clock() -> Option<libc::clockid_t> {
    #[cfg(target_env = "ohos")]
    { let mut clock = 0; (unsafe { pthread_getcpuclockid(libc::pthread_self(), &mut clock) } == 0).then_some(clock) }
    #[cfg(not(target_env = "ohos"))]
    { None }
}
fn refresh_name(thread: &mut Thread) {
    #[cfg(target_env = "ohos")]
    if thread.active {
        let mut name = [0u8; 64];
        if unsafe { pthread_getname_np(thread.handle as libc::pthread_t, name.as_mut_ptr().cast(), name.len()) } == 0 {
            let len = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            if len != 0 { thread.name = String::from_utf8_lossy(&name[..len]).into_owned(); }
        }
    }
    #[cfg(not(target_env = "ohos"))]
    { let _ = thread; }
}
pub(crate) struct ThreadGuard(Option<usize>);
pub(crate) fn register_thread(role: Role) -> ThreadGuard {
    if !ENABLED.load(Ordering::Relaxed) { return ThreadGuard(None); }
    let clock = thread_clock();
    let last = clock.and_then(clock_ns);
    let mut entries = threads().lock().unwrap();
    if entries.len() >= MAX_THREADS { DROPPED_THREADS.fetch_add(1, Ordering::Relaxed); return ThreadGuard(None); }
    let index = entries.len();
    entries.push(Thread { role, name: std::thread::current().name().unwrap_or("unnamed").chars().take(48).collect(),
        handle: unsafe { libc::pthread_self() } as usize, clock, last, final_cpu: None, active: true });
    ThreadGuard(Some(index))
}
impl Drop for ThreadGuard {
    fn drop(&mut self) {
        if let Some(i) = self.0 {
            if let Some(thread) = threads().lock().unwrap().get_mut(i) {
                refresh_name(thread);
                thread.final_cpu = thread.clock.and_then(clock_ns).or(thread.last);
                thread.active = false;
            }
        }
    }
}
struct Window { start: Instant, process_cpu: Option<u64>, frames: u64, draws: u64, submits: u64, passes: u64, host_calls: u64 }
static WINDOW: OnceLock<Mutex<Option<Window>>> = OnceLock::new();
pub(crate) fn configure(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
    if enabled {
        for stat in &STATS { stat.calls.store(0, Ordering::Relaxed); stat.samples.store(0, Ordering::Relaxed); stat.ns.store(0, Ordering::Relaxed); stat.max_ns.store(0, Ordering::Relaxed); }
        *WINDOW.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(Window { start: Instant::now(), process_cpu: clock_ns(libc::CLOCK_PROCESS_CPUTIME_ID), frames: 0, draws: 0, submits: 0, passes: 0, host_calls: 0 });
    } else if let Some(window) = WINDOW.get() { *window.lock().unwrap() = None; }
}
/// Align the first reporting interval and all exclusive buckets to GPU run.
pub(crate) fn begin_gpu_window() {
    if !enabled() { return; }
    for thread in threads().lock().unwrap().iter_mut() {
        thread.last = if thread.active { thread.clock.and_then(clock_ns) } else { thread.final_cpu };
    }
    let now = Instant::now();
    for stat in &STATS { stat.calls.store(0,Ordering::Relaxed);stat.samples.store(0,Ordering::Relaxed);stat.ns.store(0,Ordering::Relaxed);stat.max_ns.store(0,Ordering::Relaxed); }
    for value in &EXCLUSIVE {value.store(0,Ordering::Relaxed);}
    BINDING_OBSERVATIONS.with(|state| *state.borrow_mut() = BindingObservations { generation: RESOURCE_GENERATION.load(Ordering::Relaxed), ..Default::default() });
    GPU_PHASE.with(|state|*state.borrow_mut()=Some(PhaseState{phase:Phase::LoopOther,since:now}));
    *WINDOW.get_or_init(||Mutex::new(None)).lock().unwrap()=Some(Window{start:now,process_cpu:clock_ns(libc::CLOCK_PROCESS_CPUTIME_ID),frames:0,draws:0,submits:0,passes:0,host_calls:0});
}
fn ms(ns: u64) -> f64 { ns as f64 / 1_000_000.0 }
pub(crate) fn maybe_report(frames: u64, draws: u64, submits: u64, passes: u64, host_calls: u64) {
    if !ENABLED.load(Ordering::Relaxed) { return; }
    let mut guard = WINDOW.get_or_init(|| Mutex::new(None)).lock().unwrap();
    let Some(window) = guard.as_mut() else { return; };
    let report_at = Instant::now();
    let wall = report_at.duration_since(window.start);
    if wall.as_secs() < 10 { return; }
    change_phase(None, report_at);
    let now_cpu = clock_ns(libc::CLOCK_PROCESS_CPUTIME_ID);
    let process = now_cpu.zip(window.process_cpu).map(|(a,b)| a.saturating_sub(b));
    let mut role_ns = [0u64; 3];
    let mut unavailable = 0; let mut top = Vec::new();
    for thread in threads().lock().unwrap().iter_mut() {
        refresh_name(thread);
        let cpu = if thread.active { thread.clock.and_then(clock_ns) } else { thread.final_cpu };
        match cpu.zip(thread.last) {
            Some((cpu, last)) => {
                let delta = cpu.saturating_sub(last); role_ns[thread.role as usize] += delta; thread.last = Some(cpu);
                if thread.role != Role::Gpu { top.push((delta, thread.role.name(), thread.name.clone())); }
            }
            None => { if thread.active { unavailable += 1; } }
        }
    }
    top.sort_unstable_by(|a,b| b.0.cmp(&a.0)); top.truncate(8);
    let top: Vec<_> = top.into_iter().map(|(ns,role,name)| serde_json::json!({"name":name,"role":role,"cpuMs":ms(ns)})).collect();
    let mut spans = serde_json::Map::new();
    for (i, stat) in STATS.iter().enumerate() {
        let calls = stat.calls.swap(0, Ordering::Relaxed); let samples = stat.samples.swap(0, Ordering::Relaxed);
        let ns = stat.ns.swap(0, Ordering::Relaxed); let max = stat.max_ns.swap(0, Ordering::Relaxed);
        spans.insert(NAMES[i].into(), serde_json::json!({"calls":calls,"samples":samples,"totalMs":ms(ns),"maxMs":ms(max),"sampleEvery":if i==Metric::DrawSample as usize || i==Metric::StageBindGroup as usize {64} else {1},"conditionalOnStageSample":matches!(i, x if x==Metric::BindingResolve as usize || x==Metric::PackCopy as usize || x==Metric::BindingProbe as usize)}));
    }
    let tracked = role_ns.iter().sum::<u64>();
    let mut exclusive = serde_json::Map::new(); let mut exclusive_ns=0u64;
    for (i,value) in EXCLUSIVE.iter().enumerate() {let ns=value.swap(0,Ordering::Relaxed);exclusive_ns+=ns;exclusive.insert(PHASE_NAMES[i].into(),serde_json::json!(ms(ns)));}
    let wall_ns=wall.as_nanos().min(u64::MAX as u128)as u64;
    let record = serde_json::json!({"windowMs":wall.as_secs_f64()*1000.0,"frames":frames.saturating_sub(window.frames),"draws":draws.saturating_sub(window.draws),"submits":submits.saturating_sub(window.submits),"passes":passes.saturating_sub(window.passes),"hostCalls":host_calls.saturating_sub(window.host_calls),"processCpuMs":process.map(ms),"trackedGuestMainCpuMs":ms(role_ns[0]),"trackedGuestWorkersCpuMs":ms(role_ns[1]),"trackedGpuCpuMs":ms(role_ns[2]),"otherProcessCpuMs":process.map(|p|ms(p.saturating_sub(tracked))),"unavailableThreadClocks":unavailable,"droppedThreadRegistrations":DROPPED_THREADS.load(Ordering::Relaxed),"guestTop":top,"wallSpans":spans,"bindingProfile":binding_report(),"gpuExclusiveWallMs":exclusive,"gpuExclusiveSumMs":ms(exclusive_ns),"unaccountedGpuWallMs":ms(wall_ns.saturating_sub(exclusive_ns)),"scope":"guest CPU includes native host imports; wallSpans are nested, not additive and can cross window boundaries; draw/stageSample64 totals are samples only; bindingResolveSample includes packCopySample and bindingProbeOverheadSample; exclusive GPU wall buckets sum to interval"});
    eprintln!("[cpu-profile] {record}");
    *window = Window { start: report_at, process_cpu: now_cpu, frames, draws, submits, passes, host_calls };
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn default_off_then_enabled_aggregation_and_reset() {
        configure(false);
        { let _span = span(Metric::Submit); }
        assert_eq!(STATS[Metric::Submit as usize].calls.load(Ordering::Relaxed), 0);
        configure(true);
        for _ in 0..130 { let _span = sampled(Metric::DrawSample, 64); }
        let stat = &STATS[Metric::DrawSample as usize];
        assert_eq!(stat.calls.load(Ordering::Relaxed), 130);
        assert_eq!(stat.samples.load(Ordering::Relaxed), 3);
        stat.record(500); stat.record(200);
        assert!(stat.ns.load(Ordering::Relaxed) >= 700);
        assert!(stat.max_ns.load(Ordering::Relaxed) >= 500);
        begin_gpu_window();
        let start=WINDOW.get().unwrap().lock().unwrap().as_ref().unwrap().start;
        { let _commands=span(Metric::Commands); { let _submit=span(Metric::Submit); } }
        let end=Instant::now(); change_phase(None,end);
        let accounted=EXCLUSIVE.iter().map(|v|v.load(Ordering::Relaxed)).sum::<u64>();
        assert_eq!(accounted,end.duration_since(start).as_nanos()as u64);
        GPU_PHASE.with(|phase|assert_eq!(phase.borrow().as_ref().unwrap().phase as usize,Phase::LoopOther as usize));
        configure(false); let before=stat.calls.load(Ordering::Relaxed);
        { let _span = span(Metric::DrawSample); }
        assert_eq!(stat.calls.load(Ordering::Relaxed),before);
        configure(true); assert_eq!(stat.calls.load(Ordering::Relaxed),0);
        begin_gpu_window();
        binding_cache_lookup(true); binding_cache_lookup(false);
        observe_pack_signature(vec![7, 1, 64, 10, 0, 1],64);
        observe_pack_signature(vec![7, 1, 64, 10, 0, 1],64);
        // A source version or bound range change is a different exact key.
        observe_pack_signature(vec![7, 1, 64, 10, 0, 2],64);
        observe_pack_signature(vec![7, 1, 64, 10, 1, 2],64);
        invalidate_binding_observations();
        observe_pack_signature(vec![7, 1, 64, 10, 0, 1],64);
        let observations=binding_report();
        assert_eq!(observations["cacheHits"],1); assert_eq!(observations["cacheMisses"],1);
        assert_eq!(observations["sampledPackCalls"],5);
        assert_eq!(observations["sampledMetadataReuseCalls"],1);
        assert_eq!(observations["sampledMetadataReuseBytes"],64);
        assert_eq!(observations["observationObjectInvalidations"],1);
        assert_eq!(observations["observationKeys"],1);
        for i in 0..300 { observe_pack_signature(vec![i+100],16); }
        observe_pack_signature(vec![0;MAX_BINDING_SIGNATURE_WORDS+1],16);
        let observations=binding_report();
        assert!(observations["observationKeys"].as_u64().unwrap()<=256);
        assert_eq!(observations["observationCapacityResets"],1);
        assert_eq!(observations["oversizedSignaturesSkipped"],1);
        configure(false);
        let generation=RESOURCE_GENERATION.load(Ordering::Relaxed);
        invalidate_binding_observations();
        assert_eq!(RESOURCE_GENERATION.load(Ordering::Relaxed),generation);
    }
}
