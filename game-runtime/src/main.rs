// muguet - main.rs
// Constantin Clerc (c22dev) - 0.1

mod app;
mod audio;
mod config;
mod fs;
mod gpu;
mod heapcheck;
mod host;
mod input;
mod import;
mod io;
#[cfg(target_os = "ios")]
mod keyboard;
#[cfg(target_os = "ios")]
mod mouse;
mod mp;
mod opus;
mod pad;
#[cfg(feature = "jit")]
mod patch;
mod proto;
mod server;
#[cfg(target_os = "ios")]
mod touch;

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

pub fn footprint_mb() -> u64 {
    extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut u64) -> i32;
        fn getpid() -> i32;
    }

    let mut info = [0u64; 16];
    if unsafe { proc_pid_rusage(getpid(), 0, info.as_mut_ptr()) } != 0 {
        return 0;
    }
    info[2 + 7] >> 20
}

pub fn heap_resident_mb() -> u64 {
    let Some(rt) = RT.get() else { return 0 };
    let (base, len) = mem_base_len(&rt.mem);
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as usize;
    let start = (base as usize) & !(page - 1);
    let len = len + (base as usize - start);
    let mut vec = vec![0 as libc::c_char; len.div_ceil(page)];
    if unsafe { libc::mincore(start as *const libc::c_void, len, vec.as_mut_ptr()) } != 0 {
        return 0;
    }
    (vec.iter().filter(|&&v| v & 1 != 0).count() * page >> 20) as u64
}

pub fn compressed_mb() -> u64 {
    extern "C" {
        fn mach_task_self() -> u32;
        fn task_info(task: u32, flavor: i32, info: *mut u64, count: *mut u32) -> i32;
    }
    const TASK_VM_INFO: i32 = 22;
    let mut info = [0u64; 64];
    let mut count = 128u32;
    if unsafe { task_info(mach_task_self(), TASK_VM_INFO, info.as_mut_ptr(), &mut count) } != 0 {
        return 0;
    }
    info[15] >> 20
}

pub fn malloc_mb() -> u64 {
    #[repr(C)]
    struct Stats { blocks_in_use: u32, size_in_use: usize, max_size_in_use: usize, size_allocated: usize }
    extern "C" {
        fn malloc_zone_statistics(zone: *mut libc::c_void, stats: *mut Stats);
    }
    let mut s = Stats { blocks_in_use: 0, size_in_use: 0, max_size_in_use: 0, size_allocated: 0 };
    unsafe { malloc_zone_statistics(std::ptr::null_mut(), &mut s) };
    (s.size_in_use >> 20) as u64
}

pub fn gpu_allocated_mb() -> u64 {
    #[link(name = "Metal", kind = "framework")]
    extern "C" {
        fn MTLCreateSystemDefaultDevice() -> *mut objc2::runtime::AnyObject;
    }
    static DEVICE: OnceLock<usize> = OnceLock::new();
    let dev = *DEVICE.get_or_init(|| unsafe { MTLCreateSystemDefaultDevice() } as usize) as *mut objc2::runtime::AnyObject;
    if dev.is_null() {
        return 0;
    }
    let bytes: usize = unsafe { objc2::msg_send![&*dev, currentAllocatedSize] };
    (bytes >> 20) as u64
}

pub fn set_interactive_qos() {
    set_qos(QOS_CLASS_USER_INTERACTIVE);
}
const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
const QOS_CLASS_USER_INITIATED: u32 = 0x19;
fn set_qos(class: u32) {
    extern "C" {
        fn pthread_set_qos_class_self_np(qos: u32, relpri: i32) -> i32;
    }
    unsafe { pthread_set_qos_class_self_np(class, 0) };
}

fn engine_thread_qos(name: &str) -> u32 {
    if name.contains("RenderThread") || name.contains("mixer") || name.contains("Audio") && name.contains("Mix") {
        QOS_CLASS_USER_INTERACTIVE
    } else {
        QOS_CLASS_USER_INITIATED
    }
}

fn raise_open_file_limit() {
    #[repr(C)]
    struct Rlimit { cur: u64, max: u64 }
    extern "C" {
        fn getrlimit(resource: i32, rlp: *mut Rlimit) -> i32;
        fn setrlimit(resource: i32, rlp: *const Rlimit) -> i32;
    }
    const RLIMIT_NOFILE: i32 = 8;
    let mut r = Rlimit { cur: 0, max: 0 };
    unsafe {
        if getrlimit(RLIMIT_NOFILE, &mut r) == 0 {
            let want = r.max.min(10240);
            if want > r.cur {
                r.cur = want;
                setrlimit(RLIMIT_NOFILE, &r);
            }
        }
    }
}

fn main() -> Result<()> {
    raise_open_file_limit();
    heapcheck::start();
    let argv: Vec<String> = std::env::args().collect();

    if argv.get(1).map(String::as_str) == Some("--import") {
        let zip = argv.get(2).context("--import <game.zip>")?;
        let dest = config::game_dir();
        return import::import(std::path::Path::new(zip), &dest, !code_embedded(), |done, total| println!("progress {done} {total}"));
    }

    #[cfg(feature = "jit")]
    if argv.get(1).map(String::as_str) == Some("--precompile") {
        let (wasm, out) = (argv.get(2).context("--precompile <wasm> <out>")?, argv.get(3).context("--precompile <wasm> <out>")?);
        let engine = Engine::new(&engine_config_for(false)).map_err(anyhow::Error::from)?;
        let m = compile_patched(&engine, std::path::Path::new(wasm))?;
        std::fs::write(out, m.serialize()?)?;
        eprintln!("[host] precompiled {wasm} -> {out}");
        return Ok(());
    }
    #[cfg(feature = "jit")]
    if argv.get(1).map(String::as_str) == Some("--precompile-ios") {
        let (wasm, out) = (argv.get(2).context("--precompile-ios <wasm> <out>")?, argv.get(3).context("--precompile-ios <wasm> <out>")?);
        let mut config = engine_config_for(true);
        config.target("aarch64-apple-ios").map_err(anyhow::Error::from)?;
        unsafe { config.cranelift_flag_enable("has_lse") };
        let engine = Engine::new(&config).map_err(anyhow::Error::from)?;
        let wasm_bytes = std::fs::read(wasm)?;
        let patched = if patch::is_patched(&wasm_bytes) { wasm_bytes } else { patch::patch(&wasm_bytes, mp::COMMANDS)? };
        std::fs::write(out, engine.precompile_module(&patched).map_err(anyhow::Error::from)?)?;
        eprintln!("[host] precompiled {wasm} -> {out}");
        return Ok(());
    }
    #[cfg(feature = "jit")]
    if argv.get(1).map(String::as_str) == Some("--patch") {
        let (wasm, out) = (argv.get(2).context("--patch <wasm> <out>")?, argv.get(3).context("--patch <wasm> <out>")?);
        std::fs::write(out, patch::patch(&std::fs::read(wasm)?, mp::COMMANDS)?)?;
        return Ok(());
    }
    #[cfg(feature = "jit")]
    let launcher_out = {
        use std::os::fd::AsFd;
        std::io::stdout().as_fd().try_clone_to_owned().map(std::fs::File::from)
    };
    let startup = match config::setup() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[config] {e:#}");
            alert("Muguet", &format!("{e:#}"));
            std::process::exit(1);
        }
    };
    #[cfg(feature = "jit")]
    if argv.get(1).map(String::as_str) == Some("--compile") {
        if !code_cache(&startup)?.exists() {
            use std::io::Write;
            writeln!(launcher_out?, "compiling")?;
            load_module(&Engine::new(&engine_config()).map_err(anyhow::Error::from)?, &startup)?;
        }
        return Ok(());
    }
    #[cfg(not(target_os = "ios"))]
    let rt = Some(make_runtime(&startup)?);
    #[cfg(target_os = "ios")]
    let rt = None;

    let mut args: Vec<String> = [
        "./this.program", "-rootdir=/game/", "-nodisplaycalibration", "-noSocialClub",
        "-pc:nosocialclub", "-nonetwork", "-output",
        "-forceResolution", "-Script_all=warning", "-replay_all=warning",
        "-fastscriptcore",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(startup.engine_args.clone());

    args.extend(std::env::args().skip(1).filter(|a| !a.starts_with("-psn_")));

    app::run(rt, startup, args)
}

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
    set_interactive_qos();
    let (mut store, instance) = instantiate(rt, 0)?;
    call_export(&mut store, &instance, "_emscripten_tls_init", &[])?;
    call_export(&mut store, &instance, "__wasm_call_ctors", &[])?;
    let argv = build_argv(&mut store, &instance, &rt.mem, &args)?;
    eprintln!("[host] main: {}", args.join(" "));
    let main = instance.get_func(&mut store, "__main_argc_argv").context("no main")?;
    let mut out = [Val::I32(0)];
    match main.call(&mut store, &[Val::I32(args.len() as i32), Val::I64(argv as i64)], &mut out) {
        Ok(()) => eprintln!("[host] main returned {:?}", out[0]),
        Err(_) if host::take_unwind() => {
            eprintln!("[host] main unwound with a live runtime; waiting on engine threads");
            loop { std::thread::park(); }
        }
        Err(e) => {
            eprintln!("[host] main trapped: {e:?}");
            stopped();
        }
    }
    Ok(())
}

#[cfg(all(not(feature = "jit"), not(target_os = "ios"), not(game_code)))]
compile_error!("the macOS build needs the default `jit` feature, or MUGUET_GAME_CODE with the precompiled game");

#[cfg(all(not(feature = "jit"), not(target_os = "ios"), game_code))]
fn load_module(engine: &Engine, _startup: &config::Startup) -> Result<Module> {
    static GAME_CODE: &[u8] = include_bytes!(env!("MUGUET_GAME_CODE"));
    unsafe { Module::deserialize(engine, GAME_CODE) }.map_err(anyhow::Error::from).context("embedded game code")
}

#[cfg(all(not(feature = "jit"), target_os = "ios", game_code))]
std::arch::global_asm!(
    ".section __GAME,__code,regular,pure_instructions",
    ".p2align 14",
    "_muguet_game_code:",
    concat!(".incbin \"", env!("MUGUET_GAME_CODE"), "\""),
    "_muguet_game_code_end:",
    ".text",
);

#[cfg(all(not(feature = "jit"), target_os = "ios", game_code))]
fn load_module(engine: &Engine, _startup: &config::Startup) -> Result<Module> {
    const _: &[u8] = include_bytes!(env!("MUGUET_GAME_CODE"));
    extern "C" {
        #[link_name = "\x01_muguet_game_code"]
        static START: u8;
        #[link_name = "\x01_muguet_game_code_end"]
        static END: u8;
    }
    let (start, end) = (&raw const START, &raw const END);
    let code = std::ptr::slice_from_raw_parts(start, end as usize - start as usize);
    unsafe { Module::deserialize_raw(engine, std::ptr::NonNull::new(code as *mut [u8]).unwrap()) }
        .map_err(anyhow::Error::from)
        .context("embedded game code")
}

#[cfg(all(not(feature = "jit"), target_os = "ios", not(game_code)))]
fn load_module(_engine: &Engine, _startup: &config::Startup) -> Result<Module> {
    anyhow::bail!("this build has no game code: build it with ./make-ios.sh <game.zip>")
}

pub fn has_game_code() -> bool {
    cfg!(any(feature = "jit", game_code))
}

/// The game's code is part of this executable (no game.wasm needed).
pub fn code_embedded() -> bool {
    cfg!(all(not(feature = "jit"), game_code))
}

#[cfg(all(not(feature = "jit"), target_os = "ios", game_code))]
struct SignedCode;
#[cfg(all(not(feature = "jit"), target_os = "ios", game_code))]
impl CustomCodeMemory for SignedCode {
    fn required_alignment(&self) -> usize { 1 }
    fn publish_executable(&self, _ptr: *const u8, _len: usize) -> wasmtime::Result<()> { Ok(()) }
    fn unpublish_executable(&self, _ptr: *const u8, _len: usize) -> wasmtime::Result<()> { Ok(()) }
}

#[cfg(feature = "jit")]
fn code_cache(startup: &config::Startup) -> Result<std::path::PathBuf> {
    let wasm_path = &startup.wasm_path;
    let meta = std::fs::metadata(wasm_path).with_context(|| format!("{}", wasm_path.display()))?;
    let stamp = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);

    let mp_key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (patch::VERSION, mp::COMMANDS, host::MISSING_TEXT).hash(&mut h);
        h.finish() as u32
    };
    Ok(startup.cache_dir.join(format!("game-{}-{stamp}-{mp_key:08x}.cwasm", meta.len())))
}

#[cfg(feature = "jit")]
fn load_module(engine: &Engine, startup: &config::Startup) -> Result<Module> {
    let wasm_path = startup.wasm_path.clone();
    let cache = code_cache(startup)?;

    if let Some(m) = cache.exists().then(|| unsafe { Module::deserialize_file(engine, &cache) }.ok()).flatten() {
        return Ok(m);
    }
    eprintln!("[host] compiling {} to native code (one-time)…", wasm_path.display());
    let m = compile_patched(engine, &wasm_path)?;
    for e in std::fs::read_dir(&startup.cache_dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("game-") && name.ends_with(".cwasm") { let _ = std::fs::remove_file(e.path()); }
    }
    std::fs::write(&cache, m.serialize()?)?;
    Ok(m)
}

#[cfg(feature = "jit")]
fn compile_patched(engine: &Engine, wasm_path: &std::path::Path) -> Result<Module> {
    let wasm = std::fs::read(wasm_path).with_context(|| format!("{}", wasm_path.display()))?;
    let t = Instant::now();
    let patched = patch::patch(&wasm, mp::COMMANDS)?;
    eprintln!("[host] multiplayer hooks added in {:?}", t.elapsed());
    Module::new(engine, &patched).map_err(anyhow::Error::from).context("compile module")
}

const IOS_HEAP_MAX: u64 = ios_heap_gb() << 30;
const fn ios_heap_gb() -> u64 {
    match option_env!("MUGUET_IOS_HEAP_GB") {
        Some(v) => {
            let b = v.as_bytes();
            let mut n = 0u64;
            let mut k = 0;
            while k < b.len() {
                n = n * 10 + (b[k] - b'0') as u64;
                k += 1;
            }
            n
        }
        None => 5,
    }
}

fn heap_type(mt: MemoryType) -> Result<MemoryType> {
    if !ios_memory() {
        return Ok(mt);
    }
    let page = 1u64 << mt.page_size_log2();
    let cap = IOS_HEAP_MAX / page;
    let max = mt.maximum().map(|m| m.min(cap)).unwrap_or(cap).max(mt.minimum());
    MemoryTypeBuilder::new()
        .min(mt.minimum())
        .max(Some(max))
        .memory64(mt.is_64())
        .shared(mt.is_shared())
        .page_size_log2(mt.page_size_log2())
        .build()
        .map_err(anyhow::Error::from)
}

fn engine_config() -> Config {
    engine_config_for(ios_memory())
}

fn ios_memory() -> bool {
    cfg!(target_os = "ios")
}

fn engine_config_for(ios: bool) -> Config {
    let mut config = Config::new();
    config.wasm_memory64(true);
    config.wasm_threads(true);
    config.shared_memory(true);
    config.wasm_bulk_memory(true);
    config.wasm_multi_memory(true);
    #[cfg(feature = "jit")]
    config.cranelift_opt_level(OptLevel::Speed);

    if ios {
        config.memory_reservation(IOS_HEAP_MAX);
        config.memory_guard_size(64 << 10);
    } else {
        config.memory_reservation(17 << 30);
        config.memory_guard_size(2 << 30);
    }
    config.memory_may_move(false);
    #[cfg(all(not(feature = "jit"), target_os = "ios", game_code))]
    config.with_custom_code_memory(Some(Arc::new(SignedCode)));

    #[cfg(feature = "jit")]
    unsafe {
        config.cranelift_flag_set("enable_heap_access_spectre_mitigation", "false");
        config.cranelift_flag_set("enable_table_access_spectre_mitigation", "false");
    }
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
    if !host::engine_stopped() || cfg!(target_os = "ios") { return; }
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
    let a = addr as usize;
    if a + 8 > len { return None; }
    unsafe { Some(std::ptr::read_unaligned(base.add(a) as *const u64)) }
}
pub fn read_u32(mem: &SharedMemory, addr: u64) -> Option<u32> {
    let (base, len) = mem_base_len(mem);
    let a = addr as usize;
    if a + 4 > len { return None; }
    unsafe { Some(std::ptr::read_unaligned(base.add(a) as *const u32)) }
}
pub fn write_u64(mem: &SharedMemory, addr: u64, v: u64) -> bool {
    write_bytes(mem, addr, &v.to_le_bytes())
}
pub fn write_u32(mem: &SharedMemory, addr: u64, v: u32) -> bool {
    write_bytes(mem, addr, &v.to_le_bytes())
}
pub fn read_bytes(mem: &SharedMemory, addr: u64, n: u64) -> Vec<u8> {
    let (base, len) = mem_base_len(mem);
    let (a, n) = (addr as usize, n as usize);
    if a + n > len { return Vec::new(); }
    unsafe { std::slice::from_raw_parts(base.add(a), n).to_vec() }
}
pub fn write_bytes(mem: &SharedMemory, addr: u64, data: &[u8]) -> bool {
    let (base, len) = mem_base_len(mem);
    let a = addr as usize;
    if a + data.len() > len { return false; }
    heapcheck::note(addr, data.len() as u64, "host write");
    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), base.add(a), data.len()); }
    true
}
pub fn read_cstr(mem: &SharedMemory, addr: u64) -> String {
    let (base, len) = mem_base_len(mem);
    let mut a = addr as usize;
    let mut out = Vec::new();
    while a < len && out.len() < 65536 {
        let b = unsafe { *base.add(a) };
        if b == 0 { break; }
        out.push(b);
        a += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn alert(title: &str, msg: &str) {
    let script = format!("display alert \"{}\" message \"{}\" as critical",
        title.replace('"', "'"), msg.replace('"', "'"));
    let _ = std::process::Command::new("osascript").args(["-e", &script]).status();
}
