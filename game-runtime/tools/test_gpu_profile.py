#!/usr/bin/env python3
"""Run entire production profiler with recording-only mock wgpu, fresh process per config.
The mock tests scheduling, capacities, query indices, asynchronous map ownership and OFF cost.
It does not establish real Vulkan timestamps or device functionality.
"""
from pathlib import Path
import subprocess,os,json,hashlib,shutil
root=Path(__file__).resolve().parents[1];out=root/'logs/gpu-profile-model';(out/'src').mkdir(parents=True,exist_ok=True)
actual=(root/'src/gpu_profile.rs').read_text();model=(root/'src/gpu_profile_model.rs').read_text()
extra=r'''
pub fn test_case(case:&str,dir:&std::path::Path) {
 use std::sync::atomic::Ordering;
 let device=Device;let queue=Queue;
 match case {
 "off"=>{
  assert!(!requested());assert_eq!(features(&Adapter),Features::empty());
  assert!(Profiler::new(&device,&queue).is_none());assert_eq!(crate::calls(),(0,0,0));assert!(!dir.exists());
 },
 "unsupported"=>{
  crate::SUPPORTED.store(false,Ordering::Relaxed);
  assert_eq!(features(&Adapter),Features::empty());assert!(Profiler::new(&device,&queue).is_none());
  assert_eq!(crate::calls(),(0,0,1));
  let c:serde_json::Value=serde_json::from_str(&std::fs::read_to_string(dir.join("capabilities.json")).unwrap()).unwrap();assert_eq!(c["supported"],false);
 },
 "roundtrip"=>{
  let mut p=Profiler::new(&device,&queue).unwrap();assert_eq!(crate::calls(),(4,8,1));
  let mut enc=CommandEncoder::default();p.encoder_begin(&mut enc);p.encoder_begin(&mut enc);
  let pair=p.pair("render",||"real label".into()).unwrap();assert_eq!((pair.begin,pair.end),(2,3));
  let rw=pair.render();let cw=pair.compute();
  assert_eq!((rw.beginning_of_pass_write_index,rw.end_of_pass_write_index),(Some(2),Some(3)));
  assert_eq!((cw.beginning_of_pass_write_index,cw.end_of_pass_write_index),(Some(2),Some(3)));
  assert!(std::ptr::eq(rw.query_set,&pair.query)&&std::ptr::eq(cw.query_set,&pair.query));
  pair.begin(&mut enc);pair.end(&mut enc);p.finish_frame(&mut enc);
  assert_eq!(enc.events,vec!["timestamp0","timestamp2","timestamp3","timestamp1","resolve0..4","copy32"]);
  assert!(p.active.is_none());assert!(p.slots[0].state==State::Ready);assert_eq!(crate::pending(),0);
  p.submitted();assert_eq!(crate::pending(),1);assert!(p.slots[0].state==State::Mapping);
  p.collect();assert!(p.slots[0].state==State::Mapping);assert_eq!(std::fs::read(dir.join("frames.jsonl")).unwrap().len(),0);
  p.begin_frame(1);assert!(p.active.is_none());crate::pump(true);p.collect();assert!(p.slots[0].state==State::Idle);
  let text=std::fs::read_to_string(dir.join("frames.jsonl")).unwrap();let r:serde_json::Value=serde_json::from_str(text.trim()).unwrap();
  assert_eq!(r["mapSuccess"],true);assert_eq!(r["queryCount"],4);assert_eq!(r["records"][0]["label"],"real label");assert_eq!(r["frameSpanNs"],60.0);
  p.begin_frame(30);assert_eq!(p.active,Some(0));p.cancel_empty_frame();assert!(p.active.is_none());assert!(p.slots[0].state==State::Idle);
 },
 "busy"=>{
  let mut p=Profiler::new(&device,&queue).unwrap();
  for frame in [0,30,60,90] {if frame!=0{p.begin_frame(frame);}let mut enc=CommandEncoder::default();p.encoder_begin(&mut enc);p.finish_frame(&mut enc);p.submitted();}
  assert_eq!(crate::pending(),4);p.begin_frame(120);assert!(p.active.is_none());assert_eq!(p.dropped_busy,1);
  assert!(p.slots.iter().all(|s|s.state==State::Mapping));crate::pump(true);p.collect();assert!(p.slots.iter().all(|s|s.state==State::Idle));
  p.begin_frame(150);assert!(p.active.is_some());
 },
 "budget"=>{
  let mut p=Profiler::new(&device,&queue).unwrap();let mut enc=CommandEncoder::default();p.encoder_begin(&mut enc);
  for _ in 0..255 {let t=p.pair("render",||"x".repeat(1024)).unwrap();t.begin(&mut enc);t.end(&mut enc);}
  let mut formatted=false;assert!(p.pair("render",||{formatted=true;"overflow".into()}).is_none());assert!(!formatted);
  assert_eq!(p.slots[0].next,512);assert_eq!(p.slots[0].truncated,1);assert!(p.slots[0].records.iter().all(|r|r.label.len()==384&&r.label_truncated));
  p.finish_frame(&mut enc);assert!(enc.events.contains(&"resolve0..512".into()));p.submitted();crate::pump(true);
  std::fs::OpenOptions::new().write(true).open(dir.join("frames.jsonl")).unwrap().set_len(MAX_OUTPUT_BYTES).unwrap();p.bytes=MAX_OUTPUT_BYTES;p.collect();assert!(p.output_truncated);assert!(dir.join("truncated.json").exists());
  p.begin_frame(30);assert!(p.active.is_none());
 },
 "map_error"=>{
  let mut p=Profiler::new(&device,&queue).unwrap();let mut enc=CommandEncoder::default();p.encoder_begin(&mut enc);p.finish_frame(&mut enc);p.submitted();crate::pump(false);p.collect();
  assert_eq!(p.map_errors,1);assert!(p.slots[0].state==State::Idle);
  let r:serde_json::Value=serde_json::from_str(std::fs::read_to_string(dir.join("frames.jsonl")).unwrap().trim()).unwrap();assert_eq!(r["mapSuccess"],false);assert!(r.get("frameSpanNs").is_none());
 },
 _=>panic!("unknown case"),
 }
 println!("PASS {case}");
}
'''
(out/'src/actual.rs').write_text(actual+extra);(out/'src/gpu_profile_model.rs').write_text(model)
mock=r'''
extern crate self as wgpu;
use std::sync::{Arc,Mutex,atomic::{AtomicBool,AtomicU64,Ordering}};
static SUPPORTED:AtomicBool=AtomicBool::new(true);
static QUERY:AtomicU64=AtomicU64::new(0);static BUFFER:AtomicU64=AtomicU64::new(0);static PERIOD:AtomicU64=AtomicU64::new(0);
static CLOCK:AtomicU64=AtomicU64::new(100);
type Callback=Box<dyn FnOnce(Result<(),&'static str>)+Send>;
static CALLBACKS:Mutex<Vec<Callback>>=Mutex::new(Vec::new());
fn pending()->usize{CALLBACKS.lock().unwrap().len()}
fn pump(ok:bool){for c in std::mem::take(&mut*CALLBACKS.lock().unwrap()){c(if ok{Ok(())}else{Err("map failed")});}}
fn calls()->(u64,u64,u64){(QUERY.load(Ordering::Relaxed),BUFFER.load(Ordering::Relaxed),PERIOD.load(Ordering::Relaxed))}
#[derive(Clone,Copy,Debug,PartialEq,Eq)] pub struct Features(u32);
impl Features {pub const TIMESTAMP_QUERY:Self=Self(1);pub const TIMESTAMP_QUERY_INSIDE_ENCODERS:Self=Self(2);pub fn empty()->Self{Self(0)}pub fn contains(self,x:Self)->bool{self.0&x.0==x.0}}
impl std::ops::BitOr for Features {type Output=Self;fn bitor(self,r:Self)->Self{Self(self.0|r.0)}}
pub struct Adapter;impl Adapter{pub fn features(&self)->Features{if SUPPORTED.load(Ordering::Relaxed){Features(3)}else{Features(0)}}}
pub mod hal {pub mod api{pub struct Vulkan;}}
pub struct Hal;impl Hal {pub fn timestamp_query_capabilities(&self)->(u32,u32,f32){(7,36,2.0)}}
pub struct Device;impl Device {
 pub unsafe fn as_hal<A>(&self)->Option<Hal>{Some(Hal)}
 pub fn features(&self)->Features{Adapter.features()}
 pub fn create_query_set(&self,d:&QuerySetDescriptor)->QuerySet{QUERY.fetch_add(1,Ordering::Relaxed);assert_eq!(d.count,512);QuerySet(Arc::new(Mutex::new(vec![0;d.count as usize])))}
 pub fn create_buffer(&self,d:&BufferDescriptor)->Buffer{BUFFER.fetch_add(1,Ordering::Relaxed);Buffer(Arc::new(Mutex::new(vec![0;d.size as usize])))}
}
pub struct Queue;impl Queue{pub fn get_timestamp_period(&self)->f32{PERIOD.fetch_add(1,Ordering::Relaxed);2.0}}
#[derive(Clone)]pub struct QuerySet(Arc<Mutex<Vec<u64>>>);
pub struct QuerySetDescriptor<'a>{pub label:Option<&'a str>,pub ty:QueryType,pub count:u32}pub enum QueryType{Timestamp}
#[derive(Clone,Copy)]pub struct BufferUsages(u32);impl BufferUsages{pub const QUERY_RESOLVE:Self=Self(1);pub const COPY_SRC:Self=Self(2);pub const COPY_DST:Self=Self(4);pub const MAP_READ:Self=Self(8);}
impl std::ops::BitOr for BufferUsages{type Output=Self;fn bitor(self,r:Self)->Self{Self(self.0|r.0)}}
pub struct BufferDescriptor<'a>{pub label:Option<&'a str>,pub size:u64,pub usage:BufferUsages,pub mapped_at_creation:bool}
pub struct Buffer(Arc<Mutex<Vec<u8>>>);pub enum MapMode{Read}
impl Buffer {
 pub fn map_async(&self,_:MapMode,_:std::ops::Range<u64>,c:impl FnOnce(Result<(),&'static str>)+Send+'static){CALLBACKS.lock().unwrap().push(Box::new(c));}
 pub fn get_mapped_range(&self,r:std::ops::Range<u64>)->Result<Vec<u8>,&'static str>{Ok(self.0.lock().unwrap()[r.start as usize..r.end as usize].to_vec())}
 pub fn unmap(&self){}
}
pub struct RenderPassTimestampWrites<'a>{pub query_set:&'a QuerySet,pub beginning_of_pass_write_index:Option<u32>,pub end_of_pass_write_index:Option<u32>}
pub struct ComputePassTimestampWrites<'a>{pub query_set:&'a QuerySet,pub beginning_of_pass_write_index:Option<u32>,pub end_of_pass_write_index:Option<u32>}
#[derive(Default)]pub struct CommandEncoder{events:Vec<String>}
impl CommandEncoder {
 pub fn write_timestamp(&mut self,q:&QuerySet,i:u32){self.events.push(format!("timestamp{i}"));q.0.lock().unwrap()[i as usize]=CLOCK.fetch_add(10,Ordering::Relaxed);}
 pub fn resolve_query_set(&mut self,q:&QuerySet,r:std::ops::Range<u32>,b:&Buffer,offset:u64){assert_eq!(offset,0);self.events.push(format!("resolve{r:?}"));let words=q.0.lock().unwrap();let mut bytes=b.0.lock().unwrap();for i in r{bytes[i as usize*8..i as usize*8+8].copy_from_slice(&words[i as usize].to_le_bytes());}}
 pub fn copy_buffer_to_buffer(&mut self,a:&Buffer,ao:u64,b:&Buffer,bo:u64,n:u64){assert_eq!((ao,bo),(0,0));self.events.push(format!("copy{n}"));b.0.lock().unwrap()[..n as usize].copy_from_slice(&a.0.lock().unwrap()[..n as usize]);}
}
mod actual;
fn main(){let a:Vec<_>=std::env::args().collect();actual::test_case(&a[1],std::path::Path::new(&a[2]));}
'''
(out/'src/main.rs').write_text(mock)
registry=root.parent/'runtime-probe/.toolchain/cargo/registry/src/index.crates.io-1949cf8c6b5b557f'
serde=next(registry.glob('serde_json-*'))
(out/'Cargo.toml').write_text(f'[package]\nname="gpu-profile-actual-model"\nversion="0.0.0"\nedition="2021"\n[workspace]\n[dependencies]\nserde_json={{path={json.dumps(str(serde))}}}\n')
env=dict(os.environ);env['CARGO_HOME']=str(root.parent/'runtime-probe/.toolchain/cargo');env['RUSTUP_HOME']=str(root.parent/'runtime-probe/.toolchain/rustup');env['PATH']=env['CARGO_HOME']+'/bin:'+env.get('PATH','');env['RUSTFLAGS']='--cfg target_env="ohos" -Aexplicit_builtin_cfgs_in_flags'
b=subprocess.run(['cargo','build','--offline','--manifest-path',str(out/'Cargo.toml')],env=env,capture_output=True,text=True);log=b.stdout+b.stderr;cases=[]
if b.returncode==0:
 for c in ['off','unsupported','roundtrip','busy','budget','map_error']:
  d=out/('case-'+c)
  if d.exists():shutil.rmtree(d)
  e=dict(env);e.pop('MUGUET_GPU_PROFILE',None);e.pop('MUGUET_GPU_PROFILE_DIR',None)
  if c!='off':e.update(MUGUET_GPU_PROFILE='1',MUGUET_GPU_PROFILE_DIR=str(d))
  r=subprocess.run([str(out/'target/debug/gpu-profile-actual-model'),c,str(d)],env=e,capture_output=True,text=True);log+=r.stdout+r.stderr;cases.append({'case':c,'exit':r.returncode})
(root/'logs/gpu-profile-tests.log').write_text(log)
report={'method':'Entire production GPU profiler and model copied verbatim; only wgpu device/commands/maps are a recording mock. Fresh process per env mode. No real GPU qualification implied. Mock exposes no submit/poll/wait API.','sourceSHA256':hashlib.sha256(actual.encode()).hexdigest(),'modelSHA256':hashlib.sha256(model.encode()).hexdigest(),'buildExit':b.returncode,'cases':cases,'allPassed':b.returncode==0 and len(cases)==6 and all(x['exit']==0 for x in cases)}
(out/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2));raise SystemExit(0 if report['allPassed'] else 1)
