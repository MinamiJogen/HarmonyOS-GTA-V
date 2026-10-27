//! Opt-in, bounded host wall-time partitions between guest/source boundaries.
//! No GPU timestamps, queries, waits, submissions or synchronous hot-path IO.
use std::{cell::RefCell, io::Write, path::PathBuf, sync::{Arc, OnceLock,
    atomic::{AtomicBool, AtomicU64, Ordering}, mpsc::{self, SyncSender, TrySendError}}};

const MAX_RECORDS: u64 = 32768;
const QUEUE_CAPACITY: usize = 128;
const MAX_DEPTH: u32 = 32;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const PHASE_COUNT: usize = 19;
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(usize)]
pub(super) enum Stage {
    LoopOther, CommandsOther, RingWait, PresentOther, Submit, Acquire, PresentCall,
    UboWait, Op8Wait, Poll, ResourceCreate, ScopePop, PipelineCreate, EncoderFinish,
    Upload, SourcePacing, SourceSnapshot, OutputOther, OutputWait,
}
pub(super) const PHASE_NAMES: [&str; PHASE_COUNT] = ["loopOther", "commandsOther", "ringWait", "presentOther",
    "submit", "acquire", "presentCall", "uboWait", "op8Wait", "poll", "resourceCreate",
    "scopePop", "pipelineCreate", "encoderFinish", "upload", "sourcePacing", "sourceSnapshot", "outputOther", "outputWait"];
static ENABLED: AtomicBool = AtomicBool::new(false);
static STATS: OnceLock<Arc<Stats>> = OnceLock::new();
#[derive(Default)]
struct Stats {
    attempted: AtomicU64, enqueued: AtomicU64, written: AtomicU64, flushed: AtomicU64, queue_drops: AtomicU64,
    clock_failures: AtomicU64, stack_errors: AtomicU64, duplicate_sources: AtomicU64,
    resets: AtomicU64, stage_entries: AtomicU64, clock_reads: AtomicU64, boundary_calls: AtomicU64,
    invalid_boundaries: AtomicU64, writer_errors: AtomicU64, record_limit: AtomicBool,
    writer_running: AtomicBool, writer_finished: AtomicBool,
}
impl Stats {
    fn json(&self) -> serde_json::Value { serde_json::json!({
        "intervalsAttempted":self.attempted.load(Ordering::Relaxed),
        "intervalsEnqueued":self.enqueued.load(Ordering::Relaxed),
        "intervalsWritten":self.written.load(Ordering::Relaxed),
        "intervalsFlushed":self.flushed.load(Ordering::Relaxed),
        "writerQueueDrops":self.queue_drops.load(Ordering::Relaxed),
        "clockFailures":self.clock_failures.load(Ordering::Relaxed),
        "stackErrors":self.stack_errors.load(Ordering::Relaxed),
        "duplicateSourcesExcluded":self.duplicate_sources.load(Ordering::Relaxed),
        "lifecycleResets":self.resets.load(Ordering::Relaxed),
        "stageEntries":self.stage_entries.load(Ordering::Relaxed),
        "clockReads":self.clock_reads.load(Ordering::Relaxed),
        "boundaryCalls":self.boundary_calls.load(Ordering::Relaxed),
        "invalidBoundaries":self.invalid_boundaries.load(Ordering::Relaxed),
        "writerErrors":self.writer_errors.load(Ordering::Relaxed),
        "recordLimitReached":self.record_limit.load(Ordering::Relaxed),
        "writerRunning":self.writer_running.load(Ordering::Acquire),
        "writerFinished":self.writer_finished.load(Ordering::Acquire),
    }) }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity { generation: u64, epoch: Option<u64> }
#[derive(Clone, Copy, Debug)]
struct Boundary { at: u64, guest: u64, source: Option<u64>, identity: Identity }
#[derive(Default)]
struct Ledger { begin: Option<Boundary>, phases: [u64; PHASE_COUNT], incomplete: bool }
impl Ledger {
    fn prime(&mut self, boundary: Boundary) { *self = Self { begin: Some(boundary), ..Self::default() }; }
    fn charge(&mut self, stage: Stage, ns: u64) {
        if self.begin.is_some() { self.phases[stage as usize] = self.phases[stage as usize].saturating_add(ns); }
    }
    fn finish(&mut self, boundary: Boundary, kind: &'static str) -> Option<Record> {
        let begin = self.begin;
        let phases = self.phases;
        let incomplete = self.incomplete;
        self.prime(boundary);
        let begin = begin?;
        if begin.identity != boundary.identity || boundary.at <= begin.at { return None; }
        let duration = boundary.at - begin.at;
        let accounted = phases.iter().copied().sum::<u64>();
        Some(Record { kind, begin, end: boundary, phases, complete: !incomplete && accounted == duration,
            index: 0, queue_drops: 0, clock_failures: 0, stack_errors: 0 })
    }
}
struct Record {
    kind: &'static str, begin: Boundary, end: Boundary, phases: [u64; PHASE_COUNT], complete: bool,
    index: u64, queue_drops: u64, clock_failures: u64, stack_errors: u64,
}
impl Record {
    fn json(&self) -> serde_json::Value {
        let phases: serde_json::Map<String,serde_json::Value> = PHASE_NAMES.iter().zip(self.phases)
            .map(|(name,ns)| ((*name).into(),serde_json::json!(ns))).collect();
        serde_json::json!({"type":"frame_interval","schemaVersion":1,"pid":std::process::id(),
            "boundaryKind":self.kind,"recordIndex":self.index,
            "beginMonotonicNs":self.begin.at,"endMonotonicNs":self.end.at,
            "durationNs":self.end.at-self.begin.at,"presentationGeneration":self.end.identity.generation,
            "cacheEpoch":self.end.identity.epoch,
            "beginGuestSequence":self.begin.guest,"endGuestSequence":self.end.guest,
            "beginSourceSequence":self.begin.source,"endSourceSequence":self.end.source,
            "phaseWallNs":phases,"accountedWallNs":self.phases.iter().sum::<u64>(),
            "intervalCoverageComplete":self.complete,
            "coverageComplete":self.complete && self.queue_drops==0 && self.clock_failures==0 && self.stack_errors==0,
            "countersAtEnqueue":{"writerQueueDrops":self.queue_drops,"clockFailures":self.clock_failures,"stackErrors":self.stack_errors}})
    }
}
struct State {
    sender: Option<SyncSender<Record>>, stats: Arc<Stats>, guest: Ledger, source: Ledger,
    cursor: Option<u64>, stage: Stage, depth: u32, revision: u64,
    lifecycle: Option<(u64,bool)>, source_identity: Option<(Identity,u64)>,
}
impl State {
    fn new(sender: SyncSender<Record>, stats: Arc<Stats>) -> Self {
        Self { sender:Some(sender),stats,guest:Ledger::default(),source:Ledger::default(),
            cursor:None,stage:Stage::LoopOther,depth:0,revision:0,lifecycle:None,source_identity:None }
    }
    fn invalidate(&mut self) { self.guest.incomplete=true; self.source.incomplete=true; }
    fn advance(&mut self, now: Option<u64>) -> Option<u64> {
        let Some(now) = now else { self.stats.clock_failures.fetch_add(1,Ordering::Relaxed); self.invalidate(); return None; };
        if let Some(cursor) = self.cursor {
            if now < cursor { self.stats.clock_failures.fetch_add(1,Ordering::Relaxed); self.invalidate(); return None; }
            self.guest.charge(self.stage,now-cursor); self.source.charge(self.stage,now-cursor);
        }
        self.cursor=Some(now); Some(now)
    }
    fn reset(&mut self) {
        self.guest=Ledger::default();self.source=Ledger::default();self.source_identity=None;
        self.depth=0;self.stage=Stage::LoopOther;self.revision=self.revision.wrapping_add(1);
        self.stats.resets.fetch_add(1,Ordering::Relaxed);
    }
    fn lifecycle(&mut self, generation:u64, paused:bool) {
        if self.lifecycle != Some((generation,paused)) { self.reset();self.lifecycle=Some((generation,paused)); }
    }
    fn publish(&mut self, mut record: Record) {
        let index=self.stats.attempted.fetch_add(1,Ordering::Relaxed)+1;
        if index>MAX_RECORDS { return; }
        record.index=index;record.queue_drops=self.stats.queue_drops.load(Ordering::Relaxed);
        record.clock_failures=self.stats.clock_failures.load(Ordering::Relaxed);record.stack_errors=self.stats.stack_errors.load(Ordering::Relaxed);
        if let Some(sender)=self.sender.as_ref() {
            match sender.try_send(record) {
                Ok(())=>{self.stats.enqueued.fetch_add(1,Ordering::Relaxed);},
                Err(TrySendError::Full(_))=>{self.stats.queue_drops.fetch_add(1,Ordering::Relaxed);},
                Err(TrySendError::Disconnected(_))=>{self.stats.writer_errors.fetch_add(1,Ordering::Relaxed);self.sender.take();ENABLED.store(false,Ordering::Relaxed);},
            }
        }
        if index==MAX_RECORDS { self.stats.record_limit.store(true,Ordering::Relaxed);self.sender.take();ENABLED.store(false,Ordering::Relaxed); }
    }
    fn boundary(&mut self, now:Option<u64>, generation:u64, epoch:Option<u64>, guest:u64, source:Option<u64>) {
        self.stats.boundary_calls.fetch_add(1,Ordering::Relaxed);
        if self.lifecycle==Some((generation,true)) {self.stats.invalid_boundaries.fetch_add(1,Ordering::Relaxed);return;}
        self.lifecycle(generation,false);
        let Some(now)=self.advance(now) else {return;};
        let identity=Identity{generation,epoch};
        let boundary=Boundary{at:now,guest,source,identity};
        if let Some(sequence)=source {
            if let Some((old,old_sequence))=self.source_identity {
                if old==identity && sequence<=old_sequence {self.stats.duplicate_sources.fetch_add(1,Ordering::Relaxed);return;}
            }
            if self.source.begin.is_some_and(|old|old.identity==identity && now<=old.at) {
                self.stats.invalid_boundaries.fetch_add(1,Ordering::Relaxed);self.source.incomplete=true;return;
            }
            self.source_identity=Some((identity,sequence));
            if let Some(record)=self.source.finish(boundary,"source_present_call") {self.publish(record);}
        } else {
            if self.guest.begin.is_some_and(|old| guest<=old.guest || now<=old.at) {
                self.stats.invalid_boundaries.fetch_add(1,Ordering::Relaxed);self.guest.incomplete=true;return;
            }
            if let Some(record)=self.guest.finish(boundary,"guest") {self.publish(record);}
        }
    }
}
thread_local! { static STATE:RefCell<Option<State>>=const{RefCell::new(None)}; }
pub(super) struct Mark { old:Stage, depth:u32, revision:u64 }
#[inline] pub(crate) fn enabled()->bool { ENABLED.load(Ordering::Relaxed) }
fn clock_ns()->Option<u64> {
    if let Some(stats)=STATS.get(){stats.clock_reads.fetch_add(1,Ordering::Relaxed);}
    let mut t=libc::timespec{tv_sec:0,tv_nsec:0};
    if unsafe{libc::clock_gettime(libc::CLOCK_MONOTONIC,&mut t)}!=0 || t.tv_sec<0 || !(0..1_000_000_000).contains(&t.tv_nsec) {return None;}
    (t.tv_sec as u64).checked_mul(1_000_000_000)?.checked_add(t.tv_nsec as u64)
}
#[inline] pub(super) fn enter(stage:Stage)->Option<Mark> {
    if !enabled(){return None;}
    STATE.with(|state|{
        let mut state=state.borrow_mut();let state=state.as_mut()?;
        state.stats.stage_entries.fetch_add(1,Ordering::Relaxed);
        if state.depth>=MAX_DEPTH {state.stats.stack_errors.fetch_add(1,Ordering::Relaxed);state.invalidate();return None;}
        state.advance(clock_ns());let mark=Mark{old:state.stage,depth:state.depth+1,revision:state.revision};
        state.depth+=1;state.stage=stage;Some(mark)
    })
}
pub(super) fn leave(mark:Mark) {
    if !enabled(){return;}
    STATE.with(|state|if let Some(state)=state.borrow_mut().as_mut(){
        if mark.revision!=state.revision{return;}
        state.advance(clock_ns());
        if mark.depth!=state.depth {state.stats.stack_errors.fetch_add(1,Ordering::Relaxed);state.invalidate();state.depth=0;state.stage=Stage::LoopOther;state.revision=state.revision.wrapping_add(1);}
        else {state.depth-=1;state.stage=mark.old;}
    });
}
#[inline] pub(crate) fn lifecycle(generation:u64,paused:bool) {
    if enabled(){STATE.with(|state|if let Some(state)=state.borrow_mut().as_mut(){
        if state.lifecycle!=Some((generation,paused)){state.advance(clock_ns());state.lifecycle(generation,paused);}
    });}
}
#[inline] pub(crate) fn guest_boundary(generation:u64,sequence:u64) {
    if enabled(){STATE.with(|state|if let Some(state)=state.borrow_mut().as_mut(){state.boundary(clock_ns(),generation,None,sequence,None);});}
}
#[inline] pub(crate) fn source_boundary(generation:u64,epoch:Option<u64>,sequence:u64,guest_completed:u64) {
    if enabled(){STATE.with(|state|if let Some(state)=state.borrow_mut().as_mut(){state.boundary(clock_ns(),generation,epoch,guest_completed,Some(sequence));});}
}
/// Atomic counters only: safe for an existing diagnostic snapshot, no file IO.
pub(crate) fn status()->serde_json::Value {
    let Some(stats)=STATS.get() else{return serde_json::json!({"enabled":false,"requested":false});};
    serde_json::json!({"enabled":enabled(),"requested":true,"counters":stats.json(),
        "scope":"host-thread wall only; observer impact requires same-package trace-off/on comparison; absent footer requires retained-row/window coverage audit"})
}
fn mode_for_monitor_flag(value:Option<&std::ffi::OsStr>)->u32 {
    if value==Some(std::ffi::OsStr::new("1")){0o660}else{0o600}
}
pub(crate) fn initialize() {
    if std::env::var_os("MUGUET_FRAME_INTERVAL_TRACE").as_deref()!=Some(std::ffi::OsStr::new("1")){return;}
    let Some(root)=std::env::var_os("MUGUET_SUPPORT") else{return;};
    let stats=Arc::new(Stats::default());
    if STATS.set(stats.clone()).is_err(){return;}
    let mode=mode_for_monitor_flag(std::env::var_os("MUGUET_NATIVE_BENCHMARK_MONITOR").as_deref());
    let (sender,receiver)=mpsc::sync_channel(QUEUE_CAPACITY);
    let path=PathBuf::from(root).join(format!("native-frame-intervals-{}.jsonl",std::process::id()));
    let worker_stats=stats.clone();
    // Publish before spawn: a worker failure must not be undone by a late
    // producer enable after the worker has already stopped.
    ENABLED.store(true,Ordering::Relaxed);
    match std::thread::Builder::new().name("frame-interval-writer".into()).spawn(move||{
        worker_stats.writer_running.store(true,Ordering::Release);
        if let Err(error)=write_records(path,receiver,&worker_stats,mode) {
            worker_stats.writer_errors.fetch_add(1,Ordering::Relaxed);ENABLED.store(false,Ordering::Relaxed);
            eprintln!("[frame-interval] writer stopped: {error}");
        }
        worker_stats.writer_running.store(false,Ordering::Release);worker_stats.writer_finished.store(true,Ordering::Release);
    }) {
        Ok(_)=>{STATE.with(|state|*state.borrow_mut()=Some(State::new(sender,stats)));},
        Err(error)=>{ENABLED.store(false,Ordering::Relaxed);stats.writer_errors.fetch_add(1,Ordering::Relaxed);stats.writer_finished.store(true,Ordering::Release);eprintln!("[frame-interval] disabled: {error}");},
    }
}
fn write_records(path:PathBuf,receiver:mpsc::Receiver<Record>,stats:&Stats,mode:u32)->std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt,PermissionsExt};
    let file=std::fs::OpenOptions::new().create_new(true).write(true).mode(mode)
        .custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(path)?;
    if mode==0o660 {file.set_permissions(std::fs::Permissions::from_mode(mode))?;}
    let mut file=std::io::BufWriter::with_capacity(32768,file);let mut bytes=0u64;
    let mut line=|value:serde_json::Value,file:&mut std::io::BufWriter<std::fs::File>|->std::io::Result<()> {
        let data=serde_json::to_vec(&value)?;let size=data.len()as u64+1;
        if bytes+size>MAX_FILE_BYTES{return Err(std::io::Error::other("bounded trace file limit reached"));}
        file.write_all(&data)?;file.write_all(b"\n")?;bytes+=size;Ok(())
    };
    line(serde_json::json!({"type":"header","schemaVersion":1,"pid":std::process::id(),"clock":"CLOCK_MONOTONIC_ns",
        "phaseNames":PHASE_NAMES,"recordLimit":MAX_RECORDS,"writerQueueLimit":QUEUE_CAPACITY,"fileByteLimit":MAX_FILE_BYTES,
        "defaultEnabled":false,"scope":"exclusive host GPU-thread wall; not GPU execution, CPU time or input/display latency",
        "phaseSemantics":{"sourcePacing":"residual pacing-control wall including sleep/descheduling and uninstrumented work, not pure sleep",
            "pipelineCreate":"host create API elapsed, not GPU execution or pure compiler CPU time",
            "poll":"host polling/map/copy boundary, not asynchronous GPU duration",
            "outputOther":"residual output graph host construction; existing AI/SR algorithm unchanged",
            "loopOther":"residual host wall outside named stages, not proof of idle time"},
        "attribution":"temporal overlap between boundaries, not causal cost of source N; independent guest/source chains must not be summed; AI/repeat host work may overlap either chain",
        "coverage":"all intervals attempted until bound; first boundary and generation/epoch changes prime only; nested phases partition at every boundary; missing footer is not proof of full-session coverage",
        "observer":"stageEntries and clockReads report intensity, not observer cost; same-package trace-off/on benchmark required; no per-opcode clocks or GPU waits/submits added"}),&mut file)?;
    file.flush()?;let mut flushed=std::time::Instant::now();
    loop {
        match receiver.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(record)=>{line(record.json(),&mut file)?;stats.written.fetch_add(1,Ordering::Relaxed);},
            Err(mpsc::RecvTimeoutError::Timeout)=>{},
            Err(mpsc::RecvTimeoutError::Disconnected)=>break,
        }
        if flushed.elapsed()>=std::time::Duration::from_millis(200){file.flush()?;stats.flushed.store(stats.written.load(Ordering::Relaxed),Ordering::Relaxed);flushed=std::time::Instant::now();}
    }
    file.flush()?;stats.flushed.store(stats.written.load(Ordering::Relaxed),Ordering::Relaxed);
    line(serde_json::json!({"type":"footer","schemaVersion":1,"pid":std::process::id(),"producerClosed":true,
        "counters":stats.json(),"scope":"clean writer drain only; record cap and dropped/invalid intervals still limit coverage"}),&mut file)?;
    file.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn model()->(State,mpsc::Receiver<Record>) {
        let (sender,receiver)=mpsc::sync_channel(QUEUE_CAPACITY);(State::new(sender,Arc::new(Stats::default())),receiver)
    }
    // Real host CLOCK_MONOTONIC may have coarser resolution than two adjacent
    // test calls. Equal-clock rejection is covered separately with fixed input.
    fn host_tick() {std::thread::sleep(std::time::Duration::from_micros(1));}
    #[test] fn monitor_file_mode_requires_exact_os_value() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(mode_for_monitor_flag(None),0o600);
        assert_eq!(mode_for_monitor_flag(Some(OsStr::new("1"))),0o660);
        for value in ["", "0", "true", "01", " 1", "1 ", "1\n"] {
            assert_eq!(mode_for_monitor_flag(Some(OsStr::new(value))),0o600);
        }
        assert_eq!(mode_for_monitor_flag(Some(OsStr::from_bytes(b"1\xff"))),0o600);
    }
    #[test] fn nested_phases_partition_exactly_across_frame_boundaries() {
        let (mut s,r)=model();s.boundary(Some(100),1,None,1,None);
        s.advance(Some(110));s.stage=Stage::CommandsOther;
        s.advance(Some(120));s.stage=Stage::PipelineCreate;
        s.boundary(Some(140),1,None,2,None);
        s.advance(Some(150));s.stage=Stage::CommandsOther;
        s.advance(Some(170));s.stage=Stage::LoopOther;
        s.boundary(Some(180),1,None,3,None);
        let a=r.recv().unwrap();let b=r.recv().unwrap();
        assert_eq!(a.phases[Stage::LoopOther as usize],10);assert_eq!(a.phases[Stage::CommandsOther as usize],10);
        assert_eq!(a.phases[Stage::PipelineCreate as usize],20);assert_eq!(a.phases.iter().sum::<u64>(),40);
        assert_eq!(b.phases[Stage::PipelineCreate as usize],10);assert_eq!(b.phases[Stage::CommandsOther as usize],20);
        assert_eq!(b.phases[Stage::LoopOther as usize],10);assert!(a.complete&&b.complete);
    }
    #[test] fn guest_source_independent_deduplicated_and_epoch_reset() {
        let (mut s,r)=model();s.boundary(Some(100),1,None,1,None);
        s.boundary(Some(110),1,Some(3),1,Some(10));s.boundary(Some(120),1,Some(3),1,Some(10));
        s.boundary(Some(130),1,None,2,None);s.boundary(Some(150),1,Some(3),2,Some(11));
        let guest=r.recv().unwrap();let source=r.recv().unwrap();assert_eq!(guest.kind,"guest");assert_eq!(source.kind,"source_present_call");
        assert_eq!(guest.end.at-guest.begin.at,30);assert_eq!(source.end.at-source.begin.at,40);
        assert_eq!(s.stats.duplicate_sources.load(Ordering::Relaxed),1);
        s.boundary(Some(160),1,Some(4),2,Some(1));assert!(r.try_recv().is_err());
        s.boundary(Some(170),2,None,3,None);s.boundary(Some(180),2,Some(4),3,Some(1));assert!(r.try_recv().is_err());
    }
    #[test] fn paused_gap_is_excluded_and_first_resumed_boundary_only_primes() {
        let (mut s,r)=model();s.boundary(Some(100),1,None,1,None);s.advance(Some(120));s.lifecycle(2,true);
        s.advance(Some(1_000_000));s.lifecycle(3,false);s.boundary(Some(1_000_020),3,None,2,None);
        assert!(r.try_recv().is_err());s.boundary(Some(1_000_050),3,None,3,None);
        let record=r.recv().unwrap();assert_eq!(record.end.at-record.begin.at,30);assert!(record.complete);
    }
    #[test] fn clock_failure_and_regression_are_never_complete() {
        let (mut s,r)=model();s.boundary(Some(100),1,None,1,None);s.advance(None);s.advance(Some(90));
        s.boundary(Some(150),1,None,2,None);let record=r.recv().unwrap();assert!(!record.complete);
        assert_eq!(record.json()["coverageComplete"],false);assert_eq!(s.stats.clock_failures.load(Ordering::Relaxed),2);
    }
    #[test] fn backpressure_is_nonblocking_bounded_and_sequence_gaps_visible() {
        let (mut s,r)=model();
        for n in 1..=(QUEUE_CAPACITY as u64+10){s.boundary(Some(n*10),1,None,n,None);}
        assert_eq!(s.stats.enqueued.load(Ordering::Relaxed),QUEUE_CAPACITY as u64);
        assert!(s.stats.queue_drops.load(Ordering::Relaxed)>0);
        let first=r.recv().unwrap();s.boundary(Some(10000),1,None,1000,None);
        let records:Vec<_>=r.try_iter().collect();let last=records.last().unwrap();assert!(last.index>first.index+QUEUE_CAPACITY as u64);
        assert_eq!(last.json()["coverageComplete"],false);
    }
    #[test] fn fixed_record_cap_closes_producer_and_stops_capture() {
        let (mut s,_r)=model();s.stats.attempted.store(MAX_RECORDS-1,Ordering::Relaxed);
        s.boundary(Some(100),1,None,1,None);s.boundary(Some(200),1,None,2,None);
        assert!(s.sender.is_none());assert!(s.stats.record_limit.load(Ordering::Relaxed));
    }
    #[test] fn off_path_does_not_initialize_state_clock_or_writer() {
        ENABLED.store(false,Ordering::Relaxed);assert!(STATS.get().is_none());
        assert!(enter(Stage::PipelineCreate).is_none());lifecycle(1,false);guest_boundary(1,1);source_boundary(1,Some(3),1,1);
        STATE.with(|state|assert!(state.borrow().is_none()));assert_eq!(status()["requested"],false);
    }
    #[test] fn actual_guards_detect_out_of_order_drop_without_double_accounting() {
        let (state,r)=model();STATE.with(|slot|*slot.borrow_mut()=Some(state));ENABLED.store(true,Ordering::Relaxed);
        guest_boundary(1,1);let a=enter(Stage::CommandsOther).unwrap();let b=enter(Stage::PipelineCreate).unwrap();
        leave(a);leave(b);host_tick();guest_boundary(1,2);
        let record=r.recv().unwrap();assert!(!record.complete);assert_eq!(record.json()["countersAtEnqueue"]["stackErrors"],1);
        assert_eq!(record.phases.iter().sum::<u64>(),record.end.at-record.begin.at);
        ENABLED.store(false,Ordering::Relaxed);STATE.with(|slot|slot.borrow_mut().take());
    }
    #[test] fn actual_guards_cross_boundaries_and_old_generation_drop_is_ignored() {
        let (state,r)=model();STATE.with(|slot|*slot.borrow_mut()=Some(state));ENABLED.store(true,Ordering::Relaxed);
        guest_boundary(1,1);let a=enter(Stage::CommandsOther).unwrap();let b=enter(Stage::PipelineCreate).unwrap();
        host_tick();guest_boundary(1,2);leave(b);leave(a);host_tick();guest_boundary(1,3);
        STATE.with(|slot|assert_eq!(slot.borrow().as_ref().unwrap().stats.enqueued.load(Ordering::Relaxed),2));
        for _ in 0..2 {let record=r.recv_timeout(std::time::Duration::from_secs(1)).unwrap();assert!(record.complete);assert_eq!(record.phases.iter().sum::<u64>(),record.end.at-record.begin.at);}
        let old=enter(Stage::Upload).unwrap();lifecycle(2,true);lifecycle(3,false);guest_boundary(3,4);leave(old);host_tick();guest_boundary(3,5);
        let record=r.recv_timeout(std::time::Duration::from_secs(1)).unwrap();assert!(record.complete);assert_eq!(record.phases[Stage::Upload as usize],0);
        ENABLED.store(false,Ordering::Relaxed);STATE.with(|slot|slot.borrow_mut().take());
    }
    #[test] fn maximum_nesting_is_bounded_and_overflow_invalidates_coverage() {
        let (state,r)=model();STATE.with(|slot|*slot.borrow_mut()=Some(state));ENABLED.store(true,Ordering::Relaxed);
        guest_boundary(1,1);let mut guards=Vec::new();for _ in 0..MAX_DEPTH{guards.push(enter(Stage::CommandsOther).unwrap());}
        assert!(enter(Stage::PipelineCreate).is_none());while let Some(mark)=guards.pop(){leave(mark);}
        guest_boundary(1,2);assert!(!r.recv().unwrap().complete);
        ENABLED.store(false,Ordering::Relaxed);STATE.with(|slot|slot.borrow_mut().take());
    }
    #[test] fn disconnected_writer_disables_without_blocking_or_render_fallback_io() {
        let (mut s,r)=model();drop(r);ENABLED.store(true,Ordering::Relaxed);
        s.boundary(Some(100),1,None,1,None);s.boundary(Some(200),1,None,2,None);
        assert!(!enabled());assert!(s.sender.is_none());assert_eq!(s.stats.writer_errors.load(Ordering::Relaxed),1);
    }
    #[test] fn paused_boundary_is_rejected_and_equal_source_clock_is_incomplete() {
        let (mut s,r)=model();s.lifecycle(1,true);s.boundary(Some(100),1,None,1,None);assert!(r.try_recv().is_err());
        s.lifecycle(2,false);s.boundary(Some(200),2,Some(1),1,Some(1));s.boundary(Some(200),2,Some(1),1,Some(2));
        s.boundary(Some(300),2,Some(1),2,Some(3));assert!(!r.recv().unwrap().complete);
        assert_eq!(s.stats.invalid_boundaries.load(Ordering::Relaxed),2);
    }
    #[test] fn writer_emits_header_rows_and_clean_drain_footer() {
        let (mut s,r)=model();let path=std::env::temp_dir().join(format!("frame-interval-test-{}-{}.jsonl",std::process::id(),clock_ns().unwrap()));
        s.boundary(Some(100),1,None,1,None);s.boundary(Some(200),1,None,2,None);s.sender.take();
        write_records(path.clone(),r,&s.stats,0o600).unwrap();let bytes=std::fs::read(&path).unwrap();
        let rows:Vec<serde_json::Value>=std::str::from_utf8(&bytes).unwrap().lines().map(|line|serde_json::from_str(line).unwrap()).collect();
        assert_eq!(rows.len(),3);assert_eq!(rows[0]["type"],"header");assert_eq!(rows[1]["durationNs"],100);
        assert_eq!(rows[1]["accountedWallNs"],100);assert_eq!(rows[2]["counters"]["intervalsWritten"],1);
        assert_eq!(rows[2]["producerClosed"],true);std::fs::remove_file(path).unwrap();
    }
    #[test] fn writer_create_new_preserves_existing_file_contents_and_permissions() {
        use std::os::unix::fs::{OpenOptionsExt,PermissionsExt};
        let path=std::env::temp_dir().join(format!("frame-interval-existing-{}-{}.jsonl",std::process::id(),clock_ns().unwrap()));
        let mut existing=std::fs::OpenOptions::new().create_new(true).write(true).mode(0o640).open(&path).unwrap();
        existing.write_all(b"existing trace\n").unwrap();drop(existing);
        let permissions=std::fs::metadata(&path).unwrap().permissions().mode();
        for mode in [0o600,0o660] {
            let (mut s,r)=model();s.boundary(Some(100),1,None,1,None);s.boundary(Some(200),1,None,2,None);s.sender.take();
            let error=write_records(path.clone(),r,&s.stats,mode).unwrap_err();
            assert_eq!(error.kind(),std::io::ErrorKind::AlreadyExists);
            assert_eq!(std::fs::read(&path).unwrap(),b"existing trace\n");
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode(),permissions);
            assert_eq!(s.stats.written.load(Ordering::Relaxed),0);
            assert_eq!(s.stats.flushed.load(Ordering::Relaxed),0);
        }
        std::fs::remove_file(path).unwrap();
    }
}
