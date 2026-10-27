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
use sha2::{Digest,Sha256};
use std::{io::Write,path::{Path,PathBuf}};

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
enum IsaProfile {Generic,Lse}
impl IsaProfile {
    fn parse(value:&str)->anyhow::Result<Self>{match value {"generic"=>Ok(Self::Generic),"lse"=>Ok(Self::Lse),_=>anyhow::bail!("ISA profile must be generic or lse; requested={value}")}}
    fn name(self)->&'static str {match self{Self::Generic=>"generic",Self::Lse=>"lse"}}
    fn lse(self)->bool {self==Self::Lse}
}
fn target_config(profile:IsaProfile)->anyhow::Result<wasmtime::Config> {
    let mut cfg=wasmtime::Config::new();
    cfg.target("aarch64-unknown-linux-ohos")?;
    cfg.wasm_memory64(true).wasm_threads(true).shared_memory(true).wasm_bulk_memory(true).wasm_multi_memory(true);
    cfg.memory_reservation(5u64<<30).memory_guard_size(64<<10).memory_may_move(false);
    cfg.cranelift_opt_level(wasmtime::OptLevel::Speed);
    unsafe {
        // This is the only profile-dependent code generation setting. The
        // generic artifact remains usable on baseline ARM64; LSE is opt-in.
        cfg.cranelift_flag_set("has_lse",if profile.lse(){"true"}else{"false"});
        cfg.cranelift_flag_set("enable_heap_access_spectre_mitigation","false");
        cfg.cranelift_flag_set("enable_table_access_spectre_mitigation","false");
    }
    Ok(cfg)
}
fn write_new(path:&Path,bytes:&[u8])->anyhow::Result<()> {
    let mut file=std::fs::OpenOptions::new().write(true).create_new(true).open(path)
        .map_err(|error|anyhow::anyhow!("refusing to overwrite candidate output {}: {error}",path.display()))?;
    file.write_all(bytes)?;file.sync_all()?;Ok(())
}
fn metadata(wasm:&[u8],artifact:&[u8],path:&Path,profile:IsaProfile)->anyhow::Result<()> {
    let mut imports=Vec::new();let mut memories=Vec::new();let mut exports=Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(wasm) { match payload? {
        wasmparser::Payload::ImportSection(section)=>for import in section.into_imports(){let imp=import?;imports.push(format!("{{\"module\":{},\"name\":{},\"type\":{}}}",json_string(imp.module),json_string(imp.name),json_string(&format!("{:?}",imp.ty))));if let wasmparser::TypeRef::Memory(m)=imp.ty{memories.push(format!("{{\"import\":true,\"minimumPages\":{},\"maximumPages\":{},\"memory64\":{},\"shared\":{},\"pageSizeLog2\":{}}}",m.initial,m.maximum.map(|v|v.to_string()).unwrap_or("null".into()),m.memory64,m.shared,m.page_size_log2.unwrap_or(16)));}},
        wasmparser::Payload::ExportSection(section)=>for export in section{let ex=export?;exports.push(json_string(ex.name));},
        _=>{}
    }}
    let value=format!("{{\"schemaVersion\":2,\"target\":\"aarch64-unknown-linux-ohos\",\"wasmtime\":\"49.0.0\",\"cranelift\":\"0.136.0\",\"isaProfile\":{},\"requestedIsaFlags\":{{\"has_lse\":{}}},\"requiredCpuFeatures\":[{}],\"memoryReservationBytes\":5368709120,\"memoryGuardBytes\":65536,\"memoryMayMove\":false,\"optimization\":\"Speed\",\"patchVersion\":{},\"patchedBytes\":{},\"patchedWasmSha256\":\"{:x}\",\"artifactBytes\":{},\"artifactSha256\":\"{:x}\",\"performanceGainEstablished\":false,\"imports\":[{}],\"memories\":[{}],\"exports\":[{}]}}\n",json_string(profile.name()),profile.lse(),if profile.lse(){"\"FEAT_LSE\""}else{""},patch::VERSION,wasm.len(),Sha256::digest(wasm),artifact.len(),Sha256::digest(artifact),imports.join(","),memories.join(","),exports.join(","));
    write_new(path,value.as_bytes())
}
fn main()->anyhow::Result<()> {
    let mut positional=Vec::new();let mut profile=IsaProfile::Generic;let mut args=std::env::args().skip(1);
    while let Some(arg)=args.next() {
        if arg=="--isa-profile" {profile=IsaProfile::parse(&args.next().ok_or_else(||anyhow::anyhow!("missing --isa-profile value"))?)?;}
        else if let Some(value)=arg.strip_prefix("--isa-profile="){profile=IsaProfile::parse(value)?;}
        else {anyhow::ensure!(!arg.starts_with("--"),"unknown option {arg}");positional.push(PathBuf::from(arg));}
    }
    anyhow::ensure!((2..=3).contains(&positional.len()),"usage: <game.wasm> <new-output.cwasm> [new-patched.wasm] [--isa-profile generic|lse]");
    let input=&positional[0];let output=&positional[1];let metadata_path=output.with_extension("metadata.json");
    for path in std::iter::once(output).chain(positional.get(2)).chain(std::iter::once(&metadata_path)) {
        anyhow::ensure!(!path.exists(),"refusing to overwrite existing output {}",path.display());
    }
    let wasm=std::fs::read(input)?;let now=std::time::Instant::now();
    let patched=if patch::is_patched(&wasm){
        anyhow::ensure!(patch::has_fivestar_hooks(&wasm),"input is an older patched game without Five Star ioPad/control hooks; rebuild from the original game.wasm");
        wasm
    }else{patch::patch(&wasm,commands::COMMANDS)?};
    eprintln!("[aot] patched {} bytes in {:?}; isa_profile={} has_lse={}",patched.len(),now.elapsed(),profile.name(),profile.lse());
    let engine=wasmtime::Engine::new(&target_config(profile)?)?;
    let artifact=engine.precompile_module(&patched)?;
    // Publish distinct immutable candidate files. Never rewrite the public
    // generic artifact or the old shared metadata/log paths as a side effect.
    write_new(output,&artifact)?;
    if let Some(path)=positional.get(2){write_new(path,&patched)?;}
    metadata(&patched,&artifact,&metadata_path,profile)?;
    eprintln!("[aot] OHOS game artifact {} bytes, sha256={:x}, metadata={}, total {:?}",artifact.len(),Sha256::digest(&artifact),metadata_path.display(),now.elapsed());Ok(())
}

#[cfg(test)]
mod isa_tests {
    use super::*;
    #[test]
    fn profile_is_explicit_and_unknown_features_never_enable() {
        assert_eq!(IsaProfile::parse("generic").unwrap(),IsaProfile::Generic);
        assert_eq!(IsaProfile::parse("lse").unwrap(),IsaProfile::Lse);
        for value in ["native","all","true","LSE","dotprod",""]{assert!(IsaProfile::parse(value).is_err());}
        assert!(!IsaProfile::Generic.lse());assert!(IsaProfile::Lse.lse());
    }
    #[cfg(all(target_arch="aarch64",target_os="macos"))]
    #[test]
    fn runtime_compatibility_rejects_false_or_unknown_lse_and_keeps_atomic_traps() {
        let wasm=atomic_fixture();
        let mut generic=target_config(IsaProfile::Generic).unwrap();generic.target("aarch64-apple-darwin").unwrap();
        // Negative probe only; this never lies that unsupported instructions
        // are available. No LSE instructions are executed in this test.
        unsafe{generic.detect_host_feature(|_|Some(false));}
        let engine=wasmtime::Engine::new(&generic).unwrap();let artifact=engine.precompile_module(&wasm).unwrap();
        let module=unsafe{wasmtime::Module::deserialize(&engine,&artifact)}.unwrap();
        let mut store=wasmtime::Store::new(&engine,());let instance=wasmtime::Instance::new(&mut store,&module,&[]).unwrap();
        let add=instance.get_typed_func::<(i64,i32),i32>(&mut store,"atomic_add").unwrap();
        let cas=instance.get_typed_func::<(i64,i32,i32),i32>(&mut store,"atomic_cas").unwrap();
        assert_eq!(add.call(&mut store,(0,1)).unwrap(),0);assert_eq!(add.call(&mut store,(0,3)).unwrap(),1);
        assert_eq!(cas.call(&mut store,(0,4,9)).unwrap(),4);assert_eq!(cas.call(&mut store,(0,4,12)).unwrap(),9);
        assert!(add.call(&mut store,(1,1)).is_err());assert!(add.call(&mut store,(65536,1)).is_err());
        let mut lse=target_config(IsaProfile::Lse).unwrap();lse.target("aarch64-apple-darwin").unwrap();
        let specialized=wasmtime::Engine::new(&lse).unwrap().precompile_module(&wasm).unwrap();
        let error=unsafe{wasmtime::Module::deserialize(&engine,&specialized)}.unwrap_err();
        let error=format!("{error:#}");assert!(error.contains("has_lse")&&error.contains("not available"),"{error}");
        unsafe{generic.detect_host_feature(|_|None);}
        let unknown=wasmtime::Engine::new(&generic).unwrap();
        let error=unsafe{wasmtime::Module::deserialize(&unknown,&specialized)}.unwrap_err();
        let error=format!("{error:#}");assert!(error.contains("has_lse")&&error.contains("failed to detect"),"{error}");
    }
    #[test]
    fn candidate_outputs_never_clobber_existing_bytes() {
        let path=std::env::temp_dir().join(format!("aot-preserve-{}",std::process::id()));
        write_new(&path,b"preserve").unwrap();assert!(write_new(&path,b"replaced").is_err());
        assert_eq!(std::fs::read(&path).unwrap(),b"preserve");std::fs::remove_file(path).unwrap();
    }
    fn atomic_fixture()->Vec<u8> {
        use wasm_encoder::*;
    let mut module=Module::new();let mut types=TypeSection::new();
    types.ty().function([ValType::I64,ValType::I32],[ValType::I32]);
    types.ty().function([ValType::I64,ValType::I32,ValType::I32],[ValType::I32]);module.section(&types);
    let mut functions=FunctionSection::new();functions.function(0).function(1);module.section(&functions);
    let mut memory=MemorySection::new();memory.memory(MemoryType {minimum:1,maximum:Some(1),memory64:true,shared:true,page_size_log2:None});module.section(&memory);
    let mut exports=ExportSection::new();exports.export("atomic_add",ExportKind::Func,0).export("atomic_cas",ExportKind::Func,1);module.section(&exports);
    let address=MemArg {offset:0,align:2,memory_index:0};let mut code=CodeSection::new();
    let mut add=Function::new([]);add.instruction(&Instruction::LocalGet(0)).instruction(&Instruction::LocalGet(1)).instruction(&Instruction::I32AtomicRmwAdd(address)).instruction(&Instruction::End);code.function(&add);
    let mut cas=Function::new([]);cas.instruction(&Instruction::LocalGet(0)).instruction(&Instruction::LocalGet(1)).instruction(&Instruction::LocalGet(2)).instruction(&Instruction::I32AtomicRmwCmpxchg(address)).instruction(&Instruction::End);code.function(&cas);module.section(&code);
        module.finish()
    }
    #[test]
    fn atomic_codegen_profiles_change_real_instructions() {
        let directory=std::env::var_os("MG_AOT_TEST_ARTIFACT_DIR").map(PathBuf::from)
            .unwrap_or_else(||std::env::temp_dir().join(format!("aot-isa-fixture-{}",std::process::id())));
        std::fs::create_dir_all(&directory).unwrap();
        let wasm=atomic_fixture();
write_new(&directory.join("atomic-fixture.wasm"),&wasm).unwrap();
        let objdump=std::env::var_os("MG_AOT_OBJDUMP").map(PathBuf::from).unwrap_or_else(||PathBuf::from("/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native/llvm/bin/llvm-objdump"));
        for profile in [IsaProfile::Generic,IsaProfile::Lse] {
            let engine=wasmtime::Engine::new(&target_config(profile).unwrap()).unwrap();let artifact=engine.precompile_module(&wasm).unwrap();
            let path=directory.join(format!("atomic-{}.cwasm",profile.name()));write_new(&path,&artifact).unwrap();
            let output=std::process::Command::new(&objdump).args(["-d","--no-show-raw-insn","--mattr=+lse"]).arg(&path).output().unwrap();
            assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));let disassembly=String::from_utf8(output.stdout).unwrap();
            write_new(&directory.join(format!("atomic-{}.txt",profile.name())),disassembly.as_bytes()).unwrap();
            if profile.lse(){assert!(disassembly.contains("ldaddal"),"{disassembly}");assert!(disassembly.contains("casal"),"{disassembly}");}
            else {assert!(disassembly.contains("ldaxr"),"{disassembly}");assert!(disassembly.contains("stlxr"),"{disassembly}");assert!(!disassembly.contains("ldaddal"));assert!(!disassembly.contains("casal"));}
            eprintln!("[aot-test] {} fixture SHA256={:x}",profile.name(),Sha256::digest(&artifact));
        }
    }
}
