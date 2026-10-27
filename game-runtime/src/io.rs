// muguet - io.rs
// Constantin Clerc (c22dev) - 0.1

use crate::mem_base_len;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;
use wasmtime::SharedMemory;

#[path = "io_handle_cache.rs"]
mod handle_cache;
use handle_cache::{EvictionPolicy, HandleCache};

static STARTED: AtomicBool = AtomicBool::new(false);
const MAX_OPEN: usize = 1024;

pub fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var("MUGUET_DATA").unwrap_or_default())
}

fn manifest_files() -> Vec<(String, u64)> {
    crate::resource_view::files().to_vec()
}

fn atomic_i32(mem: &SharedMemory, addr: u64) -> &AtomicI32 {
    let (base, len) = mem_base_len(mem);
    let range = crate::host_memory::span(len, addr, 4, 4, "io.atomic_i32")
        .expect("invalid native I/O atomic address");
    unsafe { &*(base.add(range.start) as *const AtomicI32) }
}

pub fn start(mem: &SharedMemory, table: u64) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let mem = mem.clone();
    std::thread::Builder::new()
        .name("io".into())
        .spawn(move || serve_loop(mem, table))
        .expect("spawn io thread");
    eprintln!("[io] native I/O thread started (table=0x{table:x})");
}

fn serve_loop(mem: SharedMemory, table: u64) {
    let files = manifest_files();
    let root = data_dir();
    let policy = if std::env::var_os("MUGUET_IO_HANDLE_LRU").is_some_and(|value| value == "1") {
        EvictionPolicy::Lru
    } else {
        EvictionPolicy::ClearAll
    };
    let mut handles = HandleCache::new(MAX_OPEN, policy);
    eprintln!("[io] handle-cache policy={policy:?} capacity={MAX_OPEN}");
    let mut served: u64 = 0;
    let mut bytes: u64 = 0;
    loop {
        let (_, memory_len) = mem_base_len(&mem);
        crate::host_memory::table_span(memory_len, table, 0).expect("invalid native I/O table header");
        let seen = atomic_i32(&mem, table).load(Ordering::SeqCst);
        let n = atomic_i32(&mem, table + 4).load(Ordering::SeqCst).max(0) as u64;
        let (_, memory_len) = mem_base_len(&mem);
        // Validate the entire table before calculating any per-slot addresses.
        // A successful span proves table + 64 + 64 * slot fits for every slot.
        crate::host_memory::table_span(memory_len, table, n).expect("invalid native I/O table slots");
        for slot in 0..n {
            let w = table + 64 + 64 * slot;
            if atomic_i32(&mem, w).compare_exchange(1, 3, Ordering::SeqCst, Ordering::SeqCst).is_err() {
                continue;
            }
            let word = |k: u64| atomic_i32(&mem, w + 4 * k).load(Ordering::SeqCst) as u32 as u64;
            let id = word(1) as i32;
            let off = word(2) | (word(3) << 32);
            let dst = word(4) | (word(5) << 32);
            let mut len = word(6);
            let result: i64 = match files.get(id as usize) {
                None => -1,
                Some((path, size)) => {
                    if off >= *size {
                        0
                    } else {
                        len = len.min(size - off);

                        // The manifest and derived frontend come from the same
                        // immutable view, including for arbitrary partial reads.
                        if let Some(bytes) = crate::resource_view::overridden(path) {
                            let (base, mlen) = mem_base_len(&mem);
                            let result = crate::host_memory::span(mlen, dst, len, 1, "io.frontend_destination")
                                .and_then(|destination| usize::try_from(off).ok().and_then(|start| {
                                    start.checked_add(destination.len()).and_then(|end| bytes.get(start..end))
                                        .map(|source| {
                                            crate::heapcheck::note(dst, len, "derived frontend read");
                                            unsafe { std::ptr::copy_nonoverlapping(source.as_ptr(), base.add(destination.start), source.len()); }
                                            source.len() as i32
                                        })
                                })).unwrap_or(-1);
                            atomic_i32(&mem, w + 28).store(result, Ordering::SeqCst);
                            atomic_i32(&mem, w).store(2, Ordering::SeqCst);
                            let _ = mem.atomic_notify(w, 1);
                            continue;
                        }

                        let f = match handles.get_or_try_insert_with_resource_retry(id, || File::open(root.join(path)), |error| {
                            let pressure = matches!(error.raw_os_error(), Some(libc::EMFILE) | Some(libc::ENFILE));
                            if pressure { eprintln!("[io] open {path}: descriptor pressure; evict one LRU handle and retry once"); }
                            pressure
                        }) {
                            Ok(file) => file,
                            Err(error) => {
                                eprintln!("[io] open {path}: {error}");
                                atomic_i32(&mem, w + 28).store(-1, Ordering::SeqCst);
                                atomic_i32(&mem, w).store(2, Ordering::SeqCst);
                                let _ = mem.atomic_notify(w, 1);
                                continue;
                            }
                        };
                        let (base, mlen) = mem_base_len(&mem);
                        if let Some(range) = crate::host_memory::span(mlen, dst, len, 1, "io.file_destination") {
                            crate::heapcheck::note(dst, len, "file read");
                            let buf = unsafe { std::slice::from_raw_parts_mut(base.add(range.start), range.len()) };
                            match f.read_exact_at(buf, off) {
                                Ok(()) => len as i64,
                                Err(e) => { eprintln!("[io] read {path}@{off}+{len}: {e}"); -1 }
                            }
                        } else {
                            -1
                        }
                    }
                }
            };
            served += 1;
            if result > 0 { bytes += result as u64; }
            if served % 2000 == 0 {
                eprintln!("[io] {served} reads, {} MB", bytes >> 20);
                eprintln!("[io] handle-cache policy={policy:?} capacity={MAX_OPEN} stats={:?}", handles.stats());
            }
            atomic_i32(&mem, w + 28).store(result as i32, Ordering::SeqCst);
            atomic_i32(&mem, w).store(2, Ordering::SeqCst);
            let _ = mem.atomic_notify(w, 1);
        }

        let _ = mem.atomic_wait32(table, seen as u32, Some(Duration::from_millis(20)));
    }
}
