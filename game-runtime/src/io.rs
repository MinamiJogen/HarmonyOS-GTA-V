// muguet - io.rs
// Constantin Clerc (c22dev) - 0.1

use crate::mem_base_len;
use std::collections::HashMap;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;
use wasmtime::SharedMemory;

static STARTED: AtomicBool = AtomicBool::new(false);
const MAX_OPEN: usize = 1024;

pub fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var("MUGUET_DATA").unwrap_or_default())
}

fn manifest_files() -> Vec<(String, u64)> {
    let text = std::fs::read_to_string(data_dir().join("manifest.json")).expect("manifest.json");
    let v: serde_json::Value = serde_json::from_str(&text).expect("manifest json");
    v["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|f| (f[0].as_str().unwrap_or("").to_string(), f[1].as_u64().unwrap_or(0)))
        .collect()
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
    let mut handles: HashMap<i32, File> = HashMap::new();
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

                        if !handles.contains_key(&id) {
                            if handles.len() >= MAX_OPEN { handles.clear(); }
                            match File::open(root.join(path)) {
                                Ok(f) => { handles.insert(id, f); }
                                Err(e) => eprintln!("[io] open {path}: {e}"),
                            }
                        }
                        let (base, mlen) = mem_base_len(&mem);
                        let Some(f) = handles.get(&id) else {
                            atomic_i32(&mem, w + 28).store(-1, Ordering::SeqCst);
                            atomic_i32(&mem, w).store(2, Ordering::SeqCst);
                            let _ = mem.atomic_notify(w, 1);
                            continue;
                        };
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
            }
            atomic_i32(&mem, w + 28).store(result as i32, Ordering::SeqCst);
            atomic_i32(&mem, w).store(2, Ordering::SeqCst);
            let _ = mem.atomic_notify(w, 1);
        }

        let _ = mem.atomic_wait32(table, seen as u32, Some(Duration::from_millis(20)));
    }
}
