use harmony_runtime_probe::{rp_add, rp_last_error, rp_run_aot_add, rp_run_raw_aot_add};
use std::ffi::{CStr, CString, c_void};
use std::os::fd::AsRawFd;
unsafe extern "C" {
    fn mmap(address: *mut c_void, length: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut c_void;
    fn mprotect(address: *mut c_void, length: usize, prot: i32) -> i32;
    fn munmap(address: *mut c_void, length: usize) -> i32;
}
fn main() {
    assert_eq!(rp_add(19, 23), 42);
    for name in ["add", "memory64", "shared", "shared-memory64"] {
        let path = CString::new(format!("artifacts/host/{name}.cwasm")).unwrap();
        let mut value = 0;
        let status = unsafe { rp_run_aot_add(path.as_ptr(), 19, 23, &mut value) };
        let error = unsafe { CStr::from_ptr(rp_last_error()) }.to_string_lossy();
        println!("{name}: status={status}, value={value}, error={error}");
        assert_eq!((status, value), (0, 42));
        let file = std::fs::File::open(format!("artifacts/host/{name}.cwasm")).unwrap();
        let length = file.metadata().unwrap().len() as usize;
        // POSIX host control: file-backed RX mapping; raw API never changes permissions.
        let mut memory = unsafe { mmap(std::ptr::null_mut(), length, 1 | 4, 2, file.as_raw_fd(), 0) };
        if memory as usize == usize::MAX {
            println!("{name} direct file RX map unavailable: {}", std::io::Error::last_os_error());
            // Host-only control: publish a copy before calling the raw loader.
            // This is dynamic executable memory, never signed-HAP evidence.
            let map_anon = if cfg!(target_os = "macos") { 0x1000 } else { 0x20 };
            memory = unsafe { mmap(std::ptr::null_mut(), length, 1 | 2, 2 | map_anon, -1, 0) };
            assert_ne!(memory as usize, usize::MAX);
            let bytes = std::fs::read(format!("artifacts/host/{name}.cwasm")).unwrap();
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), memory.cast::<u8>(), length); }
            let published = unsafe { mprotect(memory, length, 1 | 4) };
            if published != 0 {
                println!("{name} raw host control blocked at mprotect: {}", std::io::Error::last_os_error());
                assert_eq!(unsafe { munmap(memory, length) }, 0);
                continue;
            }
        }
        let raw_status = unsafe { rp_run_raw_aot_add(memory.cast(), length, 19, 23, &mut value) };
        let raw_error = unsafe { CStr::from_ptr(rp_last_error()) }.to_string_lossy();
        println!("{name} raw RX: status={raw_status}, value={value}, error={raw_error}");
        assert_eq!((raw_status, value), (0, 42));
        assert_eq!(unsafe { munmap(memory, length) }, 0);
    }
}
