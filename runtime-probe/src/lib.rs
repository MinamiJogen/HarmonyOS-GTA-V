use std::cell::RefCell;
use std::ffi::{CString, c_char};
#[cfg(feature = "wasmtime-runtime")]
use std::ffi::CStr;
thread_local! { static LAST_ERROR: RefCell<CString> = RefCell::new(CString::new("").unwrap()); }
fn fail(code: i32, message: impl ToString) -> i32 {
    let message = message.to_string().replace('\0', "?");
    LAST_ERROR.with(|v| *v.borrow_mut() = CString::new(message).unwrap());
    code
}
fn clear_error() { LAST_ERROR.with(|v| *v.borrow_mut() = CString::new("").unwrap()); }
#[no_mangle]
pub extern "C" fn rp_add(left: i32, right: i32) -> i32 { left.wrapping_add(right) }
#[no_mangle]
pub extern "C" fn rp_version() -> *const c_char { concat!("harmony-runtime-probe/0.1.0; target=", env!("RP_TARGET"), "; wasmtime=49.0.0\0").as_ptr().cast() }
#[no_mangle]
pub extern "C" fn rp_runtime_available() -> i32 { if cfg!(feature = "wasmtime-runtime") { 1 } else { 0 } }
#[no_mangle]
pub extern "C" fn rp_last_error() -> *const c_char { LAST_ERROR.with(|v| v.borrow().as_ptr()) }

#[cfg(feature = "wasmtime-runtime")]
struct PrepublishedCode;
#[cfg(feature = "wasmtime-runtime")]
impl wasmtime::CustomCodeMemory for PrepublishedCode {
    fn required_alignment(&self) -> usize { 1 }
    fn publish_executable(&self, _ptr: *const u8, _len: usize) -> wasmtime::Result<()> { Ok(()) }
    fn unpublish_executable(&self, _ptr: *const u8, _len: usize) -> wasmtime::Result<()> { Ok(()) }
}
#[cfg(feature = "wasmtime-runtime")]
fn engine(prepublished: bool) -> wasmtime::Result<wasmtime::Engine> {
    let mut config = wasmtime::Config::new();
    config.wasm_memory64(true).wasm_threads(true).shared_memory(true);
    if prepublished { config.with_custom_code_memory(Some(std::sync::Arc::new(PrepublishedCode))); }
    wasmtime::Engine::new(&config)
}
#[cfg(feature = "wasmtime-runtime")]
fn invoke(engine: &wasmtime::Engine, module: &wasmtime::Module, left: i32, right: i32) -> wasmtime::Result<i32> {
    let mut store = wasmtime::Store::new(engine, ());
    let instance = wasmtime::Instance::new(&mut store, module, &[])?;
    let add = instance.get_typed_func::<(i32, i32), i32>(&mut store, "add")?;
    add.call(&mut store, (left, right))
}

#[no_mangle]
pub unsafe extern "C" fn rp_run_aot_add(path: *const c_char, left: i32, right: i32, result: *mut i32) -> i32 {
    clear_error();
    if path.is_null() || result.is_null() { return fail(1, "null path/result"); }
    #[cfg(not(feature = "wasmtime-runtime"))]
    { let _ = (path, left, right, result); fail(2, "Wasmtime runtime not linked") }
    #[cfg(feature = "wasmtime-runtime")]
    {
        let path = match CStr::from_ptr(path).to_str() { Ok(p) => p, Err(e) => return fail(1, e) };
        let run = || -> wasmtime::Result<i32> {
            let engine = engine(false)?;
            let module = wasmtime::Module::deserialize_file(&engine, path)?;
            invoke(&engine, &module, left, right)
        };
        match run() { Ok(v) => { *result = v; 0 }, Err(e) => fail(3, format!("{e:#}")) }
    }
}
#[no_mangle]
pub unsafe extern "C" fn rp_run_raw_aot_add(bytes: *const u8, length: usize, left: i32, right: i32, result: *mut i32) -> i32 {
    clear_error();
    if bytes.is_null() || result.is_null() || length == 0 { return fail(1, "null/empty raw artifact/result"); }
    #[cfg(not(feature = "wasmtime-runtime"))]
    { let _ = (bytes, length, left, right, result); fail(2, "Wasmtime runtime not linked") }
    #[cfg(feature = "wasmtime-runtime")]
    {
        let run = || -> wasmtime::Result<i32> {
            let engine = engine(true)?;
            let raw = std::ptr::NonNull::slice_from_raw_parts(std::ptr::NonNull::new(bytes as *mut u8).unwrap(), length);
            let module = wasmtime::Module::deserialize_raw(&engine, raw)?;
            invoke(&engine, &module, left, right)
        };
        match run() { Ok(v) => { *result = v; 0 }, Err(e) => fail(3, format!("{e:#}")) }
    }
}
