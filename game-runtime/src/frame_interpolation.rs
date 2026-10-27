//! Opt-in pixel-pair interpolation of the actual guest's final texture.
//! GPU copies/upload/presentation stay on the game's GPU thread. The bounded
//! worker owns only RGB bytes and the selected RIFE Vulkan / public NNRT context.
use std::sync::Arc;
use std::time::{Duration, Instant};
use wgpu::{CommandBuffer, CommandEncoder, Device, Queue, Texture};

#[cfg(any(feature = "frame-interpolation", test))]
#[path = "frame_readback_rows.rs"]
mod readback_rows;

// Process-local UI observations; never infer a live backend from a status file
// left by another process. State 2 requires a successfully presented NEW AI frame.
static LIVE_STATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
static LIVE_AI_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static LIVE_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static LIVE_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[derive(Clone,Copy)]
struct LiveSnapshot { state:u8,last:u64,generation:u64 }
// The existing writes all run on the GPU thread. A reader makes one attempt,
// without spinning, and rejects a snapshot that overlaps any of those writes.
fn publish_live(state:u8,last:u64,generation:u64) {
    use std::sync::atomic::Ordering;
    LIVE_REVISION.fetch_add(1,Ordering::SeqCst);
    LIVE_AI_NS.store(last,Ordering::SeqCst);
    LIVE_GENERATION.store(generation,Ordering::SeqCst);
    LIVE_STATE.store(state,Ordering::SeqCst);
    LIVE_REVISION.fetch_add(1,Ordering::SeqCst);
}
fn live_snapshot()->Option<LiveSnapshot> {
    use std::sync::atomic::Ordering;
    let before=LIVE_REVISION.load(Ordering::SeqCst);
    if before&1!=0 {return None;}
    let snapshot=LiveSnapshot {state:LIVE_STATE.load(Ordering::SeqCst),
        last:LIVE_AI_NS.load(Ordering::SeqCst),generation:LIVE_GENERATION.load(Ordering::SeqCst)};
    let after=LIVE_REVISION.load(Ordering::SeqCst);
    (before==after).then_some(snapshot)
}
fn live_snapshot_current(snapshot:Option<LiveSnapshot>,before:u64,after:u64)->bool {
    before==after&&snapshot.is_some_and(|value|value.generation==after)
}
fn note_live(state:u8) {
    publish_live(state,0,crate::ffi::app_presentation_generation());
}
pub(crate) fn live_status()->serde_json::Value {
    let generation_before=crate::ffi::app_presentation_generation();
    let snapshot=live_snapshot();
    let current=live_snapshot_current(snapshot,generation_before,crate::ffi::app_presentation_generation());
    let LiveSnapshot {state,last,..}=snapshot.unwrap_or(LiveSnapshot {state:0,last:0,generation:0});
    let now=monotonic_ns();
    let fresh=current&&now.is_some_and(|now|last!=0&&now>=last&&now-last<=5_000_000_000);
    let status=if !crate::config::frame_interpolation_enabled(){"off"}
        else if !cfg!(feature="frame-interpolation"){"unavailable_not_compiled"}
        else if state==3{"unavailable_runtime_error"}
        else if crate::ffi::stop_requested(){"stopped"}
        else if !crate::ffi::app_presentation_allowed(){"paused"}
        else if state==2&&fresh{"running"}else{"requested"};
    serde_json::json!({"state":status,"processId":std::process::id(),"observedMonotonicNs":now,
        "lastNewAiPresentMonotonicNs":if last==0{None}else{Some(last)},
        "fresh":fresh,"scope":"current process atomics; running requires a new AI native present within 5s in the current presentation generation; repeats never establish running"})
}

#[cfg(feature = "frame-interpolation")]
const WIDTH: u32 = 320;
#[cfg(feature = "frame-interpolation")]
const HEIGHT: u32 = 192;
#[derive(Clone)]
#[cfg_attr(not(feature = "frame-interpolation"),allow(dead_code))]
pub(crate) enum GpuFrame {
    Source(Texture),
    Flow { older:Texture,newer:Texture,half:Arc<Vec<u16>> },
}
/// The content identity survives repeated presentation slots. It never uses the
/// new output slot or the presentation label `repeat` as an image identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OutputContentKey {
    epoch:u64, class:&'static str, older_sequence:u64, older_slot:u64,
    newer_sequence:Option<u64>, newer_slot:Option<u64>,
}
#[cfg_attr(not(feature = "frame-interpolation"),allow(dead_code))]
pub(crate) struct Display {
    pub rgb: Arc<Vec<u8>>, pub slot: u64, pub source_sequence: u64,
    pub source_slot: u64, pub kind: &'static str, pub due: Instant,
    pub gpu: Option<GpuFrame>,
    pub content_key:OutputContentKey,
    pub nominal_content_slot:u64,
    pub newer_source_slot:Option<u64>,
    pub source_monotonic_ns:Option<u64>,pub newer_source_monotonic_ns:Option<u64>,pub newer_source_sequence:Option<u64>,
    true_midpoint_monotonic_ns:Option<u64>,scheduled_due_monotonic_ns:Option<u64>,midpoint_clock_error_ns:Option<i64>,
}

/// Pure scheduling policy, independent of a GPU or model implementation.
#[cfg(any(feature = "frame-interpolation", test))]
mod pairing {
    pub(super) const MAX_GAP_NS:u64=250_000_000;
    pub(super) const DELAY_NS:u64=200_000_000;
    #[derive(Clone,Copy,Debug,PartialEq,Eq)]
    pub(super) struct Endpoint {
        pub epoch:u64,pub presentation_generation:u64,pub sequence:u64,pub slot:u64,pub ordinal:u64,
        pub elapsed_ns:u64,pub monotonic_ns:Option<u64>,
    }
    #[derive(Clone,Copy,Debug,PartialEq,Eq)]
    pub(super) struct Pair {
        pub epoch:u64,pub presentation_generation:u64,pub older_sequence:u64,pub newer_sequence:u64,
        pub older_slot:u64,pub newer_slot:u64,pub midpoint_slot:u64,
        pub older_monotonic_ns:Option<u64>,pub newer_monotonic_ns:Option<u64>,
        pub true_midpoint_monotonic_ns:Option<u64>,pub scheduled_due_monotonic_ns:Option<u64>,
        pub gap_ns:u64,pub midpoint_elapsed_ns:u64,pub midpoint_clock_error_ns:i64,
        pub newer_elapsed_ns:u64,pub due_elapsed_ns:u64,pub newer_due_slot:u64,pub window_end_elapsed_ns:u64,
    }
    #[derive(Clone,Copy,Debug,PartialEq,Eq)]
    pub(super) enum Skip { Invalid, Discontinuity, Gap, Overflow }
    pub(super) fn select(a:Endpoint,b:Endpoint,period_ns:u64)->Result<Pair,Skip> {
        if a.epoch!=b.epoch||a.presentation_generation!=b.presentation_generation||a.sequence>=b.sequence||a.slot>=b.slot||a.elapsed_ns>=b.elapsed_ns||period_ns==0 {
            return Err(Skip::Invalid);
        }
        // A worker-channel drop must not interpolate across a mapped source
        // that already owns an event between these endpoints.
        if a.ordinal.checked_add(1)!=Some(b.ordinal) {return Err(Skip::Discontinuity);}
        let source_ns=period_ns.checked_mul(2).ok_or(Skip::Overflow)?;
        if a.elapsed_ns/source_ns!=a.slot||b.elapsed_ns/source_ns!=b.slot {return Err(Skip::Invalid);}
        let gap_ns=b.elapsed_ns-a.elapsed_ns;
        if gap_ns>MAX_GAP_NS {return Err(Skip::Gap);}
        let midpoint_slot=a.slot.checked_add(b.slot).ok_or(Skip::Overflow)?;
        let old_output=a.slot.checked_mul(2).ok_or(Skip::Overflow)?;
        let new_output=b.slot.checked_mul(2).ok_or(Skip::Overflow)?;
        if !(old_output<midpoint_slot&&midpoint_slot<new_output) {return Err(Skip::Invalid);}
        let midpoint_elapsed_ns=midpoint_slot.checked_mul(period_ns).ok_or(Skip::Overflow)?;
        let due_elapsed=midpoint_elapsed_ns.checked_add(DELAY_NS).ok_or(Skip::Overflow)?;
        let window_end_elapsed_ns=new_output.checked_mul(period_ns).and_then(|n|n.checked_add(DELAY_NS)).ok_or(Skip::Overflow)?;
        // Both images must exist before the nominal delayed midpoint; worker
        // results remain useful until the newer source's delayed due slot.
        if due_elapsed<=b.elapsed_ns {return Err(Skip::Invalid);}
        let true_midpoint_elapsed=a.elapsed_ns+gap_ns/2;
        let error=i128::from(midpoint_elapsed_ns)-i128::from(true_midpoint_elapsed);
        let midpoint_clock_error_ns=i64::try_from(error).map_err(|_|Skip::Overflow)?;
        let true_midpoint_monotonic_ns=match(a.monotonic_ns,b.monotonic_ns) {
            (Some(old),Some(new)) if new>old=>Some(old+(new-old)/2),
            (Some(_),Some(_))=>return Err(Skip::Invalid),_=>None,
        };
        let scheduled_due_monotonic_ns=b.monotonic_ns.and_then(|n|n.checked_add(due_elapsed-b.elapsed_ns));
        Ok(Pair {epoch:a.epoch,presentation_generation:a.presentation_generation,older_sequence:a.sequence,newer_sequence:b.sequence,
            older_slot:a.slot,newer_slot:b.slot,midpoint_slot,
            older_monotonic_ns:a.monotonic_ns,newer_monotonic_ns:b.monotonic_ns,
            true_midpoint_monotonic_ns,scheduled_due_monotonic_ns,gap_ns,midpoint_elapsed_ns,midpoint_clock_error_ns,
            newer_elapsed_ns:b.elapsed_ns,due_elapsed_ns:due_elapsed,newer_due_slot:new_output,window_end_elapsed_ns})
    }
    // Unlimited native cadence uses capture timestamps, never output-clock buckets.
    pub(super) const MAX_NATIVE_HOLD_NS:u64=50_000_000;
    pub(super) fn select_variable(a:Endpoint,b:Endpoint,deadline_ns:u64)->Result<Pair,Skip> {
        if a.epoch!=b.epoch||a.presentation_generation!=b.presentation_generation||a.sequence>=b.sequence||a.slot>=b.slot||a.elapsed_ns>=b.elapsed_ns||deadline_ns<=b.elapsed_ns {return Err(Skip::Invalid);}
        if a.ordinal.checked_add(1)!=Some(b.ordinal) {return Err(Skip::Discontinuity);}
        let gap_ns=b.elapsed_ns-a.elapsed_ns;
        if gap_ns>MAX_GAP_NS {return Err(Skip::Gap);}
        let midpoint_elapsed_ns=a.elapsed_ns+gap_ns/2;
        if midpoint_elapsed_ns<=a.elapsed_ns {return Err(Skip::Invalid);}
        let due_elapsed_ns=b.elapsed_ns+(deadline_ns-b.elapsed_ns)/2;
        let true_midpoint_monotonic_ns=match(a.monotonic_ns,b.monotonic_ns) {
            (Some(old),Some(new)) if new>old=>Some(old+(new-old)/2),
            (Some(_),Some(_))=>return Err(Skip::Invalid),_=>None,
        };
        Ok(Pair {epoch:a.epoch,presentation_generation:a.presentation_generation,older_sequence:a.sequence,newer_sequence:b.sequence,
            older_slot:a.slot,newer_slot:b.slot,midpoint_slot:midpoint_elapsed_ns,older_monotonic_ns:a.monotonic_ns,newer_monotonic_ns:b.monotonic_ns,
            true_midpoint_monotonic_ns,scheduled_due_monotonic_ns:b.monotonic_ns.and_then(|n|n.checked_add(due_elapsed_ns-b.elapsed_ns)),
            gap_ns,midpoint_elapsed_ns,midpoint_clock_error_ns:0,newer_elapsed_ns:b.elapsed_ns,due_elapsed_ns,
            newer_due_slot:b.elapsed_ns,window_end_elapsed_ns:deadline_ns})
    }
    /// At most two owned native snapshots. A new arrival releases the previous
    /// pending source immediately; a stalled producer drains within 50 ms.
    /// Optional AI work and the configured AI output budget never gate take().
    pub(super) struct NativePacer<T> {ready:Option<T>,pending:Option<(T,u64)>,last_capture:Option<u64>}
    impl<T> Default for NativePacer<T> {fn default()->Self {Self {ready:None,pending:None,last_capture:None}}}
    impl<T> NativePacer<T> {
        pub(super) fn offer(&mut self,value:T,captured:u64)->(u64,bool) {
            let gap=self.last_capture.map(|last|captured.saturating_sub(last));self.last_capture=Some(captured);
            let deadline=captured.saturating_add(gap.unwrap_or(0).clamp(1_000_000,MAX_NATIVE_HOLD_NS));
            let replaced=if gap.is_none() {self.ready.replace(value).is_some()}else {
                let old=self.pending.replace((value,deadline));old.is_some_and(|(old,_)|self.ready.replace(old).is_some())
            };
            (deadline,replaced)
        }
        pub(super) fn urgent(&self,now:u64)->bool {self.ready.is_some()||self.pending.as_ref().is_some_and(|(_,due)|now>=*due)}
        pub(super) fn busy(&self)->bool {self.ready.is_some()||self.pending.is_some()}
        pub(super) fn take(&mut self,now:u64)->Option<T> {
            self.ready.take().or_else(||if self.pending.as_ref().is_some_and(|(_,due)|now>=*due){self.pending.take().map(|(v,_)|v)}else{None})
        }
        pub(super) fn retry(&mut self,value:T) {if self.ready.is_none(){self.ready=Some(value);}}
    }
    pub(super) fn can_start(pair:Pair,now_elapsed_ns:u64)->bool {
        now_elapsed_ns>=pair.newer_elapsed_ns&&now_elapsed_ns<pair.window_end_elapsed_ns
    }
    #[derive(Clone,Copy,Debug,PartialEq,Eq)]
    pub(super) enum Admission { Future, Expired, Stale, OnTime, Late }
    /// Source events have no upper endpoint. An inferred midpoint is usable
    /// only before the newer source becomes due, and never moves content back.
    pub(super) fn admit(nominal:u64,current:u64,newer_due:Option<u64>,last_nominal:Option<u64>,same_content:bool)->Admission {
        if current<nominal {return Admission::Future;}
        if newer_due.is_some_and(|end|current>=end) {return Admission::Expired;}
        if same_content||last_nominal.is_some_and(|last|nominal<=last) {return Admission::Stale;}
        if current==nominal {Admission::OnTime}else{Admission::Late}
    }
    pub(super) fn is_late_ai_present(success:bool,is_ai:bool,nominal:u64,actual:u64)->bool {
        success&&is_ai&&actual>nominal
    }
    pub(super) fn work_is_current(allowed:bool,stopped:bool,source_generation:u64,current_generation:u64,
        source_epoch:u64,current_epoch:u64)->bool {
        allowed&&!stopped&&source_generation==current_generation&&source_epoch==current_epoch
    }
    pub(super) fn context_changed(old_generation:u64,generation:u64,old_visible:bool,visible:bool,
        old_physical:(u32,u32),physical:(u32,u32))->bool {
        old_generation!=generation||old_visible!=visible||old_physical!=physical
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        fn endpoint(slot:u64,sequence:u64,ordinal:u64,period:u64,offset:u64)->Endpoint {
            let elapsed_ns=slot*period*2+offset;
            Endpoint {epoch:7,presentation_generation:4,sequence,slot,ordinal,elapsed_ns,monotonic_ns:Some(1_000_000_000+elapsed_ns)}
        }
        fn variable_endpoint(sequence:u64,elapsed_ns:u64)->Endpoint {
            Endpoint {epoch:7,presentation_generation:4,sequence,slot:sequence,ordinal:sequence,elapsed_ns,monotonic_ns:Some(1_000_000_000+elapsed_ns)}
        }
        #[test]
        fn unlimited_native_80_120_240hz_has_no_source_or_output_bucket() {
            for hz in [80,120,240] {
                let gap=1_000_000_000/hz;let mut pacer=NativePacer::default();let mut presented=Vec::new();
                for n in 0..240u64 {
                    let capture=n*gap;let (deadline,replaced)=pacer.offer(n,capture);
                    assert!(!replaced);assert!(deadline<=capture+MAX_NATIVE_HOLD_NS);
                    if let Some(source)=pacer.take(capture) {presented.push(source);}
                    if n>0 {let pair=select_variable(variable_endpoint(n,capture-gap),variable_endpoint(n+1,capture),deadline).unwrap();
                        assert_eq!(pair.midpoint_slot,capture-gap+gap/2);assert_eq!(pair.midpoint_clock_error_ns,0);
                        assert_eq!(pair.newer_slot,n+1);assert!(pair.due_elapsed_ns<pair.window_end_elapsed_ns);}
                }
                presented.push(pacer.take(240*gap+MAX_NATIVE_HOLD_NS).unwrap());
                assert_eq!(presented,(0..240u64).collect::<Vec<_>>());
                assert!(!pacer.busy());assert!(!pacer.urgent(u64::MAX));
            }
        }
        #[test]
        fn variable_native_stalls_drain_within_50ms_without_accumulating_delay() {
            let mut pacer=NativePacer::default();pacer.offer(1,0);assert_eq!(pacer.take(0),Some(1));
            for (value,capture) in [(2,300_000_000),(3,2_000_000_000),(4,3_000_000_000)] {
                let (deadline,replaced)=pacer.offer(value,capture);assert!(!replaced);
                assert!(deadline>capture&&deadline<=capture+MAX_NATIVE_HOLD_NS);
                assert_eq!(pacer.take(deadline-1),None);assert_eq!(pacer.take(deadline),Some(value));
                assert!(!pacer.busy());
            }
        }
        #[test]
        fn irregular_native_arrivals_preserve_all_sources_without_catch_up_bursts() {
            let arrivals=[0,8_000_000,19_000_000,21_000_000,100_000_000,102_000_000,900_000_000];
            let mut pacer=NativePacer::default();let mut shown=Vec::new();
            for (index,capture) in arrivals.into_iter().enumerate() {
                if let Some(due)=pacer.pending.as_ref().map(|(_,due)|*due).filter(|due|*due<capture) {
                    shown.push((pacer.take(due).unwrap(),due));
                }
                assert!(!pacer.offer(index,capture).1);
                if let Some(source)=pacer.take(capture) {shown.push((source,capture));}
            }
            let due=pacer.pending.as_ref().unwrap().1;shown.push((pacer.take(due).unwrap(),due));
            assert_eq!(shown.iter().map(|(index,_)|*index).collect::<Vec<_>>(),(0..arrivals.len()).collect::<Vec<_>>());
            for (index,shown_ns) in shown {assert!(shown_ns>=arrivals[index]);assert!(shown_ns-arrivals[index]<=MAX_NATIVE_HOLD_NS);}
            assert!(!pacer.busy());
        }
        #[test]
        fn variable_source_backpressure_is_bounded_and_retry_keeps_owned_source() {
            let mut pacer=NativePacer::default();assert!(!pacer.offer(1,0).1);
            assert!(!pacer.offer(2,10_000_000).1);
            assert!(pacer.offer(3,20_000_000).1); // superseded unconsumed source1, fixed two slots
            assert_eq!(pacer.take(20_000_000),Some(2));pacer.retry(2);
            assert_eq!(pacer.take(20_000_000),Some(2));assert_eq!(pacer.take(30_000_000),Some(3));
            assert!(!pacer.busy());
            pacer.offer(4,40_000_000);pacer=NativePacer::default(); // same operation as lifecycle reset
            assert_eq!(pacer.take(u64::MAX),None);assert!(!pacer.busy());
        }
        #[test]
        fn variable_pair_skips_sample_holes_and_never_rewinds_after_native_newer() {
            let a=variable_endpoint(1,0);let b=variable_endpoint(2,10_000_000);
            let p=select_variable(a,b,20_000_000).unwrap();
            assert!(can_start(p,10_000_000));assert!(!can_start(p,20_000_000));
            assert_eq!(p.true_midpoint_monotonic_ns,Some(1_005_000_000));assert_eq!(p.scheduled_due_monotonic_ns,Some(1_015_000_000));
            assert_eq!(admit(p.midpoint_slot,p.midpoint_slot,None,Some(b.elapsed_ns),false),Admission::Stale);
            assert_eq!(select_variable(a,variable_endpoint(3,12_000_000),24_000_000),Err(Skip::Discontinuity));
            assert_eq!(select_variable(a,Endpoint {epoch:8,..b},20_000_000),Err(Skip::Invalid));
            assert_eq!(select_variable(a,b,b.elapsed_ns),Err(Skip::Invalid));
            assert_eq!(select_variable(a,variable_endpoint(2,MAX_GAP_NS+1),MAX_GAP_NS+2),Err(Skip::Gap));
        }
        #[test]
        fn adjacent_pairs_keep_the_original_slot_at_every_supported_clock() {
            for fps in [30,32,40,48] {
                let t=1_000_000_000/fps;
                let a=endpoint(10,21,1,t,2_000_000);let b=endpoint(11,22,2,t,2_000_000);
                let p=select(a,b,t).unwrap();
                assert_eq!(p.midpoint_slot,a.slot*2+1);assert_eq!(p.older_slot,10);assert_eq!(p.newer_slot,11);
                assert_eq!(p.older_sequence,21);assert_eq!(p.newer_sequence,22);
                assert_eq!(p.midpoint_clock_error_ns,-2_000_000);
            }
        }
        #[test]
        fn slow_real_pairs_use_actual_endpoints_and_even_midpoint_slots() {
            let t=1_000_000_000/48;
            for gap in [2,3,4,5,6] {
                let a=endpoint(10,50,9,t,1);let b=endpoint(10+gap,51,10,t,1);
                let p=select(a,b,t).unwrap();assert_eq!(p.midpoint_slot,20+gap);
                assert_eq!((p.older_slot,p.newer_slot),(10,10+gap));
                assert_eq!(p.true_midpoint_monotonic_ns,Some(1_000_000_000+a.elapsed_ns+(b.elapsed_ns-a.elapsed_ns)/2));
                let history=std::collections::BTreeMap::from([(a.slot,"older"),(b.slot,"newer")]);
                assert_eq!(history.get(&p.older_slot),Some(&"older"));assert_eq!(history.get(&p.newer_slot),Some(&"newer"));
                assert!(p.midpoint_slot>a.slot*2&&p.midpoint_slot<b.slot*2);
            }
        }
        #[test]
        fn missing_worker_input_cannot_overwrite_an_intermediate_source() {
            let t=20_833_333;let a=endpoint(0,1,1,t,0);let b=endpoint(2,3,3,t,0);
            // The old+new midpoint would be slot2, already owned by source2.
            assert_eq!(select(a,b,t),Err(Skip::Discontinuity));
            // Guest sequence holes from intentional source throttling are legal
            // when these are the consecutive mapped sources actually queued.
            assert!(select(a,Endpoint {ordinal:2,sequence:5,..b},t).is_ok());
        }
        #[test]
        fn long_gap_boundary_and_recovery_are_explicit() {
            let t=20_833_333;let a=endpoint(0,1,1,t,10_000_000);
            let b=Endpoint {slot:6,sequence:2,ordinal:2,elapsed_ns:a.elapsed_ns+MAX_GAP_NS,
                monotonic_ns:Some(a.monotonic_ns.unwrap()+MAX_GAP_NS),..a};
            assert!(select(a,b,t).is_ok());
            assert_eq!(select(a,Endpoint {elapsed_ns:b.elapsed_ns+1,..b},t),Err(Skip::Gap));
            let long=endpoint(10,2,2,t,10_000_000);
            assert_eq!(select(a,long,t),Err(Skip::Gap));
            assert!(select(long,endpoint(12,3,3,t,10_000_000),t).is_ok());
        }
        #[test]
        fn stale_epoch_order_and_overflow_never_form_a_pair() {
            let t=20_833_333;let a=endpoint(0,1,1,t,1);let b=endpoint(1,2,2,t,1);
            for bad in [Endpoint {epoch:8,..b},Endpoint {presentation_generation:6,..b},Endpoint {sequence:1,..b},Endpoint {slot:0,..b},
                Endpoint {elapsed_ns:1,..b},Endpoint {elapsed_ns:0,..b},Endpoint {monotonic_ns:a.monotonic_ns,..b}] {
                assert!(select(a,bad,t).is_err());
            }
            assert_eq!(select(a,b,0),Err(Skip::Invalid));
            assert_eq!(select(a,b,u64::MAX),Err(Skip::Overflow));
            let near=Endpoint {epoch:7,presentation_generation:4,sequence:1,ordinal:1,slot:(u64::MAX-4)/2,elapsed_ns:u64::MAX-4,monotonic_ns:None};
            let end=Endpoint {sequence:2,ordinal:2,slot:(u64::MAX-2)/2,elapsed_ns:u64::MAX-2,..near};
            assert_eq!(select(near,end,1),Err(Skip::Overflow));
        }
        #[test]
        fn no_future_endpoint_and_no_backend_submission_after_usable_window() {
            let t=20_833_333;let a=endpoint(10,1,1,t,2*t-1);let b=endpoint(13,2,2,t,2*t-1);
            let p=select(a,b,t).unwrap();
            assert_eq!(p.midpoint_clock_error_ns,-((2*t-1) as i64));
            assert!(p.scheduled_due_monotonic_ns.unwrap()>b.monotonic_ns.unwrap());
            assert!(!can_start(p,b.elapsed_ns-1));assert!(can_start(p,b.elapsed_ns));
            assert!(can_start(p,p.due_elapsed_ns-1));assert!(can_start(p,p.due_elapsed_ns));
            assert!(can_start(p,p.window_end_elapsed_ns-1));assert!(!can_start(p,p.window_end_elapsed_ns));
            assert!(!can_start(p,p.window_end_elapsed_ns+1));
            let q=select(Endpoint {monotonic_ns:None,..a},Endpoint {monotonic_ns:None,..b},t).unwrap();
            assert_eq!(q.true_midpoint_monotonic_ns,None);assert!(can_start(q,b.elapsed_ns));
        }
        #[test]
        fn chronological_pair_chain_has_no_source_or_midpoint_collision() {
            let t=20_833_333;let mut source_slots=std::collections::BTreeSet::new();let mut mids=std::collections::BTreeSet::new();
            let mut older=endpoint(1,1,1,t,123);
            source_slots.insert(older.slot*2);
            for (index,gap) in [1,2,3,1,5,2,6,1,2,3].into_iter().cycle().take(1000).enumerate() {
                let next=endpoint(older.slot+gap,index as u64+2,index as u64+2,t,123);
                let p=select(older,next,t).unwrap();assert!(mids.insert(p.midpoint_slot));
                source_slots.insert(next.slot*2);older=next;
            }
            assert!(source_slots.is_disjoint(&mids));
        }
        #[test]
        fn late_midpoint_window_is_open_only_before_the_real_newer_source() {
            assert_eq!(admit(3,2,Some(6),Some(0),false),Admission::Future);
            assert_eq!(admit(3,3,Some(6),Some(0),false),Admission::OnTime);
            for current in [4,5] {assert_eq!(admit(3,current,Some(6),Some(0),false),Admission::Late);}
            for current in [6,7,u64::MAX] {assert_eq!(admit(3,current,Some(6),Some(0),false),Admission::Expired);}
            assert_eq!(admit(3,4,Some(6),Some(4),false),Admission::Stale);
            assert_eq!(admit(3,4,Some(6),Some(3),true),Admission::Stale);
            // Adjacent old0/new1 can still use only the original midpoint slot1.
            assert_eq!(admit(1,1,Some(2),Some(0),false),Admission::OnTime);
            assert_eq!(admit(1,2,Some(2),Some(0),false),Admission::Expired);
        }
        #[test]
        fn source_ai_repeat_sequence_never_rewinds_or_uses_a_future_frame() {
            // One real late result arrives at slot4 for old0/new3(midpoint3).
            let mut events=std::collections::BTreeMap::from([(0,("source",None)),(3,("ai",Some(6))),
                (6,("source",None)),(8,("ai",Some(10))),(10,("source",None))]);
            let mut last=None;let mut emitted=Vec::new();
            for current in [0,1,2,4,5,6,7,8,9,10] {
                let mut selected=None;
                while events.first_key_value().is_some_and(|(&slot,_)|slot<=current) {
                    let (slot,(kind,end))=events.pop_first().unwrap();
                    let previous=selected.as_ref().map(|(slot,_)|*slot).or(last);
                    if matches!(admit(slot,current,end,previous,false),Admission::OnTime|Admission::Late) {
                        selected=Some((slot,kind));
                    }
                }
                if let Some((slot,kind))=selected {last=Some(slot);emitted.push((current,slot,kind));}
                else {emitted.push((current,last.unwrap(),"repeat"));}
            }
            assert_eq!(emitted,vec![(0,0,"source"),(1,0,"repeat"),(2,0,"repeat"),(4,3,"ai"),(5,3,"repeat"),
                (6,6,"source"),(7,6,"repeat"),(8,8,"ai"),(9,8,"repeat"),(10,10,"source")]);
            assert!(emitted.windows(2).all(|pair|pair[0].1<=pair[1].1));
        }
        #[test]
        fn latest_chronological_source_beats_expired_or_older_midpoint() {
            let mut last=Some(0);
            for (slot,end) in [(3,Some(6)),(6,None)] {
                if matches!(admit(slot,6,end,last,false),Admission::OnTime|Admission::Late) {last=Some(slot);}
            }
            assert_eq!(last,Some(6));
            assert_eq!(admit(3,7,Some(10),last,false),Admission::Stale);
            assert_eq!(admit(4,7,None,last,false),Admission::Stale);
            assert_eq!(admit(8,8,Some(10),Some(6),true),Admission::Stale);
        }
        #[test]
        fn late_counter_requires_a_successful_novel_ai_presentation() {
            assert!(is_late_ai_present(true,true,3,4));
            for (success,is_ai,nominal,actual) in [(false,true,3,4),(true,false,3,4),
                (true,true,3,3),(true,true,3,2),(false,false,3,9)] {
                assert!(!is_late_ai_present(success,is_ai,nominal,actual));
            }
        }
        #[test]
        fn paused_or_retired_inputs_cannot_start_or_publish_inference() {
            assert!(work_is_current(true,false,4,4,7,7));
            for (allowed,stopped,generation,epoch) in [(false,false,4,7),(true,true,4,7),
                (true,false,5,7),(true,false,6,7),(true,false,4,8)] {
                assert!(!work_is_current(allowed,stopped,4,generation,7,epoch));
            }
            // The same test gates completion: an in-flight operation's return
            // after a quick background+foreground transition cannot publish.
            assert!(!work_is_current(true,false,4,6,7,7));
        }
        #[test]
        fn quick_background_foreground_resets_even_when_visibility_matches() {
            assert!(!context_changed(4,4,true,true,(1200,800),(1200,800)));
            assert!(context_changed(4,6,true,true,(1200,800),(1200,800)));
            assert!(context_changed(4,4,true,false,(1200,800),(1200,800)));
            assert!(context_changed(4,4,true,true,(1200,800),(800,1200)));
        }
        #[test]
        fn retired_readback_error_does_not_fail_the_fresh_epoch() {
            // The real collect path checks this predicate before inspecting
            // callback failure/timeout or touching mapped pixel data.
            for ready_ok in [false,true] {
                let old=work_is_current(true,false,4,6,7,8);
                let fresh=work_is_current(true,false,6,6,8,8);
                assert!(!(old&&!ready_ok));
                assert_eq!(fresh&&!ready_ok,!ready_ok);
            }
        }
    }
}
pub(crate) fn monotonic_ns()->Option<u64> {
    let mut time:libc::timespec=unsafe{std::mem::zeroed()};
    if unsafe{libc::clock_gettime(libc::CLOCK_MONOTONIC,&mut time)}!=0{return None;}
    u64::try_from(time.tv_sec).ok()?.checked_mul(1_000_000_000)?.checked_add(u64::try_from(time.tv_nsec).ok()?)
}

/// Diagnostic only: bounded wall/CPU spans, with all JSON/file IO on a writer
/// thread. Disabled unless the app-private config explicitly requests it.
pub(crate) mod slow_trace {
    use std::{cell::RefCell,io::Write,path::Path,sync::{Arc,atomic::{AtomicBool,AtomicU64,Ordering},mpsc::{self,SyncSender}}};
    const THRESHOLD_NS:u64=10_000_000;
    // Include loading plus a moving-world diagnostic window; stop permanently
    // at this fixed bound, even if normal paced parent spans exceed 10ms.
    const EVENT_LIMIT:u64=32768;
    const QUEUE_LIMIT:usize=128;
    static ENABLED:AtomicBool=AtomicBool::new(false);
    #[derive(Clone,Copy,Default)]
    struct Context { opcode:Option<u32>,words:Option<u64>,head:Option<i32>,tail:Option<i32>,
        epoch:Option<u64>,guest_sources:Option<u64>,selected_sources:Option<u64>,mapped_sequence:Option<u64> }
    struct Event { id:u64,parent:Option<u64>,stage:&'static str,label:String,begin:u64,end:u64,cpu:Option<u64>,before:Context,after:Context,
        guest_before:u64,guest_after:u64,presents_before:u64,presents_after:u64,queued:u64 }
    struct Trace { sender:SyncSender<Event>,drops:Arc<AtomicU64>,next_id:u64,parent:Option<u64>,queued:u64,context:Context }
    thread_local! { static TRACE:RefCell<Option<Trace>>=const{RefCell::new(None)}; }
    struct Mark { id:u64,parent:Option<u64>,stage:&'static str,label:String,begin:u64,cpu:Option<u64>,before:Context,guest:u64,presents:u64 }
    pub(crate) struct Span(Option<Mark>);
    #[inline] pub(crate) fn enabled()->bool {ENABLED.load(Ordering::Relaxed)}
    fn cpu_ns()->Option<u64> {
        let mut t:libc::timespec=unsafe{std::mem::zeroed()};
        if unsafe{libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID,&mut t)}!=0{return None;}
        u64::try_from(t.tv_sec).ok()?.checked_mul(1_000_000_000)?.checked_add(u64::try_from(t.tv_nsec).ok()?)
    }
    pub(crate) fn initialize() {
        let setup=||->Result<(),String> {
            let Some(root)=std::env::var_os("MUGUET_SUPPORT") else{return Ok(());};
            let config=match std::fs::read(Path::new(&root).join("config.json")) {
                Ok(bytes)=>serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|e|e.to_string())?,
                Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(()),Err(e)=>return Err(e.to_string()),
            };
            match config.get("frame_interpolation_slow_trace") {
                None|Some(serde_json::Value::Bool(false))=>return Ok(()),Some(serde_json::Value::Bool(true))=>{},
                _=>return Err("frame_interpolation_slow_trace must be boolean".into()),
            }
            let root=std::fs::canonicalize(root).map_err(|e|e.to_string())?;
            if !root.is_dir(){return Err("app support root must be an existing directory".into());}
            let path=root.join(format!("gpu-slow-operations-{}.jsonl",std::process::id()));
            use std::os::unix::fs::OpenOptionsExt;
            let file=std::fs::OpenOptions::new().create_new(true).write(true).mode(0o600)
                .custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(&path).map_err(|e|e.to_string())?;
            let (sender,receiver)=mpsc::sync_channel::<Event>(QUEUE_LIMIT);
            let drops=Arc::new(AtomicU64::new(0));let writer_drops=drops.clone();
            std::thread::Builder::new().name("gpu-slow-trace".into()).spawn(move|| {
                let mut file=std::io::BufWriter::new(file);
                let write=||->std::io::Result<()> {
                    writeln!(file,"{}",serde_json::json!({"type":"header","pid":std::process::id(),"thresholdNsExclusive":THRESHOLD_NS,
                        "eventLimit":EVENT_LIMIT,"writerQueueLimit":QUEUE_LIMIT,"clock":"CLOCK_MONOTONIC_ns",
                        "cpuClock":"CLOCK_THREAD_CPUTIME_ID_ns","defaultEnabled":false,
                        "coverage":"targeted_stages_only_no_full_opcode_durations; legacy_output_status_stage_measures_GPU_snapshot_build_and_try_enqueue_only",
                        "scope":"diagnostic_nested_host_spans_not_GPU_execution_times; elapsedMinusThreadCpuNs_includes_waits_and_descheduling; source_counts_are_observed_boundaries_not_FPS; trace_may_perturb_timing"}))?;
                    file.flush()?;
                    for event in receiver {
                        let context=|c:Context|serde_json::json!({"opcode":c.opcode,"commandWords":c.words,"ringHead":c.head,"ringTail":c.tail,
                            "epoch":c.epoch,"guestSourceRecordCalls":c.guest_sources,"selectedSourceSnapshots":c.selected_sources,"lastMappedSourceSequence":c.mapped_sequence});
                        let wall=event.end-event.begin;
                        writeln!(file,"{}",serde_json::json!({"type":"slow_operation","pid":std::process::id(),"spanId":event.id,"parentSpanId":event.parent,
                            "stage":event.stage,"label":event.label,"beginMonotonicNs":event.begin,"endMonotonicNs":event.end,"wallNs":wall,
                            "threadCpuNs":event.cpu,"elapsedMinusThreadCpuNs":event.cpu.map(|cpu|wall.saturating_sub(cpu)),
                            "before":context(event.before),"after":context(event.after),
                            "completedGuestPresentSequenceBefore":event.guest_before,"completedGuestPresentSequenceAfter":event.guest_after,
                            "nativePresentCallsBefore":event.presents_before,"nativePresentCallsAfter":event.presents_after,
                            "queuedEventIndex":event.queued,"writerQueueDrops":writer_drops.load(Ordering::Relaxed),"eventLimitReached":event.queued==EVENT_LIMIT}))?;
                        file.flush()?;
                    }
                    Ok(())
                };
                if let Err(e)=write(){eprintln!("[gpu-slow-trace] writer stopped: {e}");ENABLED.store(false,Ordering::Relaxed);}
            }).map_err(|e|e.to_string())?;
            TRACE.with(|state|*state.borrow_mut()=Some(Trace{sender,drops,next_id:0,parent:None,queued:0,context:Context::default()}));
            ENABLED.store(true,Ordering::Relaxed);
            eprintln!("[gpu-slow-trace] enabled threshold>10ms limit={EVENT_LIMIT} queue={QUEUE_LIMIT} path={}",path.display());
            Ok(())
        };
        if let Err(e)=setup(){eprintln!("[gpu-slow-trace] disabled: {e}");}
    }
    #[inline] pub(crate) fn command(opcode:Option<u32>,words:Option<u64>,head:i32,tail:i32) {
        if enabled(){TRACE.with(|s|if let Some(t)=s.borrow_mut().as_mut(){t.context.opcode=opcode;t.context.words=words;t.context.head=Some(head);t.context.tail=Some(tail);});}
    }
    #[cfg_attr(not(feature="frame-interpolation"),allow(dead_code))]
    #[inline] pub(crate) fn source(epoch:u64,guest:u64,selected:u64,mapped:Option<u64>) {
        if enabled(){TRACE.with(|s|if let Some(t)=s.borrow_mut().as_mut(){t.context.epoch=Some(epoch);t.context.guest_sources=Some(guest);t.context.selected_sources=Some(selected);t.context.mapped_sequence=mapped;});}
    }
    #[inline] pub(crate) fn span(stage:&'static str,label:&str)->Span {
        if !enabled(){return Span(None);}
        Span(TRACE.with(|state|{
            let mut state=state.borrow_mut();let t=state.as_mut()?;
            let begin=super::monotonic_ns()?;let cpu=cpu_ns();t.next_id+=1;
            let mark=Mark{id:t.next_id,parent:t.parent,stage,label:label.chars().take(160).collect(),begin,cpu,before:t.context,
                guest:crate::gpu::GUEST_PRESENTS.load(Ordering::Relaxed),presents:crate::gpu::PRESENTS.load(Ordering::Relaxed)};
            t.parent=Some(mark.id);Some(mark)
        }))
    }
    impl Drop for Span {
        fn drop(&mut self) {
            let Some(mark)=self.0.take() else{return;};
            let end=super::monotonic_ns();let end_cpu=cpu_ns();
            TRACE.with(|state|{
                let mut state=state.borrow_mut();let Some(t)=state.as_mut() else{return;};t.parent=mark.parent;
                let Some(end)=end else{return;};
                if end.saturating_sub(mark.begin)<=THRESHOLD_NS||t.queued>=EVENT_LIMIT{return;}
                let event=Event{id:mark.id,parent:mark.parent,stage:mark.stage,label:mark.label,begin:mark.begin,end,
                    cpu:mark.cpu.zip(end_cpu).map(|(a,b)|b.saturating_sub(a)),before:mark.before,after:t.context,
                    guest_before:mark.guest,guest_after:crate::gpu::GUEST_PRESENTS.load(Ordering::Relaxed),
                    presents_before:mark.presents,presents_after:crate::gpu::PRESENTS.load(Ordering::Relaxed),queued:t.queued+1};
                match t.sender.try_send(event) {
                    Ok(())=>{t.queued+=1;if t.queued==EVENT_LIMIT{ENABLED.store(false,Ordering::Relaxed);}},
                    Err(mpsc::TrySendError::Full(_))=>{t.drops.fetch_add(1,Ordering::Relaxed);},
                    Err(mpsc::TrySendError::Disconnected(_))=>{ENABLED.store(false,Ordering::Relaxed);},
                }
            });
        }
    }
}

#[cfg(feature="frame-interpolation")]
mod status_writer {
    use serde_json::{json,Value};
    use std::{io::Write,path::{Path,PathBuf},sync::{Arc,atomic::{AtomicBool,AtomicU64,Ordering},mpsc::{self,SyncSender,TrySendError}}};
    use std::os::unix::fs::{OpenOptionsExt,PermissionsExt};
    const MAX_BYTES:usize=4*1024*1024;
    fn mode_for_monitor_flag(value:Option<&std::ffi::OsStr>)->u32 {
        if value==Some(std::ffi::OsStr::new("1")){0o660}else{0o600}
    }
    fn startup_file_mode()->u32 {
        static MODE:std::sync::OnceLock<u32>=std::sync::OnceLock::new();
        *MODE.get_or_init(||mode_for_monitor_flag(std::env::var_os("MUGUET_NATIVE_BENCHMARK_MONITOR").as_deref()))
    }
    #[derive(Default)]
    struct Stats {
        generated:AtomicU64,enqueued:AtomicU64,full_drops:AtomicU64,disconnected_drops:AtomicU64,
        serialize_errors:AtomicU64,oversize_drops:AtomicU64,io_errors:AtomicU64,spawn_errors:AtomicU64,
        published:AtomicU64,last_sequence:AtomicU64,last_complete_ns:AtomicU64,last_error:AtomicU64,
        running:AtomicBool,exited:AtomicBool,
    }
    struct Snapshot {value:Value,sequence:u64,began_ns:Option<u64>,generated_ns:Option<u64>}
    pub(super) struct Writer {sender:Option<SyncSender<Snapshot>>,stats:Arc<Stats>}
    struct Liveness(Arc<Stats>);
    impl Drop for Liveness {fn drop(&mut self){self.0.running.store(false,Ordering::Release);self.0.exited.store(true,Ordering::Release);}}
    impl Writer {
        pub(super) fn new(root:PathBuf)->Self {
            let mode=startup_file_mode();
            Self::with_sink(move|snapshot,stats|publish(&root,snapshot,stats,mode))
        }
        fn with_sink(mut sink:impl FnMut(Snapshot,&Stats)+Send+'static)->Self {
            let stats=Arc::new(Stats::default());let (sender,receiver)=mpsc::sync_channel(1);let worker_stats=stats.clone();
            match std::thread::Builder::new().name("game-frame-status".into()).spawn(move|| {
                let _liveness=Liveness(worker_stats.clone());worker_stats.running.store(true,Ordering::Release);
                // Closing the only sender drains the one queued snapshot. No
                // GPU-thread join or wait is needed, including Interpolator drop.
                for snapshot in receiver {sink(snapshot,&worker_stats);}
            }) {
                Ok(_)=>Self{sender:Some(sender),stats},
                Err(error)=>{stats.spawn_errors.fetch_add(1,Ordering::Relaxed);stats.last_error.store(4,Ordering::Relaxed);
                    stats.exited.store(true,Ordering::Release);eprintln!("[frame-interpolation] state writer unavailable: {error}");Self{sender:None,stats}},
            }
        }
        pub(super) fn submit(&self,value:Value,began_ns:Option<u64>,generated_ns:Option<u64>) {
            let sequence=self.stats.generated.fetch_add(1,Ordering::Relaxed)+1;
            let Some(sender)=self.sender.as_ref() else{self.stats.disconnected_drops.fetch_add(1,Ordering::Relaxed);return;};
            match sender.try_send(Snapshot{value,sequence,began_ns,generated_ns}) {
                Ok(())=>{self.stats.enqueued.fetch_add(1,Ordering::Relaxed);},
                Err(TrySendError::Full(_))=>{self.stats.full_drops.fetch_add(1,Ordering::Relaxed);},
                Err(TrySendError::Disconnected(_))=>{self.stats.disconnected_drops.fetch_add(1,Ordering::Relaxed);self.stats.last_error.store(5,Ordering::Relaxed);},
            }
        }
    }
    fn report_error(stats:&Stats,code:u64,error:impl std::fmt::Display) {
        stats.last_error.store(code,Ordering::Relaxed);
        let count=match code {1=>stats.serialize_errors.fetch_add(1,Ordering::Relaxed)+1,
            2=>stats.oversize_drops.fetch_add(1,Ordering::Relaxed)+1,_=>stats.io_errors.fetch_add(1,Ordering::Relaxed)+1};
        if count<=3||count.is_power_of_two(){eprintln!("[frame-interpolation] state writer error code={code} count={count}: {error}");}
    }
    fn publish(root:&Path,mut snapshot:Snapshot,stats:&Stats,mode:u32) {
        let started_ns=super::monotonic_ns();
        let Some(object)=snapshot.value.as_object_mut() else{report_error(stats,1,"snapshot is not an object");return;};
        object.insert("statusSnapshotSequence".into(),json!(snapshot.sequence));
        object.insert("statusSnapshotGenerationBeginMonotonicNs".into(),json!(snapshot.began_ns));
        object.insert("statusSnapshotGeneratedMonotonicNs".into(),json!(snapshot.generated_ns));
        object.insert("statusSnapshotGenerationNs".into(),json!(snapshot.began_ns.zip(snapshot.generated_ns).map(|(a,b)|b.saturating_sub(a))));
        object.insert("statusWriterSerializationAndWriteBeginMonotonicNs".into(),json!(started_ns));
        object.insert("statusWriter".into(),json!({"path":"bounded_async_single_worker_atomic_snapshot","queueCapacity":1,"maxSerializedBytes":MAX_BYTES,
            "snapshotsGenerated":stats.generated.load(Ordering::Relaxed),"snapshotsEnqueued":stats.enqueued.load(Ordering::Relaxed),
            "queueFullSnapshotDrops":stats.full_drops.load(Ordering::Relaxed),"disconnectedSnapshotDrops":stats.disconnected_drops.load(Ordering::Relaxed),
            "serializationErrors":stats.serialize_errors.load(Ordering::Relaxed),"oversizeSnapshotDrops":stats.oversize_drops.load(Ordering::Relaxed),
            "fileWriteErrors":stats.io_errors.load(Ordering::Relaxed),"workerSpawnErrors":stats.spawn_errors.load(Ordering::Relaxed),
            "publishedSnapshots":stats.published.load(Ordering::Relaxed),"lastCompletedSnapshotSequence":stats.last_sequence.load(Ordering::Relaxed),
            "lastPublishCompleteMonotonicNs":match stats.last_complete_ns.load(Ordering::Relaxed){0=>None,n=>Some(n)},
            "lastErrorCode":stats.last_error.load(Ordering::Relaxed),"workerRunning":stats.running.load(Ordering::Acquire),
            "timestampScope":"snapshot_generation_is_GPU_thread; serialization_write_begin_is_this_snapshot_on_worker; last_publish_complete_refers_to_lastCompletedSnapshotSequence_before_this_write; counters_are_worker_observations_not_render_statistics"}));
        let mut bytes=match serde_json::to_vec(&snapshot.value){Ok(bytes)=>bytes,Err(error)=>{report_error(stats,1,error);return;}};
        if bytes.len()>=MAX_BYTES{report_error(stats,2,"serialized snapshot exceeds 4MiB bound");return;}
        bytes.push(b'\n');
        let tmp=root.join(format!(".frame-interpolation-status-{}.json.tmp",std::process::id()));
        let write=||->std::io::Result<()> {
            let mut file=std::fs::OpenOptions::new().create(true).truncate(true).write(true)
                .custom_flags(libc::O_CLOEXEC|libc::O_NOFOLLOW).mode(mode).open(&tmp)?;
            file.set_permissions(std::fs::Permissions::from_mode(mode))?;
            // One contiguous write_all, not serde Display's per-token File writes.
            file.write_all(&bytes)?;drop(file);
            std::fs::rename(&tmp,root.join("frame-interpolation-status.json"))
        };
        if let Err(error)=write(){report_error(stats,3,error);return;}
        stats.published.fetch_add(1,Ordering::Relaxed);stats.last_sequence.store(snapshot.sequence,Ordering::Relaxed);
        stats.last_complete_ns.store(super::monotonic_ns().unwrap_or(0),Ordering::Relaxed);
    }
}

// A normal runtime has no references to the optional SDK's native symbols.
#[cfg(not(feature = "frame-interpolation"))]
mod implementation {
    use super::*;
    pub(crate) struct Interpolator;
    impl Interpolator {
        pub(crate) fn from_env() -> Option<Self> {
            note_live(0);
            if !crate::config::frame_interpolation_enabled() { return None; }
            if std::env::var("MG_FRAME_INTERPOLATION").is_ok_and(|v| v == "rife" || v == "npu") {
                eprintln!("[frame-interpolation] unavailable: runtime was built without frame-interpolation feature");
            }
            None
        }
        pub(crate) fn cache_epoch(&self)->u64 {0}
        pub(crate) fn active(&self) -> bool { false }
        pub(crate) fn unlimited_native(&self)->bool {false}
        pub(crate) fn context(&mut self, _: bool, _: (u32,u32)) {}
        pub(crate) fn record(&mut self, _: &Device, _: &Texture, _: u64) -> Option<CommandBuffer> { None }
        pub(crate) fn tick(&mut self, _: &Device) -> Option<Display> { None }
        pub(crate) fn prepare_output(&mut self, _: &Device, _: &Queue, _: &Display, _: &mut CommandEncoder) -> Option<Texture> { None }
        pub(crate) fn presented(&mut self, _: &Display, _: bool, _: Option<u64>, _: Option<u64>) {}
        pub(crate) fn output_skipped(&mut self, _: &str) {}
        pub(crate) fn output_failed(&mut self,_:String) {}
        pub(crate) fn output_size(&mut self, _: (u32,u32), _: bool) {}
        pub(crate) fn validate_adapter(&mut self, _: &wgpu::Adapter) {}
        pub(crate) fn note_output_capture(&mut self, _: &str, _: &Display) {}
        pub(crate) fn wait(&self) -> Duration { Duration::from_millis(100) }
        pub(crate) fn source_wait(&self) -> Duration { Duration::ZERO }
        pub(crate) fn update_slow_trace_context(&self) {}
    }
}

#[cfg(feature = "frame-interpolation")]
mod implementation {
    use super::*;
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use std::{collections::{BTreeMap,VecDeque}, ffi::{c_char,c_void,CStr,CString},
        path::{Path,PathBuf},
        sync::{Mutex,atomic::{AtomicBool,AtomicU64,Ordering},mpsc::{self,Receiver,SyncSender,TryRecvError,TrySendError}}};
    use wgpu::*;
    use wgpu::util::DeviceExt;

    const DELAY: Duration = Duration::from_nanos(super::pairing::DELAY_NS);
    const GUARD: usize = 64;
    const RGB_BYTES: usize = WIDTH as usize * HEIGHT as usize * 3;
    const MODEL_WIDTH:u32=448;
    const MODEL_HEIGHT:u32=208;
    const MODEL_PIXELS:usize=MODEL_WIDTH as usize*MODEL_HEIGHT as usize;
    const HALF_ELEMENTS:usize=MODEL_PIXELS*6;
    const MODEL_INPUT_BYTES:usize=MODEL_PIXELS*4;
    const PENDING_LIMIT: usize = 3;
    const EVENT_LIMIT: usize = 12;
    const TRACE_LIMIT: usize = 180;
    const SENTINEL: u8 = 0xa7;
    const CLASSIFIER_CONSERVATIVE:&str="conservative_reject_second_gt_0_99";
    const CLASSIFIER_OBSERVE:&str="observe_only";
    const SHADER: &str = "@group(0) @binding(0) var src: texture_2d<f32>;
      @vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4f {
        var p=array<vec2f,3>(vec2f(-1,-1),vec2f(3,-1),vec2f(-1,3));return vec4f(p[i],0,1);}
      @fragment fn fs(@builtin(position) pos:vec4f)->@location(0) vec4f {
        let dims=vec2i(textureDimensions(src));let p=pos.xy*vec2f(dims)/vec2f(320,192)-vec2f(0.5);
        let at=vec2i(floor(p));let f=fract(p);let hi=dims-vec2i(1);
        let a=textureLoad(src,clamp(at,vec2i(0),hi),0);let b=textureLoad(src,clamp(at+vec2i(1,0),vec2i(0),hi),0);
        let c=textureLoad(src,clamp(at+vec2i(0,1),vec2i(0),hi),0);let d=textureLoad(src,clamp(at+vec2i(1,1),vec2i(0),hi),0);
        return vec4f(mix(mix(a.rgb,b.rgb,f.x),mix(c.rgb,d.rgb,f.x),f.y),1);}";
    const FLOW_TRANSFORM_SHADER:&str="@group(0) @binding(0) var<storage,read> raw:array<u32>;
      @vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4f {
        var p=array<vec2f,3>(vec2f(-1,-1),vec2f(3,-1),vec2f(-1,3));return vec4f(p[i],0,1);}
      fn field(channel:u32,pixel:u32)->f32 {let index=channel*93184u+pixel;
        let pair=unpack2x16float(raw[index/2u]);return select(pair.x,pair.y,(index&1u)!=0u);}
      fn sigmoid(v:f32)->f32 {if v>=0 {return 1.0/(1.0+exp(-v));}let e=exp(v);return e/(1.0+e);}
      struct Fields {@location(0) flow:vec4f,@location(1) masks:vec2f}
      @fragment fn fs(@builtin(position) pos:vec4f)->Fields {let p=u32(pos.y)*448u+u32(pos.x);
        var o:Fields;o.flow=vec4f(field(0u,p),field(1u,p),field(2u,p),field(3u,p));
        o.masks=vec2f(sigmoid(field(4u,p)),sigmoid(field(5u,p)));return o;}";
    const WARP_SHADER:&str="@group(0) @binding(0) var older:texture_2d<f32>;
      @group(0) @binding(1) var newer:texture_2d<f32>;
      @group(0) @binding(2) var flow:texture_2d<f32>;
      @group(0) @binding(3) var masks:texture_2d<f32>;
      @group(0) @binding(4) var linear_sampler:sampler;
      @vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4f {
        var p=array<vec2f,3>(vec2f(-1,-1),vec2f(3,-1),vec2f(-1,3));return vec4f(p[i],0,1);}
      @fragment fn fs(@builtin(position) pos:vec4f)->@location(0) vec4f {
        let uv=pos.xy/vec2f(textureDimensions(newer));
        let f=textureSampleLevel(flow,linear_sampler,uv,0);
        let m=textureSampleLevel(masks,linear_sampler,uv,0).rg;
        let a=textureSampleLevel(older,linear_sampler,uv+f.xy/vec2f(448,208),0).rgb;
        let b=textureSampleLevel(newer,linear_sampler,uv+f.zw/vec2f(448,208),0).rgb;
        let n=textureSampleLevel(newer,linear_sampler,uv,0).rgb;
        return vec4f(m.y*n+(1.0-m.y)*(m.x*a+(1.0-m.x)*b),1);}";
    fn rgba_pipeline(device:&Device,label:&str,source:&str,formats:&[TextureFormat])->RenderPipeline {
        let shader=device.create_shader_module(ShaderModuleDescriptor {label:Some(label),source:ShaderSource::Wgsl(source.into())});
        let targets:Vec<_>=formats.iter().map(|&format|Some(ColorTargetState {format,blend:None,write_mask:ColorWrites::ALL})).collect();
        device.create_render_pipeline(&RenderPipelineDescriptor {label:Some(label),layout:None,
            vertex:VertexState {module:&shader,entry_point:Some("vs"),compilation_options:Default::default(),buffers:&[]},
            primitive:PrimitiveState::default(),depth_stencil:None,multisample:MultisampleState::default(),
            fragment:Some(FragmentState {module:&shader,entry_point:Some("fs"),compilation_options:Default::default(),targets:&targets}),
            multiview_mask:None,cache:None})
    }
    fn texture(device:&Device,label:&str,width:u32,height:u32,format:TextureFormat,usage:TextureUsages)->Texture {
        device.create_texture(&TextureDescriptor {label:Some(label),size:Extent3d{width,height,depth_or_array_layers:1},
            mip_level_count:1,sample_count:1,dimension:TextureDimension::D2,format,usage,view_formats:&[]})
    }
    fn draw(enc:&mut CommandEncoder,label:&str,pipeline:&RenderPipeline,binding:&BindGroup,target:&TextureView) {
        let mut p=enc.begin_render_pass(&RenderPassDescriptor {label:Some(label),
            color_attachments:&[Some(RenderPassColorAttachment {view:target,depth_slice:None,resolve_target:None,ops:Operations {load:LoadOp::Clear(Color::BLACK),store:StoreOp::Store}})],
            depth_stencil_attachment:None,timestamp_writes:None,occlusion_query_set:None,multiview_mask:None});
        p.set_pipeline(pipeline);p.set_bind_group(0,binding,&[]);p.draw(0..3,0..1);
    }

    #[repr(C)]
    struct DeviceInfo {
        gpu_index:i32, api_version:u32, driver_version:u32, vendor_id:u32,device_id:u32,
        fp16_packed:i32, fp16_storage:i32, fp16_arithmetic:i32,int8_storage:i32,
        name:[c_char;256],
    }
    #[repr(C)]
    struct FlowInfo {
        struct_size:u32,abi_version:u32,width:u32,height:u32,input_bytes_per_frame:u32,flow_half_elements:u32,
        logit_elements:u32,npu_only_fallback_disabled:u32,backend_id:u64,successful_runs:u64,last_run_sync_ns:u64,
        last_run_sync_code:i32,input_data_type:i32,flow_data_type:i32,logit_data_type:i32,tensor_format:i32,reserved:u32,
        backend_name:[c_char;64],cann_version:[c_char;128],input_names:[[c_char;64];2],output_names:[[c_char;64];2],
    }
    const _:()=assert!(std::mem::size_of::<FlowInfo>()==528);
    unsafe extern "C" {
        fn muguet_interpolation_create(model:*const c_char,gpu:i32,error:*mut c_char,capacity:usize)->*mut c_void;
        fn muguet_interpolation_process_rgb8(ctx:*mut c_void,a:*const u8,b:*const u8,width:i32,height:i32,
            output:*mut u8,bytes:usize,t:f32)->i32;
        fn muguet_interpolation_get_device_info(ctx:*mut c_void,info:*mut DeviceInfo)->i32;
        fn muguet_interpolation_destroy(ctx:*mut c_void);
        fn muguet_interpolation_build_revision()->*const c_char;
        fn muguet_interpolation_rife_available()->i32;
        fn muguet_fg_npu_flow_create(context:*mut *mut c_void)->i32;
        fn muguet_fg_npu_flow_process_rgba8(context:*mut c_void,input0:*const u8,bytes0:usize,input1:*const u8,bytes1:usize,
            half:*mut u16,half_capacity:usize,logits:*mut f32,logit_capacity:usize)->i32;
        fn muguet_fg_npu_flow_get_info(context:*const c_void,info:*mut FlowInfo,info_bytes:usize)->i32;
        fn muguet_fg_npu_flow_last_error(context:*const c_void)->*const c_char;
        fn muguet_fg_npu_flow_destroy(context:*mut c_void);
        fn muguet_fg_npu_rgb_count_nonfinite_half(values:*const u16,count:usize,nonfinite:*mut u64)->i32;
    }
    struct Context(*mut c_void,bool);
    impl Drop for Context { fn drop(&mut self) { unsafe {if self.1 {muguet_fg_npu_flow_destroy(self.0)}else{muguet_interpolation_destroy(self.0)}} } }
    #[derive(Clone)]
    struct Source { endpoint:super::pairing::Endpoint,origin:Instant,period_ns:u64,native_deadline_ns:Option<u64>,rgb:Arc<Vec<u8>> }
    enum WorkerResult {
        Started(Value),
        Mid { pair:super::pairing::Pair,rgb:Arc<Vec<u8>>,milliseconds:f64,different_pixels:usize },
        Flow { pair:super::pairing::Pair,half:Arc<Vec<u16>>,milliseconds:f64 },
        Rejected { pair:super::pairing::Pair,code:i32 },
        InferenceFailed {pair:super::pairing::Pair,error:String},
        Failed(String),
    }
    #[derive(Clone,Copy)]
    struct ClassifierObservation { pair:super::pairing::Pair,logits:[f32;2],probability_second:f64,high_score:bool,rejected:bool,policy:&'static str }
    #[derive(Default)]
    struct WorkerStats {
        completed:AtomicU64,result_drops:AtomicU64,stale_results:AtomicU64,rejected_mids:AtomicU64,
        last_run_sync_ns:AtomicU64,last_finite_scan_ns:AtomicU64,high_score_pairs:AtomicU64,
        pair_candidates:AtomicU64,nonadjacent_pairs:AtomicU64,pair_gap_skips:AtomicU64,
        pair_invalid_skips:AtomicU64,pair_discontinuity_skips:AtomicU64,pair_deadline_skips:AtomicU64,pair_runs:AtomicU64,
        presentation_gate_drops:AtomicU64,
        classifier:Mutex<VecDeque<ClassifierObservation>>,
    }
    fn presentation_current(generation:u64,source_epoch:u64,epoch:&AtomicU64,stop:&AtomicBool)->bool {
        let current_generation=crate::ffi::app_presentation_generation();
        super::pairing::work_is_current(crate::ffi::app_presentation_allowed(),stop.load(Ordering::Acquire),
            generation,current_generation,source_epoch,epoch.load(Ordering::Acquire))&&
            crate::ffi::app_presentation_generation()==current_generation
    }
    fn model_hash(path:&Path,expected:&str)->Result<(),String> {
        let bytes=std::fs::read(path).map_err(|e|format!("read pinned model {}: {e}",path.display()))?;
        if format!("{:x}",Sha256::digest(bytes))!=expected { return Err(format!("pinned model hash mismatch: {}",path.display())); }
        Ok(())
    }
    fn configured_options(root:&Path,mode:&str)->Result<(u32,&'static str),String> {
        let config=match std::fs::read(root.join("config.json")) {
            Ok(bytes)=>serde_json::from_slice::<Value>(&bytes).map_err(|e|format!("interpolation clock config JSON: {e}"))?,
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok((30,CLASSIFIER_CONSERVATIVE)),
            Err(e)=>return Err(format!("interpolation clock config read: {e}")),
        };
        let output_fps=match config.get("frame_interpolation_output_fps") {
            None=>30,Some(requested)=>match requested.as_u64() {
                Some(30)=>30,Some(32) if mode=="npu"=>32,Some(40) if mode=="npu"=>40,Some(48) if mode=="npu"=>48,
                _=>return Err(format!("frame_interpolation_output_fps requires integer 30/32/40/48 for NPU, or 30 for RIFE; requested={requested}")),
            }
        };
        let classifier_policy=match config.get("frame_interpolation_classifier_policy") {
            None=>CLASSIFIER_CONSERVATIVE,Some(policy)=>match policy.as_str() {
                Some(CLASSIFIER_CONSERVATIVE)=>CLASSIFIER_CONSERVATIVE,Some(CLASSIFIER_OBSERVE)=>CLASSIFIER_OBSERVE,
                _=>return Err(format!("frame_interpolation_classifier_policy requires {CLASSIFIER_CONSERVATIVE} or {CLASSIFIER_OBSERVE}; requested={policy}")),
            }
        };
        Ok((output_fps,classifier_policy))
    }
    fn worker(model:PathBuf,mode:&'static str,classifier_policy:&'static str,rx:Receiver<Source>,tx:SyncSender<WorkerResult>,epoch:Arc<AtomicU64>,stop:Arc<AtomicBool>,stats:Arc<WorkerStats>) {
        let started=Instant::now();
        let initialize=||->Result<(Context,Value),String> {
            if mode=="npu" {
                let mut ptr=std::ptr::null_mut();
                let result=unsafe {muguet_fg_npu_flow_create(&mut ptr)};
                if result!=0||ptr.is_null() {
                    let error=unsafe {muguet_fg_npu_flow_last_error(ptr)};
                    return Err(format!("raw NPU create returned {result}: {}",if error.is_null(){"unknown".into()}else{unsafe {CStr::from_ptr(error)}.to_string_lossy()}));
                }
                let ctx=Context(ptr,true);let mut info:FlowInfo=unsafe{std::mem::zeroed()};
                let result=unsafe{muguet_fg_npu_flow_get_info(ctx.0,&mut info,std::mem::size_of::<FlowInfo>())};
                if result!=0||info.width!=MODEL_WIDTH||info.height!=MODEL_HEIGHT||info.input_bytes_per_frame as usize!=MODEL_INPUT_BYTES||
                    info.flow_half_elements as usize!=HALF_ELEMENTS||info.logit_elements!=2||info.input_data_type!=6||info.flow_data_type!=10||info.logit_data_type!=11 {
                    return Err(format!("raw NPU schema mismatch: getInfo={result} width={} height={} halfCount={} types={}/{}/{}",
                        info.width,info.height,info.flow_half_elements,info.input_data_type,info.flow_data_type,info.logit_data_type));
                }
                info.backend_name[63]=0;info.cann_version[127]=0;
                return Ok((ctx,json!({"createWallMs":started.elapsed().as_secs_f64()*1000.0,"backend":"npu_raw_flow",
                    "deviceName":unsafe{CStr::from_ptr(info.backend_name.as_ptr())}.to_string_lossy(),"backendId":info.backend_id,
                    "cannVersion":unsafe{CStr::from_ptr(info.cann_version.as_ptr())}.to_string_lossy(),
                    "width":info.width,"height":info.height,"halfElements":info.flow_half_elements,
                    "npuOnlyFallbackDisabledConfiguration":info.npu_only_fallback_disabled,
                    "hardwarePlacementIndependentlyVerified":false,"cpuWarpEnabled":false,"gpuWarpEnabled":true,
                    "contextOwnership":"public_NNRT_CANN_rawflow_worker_context"})));
            }
            if mode=="rife" && unsafe {muguet_interpolation_rife_available()}==0 {
                return Err("RIFE unavailable: this build contains NPU frame interpolation only".into());
            }
            if mode=="rife" { model_hash(&model.join("flownet.param"),"724569596bcd1e7b9fa50455c604777ebed99746d2ef40aa86e31b5725f1053c")?;
            model_hash(&model.join("flownet.bin"),"f334ed2260149ce0188a6dcf049844e8b0cdd912e01cbcfb63553157d2508958")?; }
            let model=CString::new(model.as_os_str().as_encoded_bytes()).map_err(|e|format!("model path: {e}"))?;
            let mut error=[0 as c_char;1024];
            let ptr=unsafe {muguet_interpolation_create(model.as_ptr(),-1,error.as_mut_ptr(),error.len())};
            if ptr.is_null() { return Err(format!("{mode} create: {}",unsafe {CStr::from_ptr(error.as_ptr())}.to_string_lossy())); }
            let ctx=Context(ptr,false);
            let mut info:DeviceInfo=unsafe {std::mem::zeroed()};
            let result=unsafe {muguet_interpolation_get_device_info(ctx.0,&mut info)};
            if result!=0 { return Err(format!("{mode} get_device_info returned {result}")); }
            info.name[255]=0;
            let revision=unsafe {muguet_interpolation_build_revision()};
            Ok((ctx,json!({"createWallMs":started.elapsed().as_secs_f64()*1000.0,
                "gpuIndex":info.gpu_index,"deviceName":unsafe {CStr::from_ptr(info.name.as_ptr())}.to_string_lossy(),
                "apiVersion":info.api_version,"driverVersion":info.driver_version,"vendorId":info.vendor_id,"deviceId":info.device_id,
                "fp16Packed":info.fp16_packed,"fp16Storage":info.fp16_storage,"fp16Arithmetic":info.fp16_arithmetic,
                "int8Storage":info.int8_storage,"revision":if revision.is_null(){"unknown".into()}else{unsafe {CStr::from_ptr(revision)}.to_string_lossy()},
                "pinnedRifeModelHashesVerified":mode=="rife","backend":mode,"contextOwnership":if mode=="rife"{"independent_RIFE_Vulkan_device_worker_thread"}else{"public_NNRT_NPU_worker_context"}})))
        };
        let (ctx,info)=match initialize() { Ok(v)=>v,Err(error)=>{let _=tx.send(WorkerResult::Failed(error));return;} };
        if tx.send(WorkerResult::Started(info)).is_err() { return; }
        let mut previous:Option<Source>=None;
        while !stop.load(Ordering::Acquire) {
            let source=match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(v)=>v,Err(mpsc::RecvTimeoutError::Timeout)=>continue,Err(mpsc::RecvTimeoutError::Disconnected)=>return,
            };
            if !presentation_current(source.endpoint.presentation_generation,source.endpoint.epoch,&epoch,&stop) {
                stats.presentation_gate_drops.fetch_add(1,Ordering::Relaxed);previous=None;continue;
            }
            if previous.as_ref().is_some_and(|previous|previous.endpoint.presentation_generation!=source.endpoint.presentation_generation||
                previous.endpoint.epoch!=source.endpoint.epoch) {previous=None;}
            if let Some(a)=previous.as_ref() {
                stats.pair_candidates.fetch_add(1,Ordering::Relaxed);
                let selection=match source.native_deadline_ns {Some(deadline)=>super::pairing::select_variable(a.endpoint,source.endpoint,deadline),None=>super::pairing::select(a.endpoint,source.endpoint,source.period_ns)};
                let pair=match selection {
                    Ok(pair) if a.origin==source.origin&&a.period_ns==source.period_ns=>pair,
                    result=>{
                        let counter=match result {
                            Err(super::pairing::Skip::Gap)=>&stats.pair_gap_skips,
                            Err(super::pairing::Skip::Discontinuity)=>&stats.pair_discontinuity_skips,
                            _=>&stats.pair_invalid_skips,
                        };
                        counter.fetch_add(1,Ordering::Relaxed);previous=Some(source);continue;
                    }
                };
                let Some(now_elapsed_ns)=Instant::now().checked_duration_since(source.origin)
                    .and_then(|duration|u64::try_from(duration.as_nanos()).ok()) else {
                    stats.pair_invalid_skips.fetch_add(1,Ordering::Relaxed);previous=Some(source);continue;
                };
                if !super::pairing::can_start(pair,now_elapsed_ns) {
                    stats.pair_deadline_skips.fetch_add(1,Ordering::Relaxed);previous=Some(source);continue;
                }
                if ctx.1 {
                    let mut half=vec![0x5a37u16;HALF_ELEMENTS+GUARD];
                    let sentinel=f32::from_bits(0x4e17bdc1);let mut logits=[sentinel;4];
                    if !presentation_current(pair.presentation_generation,pair.epoch,&epoch,&stop) {
                        stats.presentation_gate_drops.fetch_add(1,Ordering::Relaxed);previous=None;continue;
                    }
                    if pair.newer_slot-pair.older_slot>1 {stats.nonadjacent_pairs.fetch_add(1,Ordering::Relaxed);}
                    stats.pair_runs.fetch_add(1,Ordering::Relaxed);
                    let began=Instant::now();
                    let result=unsafe {muguet_fg_npu_flow_process_rgba8(ctx.0,a.rgb.as_ptr(),a.rgb.len(),source.rgb.as_ptr(),source.rgb.len(),
                        half.as_mut_ptr().add(GUARD/2),HALF_ELEMENTS,logits.as_mut_ptr().add(1),2)};
                    if !presentation_current(pair.presentation_generation,pair.epoch,&epoch,&stop) {
                        stats.stale_results.fetch_add(1,Ordering::Relaxed);previous=None;continue;
                    }
                    let guards=half[..GUARD/2].iter().chain(&half[HALF_ELEMENTS+GUARD/2..]).all(|&v|v==0x5a37)&&
                        logits[0].to_bits()==sentinel.to_bits()&&logits[3].to_bits()==sentinel.to_bits();
                    if result!=0||!guards {
                        let error=unsafe{muguet_fg_npu_flow_last_error(ctx.0)};
                        if tx.send(WorkerResult::InferenceFailed {pair,error:format!("raw NPU process returned {result}, guards={guards}: {}",
                            if error.is_null(){"unknown".into()}else{unsafe{CStr::from_ptr(error)}.to_string_lossy()})}).is_err(){return;}
                        previous=None;continue;
                    }
                    let finite_began=Instant::now();let mut nonfinite=0u64;
                    let finite_result=unsafe{muguet_fg_npu_rgb_count_nonfinite_half(half.as_ptr().add(GUARD/2),HALF_ELEMENTS,&mut nonfinite)};
                    stats.last_finite_scan_ns.store(finite_began.elapsed().as_nanos().min(u64::MAX as u128) as u64,Ordering::Relaxed);
                    if finite_result!=0||nonfinite!=0||!logits[1].is_finite()||!logits[2].is_finite() {
                        if tx.send(WorkerResult::InferenceFailed {pair,error:format!("raw NPU nonfinite outputs: scan={finite_result}, halfCount={nonfinite}, logits={:?}",&logits[1..3])}).is_err(){return;}
                        previous=None;continue;
                    }
                    let mut info:FlowInfo=unsafe{std::mem::zeroed()};
                    if unsafe{muguet_fg_npu_flow_get_info(ctx.0,&mut info,std::mem::size_of::<FlowInfo>())}==0 {
                        stats.last_run_sync_ns.store(info.last_run_sync_ns,Ordering::Relaxed);
                    }
                    let maximum=logits[1].max(logits[2]) as f64;
                    let first=(logits[1] as f64-maximum).exp();let second=(logits[2] as f64-maximum).exp();
                    let probability_second=second/(first+second);let high_score=probability_second>0.99;
                    let rejected=high_score&&classifier_policy==CLASSIFIER_CONSERVATIVE;
                    if high_score {stats.high_score_pairs.fetch_add(1,Ordering::Relaxed);}
                    {
                        let mut observation=stats.classifier.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        if observation.len()==TRACE_LIMIT {observation.pop_front();}
                        observation.push_back(ClassifierObservation {pair,logits:[logits[1],logits[2]],probability_second,high_score,rejected,policy:classifier_policy});
                    }
                    if rejected {
                        stats.rejected_mids.fetch_add(1,Ordering::Relaxed);
                        let _=tx.try_send(WorkerResult::Rejected {pair,code:5});
                        previous=Some(source);continue;
                    }
                    stats.completed.fetch_add(1,Ordering::Relaxed);
                    if presentation_current(pair.presentation_generation,pair.epoch,&epoch,&stop) {
                        let result=WorkerResult::Flow {pair,
                            half:Arc::new(half[GUARD/2..GUARD/2+HALF_ELEMENTS].to_vec()),milliseconds:began.elapsed().as_secs_f64()*1000.0};
                        match tx.try_send(result) {Ok(())=>{},Err(TrySendError::Full(_))=>{stats.result_drops.fetch_add(1,Ordering::Relaxed);},Err(TrySendError::Disconnected(_))=>return}
                    }else{stats.stale_results.fetch_add(1,Ordering::Relaxed);}
                    previous=Some(source);continue;
                }
                let mut output=vec![0xb9;RGB_BYTES+2*GUARD];
                if !presentation_current(pair.presentation_generation,pair.epoch,&epoch,&stop) {
                    stats.presentation_gate_drops.fetch_add(1,Ordering::Relaxed);previous=None;continue;
                }
                if pair.newer_slot-pair.older_slot>1 {stats.nonadjacent_pairs.fetch_add(1,Ordering::Relaxed);}
                stats.pair_runs.fetch_add(1,Ordering::Relaxed);
                let began=Instant::now();
                let result=unsafe {muguet_interpolation_process_rgb8(ctx.0,a.rgb.as_ptr(),source.rgb.as_ptr(),WIDTH as i32,HEIGHT as i32,
                    output.as_mut_ptr().add(GUARD),RGB_BYTES,0.5)};
                if !presentation_current(pair.presentation_generation,pair.epoch,&epoch,&stop) {
                    stats.stale_results.fetch_add(1,Ordering::Relaxed);previous=None;continue;
                }
                let milliseconds=began.elapsed().as_secs_f64()*1000.0;
                let guards_intact=output[..GUARD].iter().chain(&output[GUARD+RGB_BYTES..]).all(|&v|v==0xb9);
                // The public NNRT adapter's quality gate deliberately skips
                // uncertain pairs (code 5). This is not a scene-cut classifier
                // contract and must not disable future valid interpolation.
                if mode=="npu"&&result==5&&guards_intact {
                    stats.rejected_mids.fetch_add(1,Ordering::Relaxed);
                    let _=tx.try_send(WorkerResult::Rejected {pair,code:result});
                    previous=Some(source);continue;
                }
                if result!=0 || !guards_intact {
                    if tx.send(WorkerResult::InferenceFailed {pair,error:format!("{mode} process returned {result}; output guards intact={}",
                        output[..GUARD].iter().chain(&output[GUARD+RGB_BYTES..]).all(|&v|v==0xb9))}).is_err(){return;}
                    previous=None;continue;
                }
                stats.completed.fetch_add(1,Ordering::Relaxed);
                if presentation_current(pair.presentation_generation,pair.epoch,&epoch,&stop) {
                    let rgb=output[GUARD..GUARD+RGB_BYTES].to_vec();
                    let different_pixels=rgb.chunks_exact(3).zip(a.rgb.chunks_exact(3)).filter(|(a,b)|a!=b).count();
                    // A bounded channel cannot make GPU progress wait on inference.
                    match tx.try_send(WorkerResult::Mid {pair,
                        rgb:Arc::new(rgb),milliseconds,different_pixels}) {
                        Ok(())=>{},Err(TrySendError::Full(_))=>{stats.result_drops.fetch_add(1,Ordering::Relaxed);},
                        Err(TrySendError::Disconnected(_))=>return,
                    }
                } else {
                    stats.stale_results.fetch_add(1,Ordering::Relaxed);
                }
            }
            previous=Some(source);
        }
    }
    struct Pending {
        epoch:u64,presentation_generation:u64,sequence:u64,slot:u64,buffer:Buffer,_snapshot:Texture,
        ready:Receiver<Result<(),String>>,began:Instant,
        width:u32,height:u32,pitch:usize,bytes:usize,
        monotonic_ns:Option<u64>,
        origin:Instant,elapsed_ns:u64,period_ns:u64,native_deadline_ns:Option<u64>,
    }
    #[derive(Clone)]
    struct Ready { rgb:Arc<Vec<u8>>,sequence:u64,source_slot:u64,content_slot:u64,kind:&'static str,gpu:Option<GpuFrame>,
        content_key:OutputContentKey,pair:Option<super::pairing::Pair>,source_monotonic_ns:Option<u64> }
    impl Ready {
        fn midpoint_key(pair:super::pairing::Pair)->OutputContentKey {
            OutputContentKey {epoch:pair.epoch,class:"ai",older_sequence:pair.older_sequence,older_slot:pair.older_slot,
                newer_sequence:Some(pair.newer_sequence),newer_slot:Some(pair.newer_slot)}
        }
        fn midpoint(pair:super::pairing::Pair,rgb:Arc<Vec<u8>>,gpu:Option<GpuFrame>)->Self {
            Self {rgb,sequence:pair.older_sequence,source_slot:pair.older_slot,content_slot:pair.midpoint_slot,kind:"ai",gpu,
                content_key:Self::midpoint_key(pair),
                pair:Some(pair),source_monotonic_ns:pair.older_monotonic_ns}
        }
    }
    #[derive(Default)]
    struct Counts {
        guest_sources:u64,selected_sources:u64,throttled_sources:u64,readback_drops:u64,
        mapped_sources:u64,worker_drops:u64,completed_ai:u64,late_ai:u64,
        source_presents:u64,ai_presents:u64,repeats:u64,underruns:u64,missed_slots:u64,
        acquire_failures:u64,resets:u64,event_drops:u64,
        orphan_flows:u64,orphan_mids:u64,pair_collisions:u64,output_skips:u64,gpu_warps:u64,
        late_ai_presented:u64,ai_window_expired:u64,ai_not_forward:u64,source_not_forward:u64,
        retired_readbacks:u64,retired_worker_results:u64,native_superseded:u64,
    }
    pub(crate) struct Interpolator {
        mode:&'static str,unlimited_native:bool,native_pacer:super::pairing::NativePacer<Ready>,native_offered:Option<Ready>,native_retry_after:Option<Instant>,last_ai_emit:Option<Instant>, began:Instant, origin:Option<Instant>, next_source_slot:u64,next_slot:u64,
        status_writer:super::status_writer::Writer,
        classifier_policy:&'static str,
        output_fps:u32,period_ns:u64,source_ns:u64,
        next_poll:Instant,next_report:Instant,visible:bool,presentation_generation:u64,physical:(u32,u32),source_desc:Option<(u32,u32,TextureFormat)>,
        epoch:Arc<AtomicU64>,stop:Arc<AtomicBool>,jobs:SyncSender<Source>,results:Receiver<WorkerResult>,
        worker_stats:Arc<WorkerStats>,
        pending:VecDeque<Pending>,events:BTreeMap<u64,Ready>,last:Option<Ready>,
        pipeline:Option<RenderPipeline>,output:Option<Texture>,failed:Option<String>,
        snapshot_pipeline:Option<RenderPipeline>,transform_pipeline:Option<RenderPipeline>,warp_pipeline:Option<RenderPipeline>,
        history:BTreeMap<u64,Texture>,native_output:(u32,u32),sr_enabled:bool,
        source_times:BTreeMap<u64,(u64,Option<u64>)>,
        output_capture:Option<String>,output_capture_rows:Vec<Value>,
        info:Option<Value>,counts:Counts,inference_ms:VecDeque<f64>,trace:VecDeque<Value>,last_present:Option<Instant>,
        rejections:VecDeque<Value>,
    }
    impl Drop for Interpolator { fn drop(&mut self) {note_live(0);crate::native_telemetry::set_source_target(None);self.stop.store(true,Ordering::Release);self.epoch.fetch_add(1,Ordering::AcqRel);} }
    impl Interpolator {
        pub(crate) fn from_env()->Option<Self> {
            note_live(0);
            if !crate::config::frame_interpolation_enabled() {
                eprintln!("[frame-interpolation] disabled by frame_interpolation_enabled; backend marker retained");
                return None;
            }
            let mode=match std::env::var("MG_FRAME_INTERPOLATION").as_deref(){Ok("rife")=>"rife",Ok("npu")=>"npu",_=>return None};
            note_live(1);
            let setup=||->Result<Self,String> {
                let root=std::fs::canonicalize(std::env::var_os("MUGUET_SUPPORT").ok_or("missing MUGUET_SUPPORT")?).map_err(|e|e.to_string())?;
                let (output_fps,classifier_policy)=configured_options(&root,mode)?;
                let period_ns=1_000_000_000/output_fps as u64;
                let model=if mode=="rife" {std::fs::canonicalize(std::env::var_os("MG_RIFE_MODEL_DIR").map(PathBuf::from).unwrap_or_else(||root.join("rife-v4.6")))
                    .map_err(|e|format!("model directory: {e}"))?}else{root.clone()};
                if mode=="rife"&&(!model.is_dir()||model==root||!model.starts_with(&root)) {return Err("model directory must be inside app files".into());}
                let epoch=Arc::new(AtomicU64::new(1));let stop=Arc::new(AtomicBool::new(false));
                let worker_stats=Arc::new(WorkerStats::default());
                let (jobs,rx)=mpsc::sync_channel(2);let (tx,results)=mpsc::sync_channel(6);
                let (e,s,stats)=(epoch.clone(),stop.clone(),worker_stats.clone());
                std::thread::Builder::new().name("game-frame-ai".into()).stack_size(8<<20).spawn(move||worker(model,mode,classifier_policy,rx,tx,e,s,stats))
                    .map_err(|e|format!("spawn interpolation worker: {e}"))?;
                let now=Instant::now();
                Ok(Self {status_writer:super::status_writer::Writer::new(root),mode,unlimited_native:crate::config::fps_target()<=0.0,native_pacer:Default::default(),native_offered:None,native_retry_after:None,last_ai_emit:None,classifier_policy,began:now,origin:None,next_source_slot:0,next_slot:0,next_poll:now,next_report:now,
                    output_fps,period_ns,source_ns:period_ns*2,
                    visible:crate::ffi::app_presentation_allowed(),presentation_generation:crate::ffi::app_presentation_generation(),
                    physical:crate::ffi::physical_size(),source_desc:None,epoch,stop,jobs,results,worker_stats,
                    pending:VecDeque::new(),events:BTreeMap::new(),last:None,pipeline:None,output:None,
                    snapshot_pipeline:None,transform_pipeline:None,warp_pipeline:None,history:BTreeMap::new(),native_output:(0,0),sr_enabled:false,
                    source_times:BTreeMap::new(),
                    output_capture:None,output_capture_rows:Vec::new(),
                    failed:None,info:None,counts:Counts::default(),inference_ms:VecDeque::new(),trace:VecDeque::new(),last_present:None,rejections:VecDeque::new()})
            };
            match setup() {Ok(mut v)=>{crate::native_telemetry::set_source_target(if v.unlimited_native{None}else{Some(v.output_fps as f64 / 2.0)});eprintln!("[frame-interpolation] native cadence={} maxNativeHoldMs={}",if v.unlimited_native{"unlimited_capture_timestamps"}else{"legacy_half_output"},if v.unlimited_native{50}else{200});eprintln!("[frame-interpolation] enabled: backend {}; original GPU resolution; nativeHz={:?}; AI budget={}Hz; classifier_policy={}",v.mode,if v.unlimited_native{None}else{Some(v.output_fps/2)},v.output_fps,v.classifier_policy);v.report();Some(v)},
                Err(e)=>{note_live(3);eprintln!("[frame-interpolation] rejected; original presentation retained: {e}");None}}
        }
        pub(crate) fn active(&self)->bool { self.failed.is_none() }
        pub(crate) fn unlimited_native(&self)->bool {self.unlimited_native}
        pub(crate) fn output_failed(&mut self,error:String) {self.fail(error);}
        pub(crate) fn cache_epoch(&self)->u64 {self.epoch.load(Ordering::Acquire)}
        pub(crate) fn update_slow_trace_context(&self) {
            if super::slow_trace::enabled(){super::slow_trace::source(self.epoch.load(Ordering::Relaxed),self.counts.guest_sources,
                self.counts.selected_sources,self.source_times.last_key_value().map(|(_,v)|v.0));}
        }
        pub(crate) fn validate_adapter(&mut self,adapter:&Adapter) {
            if self.mode!="npu" {return;}
            for format in [TextureFormat::Rgba16Float,TextureFormat::Rg16Float] {
                let features=adapter.get_texture_format_features(format);
                if !features.allowed_usages.contains(TextureUsages::RENDER_ATTACHMENT|TextureUsages::TEXTURE_BINDING)||
                    !features.flags.contains(TextureFormatFeatureFlags::FILTERABLE) {
                    self.fail(format!("GPU flow transform requires filterable renderable {format:?}: {features:?}"));return;
                }
            }
        }
        pub(crate) fn output_skipped(&mut self,reason:&str) {
            self.counts.output_skips+=1;
            if self.counts.output_skips<=3 {eprintln!("[frame-interpolation] output slot skipped: {reason}");}
        }
        pub(crate) fn output_size(&mut self,size:(u32,u32),sr:bool) {self.native_output=size;self.sr_enabled=sr;}
        pub(crate) fn note_output_capture(&mut self,directory:&str,display:&Display) {
            self.output_capture=Some(directory.into());
            if self.output_capture_rows.len()<6 {self.output_capture_rows.push(json!({"outputSlot":display.slot,"kind":display.kind,
                "sourceSequence":display.source_sequence,"sourceSlot":display.source_slot,
                "newerSourceSequence":display.newer_source_sequence,"newerSourceSlot":display.newer_source_slot}));}
        }
        pub(crate) fn wait(&self)->Duration {if self.active()&&self.visible&&(!self.unlimited_native||self.native_pacer.busy()||!self.pending.is_empty()||!self.events.is_empty()){Duration::from_millis(1)}else{Duration::from_millis(100)}}
        pub(crate) fn source_wait(&self)->Duration {
            if self.unlimited_native||!self.active()||!self.visible||!presentation_current(self.presentation_generation,self.epoch.load(Ordering::Acquire),&self.epoch,&self.stop) {return Duration::ZERO;}
            self.origin.map(|origin|(origin+Duration::from_nanos(self.next_source_slot.saturating_mul(self.source_ns)))
                .saturating_duration_since(Instant::now())).unwrap_or(Duration::ZERO)
        }
        pub(crate) fn context(&mut self,visible:bool,physical:(u32,u32)) {
            let generation=crate::ffi::app_presentation_generation();
            let visible=visible&&crate::ffi::app_presentation_allowed();
            if super::pairing::context_changed(self.presentation_generation,generation,self.visible,visible,self.physical,physical) {
                let reason=if self.presentation_generation!=generation {"app_presentation_generation_changed"}else{"visibility_or_physical_resize"};
                self.presentation_generation=generation;self.visible=visible;self.physical=physical;self.reset(reason);
            }
        }
        fn reset(&mut self,reason:&str) {
            note_live(if self.failed.is_some(){3}else{1});
            self.epoch.fetch_add(1,Ordering::AcqRel);self.origin=None;self.next_source_slot=0;self.next_slot=0;
            self.events.clear();self.history.clear();self.source_times.clear();self.native_pacer=Default::default();self.native_offered=None;self.native_retry_after=None;self.last_ai_emit=None;self.last=None;self.last_present=None;self.counts.resets+=1;
            // Pending maps retain their buffers until callbacks complete. Their
            // epoch is discarded rather than accessing stale image contents.
            eprintln!("[frame-interpolation] history cleared: {reason}");
        }
        fn fail(&mut self,error:String) {
            crate::native_telemetry::set_source_target(None);
            eprintln!("[frame-interpolation] disabled; original presentation restored: {error}");
            self.failed=Some(error);self.stop.store(true,Ordering::Release);self.reset("error");self.report();
        }
        fn insert(&mut self,slot:u64,ready:Ready) {
            if self.events.len()>=EVENT_LIMIT && !self.events.contains_key(&slot) {self.counts.event_drops+=1;return;}
            self.events.insert(slot,ready);
        }
        pub(crate) fn record(&mut self,device:&Device,source:&Texture,sequence:u64)->Option<CommandBuffer> {
            self.counts.guest_sources+=1;
            if !self.active()||!self.visible||!presentation_current(self.presentation_generation,self.epoch.load(Ordering::Acquire),&self.epoch,&self.stop) {return None;}
            let desc=(source.width(),source.height(),source.format());
            if self.source_desc.is_some_and(|old|old!=desc) {self.reset("guest_source_extent_or_format_changed");}
            self.source_desc=Some(desc);
            let now=Instant::now();let origin=*self.origin.get_or_insert(now);
            if self.unlimited_native {return self.record_unlimited(device,source,sequence,origin,now);}
            let source_slot=(now.duration_since(origin).as_nanos()/self.source_ns as u128).min(u64::MAX as u128) as u64;
            if source_slot<self.next_source_slot {self.counts.throttled_sources+=1;return None;}
            let Some(next)=source_slot.checked_add(1) else {self.fail("source clock overflow".into());return None;};
            self.next_source_slot=next;
            if self.pending.len()>=PENDING_LIMIT {self.counts.readback_drops+=1;return None;}
            match self.encode(device,source,sequence,source_slot,origin,now) {
                Ok((command,pending))=>{self.counts.selected_sources+=1;self.pending.push_back(pending);Some(command)},
                Err(error)=>{self.fail(error);None}
            }
        }
        fn record_unlimited(&mut self,device:&Device,source:&Texture,sequence:u64,origin:Instant,now:Instant)->Option<CommandBuffer> {
            let elapsed_ns=u64::try_from(now.duration_since(origin).as_nanos()).ok()?;
            // CPU readback capacity controls optional inference only. Every real
            // source still gets an immutable full-resolution GPU snapshot.
            let (command,snapshot,mut pending,mono)=if self.pending.len()<PENDING_LIMIT {
                match self.encode(device,source,sequence,sequence,origin,now) {
                    Ok((command,pending))=>{let snapshot=pending._snapshot.clone();let mono=pending.monotonic_ns;(command,snapshot,Some(pending),mono)},
                    Err(error)=>{self.fail(error);return None;}
                }
            }else {
                self.counts.readback_drops+=1;
                match self.encode_native_snapshot(device,source) {
                    Ok((command,snapshot))=>(command,snapshot,None,super::monotonic_ns()),
                    Err(error)=>{self.fail(error);return None;}
                }
            };
            let ready=Ready {rgb:Arc::new(Vec::new()),sequence,source_slot:sequence,content_slot:elapsed_ns,kind:"source",gpu:Some(GpuFrame::Source(snapshot)),
                content_key:OutputContentKey {epoch:self.epoch.load(Ordering::Acquire),class:"source",older_sequence:sequence,older_slot:sequence,newer_sequence:None,newer_slot:None},
                pair:None,source_monotonic_ns:mono};
            let (deadline,replaced)=self.native_pacer.offer(ready,elapsed_ns);
            if replaced {self.counts.native_superseded+=1;}
            if let Some(mut sample)=pending.take() {sample.native_deadline_ns=Some(deadline);self.pending.push_back(sample);}
            self.counts.selected_sources+=1;Some(command)
        }
        fn encode_native_snapshot(&mut self,device:&Device,source:&Texture)->Result<(CommandBuffer,Texture),String> {
            let linear=source.format().remove_srgb_suffix();
            if !matches!(linear,TextureFormat::Rgba8Unorm|TextureFormat::Bgra8Unorm)||source.dimension()!=TextureDimension::D2||source.sample_count()!=1||
                !source.usage().contains(TextureUsages::TEXTURE_BINDING)||source.width()==0||source.height()==0||source.width() as u64*source.height() as u64*4>16*1024*1024 {
                return Err("unsupported/bounded unlimited native source".into());
            }
            let validation=device.push_error_scope(ErrorFilter::Validation);let oom=device.push_error_scope(ErrorFilter::OutOfMemory);
            let encoded=(||->Result<(CommandBuffer,Texture),String>{
                if self.snapshot_pipeline.is_none(){self.snapshot_pipeline=Some(crate::spatial_upscale::blit_pipeline(device,TextureFormat::Rgba8Unorm));}
                let pipeline=self.snapshot_pipeline.as_ref().ok_or("native snapshot pipeline missing")?;
                let snapshot=texture(device,"unlimited-native-owned-source",source.width(),source.height(),TextureFormat::Rgba8Unorm,
                    TextureUsages::RENDER_ATTACHMENT|TextureUsages::TEXTURE_BINDING|TextureUsages::COPY_SRC);
                let view=source.create_view(&TextureViewDescriptor {format:Some(linear),dimension:Some(TextureViewDimension::D2),mip_level_count:Some(1),array_layer_count:Some(1),..Default::default()});
                let binding=device.create_bind_group(&BindGroupDescriptor {label:Some("unlimited-native-snapshot-binding"),layout:&pipeline.get_bind_group_layout(0),entries:&[BindGroupEntry {binding:0,resource:BindingResource::TextureView(&view)}]});
                let mut encoder=device.create_command_encoder(&CommandEncoderDescriptor {label:Some("unlimited-native-snapshot")});
                draw(&mut encoder,"unlimited-native-copy",pipeline,&binding,&snapshot.create_view(&Default::default()));
                Ok((encoder.finish(),snapshot))
            })();
            let allocation_error=pollster::block_on(oom.pop());let validation_error=pollster::block_on(validation.pop());
            if let Some(error)=allocation_error {return Err(format!("native snapshot allocation: {error}"));}
            if let Some(error)=validation_error {return Err(format!("native snapshot validation: {error}"));}
            encoded
        }
        fn encode(&mut self,device:&Device,source:&Texture,sequence:u64,slot:u64,origin:Instant,captured:Instant)->Result<(CommandBuffer,Pending),String> {
            let monotonic_ns=super::monotonic_ns();
            let elapsed_ns=u64::try_from(captured.duration_since(origin).as_nanos()).map_err(|_|"source timestamp overflow")?;
            let linear=source.format().remove_srgb_suffix();
            if !matches!(linear,TextureFormat::Rgba8Unorm|TextureFormat::Bgra8Unorm)||source.dimension()!=TextureDimension::D2||
                source.sample_count()!=1||!source.usage().contains(TextureUsages::TEXTURE_BINDING)||source.width()==0||source.height()==0||
                source.width() as u64*source.height() as u64*4>16*1024*1024 {
                return Err(format!("unsupported/bounded guest final source {:?} {}x{} samples={} usage={:?}",source.format(),source.width(),source.height(),source.sample_count(),source.usage()));
            }
            let raw=self.mode=="npu";
            let (width,height)=if raw{(MODEL_WIDTH,MODEL_HEIGHT)}else{(WIDTH,HEIGHT)};
            let pitch=(width as usize*4+255)&!255;
            let bytes=GUARD*2+pitch*height as usize;
            let validation=device.push_error_scope(ErrorFilter::Validation);let oom=device.push_error_scope(ErrorFilter::OutOfMemory);
            let encoded=(||->Result<(CommandBuffer,Pending),String> {
                if self.pipeline.is_none() {
                    let source=if raw {SHADER.replace("vec2f(320,192)","vec2f(448,208)")}else{SHADER.into()};
                    self.pipeline=Some(rgba_pipeline(device,"game-interpolation-model-downsample",&source,&[TextureFormat::Rgba8Unorm]));
                }
                let pipeline=self.pipeline.as_ref().ok_or("downsample pipeline missing")?;
                let (snapshot_width,snapshot_height)=if raw||self.unlimited_native{(source.width(),source.height())}else{(width,height)};
                let snapshot=texture(device,"game-interpolation-private-history",snapshot_width,snapshot_height,TextureFormat::Rgba8Unorm,
                    TextureUsages::RENDER_ATTACHMENT|TextureUsages::TEXTURE_BINDING|TextureUsages::COPY_SRC);
                let source_view=source.create_view(&TextureViewDescriptor {format:Some(linear),dimension:Some(TextureViewDimension::D2),
                    mip_level_count:Some(1),array_layer_count:Some(1),..Default::default()});
                let input_texture=if raw||self.unlimited_native {texture(device,"game-npu-448x208-input",width,height,TextureFormat::Rgba8Unorm,
                    TextureUsages::RENDER_ATTACHMENT|TextureUsages::COPY_SRC)}else{snapshot.clone()};
                let input_view=input_texture.create_view(&TextureViewDescriptor::default());
                let binding=device.create_bind_group(&BindGroupDescriptor {label:Some("game-model-downsample-source"),layout:&pipeline.get_bind_group_layout(0),
                    entries:&[BindGroupEntry {binding:0,resource:BindingResource::TextureView(&source_view)}]});
                let buffer=device.create_buffer(&BufferDescriptor {label:Some("game-model-guarded-readback"),size:bytes as u64,
                    usage:BufferUsages::MAP_READ|BufferUsages::COPY_DST,mapped_at_creation:true});
                {let mut view=buffer.slice(..).get_mapped_range_mut().map_err(|e|format!("initial readback map: {e:?}"))?;view.slice(..).fill(SENTINEL);}
                buffer.unmap();
                let mut enc=device.create_command_encoder(&CommandEncoderDescriptor {label:Some("game-model-isolated-source-copy")});
                if raw||self.unlimited_native {
                    if self.snapshot_pipeline.is_none(){self.snapshot_pipeline=Some(crate::spatial_upscale::blit_pipeline(device,TextureFormat::Rgba8Unorm));}
                    let copy=self.snapshot_pipeline.as_ref().ok_or("history copy pipeline missing")?;
                    let binding=device.create_bind_group(&BindGroupDescriptor {label:Some("game-private-history-binding"),layout:&copy.get_bind_group_layout(0),
                        entries:&[BindGroupEntry {binding:0,resource:BindingResource::TextureView(&source_view)}]});
                    let target=snapshot.create_view(&TextureViewDescriptor::default());
                    draw(&mut enc,"game-original-resolution-history-copy",copy,&binding,&target);
                }
                draw(&mut enc,"game-fixed-model-downsample",pipeline,&binding,&input_view);
                enc.copy_texture_to_buffer(TexelCopyTextureInfo {texture:&input_texture,mip_level:0,origin:Origin3d::ZERO,aspect:TextureAspect::All},
                    TexelCopyBufferInfo {buffer:&buffer,layout:TexelCopyBufferLayout {offset:GUARD as u64,bytes_per_row:Some(pitch as u32),rows_per_image:Some(height)}},
                    Extent3d {width,height,depth_or_array_layers:1});
                let (tx,ready)=mpsc::channel();enc.map_buffer_on_submit(&buffer,MapMode::Read,..,move|r|{let _=tx.send(r.map_err(|e|e.to_string()));});
                Ok((enc.finish(),Pending {epoch:self.epoch.load(Ordering::Acquire),presentation_generation:self.presentation_generation,sequence,slot,buffer,_snapshot:snapshot,ready,began:Instant::now(),width,height,pitch,bytes,monotonic_ns,
                    origin,elapsed_ns,period_ns:self.period_ns,native_deadline_ns:None}))
            })();
            let allocation_error=pollster::block_on(oom.pop());let validation_error=pollster::block_on(validation.pop());
            if let Some(e)=allocation_error {return Err(format!("downsample allocation: {e}"));}
            if let Some(e)=validation_error {return Err(format!("downsample validation: {e}"));}
            encoded
        }
        fn read(p:&Pending,raw:bool)->Result<Arc<Vec<u8>>,String> {
            let view=p.buffer.slice(..).get_mapped_range().map_err(|e|format!("readback mapped range: {e:?}"))?;
            let rgb=super::readback_rows::copy_guarded_rgba(&view,p.width,p.height,p.pitch,p.bytes,GUARD,SENTINEL,raw)?;
            drop(view);Ok(Arc::new(rgb))
        }
        fn collect(&mut self,device:&Device) {
            if !self.pending.is_empty() {
                if let Err(e)=device.poll(PollType::Poll) {self.fail(format!("readback poll: {e:?}"));return;}
            }
            // Queue order preserves source chronology even if callbacks are delivered together.
            while let Some(p)=self.pending.front() {
                let ready=match p.ready.try_recv() {
                    Ok(r)=>Some(r),Err(TryRecvError::Disconnected)=>Some(Err("readback callback disconnected".into())),
                    Err(TryRecvError::Empty) if p.began.elapsed()>Duration::from_secs(5)=>Some(Err("readback exceeded 5s bounded timeout".into())),
                    Err(TryRecvError::Empty)=>None,
                };
                let Some(ready)=ready else {break;};
                let Some(p)=self.pending.pop_front() else {break;};
                // Retired maps keep their buffer until callback completion or
                // the existing bounded timeout. An old callback error must
                // never disable a newly resumed epoch before any pixel read.
                if !self.active()||!presentation_current(p.presentation_generation,p.epoch,&self.epoch,&self.stop) {
                    if ready.is_ok(){p.buffer.unmap();}self.counts.retired_readbacks+=1;continue;
                }
                let mapped=ready.is_ok();let result=ready.and_then(|()|Self::read(&p,self.mode=="npu"));if mapped {p.buffer.unmap();}
                let rgb=match result {Ok(v)=>v,Err(e)=>{self.fail(e);break;}};
                self.counts.mapped_sources+=1;
                self.source_times.insert(p.slot,(p.sequence,p.monotonic_ns));
                while self.source_times.len()>16 {self.source_times.pop_first();}
                let gpu=if self.mode=="npu" {
                    self.history.insert(p.slot,p._snapshot.clone());
                    while self.history.len()>8 {self.history.pop_first();}
                    Some(GpuFrame::Source(p._snapshot.clone()))
                }else{None};
                if !self.unlimited_native {
                let Some(output_slot)=p.slot.checked_mul(2) else {self.fail("source output clock overflow".into());break;};
                self.insert(output_slot,Ready {rgb:if gpu.is_some(){Arc::new(Vec::new())}else{rgb.clone()},sequence:p.sequence,source_slot:p.slot,content_slot:output_slot,kind:"source",gpu,
                    content_key:OutputContentKey {epoch:p.epoch,class:"source",older_sequence:p.sequence,older_slot:p.slot,newer_sequence:None,newer_slot:None},
                    pair:None,source_monotonic_ns:p.monotonic_ns});
                }
                match self.jobs.try_send(Source {endpoint:super::pairing::Endpoint {epoch:p.epoch,presentation_generation:p.presentation_generation,sequence:p.sequence,slot:p.slot,
                    ordinal:if self.unlimited_native{p.sequence}else{self.counts.mapped_sources},elapsed_ns:p.elapsed_ns,monotonic_ns:p.monotonic_ns},origin:p.origin,period_ns:p.period_ns,native_deadline_ns:p.native_deadline_ns,rgb}) {
                    Ok(())=>{},Err(TrySendError::Full(_))=>self.counts.worker_drops+=1,
                    Err(TrySendError::Disconnected(_))=>{self.fail("interpolation worker input disconnected".into());break;}
                }
            }
            loop {
                match self.results.try_recv() {
                    Ok(WorkerResult::Started(info))=>{eprintln!("[frame-interpolation] interpolation worker ready: {info}");self.info=Some(info);},
                    Ok(WorkerResult::Failed(e))=>{self.fail(e);break;},
                    Ok(WorkerResult::InferenceFailed {pair,error})=>{
                        if !presentation_current(pair.presentation_generation,pair.epoch,&self.epoch,&self.stop) {
                            self.counts.retired_worker_results+=1;continue;
                        }
                        self.fail(error);break;
                    },
                    Ok(WorkerResult::Rejected {pair,code})=>{
                        if !presentation_current(pair.presentation_generation,pair.epoch,&self.epoch,&self.stop) {self.counts.retired_worker_results+=1;continue;}
                        if self.rejections.len()==16 {self.rejections.pop_front();}
                        self.rejections.push_back(json!({"sourceSequence":pair.older_sequence,"newerSourceSequence":pair.newer_sequence,
                            "olderSourceSlot":pair.older_slot,"newerSourceSlot":pair.newer_slot,"midpointSlot":pair.midpoint_slot,"backendReturnCode":code,
                            "reason":"NPU_quality_gate_unknown_classifier_rejected_pair; no_scene_cut_semantics_assumed"}));
                    },
                    Ok(WorkerResult::Mid {pair,rgb,milliseconds,different_pixels})=>{
                        if !presentation_current(pair.presentation_generation,pair.epoch,&self.epoch,&self.stop) {self.counts.retired_worker_results+=1;continue;}
                        self.counts.completed_ai+=1;if self.inference_ms.len()==TRACE_LIMIT {self.inference_ms.pop_front();}self.inference_ms.push_back(milliseconds);
                        if !self.pair_can_queue(pair) {continue;}
                        if !self.pair_endpoints_available(pair) {self.counts.orphan_mids+=1;continue;}
                        if self.events.contains_key(&if self.unlimited_native{pair.due_elapsed_ns}else{pair.midpoint_slot}) {self.counts.pair_collisions+=1;continue;}
                        self.insert(if self.unlimited_native{pair.due_elapsed_ns}else{pair.midpoint_slot},Ready::midpoint(pair,rgb,None));
                        if self.counts.completed_ai<=3 {eprintln!("[frame-interpolation] real pair AI: sources={}/{} slots={}/{} midpoint={} inferenceMs={milliseconds:.3} differentFromPreviousPixels={different_pixels}",
                            pair.older_sequence,pair.newer_sequence,pair.older_slot,pair.newer_slot,pair.midpoint_slot);}
                    },
                    Ok(WorkerResult::Flow {pair,half,milliseconds})=>{
                        if !presentation_current(pair.presentation_generation,pair.epoch,&self.epoch,&self.stop) {self.counts.retired_worker_results+=1;continue;}
                        self.counts.completed_ai+=1;if self.inference_ms.len()==TRACE_LIMIT{self.inference_ms.pop_front();}self.inference_ms.push_back(milliseconds);
                        if !self.pair_can_queue(pair) {continue;}
                        if !self.pair_endpoints_available(pair) {self.counts.orphan_flows+=1;continue;}
                        if self.events.contains_key(&if self.unlimited_native{pair.due_elapsed_ns}else{pair.midpoint_slot}) {self.counts.pair_collisions+=1;continue;}
                        match (self.history.get(&pair.older_slot),self.history.get(&pair.newer_slot)) {
                            (Some(older),Some(newer))=>self.insert(if self.unlimited_native{pair.due_elapsed_ns}else{pair.midpoint_slot},Ready::midpoint(pair,Arc::new(Vec::new()),
                                Some(GpuFrame::Flow{older:older.clone(),newer:newer.clone(),half}))),
                            _=>self.counts.orphan_flows+=1,
                        }
                    },
                    Err(TryRecvError::Empty)=>break,
                    Err(TryRecvError::Disconnected)=>{if self.active(){self.fail("interpolation worker result disconnected".into());}break;}
                }
            }
        }
        fn pair_endpoints_available(&self,pair:super::pairing::Pair)->bool {
            self.source_times.get(&pair.older_slot)==Some(&(pair.older_sequence,pair.older_monotonic_ns))&&
                self.source_times.get(&pair.newer_slot)==Some(&(pair.newer_sequence,pair.newer_monotonic_ns))
        }
        fn pair_can_queue(&mut self,pair:super::pairing::Pair)->bool {
            if self.unlimited_native {
                let now=self.origin.map(|origin|origin.elapsed().as_nanos().min(u64::MAX as u128) as u64).unwrap_or(u64::MAX);
                if now>=pair.window_end_elapsed_ns {self.counts.late_ai+=1;self.counts.ai_window_expired+=1;return false;}
                if self.last.as_ref().is_some_and(|ready|ready.content_slot>=pair.midpoint_elapsed_ns) {
                    self.counts.late_ai+=1;self.counts.ai_not_forward+=1;return false;
                }
                return true;
            }
            let previous=self.last.as_ref();
            match super::pairing::admit(pair.midpoint_slot,self.next_slot,Some(pair.newer_due_slot),
                previous.map(|ready|ready.content_slot),previous.is_some_and(|ready|ready.content_key==Ready::midpoint_key(pair))) {
                super::pairing::Admission::Expired=>{self.counts.late_ai+=1;self.counts.ai_window_expired+=1;false},
                super::pairing::Admission::Stale=>{self.counts.late_ai+=1;self.counts.ai_not_forward+=1;false},
                _=>true,
            }
        }
        pub(crate) fn tick(&mut self,device:&Device)->Option<Display> {
            if self.unlimited_native {return self.tick_unlimited(device);}
            let now=Instant::now();if now<self.next_poll {return None;}self.next_poll=now+Duration::from_millis(1);
            {let _slow=super::slow_trace::span("output_collect_readback_and_results","");self.collect(device);}
            if now>=self.next_report {self.next_report=now+Duration::from_secs(1);
                let _slow=super::slow_trace::span("output_status_json_and_file_write","");self.report();}
            if !self.active()||!self.visible||!presentation_current(self.presentation_generation,self.epoch.load(Ordering::Acquire),&self.epoch,&self.stop) {return None;}
            let now=Instant::now(); // Completion polling/status IO may have crossed a display deadline.
            let first_due=self.origin?+DELAY;if now<first_due {return None;}
            let current=(now.duration_since(first_due).as_nanos()/self.period_ns as u128).min(u64::MAX as u128) as u64;
            if current<self.next_slot {return None;}
            self.counts.missed_slots+=current-self.next_slot;self.next_slot=current+1;
            let mut selected=None;
            while self.events.first_key_value().is_some_and(|(&slot,_)|slot<=current) {
                let Some((slot,ready))=self.events.pop_first() else {break;};
                if slot!=ready.content_slot {self.counts.event_drops+=1;continue;}
                let previous=selected.as_ref().or(self.last.as_ref());
                let admission=super::pairing::admit(ready.content_slot,current,ready.pair.map(|pair|pair.newer_due_slot),
                    previous.map(|old|old.content_slot),previous.is_some_and(|old|old.content_key==ready.content_key));
                match admission {
                    super::pairing::Admission::OnTime|super::pairing::Admission::Late=>selected=Some(ready),
                    super::pairing::Admission::Expired=>{self.counts.late_ai+=1;self.counts.ai_window_expired+=1;},
                    super::pairing::Admission::Stale=>{
                        if ready.kind=="ai" {self.counts.late_ai+=1;self.counts.ai_not_forward+=1;}
                        else {self.counts.source_not_forward+=1;}
                    },
                    super::pairing::Admission::Future=>{self.counts.event_drops+=1;},
                }
            }
            let kind=if selected.is_some(){selected.as_ref().map(|v|v.kind).unwrap_or("repeat")}else{"repeat"};
            if let Some(ready)=selected {self.last=Some(ready);}
            let Some(ready)=self.last.as_ref() else {self.counts.underruns+=1;return None;};
            if kind=="repeat" {self.counts.underruns+=1;}
            Some(Display {rgb:ready.rgb.clone(),slot:current,source_sequence:ready.sequence,source_slot:ready.source_slot,kind,
                due:first_due+Duration::from_nanos(current.saturating_mul(self.period_ns)),gpu:ready.gpu.clone(),
                content_key:ready.content_key,nominal_content_slot:ready.content_slot,newer_source_slot:ready.pair.map(|p|p.newer_slot),
                source_monotonic_ns:ready.source_monotonic_ns,
                newer_source_monotonic_ns:ready.pair.and_then(|p|p.newer_monotonic_ns),
                newer_source_sequence:ready.pair.map(|p|p.newer_sequence),
                true_midpoint_monotonic_ns:ready.pair.and_then(|p|p.true_midpoint_monotonic_ns),
                scheduled_due_monotonic_ns:ready.pair.and_then(|p|p.scheduled_due_monotonic_ns),
                midpoint_clock_error_ns:ready.pair.map(|p|p.midpoint_clock_error_ns)})
        }
        fn tick_unlimited(&mut self,device:&Device)->Option<Display> {
            if !self.active()||!self.visible||!presentation_current(self.presentation_generation,self.epoch.load(Ordering::Acquire),&self.epoch,&self.stop) {return None;}
            if self.native_retry_after.is_some_and(|at|Instant::now()<at) {return None;}
            self.native_retry_after=None;
            // A caller that could not acquire/encode must not commit source
            // identity. Retry the owned source; normal completion clears it.
            if let Some(ready)=self.native_offered.take() {if ready.kind=="source"{self.native_pacer.retry(ready);}}
            let now=Instant::now();
            let mut elapsed=self.origin.map(|origin|now.duration_since(origin).as_nanos().min(u64::MAX as u128) as u64).unwrap_or(0);
            if !self.native_pacer.urgent(elapsed) {
                if now<self.next_poll {return None;}self.next_poll=now+Duration::from_millis(1);
                {let _slow=super::slow_trace::span("output_collect_readback_and_results","");self.collect(device);}
                if now>=self.next_report {self.next_report=now+Duration::from_secs(1);self.report();}
                if !self.active(){return None;}
                elapsed=self.origin.map(|origin|origin.elapsed().as_nanos().min(u64::MAX as u128) as u64).unwrap_or(0);
            }
            let mut ready=self.native_pacer.take(elapsed);
            if ready.is_none()&&self.last_ai_emit.is_none_or(|last|last.elapsed()>=Duration::from_nanos(self.period_ns)) {
                while self.events.first_key_value().is_some_and(|(&due,_)|due<=elapsed) {
                    let (_,candidate)=self.events.pop_first()?;
                    if candidate.pair.is_some_and(|pair|self.pair_can_queue(pair)) {ready=Some(candidate);}
                }
            }
            let ready=ready?;
            if self.last.as_ref().is_some_and(|last|ready.content_slot<=last.content_slot) {
                if ready.kind=="source"{self.counts.source_not_forward+=1;}else{self.counts.ai_not_forward+=1;}return None;
            }
            let due=self.origin?+Duration::from_nanos(ready.pair.map(|p|p.due_elapsed_ns).unwrap_or(elapsed));
            let display=Display {rgb:ready.rgb.clone(),slot:elapsed,source_sequence:ready.sequence,source_slot:ready.source_slot,kind:ready.kind,due,gpu:ready.gpu.clone(),
                content_key:ready.content_key,nominal_content_slot:ready.content_slot,newer_source_slot:ready.pair.map(|p|p.newer_slot),source_monotonic_ns:ready.source_monotonic_ns,
                newer_source_monotonic_ns:ready.pair.and_then(|p|p.newer_monotonic_ns),newer_source_sequence:ready.pair.map(|p|p.newer_sequence),
                true_midpoint_monotonic_ns:ready.pair.and_then(|p|p.true_midpoint_monotonic_ns),scheduled_due_monotonic_ns:ready.pair.and_then(|p|p.scheduled_due_monotonic_ns),midpoint_clock_error_ns:ready.pair.map(|p|p.midpoint_clock_error_ns)};
            self.native_offered=Some(ready);Some(display)
        }
        pub(crate) fn prepare_output(&mut self,device:&Device,queue:&Queue,display:&Display,encoder:&mut CommandEncoder)->Option<Texture> {
            if let Some(gpu)=display.gpu.as_ref() {
                return match gpu {
                    GpuFrame::Source(source)=>Some(source.clone()),
                    GpuFrame::Flow{older,newer,half}=>match self.warp(device,encoder,older,newer,half) {
                        Ok(texture)=>{self.counts.gpu_warps+=1;Some(texture)},Err(error)=>{self.fail(error);None}
                    }
                };
            }
            if display.rgb.len()!=RGB_BYTES {self.fail("display RGB byte length mismatch".into());return None;}
            if self.output.is_none() {self.output=Some(device.create_texture(&TextureDescriptor {label:Some("game-rife-display-rgba8"),
                size:Extent3d {width:WIDTH,height:HEIGHT,depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:TextureDimension::D2,
                format:TextureFormat::Rgba8Unorm,usage:TextureUsages::COPY_DST|TextureUsages::TEXTURE_BINDING,view_formats:&[]}));}
            let output=self.output.as_ref()?;
            let mut rgba=Vec::with_capacity(WIDTH as usize*HEIGHT as usize*4);
            for rgb in display.rgb.chunks_exact(3) {rgba.extend_from_slice(&[rgb[0],rgb[1],rgb[2],255]);}
            queue.write_texture(TexelCopyTextureInfo {texture:output,mip_level:0,origin:Origin3d::ZERO,aspect:TextureAspect::All},&rgba,
                TexelCopyBufferLayout {offset:0,bytes_per_row:Some(WIDTH*4),rows_per_image:Some(HEIGHT)},Extent3d {width:WIDTH,height:HEIGHT,depth_or_array_layers:1});
            Some(output.clone())
        }
        fn warp(&mut self,device:&Device,encoder:&mut CommandEncoder,older:&Texture,newer:&Texture,half:&[u16])->Result<Texture,String> {
            if half.len()!=HALF_ELEMENTS||older.size()!=newer.size()||older.format()!=TextureFormat::Rgba8Unorm||newer.format()!=TextureFormat::Rgba8Unorm {
                return Err("GPU history/flow dimensions or format mismatch".into());
            }
            let validation=device.push_error_scope(ErrorFilter::Validation);let allocation=device.push_error_scope(ErrorFilter::OutOfMemory);
            let encoded=(||->Result<Texture,String>{
                if self.transform_pipeline.is_none(){self.transform_pipeline=Some(rgba_pipeline(device,"game-flow-grid-transform",FLOW_TRANSFORM_SHADER,&[TextureFormat::Rgba16Float,TextureFormat::Rg16Float]));}
                if self.warp_pipeline.is_none(){self.warp_pipeline=Some(rgba_pipeline(device,"game-original-resolution-flow-warp",WARP_SHADER,&[TextureFormat::Rgba8Unorm]));}
                let transform=self.transform_pipeline.as_ref().ok_or("flow transform pipeline missing")?;
                let warp=self.warp_pipeline.as_ref().ok_or("flow warp pipeline missing")?;
                // The target ABI is little-endian AArch64; borrowing immutable
                // u16 storage as bytes preserves the FP16 bits without conversion.
                if cfg!(target_endian="big"){return Err("FP16 upload requires little-endian target".into());}
                let bytes=unsafe{std::slice::from_raw_parts(half.as_ptr().cast::<u8>(),half.len()*2)};
                let storage=device.create_buffer_init(&wgpu::util::BufferInitDescriptor {label:Some("game-raw-half-flow-storage"),contents:bytes,usage:BufferUsages::STORAGE});
                let flow=texture(device,"game-model-grid-flow",MODEL_WIDTH,MODEL_HEIGHT,TextureFormat::Rgba16Float,TextureUsages::RENDER_ATTACHMENT|TextureUsages::TEXTURE_BINDING);
                let masks=texture(device,"game-model-grid-sigmoid-masks",MODEL_WIDTH,MODEL_HEIGHT,TextureFormat::Rg16Float,TextureUsages::RENDER_ATTACHMENT|TextureUsages::TEXTURE_BINDING);
                let flow_view=flow.create_view(&TextureViewDescriptor::default());let mask_view=masks.create_view(&TextureViewDescriptor::default());
                let transform_bg=device.create_bind_group(&BindGroupDescriptor{label:Some("game-flow-transform-fields"),layout:&transform.get_bind_group_layout(0),
                    entries:&[BindGroupEntry{binding:0,resource:storage.as_entire_binding()}]});
                {
                    let mut pass=encoder.begin_render_pass(&RenderPassDescriptor {label:Some("game-model-grid-transform-sigmoid-before-bilinear"),
                        color_attachments:&[
                            Some(RenderPassColorAttachment{view:&flow_view,depth_slice:None,resolve_target:None,ops:Operations{load:LoadOp::Clear(Color::BLACK),store:StoreOp::Store}}),
                            Some(RenderPassColorAttachment{view:&mask_view,depth_slice:None,resolve_target:None,ops:Operations{load:LoadOp::Clear(Color::BLACK),store:StoreOp::Store}})],
                        depth_stencil_attachment:None,timestamp_writes:None,occlusion_query_set:None,multiview_mask:None});
                    pass.set_pipeline(transform);pass.set_bind_group(0,&transform_bg,&[]);pass.draw(0..3,0..1);
                }
                let target=texture(device,"game-original-resolution-AI-midpoint",newer.width(),newer.height(),TextureFormat::Rgba8Unorm,
                    TextureUsages::RENDER_ATTACHMENT|TextureUsages::TEXTURE_BINDING|TextureUsages::COPY_SRC);
                let target_view=target.create_view(&TextureViewDescriptor::default());let old_view=older.create_view(&TextureViewDescriptor::default());let new_view=newer.create_view(&TextureViewDescriptor::default());
                let sampler=device.create_sampler(&SamplerDescriptor{label:Some("game-flow-warp-clamp-linear"),
                    address_mode_u:AddressMode::ClampToEdge,address_mode_v:AddressMode::ClampToEdge,address_mode_w:AddressMode::ClampToEdge,
                    mag_filter:FilterMode::Linear,min_filter:FilterMode::Linear,mipmap_filter:MipmapFilterMode::Nearest,..Default::default()});
                let binding=device.create_bind_group(&BindGroupDescriptor{label:Some("game-full-resolution-warp-inputs"),layout:&warp.get_bind_group_layout(0),entries:&[
                    BindGroupEntry{binding:0,resource:BindingResource::TextureView(&old_view)},BindGroupEntry{binding:1,resource:BindingResource::TextureView(&new_view)},
                    BindGroupEntry{binding:2,resource:BindingResource::TextureView(&flow_view)},BindGroupEntry{binding:3,resource:BindingResource::TextureView(&mask_view)},
                    BindGroupEntry{binding:4,resource:BindingResource::Sampler(&sampler)}]});
                draw(encoder,"game-full-resolution-midpoint-warp",warp,&binding,&target_view);Ok(target)
            })();
            let oom=pollster::block_on(allocation.pop());let invalid=pollster::block_on(validation.pop());
            if let Some(e)=oom{return Err(format!("GPU warp allocation: {e}"));}if let Some(e)=invalid{return Err(format!("GPU warp validation: {e}"));}encoded
        }
        pub(crate) fn presented(&mut self,display:&Display,success:bool,present_begin_ns:Option<u64>,present_complete_ns:Option<u64>) {
            let now=Instant::now();
            if self.unlimited_native {
                if let Some(ready)=self.native_offered.take() {
                    if success {self.last=Some(ready);if display.kind=="ai" {self.last_ai_emit=Some(now);}}
                    else if ready.kind=="source" {self.native_pacer.retry(ready);self.native_retry_after=Some(now+Duration::from_millis(1));}
                }
            }
            if success&&display.kind=="ai" {
                publish_live(2,present_complete_ns.or_else(super::monotonic_ns).unwrap_or(0),self.presentation_generation);
            }
            if success {match display.kind {"source"=>self.counts.source_presents+=1,"ai"=>self.counts.ai_presents+=1,_=>self.counts.repeats+=1}}
            else {self.counts.acquire_failures+=1;}
            let late_ai=if self.unlimited_native {success&&display.kind=="ai"&&now>display.due}else{super::pairing::is_late_ai_present(success,display.kind=="ai",display.nominal_content_slot,display.slot)};
            if late_ai {self.counts.late_ai_presented+=1;}
            let interval=self.last_present.map(|at|now.duration_since(at).as_secs_f64()*1000.0);
            if self.trace.len()==TRACE_LIMIT {self.trace.pop_front();}
            self.trace.push_back(json!({"slot":display.slot,"kind":display.kind,"sourceSequence":display.source_sequence,"sourceSlot":display.source_slot,
                "contentEpoch":display.content_key.epoch,"contentKind":display.content_key.class,
                "scheduledContentSlot":display.nominal_content_slot,"actualPresentationSlot":display.slot,
                "scheduledMidpointSlot":if display.content_key.class=="ai"{Some(display.nominal_content_slot)}else{None},
                "newerSourceDueSlot":if self.unlimited_native{None}else{display.newer_source_slot.and_then(|slot|slot.checked_mul(2))},
                "coordinateUnits":if self.unlimited_native{"elapsed_ns_not_fixed_output_slots"}else{"fixed_output_slots"},
                "lateAiPresentation":late_ai,
                "contentSlotAge":display.slot.saturating_sub(display.nominal_content_slot),
                "nativePresentCalled":success,"hostElapsedMs":now.duration_since(self.began).as_secs_f64()*1000.0,
                "latenessMs":now.saturating_duration_since(display.due).as_secs_f64()*1000.0,"intervalMs":interval,
                "presentBeginMonotonicNs":present_begin_ns,"presentCompleteMonotonicNs":present_complete_ns,
                "sourceCaptureMonotonicNs":display.source_monotonic_ns,"newerSourceCaptureMonotonicNs":display.newer_source_monotonic_ns,
                "newerSourceSequence":display.newer_source_sequence,"newerSourceSlot":display.newer_source_slot,
                "trueCaptureMidpointMonotonicNs":display.true_midpoint_monotonic_ns,
                "contentMidpointScheduledDueMonotonicNs":display.scheduled_due_monotonic_ns,
                "midpointClockQuantizationErrorNs":display.midpoint_clock_error_ns,
                "sourceCaptureToPresentBeginMs":present_begin_ns.zip(display.source_monotonic_ns).map(|(p,s)|p.saturating_sub(s) as f64/1e6)}));
            if success {self.last_present=Some(now);}
        }
        fn report(&mut self) {
            let snapshot_began_ns=super::monotonic_ns();
            let c=&self.counts;let elapsed=self.began.elapsed().as_secs_f64();
            let classifier_rows:Vec<_>={
                let observation=self.worker_stats.classifier.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                observation.iter().copied().collect()
            };
            let recent_classifier:Vec<_>=classifier_rows.into_iter().map(|v|json!({"epoch":v.pair.epoch,"olderSourceSequence":v.pair.older_sequence,"newerSourceSequence":v.pair.newer_sequence,
                "olderSourceSlot":v.pair.older_slot,"newerSourceSlot":v.pair.newer_slot,"midpointSlot":v.pair.midpoint_slot,
                "newerSourceDueSlot":v.pair.newer_due_slot,
                "olderSourceCaptureMonotonicNs":v.pair.older_monotonic_ns,"newerSourceCaptureMonotonicNs":v.pair.newer_monotonic_ns,
                "trueCaptureMidpointMonotonicNs":v.pair.true_midpoint_monotonic_ns,
                "scheduledDueMonotonicNs":v.pair.scheduled_due_monotonic_ns,"captureGapNs":v.pair.gap_ns,
                "midpointClockQuantizationErrorNs":v.pair.midpoint_clock_error_ns,
                "logits":v.logits,"probabilitySecond":v.probability_second,
                "highScore":v.high_score,"policyApplied":v.policy,"rejectedByPolicy":v.rejected,
                "semantics":"unknown_classifier_logits_not_a_validated_scene_cut_detector"})).collect();
            let last_classifier=recent_classifier.last().cloned();
            let mut times:Vec<_>=self.inference_ms.iter().copied().collect();times.sort_by(f64::total_cmp);
            let percentile=|p:f64|->Option<f64> {if times.is_empty(){None}else{times.get(((times.len() as f64*p).ceil() as usize).saturating_sub(1)).copied()}};
            let mut value=json!({"status":if self.failed.is_some(){"disabled_error_original_presentation_restored"}else if self.info.is_none(){"initializing"}else{"active_experimental_game_interpolation"},
                "pid":std::process::id(),"mode":self.mode,"targetSourceHz":if self.unlimited_native{None}else{Some(self.output_fps/2)},"targetPresentationHz":if self.unlimited_native{None}else{Some(self.output_fps)},"bufferedDelayMs":if self.unlimited_native{None}else{Some(200)},"unlimitedNativeCadence":self.unlimited_native,"maxNativeHoldMs":if self.unlimited_native{Some(50)}else{None},"aiOutputBudgetHz":self.output_fps,
                "inferenceWidth":if self.mode=="npu"{448}else{WIDTH},"inferenceHeight":if self.mode=="npu"{208}else{HEIGHT},
                "modelReadbackWidth":if self.mode=="npu"{MODEL_WIDTH}else{WIDTH},"modelReadbackHeight":if self.mode=="npu"{MODEL_HEIGHT}else{HEIGHT},
                "nativeBufferWidth":self.native_output.0,"nativeBufferHeight":self.native_output.1,
                "backendImagePath":if self.mode=="npu"{"original_resolution_RGBA8_GPU_history_plus_RGBA8_448x208_public_NNRT_raw_half6planes_then_original_resolution_GPU_warp"}else{"RGB8_320x192_RIFE_Vulkan_interpolation_RGB8_320x192"},
                "guestFinalSource":self.source_desc.map(|(w,h,f)|json!({"width":w,"height":h,"actualFormat":format!("{f:?}")})),
                "sourceBoundary":"actual_guest_opcode_7_final_texture_including_already_composited_game_HUD",
                "sourcePacing":if self.unlimited_native{"every_guest_source_owned_GPU_snapshot_no_output_clock_gate_no_CPU_readback_gate_one_pending_native_max50ms_AI_best_effort"}else{"GPU_guest_present_boundary_capped_at_half_configured_output_rate_while_continuing_output_ticks_and_map_callbacks; late_sources_do_not_catch_up"},
                "normalization":if self.mode=="npu"{"full_resolution_UNORM_logical_RGBA_private_history; bilinear_RGBA8_448x208_only_for_model; no_depth_camera_inputs"}else{"bilinear_encoded_UNORM_RGBA_to_320x192_RGB8_debug_backend"},
                "spatialUpscaling":self.sr_enabled,
                "visible":self.visible,"physicalWindow":self.physical,"worldPlayerTaskObserved":crate::ffi::world_player_observed(),
                "appPresentationAllowed":crate::ffi::app_presentation_allowed(),
                "appPresentationGeneration":crate::ffi::app_presentation_generation(),"historyPresentationGeneration":self.presentation_generation,
                "epoch":self.epoch.load(Ordering::Acquire),"elapsedSeconds":elapsed,"workerDevice":self.info,"error":self.failed,
                "bounds":{"readbacks":PENDING_LIMIT,"workerInputQueue":2,"workerResultQueue":6,"readyEvents":EVENT_LIMIT,"recentPresentRows":TRACE_LIMIT},
                "recentInferenceMs":self.inference_ms,"recentInferenceMedianMs":percentile(0.5),"recentInferenceP95Ms":percentile(0.95),
                "recentPresents":self.trace,"nativePresentCallsPerElapsedSecond":(c.source_presents+c.ai_presents+c.repeats) as f64/elapsed.max(0.001),
                "scope":"Actual game texture integration experiment. Host queue.present calls and deadlines do not prove compositor/display FPS. No catch-up bursts: at most one present per tick; elapsed slots are counted as missed. A novel late midpoint may be presented only before its real newer source becomes due; repeated content is always counted as repeat."});
            let counters=json!({
                "guestFinalFramesSeen":c.guest_sources,"selectedSourceFrames":c.selected_sources,"throttledSourceFrames":c.throttled_sources,
                "mappedSourceFrames":c.mapped_sources,"readbackBackpressureDrops":c.readback_drops,"workerBackpressureDrops":c.worker_drops,
                "aiCompleted":self.worker_stats.completed.load(Ordering::Relaxed),"aiResultsReceived":c.completed_ai,
                "workerResultBackpressureDrops":self.worker_stats.result_drops.load(Ordering::Relaxed),
                "workerStaleInferenceResults":self.worker_stats.stale_results.load(Ordering::Relaxed),
                "workerPresentationGateDrops":self.worker_stats.presentation_gate_drops.load(Ordering::Relaxed),
                "nativeSnapshotSupersededBeforePresent":c.native_superseded,"retiredReadbackCompletions":c.retired_readbacks,"retiredWorkerResults":c.retired_worker_results,
                "actualSourcePairCandidates":self.worker_stats.pair_candidates.load(Ordering::Relaxed),
                "nonAdjacentClockPairsRun":self.worker_stats.nonadjacent_pairs.load(Ordering::Relaxed),
                "sourcePairGapSkips":self.worker_stats.pair_gap_skips.load(Ordering::Relaxed),
                "sourcePairInvalidSkips":self.worker_stats.pair_invalid_skips.load(Ordering::Relaxed),
                "sourcePairDiscontinuitySkips":self.worker_stats.pair_discontinuity_skips.load(Ordering::Relaxed),
                "sourcePairDeadlineSkips":self.worker_stats.pair_deadline_skips.load(Ordering::Relaxed),
                "sourcePairBackendRuns":self.worker_stats.pair_runs.load(Ordering::Relaxed),
                "sourcePairEventCollisionSkips":c.pair_collisions,"orphanRgbMidpointResults":c.orphan_mids,
                "qualityGateRejectedMidpoints":self.worker_stats.rejected_mids.load(Ordering::Relaxed),
                "recentRejectedMidpoints":self.rejections,
                "lateAiDiscarded":c.late_ai,"lateAiPresented":c.late_ai_presented,
                "lateAiWindowExpired":c.ai_window_expired,"aiNonForwardDiscarded":c.ai_not_forward,
                "sourceNonForwardDiscarded":c.source_not_forward,"sourcePresents":c.source_presents,"aiPresents":c.ai_presents,
                "repeatPresents":c.repeats,"underrunSlots":c.underruns,"missedDeadlineSlots":c.missed_slots,"surfaceAcquireFailures":c.acquire_failures,
                "historyResets":c.resets,"eventCapacityDrops":c.event_drops,"pendingReadbacks":self.pending.len(),"readyEvents":self.events.len()
            });
            if let (Some(value),Some(counters))=(value.as_object_mut(),counters.as_object()) {value.extend(counters.clone());}
            let graph=json!({
                "classifierPolicy":self.classifier_policy,"classifierPolicyApplies":self.mode=="npu",
                "classifierSecondProbabilityThreshold":0.99,"classifierHighScorePairs":self.worker_stats.high_score_pairs.load(Ordering::Relaxed),
                "lastFiniteClassifier":last_classifier,
                "recentClassifierObservations":recent_classifier,"classifierObservationLimit":TRACE_LIMIT,
                "configuredOutputFps":self.output_fps,"outputPeriodNs":self.period_ns,"sourcePeriodNs":if self.unlimited_native{None}else{Some(self.source_ns)},"contentCoordinateUnits":if self.unlimited_native{"capture_elapsed_nanoseconds_sourceSlot_is_unique_guest_sequence"}else{"configured_output_clock_slots"},
                "clockConfiguration":"MUGUET_SUPPORT/config.json frame_interpolation_output_fps read_once_at_interpolator_creation; default_30",
                "pairingPolicy":"consecutive_mapped_source_ordinals_in_same_epoch_with_explicit_actual_endpoints; checked_olderClockSlot_plus_newerClockSlot; model_t_0_5; no_intermediate_source_event_overwrite; no_clock_adjacent_requirement",
                "lateAiPolicy":"nominal_midpoint_slot_lte_actual_output_slot_lt_actual_newer_source_due_slot; strictly_forward_content_slots; same_content_key_only_repeat; chronological_latest_eligible_event_wins; count_late_only_after_successful_native_present",
                "sourcePairMaximumGapNs":super::pairing::MAX_GAP_NS,
                "pairingTimingScope":"250ms is a conservative scheduling limit, not a validated model quality bound. Both real endpoints must exist before inference. Worker submission and novel midpoint presentation stop at the newer source's delayed due slot. Clock-bucket nominal midpoint retains adjacent-pair scheduling and may precede the true capture midpoint by less than one source period; signed quantization error and actual presentation slot are reported. Capture timestamps are CPU source-record samples, not GPU completion.",
                "sourcePairDeadlineScope":"sourcePairDeadlineSkips counts pairs whose newer-source usable-window deadline has already closed before backend submission",
                "presentationPausePolicy":"atomic_allowed_and_generation_checked_at_worker_receive_before_backend_call_after_backend_return_and_at_result_collection; generation_change_resets_history_even_after_quick_background_foreground; inflight_NNRT_RIFE_calls_finish_naturally_and_retired_results_are_discarded; no_join_or_forced_inference_cancellation",
                "postSrRepeatCache":crate::gpu::output_cache_status(),
                "pipeline":if self.mode=="npu"{if self.sr_enabled{"real_guest_texture_to_owned_full_resolution_GPU_history_to_NNRT_raw_flow_to_GPU_midpoint_warp_to_public_XEngine_SR_to_game_window"}else{"real_guest_texture_to_owned_full_resolution_GPU_history_to_NNRT_raw_flow_to_GPU_midpoint_warp_to_game_window"}}else{"RIFE_RGB8_debug_path_with_optional_public_XEngine_SR"},
                "gpuWarpEncoded":c.gpu_warps,"orphanFlowResults":c.orphan_flows,"outputSlotsSkipped":c.output_skips,
                "gpuHistoryTextures":self.history.len(),"gpuHistoryTextureLimit":8,
                "lastNnrtRunSyncNs":self.worker_stats.last_run_sync_ns.load(Ordering::Relaxed),
                "lastHalfFiniteScanNs":self.worker_stats.last_finite_scan_ns.load(Ordering::Relaxed),
                "cpuWarpEnabled":false,"gpuWarpEnabled":self.mode=="npu",
                "maskStorage":"model_grid_sigmoid_then_RG16Float; quantization_may_change_rounded_RGB_by_one_byte",
                "outputScheduling":"independent_output_encoders_and_three_SR_slots_with_individual_completed_submission_fences; no_guest_pass_flush",
                "timestampClock":"CLOCK_MONOTONIC_ns_explicit_clock_gettime; Instant_elapsed_is_separate",
                "outputCaptureDirectory":self.output_capture,"outputCaptureFrames":self.output_capture_rows
            });
            if let (Some(value),Some(graph))=(value.as_object_mut(),graph.as_object()){value.extend(graph.clone());}
            if self.unlimited_native {
                if let Some(value)=value.as_object_mut() {
                    value.insert("pairingPolicy".into(),json!("consecutive_real_guest_source_sequences_same_epoch; true_capture_midpoint_ns; no_clock_bucket_snap; skipped_AI_samples_break_pair_chain"));
                    value.insert("lateAiPolicy".into(),json!("actual_now_before_newer_native_bounded_deadline_and_content_strictly_after_last_successful_present; native_ready_or_due_always_wins; no_repeat_tick"));
                    value.insert("pairingTimingScope".into(),json!("Native sources are not paced by AI output budget. At most one pending source is released by the next arrival or clamped previous capture gap (1..50ms). First native source presents immediately. AI may be skipped if capture/readback/inference misses this window; source timestamps are CPU capture samples, not GPU completion or display time."));
                    value.insert("scope".into(),json!("Host native queue.present calls do not prove compositor/display FPS. Unlimited mode has no fixed source or output presentation clock; AI alone has a configured best-effort rate budget. Native GPU snapshots do not await model readback. SR retains its real submission-fence backpressure. Two native snapshot slots bound overload; supersession is counted. No extrapolation, no repeated-content clock, no backwards midpoint."));
                    value.insert("nativePendingSnapshots".into(),json!(self.native_pacer.busy()));
                    value.insert("nativeSnapshotLimit".into(),json!(2));
                }
            }
            self.status_writer.submit(value,snapshot_began_ns,super::monotonic_ns());
        }
    }
}
pub(crate) use implementation::Interpolator;
