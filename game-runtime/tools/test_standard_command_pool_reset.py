#!/usr/bin/env python3
"""Compile the real HAL reset/drop bodies against a recording Vulkan driver."""
from pathlib import Path
import hashlib
import json
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
command = (root / 'vendor/wgpu-hal/src/vulkan/command.rs').read_text()
module = (root / 'vendor/wgpu-hal/src/vulkan/mod.rs').read_text()


def body_after(source, token):
    start = source.index('{', source.index(token)) + 1
    depth = 1
    for end in range(start, len(source)):
        if source[end] == '{':
            depth += 1
        elif source[end] == '}':
            depth -= 1
            if depth == 0:
                return source[start:end]
    raise ValueError('unclosed source body')


reset = body_after(command, 'unsafe fn reset_all<I>')
drop = body_after(module, 'impl Drop for CommandEncoder')
checkpoint = body_after(command, 'pub(super) fn checkpoint_pool_trace')
source = r'''
#![allow(dead_code,unused_imports)]
use std::{sync::{Arc,Mutex,atomic::{AtomicU64,Ordering}},collections::HashMap};
mod ash { pub mod vk {
    #[derive(Copy,Clone,Debug,PartialEq,Eq,Hash)] pub struct Raw(pub u64);
    impl Raw { pub fn null()->Self { Self(0) } }
    pub trait Handle { fn as_raw(self)->u64; }
    impl Handle for Raw { fn as_raw(self)->u64 { self.0 } }
    pub type Device=Raw; pub type CommandBuffer=Raw; pub type CommandPool=Raw;
    #[derive(Copy,Clone,Default)] pub struct CommandPoolResetFlags(pub u32);
    impl CommandPoolResetFlags { pub fn empty()->Self { Self(0) } }
} }
use ash::vk;
#[derive(Copy,Clone,Debug,PartialEq)] enum Error { DeviceLost, DeviceOOM }
#[derive(Clone)] struct MockDevice { events:Arc<Mutex<Vec<String>>>,fail:Option<Error> }
impl MockDevice {
    fn handle(&self)->vk::Device { vk::Raw(77) }
    unsafe fn device_wait_idle(&self)->Result<(),Error> { panic!("unexpected device wait idle"); }
    unsafe fn reset_command_buffer(&self,_:vk::CommandBuffer,_:())->Result<(),Error> { panic!("unexpected individual command-buffer reset"); }
    unsafe fn reset_command_pool(&self,p:vk::CommandPool,f:vk::CommandPoolResetFlags)->Result<(),Error> {
        self.events.lock().unwrap().push(format!("reset-pool:{}:flags{}",p.0,f.0));
        match self.fail {Some(e)=>Err(e),None=>Ok(())}
    }
    unsafe fn destroy_command_pool(&self,p:vk::CommandPool,_:Option<()>){self.events.lock().unwrap().push(format!("pool-drop:{}",p.0));}
    unsafe fn destroy_framebuffer(&self,p:vk::Raw,_:Option<()>){self.events.lock().unwrap().push(format!("fb-drop:{}",p.0));}
    unsafe fn destroy_image_view(&self,p:vk::Raw,_:Option<()>){self.events.lock().unwrap().push(format!("view-drop:{}",p.0));}
}
struct Device {raw:MockDevice}
impl Device { fn capture_device_fault_if_lost(&self,e:Error,reason:&str){
    if e==Error::DeviceLost {self.raw.events.lock().unwrap().push(format!("capture:{reason}"));}
} }
struct Counter; impl Counter {fn sub(&self,_:u32){}}
struct Counters {command_encoders:Counter}
struct View {raw:vk::Raw}
struct CommandBuffer {raw:vk::CommandBuffer}
struct CommandEncoder {
    temp:Vec<u8>,free:Vec<vk::CommandBuffer>,discarded:Vec<vk::CommandBuffer>,active:vk::CommandBuffer,
    raw:vk::CommandPool,pool_generation:u64,device:Device,
    framebuffers:HashMap<u64,vk::Raw>,temp_texture_views:HashMap<u64,View>,counters:Counters,
}
static POOL_RETIREMENT_SEQUENCE:AtomicU64=AtomicU64::new(0);
fn checkpoint_pool_trace(path:Option<&std::ffi::OsStr>,phase:&str,record:&str) { CHECKPOINT_BODY }
mod command {
    pub(super) fn checkpoint_pool_trace(p:Option<&std::ffi::OsStr>,s:&str,r:&str){super::checkpoint_pool_trace(p,s,r)}
    // Pool event telemetry does not affect the reset/drop algorithm under test.
    pub(super) fn trace_pool_event(_: &str, _: super::vk::Device, _: super::vk::CommandPool, _: u64) {}
}
impl CommandEncoder {
    unsafe fn reset_all<I>(&mut self,cmd_bufs:I) where I:Iterator<Item=CommandBuffer> { RESET_BODY }
}
impl Drop for CommandEncoder { DROP_IMPL }
fn encoder(fail:Option<Error>)->(CommandEncoder,Arc<Mutex<Vec<String>>>) {
    std::env::remove_var("MUGUET_VK_POOL_TRACE");
    let events=Arc::new(Mutex::new(vec![]));
    (CommandEncoder{temp:vec![9],free:vec![vk::Raw(1),vk::Raw(2)],discarded:vec![vk::Raw(4)],
    active:vk::Raw(0),raw:vk::Raw(88),pool_generation:1,
    device:Device{raw:MockDevice{events:events.clone(),fail}},
    framebuffers:HashMap::from([(1,vk::Raw(91))]),temp_texture_views:HashMap::from([(1,View{raw:vk::Raw(92)})]),
    counters:Counters{command_encoders:Counter}},events)
}
#[test] fn one_pool_reset_retains_returned_and_discarded_handles_then_releases_framebuffer() {
    let(mut e,events)=encoder(None);
    unsafe{e.reset_all([CommandBuffer{raw:vk::Raw(3)}].into_iter())};
    assert_eq!(e.free,vec![vk::Raw(1),vk::Raw(2),vk::Raw(3),vk::Raw(4)]);
    assert!(e.discarded.is_empty());assert!(e.temp.is_empty());assert_eq!(e.raw,vk::Raw(88));
    assert!(e.framebuffers.is_empty());assert_eq!(e.temp_texture_views.len(),1);
    assert_eq!(*events.lock().unwrap(),vec!["reset-pool:88:flags0","fb-drop:91"]);
    drop(e);
    assert_eq!(&events.lock().unwrap()[2..], &["pool-drop:88","view-drop:92"]);
}
#[test] fn repeated_128_handle_retirements_make_one_reset_each_with_no_idle_or_per_cb_reset() {
    let(mut e,events)=encoder(None);
    e.free=(1..=128).map(vk::Raw).collect();e.discarded.clear();
    for _ in 0..100 {unsafe{e.reset_all(std::iter::empty())};assert_eq!(e.free.len(),128);assert_eq!(e.raw,vk::Raw(88));}
    let before=events.lock().unwrap().clone();
    assert_eq!(before.iter().filter(|v|v.starts_with("reset-pool:")).count(),100);
    assert!(!before.iter().any(|v|v.starts_with("pool-drop:")));
    drop(e);
    assert_eq!(events.lock().unwrap().iter().filter(|v|*v=="pool-drop:88").count(),1);
}
#[test] fn lost_is_captured_before_panic_and_preserves_handles_and_framebuffer_until_final_drop() {
    let(mut e,events)=encoder(Some(Error::DeviceLost));
    let r=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||unsafe{e.reset_all(std::iter::empty())}));
    assert!(r.is_err());assert_eq!(e.raw,vk::Raw(88));assert_eq!(e.free.len(),3);assert_eq!(e.framebuffers.len(),1);
    assert_eq!(*events.lock().unwrap(),vec!["reset-pool:88:flags0","capture:reset_all vkResetCommandPool"]);
    drop(e);
    assert_eq!(&events.lock().unwrap()[2..], &["pool-drop:88","fb-drop:91","view-drop:92"]);
}
#[test] fn oom_keeps_its_original_error_and_never_releases_framebuffer_in_reset() {
    let(mut e,events)=encoder(Some(Error::DeviceOOM));
    let panic=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||unsafe{e.reset_all(std::iter::empty())})).unwrap_err();
    let message=panic.downcast_ref::<String>().unwrap();
    assert!(message.contains("DeviceOOM"));assert_eq!(e.framebuffers.len(),1);
    assert_eq!(*events.lock().unwrap(),vec!["reset-pool:88:flags0"]);
}
#[test] fn active_recording_is_rejected_before_any_native_reset() {
    let(mut e,events)=encoder(None);e.active=vk::Raw(7);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(||unsafe{e.reset_all(std::iter::empty())})).is_err());
    assert!(events.lock().unwrap().is_empty());e.active=vk::Raw(0);
}
#[test] fn empty_live_pool_still_resets_once_and_is_destroyed_normally() {
    let(mut e,events)=encoder(None);e.free.clear();e.discarded.clear();e.framebuffers.clear();
    unsafe{e.reset_all(std::iter::empty())};
    assert_eq!(*events.lock().unwrap(),vec!["reset-pool:88:flags0"]);
    drop(e);assert_eq!(&events.lock().unwrap()[1..], &["pool-drop:88","view-drop:92"]);
}
#[test] fn diagnostics_off_has_no_file_io_or_trace_sequence_increment() {
    let(mut e,_)=encoder(None);let old=POOL_RETIREMENT_SEQUENCE.load(Ordering::Relaxed);
    let absent=std::env::temp_dir().join(format!("muguet-pool-off-{}",std::process::id()));
    let _=std::fs::remove_dir_all(&absent);
    unsafe{e.reset_all(std::iter::empty())};
    assert_eq!(POOL_RETIREMENT_SEQUENCE.load(Ordering::Relaxed),old);assert!(!absent.exists());
}
#[test] fn trace_reports_actual_pool_reset_result_without_a_fabricated_idle_result() {
    let(mut e,_)=encoder(None);
    let path=std::env::temp_dir().join(format!("muguet-pool-trace-{}.txt",std::process::id()));
    std::env::set_var("MUGUET_VK_POOL_TRACE",&path);
    unsafe{e.reset_all(std::iter::empty())};
    let text=std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("phase=after\n"));assert!(text.contains("action=reset-command-pool flags=EMPTY retainPool=true"));
    assert!(text.contains("resetResult=Ok(())"));assert!(!text.contains("waitIdle"));assert!(!text.contains("individual"));
    drop(e);
    assert!(std::fs::read_to_string(&path).unwrap().contains("phase=after-final-command-encoder-drop"));
    std::env::remove_var("MUGUET_VK_POOL_TRACE");std::fs::remove_file(path).unwrap();
}
'''.replace('RESET_BODY', reset).replace('DROP_IMPL', drop).replace('CHECKPOINT_BODY', checkpoint)
out = root / 'logs/standard-command-pool-reset-model'
out.mkdir(exist_ok=True)
rs = out / 'actual-reset-and-drop-tests.rs'
rs.write_text(source)
with tempfile.TemporaryDirectory(prefix='muguet-standard-command-pool-reset-') as temporary:
    binary = Path(temporary) / 'reset-tests'
    subprocess.run(['rustc', '--edition=2021', '--test', '-Aexplicit_builtin_cfgs_in_flags', '--cfg', 'target_env="ohos"', str(rs), '-o', str(binary)], check=True)
    result = subprocess.run([str(binary), '--test-threads=1'], text=True, capture_output=True)
(root / 'logs/standard-command-pool-reset-tests.log').write_text(result.stdout + result.stderr)
report = {
    'method': 'Actual reset_all, final Drop, and checkpoint bodies extracted verbatim; recording/failing Vulkan stub. Host control-flow test, not real Vulkan execution.',
    'sourceSHA256': {'command.rs': hashlib.sha256(command.encode()).hexdigest(), 'mod.rs': hashlib.sha256(module.encode()).hexdigest()},
    'resetBodySHA256': hashlib.sha256(reset.encode()).hexdigest(),
    'testExitCode': result.returncode,
    'testLog': 'game-runtime/logs/standard-command-pool-reset-tests.log',
}
(out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(result.stdout, result.stderr)
raise SystemExit(result.returncode)
