#!/usr/bin/env python3
"""Exercise the actual production eligibility and upload_buffer body on host.

The queue double synchronously owns its bytes, matching verified WGPU30's API.
This checks byte/order/fallback contracts, not Vulkan/device performance.
"""
from pathlib import Path
import hashlib
import json
import os
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
GPU = ROOT / "game-runtime/src/gpu.rs"
HELPER = ROOT / "game-runtime/src/inline_upload.rs"

HARNESS = r'''
use std::{cell::{Cell, RefCell, UnsafeCell}, rc::Rc, sync::atomic::{AtomicU64, Ordering}};
enum CpuMetric { Upload }
mod cpu_profile { pub fn span(_: super::CpuMetric) {} }
struct SharedReader { bytes: UnsafeCell<Vec<u8>>, borrowed: Cell<u64>, checks: Cell<u64> }
impl SharedReader {
    fn span(&self, at:u64, n:u64) -> std::ops::Range<usize> {
        self.checks.set(self.checks.get()+1);
        let end=at.checked_add(n).unwrap();
        assert!(end <= unsafe{&*self.bytes.get()}.len() as u64);
        usize::try_from(at).unwrap()..usize::try_from(end).unwrap()
    }
    unsafe fn published_inline_bytes(&self,at:u64,n:u64)->&[u8] {
        self.borrowed.set(self.borrowed.get()+1);
        let range=self.span(at,n);
        unsafe { &(&*self.bytes.get())[range] }
    }
}
#[derive(Clone)] struct Buffer;
#[derive(Default)] struct Queue { writes:RefCell<Vec<(u64,Vec<u8>)>> }
impl Queue { fn write_buffer(&self,_:&Buffer,offset:u64,data:&[u8]) {
    self.writes.borrow_mut().push((offset,data.to_vec()));
} }
struct Buf { shadow:Option<Vec<u8>>,version:u64,gpu:Option<Buffer>,bytes:u64,used:bool }
struct Gpu {
    upload_trace_path:Option<std::ffi::OsString>,
    shared_reader:Rc<SharedReader>, inline_upload_borrow:bool,
    inline_upload_counters:inline_upload::Counters, buffer:Option<Buf>, queue:Queue,
    presents:u64,submits:u64,enc_epoch:u64,flushes:u64,mutate_on_flush:bool,
}
impl Gpu {
    fn used(&self,id:u32)->bool { id==1 && self.buffer.as_ref().is_some_and(|b|b.used) }
    fn buf_mut(&mut self,id:u32)->Option<&mut Buf> { if id==1 {self.buffer.as_mut()} else {None} }
    fn bytes(&self,at:u64,n:u64)->&[u8] {
        let range=self.shared_reader.span(at,n);
        unsafe { &(&*self.shared_reader.bytes.get())[range] }
    }
    fn flush(&mut self) {
        self.flushes+=1; self.submits+=1;self.enc_epoch+=1;
        // Adversarial alias change models why the original pre-flush snapshot
        // remains mandatory in this candidate. This is not a claim that the
        // ordinary production flush writes ring data.
        if self.mutate_on_flush { unsafe { (&mut *self.shared_reader.bytes.get()).fill(0xee) }; }
    }
__UPLOAD_BODY__
}
fn gpu(enabled:bool,used:bool,gpu_buffer:bool,shadow:bool)->Gpu {
    Gpu{upload_trace_path:std::env::var_os("MUGUET_GPU_UPLOAD_TRACE"),shared_reader:Rc::new(SharedReader{bytes:UnsafeCell::new((0..512).map(|n|n as u8).collect()),borrowed:Cell::new(0),checks:Cell::new(0)}),
        inline_upload_borrow:enabled,inline_upload_counters:Default::default(),
        buffer:Some(Buf{shadow:shadow.then(||vec![0;128]),version:0,gpu:gpu_buffer.then_some(Buffer),bytes:128,used}),
        queue:Default::default(),presents:0,submits:0,enc_epoch:1,flushes:0,mutate_on_flush:false}
}
fn command(size:u64)->inline_upload::Command {
    inline_upload::published_buffer_command(0,512,64,256,18,3+(size+3)/4).unwrap()
}
#[test] fn exact_empty_aligned_and_padded_payloads() {
    for n in 0..64 { assert!(command(n).contains_exact_payload(84,n)); }
    assert!(!command(4).contains_exact_payload(85,4));
    assert!(!command(4).contains_exact_payload(84,5));
    assert!(!command(4).contains_exact_payload(84,u64::MAX));
}
#[test] fn segment_end_and_wrapped_segments() {
    assert!(inline_upload::published_buffer_command(0,512,64,100,18,7).is_some());
    assert!(inline_upload::published_buffer_command(0,512,64,96,18,7).is_none());
    // Old side of an acquired wrapped batch; cannot reach beyond allocation.
    assert!(inline_upload::published_buffer_command(0,512,400,64,102,25).is_some());
    assert!(inline_upload::published_buffer_command(0,512,400,64,102,27).is_none());
}
#[test] fn invalid_ring_and_header_shapes_fallback() {
    for (ring,size,tail,head,p,n) in [
        (0,512,-1,256,18,4),(0,512,64,-1,18,4),(0,512,64,64,18,4),
        (0,512,60,256,17,4),(0,512,64,516,18,4),(0,512,512,64,130,4),
        (1,512,64,256,18,4),(0,512,65,256,18,4),(0,512,64,255,18,4),
        (0,64,64,64,18,4),(0,512,64,256,18,2),(0,512,64,256,19,4),
        (0,512,64,256,u64::MAX,4),(u64::MAX-63,512,64,256,18,4),
        (0,512,64,256,18,u64::MAX),(0,u64::MAX,64,256,18,4),
    ] { assert!(inline_upload::published_buffer_command(ring,size,tail,head,p,n).is_none()); }
}
#[test] fn memory64_ring_address_keeps_high_bits() {
    let base=1u64<<32;
    let c=inline_upload::published_buffer_command(base,512,64,256,(base+72)/4,4).unwrap();
    assert!(c.contains_exact_payload(base+84,4));
    assert!(!c.contains_exact_payload(84,4));
}
#[test] fn audited_chunk_limit_and_unrepresentable_payload_fallback() {
    let size=4<<20;
    let c=inline_upload::published_buffer_command(0,8<<20,64,6<<20,18,3+size/4).unwrap();
    assert!(c.contains_exact_payload(84,size));
    let too_large=inline_upload::published_buffer_command(0,8<<20,64,6<<20,18,4+size/4).unwrap();
    assert!(!too_large.contains_exact_payload(84,size+1));
}
#[test] fn off_hazard_unknown_or_separate_source_never_borrows() {
    let c=Some(command(8));
    assert!(!inline_upload::borrow_allowed(false,false,false,c,84,8));
    assert!(!inline_upload::borrow_allowed(true,true,false,c,84,8));
    assert!(!inline_upload::borrow_allowed(true,false,false,None,84,8));
    assert!(!inline_upload::borrow_allowed(true,false,false,c,88,8));
    assert!(inline_upload::borrow_allowed(true,true,true,c,84,8));
}
#[test] fn actual_upload_matches_reference_bytes_shadow_padding_and_flushes() {
    for n in 0..64 { for used in [false,true] {for no_overwrite in [false,true] {
        for shadow in [false,true] {for gpu_buffer in [false,true] {
            let mut reference=gpu(false,used,gpu_buffer,shadow);
            let mut candidate=gpu(true,used,gpu_buffer,shadow);
            reference.upload_buffer(1,4,n,84,no_overwrite,Some(command(n)));
            candidate.upload_buffer(1,4,n,84,no_overwrite,Some(command(n)));
            assert_eq!(*candidate.queue.writes.borrow(),*reference.queue.writes.borrow());
            assert_eq!(candidate.buffer.as_ref().unwrap().shadow,reference.buffer.as_ref().unwrap().shadow);
            assert_eq!(candidate.buffer.as_ref().unwrap().version,reference.buffer.as_ref().unwrap().version);
            assert_eq!(candidate.flushes,reference.flushes);
            let borrowed=no_overwrite || !used;
            assert_eq!(candidate.inline_upload_counters.borrowed_calls,u64::from(borrowed));
            assert_eq!(candidate.inline_upload_counters.snapshot_bytes,if borrowed{0}else{n});
            assert_eq!(candidate.shared_reader.checks.get(),1);
            assert_eq!(candidate.shared_reader.borrowed.get(),u64::from(borrowed));
            for (_,write) in candidate.queue.writes.borrow().iter() {assert_eq!(write.len()%4,0);}
            if n%4!=0 && gpu_buffer {assert_eq!(&candidate.queue.writes.borrow().last().unwrap().1[(n%4) as usize..],&vec![0;4-(n%4) as usize]);}
        }}
    }}}
}
#[test] fn hazard_retains_snapshot_before_flush_even_if_alias_changes() {
    let mut g=gpu(true,true,true,true);g.mutate_on_flush=true;
    let old=g.bytes(84,7).to_vec();
    g.upload_buffer(1,0,7,84,false,Some(command(7)));
    assert_eq!(g.flushes,1);assert_eq!(g.inline_upload_counters.snapshot_bytes,7);
    assert_eq!(g.shared_reader.borrowed.get(),0);
    let writes=g.queue.writes.borrow();assert_eq!(&writes[0].1,&old[..4]);
    assert_eq!(&writes[1].1[..3],&old[4..]);
    assert_eq!(&g.buffer.as_ref().unwrap().shadow.as_ref().unwrap()[..7],&old);
}
#[test] fn separate_address_and_malformed_shapes_keep_reference() {
    for c in [None,Some(command(20))] {
        let mut g=gpu(true,false,true,false);
        g.upload_buffer(1,0,8,88,false,c);
        assert_eq!(g.inline_upload_counters.snapshot_bytes,8);
        assert_eq!(g.shared_reader.borrowed.get(),0);
        assert_eq!(g.queue.writes.borrow()[0].1,(88..96).map(|n|n as u8).collect::<Vec<_>>());
    }
}
#[test] fn absent_destination_still_checks_payload_and_counts_decision() {
    let mut g=gpu(true,false,true,false);
    g.upload_buffer(999,0,8,84,false,Some(command(8)));
    assert_eq!(g.shared_reader.checks.get(),1);assert_eq!(g.inline_upload_counters.calls,1);
    assert!(g.queue.writes.borrow().is_empty());assert_eq!(g.flushes,0);
}
#[test] #[should_panic] fn invalid_guest_range_still_rejected_on_snapshot() {
    gpu(false,false,true,false).upload_buffer(999,0,8,510,false,None);
}
#[test] #[should_panic] fn invalid_guest_range_still_rejected_on_borrow() {
    let mut g=gpu(true,false,true,false);
    unsafe {(&mut *g.shared_reader.bytes.get()).truncate(88)};
    g.upload_buffer(1,0,8,84,false,Some(command(8)));
}
#[test] fn queue_owns_bytes_before_source_is_reused() {
    let mut g=gpu(true,false,true,false);
    g.upload_buffer(1,0,8,84,false,Some(command(8)));
    let expected=g.queue.writes.borrow().clone();
    unsafe {(&mut *g.shared_reader.bytes.get()).fill(0xff)};
    assert_eq!(*g.queue.writes.borrow(),expected);
}
#[test] fn counter_accounting_saturates_without_wrap() {
    let mut c=inline_upload::Counters::default();c.record(true,4);c.record(false,8);
    assert_eq!((c.calls,c.borrowed_calls,c.borrowed_bytes,c.snapshot_calls,c.snapshot_bytes),(2,1,4,1,8));
    c.calls=u64::MAX;c.borrowed_bytes=u64::MAX;c.record(true,8);
    assert_eq!(c.calls,u64::MAX);assert_eq!(c.borrowed_bytes,u64::MAX);
}
'''


def main():
    source = GPU.read_text()
    begin = source.index("    fn upload_buffer(")
    end = source.index("    #[allow(clippy::too_many_arguments)]", begin)
    body = source[begin:end]
    # The actual production consumer must classify the acquired batch before
    # exec, and acknowledge tail only afterwards. No producer contract is
    # inferred from these guards; the disassembly review supplies that proof.
    run = source[source.index("    fn run_commands("):source.index("    fn exec(")]
    assert run.index("published_buffer_command(") < run.index("self.exec(") < run.index("self.hdr(1).store(tail")
    assert 'matches!(op, 12 | 47)' in run
    assert 'std::env::var_os("MUGUET_INLINE_UPLOAD_BORROW").is_some_and(|value| value == "1")' in source
    assert body.index("Cow::Owned(self.bytes") < body.index("if !no_overwrite && used { self.flush(); }")
    assert body.count("write_buffer(") == 2
    rustc = shutil.which("rustc")
    if not rustc:
        raise SystemExit("rustc unavailable; source game-runtime/scripts/env.sh")
    before = {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in (GPU, HELPER)}
    with tempfile.TemporaryDirectory(prefix="inline-upload-contract-") as td:
        td = Path(td)
        test = td / "contract.rs"
        test.write_text('#[path=' + json.dumps(str(HELPER), ensure_ascii=False) + '] mod inline_upload;\n' + HARNESS.replace("__UPLOAD_BODY__", body))
        binary = td / "tests"
        subprocess.run([rustc, "--edition=2021", "--test", str(test), "-o", str(binary)], check=True)
        subprocess.run([str(binary), "--test-threads=1"], check=True, env={k:v for k,v in os.environ.items() if k != "MUGUET_GPU_UPLOAD_TRACE"})
    assert before == {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in (GPU, HELPER)}
    print(json.dumps({"status":"pass", "tests":14, "differentialCases":1024,
        "scope":"actual production upload_buffer and eligibility helper with synchronous queue double; no device/FPS evidence", "sourceSHA256":before}))


if __name__ == "__main__":
    main()
