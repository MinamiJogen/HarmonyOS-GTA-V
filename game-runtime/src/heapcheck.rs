// Diagnostic-only observation of the supplied game's dlmalloc metadata.
// Concurrent snapshots are evidence of observations, never proof of a writer.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

// Verified in original emscripten_builtin_free/malloc, not guessed from a host ABI.
// free: DV=29600848, dvsize=29600824; TOP=29600856, topsize=29600832.
// smallbin sentinel=29600888+16*i; tree root slot=29601416+8*i.
const GM: u64 = 29_600_816;
const GM_DVSIZE: u64 = 8;
const GM_TOPSIZE: u64 = 16;
const GM_DV: u64 = 32;
const GM_TOP: u64 = 40;
const GM_SMALLBINS: u64 = 72;
const GM_TREEBINS: u64 = 600;
const GM_SEG_BASE: u64 = 928;
const GM_SEG_SIZE: u64 = 936;
const PINUSE: u64 = 1;
const CINUSE: u64 = 2;
const MAX_ERRORS: usize = 64;
const MAX_CHUNKS: u64 = 1_000_000;
const JOURNAL_MAX: usize = 65_536;
const JOURNAL_AGE: Duration = Duration::from_secs(60);
#[cfg(not(test))]
const RECHECK_DELAY: Duration = Duration::from_millis(80);

static ENABLED: AtomicBool = AtomicBool::new(false);
static JOURNAL: Mutex<VecDeque<(u64, u64, &'static str, Instant)>> = Mutex::new(VecDeque::new());
static PENDING: Mutex<Option<&'static str>> = Mutex::new(None);
static WORKER: OnceLock<std::thread::Thread> = OnceLock::new();
static REQUESTS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static SNAPSHOTS: AtomicU64 = AtomicU64::new(0);

pub fn enabled() -> bool { ENABLED.load(Ordering::Relaxed) }
pub fn note(addr: u64, len: u64, what: &'static str) {
    if !enabled() { return; }
    let now = Instant::now();
    let mut journal = JOURNAL.lock().unwrap_or_else(|p| p.into_inner());
    while journal.front().is_some_and(|entry| now.duration_since(entry.3) > JOURNAL_AGE) {
        journal.pop_front();
    }
    if journal.len() == JOURNAL_MAX { journal.pop_front(); }
    journal.push_back((addr, len, what, now));
}

/// The caller additionally gates this with its private diagnostic marker.
pub fn start() {
    if std::env::var_os("MUGUET_HEAPCHECK").is_none() || ENABLED.swap(true, Ordering::AcqRel) { return; }
    #[cfg(not(test))]
    match std::thread::Builder::new().name("heapcheck".into()).spawn(run) {
        Ok(worker) => { let _ = WORKER.set(worker.thread().clone()); }
        Err(error) => {
            ENABLED.store(false, Ordering::Release);
            eprintln!("[heapcheck] worker could not start: {error}");
            return;
        }
    }
    eprintln!("[heapcheck] enabled on-demand only; journal_limit={JOURNAL_MAX}; concurrent observations are not root-cause proof");
    request_snapshot("startup");
}

/// Non-blocking request; multiple pending requests coalesce to their latest reason.
pub fn request_snapshot(reason: &'static str) {
    if !enabled() { return; }
    *PENDING.lock().unwrap_or_else(|p| p.into_inner()) = Some(reason);
    REQUESTS.fetch_add(1, Ordering::Relaxed);
    if let Some(worker) = WORKER.get() { worker.unpark(); }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Bad { kind: &'static str, chunk: u64, address: u64, observed: u64, expected: u64 }
#[derive(Clone, Debug, Default)]
struct Snapshot {
    chunks: u64, free_chunks: u64, top: u64, topsize: u64, dv: u64, dvsize: u64,
    base: u64, segment_size: u64, next_segment: u64, complete: bool,
    errors: Vec<Bad>, errors_truncated: bool,
}
impl Snapshot {
    fn bad(&mut self, kind: &'static str, chunk: u64, address: u64, observed: u64, expected: u64) {
        if self.errors.len() < MAX_ERRORS { self.errors.push(Bad { kind, chunk, address, observed, expected }); }
        else { self.errors_truncated = true; }
    }
}
fn span(addr: u64, bytes: u64, len: u64) -> bool {
    addr % 8 == 0 && addr.checked_add(bytes).is_some_and(|end| end <= len)
}
fn overlap(addr: u64, bytes: u64, lo: u64, hi: u64) -> bool {
    addr < hi && addr.saturating_add(bytes.max(1)) > lo
}
fn stable_errors(first: &Snapshot, second: &Snapshot) -> Vec<Bad> {
    first.errors.iter().filter(|bad| second.errors.contains(bad)).cloned().collect()
}
fn inspect(read: &impl Fn(u64) -> Option<u64>, len: u64) -> Snapshot {
    let mut s = Snapshot::default();
    if !span(GM, 960, len) { s.bad("allocator_header_unavailable", 0, GM, len, GM + 960); return s; }
    let g = |offset| read(GM + offset).unwrap_or(0);
    s.top = g(GM_TOP); s.topsize = g(GM_TOPSIZE); s.dv = g(GM_DV); s.dvsize = g(GM_DVSIZE);
    s.base = g(GM_SEG_BASE); s.segment_size = g(GM_SEG_SIZE); s.next_segment = g(944);
    if s.base == 0 || s.top == 0 { return s; }
    let Some(end) = s.base.checked_add(s.segment_size).filter(|end| *end <= len) else {
        s.bad("segment_out_of_bounds", s.base, GM + GM_SEG_SIZE, s.segment_size, len); return s;
    };
    let least = g(24);
    let heap_pointer = |p, bytes| p >= least && p != 0 && span(p, bytes, len);
    // Primary-segment chunk walk. Additional segments are explicitly reported.
    // Free-list roots are still examined independently below.
    if s.top >= s.base && s.top < end {
        let mut q = (s.base.saturating_add(7)) & !7;
        let mut previous_inuse = true;
        while q < s.top && s.chunks < MAX_CHUNKS {
            let Some(head) = read(q + 8) else { s.bad("chunk_head_unreadable", q, q + 8, 0, 0); break; };
            let size = head & !7;
            if size < 32 || size % 8 != 0 {
                s.bad("chunk_size_invalid", q, q + 8, head, 32); break;
            }
            let Some(next) = q.checked_add(size).filter(|next| *next <= s.top) else {
                s.bad("chunk_crosses_top", q, q + 8, size, s.top); break;
            };
            if (head & PINUSE != 0) != previous_inuse {
                s.bad("previous_inuse_mismatch", q, q + 8, head & PINUSE, previous_inuse as u64);
            }
            let inuse = head & CINUSE != 0;
            if !inuse {
                s.free_chunks += 1;
                if !previous_inuse { s.bad("adjacent_free_chunks", q, q + 8, head, 0); }
                let footer = read(next).unwrap_or(0);
                if footer != size { s.bad("free_footer_mismatch", q, next, footer, size); }
                if q == s.dv {
                    if size != s.dvsize { s.bad("dv_size_mismatch", q, GM + GM_DVSIZE, s.dvsize, size); }
                } else {
                    check_links(&mut s, read, len, least, q, size);
                }
            }
            previous_inuse = inuse; q = next; s.chunks += 1;
        }
        if q == s.top {
            let head = read(s.top + 8).unwrap_or(0);
            if head & !7 != s.topsize { let (top, size)=(s.top,s.topsize); s.bad("top_size_mismatch",top,top+8,head & !7,size); }
            if (head & PINUSE != 0) != previous_inuse { let top=s.top; s.bad("top_previous_inuse_mismatch",top,top+8,head & PINUSE,previous_inuse as u64); }
            s.complete = s.next_segment == 0;
        } else if s.chunks == MAX_CHUNKS { s.bad("chunk_walk_limit", q, q, s.chunks, MAX_CHUNKS); }
    }
    if s.dv != 0 && !heap_pointer(s.dv, 32) { let dv=s.dv; s.bad("dv_pointer_invalid",dv,GM+GM_DV,dv,len); }
    let maps = g(0);
    let smallmap = maps as u32;
    let treemap = (maps >> 32) as u32;
    for index in 0..32_u64 {
        // Empty bins may retain stale links after the bitmap is cleared.
        if smallmap & (1 << index) != 0 {
            let sentinel = GM + GM_SMALLBINS + index * 16;
            for (offset, reciprocal) in [(16,24),(24,16)] {
                let p = read(sentinel + offset).unwrap_or(0);
                if !heap_pointer(p,32) || p == sentinel {
                    s.bad("smallbin_root_invalid",sentinel,sentinel+offset,p,0);
                } else if read(p + reciprocal) != Some(sentinel) {
                    s.bad("smallbin_root_reciprocal",p,p+reciprocal,read(p+reciprocal).unwrap_or(0),sentinel);
                }
            }
        }
        if treemap & (1 << index) != 0 {
            let slot = GM + GM_TREEBINS + index * 8;
            let p = read(slot).unwrap_or(0);
            if !heap_pointer(p,64) { s.bad("tree_root_invalid",p,slot,p,0); }
            else if read(p+48) != Some(slot) {
                s.bad("tree_root_parent",p,p+48,read(p+48).unwrap_or(0),slot);
            }
        }
    }
    s
}
fn check_links(s: &mut Snapshot, read: &impl Fn(u64)->Option<u64>, len: u64, least: u64, q: u64, size: u64) {
    let sentinel = (size < 256).then_some(GM + GM_SMALLBINS + (size >> 3) * 16);
    for (offset,reciprocal,kind) in [(16,24,"fd_reciprocal"),(24,16,"bk_reciprocal")] {
        let p = read(q + offset).unwrap_or(0);
        let valid = sentinel == Some(p) || (p != 0 && p >= least && span(p,32,len));
        if !valid { s.bad("free_link_pointer_invalid",q,q+offset,p,len); }
        else {
            let back = read(p+reciprocal).unwrap_or(0);
            if back != q { s.bad(kind,q,p+reciprocal,back,q); }
        }
    }
    if size < 256 { return; }
    let index = read(q+56).unwrap_or(u64::MAX) & 0xffff_ffff;
    if index >= 32 { s.bad("tree_index_invalid",q,q+56,index,31); return; }
    let root_slot = GM + GM_TREEBINS + index * 8;
    let parent = read(q+48).unwrap_or(0);
    if parent == root_slot {
        if read(root_slot) != Some(q) { s.bad("tree_root_membership",q,root_slot,read(root_slot).unwrap_or(0),q); }
    } else if parent != 0 {
        if parent < least || !span(parent,64,len) { s.bad("tree_parent_pointer_invalid",q,q+48,parent,len); }
        else if read(parent+32) != Some(q) && read(parent+40) != Some(q) {
            s.bad("tree_parent_child_reciprocal",q,q+48,parent,q);
        }
    } else if read(q+16) == Some(q) {
        s.bad("tree_orphan_singleton",q,q+48,0,root_slot);
    }
    for offset in [32,40] {
        let child = read(q+offset).unwrap_or(0);
        if child == 0 { continue; }
        if child < least || !span(child,64,len) { s.bad("tree_child_pointer_invalid",q,q+offset,child,len); }
        else if read(child+48) != Some(q) { s.bad("tree_child_parent_reciprocal",q,child+48,read(child+48).unwrap_or(0),q); }
    }
}

#[cfg(not(test))]
fn runtime_read(addr: u64) -> Option<u64> {
    let rt = crate::RT.get()?;
    let (base,len) = crate::mem_base_len(&rt.mem);
    if !span(addr,8,len as u64) { return None; }
    // Aligned independent reads only. No allocator lock or transactional claim.
    Some(unsafe { (&*(base.add(usize::try_from(addr).ok()?) as *const AtomicU64)).load(Ordering::Relaxed) })
}
#[cfg(not(test))]
fn run() {
    loop {
        let reason = PENDING.lock().unwrap_or_else(|p| p.into_inner()).take();
        let Some(reason) = reason else { std::thread::park(); continue; };
        let Some(rt) = crate::RT.get() else { continue; };
        let started = Instant::now();
        let first = inspect(&runtime_read,rt.mem.data_size() as u64);
        std::thread::sleep(RECHECK_DELAY);
        let second = inspect(&runtime_read,rt.mem.data_size() as u64);
        let stable = stable_errors(&first,&second);
        let sequence = SNAPSHOTS.fetch_add(1,Ordering::Relaxed)+1;
        eprintln!("[heapcheck] snapshot={sequence} reason={reason} chunks={} free={} stable={} first_only={} second_only={} complete={} extra_segment={} elapsed_ms={} concurrent=true root_cause_proven=false",second.chunks,second.free_chunks,stable.len(),first.errors.len()-stable.len(),second.errors.len()-stable.len(),second.complete,second.next_segment,started.elapsed().as_millis());
        for bad in &stable { eprintln!("[heapcheck] repeated anomaly kind={} chunk=0x{:x} address=0x{:x} observed=0x{:x} expected=0x{:x}",bad.kind,bad.chunk,bad.address,bad.observed,bad.expected); }
        save_report(sequence,reason,&first,&second,&stable);
    }
}
#[cfg(not(test))]
fn save_report(sequence:u64,reason:&str,first:&Snapshot,second:&Snapshot,stable:&[Bad]) {
    use serde_json::json;
    let Some(root)=std::env::var_os("MUGUET_SUPPORT").map(std::path::PathBuf::from) else {return;};
    // Ring filenames bound report/artifact growth even if the private harness repeats.
    let slot=(sequence-1)%16;
    let now=Instant::now();
    let mut binary=Vec::new();
    let mut ranges=Vec::new();
    let mut records=Vec::new();
    let journal:Vec<_>=JOURNAL.lock().unwrap_or_else(|p|p.into_inner()).iter().copied().collect();
    for bad in stable {
        let center=if bad.chunk!=0 {bad.chunk}else{bad.address};
        let lo=center.saturating_sub(64)&!7;
        let hi=center.saturating_add(256);
        let hits:Vec<_>=journal.iter().rev().filter(|(addr,len,_,time)|now.duration_since(*time)<=JOURNAL_AGE && (overlap(*addr,*len,lo,hi)||overlap(*addr,*len,bad.address,bad.address.saturating_add(8))))
            .take(32).map(|(addr,len,kind,time)|json!({"address":addr,"bytes":len,"kind":kind,"ageMs":now.duration_since(*time).as_millis()})).collect();
        let words:Vec<_>=(0..40).map(|i|json!({"address":lo+i*8,"value":runtime_read(lo+i*8)})).collect();
        records.push(json!({"kind":bad.kind,"chunk":bad.chunk,"address":bad.address,"observed":bad.observed,"expected":bad.expected,"words":words,"recentOverlappingWrites":hits}));
        // Fail-only bounded fragments: at most eight 320-byte regions + GM header.
        if ranges.len()<8 {
            let offset=binary.len();
            for i in 0..40 {binary.extend_from_slice(&runtime_read(lo+i*8).unwrap_or(0).to_le_bytes());}
            ranges.push(json!({"linearAddress":lo,"fileOffset":offset,"bytes":320}));
        }
    }
    let journal_size=journal.len();
    if !stable.is_empty() {
        let offset=binary.len();
        for i in 0..120 {binary.extend_from_slice(&runtime_read(GM+i*8).unwrap_or(0).to_le_bytes());}
        ranges.push(json!({"linearAddress":GM,"fileOffset":offset,"bytes":960}));
        if let Err(error)=std::fs::write(root.join(format!("heap-diagnostic-{slot}.bin")),&binary) {eprintln!("[heapcheck] fragment write failed: {error}");}
    } else {let _=std::fs::remove_file(root.join(format!("heap-diagnostic-{slot}.bin")));}
    let raw_anomalies=|sample:&Snapshot|sample.errors.iter().map(|bad|json!({"kind":bad.kind,"chunk":bad.chunk,"address":bad.address,"observed":bad.observed,"expected":bad.expected})).collect::<Vec<_>>();
    let report=json!({"sequence":sequence,"reason":reason,"concurrentObservation":true,"rootCauseProven":false,
        "scope":"Primary-segment heap walk and active bin roots; additional linked segments are not walked; all reads are independent and nontransactional",
        "chunks":second.chunks,"freeChunks":second.free_chunks,"complete":second.complete,"additionalSegment":second.next_segment,
        "top":second.top,"topsize":second.topsize,"dv":second.dv,"dvsize":second.dvsize,"segmentBase":second.base,"segmentBytes":second.segment_size,
        "firstAnomalies":first.errors.len(),"secondAnomalies":second.errors.len(),"repeatedAnomalies":records,"anomaliesTruncated":first.errors_truncated||second.errors_truncated,
        "firstObservedAnomalies":raw_anomalies(first),"secondObservedAnomalies":raw_anomalies(second),
        "journalEntries":journal_size,"journalLimit":JOURNAL_MAX,"journalAgeSeconds":60,"binaryFragments":ranges,
        "binaryBytes":binary.len(),"limitations":"Repeated metadata anomalies do not identify their writer. Write overlaps are candidates, not causality. A clean sampled walk does not exclude changes between snapshots."});
    if let Err(error)=std::fs::write(root.join(format!("heap-diagnostic-{slot}.json")),format!("{report}\n")) {eprintln!("[heapcheck] report write failed: {error}");}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    const BASE:u64=GM+4096;
    const LEN:u64=GM+65536;
    fn setup()->BTreeMap<u64,u64> {
        let mut m=BTreeMap::new();
        for (o,v) in [(24,BASE),(GM_SEG_BASE,BASE),(GM_SEG_SIZE,8192),(GM_TOP,BASE+1024),(GM_TOPSIZE,7168)] {m.insert(GM+o,v);}
        m.insert(BASE+8,1027);m.insert(BASE+1024+8,7169);m
    }
    fn scan(m:&BTreeMap<u64,u64>)->Snapshot {inspect(&|a|if span(a,8,LEN){Some(*m.get(&a).unwrap_or(&0))}else{None},LEN)}
    fn free(m:&mut BTreeMap<u64,u64>,q:u64,size:u64) {m.insert(q+8,size|1);m.insert(q+size,size);m.insert(q+size+8,(1024-(q+size-BASE))|2);}
    fn tree(m:&mut BTreeMap<u64,u64>,q:u64,size:u64,index:u64,parent:u64) {
        m.insert(q+16,q);m.insert(q+24,q);m.insert(q+48,parent);m.insert(q+56,index);
        m.insert(GM+GM_TREEBINS+index*8,q);m.insert(GM,1_u64<<(32+index));free(m,q,size);
    }
    #[test] fn allocated_heap_is_clean(){let s=scan(&setup());assert!(s.errors.is_empty(),"{:?}",s.errors);assert!(s.complete);}
    #[test] fn dv_is_not_a_linked_free_chunk(){let mut m=setup();free(&mut m,BASE,256);m.insert(GM+GM_DV,BASE);m.insert(GM+GM_DVSIZE,256);let s=scan(&m);assert!(s.errors.is_empty(),"{:?}",s.errors);}
    #[test] fn root_parent_slot_and_tree_ring_are_valid(){let mut m=setup();tree(&mut m,BASE,256,0,GM+GM_TREEBINS);let s=scan(&m);assert!(s.errors.is_empty(),"{:?}",s.errors);}
    #[test] fn invalid_fd_is_detected_without_dereference(){let mut m=setup();tree(&mut m,BASE,256,0,GM+GM_TREEBINS);m.insert(BASE+16,u64::MAX-7);let s=scan(&m);assert!(s.errors.iter().any(|b|b.kind=="free_link_pointer_invalid"));}
    #[test] fn broken_bk_and_child_parent_are_both_reported(){let mut m=setup();tree(&mut m,BASE,256,0,GM+GM_TREEBINS);m.insert(BASE+24,BASE+512);m.insert(BASE+32,BASE+512);let s=scan(&m);assert!(s.errors.iter().any(|b|b.kind=="bk_reciprocal"));assert!(s.errors.iter().any(|b|b.kind=="tree_child_parent_reciprocal"));}
    #[test] fn smallbin_sentinel_is_valid(){let mut m=setup();free(&mut m,BASE,64);let sent=GM+GM_SMALLBINS+8*16;m.insert(GM,1<<8);m.insert(BASE+16,sent);m.insert(BASE+24,sent);m.insert(sent+16,BASE);m.insert(sent+24,BASE);assert!(scan(&m).errors.is_empty());}
    #[test] fn unused_bin_stale_words_are_not_errors(){let mut m=setup();m.insert(GM+GM_SMALLBINS+16,u64::MAX);m.insert(GM+GM_TREEBINS,u64::MAX);assert!(scan(&m).errors.is_empty());}
    #[test] fn invalid_child_and_parent_are_bounded(){let mut m=setup();tree(&mut m,BASE,256,0,u64::MAX-7);m.insert(BASE+32,u64::MAX-7);let s=scan(&m);assert!(s.errors.iter().any(|b|b.kind=="tree_parent_pointer_invalid"));assert!(s.errors.iter().any(|b|b.kind=="tree_child_pointer_invalid"));}
    #[test] fn changed_or_disappeared_anomalies_are_transient(){let mut m=setup();tree(&mut m,BASE,256,0,GM+GM_TREEBINS);m.insert(BASE+16,u64::MAX-7);let a=scan(&m);assert!(!stable_errors(&a,&a).is_empty());m.insert(BASE+16,u64::MAX-15);assert!(stable_errors(&a,&scan(&m)).is_empty());assert!(stable_errors(&a,&scan(&setup())).is_empty());}
    #[test] fn bad_size_and_footer_are_observations(){let mut m=setup();free(&mut m,BASE,64);m.insert(BASE+64,32);let s=scan(&m);assert!(s.errors.iter().any(|b|b.kind=="free_footer_mismatch"));m.insert(BASE+8,u64::MAX);assert!(scan(&m).errors.iter().any(|b|b.kind=="chunk_crosses_top"));}
    #[test] fn overflow_overlap_and_span_are_safe(){assert!(!span(u64::MAX-7,16,u64::MAX));assert!(!span(1,8,LEN));assert!(overlap(u64::MAX-7,32,u64::MAX-16,u64::MAX));assert!(!overlap(1,3,4,8));}
}
