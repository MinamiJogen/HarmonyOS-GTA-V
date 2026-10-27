#!/usr/bin/env python3
"""Exercise production PSO recipes and replay with real wgpu NOOP validation.

Extracts the renderer's recipe, descriptor creation, lookup, bind-layout and
compile methods without copying their implementations. Fixture Programs use
small WGSL modules; source loading and shader transformations are fixtures,
not coverage claims. Also executes the actual loading-stage replay method;
its only source edit permits its OHOS gate on the host. Build digests are fixed
fixture identities. The NOOP backend validates CPU descriptors and commands;
it does not prove device pixels, Vulkan driver behavior or shorter stalls.
"""
from pathlib import Path
import json
import os
import platform
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]


def block(source: str, marker: str) -> str:
    start = source.index(marker)
    cursor = source.index("{", start) + 1
    depth = 1
    while depth:
        depth += (source[cursor] == "{") - (source[cursor] == "}")
        cursor += 1
    return source[start:cursor]


HARNESS = r'''
#![allow(dead_code, unused_imports)]
use std::{cell::Cell, collections::{BTreeMap, HashMap, HashSet}, rc::Rc,
    sync::{atomic::{AtomicBool, Ordering}, mpsc}, time::{Duration, Instant}};
use wgpu::*;
use pso_warmup::{Recipe as PsoRecipe, Shader as PsoShader};
type FxHashMap<K,V> = HashMap<K,V>;
const COMPACT_VERTEX_BINDINGS: bool = true;
const PIPELINE_WAIT: Duration = Duration::ZERO;
static SUSPENDED: AtomicBool = AtomicBool::new(false);
fn pipeline_build_digests()->(String,String) { ("fixture-runtime-build".into(),"fixture-wgpu-build".into()) }
mod slow_trace { pub fn span(_: &str, _: &str) {} }
mod cpu_profile {
    pub enum CpuMetric { PipelineCreate, ScopePop }
    pub fn span(_: CpuMetric) {}
}
use cpu_profile::CpuMetric;
fn log(message: &str) { eprintln!("{message}"); }
fn log_once(_: &str, message: &str) { log(message); }
fn error_chain(error: &wgpu::Error) -> String { error.to_string() }

#[derive(Clone, Copy)] struct SigEl { sem:u32, idx:u32, reg:u32 }
#[derive(Clone, Copy)] struct El { sem:u32, idx:u32, fmt:u32, slot:u32, offset:u32, cls:u32 }
struct LayoutObj { els:Vec<El>, slots:Vec<(u32,u32)> }
struct StateObj { words:Vec<u32>, sampler:Option<Sampler> }
#[derive(Clone, Copy, PartialEq)] enum Kind { Uniform, Storage, ReadOnlyStorage, Sampler, SamplerCmp, Texture, StorageTexture }
#[derive(Clone, Copy)] enum Sample { Float, Unfilterable, Uint, Sint, Depth }
struct Binding {
    group:u32, binding:u32, kind:Kind, sample:Sample, dim:TextureViewDimension,
    ms:bool, access:StorageTextureAccess, format:Option<TextureFormat>,
}
struct Info { bindings:Vec<Binding>, outputs:Vec<u32> }
struct Program { uid:u64, source:PsoShader, module:ShaderModule, info:Info, flat:Vec<u32>, label:String }
#[derive(Clone, Default)] struct Vb { buf:u32, stride:u32 }
struct Buf { gpu:Option<Buffer> }
struct State { topo:u32, layout:u32, vb:Vec<Vb>, ib:(u32,u32,u32), blend:(u32,[f64;4]), dss:(u32,u32), rs:u32 }
PRODUCTION_TYPES

struct Gpu {
    device:Device, queue:Queue, adapter:Adapter, pipeline_cache:Option<PipelineCache>,
    s:State, pass_info:Option<PassInfo>, signature:Vec<SigEl>,
    states:HashMap<u32,Rc<StateObj>>, buffers:HashMap<u32,Buf>,
    unfilt:HashMap<u64,(u128,u128)>, pipelines:FxHashMap<Vec<i64>,PipeEntry>,
    warm_pipelines:HashMap<PsoRecipe,Rc<Pl>>, pso_store:Option<pso_warmup::Store>, pso_stats:PsoStats,
    bgl_cache:FxHashMap<(u64,u128,u128,u32,u32),(BindGroupLayout,u64)>, empty_bgl:Option<(BindGroupLayout,u64)>,
    pipe_creates:u64, pipe_jobs:Option<mpsc::Sender<(u64,PipeJob)>>,
    pipe_results:mpsc::Receiver<(u64,Option<RenderPipeline>)>,
    pipe_pending:HashMap<u64,(Vec<i64>,Vec<VbReq>,vertex_bindings::Plan,String,Option<Rc<PsoRecipe>>)>,
    pending_skips:u64, pipe_wait_left:Duration, pdirty:bool, next_uid:u64,
    fixtures:HashMap<(String,u32,bool),Rc<Program>>,
    shader_index:HashMap<String,serde_json::Value>, shader_dir:std::path::PathBuf,
}

impl Gpu {
    fn new() -> Self {
        let mut options=InstanceDescriptor::new_without_display_handle();
        options.backends=Backends::NOOP;
        options.backend_options.noop.enable=true;
        let instance=Instance::new(options);
        let adapter=pollster::block_on(instance.request_adapter(&RequestAdapterOptions::default())).unwrap();
        assert_eq!(adapter.get_info().backend, Backend::Noop);
        let (device,queue)=pollster::block_on(adapter.request_device(&DeviceDescriptor {
            required_features:Features::empty(), required_limits:Limits::default(), ..Default::default()
        })).unwrap();
        let (_,rx)=mpsc::channel();
        let mut g=Self { device,queue,adapter,pipeline_cache:None,
            s:State { topo:4,layout:100,vb:vec![Vb::default();16],ib:(0,0,0),blend:(0,[1.;4]),dss:(0,0),rs:0 },
            pass_info:Some(PassInfo { rt_formats:vec![Some(TextureFormat::Rgba8Unorm)],
                samples:1,ds_format:None,ro_depth:false,ro_stencil:false,width:16,height:16 }),
            signature:vec![SigEl { sem:1,idx:0,reg:0 }], states:HashMap::new(),buffers:HashMap::new(),
            unfilt:HashMap::new(), pipelines:HashMap::new(),warm_pipelines:HashMap::new(),
            pso_store:Some(pso_warmup::Store::new(std::path::Path::new("/unused"), "test".into(),true)),
            pso_stats:PsoStats::default(),bgl_cache:HashMap::new(),empty_bgl:None,pipe_creates:0,
            pipe_jobs:None,pipe_results:rx,pipe_pending:HashMap::new(),pending_skips:0,
            pipe_wait_left:Duration::ZERO,pdirty:false,next_uid:1000,fixtures:HashMap::new(),
            shader_index:HashMap::new(),shader_dir:std::path::PathBuf::new() };
        g.buffer(12,16);
        g
    }
    fn uid(&mut self)->u64 { self.next_uid+=1; self.next_uid }
    fn state(&self,id:u32)->Option<Rc<StateObj>> { self.states.get(&id).cloned() }
    fn buf(&self,id:u32)->Option<&Buf> { self.buffers.get(&id) }
    fn vs_sig(&self)->Option<Vec<SigEl>> { Some(self.signature.clone()) }
    fn note_pipeline_cache_change(&self) {}
    fn load_map_program(&mut self,hash:&str,stage:u32,pause:bool)->Option<Rc<Program>> {
        self.fixtures.get(&(hash.into(),stage,pause)).cloned()
    }
    fn buffer(&mut self,id:u32,stride:u32) {
        let buffer=self.device.create_buffer(&BufferDescriptor { label:None,size:1024,
            usage:BufferUsages::VERTEX,mapped_at_creation:false });
        self.buffers.insert(id,Buf { gpu:Some(buffer) });
        self.s.vb[2]=Vb { buf:id,stride };
    }
    fn inter_stage_remap(&self,_:&Program,_:Option<&Program>)->Option<(HashMap<u32,u32>,String,bool)> { None }
    fn vertex_module(&self,vs:&Program,_:&[u32],_:Option<&(HashMap<u32,u32>,String,bool)>,_:bool)->Option<ShaderModule> { Some(vs.module.clone()) }
    fn pixel_module(&self,ps:&Program,_:bool,_:Option<&(HashMap<u32,u32>,String,bool)>)->Option<ShaderModule> { Some(ps.module.clone()) }
    fn fixture(&mut self,stage:u32,uid:u64)->Rc<Program> {
        let code=if stage==0 { VS } else { PS };
        let module=self.device.create_shader_module(ShaderModuleDescriptor { label:None, source:ShaderSource::Wgsl(code.into()) });
        let bindings=if stage==0 { vec![binding(0,15,Kind::Uniform)] }
            else { vec![binding(1,16,Kind::Sampler),binding(1,32,Kind::Texture)] };
        Rc::new(Program { uid,source:PsoShader { hash:format!("{stage:016x}"),pause:false,source_sha256:"a".repeat(64) },
            module,info:Info { bindings,outputs:if stage==1 { vec![0] } else { vec![] } },flat:vec![],label:format!("fixture-{stage}") })
    }
    fn inputs(&self,vs:&Program,ps:&Program)->PipelineInputs {
        PipelineInputs { signature:self.signature.clone(),blend:self.state(self.s.blend.0),
            depth:self.state(self.s.dss.0),raster:self.state(self.s.rs),
            masks:[self.unfilt.get(&vs.uid).copied().unwrap_or_default(),self.unfilt.get(&ps.uid).copied().unwrap_or_default()] }
    }
    fn recipe(&self,vs:&Program,ps:&Rc<Program>,layout:&LayoutObj)->PsoRecipe {
        let strides:Vec<_>=self.s.vb.iter().map(|v|if v.buf!=0 { v.stride } else { 0 }).collect();
        self.pipeline_recipe(vs,Some(ps),layout,&strides,None,self.pass_info.as_ref().unwrap(),&self.inputs(vs,ps)).unwrap()
    }
    // This fixture invokes the same descriptor builder used by loading replay;
    // retained lookups below invoke the complete, unmodified get_pipeline.
    fn replay(&mut self,recipe:PsoRecipe,vs:&Rc<Program>,ps:&Rc<Program>)->Option<Rc<Pl>> {
        let layout=recipe_layout(&recipe);
        let state=|w:&Option<Vec<u32>>| w.as_ref().map(|words|Rc::new(StateObj { words:words.clone(),sampler:None }));
        let inputs=PipelineInputs { signature:recipe.signature.iter().map(|s|SigEl { sem:s[0],idx:s[1],reg:s[2] }).collect(),
            blend:state(&recipe.blend),depth:state(&recipe.depth),raster:state(&recipe.raster),
            masks:recipe.masks.map(|m|(m[0] as u128|((m[1] as u128)<<64),m[2] as u128|((m[3] as u128)<<64))) };
        let pi=PassInfo { rt_formats:recipe.colors.clone(),ds_format:recipe.depth_format,samples:recipe.samples,
            ro_depth:recipe.read_only_depth,ro_stencil:recipe.read_only_stencil,width:1,height:1 };
        let (topo,_)=topology(recipe.topology);
        let strip=match recipe.strip { 1=>Some(IndexFormat::Uint16),2=>Some(IndexFormat::Uint32),_=>None };
        let (job,vb_req,vertex_bindings)=self.create_pipeline(vs,Some(ps),&layout,&recipe.strides,topo,strip,&pi,&inputs)?;
        let pipeline=compile_pipeline(&self.device,&job)?;
        let uid=self.uid();
        let pl=Rc::new(Pl { pipeline,uid,vb_req,vertex_bindings,label:job.label,recipe:Some(Rc::new(recipe)),observed:Cell::new(false),warmed:true });
        self.retain_pso(&pl); Some(pl)
    }
PRODUCTION_METHODS
}

fn binding(group:u32,id:u32,kind:Kind)->Binding {
    Binding { group,binding:id,kind,sample:Sample::Float,dim:TextureViewDimension::D2,ms:false,
        access:StorageTextureAccess::WriteOnly,format:None }
}
fn layout()->Rc<LayoutObj> { Rc::new(LayoutObj {
    els:vec![El { sem:99,idx:0,fmt:2,slot:7,offset:0,cls:1 },El { sem:1,idx:0,fmt:2,slot:2,offset:0,cls:0 }],
    slots:vec![(7,1),(2,0)] }) }
fn recipe_layout(recipe:&PsoRecipe)->Rc<LayoutObj> {
    let els:Vec<_>=recipe.elements.iter().map(|e|El { sem:e[0],idx:e[1],fmt:e[2],slot:e[3],offset:e[4],cls:e[5] }).collect();
    let mut slots=Vec::new();for e in &els { if !slots.iter().any(|&(slot,_)|slot==e.slot) { slots.push((e.slot,e.cls)); } }
    Rc::new(LayoutObj { els,slots })
}
const VS:&str=r#"
struct Constants { offset:vec4<f32> }
@group(0) @binding(15) var<uniform> constants:Constants;
@vertex fn main(@location(0) position:vec4<f32>)->@builtin(position) vec4<f32> { return position+constants.offset; }
"#;
const PS:&str=r#"
@group(1) @binding(16) var source_sampler:sampler;
@group(1) @binding(32) var source_texture:texture_2d<f32>;
@fragment fn main()->@location(0) vec4<f32> { return textureSampleLevel(source_texture,source_sampler,vec2<f32>(0.5),0.0); }
"#;

// Real commands bind BGLs recreated from new Program UIDs to the retained PSO.
fn draw_with_recreated_layouts(gpu:&mut Gpu,pl:&Pl,vs:&Program,ps:&Program) {
    let scope=gpu.device.push_error_scope(ErrorFilter::Validation);
    let (bgl0,_)=gpu.bgl_for_mask(vs,0,0,(0,0));
    let (bgl1,_)=gpu.bgl_for_mask(ps,1,1,(0,0));
    let constants=gpu.device.create_buffer(&BufferDescriptor { label:None,size:16,usage:BufferUsages::UNIFORM,mapped_at_creation:false });
    let tex=gpu.device.create_texture(&TextureDescriptor { label:None,size:Extent3d { width:16,height:16,depth_or_array_layers:1 },
        mip_level_count:1,sample_count:1,dimension:TextureDimension::D2,format:TextureFormat::Rgba8Unorm,
        usage:TextureUsages::TEXTURE_BINDING|TextureUsages::RENDER_ATTACHMENT,view_formats:&[] });
    let sampled=tex.create_view(&Default::default());
    let sampler=gpu.device.create_sampler(&Default::default());
    let group0=gpu.device.create_bind_group(&BindGroupDescriptor { label:None,layout:&bgl0,
        entries:&[BindGroupEntry { binding:15,resource:constants.as_entire_binding() }] });
    let group1=gpu.device.create_bind_group(&BindGroupDescriptor { label:None,layout:&bgl1,
        entries:&[BindGroupEntry { binding:16,resource:BindingResource::Sampler(&sampler) },
            BindGroupEntry { binding:32,resource:BindingResource::TextureView(&sampled) }] });
    let output=gpu.device.create_texture(&TextureDescriptor { label:None,size:Extent3d { width:16,height:16,depth_or_array_layers:1 },
        mip_level_count:1,sample_count:1,dimension:TextureDimension::D2,format:TextureFormat::Rgba8Unorm,
        usage:TextureUsages::RENDER_ATTACHMENT,view_formats:&[] }).create_view(&Default::default());
    let mut encoder=gpu.device.create_command_encoder(&Default::default());
    {
        let mut pass=encoder.begin_render_pass(&RenderPassDescriptor { color_attachments:&[Some(RenderPassColorAttachment {
            view:&output,depth_slice:None,resolve_target:None,ops:Operations { load:LoadOp::Clear(Color::BLACK),store:StoreOp::Store }
        })],..Default::default() });
        pass.set_pipeline(&pl.pipeline);pass.set_bind_group(0,&group0,&[]);pass.set_bind_group(1,&group1,&[]);
        pass.set_vertex_buffer(pl.vertex_bindings.binding(2).unwrap(),gpu.buf(gpu.s.vb[2].buf).unwrap().gpu.as_ref().unwrap().slice(..));
        pass.draw(0..3,0..1);
    }
    gpu.queue.submit([encoder.finish()]);
    assert!(pollster::block_on(scope.pop()).is_none(),"retained PSO must accept structurally equal explicit BGLs");
}

// Tests are run serially. Restore inherited environment even if a test panics.
struct PrivateFixture {
    root:std::path::PathBuf, shaders:std::path::PathBuf,
    previous:Vec<(&'static str,Option<std::ffi::OsString>)>,
}
impl PrivateFixture {
    fn new()->Self {
        use std::os::unix::fs::PermissionsExt;
        static ORDINAL:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
        let root=std::env::temp_dir().join(format!("gta-pso-production-replay-{}-{}",std::process::id(),ORDINAL.fetch_add(1,Ordering::Relaxed)));
        std::fs::create_dir(&root).unwrap();std::fs::set_permissions(&root,std::fs::Permissions::from_mode(0o700)).unwrap();
        let shaders=root.join("b/fixture-build/shaders");std::fs::create_dir_all(&shaders).unwrap();
        std::fs::write(shaders.join("index.json"),br#"{"fixture":{"ok":true}}"#).unwrap();
        let previous=["MUGUET_SUPPORT","MUGUET_PSO_WARMUP","MUGUET_PSO_WARMUP_READ_ONLY","MUGUET_PIPELINE_CACHE_READ_ONLY"]
            .into_iter().map(|name|(name,std::env::var_os(name))).collect();
        std::env::set_var("MUGUET_SUPPORT",&root);
        for name in ["MUGUET_PSO_WARMUP","MUGUET_PSO_WARMUP_READ_ONLY","MUGUET_PIPELINE_CACHE_READ_ONLY"] { std::env::remove_var(name); }
        Self { root,shaders,previous }
    }
    fn configure(&self,gpu:&mut Gpu) {
        gpu.pso_store=None;gpu.shader_dir=self.shaders.clone();
        gpu.shader_index.insert("fixture".into(),serde_json::json!({"ok":true}));
    }
}
impl Drop for PrivateFixture {
    fn drop(&mut self) {
        for (name,value) in &self.previous { match value { Some(value)=>std::env::set_var(name,value),None=>std::env::remove_var(name) } }
        let _=std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn replay_production_loading_reconstructs_saved_json_and_reuses_pipeline_at_first_draw() {
    let fixture=PrivateFixture::new();
    let mut cold=Gpu::new();fixture.configure(&mut cold);cold.warmup_pipelines();
    assert!(cold.pso_store.is_some());assert_eq!(cold.pso_stats.loaded,0);assert_eq!(cold.pso_stats.built,0);
    let vs=cold.fixture(0,1);let ps=cold.fixture(1,2);let layout=layout();
    let first=cold.get_pipeline(7,8,&vs,Some(&ps),&layout).unwrap();
    assert!(!first.warmed);assert_eq!(cold.pso_stats.draw_compiles,1);
    draw_with_recreated_layouts(&mut cold,&first,&vs,&ps);
    let recipe=first.recipe.as_ref().unwrap().clone();
    // Store::record is called only after the real validated fixture draw.
    let store=cold.pso_store.as_mut().unwrap();store.record(&recipe);store.checkpoint();
    let deadline=Instant::now()+Duration::from_secs(3);
    while store.status()["writes"].as_u64()!=Some(1) {
        assert!(Instant::now()<deadline,"production writer did not publish: {}",store.status());
        std::thread::sleep(Duration::from_millis(5));store.checkpoint();
    }
    let published=fixture.root.join("native-pso-warmup-v1/recipes.json");
    let bytes=std::fs::read(&published).unwrap();
    let envelope:serde_json::Value=serde_json::from_slice(&bytes).unwrap();
    assert_eq!(envelope["recipes"].as_array().unwrap().len(),1);
    drop(first);drop(cold);

    let mut warm=Gpu::new();fixture.configure(&mut warm);
    let loading_vs=warm.fixture(0,777);let loading_ps=warm.fixture(1,888);
    warm.fixtures.insert((loading_vs.source.hash.clone(),0,false),loading_vs);
    warm.fixtures.insert((loading_ps.source.hash.clone(),1,false),loading_ps);
    // This is the extracted production disk-to-replay method, including actual
    // Store::load, recipe reconstruction, source checks and failure handling.
    warm.warmup_pipelines();
    assert_eq!(warm.pso_stats.loaded,1);assert_eq!(warm.pso_stats.built,1);
    assert_eq!(warm.pso_stats.rejected,0);assert_eq!(warm.pso_stats.failed,0);
    assert_eq!(warm.warm_pipelines.len(),1);assert!(warm.pipelines.is_empty());
    let retained=warm.warm_pipelines.get(recipe.as_ref()).unwrap().clone();
    let live_vs=warm.fixture(0,9999);let live_ps=warm.fixture(1,10000);
    warm.s.layout=999;warm.buffer(12345,16);
    let first=warm.get_pipeline(700,800,&live_vs,Some(&live_ps),&layout).unwrap();
    assert!(Rc::ptr_eq(&first,&retained));assert!(first.warmed);
    assert_eq!(warm.pso_stats.warm_hits,1);assert_eq!(warm.pso_stats.draw_compiles,0);
    draw_with_recreated_layouts(&mut warm,&first,&live_vs,&live_ps);
}

#[test]
fn replay_retained_lookup_ignores_program_and_guest_ids_and_records_real_draw() {
    let mut gpu=Gpu::new();let vs=gpu.fixture(0,1);let ps=gpu.fixture(1,2);let layout=layout();
    let recipe=gpu.recipe(&vs,&ps,&layout);
    let roundtrip: PsoRecipe=serde_json::from_slice(&serde_json::to_vec(&recipe).unwrap()).unwrap();
    assert_eq!(recipe,roundtrip);
    let retained=gpu.replay(roundtrip,&vs,&ps).unwrap();
    let new_vs=gpu.fixture(0,777);let new_ps=gpu.fixture(1,888);
    gpu.s.layout=999;gpu.buffer(900,16);gpu.s.blend.1=[0.1,0.2,0.3,0.4];gpu.s.dss.1=7;
    gpu.pass_info.as_mut().unwrap().width=1280;gpu.pass_info.as_mut().unwrap().height=800;
    let found=gpu.get_pipeline(501,502,&new_vs,Some(&new_ps),&layout).unwrap();
    assert!(Rc::ptr_eq(&retained,&found));assert_eq!(gpu.pso_stats.warm_hits,1);
    assert_eq!(gpu.pso_stats.draw_compiles,0);
    assert_eq!(found.vertex_bindings.binding(2),Some(0));assert_eq!(found.vertex_bindings.binding(7),Some(1));
    draw_with_recreated_layouts(&mut gpu,&found,&new_vs,&new_ps);
}

#[test]
fn replay_recipe_distinguishes_every_baked_input_and_keeps_dynamic_constants_out() {
    let mut gpu=Gpu::new();let vs=gpu.fixture(0,1);let ps=gpu.fixture(1,2);let layout=layout();
    let original=gpu.recipe(&vs,&ps,&layout);let mut cases=Vec::new();
    macro_rules! changed { ($field:ident,$value:expr) => {{ let mut r=original.clone();r.$field=$value;cases.push(r); }} }
    changed!(blend,Some(vec![0,0,1,5,6,1,2,1,1,15]));
    changed!(depth,Some(vec![1,1,2]));changed!(raster,Some(vec![0,2,1]));
    let mut stride=original.strides;stride[2]=32;changed!(strides,stride);
    changed!(colors,vec![Some(TextureFormat::Bgra8Unorm)]);changed!(depth_format,Some(TextureFormat::Depth32Float));
    changed!(samples,4);changed!(read_only_depth,true);changed!(read_only_stencil,true);
    changed!(topology,5);changed!(strip,1);
    let mut masks=original.masks;masks[0][1]=1<<20;changed!(masks,masks);
    let mut masks=original.masks;masks[1][2]=1<<16;changed!(masks,masks);
    let mut els=original.elements.clone();els[1][4]=4;changed!(elements,els);
    let mut sig=original.signature.clone();sig[0][2]=1;changed!(signature,sig);
    let mut shader=original.vs.clone();shader.pause=true;changed!(vs,shader);
    let mut shader=original.ps.clone().unwrap();shader.source_sha256="b".repeat(64);changed!(ps,Some(shader));
    let mut unique=HashSet::from([original]);for case in cases { assert!(unique.insert(case),"baked descriptor inputs cannot collide"); }
}

#[test]
fn replay_nonmatching_stride_blend_target_and_mask_compile_on_demand() {
    for variant in 0..4 {
        let mut gpu=Gpu::new();let vs=gpu.fixture(0,1);let ps=gpu.fixture(1,2);let layout=layout();
        let recipe=gpu.recipe(&vs,&ps,&layout);let warmed=gpu.replay(recipe,&vs,&ps).unwrap();
        match variant {
            0=>gpu.buffer(900,32),
            1=>{ gpu.states.insert(901,Rc::new(StateObj { words:vec![0,0,0,0,0,0,0,0,0,7],sampler:None }));gpu.s.blend.0=901; },
            2=>gpu.pass_info.as_mut().unwrap().rt_formats=vec![Some(TextureFormat::Bgra8Unorm)],
            _=>{gpu.unfilt.insert(ps.uid,(1u128<<32,1u128<<16));},
        }
        let found=gpu.get_pipeline(1,2,&vs,Some(&ps),&layout).unwrap();
        assert!(!Rc::ptr_eq(&warmed,&found));assert_eq!(gpu.pso_stats.warm_hits,0);
        assert_eq!(gpu.pso_stats.draw_compiles,1);assert_eq!(gpu.warm_pipelines.len(),2);
    }
}

#[test]
fn replay_missing_shader_and_failed_warm_compile_leave_first_use_fallback_available() {
    let mut gpu=Gpu::new();let vs=gpu.fixture(0,1);let ps=gpu.fixture(1,2);let layout=layout();
    let recipe=gpu.recipe(&vs,&ps,&layout);let mut programs=HashMap::new();
    assert!(gpu.warmup_program(&recipe.vs,0,&mut programs).is_none());
    assert!(gpu.pipelines.is_empty());assert!(gpu.warm_pipelines.is_empty());
    let mut bad=recipe;bad.strides[2]=1;
    assert!(gpu.replay(bad,&vs,&ps).is_none(),"wgpu must reject an invalid baked vertex stride");
    assert!(gpu.pipelines.is_empty());assert!(gpu.warm_pipelines.is_empty());
    let found=gpu.get_pipeline(1,2,&vs,Some(&ps),&layout).unwrap();
    assert!(!found.warmed);assert_eq!(gpu.pso_stats.draw_compiles,1);
}

#[test]
fn replay_shader_source_digest_and_pause_discriminator_are_verified_before_reuse() {
    let mut gpu=Gpu::new();let program=gpu.fixture(0,1);
    gpu.fixtures.insert((program.source.hash.clone(),0,false),program.clone());
    let mut loaded=HashMap::new();
    assert!(Rc::ptr_eq(&gpu.warmup_program(&program.source,0,&mut loaded).unwrap(),&program));
    let mut stale=program.source.clone();stale.source_sha256="b".repeat(64);
    assert!(gpu.warmup_program(&stale,0,&mut loaded).is_none());
    let mut pause=program.source.clone();pause.pause=true;
    assert!(gpu.warmup_program(&pause,0,&mut loaded).is_none());
}
PRODUCTION_FUNCTIONS
'''


def main() -> None:
    gpu = (ROOT / "game-runtime/src/gpu.rs").read_text()
    methods = "\n".join(block(gpu, f"    fn {name}(") for name in (
        "pipeline_recipe", "retain_pso", "warmup_program", "pso_status", "get_pipeline", "create_pipeline",
        "finish_pipeline", "layout_entry", "layout_entry_masked", "bgl_for_mask", "empty_bgl"))
    warmup = block(gpu, "    fn warmup_pipelines(")
    gate = '!cfg!(target_env = "ohos")'
    if warmup.count(gate) != 1:
        raise ValueError("warmup host-only gate override drift; do not modify production replay logic")
    methods += "\n" + warmup.replace(gate, "false")
    types = []
    for name, derive in (("PassInfo", "Clone"), ("Pl", None), ("PipelineInputs", None),
                         ("PsoStats", "Default"), ("PipeJob", None), ("VbReq", "Clone, Copy")):
        types.append((f"#[derive({derive})]\n" if derive else "") + block(gpu, f"struct {name} {{"))
    types.append("enum PipeEntry { Pending, Done(Option<Rc<Pl>>) }")
    functions = "\n".join(block(gpu, f"fn {name}(") for name in (
        "compile_pipeline", "cmp", "stencil_op", "blend_factor", "blend_op", "topology",
        "vertex_format", "wf", "format_id", "blend_for", "has_stencil", "cache_source_digest"))
    harness = HARNESS.replace("PRODUCTION_TYPES", "\n".join(types)).replace("PRODUCTION_METHODS", methods)
    harness = harness.replace("PRODUCTION_FUNCTIONS", functions)
    env = dict(os.environ)
    cargo_home = ROOT / "runtime-probe/.toolchain/cargo"
    env["CARGO_HOME"] = str(cargo_home)
    env["RUSTUP_HOME"] = str(ROOT / "runtime-probe/.toolchain/rustup")
    env["RUSTFLAGS"] = "-Awarnings"
    host = "aarch64-apple-darwin" if platform.system() == "Darwin" else "x86_64-unknown-linux-gnu"
    with tempfile.TemporaryDirectory(prefix="gta-pso-replay-") as temporary:
        work = Path(temporary)
        manifest = '''[package]
name="pso-pipeline-replay-tests"
version="0.1.0"
edition="2021"
[dependencies]
wgpu={version="=30.0.1",default-features=false,features=["std","noop","wgsl"]}
wgpu-types={version="=30.0.1",features=["serde"]}
serde={version="1",features=["derive"]}
serde_json="1"
sha2="=0.10.9"
libc="0.2"
pollster="=1.0.1"
[lib]
path="lib.rs"
[workspace]
[patch.crates-io]
'''
        for crate in ("wgpu-core", "wgpu-hal"):
            manifest += crate + "={path=" + json.dumps(str(ROOT / "game-runtime/vendor" / crate), ensure_ascii=False) + "}\n"
        (work / "Cargo.toml").write_text(manifest)
        prefix = "\n".join(f"#[path={json.dumps(str(ROOT / 'game-runtime/src' / (name+'.rs')), ensure_ascii=False)}] mod {name};"
                           for name in ("pso_warmup", "vertex_bindings"))
        # Inner crate attributes must precede path modules.
        harness = harness.replace("#![allow(dead_code, unused_imports)]", "#![allow(dead_code, unused_imports)]\n" + prefix)
        (work / "lib.rs").write_text(harness)
        subprocess.run([str(cargo_home / "bin/cargo"), "test", "--offline", "--manifest-path", str(work / "Cargo.toml"),
                        "--target", host, "replay_", "--", "--test-threads=1", "--nocapture"], env=env, check=True)
    print("production PSO recipe/descriptor/lookup and explicit BGL compatibility passed with wgpu NOOP; no device or performance acceptance claim")


if __name__ == "__main__":
    main()
