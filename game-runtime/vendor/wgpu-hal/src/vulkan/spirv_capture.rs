//! Bounded capture of the exact final SPIR-V at vkCreateShaderModule.
use std::{cell::RefCell, collections::BTreeMap, fmt::Write as _, fs, io::Write as _, path::PathBuf, string::{String, ToString}, sync::{atomic::{AtomicU64, Ordering}, Mutex, OnceLock}, write, vec::Vec};
#[cfg(test)] use std::vec;

const MAX_FILES: usize = 1024;
const MAX_BYTES: usize = 128 << 20;

#[derive(Clone, Default)]
struct Context { sequence: u64, kind: String, label: Option<String> }
std::thread_local! { static PIPELINE: RefCell<Option<Context>> = const { RefCell::new(None) }; }
static PIPELINE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
pub struct PipelineScope(Option<Context>);
impl Drop for PipelineScope { fn drop(&mut self) { PIPELINE.with(|c| { *c.borrow_mut() = self.0.take(); }); } }
pub fn pipeline_scope(kind: &str, label: Option<&str>) -> PipelineScope {
    let context = Context { sequence: PIPELINE_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1, kind: kind.into(), label: label.map(Into::into) };
    PipelineScope(PIPELINE.with(|c| c.replace(Some(context))))
}
pub fn pipeline_sequence() -> Option<u64> { PIPELINE.with(|c| c.borrow().as_ref().map(|v| v.sequence)) }

pub struct Metadata<'a> {
    pub stage: &'a str,
    pub entry_point: Option<&'a str>,
    pub shader_label: Option<&'a str>,
    pub layout_report: &'a str,
    pub sample_mask_report: &'a str,
    pub retained_mixed: usize,
}

#[derive(Clone)]
struct OwnedMetadata { stage: String, entry_point: Option<String>, shader_label: Option<String>, layout_report: String, sample_mask_report: String, retained_mixed: usize }
std::thread_local! { static SHADER: RefCell<Option<OwnedMetadata>> = const { RefCell::new(None) }; }
pub struct ShaderScope(Option<OwnedMetadata>);
impl Drop for ShaderScope { fn drop(&mut self) { SHADER.with(|c| { *c.borrow_mut() = self.0.take(); }); } }
pub fn shader_scope(meta: Metadata<'_>) -> ShaderScope {
    let own = OwnedMetadata { stage:meta.stage.into(), entry_point:meta.entry_point.map(Into::into), shader_label:meta.shader_label.map(Into::into), layout_report:meta.layout_report.into(), sample_mask_report:meta.sample_mask_report.into(), retained_mixed:meta.retained_mixed };
    ShaderScope(SHADER.with(|c| c.replace(Some(own))))
}
pub fn capture_final(words: &[u32], label: Option<&str>) {
    let own = SHADER.with(|c| c.borrow().clone());
    let meta = match own.as_ref() {
        Some(v) => Metadata { stage:&v.stage, entry_point:v.entry_point.as_deref(), shader_label:v.shader_label.as_deref().or(label), layout_report:&v.layout_report, sample_mask_report:&v.sample_mask_report, retained_mixed:v.retained_mixed },
        None => Metadata { stage:"RawSpirV", entry_point:None, shader_label:label, layout_report:"direct SPIR-V input; no Naga legalization", sample_mask_report:"not applied", retained_mixed:0 },
    };
    capture(words, meta);
}

fn quoted(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""), '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"), '\r' => out.push_str("\\r"), '\t' => out.push_str("\\t"),
            c if c < '\u{20}' => { let _ = write!(out, "\\u{:04x}", c as u32); },
            c => out.push(c),
        }
    }
    out.push('"'); out
}
fn optional(value: Option<&str>) -> String { value.map(quoted).unwrap_or_else(|| "null".into()) }

struct State {
    root: PathBuf,
    directory: PathBuf,
    seen: BTreeMap<String, usize>,
    attempts: u64,
    duplicates: u64,
    missed: u64,
    failed: u64,
    mixed: u64,
    total_bytes: usize,
    truncated: bool,
    diagnostic_bytes: usize,
    diagnostic_truncated: bool,
    max_files: usize,
    max_bytes: usize,
}
impl State {
    fn new(root: PathBuf, max_files: usize, max_bytes: usize) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|v| v.as_millis()).unwrap_or(0);
        let directory = root.join(std::format!("all-modules-{}-{stamp}", std::process::id()));
        Self { root, directory, seen: BTreeMap::new(), attempts: 0, duplicates: 0, missed: 0, failed: 0, mixed: 0, total_bytes: 0, truncated: false, diagnostic_bytes:0, diagnostic_truncated:false, max_files, max_bytes }
    }
    fn status(&self) -> std::io::Result<()> {
        let record = std::format!("{{\"directory\":{},\"attempts\":{},\"uniqueFiles\":{},\"duplicateModules\":{},\"missedModules\":{},\"failedWrites\":{},\"mixedLayoutReports\":{},\"retainedBytes\":{},\"diagnosticBytes\":{},\"diagnosticTruncated\":{},\"maxFiles\":{},\"maxBytes\":{},\"truncated\":{},\"hash\":\"SHA-256\"}}\n", quoted(&self.directory.to_string_lossy()), self.attempts, self.seen.len(), self.duplicates, self.missed, self.failed, self.mixed, self.total_bytes, self.diagnostic_bytes, self.diagnostic_truncated, self.max_files, self.max_bytes, self.truncated);
        fs::create_dir_all(&self.root)?;
        write_sync(&self.root.join("full-capture-status.json"), record.as_bytes())
    }
    fn diagnostic(&mut self, kind: &str, record: &str) -> std::io::Result<()> {
        let line = std::format!("{{\"event\":{},\"record\":{}}}\n", quoted(kind), record.trim());
        let limit = self.max_bytes.min(1 << 20) / 8;
        if self.diagnostic_bytes + line.len() > limit { self.diagnostic_truncated = true; return Ok(()); }
        fs::create_dir_all(&self.directory)?;
        let mut file=fs::OpenOptions::new().create(true).append(true).open(self.directory.join("diagnostic-events.jsonl"))?;
        file.write_all(line.as_bytes())?;file.sync_all()?;self.diagnostic_bytes += line.len();Ok(())
    }
    fn capture(&mut self, words: &[u32], meta: &Metadata<'_>, context: Option<&Context>) -> std::io::Result<()> {
        self.attempts += 1;
        self.mixed += (meta.retained_mixed != 0) as u64;
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let hash = sha256(&bytes);
        let index = self.seen.get(&hash).copied().unwrap_or(self.seen.len() + 1);
        let name = std::format!("module-{index:04}-{hash}");
        let sequence = context.map(|c| c.sequence.to_string()).unwrap_or_else(|| "null".into());
        let record = std::format!("{{\"counter\":{index},\"sha256\":{},\"file\":{},\"bytes\":{},\"wordCount\":{},\"stage\":{},\"entryPoint\":{},\"shaderLabel\":{},\"firstPipelineSequence\":{sequence},\"pipelineKind\":{},\"pipelineLabel\":{},\"retainedMixed\":{},\"layoutReport\":{},\"sampleMaskReport\":{}}}\n", quoted(&hash), quoted(&std::format!("{name}.spv")), bytes.len(), words.len(), quoted(meta.stage), optional(meta.entry_point), optional(meta.shader_label), optional(context.map(|c| c.kind.as_str())), optional(context.and_then(|c| c.label.as_deref())), meta.retained_mixed, quoted(meta.layout_report), quoted(meta.sample_mask_report));
        if meta.retained_mixed != 0 { self.diagnostic("mixed-layout", &record)?; }
        if self.seen.contains_key(&hash) { self.duplicates += 1; return Ok(()); }
        // Reserve space for status and bounded manifest overhead as well.
        let cost = bytes.len().checked_add(record.len() * 2).and_then(|v| v.checked_add(256)).unwrap_or(usize::MAX);
        if index > self.max_files || self.total_bytes.checked_add(cost).map_or(true, |n| n > self.max_bytes.saturating_sub(self.max_bytes.min(1 << 20) / 8 + 4096)) {
            self.truncated = true; self.missed += 1; self.diagnostic("capture-capacity-missed", &record)?; return Ok(());
        }
        fs::create_dir_all(&self.directory)?;
        write_sync(&self.directory.join(std::format!("{name}.spv")), &bytes)?;
        write_sync(&self.directory.join(std::format!("{name}.json")), record.as_bytes())?;
        let mut manifest = fs::OpenOptions::new().create(true).append(true).open(self.directory.join("manifest.jsonl"))?;
        manifest.write_all(record.as_bytes())?; manifest.sync_all()?;
        self.seen.insert(hash, index); self.total_bytes += cost;
        Ok(())
    }
}
fn write_sync(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new().create(true).truncate(true).write(true).open(path)?;
    file.write_all(bytes)?; file.sync_all()
}
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
pub fn capture(words: &[u32], meta: Metadata<'_>) {
    let Some(root) = std::env::var_os("MUGUET_PIPELINE_TRACE_DIR") else { return; };
    let state = STATE.get_or_init(|| Mutex::new(State::new(PathBuf::from(root), MAX_FILES, MAX_BYTES)));
    if let Ok(mut state) = state.lock() {
        let context = PIPELINE.with(|c| c.borrow().clone());
        if let Err(error) = state.capture(words, &meta, context.as_ref()) {
            state.failed += 1;
            std::eprintln!("[wgpu] full SPIR-V capture write failed: {error}");
        }
        if let Err(error) = state.status() { state.failed += 1; std::eprintln!("[wgpu] full SPIR-V capture status write failed: {error}"); }
    }
}

fn sha256(input: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
        0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
        0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
        0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
        0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
        0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
        0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
        0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2,
    ];
    let mut data = input.to_vec(); data.push(0x80);
    while data.len() % 64 != 56 { data.push(0); }
    data.extend_from_slice(&(input.len() as u64).wrapping_mul(8).to_be_bytes());
    let mut h: [u32; 8] = [0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19];
    for chunk in data.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, b) in chunk.chunks_exact(4).enumerate() { w[i] = u32::from_be_bytes(b.try_into().unwrap()); }
        for i in 16..64 {
            let s0 = w[i-15].rotate_right(7) ^ w[i-15].rotate_right(18) ^ (w[i-15] >> 3);
            let s1 = w[i-2].rotate_right(17) ^ w[i-2].rotate_right(19) ^ (w[i-2] >> 10);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }
        let [mut a,mut b,mut c,mut d,mut e,mut f,mut g,mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let t2 = s0.wrapping_add((a & b) ^ (a & c) ^ (b & c));
            hh=g;g=f;f=e;e=d.wrapping_add(t1);d=c;c=b;b=a;a=t1.wrapping_add(t2);
        }
        for (v, n) in h.iter_mut().zip([a,b,c,d,e,f,g,hh]) { *v = v.wrapping_add(n); }
    }
    let mut out = String::new(); for v in h { let _ = write!(out, "{v:08x}"); } out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn sha256_known_vectors() {
        assert_eq!(sha256(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(sha256(&vec![b'a'; 1_000_000]), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }
    #[test] fn quote_preserves_labels_as_json_data() { assert_eq!(quoted("a\"b\n\\c\u{1}"), "\"a\\\"b\\n\\\\c\\u0001\""); }
    #[test] fn dedup_and_capacity_are_explicit() {
        let root = std::env::temp_dir().join(std::format!("muguet-capture-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let mut state = State::new(root.clone(), 1, 16384);
        let meta = Metadata { stage:"Fragment", entry_point:Some("main"), shader_label:None, layout_report:"[]", sample_mask_report:"[]", retained_mixed:0 };
        state.capture(&[0x07230203, 1, 2], &meta, None).unwrap();
        state.capture(&[0x07230203, 1, 2], &meta, None).unwrap();
        state.capture(&[0x07230203, 1, 3], &meta, None).unwrap();
        assert_eq!(state.seen.len(),1);assert_eq!(state.duplicates,1);assert_eq!(state.missed,1);assert!(state.truncated);
        state.status().unwrap(); let status=fs::read_to_string(root.join("full-capture-status.json")).unwrap();
        assert!(status.contains("\"truncated\":true")); assert!(status.contains("\"missedModules\":1"));
        fs::remove_dir_all(root).unwrap();
    }
    #[test] fn nested_context_restores_first_pipeline_identity() {
        assert!(pipeline_sequence().is_none());
        let outer=pipeline_scope("graphics",Some("outer"));let first=pipeline_sequence();
        { let _inner=pipeline_scope("compute",None);assert_ne!(pipeline_sequence(),first); }
        assert_eq!(pipeline_sequence(),first); drop(outer); assert!(pipeline_sequence().is_none());
    }
}
