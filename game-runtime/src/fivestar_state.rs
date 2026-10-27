//! Read-only control context from Five Star Port 8e2e4628's EngineHost.c and
//! NativeTouchContext.h. Original copyright (c) 2026 GTA V iOS port contributors,
//! MIT (upstream/fivestar-input-8e2e4628/LICENSE); HarmonyOS adaptation GPL-3.0-or-later.
//! Offsets are for the byte-identical
//! pinned game.wasm. Published only after the new ioPad hook actually runs.
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use wasmtime::SharedMemory;

const PLAYER_INFO: u64 = 20_159_720;
const POLL_WINDOW_MS: u64 = 700;
static STORY_MODE: AtomicBool = AtomicBool::new(false);
static TRAILER_ATTACHED: AtomicBool = AtomicBool::new(false);

/// Separate poll and script-read tables mirror PGInputPolledWithin and
/// PGInputReadWithin. Script wrappers inform diagnostics, never layout polls.
struct Reads { polled: [u64; 400], read: [u64; 400], native_count: u64, script_count: u64 }
static READS: Mutex<Reads> = Mutex::new(Reads { polled:[0;400],read:[0;400],native_count:0,script_count:0 });
fn now_ms() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_millis().min(u64::MAX as u128 - 1) as u64
}
pub fn record_control_read(_kind: u32, _control: u32, action: u32, _value: f32) {
    if action >= 400 { return; }
    let mut r=READS.lock().unwrap();let at=now_ms()+1;
    r.polled[action as usize]=at;r.read[action as usize]=at;r.native_count+=1;
}
pub fn record_script_read(mem:&SharedMemory, _kind:u32, info:u64, _float_answer:bool) {
    let memory=SharedView::new(mem);
    if let Some(args)=memory.u64(info.saturating_add(16)) {
        if let Some(action)=memory.u32(args.saturating_add(8)).filter(|a|*a<400) {
            let mut r=READS.lock().unwrap();r.read[action as usize]=now_ms()+1;r.script_count+=1;
        }
    }
}
fn recent(table:&[u64;400],at:u64)->Vec<u32> {
    table.iter().enumerate().filter(|(_,t)|**t>0&&at.saturating_sub(**t-1)<POLL_WINDOW_MS)
        .map(|(n,_)|n as u32).collect()
}
pub fn set_story_mode(story:bool) { STORY_MODE.store(story,Ordering::Relaxed); }
pub fn set_trailer_attached(attached:bool) { TRAILER_ATTACHED.store(attached,Ordering::Relaxed); }

trait Memory {
    fn len(&self)->u64;
    fn u8(&self,at:u64)->Option<u8>;
    fn u16(&self,at:u64)->Option<u16>;
    fn u32(&self,at:u64)->Option<u32>;
    fn u64(&self,at:u64)->Option<u64>;
    fn at_u8(&self,base:u64,off:u64)->Option<u8>{self.u8(base.checked_add(off)?)}
    fn at_u32(&self,base:u64,off:u64)->Option<u32>{self.u32(base.checked_add(off)?)}
    fn at_u64(&self,base:u64,off:u64)->Option<u64>{self.u64(base.checked_add(off)?)}
    fn at_f32(&self,base:u64,off:u64)->Option<f32>{self.at_u32(base,off).map(f32::from_bits).filter(|n|n.is_finite())}
    fn bytes(&self,at:u64,n:u64)->Option<Vec<u8>>{
        if at.checked_add(n)?>self.len(){return None;}
        (0..n).map(|i|self.u8(at+i)).collect()
    }
}
struct SharedView { base:*mut u8,len:usize }
impl SharedView {
    fn new(mem:&SharedMemory)->Self {let(base,len)=crate::mem_base_len(mem);Self{base,len}}
    fn valid(&self,at:u64,n:u64)->bool {at%n==0&&at.checked_add(n).is_some_and(|end|end<=self.len as u64)}
}
impl Memory for SharedView {
    fn len(&self)->u64{self.len as u64}
    fn u8(&self,at:u64)->Option<u8>{self.valid(at,1).then(||unsafe{(&*(self.base.add(at as usize)as*const AtomicU8)).load(Ordering::Relaxed)})}
    fn u16(&self,at:u64)->Option<u16>{self.valid(at,2).then(||unsafe{(&*(self.base.add(at as usize)as*const AtomicU16)).load(Ordering::Relaxed)})}
    fn u32(&self,at:u64)->Option<u32>{self.valid(at,4).then(||unsafe{(&*(self.base.add(at as usize)as*const AtomicU32)).load(Ordering::Relaxed)})}
    fn u64(&self,at:u64)->Option<u64>{self.valid(at,8).then(||unsafe{(&*(self.base.add(at as usize)as*const AtomicU64)).load(Ordering::Relaxed)})}
}

#[derive(Default,Serialize,Debug)]
#[serde(rename_all="camelCase")]
struct Hints {
    special_shown:bool,special_ready:bool,special_active:bool,special_fill:f32,
    can_detonate:bool,help_detonate:bool,help_shown:bool,help_context:bool,help_inputs:u32,
    radio_wheel_open:bool,vehicle_model:u32,ped_model:u32,trailer_attached:bool,
    passenger:bool,taxi_menu:bool,phone_browser:bool,
}
fn player(memory:&impl Memory)->Option<(u64,u64)> {
    let info=memory.u64(PLAYER_INFO).filter(|n|*n!=0)?;
    let ped=memory.at_u64(info,8).filter(|n|*n!=0&&n.checked_add(5584).is_some_and(|end|end<=memory.len()))?;
    Some((info,ped))
}
fn vehicle_context(memory:&impl Memory)->u32 {
    let Some((info,ped))=player(memory)else{return 0};
    if memory.at_u8(ped,5291).unwrap_or(0)&64==0{return 0;}
    let Some(vehicle)=memory.at_u64(ped,5576).filter(|n|*n!=0)else{return 0};
    let Some(kind)=memory.at_u32(vehicle,3088).filter(|n|*n<=31)else{return 0};
    // Discard a torn player/vehicle pointer chain, as upstream does.
    if memory.at_u64(info,8)!=Some(ped)||memory.at_u64(ped,5576)!=Some(vehicle)||memory.at_u8(ped,5291).unwrap_or(0)&64==0{return 0;}
    kind+1
}
fn status(memory:&impl Memory)->u32 {
    let mut bits=0;
    if let Some((_,ped))=player(memory) {
        if let Some(controls)=memory.at_u64(ped,4328).filter(|n|*n!=0).and_then(|info|memory.at_u32(info,936)) {
            if controls!=0 {bits|=1;}
        }else{bits|=16;}
    }else{bits|=16;}
    if memory.u8(12_321_468).unwrap_or(0)!=0{bits|=32;}
    if memory.u8(19_505_653).unwrap_or(0)!=0{bits|=2;}
    if memory.u8(19_577_926).unwrap_or(0)!=0||memory.u8(19_577_929).unwrap_or(0)&1!=0{bits|=8;}
    if memory.u64(19_339_168).filter(|n|*n!=0).and_then(|p|memory.at_u8(p,3215)).unwrap_or(0)!=0{bits|=4;}
    bits
}
fn can_detonate(m:&impl Memory,ped:u64)->bool {
    let Some(count)=m.u32(28_498_240)else{return false};
    let Some(table)=m.u64(28_496_576).filter(|n|*n!=0)else{return false};
    let Some(infos)=m.u16(28_496_584)else{return false};
    for i in 0..count.min(50) {
        let Some(p)=m.u64(28_497_840+8*i as u64).filter(|n|*n!=0)else{continue};
        if m.at_u64(p,1320)!=Some(ped){continue;}
        let Some(hash)=m.at_u32(p,1312).filter(|n|*n!=0)else{continue};
        let(mut lo,mut hi)=(0,infos as i32-1);let mut info=0;
        for _ in 0..20 {
            if lo>hi{break;}let mid=(lo+hi)/2;
            let Some(candidate)=m.at_u64(table,8*mid as u64).filter(|n|*n!=0)else{break};
            let Some(key)=m.at_u32(candidate,16)else{break};
            if key==hash{info=candidate;break;}if key<hash{lo=mid+1;}else{hi=mid-1;}
        }
        if info!=0&&m.at_u8(info,2255).unwrap_or(0)&4!=0&&m.at_u8(p,1538).unwrap_or(0)!=0{return true;}
    }
    false
}
fn contains(text:&[u8],token:&str)->bool {text.windows(token.len()).any(|v|v==token.as_bytes())}
fn help_hints(text:&[u8],h:&mut Hints) {
    // CMessages retains three 600-byte slots. Only the first NUL-terminated
    // message is visible; later slots can still contain an old prompt.
    let end=text.iter().position(|c|*c==0).unwrap_or(text.len());let text=&text[..end];
    h.help_shown=true;
    h.help_context=["INPUT_CONTEXT","INPUT_TALK","~INPUT_SCRIPT_PAD_RIGHT~"].iter().any(|t|contains(text,t));
    h.help_detonate=contains(text,"~INPUT_DETONATE~");
    const NAMED:&[(&str,u32)]=&[
        ("~INPUT_SCRIPT_LEFT_AXIS_X~",218),("~INPUT_SCRIPT_LEFT_AXIS_Y~",219),
        ("~INPUT_SCRIPT_RIGHT_AXIS_X~",220),("~INPUT_SCRIPT_RIGHT_AXIS_Y~",221),
        ("~INPUTGROUP_SCRIPT_RSTICK_ALL~",220),("~INPUT_SCRIPT_RUP~",222),
        ("~INPUT_SCRIPT_RDOWN~",223),("~INPUT_SCRIPT_RLEFT~",224),("~INPUT_SCRIPT_RRIGHT~",225),
        ("~INPUT_SCRIPT_LB~",226),("~INPUT_SCRIPT_RB~",227),("~INPUT_SCRIPT_LT~",228),("~INPUT_SCRIPT_RT~",229),
        ("~INPUT_SCRIPT_LS~",230),("~INPUT_SCRIPT_RS~",231),("~PAD_RSTICK_ROTATE~",220),
        ("~PAD_RSTICK_ROTATE~",229),("~INPUTGROUP_SCRIPT_DPAD_ALL~",232),
        ("~INPUT_SCRIPT_PAD_UP~",232),("~INPUT_SCRIPT_PAD_DOWN~",233),("~INPUT_SCRIPT_PAD_LEFT~",234)];
    for(token,input)in NAMED{if contains(text,token){h.help_inputs|=1<<(input-218);}}
}
fn hints(m:&impl Memory,context:u32)->Hints {
    let mut h=Hints::default();let Some((_,ped))=player(m)else{return h};
    let ability=m.at_u32(ped,5088).filter(|slot|*slot<2)
        .and_then(|slot|m.at_u64(ped,5072+8*slot as u64)).filter(|a|*a!=0);
    if let Some(ability)=ability {
        if let(Some(data),Some(model_info))=(m.u64(ability).filter(|n|*n!=0),m.at_u64(ped,32).filter(|n|*n!=0)) {
            let enabled=m.at_u8(ability,106).unwrap_or(0);let active=m.at_u8(ability,107).unwrap_or(0);
            let current=m.at_f32(ability,52).unwrap_or(0.0);let maximum=m.at_f32(ability,60).unwrap_or(0.0);
            let minimum=m.at_u32(data,8).unwrap_or(0)as i32;let kind=m.at_u32(data,60).unwrap_or(0)as i32;
            let model=m.at_u32(model_info,24).unwrap_or(0);let mut unlocked=false;
            for(model_at,flag_at)in [(19_625_424,20_197_688),(19_625_440,20_197_689),(19_625_456,20_197_690)] {
                if model!=0&&m.u32(model_at)==Some(model){unlocked=m.u8(flag_at).unwrap_or(0)!=0;break;}
            }
            let driving=context!=0&&![2,9,10,11,13,14,16].contains(&context);
            if enabled&1!=0&&unlocked&&kind!=-1&&(kind>=1||(kind==0&&driving)) {
                h.special_shown=true;h.special_active=active&1!=0;
                h.special_fill=if maximum>0.0{(current/maximum).clamp(0.0,1.0)}else{0.0};
                h.special_ready=!h.special_active&&current>0.0&&current.floor()>=3.0&&current.round()>=minimum as f32;
            }
        }
    }
    h.can_detonate=can_detonate(m,ped);h.trailer_attached=TRAILER_ATTACHED.load(Ordering::Relaxed);
    h.ped_model=m.at_u64(ped,32).filter(|p|*p!=0).and_then(|p|m.at_u32(p,24)).unwrap_or(0);
    if m.u8(25_477_136).unwrap_or(0)!=0 {
        if let Some(text)=m.bytes(25_477_136,1800){help_hints(&text,&mut h);}
    }
    h.radio_wheel_open=m.u32(19_480_044).unwrap_or(0)&1!=0;
    if context!=0 {
        if let Some(vehicle)=m.at_u64(ped,5576).filter(|p|*p!=0) {
            h.vehicle_model=m.at_u64(vehicle,32).filter(|p|*p!=0).and_then(|p|m.at_u32(p,24)).unwrap_or(0);
            if h.vehicle_model!=0{h.passenger=m.at_u64(vehicle,3216).is_some_and(|driver|driver!=ped);}
        }
    }
    if let(Some(threads),Some(count))=(m.u64(15_409_704).filter(|p|*p!=0),m.u16(15_409_712)) {
        for i in 0..count.min(512) {
            let Some(thread)=m.at_u64(threads,8*i as u64).filter(|p|*p!=0)else{continue};
            let Some(name)=thread.checked_add(428).and_then(|at|m.bytes(at,12))else{continue};
            if h.passenger&&name.eq_ignore_ascii_case(b"taxiservice\0"){h.taxi_menu=true;}
            if name.eq_ignore_ascii_case(b"appinternet\0"){h.phone_browser=true;}
        }
    }
    h
}

struct Published { state:Option<Value>,at:Option<Instant>,seq:u64,generation:u64 }
static PUBLISHED:Mutex<Published>=Mutex::new(Published{state:None,at:None,seq:0,generation:0});
pub fn publish(mem:&SharedMemory) {
    let mut p=PUBLISHED.lock().unwrap();let now=Instant::now();
    let generation=crate::ffi::app_presentation_generation();
    if p.generation==generation&&p.at.is_some_and(|at|now.saturating_duration_since(at).as_millis()<16){return;}
    let m=SharedView::new(mem);let vehicle=vehicle_context(&m);
    let pause_menu=(m.u8(19_565_906).unwrap_or(0)|m.u8(19_566_104).unwrap_or(0))&1!=0;
    p.seq=p.seq.wrapping_add(1);p.generation=generation;
    p.state=Some(json!({"known":player(&m).is_some(),"hookReady":true,
        "vehicle":vehicle,"pauseMenu":pause_menu,"status":status(&m),
        "characterSwitchEnabled":STORY_MODE.load(Ordering::Relaxed),"hints":hints(&m,vehicle),
        "lastInputDevice":m.u32(24_627_136).map(|n|n as i32),
        "targetingMode":m.u32(19_566_124).map(|n|n as i32)}));
    p.at=Some(now);
}
pub fn snapshot_json()->Value {
    let p=PUBLISHED.lock().unwrap();let generation=crate::ffi::app_presentation_generation();
    let age=p.at.map(|at|at.elapsed().as_millis().min(u64::MAX as u128)as u64);
    let mut state=p.state.clone().unwrap_or_else(||json!({"known":false,"hookReady":false,
        "vehicle":0,"pauseMenu":false,"status":16,"characterSwitchEnabled":STORY_MODE.load(Ordering::Relaxed),
        "hints":Hints::default(),"reason":"awaitingIoPadHook"}));
    if generation!=p.generation||age.is_none_or(|ms|ms>500){state["known"]=false.into();}
    let reads=READS.lock().unwrap();let now=now_ms();
    state["missionInputs"]=json!(recent(&reads.polled,now));
    state["scriptReadInputs"]=json!(recent(&reads.read,now));
    state["nativeControlReads"]=reads.native_count.into();state["scriptControlReads"]=reads.script_count.into();
    state["seq"]=p.seq.into();state["generation"]=p.generation.into();state["ageMs"]=json!(age);
    state["source"]=json!("kevinerikjs/gta5-ios-port@8e2e4628");state
}

#[cfg(test)]
mod tests {
    use super::*;use std::collections::HashMap;
    #[derive(Default)]struct Fake{words:HashMap<u64,u64>}
    impl Fake{fn put(&mut self,at:u64,n:u64){self.words.insert(at,n);}}
    impl Memory for Fake {
        fn len(&self)->u64{32<<20}fn u8(&self,at:u64)->Option<u8>{self.words.get(&at).map(|v|*v as u8)}
        fn u16(&self,at:u64)->Option<u16>{self.words.get(&at).map(|v|*v as u16)}
        fn u32(&self,at:u64)->Option<u32>{self.words.get(&at).map(|v|*v as u32)}
        fn u64(&self,at:u64)->Option<u64>{self.words.get(&at).copied()}
    }
    #[test]fn context_uses_engine_vehicle_kind_and_status_flags(){
        let mut m=Fake::default();m.put(PLAYER_INFO,1000);m.put(1008,2000);m.put(2000+5291,64);
        m.put(2000+5576,10000);m.put(10000+3088,8);assert_eq!(vehicle_context(&m),9);
        m.put(2000+4328,8000);m.put(8000+936,1);m.put(12_321_468,1);m.put(19_505_653,1);
        assert_eq!(status(&m),35);m.put(2000+5291,0);assert_eq!(vehicle_context(&m),0);
    }
    #[test]fn malformed_chains_and_sparse_memory_cannot_panic_or_invent_player(){
        let mut m=Fake::default();assert_eq!(status(&m),16);assert_eq!(vehicle_context(&m),0);
        m.put(PLAYER_INFO,u64::MAX);assert_eq!(status(&m),16);assert_eq!(vehicle_context(&m),0);
        assert!(!hints(&m,0).special_shown);
        m.put(PLAYER_INFO,1000);m.put(1008,u64::MAX);
        assert!(player(&m).is_none());assert_eq!(status(&m),16);
    }
    #[test]fn tooth_rotation_names_both_right_stick_and_rt_and_context_is_preserved(){
        let mut h=Hints::default();help_hints(b"Rotate ~PAD_RSTICK_ROTATE~. ~INPUT_SCRIPT_PAD_RIGHT~ ~INPUT_DETONATE~",&mut h);
        assert!(h.help_context&&h.help_detonate&&h.help_shown);
        assert_eq!(h.help_inputs,(1<<(220-218))|(1<<(229-218)));
    }
    #[test]fn stale_tokens_after_the_visible_help_string_do_not_create_buttons(){
        let mut h=Hints::default();help_hints(b"Drive to the checkpoint.\0Old ~INPUT_CONTEXT~ ~INPUT_DETONATE~ ~INPUT_SCRIPT_RT~",&mut h);
        assert!(h.help_shown);assert!(!h.help_context);assert!(!h.help_detonate);assert_eq!(h.help_inputs,0);
    }
    #[test]fn poll_window_expires_and_never_read_inputs_stay_absent(){
        let mut table=[0;400];table[229]=101;
        assert_eq!(recent(&table,799),vec![229]);assert!(recent(&table,800).is_empty());
    }
}
