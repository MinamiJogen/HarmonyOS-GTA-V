#!/usr/bin/env python3
"""Exercise the actual buffer-init function with mock HAL; no game/device build.

The complete production function is extracted verbatim except its OHOS cfg is
removed in this isolated host crate. The reference removes only the new branch.
The production InitTracker implementation and env helper are compiled as well.
HAL and device-state tracker are recording mocks, not Vulkan validation.
"""
from pathlib import Path
import hashlib
import json
import os
import subprocess

ROOT = Path(__file__).resolve().parents[2]
MEMORY = ROOT / "game-runtime/vendor/wgpu-core/src/command/memory_init.rs"
HELPER = MEMORY.with_name("empty_init_transition.rs")
INIT = ROOT / "game-runtime/vendor/wgpu-core/src/init_tracker/mod.rs"
BUFFER = INIT.with_name("buffer.rs")
CONFIG = ROOT / "game-runtime/src/config.rs"
OUT = ROOT / "native-platform/evidence/queue-submit-review/empty-init-tests"
HOST = ROOT / "game-runtime/logs/empty-init-transition-host"
CFG = '#[cfg(all(target_env = "ohos", feature = "std"))]'


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def block(text, marker):
    start = text.index(marker)
    i = text.index("{", start)
    depth = 0
    while i < len(text):
        if text.startswith("//", i):
            end = text.find("\n", i)
            i = len(text) if end < 0 else end + 1
            continue
        if text.startswith("/*", i):
            i = text.index("*/", i + 2) + 2
            continue
        if text[i] == '"':
            i += 1
            while i < len(text):
                if text[i] == "\\":
                    i += 2
                elif text[i] == '"':
                    i += 1
                    break
                else:
                    i += 1
            continue
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                return text[start:i + 1]
        i += 1
    raise AssertionError("Unterminated Rust block")


PRELUDE = r'''
#![allow(dead_code, unexpected_cfgs)]
extern crate alloc;
extern crate self as wgt;
use core::{fmt, iter, ops::Range};
use std::{collections::{HashMap, hash_map::{Entry, DefaultHasher}}, hash::BuildHasherDefault,
          sync::{Arc, Mutex, MutexGuard}};
use smallvec::SmallVec;
pub type BufferAddress = u64;
pub const COPY_BUFFER_ALIGNMENT: u64 = 4;
type FastHashMap<K,V> = HashMap<K,V,BuildHasherDefault<DefaultHasher>>;
#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub struct BufferUses(u8);
impl BufferUses { pub const COPY_DST: Self = Self(4); }
#[derive(Debug,Clone,PartialEq,Eq)]
enum Event { TryRaw(u32), SetSingle(u32,BufferUses), Transition(Vec<(u32,BufferUses,BufferUses)>), Clear(u32,Range<u64>) }
type Journal = Arc<Mutex<Vec<Event>>>;
#[derive(Debug)]
struct WriteLock<T>(Mutex<T>);
impl<T> WriteLock<T> { fn write(&self)->MutexGuard<'_,T>{self.0.lock().unwrap()} }
#[derive(Debug)]
struct RawBuffer { id:u32, journal:Journal }
#[derive(Debug)]
struct Buffer { id:u32, initialization_status:WriteLock<InitTracker<u64>>, raw:RawBuffer, destroyed:bool }
impl Buffer {
 fn tracker_index(&self)->u32 {self.id}
 fn try_raw<'a>(&'a self,_:&SnatchGuard<'_>)->Result<&'a RawBuffer,DestroyedResourceError>{
  self.raw.journal.lock().unwrap().push(Event::TryRaw(self.id));
  if self.destroyed {Err(DestroyedResourceError(self.id))} else {Ok(&self.raw)}
 }
}
#[derive(Debug,Clone,PartialEq,Eq)]
struct DestroyedResourceError(u32);
struct SnatchGuard<'a>(std::marker::PhantomData<&'a ()>);
#[derive(Clone)]
struct HalBarrier {id:u32,from:BufferUses,to:BufferUses}
struct Pending(HalBarrier);
impl Pending {fn into_hal(self,_:&Arc<Buffer>,_:&SnatchGuard<'_>)->HalBarrier{self.0}}
struct MockBuffers {states:HashMap<u32,BufferUses>,journal:Journal}
impl MockBuffers {
 fn set_single(&mut self,b:&Arc<Buffer>,state:BufferUses)->Option<Pending>{
  self.journal.lock().unwrap().push(Event::SetSingle(b.id,state));
  let old=self.states.insert(b.id,state).unwrap_or(BufferUses(0));
  Some(Pending(HalBarrier{id:b.id,from:old,to:state}))
 }
}
struct DeviceTracker {buffers:MockBuffers}
struct RawEncoder {journal:Journal}
impl RawEncoder {
 unsafe fn transition_buffers(&mut self,b:&[HalBarrier]){
  self.journal.lock().unwrap().push(Event::Transition(b.iter().map(|v|(v.id,v.from,v.to)).collect()));
 }
 unsafe fn clear_buffer(&mut self,b:&RawBuffer,r:Range<u64>){
  self.journal.lock().unwrap().push(Event::Clear(b.id,r));
 }
}
struct Encoder {raw:RawEncoder}
struct BakedCommands {buffer_memory_init_actions:Vec<BufferInitTrackerAction>,encoder:Encoder}
'''


TESTS = r'''
#[cfg(test)]
mod tests {
 use super::*;
 #[derive(Clone)]
 struct Case {name:&'static str,preinit:Vec<Range<u64>>,actions:Vec<(MemoryInitKind,Range<u64>)>,clears:Vec<Range<u64>>,empty:bool,destroyed:bool}
 struct ResultState {events:Vec<Event>,remaining:Vec<Range<u64>>,device:BufferUses,result:Result<(),DestroyedResourceError>,actions_left:usize}
 fn run(c:&Case,reference:bool)->ResultState {
  let journal=Arc::new(Mutex::new(vec![]));
  let mut init=InitTracker::new(32u64);
  for r in &c.preinit {drop(init.drain(r.clone()));}
  let b=Arc::new(Buffer{id:7,initialization_status:WriteLock(Mutex::new(init)),raw:RawBuffer{id:7,journal:journal.clone()},destroyed:c.destroyed});
  let actions=c.actions.iter().map(|(kind,r)|BufferInitTrackerAction{buffer:b.clone(),range:r.clone(),kind:*kind}).collect();
  let mut baked=BakedCommands{buffer_memory_init_actions:actions,encoder:Encoder{raw:RawEncoder{journal:journal.clone()}}};
  let mut tracker=DeviceTracker{buffers:MockBuffers{states:HashMap::from([(7,BufferUses(1))]),journal:journal.clone()}};
  let guard=SnatchGuard(std::marker::PhantomData);
  let result=if reference {baked.initialize_buffer_memory_reference(&mut tracker,&guard)} else {baked.initialize_buffer_memory(&mut tracker,&guard)};
  let remaining=b.initialization_status.write().uninitialized(0..32).collect();
  let events=journal.lock().unwrap().clone();
  ResultState{events,remaining,device:tracker.buffers.states[&7],result,actions_left:baked.buffer_memory_init_actions.len()}
 }
 #[test]
 fn actual_complete_buffer_init_function_matrix() {
  use MemoryInitKind::{ImplicitlyInitialized as I,NeedsInitializedMemory as N};
  let enabled=std::env::var_os("MUGUET_SKIP_EMPTY_INIT_TRANSITION").is_some_and(|v|v=="1");
  let cases=vec![
   Case{name:"same-CB full write then read",preinit:vec![],actions:vec![(I,0..16),(N,0..16)],clears:vec![],empty:true,destroyed:false},
   Case{name:"queue-write between encode and submit",preinit:vec![0..16],actions:vec![(N,0..16)],clears:vec![],empty:true,destroyed:false},
   Case{name:"first read requires clear",preinit:vec![],actions:vec![(N,0..16)],clears:vec![0..16],empty:false,destroyed:false},
   Case{name:"partial write retains uncovered clear",preinit:vec![],actions:vec![(I,0..8),(N,0..16)],clears:vec![8..16],empty:false,destroyed:false},
   Case{name:"touching ranges collapse",preinit:vec![],actions:vec![(N,0..8),(N,8..16)],clears:vec![0..16],empty:false,destroyed:false},
   Case{name:"disjoint ranges remain",preinit:vec![],actions:vec![(N,0..4),(N,8..12)],clears:vec![0..4,8..12],empty:false,destroyed:false},
   Case{name:"read before write still requires clear",preinit:vec![],actions:vec![(N,0..16),(I,0..16)],clears:vec![0..16],empty:false,destroyed:false},
   Case{name:"duplicate reads do not skip nonempty aggregate",preinit:vec![],actions:vec![(N,0..16),(N,0..16)],clears:vec![0..16],empty:false,destroyed:false},
   Case{name:"end rounded to COPY_BUFFER_ALIGNMENT",preinit:vec![],actions:vec![(N,0..5)],clears:vec![0..8],empty:false,destroyed:false},
   Case{name:"empty destroyed retains same error",preinit:vec![0..16],actions:vec![(N,0..16)],clears:vec![],empty:true,destroyed:true},
   Case{name:"nonempty destroyed retains old sequence",preinit:vec![],actions:vec![(N,0..16)],clears:vec![],empty:false,destroyed:true},
   Case{name:"no init actions",preinit:vec![],actions:vec![],clears:vec![],empty:false,destroyed:false},
   Case{name:"implicit actions only",preinit:vec![],actions:vec![(I,0..16)],clears:vec![],empty:false,destroyed:false},
  ];
  for c in cases {
   let old=run(&c,true);let new=run(&c,false);
   assert_eq!(old.result,new.result,"{}",c.name);
   assert_eq!(old.remaining,new.remaining,"{}",c.name);
   assert_eq!((old.actions_left,new.actions_left),(0,0),"{}",c.name);
   let clears:Vec<_>=new.events.iter().filter_map(|e|if let Event::Clear(_,r)=e{Some(r.clone())}else{None}).collect();
   assert_eq!(clears,c.clears,"{}",c.name);
   let raw_checks=|events:&[Event]|events.iter().filter(|e|matches!(e,Event::TryRaw(_))).count();
   assert_eq!(raw_checks(&old.events),raw_checks(&new.events),"{}",c.name);
   if enabled && c.empty {
    assert_eq!(new.events,vec![Event::TryRaw(7)],"{}",c.name);
    assert_eq!(new.device,BufferUses(1),"skip must not poison usage: {}",c.name);
    assert_eq!(old.device,BufferUses::COPY_DST,"reference synthetic state: {}",c.name);
   } else {
    assert_eq!(old.events,new.events,"old path must be exact: {}",c.name);
    assert_eq!(old.device,new.device,"{}",c.name);
   }
   println!("PASS enabled={enabled} case={}",c.name);
  }
 }
}
'''


CONFIG_TEST = r'''
fn code_embedded()->bool{true}
mod gpu {pub fn max_msaa()->u32{4}}
mod cpu_profile {pub fn configure(_:bool){}}
mod qos {
 pub static ENABLED: std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
 pub fn configure(value:bool){ENABLED.store(value,std::sync::atomic::Ordering::Relaxed);}
}
mod actual_config {
 include!(CONFIG_PATH);
 #[cfg(test)] mod strict_flags {
  use super::*;
  #[test] fn actual_config_strict_false_flags_and_qualified_defaults(){
   let dir=std::env::temp_dir().join(format!("empty-init-actual-config-{}",std::process::id()));
   std::fs::create_dir_all(&dir).unwrap();let path=dir.join("config.json");
   std::env::set_var("MUGUET_SUPPORT",&dir);
   for key in ["MUGUET_RES","MUGUET_SCALE","MUGUET_FPS"]{std::env::remove_var(key);}
   let defaults:serde_json::Value=serde_json::from_str(DEFAULT_CONFIG).unwrap();
   for key in ["skip_empty_init_transition","readback_pool","luminance_no_zero"]{assert_eq!(defaults[key],false);}
   for key in ["thread_qos","depth_reduce_opt","depth_reduce_no_zero"]{assert_eq!(defaults[key],true);}
   assert_eq!(defaults["scale"],0.4);assert_eq!(defaults["fps_limit"],30);
   for (key,envkey,otherenv) in [
    ("skip_empty_init_transition","MUGUET_SKIP_EMPTY_INIT_TRANSITION","MUGUET_READBACK_POOL"),
    ("readback_pool","MUGUET_READBACK_POOL","MUGUET_SKIP_EMPTY_INIT_TRANSITION"),
    ("luminance_no_zero","MUGUET_LUMINANCE_NO_ZERO","MUGUET_READBACK_POOL")
   ]{
    let values:Vec<Option<serde_json::Value>>=vec![None,Some(false.into()),Some(true.into()),Some("true".into()),Some(1.into()),Some(serde_json::Value::Null),Some(serde_json::json!([])),Some(serde_json::json!({})),Some(true.into()),Some(false.into()),None];
    for (i,value) in values.into_iter().enumerate(){
     let mut cfg=serde_json::json!({"resolution":"1200x800","fps_limit":60});
     if let Some(v)=value.clone(){cfg[key]=v;}
     // Deliberately stale values must be cleared, not left to presence checks.
     std::env::set_var(envkey,"stale");std::env::set_var(otherenv,"1");
     std::fs::write(&path,serde_json::to_string(&cfg).unwrap()).unwrap();
     let args=settings(&path);
     let expected=value.as_ref().and_then(|v|v.as_bool())==Some(true);
     assert_eq!(std::env::var(envkey).ok().as_deref(),if expected{Some("1")}else{None},"{key} case{i}");
     assert!(std::env::var_os(otherenv).is_none(),"missing other flag clears stale state");
     assert!(crate::qos::ENABLED.load(std::sync::atomic::Ordering::Relaxed));
     for k in ["MUGUET_DEPTH_REDUCE_OPT","MUGUET_DEPTH_REDUCE_NO_ZERO"]{assert_eq!(std::env::var(k).as_deref(),Ok("1"));}
     assert_eq!(render_size(3000,2000),(1202,800));assert_eq!(fps_target(),60.0);
     assert!(args.iter().any(|a|a=="-frameLimit=1"));
    }
   }
   std::fs::write(&path,DEFAULT_CONFIG).unwrap();settings(&path);
   assert_eq!(render_size(3000,2000),(1202,800));assert_eq!(fps_target(),30.0);
   for k in ["MUGUET_SKIP_EMPTY_INIT_TRANSITION","MUGUET_READBACK_POOL","MUGUET_LUMINANCE_NO_ZERO"]{assert!(std::env::var_os(k).is_none());}
   std::fs::remove_dir_all(dir).unwrap();
  }
 }
}
'''


def main():
    HOST.mkdir(parents=True, exist_ok=True)
    OUT.mkdir(parents=True, exist_ok=True)
    sources = [MEMORY, HELPER, INIT, BUFFER, CONFIG]
    before = {str(p.relative_to(ROOT)): sha(p) for p in sources}
    memory = MEMORY.read_text()
    actual = block(memory, "pub(crate) fn initialize_buffer_memory(")
    skip = block(actual, "if empty_init_transition::observe(ranges.is_empty())")
    assert skip.count("buffer.try_raw(snatch_guard)?;") == 1
    assert "continue;" in skip and "set_single" not in skip
    assert actual.index(skip) < actual.index(".set_single(") < actual.index("let raw_buf = buffer.try_raw")
    assert actual.count(CFG) == 1
    reference = actual.replace("            " + CFG + "\n            " + skip + "\n\n", "")
    assert "empty_init_transition" not in reference, "Reference must remove only the experimental branch"
    reference = reference.replace("fn initialize_buffer_memory(", "fn initialize_buffer_memory_reference(", 1)
    init_text = INIT.read_text()
    init_text = init_text[init_text.index("#[derive(Debug, Clone, Copy)]"):init_text.index("#[cfg(test)]")]
    buffer_text = BUFFER.read_text()
    buffer_text = buffer_text[buffer_text.index("#[derive(Debug, Clone)]"):buffer_text.index("impl BufferInitTracker")]
    helper_path = json.dumps(str(HELPER), ensure_ascii=False)
    config_test = CONFIG_TEST.replace("CONFIG_PATH", json.dumps(str(CONFIG), ensure_ascii=False))
    rust = PRELUDE + init_text + buffer_text
    rust += f'\n#[path = {helper_path}]\nmod empty_init_transition;\n'
    rust += "\nimpl BakedCommands {\n" + actual.replace(CFG, "") + "\n" + reference + "\n}\n"
    rust += TESTS + config_test
    (HOST / "lib.rs").write_text(rust)
    (HOST / "Cargo.toml").write_text('''[package]
name = "actual-empty-init-transition-host"
version = "0.1.0"
edition = "2021"
[lib]
path = "lib.rs"
[dependencies]
smallvec = "=1.16.3"
log = "=0.4.34"
profiling = { version = "=1.0.18", default-features = false }
anyhow = "1"
regex = "1"
serde_json = "1"
[workspace]
''')
    env = dict(os.environ)
    env["CARGO_HOME"] = str(ROOT / "runtime-probe/.toolchain/cargo")
    env["RUSTUP_HOME"] = str(ROOT / "runtime-probe/.toolchain/rustup")
    env["PATH"] = env["CARGO_HOME"] + "/bin:" + env["PATH"]
    env["CARGO_TARGET_DIR"] = str(HOST / "target")
    cargo = [str(Path(env["CARGO_HOME"]) / "bin/cargo"), "test", "--offline", "--manifest-path", str(HOST / "Cargo.toml"), "--target", "aarch64-apple-darwin"]
    runs = []
    # Separate processes are intentional: production helper caches its env once.
    for index, value in enumerate([None, "0", "1", "true", "01", " 1", "1 ", ""]):
        run_env = dict(env)
        if value is None:
            run_env.pop("MUGUET_SKIP_EMPTY_INIT_TRANSITION", None)
        else:
            run_env["MUGUET_SKIP_EMPTY_INIT_TRANSITION"] = value
        result = subprocess.run(cargo + ["tests::actual_complete_buffer_init_function_matrix", "--", "--nocapture", "--test-threads=1"], cwd=ROOT, env=run_env, text=True, capture_output=True, timeout=120)
        log = HOST / f"buffer-matrix-{index}.log"
        log.write_text(result.stdout + result.stderr)
        assert result.returncode == 0, result.stdout + result.stderr
        assert result.stdout.count("PASS enabled=") == 13
        runs.append({"environment": value, "enabled": value == "1", "cases": 13, "log": str(log.relative_to(ROOT))})
    result = subprocess.run(cargo + ["actual_config::strict_flags", "--", "--nocapture", "--test-threads=1"], cwd=ROOT, env=env, text=True, capture_output=True, timeout=120)
    (HOST / "actual-config.log").write_text(result.stdout + result.stderr)
    assert result.returncode == 0, result.stdout + result.stderr
    assert before == {str(p.relative_to(ROOT)): sha(p) for p in sources}, "Production changed during host verification"
    report = {
        "status": "pass", "sourceSHA256": before, "sourceStableDuringRun": True,
        "scope": "Complete actual initialize_buffer_memory function, actual generic InitTracker and actual helper; host mocks only for HAL/device state/resource handle. Complete actual config.rs compiled with unrelated runtime stubs.",
        "transformationsForHost": ["Remove the single OHOS/std cfg attribute to exercise the OHOS branch on macOS.", "Reference function removes only the new branch and renames function; all old collection, range merge, validation, transition and clear code is unchanged."],
        "matrixRuns": runs, "bufferCaseExecutions": 104,
        "configChecks": {"newFlags": ["skip_empty_init_transition", "readback_pool", "luminance_no_zero"], "casesPerFlag": 11, "missingFalseInvalidClearStaleEnv": True, "onlyBooleanTrueEnables": True, "qualifiedThreeTrueDefaultsPreserved": True, "default1202x800At30AndExplicit60Preserved": True},
        "validationBoundary": "Host call-sequence and initialization-range equivalence; does not validate actual Vulkan synchronization or device performance. Real tracker source reviewed separately; no game/HAP build, HDC or device operations.",
    }
    (OUT / "actual-source-tests.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": "pass", "bufferCaseExecutions": 104, "configFlagCases": 33, "report": str(OUT / "actual-source-tests.json")}))


if __name__ == "__main__":
    main()
