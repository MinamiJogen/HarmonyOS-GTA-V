// OHOS staticlib port of upstream Muguet 1a0300d. GPL-3.0-or-later.
mod audio;
mod config;
mod engine_args;
pub(crate) mod native_telemetry;
mod native_capabilities;
mod status_sampler;
mod cpu_profile;
mod compute_depth_probe;
mod compute_luminance_probe;
mod readback_pool_probe;
mod resource_view;
mod pipeline_cache_store;
mod pso_warmup;
mod spatial_upscale;
mod spatial_dimensions;
mod spatial_resize_policy;
mod fs;
mod gpu;
mod frame_capture;
mod frame_interpolation;
mod heapcheck;
mod host;
mod host_memory;
mod input;
mod io;
mod mp;
mod native_platform;
mod opus;
mod pad;
mod virtual_pad;
mod fivestar_state;
mod touch_state;
mod touch_controls;
mod proto;
mod qos;
mod server;
mod ffi;
mod wgpu_atlas_probe;
mod wgpu_spatial_probe;
use anyhow::{Context, Result};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use wasmtime::*;
pub struct ImportSpec {
    pub module: String,
    pub name: String,
    pub ty: FuncType,
}

pub struct Runtime {
    pub engine: Engine,
    pub module: Module,
    pub mem: SharedMemory,
    pub imports: Vec<ImportSpec>,
    pub counts: Vec<AtomicU64>,
    pub start: Instant,
}

pub struct HostState {
    pub pthread: u64,
}

pub static RT: OnceLock<Arc<Runtime>> = OnceLock::new();

pub fn prepare_process_limits() {
    unsafe {
        let mut limit:libc::rlimit=std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE,&mut limit)==0 {
            let before=limit.rlim_cur;let wanted=limit.rlim_max.min(10240);
            if wanted>before {limit.rlim_cur=wanted;let result=libc::setrlimit(libc::RLIMIT_NOFILE,&limit);eprintln!("[host] file descriptor limit {before} -> {wanted}, result {result}");}
            else {eprintln!("[host] file descriptor limit {before}, hard {}",limit.rlim_max);}
        }
    }
}
pub fn footprint_mb() -> u64 {
    sample_footprint_mb().unwrap_or(0)
}
pub(crate) fn sample_footprint_mb() -> std::io::Result<u64> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    status.lines().find(|line| line.starts_with("VmRSS:")).and_then(|line| line.split_whitespace().nth(1))
        .and_then(|number| number.parse::<u64>().ok()).map(|kb| kb >> 10)
        .ok_or_else(|| std::io::Error::other("VmRSS missing or invalid in /proc/self/status"))
}
pub fn heap_resident_mb() -> u64 {
    sample_heap_resident_mb().ok().flatten().unwrap_or(0)
}
pub(crate) fn sample_heap_resident_mb() -> std::io::Result<Option<u64>> {
    let Some(rt)=RT.get() else{return Ok(None)};let (base,len)=mem_base_len(&rt.mem);
    if len == 0 { return Ok(Some(0)); }
    let page=unsafe{libc::sysconf(libc::_SC_PAGESIZE)}.max(4096) as usize;
    let start=(base as usize)&!(page-1);let len=len+(base as usize-start);let mut pages=vec![0u8;len.div_ceil(page)];
    if unsafe{libc::mincore(start as *mut libc::c_void,len,pages.as_mut_ptr())}!=0{return Err(std::io::Error::last_os_error());}
    Ok(Some((pages.iter().filter(|p|**p&1!=0).count()*page>>20) as u64))
}
pub fn compressed_mb() -> u64 { 0 }
pub fn malloc_mb() -> u64 { 0 }
pub fn gpu_allocated_mb() -> u64 { 0 }
pub fn set_interactive_qos() { qos::interactive(); }
fn set_qos(class: Option<i32>) { qos::apply(class); }
fn engine_thread_qos(name: &str) -> Option<i32> { qos::engine_thread_level(name) }
pub fn make_runtime(startup: &config::Startup) -> Result<Arc<Runtime>> {
    let engine = Engine::new(&engine_config()).map_err(anyhow::Error::from).context("engine")?;
    let t0 = Instant::now();
    let module = load_module(&engine, &startup)?;
    eprintln!("[host] module ready in {:?}", t0.elapsed());

    let mut mem = None;
    let mut imports = Vec::new();
    for imp in module.imports() {
        match imp.ty() {
            ExternType::Memory(mt) => mem = Some(SharedMemory::new(&engine, heap_type(mt)?)?),
            ExternType::Func(ty) => imports.push(ImportSpec {
                module: imp.module().to_string(),
                name: imp.name().to_string(),
                ty,
            }),
            other => anyhow::bail!("unsupported import {}.{}: {other:?}", imp.module(), imp.name()),
        }
    }
    let counts = imports.iter().map(|_| AtomicU64::new(0)).collect();
    let rt = Arc::new(Runtime {
        engine,
        module,
        mem: mem.context("module must import env.memory")?,
        imports,
        counts,
        start: Instant::now(),
    });
    RT.set(rt.clone()).ok();
    Ok(rt)
}

pub fn run_engine(rt: &Arc<Runtime>, args: Vec<String>) -> Result<()> {
    let _cpu_thread = cpu_profile::register_thread(cpu_profile::Role::GuestMain);
    set_interactive_qos();
    ffi::set_stage("instantiating-guest");
    let (mut store, instance) = instantiate(rt, 0)?;
    ffi::set_stage("initializing-guest-tls");
    call_export(&mut store, &instance, "_emscripten_tls_init", &[])?;
    ffi::set_stage("initializing-guest-constructors");
    call_export(&mut store, &instance, "__wasm_call_ctors", &[])?;
    let argv = build_argv(&mut store, &instance, &rt.mem, &args)?;
    eprintln!("[host] main: {}", args.join(" "));
    let main = instance.get_func(&mut store, "__main_argc_argv").context("no main")?;
    let mut out = [Val::I32(0)];
    ffi::set_stage("running-game-main");
    match main.call(&mut store, &[Val::I32(args.len() as i32), Val::I64(argv as i64)], &mut out) {
        Ok(()) => eprintln!("[host] main returned {:?}", out[0]),
        Err(_) if host::take_unwind() => {
            eprintln!("[host] main unwound with a live runtime; waiting on engine threads");
            while !ffi::stop_requested() { std::thread::park_timeout(std::time::Duration::from_secs(1)); }
        }
        Err(e) => {
            eprintln!("[host] main trapped: {e:?}");
            ffi::set_error(format!("{e:?}"));
            stopped();
        }
    }
    Ok(())
}

extern "C" { static mg_game_aot_start: u8; static mg_game_aot_end: u8; }
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
pub fn instantiate(rt: &Arc<Runtime>, pthread: u64) -> Result<(Store<HostState>, Instance)> {
    let mut store = Store::new(&rt.engine, HostState { pthread });
    let mut linker: Linker<HostState> = Linker::new(&rt.engine);
    linker.define(&store, "env", "memory", rt.mem.clone())?;
    for (idx, imp) in rt.imports.iter().enumerate() {
        linker.func_new(&imp.module, &imp.name, imp.ty.clone(), move |caller, params, results| {
            host::call(caller, idx, params, results).map_err(wasmtime::Error::from_anyhow)
        })?;
    }
    let instance = linker.instantiate(&mut store, &rt.module)?;
    Ok((store, instance))
}

pub fn spawn_pthread(pthread_ptr: u64, start_routine: u64, arg: u64) -> i32 {
    let rt = RT.get().expect("runtime").clone();

    let name = if arg != 0 { read_cstr(&rt.mem, arg + 24) } else { String::new() };
    let name = if name.is_empty() || name.len() > 40 { format!("pthread-{pthread_ptr:x}") } else { name };
    let qos = engine_thread_qos(&name);
    let spawned = std::thread::Builder::new()
        .name(name)
        .stack_size(16 << 20)
        .spawn(move || {
            set_qos(qos);
            if let Err(e) = run_pthread(&rt, pthread_ptr, start_routine, arg) {
                eprintln!("[host] pthread 0x{pthread_ptr:x} failed: {e:?}");
            }
        });
    if spawned.is_ok() { 0 } else { 6 }
}

fn run_pthread(rt: &Arc<Runtime>, ptr: u64, start_routine: u64, arg: u64) -> Result<()> {
    let _cpu_thread = cpu_profile::register_thread(cpu_profile::Role::GuestWorker);
    let (mut store, inst) = instantiate(rt, ptr)?;

    let high = read_u64(&rt.mem, ptr + 80).unwrap_or(0);
    let size = read_u64(&rt.mem, ptr + 88).unwrap_or(0);
    call_export(&mut store, &inst, "emscripten_stack_set_limits", &[Val::I64(high as i64), Val::I64((high - size) as i64)])?;
    call_export(&mut store, &inst, "_emscripten_stack_restore", &[Val::I64(high as i64)])?;

    call_export(&mut store, &inst, "_emscripten_thread_init", &[Val::I64(ptr as i64), Val::I32(0), Val::I32(0), Val::I32(1), Val::I64(0), Val::I32(0)])?;
    call_export(&mut store, &inst, "_emscripten_tls_init", &[])?;
    host::mailbox_await(&rt.mem, ptr);

    let table = inst
        .get_table(&mut store, "__indirect_function_table")
        .context("no function table")?;
    let entry = table
        .get(&mut store, start_routine)
        .and_then(|r| r.as_func().flatten().cloned())
        .with_context(|| format!("null start routine {start_routine}"))?;
    let mut res = [Val::I64(0)];
    let result = match entry.call(&mut store, &[Val::I64(arg as i64)], &mut res) {
        Ok(()) => match res[0] { Val::I64(v) => v, Val::I32(v) => v as i64, _ => 0 },
        Err(_) if host::take_unwind() => return Ok(()),
        Err(e) => {
            let name = read_thread_name(&rt.mem, ptr);
            eprintln!("[host] thread {name} trapped: {e:?}");
            ffi::set_error(format!("{e:?}"));
            stopped();
            return Ok(());
        }
    };
    match call_export(&mut store, &inst, "_emscripten_thread_exit", &[Val::I64(result)]) {
        Err(e) if !host::take_unwind() => Err(e.into()),
        _ => Ok(()),
    }
}

fn stopped() {
    if !host::engine_stopped() { return; }
    if let Some(reason) = host::stop_reason() {
        alert("Muguet stopped", &format!("The game hit an error and can't continue:\n\n{reason}"));
    }
}

fn read_thread_name(_mem: &SharedMemory, ptr: u64) -> String {
    format!("0x{ptr:x}")
}

pub fn call_export(store: &mut Store<HostState>, inst: &Instance, name: &str, args: &[Val]) -> Result<Vec<Val>> {
    let f = inst.get_func(&mut *store, name).with_context(|| format!("no export {name}"))?;
    let ty = f.ty(&mut *store);
    let args = coerce_args(args, &ty);
    let mut results: Vec<Val> = ty.results().map(|t| default_val(&t)).collect();
    f.call(&mut *store, &args, &mut results)?;
    Ok(results)
}

pub fn coerce_args(args: &[Val], ty: &FuncType) -> Vec<Val> {
    ty.params()
        .zip(args)
        .map(|(pt, v)| {
            let i = match v { Val::I32(x) => *x as i64, Val::I64(x) => *x, other => return other.clone() };
            match pt { ValType::I32 => Val::I32(i as i32), ValType::I64 => Val::I64(i), _ => v.clone() }
        })
        .collect()
}

fn build_argv(store: &mut Store<HostState>, inst: &Instance, mem: &SharedMemory, args: &[String]) -> Result<u64> {
    let alloc = inst.get_typed_func::<i64, i64>(&mut *store, "_emscripten_stack_alloc")?;
    let mut ptrs = Vec::new();
    for a in args {
        let bytes = [a.as_bytes(), &[0]].concat();
        let p = alloc.call(&mut *store, bytes.len() as i64)? as u64;
        write_bytes(mem, p, &bytes);
        ptrs.push(p);
    }
    let argv = alloc.call(&mut *store, ((ptrs.len() + 1) * 8) as i64)? as u64;
    for (i, p) in ptrs.iter().chain(std::iter::once(&0)).enumerate() {
        write_u64(mem, argv + i as u64 * 8, *p);
    }
    Ok(argv)
}


pub fn default_val(t: &ValType) -> Val {
    match t {
        ValType::I32 => Val::I32(0),
        ValType::I64 => Val::I64(0),
        ValType::F32 => Val::F32(0),
        ValType::F64 => Val::F64(0),
        ValType::V128 => Val::V128(0u128.into()),
        ValType::Ref(_) => Val::null_any_ref(),
    }
}

pub fn mem_base_len(mem: &SharedMemory) -> (*mut u8, usize) {
    let d = mem.data();
    (d.as_ptr() as *mut u8, d.len())
}
pub fn read_u64(mem: &SharedMemory, addr: u64) -> Option<u64> {
    let (base, len) = mem_base_len(mem);
    let range = host_memory::span(len, addr, 8, 1, "read_u64")?;
    unsafe { Some(std::ptr::read_unaligned(base.add(range.start) as *const u64)) }
}
pub fn read_u32(mem: &SharedMemory, addr: u64) -> Option<u32> {
    let (base, len) = mem_base_len(mem);
    let range = host_memory::span(len, addr, 4, 1, "read_u32")?;
    unsafe { Some(std::ptr::read_unaligned(base.add(range.start) as *const u32)) }
}
pub fn write_u64(mem: &SharedMemory, addr: u64, v: u64) -> bool {
    write_bytes(mem, addr, &v.to_le_bytes())
}
pub fn write_u32(mem: &SharedMemory, addr: u64, v: u32) -> bool {
    write_bytes(mem, addr, &v.to_le_bytes())
}
pub fn read_bytes(mem: &SharedMemory, addr: u64, n: u64) -> Vec<u8> {
    let (base, len) = mem_base_len(mem);
    let Some(range) = host_memory::span(len, addr, n, 1, "read_bytes") else { return Vec::new(); };
    unsafe { std::slice::from_raw_parts(base.add(range.start), range.len()).to_vec() }
}
pub fn write_bytes(mem: &SharedMemory, addr: u64, data: &[u8]) -> bool {
    let (base, len) = mem_base_len(mem);
    let Some(range) = host_memory::span(len, addr, data.len() as u64, 1, "write_bytes") else { return false; };
    heapcheck::note(addr, data.len() as u64, "host write");
    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), base.add(range.start), range.len()); }
    true
}
pub fn read_cstr(mem: &SharedMemory, addr: u64) -> String {
    let (base, len) = mem_base_len(mem);
    let Some(range) = host_memory::span(len, addr, 0, 1, "read_cstr") else { return String::new(); };
    let mut a = range.start;
    let mut out = Vec::new();
    while a < len && out.len() < 65536 {
        let b = unsafe { *base.add(a) };
        if b == 0 { break; }
        out.push(b);
        a += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn alert(title: &str, msg: &str) { eprintln!("[alert] {title}: {msg}"); ffi::set_error(format!("{title}: {msg}")); }
