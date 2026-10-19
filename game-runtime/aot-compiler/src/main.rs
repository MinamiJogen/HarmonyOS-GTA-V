mod host {
pub const MISSING_TEXT: &[(&str, &str)] = &[
    ("HELP_DCK", "~s~Hold ~INPUT_VEH_DUCK~ to duck when sitting inside a vehicle."),
    ("HELP_KM", "~s~Press ~INPUT_CONTEXT~ to use the camera in your glasses, then ~INPUT_ATTACK~ to take a photo."),
];}
mod proto {
pub fn joaat(name: &str) -> u32 {
    let mut h: u32 = 0;
    for c in name.bytes() {
        h = h.wrapping_add(c.to_ascii_lowercase() as u32);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}
}
#[path = "../../src/patch.rs"] mod patch;
mod commands;
fn json_string(s: &str) -> String { format!("\"{}\"",s.replace('\\',"\\\\").replace('\"',"\\\"").replace('\n',"\\n").replace('\r',"\\r").replace('\t',"\\t")) }
fn metadata(wasm:&[u8],path:&str)->anyhow::Result<()> {
    let mut imports=Vec::new();let mut memories=Vec::new();let mut exports=Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(wasm) { match payload? {
        wasmparser::Payload::ImportSection(section)=>for import in section.into_imports(){let imp=import?;imports.push(format!("{{\"module\":{},\"name\":{},\"type\":{}}}",json_string(imp.module),json_string(imp.name),json_string(&format!("{:?}",imp.ty))));if let wasmparser::TypeRef::Memory(m)=imp.ty{memories.push(format!("{{\"import\":true,\"minimumPages\":{},\"maximumPages\":{},\"memory64\":{},\"shared\":{},\"pageSizeLog2\":{}}}",m.initial,m.maximum.map(|v|v.to_string()).unwrap_or("null".into()),m.memory64,m.shared,m.page_size_log2.unwrap_or(16)));}},
        wasmparser::Payload::ExportSection(section)=>for export in section{let ex=export?;exports.push(json_string(ex.name));},
        _=>{}
    }}
    std::fs::write(path,format!("{{\"target\":\"aarch64-unknown-linux-ohos\",\"wasmtime\":\"49.0.0\",\"patchVersion\":{},\"patchedBytes\":{},\"imports\":[{}],\"memories\":[{}],\"exports\":[{}]}}",patch::VERSION,wasm.len(),imports.join(","),memories.join(","),exports.join(",")))?;Ok(())
}
fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let input = args.get(1).ok_or_else(|| anyhow::anyhow!("usage: <game.wasm> <game.cwasm> [patched.wasm]"))?;
    let output = args.get(2).ok_or_else(|| anyhow::anyhow!("missing output"))?;
    let wasm = std::fs::read(input)?;
    let now = std::time::Instant::now();
    let patched = if patch::is_patched(&wasm) { wasm } else { patch::patch(&wasm, commands::COMMANDS)? };
    metadata(&patched,"logs/game-wasm-metadata.json")?;
    eprintln!("[aot] patched {} bytes in {:?}", patched.len(), now.elapsed());
    if let Some(path) = args.get(3) { std::fs::write(path, &patched)?; }
    let mut cfg = wasmtime::Config::new();
    cfg.target("aarch64-unknown-linux-ohos")?;
    cfg.wasm_memory64(true).wasm_threads(true).shared_memory(true).wasm_bulk_memory(true).wasm_multi_memory(true);
    cfg.memory_reservation(5u64 << 30).memory_guard_size(64 << 10).memory_may_move(false);
    cfg.cranelift_opt_level(wasmtime::OptLevel::Speed);
    unsafe {
        cfg.cranelift_flag_set("enable_heap_access_spectre_mitigation", "false");
        cfg.cranelift_flag_set("enable_table_access_spectre_mitigation", "false");
    }
    let engine = wasmtime::Engine::new(&cfg)?;
    let artifact = engine.precompile_module(&patched)?;
    std::fs::write(output, &artifact)?;
    eprintln!("[aot] OHOS game artifact {} bytes, total {:?}", artifact.len(), now.elapsed());
    Ok(())
}
