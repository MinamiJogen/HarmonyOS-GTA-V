from pathlib import Path
p=Path(__file__).resolve().parents[1]
s=(p/'src/main.rs').read_text()
head='''// OHOS staticlib port of upstream Muguet 1a0300d. GPL-3.0-or-later.
mod audio;
mod config;
mod fs;
mod gpu;
mod heapcheck;
mod host;
mod input;
mod io;
mod mp;
mod native_platform;
mod opus;
mod pad;
mod proto;
mod server;
mod ffi;
use anyhow::{Context, Result};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use wasmtime::*;
'''
structs=s[s.index('pub struct ImportSpec'):s.index('pub fn footprint_mb')]
platform='''pub fn footprint_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|n| n.parse::<u64>().ok())).unwrap_or(0) >> 10
}
pub fn heap_resident_mb() -> u64 { RT.get().map(|r| (r.mem.data_size() >> 20) as u64).unwrap_or(0) }
pub fn compressed_mb() -> u64 { 0 }
pub fn malloc_mb() -> u64 { 0 }
pub fn gpu_allocated_mb() -> u64 { 0 }
pub fn set_interactive_qos() {}
fn set_qos(_class: u32) {}
fn engine_thread_qos(_name: &str) -> u32 { 0 }
'''
rt=s[s.index('pub fn make_runtime'):s.index('#[cfg(all(not(feature = "jit"), not(target_os = "ios"), not(game_code)))]')]
rt=rt.replace('loop { std::thread::park(); }','while !ffi::stop_requested() { std::thread::park_timeout(std::time::Duration::from_secs(1)); }')
rt=rt.replace('stopped();','ffi::set_error(format!("{e:?}"));\n            stopped();')
loader='''extern "C" { static mg_game_aot_start: u8; static mg_game_aot_end: u8; }
fn load_module(engine: &Engine, _startup: &config::Startup) -> Result<Module> {
    let start = &raw const mg_game_aot_start;
    let end = &raw const mg_game_aot_end;
    let length = end as usize - start as usize;
    anyhow::ensure!(length > 0, "game AOT section is empty");
    let raw = std::ptr::NonNull::slice_from_raw_parts(std::ptr::NonNull::new(start as *mut u8).unwrap(), length);
    unsafe { Module::deserialize_raw(engine, raw) }.map_err(anyhow::Error::from).context("signed OHOS game AOT")
}
pub fn has_game_code() -> bool { true }
pub fn code_embedded() -> bool { true }
struct SignedCode;
impl CustomCodeMemory for SignedCode {
    fn required_alignment(&self) -> usize { 1 }
    fn publish_executable(&self, _ptr: *const u8, _len: usize) -> wasmtime::Result<()> { Ok(()) }
    fn unpublish_executable(&self, _ptr: *const u8, _len: usize) -> wasmtime::Result<()> { Ok(()) }
}
const IOS_HEAP_MAX: u64 = 5 << 30;
fn heap_type(mt: MemoryType) -> Result<MemoryType> {
    let cap = IOS_HEAP_MAX / (1u64 << mt.page_size_log2());
    let max = mt.maximum().map(|m| m.min(cap)).unwrap_or(cap).max(mt.minimum());
    MemoryTypeBuilder::new().min(mt.minimum()).max(Some(max)).memory64(mt.is_64()).shared(mt.is_shared()).page_size_log2(mt.page_size_log2()).build().map_err(anyhow::Error::from)
}
fn engine_config() -> Config {
    let mut config = Config::new();
    config.wasm_memory64(true).wasm_threads(true).shared_memory(true).wasm_bulk_memory(true).wasm_multi_memory(true);
    config.memory_reservation(IOS_HEAP_MAX).memory_guard_size(64 << 10).memory_may_move(false);
    config.with_custom_code_memory(Some(Arc::new(SignedCode)));
    config
}
'''
tail=s[s.index('pub fn instantiate'):s.index('pub fn alert')]
tail=tail.replace('if !host::engine_stopped() || cfg!(target_os = "ios") { return; }','if !host::engine_stopped() { return; }')
tail=tail.replace('stopped();','ffi::set_error(format!("{e:?}"));\n            stopped();')
(p/'src/lib.rs').write_text(head+structs+platform+rt+loader+tail+'''pub fn alert(title: &str, msg: &str) { eprintln!("[alert] {title}: {msg}"); ffi::set_error(format!("{title}: {msg}")); }
''')
pad=(p/'src/pad.rs').read_text()
pad=pad.replace('use objc2_game_controller::{GCController, GCControllerButtonInput, GCExtendedGamepad};','')
a=pad.index('fn gamepad()');b=pad.index('\nfn release(st: State)')
pad=pad[:a]+'''// OHOS input arrives through mg_game_key/mg_game_control. No Apple controller dependency.
pub fn poll() -> bool { ACTIVE.load(Ordering::Relaxed) }
'''+pad[b:]
(p/'src/pad.rs').write_text(pad)
cfg=(p/'src/config.rs').read_text()
a=cfg.index('    let home =',cfg.index('fn support_dir'));b=cfg.index('\n}',a)
cfg=cfg[:a]+'''    PathBuf::from(std::env::var("MUGUET_SUPPORT").unwrap_or_else(|_| "/data/storage/el2/base/haps/entry/files".into()))'''+cfg[b:]
cfg=cfg.replace('if cfg!(target_os = "ios") && !engine_args','if !engine_args')
(p/'src/config.rs').write_text(cfg)
h=(p/'src/host.rs').read_text().replace('std::process::exit(int(params, 0) as i32);','crate::ffi::set_error(format!("game exited with code {}", int(params, 0))); return Err(anyhow::anyhow!("game exit({})", int(params, 0)));').replace('cfg!(target_os = "ios") { 2560 << 20 }','true { 2560 << 20 }')
(p/'src/host.rs').write_text(h)
