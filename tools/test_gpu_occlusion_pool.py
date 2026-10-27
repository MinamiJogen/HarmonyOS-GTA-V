#!/usr/bin/env python3
"""Host tests for the production paired cache and resolve-recording function.

The encoder harness checks allocation/overwrite/order, not Vulkan execution.
The production completion path is checked for lifecycle/notification ordering.
"""
from pathlib import Path
import json
import shutil
import subprocess
import tempfile


def function(source: str, name: str) -> str:
    begin = source.index(f"    fn {name}(")
    opening = source.index("{", begin)
    level, cursor = 1, opening + 1
    while level:
        level += (source[cursor] == "{") - (source[cursor] == "}")
        cursor += 1
    return source[begin:cursor]


HARNESS = r'''
use std::{cell::{Cell, RefCell}, collections::HashMap, ops::{BitOr, Range}, rc::Rc,
    sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}}};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BufferUsages(u32);
impl BufferUsages {
    const QUERY_RESOLVE: Self = Self(1); const COPY_SRC: Self = Self(2);
    const COPY_DST: Self = Self(4); const MAP_READ: Self = Self(8);
    fn empty() -> Self { Self(0) }
}
impl BitOr for BufferUsages { type Output = Self; fn bitor(self, rhs: Self) -> Self { Self(self.0 | rhs.0) } }
struct BufferDescriptor<'a> { label: Option<&'a str>, size: u64, usage: BufferUsages, mapped_at_creation: bool }
#[derive(Clone)]
struct Buffer { id: u64, size: u64, usage: BufferUsages, bytes: Rc<RefCell<Vec<u8>>> }
struct Device { created: Cell<u64> }
impl Device {
    fn create_buffer(&self, desc: &BufferDescriptor<'_>) -> Buffer {
        assert!(desc.label.is_some()); assert!(!desc.mapped_at_creation);
        self.created.set(self.created.get() + 1);
        Buffer { id: self.created.get(), size: desc.size, usage: desc.usage,
            bytes: Rc::new(RefCell::new(vec![0; desc.size as usize])) }
    }
}
#[derive(Clone)] struct QuerySet;
#[derive(Default)] struct Encoder { events: Vec<&'static str> }
impl Encoder {
    fn clear_buffer(&mut self, buffer: &Buffer, offset: u64, size: Option<u64>) {
        assert_eq!(buffer.usage.0 & BufferUsages::COPY_DST.0, BufferUsages::COPY_DST.0);
        self.events.push("clear"); let end = offset + size.unwrap_or(buffer.size - offset);
        buffer.bytes.borrow_mut()[offset as usize..end as usize].fill(0);
    }
    fn resolve_query_set(&mut self, _: &QuerySet, range: Range<u32>, buffer: &Buffer, offset: u64) {
        self.events.push("resolve"); assert_eq!(offset % 256, 0);
        for (i, slot) in range.enumerate() {
            let from = offset as usize + i * 8;
            buffer.bytes.borrow_mut()[from..from + 8].copy_from_slice(&(slot as u64 + 1).to_le_bytes());
        }
    }
    fn copy_buffer_to_buffer(&mut self, source: &Buffer, src: u64, target: &Buffer, dst: u64, bytes: u64) {
        self.events.push("copy");
        target.bytes.borrow_mut()[dst as usize..(dst + bytes) as usize]
            .copy_from_slice(&source.bytes.borrow()[src as usize..(src + bytes) as usize]);
    }
}
struct QueryObj { addr: u64 }
enum Obj { Query(QueryObj) }
struct PendingMap { buf: Buffer, done: Arc<AtomicBool>, failed: Arc<AtomicBool>, kind: MapKind }
enum MapKind { Occlusion { list: Vec<(u64, u32, u64, bool, usize)>, resolve: Buffer, recycle: bool, error_generation: u64 } }
const OCCLUSION_CAPACITY: u32 = 2048;
struct Gpu {
    pending_resolve: Vec<(u32, u32, Vec<u32>)>, objects: HashMap<u32, Obj>,
    occlusion_next: u32, occlusion_active: Option<u32>, occlusion_set: Option<QuerySet>,
    occlusion_pool: Option<occlusion_pool::Pool<Buffer>>, errors: AtomicU64,
    device: Device, enc: Encoder, stored: Vec<(u64, u64, u32)>,
}
impl Gpu {
    fn new(enabled: bool) -> Self {
        Self { pending_resolve: vec![], objects: [(1, Obj::Query(QueryObj { addr: 100 }))].into(),
            occlusion_next: 0, occlusion_active: None, occlusion_set: Some(QuerySet),
            occlusion_pool: enabled.then(occlusion_pool::Pool::new), errors: AtomicU64::new(0),
            device: Device { created: Cell::new(0) }, enc: Encoder::default(), stored: vec![] }
    }
    fn obj(&self, id: u32) -> Option<&Obj> { self.objects.get(&id) }
    fn query_store(&mut self, address: u64, result: u64, sequence: u32) { self.stored.push((address, result, sequence)); }
    fn encoder(&mut self) -> &mut Encoder { &mut self.enc }
RECORD_RESOLVES
}
fn read(buffer: &Buffer, at: usize) -> u64 {
    u64::from_le_bytes(buffer.bytes.borrow()[at..at + 8].try_into().unwrap())
}
#[test]
fn recorded_pooled_leases_overwrite_old_values_and_all_padding() {
    let mut gpu = Gpu::new(true);
    gpu.pending_resolve.push((1, 10, vec![1, 2, 3]));
    let pending = gpu.record_resolves().unwrap();
    assert!(!pending.done.load(Ordering::SeqCst)); assert!(!pending.failed.load(Ordering::SeqCst));
    assert_eq!(pending.buf.size, 256); assert_eq!(read(&pending.buf, 0), 2);
    assert_eq!(read(&pending.buf, 8), 3); assert_eq!(read(&pending.buf, 16), 4);
    let map_id = pending.buf.id;
    let MapKind::Occlusion { resolve, list, recycle, error_generation } = pending.kind;
    assert_eq!(list, vec![(0, 3, 100, true, 10)]); assert!(recycle);
    assert_eq!(gpu.enc.events, vec!["clear", "resolve", "copy"]);
    resolve.bytes.borrow_mut().fill(0xa5); pending.buf.bytes.borrow_mut().fill(0xb6);
    gpu.occlusion_pool.as_mut().unwrap().return_completed(256,
        occlusion_pool::Pair { resolve, map: pending.buf }, error_generation, 0, true, true);
    gpu.enc.events.clear(); gpu.pending_resolve.push((1, 11, vec![4]));
    let pending = gpu.record_resolves().unwrap();
    assert_eq!(pending.buf.id, map_id); assert_eq!(gpu.device.created.get(), 2);
    assert_eq!(read(&pending.buf, 0), 5);
    assert!(pending.buf.bytes.borrow()[8..].iter().all(|b| *b == 0));
    assert_eq!(gpu.enc.events, vec!["clear", "resolve", "copy"]);
    let MapKind::Occlusion { list, .. } = pending.kind;
    assert_eq!(list, vec![(0, 1, 100, true, 11)]);
}
#[test]
fn scattered_resolves_use_exact_bytes_and_clear_every_alignment_gap() {
    let mut gpu = Gpu::new(true); gpu.pending_resolve.push((1, 12, vec![2, 7, 9]));
    let pending = gpu.record_resolves().unwrap(); assert_eq!(pending.buf.size, 768);
    assert_eq!(read(&pending.buf, 0), 3); assert_eq!(read(&pending.buf, 256), 8);
    assert_eq!(read(&pending.buf, 512), 10);
    for (i, byte) in pending.buf.bytes.borrow().iter().enumerate() {
        if ![0..8, 256..264, 512..520].iter().any(|range| range.contains(&i)) { assert_eq!(*byte, 0); }
    }
    let MapKind::Occlusion { list, .. } = pending.kind;
    assert_eq!(list, vec![(0, 3, 100, false, 12)]);
}
#[test]
fn disabled_pool_keeps_the_original_allocation_and_encoder_work() {
    let mut gpu = Gpu::new(false); gpu.pending_resolve.push((1, 13, vec![2, 7, 9]));
    let pending = gpu.record_resolves().unwrap(); assert_eq!(pending.buf.size, 1024);
    assert_eq!(gpu.enc.events, vec!["resolve", "resolve", "resolve", "copy"]);
    let MapKind::Occlusion { resolve, recycle, .. } = pending.kind;
    assert!(!recycle); assert_eq!(resolve.usage, BufferUsages::QUERY_RESOLVE | BufferUsages::COPY_SRC);
}
#[test]
fn removed_queries_empty_results_and_missing_set_keep_existing_behavior() {
    let mut gpu = Gpu::new(true);
    gpu.pending_resolve = vec![(99, 1, vec![1]), (1, 14, vec![])];
    assert!(gpu.record_resolves().is_none()); assert_eq!(gpu.device.created.get(), 0);
    assert_eq!(gpu.stored, vec![(100, 0, 14)]);
    gpu.occlusion_set = None; gpu.pending_resolve.push((1, 15, vec![1]));
    assert!(gpu.record_resolves().is_none()); assert_eq!(gpu.device.created.get(), 0);
}
'''


def main() -> None:
    root = Path(__file__).resolve().parents[1]
    gpu = (root / "game-runtime/src/gpu.rs").read_text()
    completion = function(gpu, "finish_ready_maps")
    assert completion.index("pm.failed.load(") < completion.index("get_mapped_range()")
    assert completion.index("invalidate_failed_map()") < completion.index("panic!")
    assert completion.index("self.query_store(") < completion.index("pm.buf.unmap()") < completion.index("pool.return_completed(")
    assert "MapKind::Occlusion { resolve, recycle, error_generation, .. }" in completion
    assert "error_generation, current_generation, recycle && exact_usages, guest_copies_complete" in completion
    assert "if !self.pending_maps[i].done.load(Ordering::SeqCst)" in completion
    record = function(gpu, "record_resolves")
    assert record.index("enc.clear_buffer(") < record.index("enc.resolve_query_set(") < record.index("enc.copy_buffer_to_buffer(")
    assert "kind: MapKind::Occlusion { list: out, resolve: rb, recycle, error_generation }" in record
    query = function(gpu, "query_store")
    assert query.index("!self.live_queries.contains(&addr)") < query.index("guest_span(")
    assert "store(seq as i32, Ordering::SeqCst)" in query
    assert 'var_os("MUGUET_OCCLUSION_POOL").is_some_and(|v|v=="1")' in gpu
    rustc = shutil.which("rustc")
    if rustc is None:
        raise SystemExit("rustc unavailable: source game-runtime/scripts/env.sh first")
    paths = "\n".join(f'#[path = {json.dumps(str(root / ("game-runtime/src/" + name + ".rs")), ensure_ascii=False)}]\nmod {name};'
        for name in ("readback_pool", "occlusion_pool"))
    with tempfile.TemporaryDirectory(prefix="gpu-occlusion-pool-") as temporary:
        source, binary = Path(temporary) / "tests.rs", Path(temporary) / "tests"
        source.write_text(paths + "\n" + HARNESS.replace("RECORD_RESOLVES", record))
        subprocess.run([rustc, "--edition=2021", "--test", str(source), "-o", str(binary)], check=True)
        subprocess.run([str(binary)], check=True)
    print("production pool, resolve recording, completion/query integration guards passed; no GPU performance claim")


if __name__ == "__main__":
    main()
