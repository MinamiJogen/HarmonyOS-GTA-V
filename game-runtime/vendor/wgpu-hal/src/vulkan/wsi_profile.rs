//! Opt-in wall timing of the two existing native acquire waits. No new wait or query.
use std::{cell::RefCell, io::Write, path::PathBuf, sync::OnceLock, time::Instant};
const MAX_WINDOWS: u32 = 360;
struct Config { dir: PathBuf }
static CONFIG: OnceLock<Option<Config>> = OnceLock::new();
fn config() -> Option<&'static Config> {
    CONFIG.get_or_init(|| {
        if std::env::var_os("MUGUET_GPU_PROFILE").is_none_or(|v| v != "1") { return None; }
        let dir=PathBuf::from(std::env::var_os("MUGUET_GPU_PROFILE_DIR")?);
        let _=std::fs::create_dir_all(&dir);
        let _=std::fs::File::create(dir.join("wsi.jsonl"));
        Some(Config {dir})
    }).as_ref()
}
#[derive(Clone,Copy)]
pub(super) enum Kind { ReuseSubmissionFence, AcquireNextImage }
struct Stats { since:Instant, calls:[u64;2], ns:[u64;2], max:[u64;2], windows:u32 }
std::thread_local! { static STATS: RefCell<Option<Stats>> = const {RefCell::new(None)}; }
pub(super) struct Span(Option<(Kind,Instant)>);
pub(super) fn span(kind:Kind) -> Span {
    if config().is_none() { return Span(None); }
    Span(Some((kind,Instant::now())))
}
impl Drop for Span {
    fn drop(&mut self) {
        let Some((kind,start))=self.0 else {return;};
        let now=Instant::now(); let ns=now.duration_since(start).as_nanos().min(u64::MAX as u128) as u64;
        STATS.with(|state| {
            let mut state=state.borrow_mut();
            let s=state.get_or_insert_with(||Stats {since:start,calls:[0;2],ns:[0;2],max:[0;2],windows:0});
            let k=kind as usize; s.calls[k]+=1; s.ns[k]=s.ns[k].saturating_add(ns); s.max[k]=s.max[k].max(ns);
            if now.duration_since(s.since).as_secs()<10 || s.windows>=MAX_WINDOWS {return;}
            s.windows+=1;
            if let Some(c)=config() {
                if let Ok(mut f)=std::fs::OpenOptions::new().append(true).create(true).open(c.dir.join("wsi.jsonl")) {
                    let _=writeln!(f,"{{\"processId\":{},\"window\":{},\"wallNs\":{},\"reuseSubmissionFence\":{{\"calls\":{},\"ns\":{},\"maxNs\":{}}},\"vkAcquireNextImageKHR\":{{\"calls\":{},\"ns\":{},\"maxNs\":{}}},\"truncated\":{}}}",
                        std::process::id(),s.windows,now.duration_since(s.since).as_nanos(),s.calls[0],s.ns[0],s.max[0],s.calls[1],s.ns[1],s.max[1],s.windows==MAX_WINDOWS);
                }
            }
            s.since=now;s.calls=[0;2];s.ns=[0;2];s.max=[0;2];
        });
    }
}
