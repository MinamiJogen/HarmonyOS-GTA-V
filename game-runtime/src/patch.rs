// muguet - patch.rs
// Constantin Clerc (c22dev) - 0.1

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use wasm_encoder::reencode::{utils, Error, Reencode};
use wasm_encoder::ValType::I64;
use wasmparser::{KnownCustom, Name, Parser, Payload, TypeRef};

#[derive(Clone, Copy, PartialEq)]
enum Arg {
    Local0,
    TlsBase,

    Local1Replaced,

    MissingText,

    TxdLoaded,

    PoolGrow,
    CashWrite,
    OpusFrame,

    ResourceMap,
    VirtualPad,
}

const HOOKS: &[(&str, &str, &[Arg])] = &[
    ("muguet_script", "GtaThread::Update(int)", &[Arg::Local0, Arg::TlsBase]),
    ("muguet_explosion", "CExplosionManager::AddExplosion(CExplosionManager::CExplosionArgs&, CProjectile*, bool)", &[Arg::Local0]),
    ("muguet_timewarp", "CTimeWarper::SetTargetTimeWarp(TW_Dir)", &[Arg::Local1Replaced]),
    ("muguet_text", "CTextFile::GetInternal(unsigned int)", &[Arg::MissingText]),
    ("muguet_txd", "graphics_commands::HasStreamedTxdLoaded(char const*)", &[Arg::TxdLoaded, Arg::TxdLoaded]),
    ("muguet_pool", "rage::sysMemGrowBuddyAllocator::GrowHeap()", &[Arg::PoolGrow]),
    ("muguet_cash", "stats_commands::CommandStatSetInt(int, int, bool)", &[Arg::CashWrite, Arg::CashWrite]),
    ("muguet_opus_frame", "opus_decode_frame", &[Arg::OpusFrame; 6]),
    ("muguet_resource_map", "rage::datResourceInfo::GenerateMap(rage::datResourceMap&) const", &[Arg::ResourceMap]),
    ("muguet_virtual_pad", "rage::ioPad::Update(bool)", &[Arg::VirtualPad]),
];

// Five Star Port's prepare_control_trace.py. The shared imports preserve every
// original result and distinguish layout polls from direct script-wrapper reads.
const TRACE_IMPORTS: &[(&str, usize)] = &[("muguet_control_read", 4), ("muguet_control_script_read", 3)];
const CONTROL_READS: &[(&str, i64, bool)] = &[
    ("pad_commands::CommandIsControlPressed(int, int)",0,false),
    ("pad_commands::CommandGetControlValue(int, int)",1,false),
    ("pad_commands::CommandGetControlNormal(int, int)",2,true),
    ("pad_commands::CommandIsDisabledControlPressed(int, int)",3,false),
    ("pad_commands::CommandGetDisabledControlNormal(int, int)",4,true),
    ("pad_commands::CommandGetControlUnboundNormal(int, int)",2,true),
    ("pad_commands::CommandGetDisabledControlUnboundNormal(int, int)",4,true),
    ("pad_commands::CommandIsControlJustPressed(int, int)",0,false),
    ("pad_commands::CommandIsControlJustReleased(int, int)",0,false),
    ("pad_commands::CommandIsDisabledControlJustPressed(int, int)",3,false),
    ("pad_commands::CommandIsDisabledControlJustReleased(int, int)",3,false),
];
const SCRIPT_READS: &[(&str, i64, bool)] = &[
    ("IS_CONTROL_PRESSED",0,false),("IS_CONTROL_JUST_PRESSED",0,false),
    ("IS_CONTROL_JUST_RELEASED",0,false),("IS_CONTROL_RELEASED",0,false),
    ("GET_CONTROL_VALUE",1,false),("GET_CONTROL_UNBOUND_NORMAL",2,true),
    ("GET_DISABLED_CONTROL_UNBOUND_NORMAL",4,true),
];
const PAD_DEBUG: &str = "rage::ioPad::UpdateDebug()";
const INPUT_EXCLUSIVE: &str = "CControl::DisableInputsByActiveSources(rage::atFixedArray<rage::ioSource, 192> const&, rage::ioValue::DisableOptions const&)";

const SCENARIO_CHECK: &str = "CScenarioManager::PerformCollisionChecksForScenarioPoint(CScenarioManager::CollisionCheckParams const&)";
const MOVIE_PRELOAD: &str = "CMovieMgr::PreloadMovie(char const*, bool)";

pub const VERSION: u32 = 12;

fn returns(args: &[Arg]) -> bool {
    args.contains(&Arg::Local1Replaced) || args.contains(&Arg::MissingText) || args.contains(&Arg::TxdLoaded) || args.contains(&Arg::PoolGrow) || args.contains(&Arg::CashWrite) || args.contains(&Arg::OpusFrame)
}

struct Patcher {
    imported_funcs: u32,
    types: u32,
    tls_global: u32,

    targets: HashMap<u32, usize>,
    scenario_check: Option<u32>,
    movie_preload: Option<u32>,
    pool_size: Option<u32>,
    exports: Vec<(String, u32)>,
    next_func: u32,
    pad_debug: u32,
    input_exclusive: u32,
    control_reads: HashMap<u32, (i64, bool)>,
    script_reads: HashMap<u32, (i64, bool)>,
}

impl Patcher {
    fn shift(&self) -> u32 {
        (HOOKS.len() + TRACE_IMPORTS.len()) as u32
    }
}

impl Reencode for Patcher {
    type Error = anyhow::Error;

    fn function_index(&mut self, f: u32) -> Result<u32, Error<Self::Error>> {
        Ok(if f >= self.imported_funcs { f + self.shift() } else { f })
    }

    fn parse_type_section(&mut self, types: &mut wasm_encoder::TypeSection, section: wasmparser::TypeSectionReader<'_>) -> Result<(), Error<Self::Error>> {
        utils::parse_type_section(self, types, section)?;

        for (_, _, args) in HOOKS {
            types.ty().function(vec![I64; args.len()], if returns(args) { vec![I64] } else { vec![] });
        }
        for (_, count) in TRACE_IMPORTS { types.ty().function(vec![I64; *count], []); }
        Ok(())
    }

    fn parse_import_section(&mut self, imports: &mut wasm_encoder::ImportSection, section: wasmparser::ImportSectionReader<'_>) -> Result<(), Error<Self::Error>> {
        utils::parse_import_section(self, imports, section)?;
        for (k, (name, _, _)) in HOOKS.iter().enumerate() {
            imports.import("env", name, wasm_encoder::EntityType::Function(self.types + k as u32));
        }
        for (k, (name, _)) in TRACE_IMPORTS.iter().enumerate() {
            imports.import("env", name, wasm_encoder::EntityType::Function(self.types + HOOKS.len() as u32 + k as u32));
        }
        Ok(())
    }

    fn parse_export_section(&mut self, exports: &mut wasm_encoder::ExportSection, section: wasmparser::ExportSectionReader<'_>) -> Result<(), Error<Self::Error>> {
        utils::parse_export_section(self, exports, section)?;
        for (name, f) in std::mem::take(&mut self.exports) {
            exports.export(&name, wasm_encoder::ExportKind::Func, f + self.shift());
        }
        Ok(())
    }

    fn parse_function_body(&mut self, code: &mut wasm_encoder::CodeSection, func: wasmparser::FunctionBody<'_>) -> Result<(), Error<Self::Error>> {
        let index = self.next_func;
        self.next_func += 1;
        if let Some(&(kind, float)) = self.control_reads.get(&index) {
            use wasm_encoder::Instruction as I;
            let mut locals = Vec::new(); let mut result_local = 2;
            for local in func.get_locals_reader()? { let (count, ty) = local?; result_local += count; locals.push((count, self.val_type(ty)?)); }
            locals.push((1, if float { wasm_encoder::ValType::F32 } else { wasm_encoder::ValType::I32 }));
            let mut f = wasm_encoder::Function::new(locals);
            let mut reader=func.get_operators_reader()?; let mut body=Vec::new();
            while !reader.eof() { body.push(self.parse_instruction(&mut reader)?); }
            let last=body.len().saturating_sub(1);
            for (position,instruction) in body.iter().enumerate() {
                if matches!(instruction,I::Return) || position==last {
                    f.instruction(&I::LocalTee(result_local)).instruction(&I::I64Const(kind));
                    f.instruction(&I::LocalGet(0)).instruction(&I::I64ExtendI32U);
                    f.instruction(&I::LocalGet(1)).instruction(&I::I64ExtendI32U);
                    f.instruction(&I::LocalGet(result_local));
                    if !float { f.instruction(&I::F32ConvertI32S); }
                    f.instruction(&I::I32ReinterpretF32).instruction(&I::I64ExtendI32U);
                    f.instruction(&I::Call(self.imported_funcs+HOOKS.len() as u32));
                }
                f.instruction(instruction);
            }
            code.function(&f); return Ok(());
        }
        if let Some(&(kind,float))=self.script_reads.get(&index) {
            use wasm_encoder::Instruction as I;
            let mut f=self.new_function_with_parsed_locals(&func)?;
            let mut reader=func.get_operators_reader()?;let mut body=Vec::new();
            while !reader.eof(){body.push(self.parse_instruction(&mut reader)?);}
            let last=body.len().saturating_sub(1);
            for(position,instruction)in body.iter().enumerate(){
                if matches!(instruction,I::Return)||position==last {
                    f.instruction(&I::I64Const(kind)).instruction(&I::LocalGet(0)).instruction(&I::I64Const(float as i64));
                    f.instruction(&I::Call(self.imported_funcs+HOOKS.len() as u32+1));
                }
                f.instruction(instruction);
            }
            code.function(&f);return Ok(());
        }
        if index==self.input_exclusive {
            use wasm_encoder::Instruction as I;
            let mut f=self.new_function_with_parsed_locals(&func)?;
            let mut reader=func.get_operators_reader()?;let mut body=Vec::new();
            while !reader.eof(){body.push(self.parse_instruction(&mut reader)?);}
            let mut changed=false;
            for(position,instruction)in body.iter().enumerate(){
                f.instruction(instruction);
                // Upstream GTA-98 changes the first active-source loop only:
                // (index == -4 || device == IOMS_KEYBOARD) && key == KEY_NULL.
                // This avoids treating every unbound keyboard slot as sharing
                // one key while retaining exclusivity of real keys/pad buttons.
                if !changed && position>=3 && matches!(instruction,I::I32Eq) {
                    if let (I::LocalGet(local),I::I32Load(memarg),I::I32Const(-4),Some(I::If(_)),Some(I::LocalGet(next)),Some(I::I32Load(key)),Some(I::I32Eqz))=
                        (&body[position-3],&body[position-2],&body[position-1],body.get(position+1),body.get(position+2),body.get(position+3),body.get(position+4)) {
                        if local==next&&memarg.offset==8&&key.offset==4&&memarg.memory_index==0&&key.memory_index==0 {
                            let mut device=*memarg;device.offset=0;
                            f.instruction(&I::LocalGet(*local)).instruction(&I::I32Load(device)).instruction(&I::I32Eqz).instruction(&I::I32Or);
                            changed=true;
                        }
                    }
                }
            }
            if !changed{return Err(Error::UserError(anyhow::anyhow!("pinned input-exclusive unbound-key pattern changed")));}
            code.function(&f);return Ok(());
        }
        if Some(index) == self.scenario_check {
            use wasm_encoder::Instruction as I;
            let mut f = self.new_function_with_parsed_locals(&func)?;
            let mut reader = func.get_operators_reader()?;
            while !reader.eof() {
                let i = self.parse_instruction(&mut reader)?;
                f.instruction(if matches!(i, I::I32Const(6)) { &I::I32Const(5) } else { &i });
            }
            code.function(&f);
            return Ok(());
        }
        if Some(index) == self.movie_preload {
            use wasm_encoder::Instruction as I;
            let mut f = self.new_function_with_parsed_locals(&func)?;
            f.instruction(&I::I32Const(0)).instruction(&I::Return);
            let mut reader = func.get_operators_reader()?;
            while !reader.eof() {
                f.instruction(&self.parse_instruction(&mut reader)?);
            }
            code.function(&f);
            return Ok(());
        }
        let Some(&hook) = self.targets.get(&index) else {
            return utils::parse_function_body(self, code, func);
        };
        let mut f = self.new_function_with_parsed_locals(&func)?;
        use wasm_encoder::Instruction as I;
        let call = I::Call(self.imported_funcs + hook as u32);
        if HOOKS[hook].2==[Arg::VirtualPad] {
            let mut reader=func.get_operators_reader()?;let mut found=0;
            while !reader.eof(){
                let instruction=self.parse_instruction(&mut reader)?;
                if matches!(instruction,I::Call(function) if function==self.pad_debug+self.shift()) {
                    f.instruction(&I::LocalGet(0)).instruction(&call);found+=1;
                }
                f.instruction(&instruction);
            }
            if found!=1{return Err(Error::UserError(anyhow::anyhow!("pinned ioPad::UpdateDebug call count changed: {found}")));}
            code.function(&f);return Ok(());
        }
        if HOOKS[hook].2 == [Arg::ResourceMap] {
            let mut reader = func.get_operators_reader()?;
            let mut body = Vec::new();
            while !reader.eof() {
                body.push(self.parse_instruction(&mut reader)?);
            }
            let end = body.pop();
            for i in &body {
                f.instruction(i);
            }
            f.instruction(&I::LocalGet(1)).instruction(&call);
            if let Some(e) = end {
                f.instruction(&e);
            }
            code.function(&f);
            return Ok(());
        }
        if HOOKS[hook].2 == [Arg::PoolGrow] {
            let Some(size) = self.pool_size else { return Err(Error::UserError(anyhow::anyhow!("GetHeapSize not found"))) };
            f.instruction(&I::LocalGet(0)).instruction(&I::Call(size + self.shift())).instruction(&call).instruction(&I::I64Eqz);
            f.instruction(&I::If(wasm_encoder::BlockType::Empty));
            f.instruction(&I::I32Const(-1)).instruction(&I::Return);
            f.instruction(&I::End);
        } else if HOOKS[hook].2.contains(&Arg::OpusFrame) {
            for (k, wide) in [true, true, false, true, false, false].into_iter().enumerate() {
                f.instruction(&I::LocalGet(k as u32));
                if !wide { f.instruction(&I::I64ExtendI32S); }
            }
            f.instruction(&call).instruction(&I::I32WrapI64).instruction(&I::Return);
        } else if HOOKS[hook].2 == [Arg::MissingText] {
            // only call out for the listed hashes: text lookups are hot
            for (label, _) in crate::host::MISSING_TEXT {
                f.instruction(&I::LocalGet(0)).instruction(&I::I32Const(crate::proto::joaat(label) as i32)).instruction(&I::I32Eq);
                f.instruction(&I::If(wasm_encoder::BlockType::Empty));
                f.instruction(&I::LocalGet(0)).instruction(&I::I64ExtendI32U).instruction(&call).instruction(&I::Return);
                f.instruction(&I::End);
            }
        } else if HOOKS[hook].2.first() == Some(&Arg::CashWrite) {
            for k in 0..3 {
                f.instruction(&I::LocalGet(0)).instruction(&I::I32Const(crate::proto::joaat(&format!("SP{k}_TOTAL_CASH")) as i32)).instruction(&I::I32Eq);
                f.instruction(&I::If(wasm_encoder::BlockType::Empty));
                f.instruction(&I::LocalGet(1)).instruction(&I::I64ExtendI32S).instruction(&I::GlobalGet(self.tls_global)).instruction(&call).instruction(&I::I64Eqz).instruction(&I::I32Eqz);
                f.instruction(&I::If(wasm_encoder::BlockType::Empty)).instruction(&I::I32Const(1)).instruction(&I::Return).instruction(&I::End);
                f.instruction(&I::End);
            }
        } else if HOOKS[hook].2.first() == Some(&Arg::TxdLoaded) {
            f.instruction(&I::LocalGet(0)).instruction(&I::I64Const(0)).instruction(&call).instruction(&I::Drop);
            let mut reader = func.get_operators_reader()?;
            let mut last = None;
            while !reader.eof() {
                let i = self.parse_instruction(&mut reader)?;
                if let (I::Return, Some(I::LocalGet(2))) = (&i, &last) {
                    f.instruction(&I::Drop);
                    f.instruction(&I::I64Const(0)).instruction(&I::LocalGet(2)).instruction(&I::I64ExtendI32U).instruction(&I::I64Const(1)).instruction(&I::I64Add);
                    f.instruction(&call).instruction(&I::I32WrapI64);
                }
                f.instruction(&i);
                last = Some(i);
            }
            code.function(&f);
            return Ok(());
        } else {
            for arg in HOOKS[hook].2 {
                match arg {
                    Arg::Local0 => f.instruction(&I::LocalGet(0)),
                    Arg::TlsBase => f.instruction(&I::GlobalGet(self.tls_global)),
                    Arg::Local1Replaced => f.instruction(&I::LocalGet(1)).instruction(&I::I64ExtendI32U),
                    Arg::MissingText | Arg::TxdLoaded | Arg::PoolGrow | Arg::CashWrite | Arg::OpusFrame | Arg::ResourceMap | Arg::VirtualPad => unreachable!(),
                };
            }
            f.instruction(&call);
            if returns(HOOKS[hook].2) {
                f.instruction(&I::I32WrapI64);
                f.instruction(&I::LocalSet(1));
            }
        }
        let mut reader = func.get_operators_reader()?;
        while !reader.eof() {
            f.instruction(&self.parse_instruction(&mut reader)?);
        }
        code.function(&f);
        Ok(())
    }
}

pub fn is_patched(wasm: &[u8]) -> bool {
    Parser::new(0).parse_all(wasm).any(|p| match p {
        Ok(Payload::ImportSection(s)) => s.into_imports().any(|i| i.is_ok_and(|i| i.name == HOOKS[0].0)),
        _ => false,
    })
}

pub fn has_fivestar_hooks(wasm: &[u8]) -> bool {
    let mut found=[false;3];
    for payload in Parser::new(0).parse_all(wasm) {
        if let Ok(Payload::ImportSection(imports))=payload {
            for import in imports.into_imports().flatten() {
                if import.module!="env" {continue;}
                for(index,name)in ["muguet_virtual_pad","muguet_control_read","muguet_control_script_read"].iter().enumerate() {
                    if import.name==*name {found[index]=true;}
                }
            }
        }
    }
    found.into_iter().all(|found|found)
}

pub fn patch(wasm: &[u8], wanted: &[(&str, &str)]) -> Result<Vec<u8>> {
    // Import names are authoritative. Newly injected imports need not have
    // entries in the optional Wasm name section.
    if is_patched(wasm) { bail!("game.wasm is already patched; use the original pinned game.wasm"); }
    let (mut imported_funcs, mut types, mut tls_global) = (0u32, 0u32, None);
    let mut by_name: HashMap<String, u32> = HashMap::new();
    for payload in Parser::new(0).parse_all(wasm) {
        match payload? {
            Payload::TypeSection(s) => types = s.count(),
            Payload::ImportSection(s) => {
                for imp in s.into_imports() {
                    if matches!(imp?.ty, TypeRef::Func(_)) {
                        imported_funcs += 1;
                    }
                }
            }
            Payload::CustomSection(c) => {
                if let KnownCustom::Name(names) = c.as_known() {
                    for sub in names {
                        match sub? {
                            Name::Function(map) => {
                                for n in map {
                                    let n = n?;
                                    by_name.entry(n.name.to_string()).or_insert(n.index);
                                }
                            }
                            Name::Global(map) => {
                                for n in map {
                                    let n = n?;
                                    if n.name == "__tls_base" { tls_global = Some(n.index); }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if by_name.contains_key(HOOKS[0].0) {
        bail!("game.wasm is already patched");
    }
    let mut targets = HashMap::new();
    for (k, (_, func, _)) in HOOKS.iter().enumerate() {
        let f = *by_name.get(*func).with_context(|| format!("{func} not found (no name section?)"))?;
        eprintln!("[patch] required hook {} -> function {f}: {func}", HOOKS[k].0);
        targets.insert(f, k);
    }
    let mut exports = Vec::new();
    let required = |name:&str| by_name.get(name).copied().with_context(||format!("required Five Star input symbol missing: {name}"));
    let mut control_reads=HashMap::new();
    for (name,kind,float)in CONTROL_READS {control_reads.insert(required(name)?,(*kind,*float));}
    let mut script_reads=HashMap::new();
    for (name,kind,float)in SCRIPT_READS {
        script_reads.insert(required(&format!("pad_commands::SetupScriptCommands()::scrWrapped_{name}::Call(rage::scrThread::Info&)"))?,(*kind,*float));
    }
    let pad_debug=required(PAD_DEBUG)?;let input_exclusive=required(INPUT_EXCLUSIVE)?;
    if let Some(&malloc) = by_name.get("emscripten_builtin_malloc") {
        exports.push(("muguet:malloc".to_string(), malloc));
    }
    for (export, func) in wanted {
        match by_name.get(*func) {
            Some(&i) => exports.push((format!("mp:{export}"), i)),
            None => eprintln!("[patch] {func} not found; multiplayer will skip it"),
        }
    }
    anyhow::ensure!(tls_global.is_some(), "required __tls_base global missing");
    anyhow::ensure!(by_name.contains_key("emscripten_builtin_malloc"), "required emscripten_builtin_malloc missing");
    eprintln!("[patch] tls={:?}, scenario={:?}, movie_preload={:?}, pool_size={:?}, command_exports={}",tls_global,by_name.get(SCENARIO_CHECK),by_name.get(MOVIE_PRELOAD),by_name.get("rage::sysMemGrowBuddyAllocator::GetHeapSize() const"),exports.len());
    let mut p = Patcher {
        imported_funcs,
        types,
        tls_global: tls_global.unwrap_or(1),
        targets,
        scenario_check: by_name.get(SCENARIO_CHECK).copied(),
        movie_preload: by_name.get(MOVIE_PRELOAD).copied(),
        pool_size: by_name.get("rage::sysMemGrowBuddyAllocator::GetHeapSize() const").copied(),
        exports,
        next_func: imported_funcs,
        pad_debug,input_exclusive,control_reads,script_reads,
    };
    let mut module = wasm_encoder::Module::new();
    p.parse_core_module(&mut module, Parser::new(0), wasm).map_err(|e| anyhow::anyhow!("patch game.wasm: {e:?}"))?;
    Ok(module.finish())
}

#[cfg(test)]
mod fivestar_tests {
    use super::*;
    fn pinned_wasm()->Option<Vec<u8>> {
        let path=std::env::var_os("MG_PINNED_GAME_WASM")?;
        Some(std::fs::read(path).expect("MG_PINNED_GAME_WASM must be readable"))
    }
    fn names(wasm:&[u8])->HashMap<String,u32>{
        let mut names=HashMap::new();
        for payload in Parser::new(0).parse_all(wasm){
            if let Payload::CustomSection(c)=payload.unwrap(){
                if let KnownCustom::Name(map)=c.as_known(){
                    for item in map{if let Name::Function(map)=item.unwrap(){
                        for item in map{let item=item.unwrap();names.insert(item.name.into(),item.index);}
                    }}
                }
            }
        }
        names
    }
    #[test]
    fn pinned_wasm_all_input_hooks_validate_and_preserve_typed_returns(){
        let Some(original)=pinned_wasm()else{return};
        let patched=patch(&original,&[]).unwrap();
        wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::all())
            .validate_all(&patched).expect("all rewritten function types, branches and indices must validate");
        let names=names(&patched);let pad=names["rage::ioPad::Update(bool)"];
        let mut imports=HashMap::new();let mut count=0;
        for payload in Parser::new(0).parse_all(&patched){if let Payload::ImportSection(s)=payload.unwrap(){
            for import in s.into_imports(){let import=import.unwrap();if matches!(import.ty,TypeRef::Func(_)){
                imports.insert(import.name.to_owned(),count);count+=1;
            }}
        }}
        for name in ["muguet_virtual_pad","muguet_control_read","muguet_control_script_read"]{assert!(imports.contains_key(name));}
        let mut io_hook=0;let mut exclusive_or=0;let mut traced=0;
        for payload in Parser::new(0).parse_all(&patched){if let Payload::CodeSectionEntry(body)=payload.unwrap(){
            let mut operators=body.get_operators_reader().unwrap();let mut calls=Vec::new();
            while !operators.eof(){match operators.read().unwrap(){
                wasmparser::Operator::Call{function_index}=>calls.push(function_index),
                wasmparser::Operator::I32Or if count==names[INPUT_EXCLUSIVE]=>exclusive_or+=1,
                _=>{}
            }}
            if count==pad{
                io_hook=calls.iter().filter(|f|**f==imports["muguet_virtual_pad"]).count();
                let hook=calls.iter().position(|f|*f==imports["muguet_virtual_pad"]).unwrap();
                assert_eq!(calls[hook+1],names[PAD_DEBUG],"pad sample is applied before UpdateDebug");
            }
            if CONTROL_READS.iter().any(|(name,_,_)|names[*name]==count){
                assert!(calls.contains(&imports["muguet_control_read"]));traced+=1;
            }
            if SCRIPT_READS.iter().any(|(name,_,_)|names[&format!("pad_commands::SetupScriptCommands()::scrWrapped_{name}::Call(rage::scrThread::Info&)")]==count){
                assert!(calls.contains(&imports["muguet_control_script_read"]));traced+=1;
            }
            count+=1;
        }}
        assert_eq!(io_hook,1);assert!(exclusive_or>=1);assert_eq!(traced,18);
        assert!(is_patched(&patched));assert!(patch(&patched,&[]).is_err());
    }
    #[test]
    fn pinned_wasm_missing_io_pad_symbol_fails_closed(){
        let Some(mut original)=pinned_wasm()else{return};
        let name=b"rage::ioPad::Update(bool)";
        let at=original.windows(name.len()).position(|part|part==name).unwrap();
        original[at+9]=b'X';
        assert!(format!("{:#}",patch(&original,&[]).unwrap_err()).contains("ioPad::Update"));
    }
}
