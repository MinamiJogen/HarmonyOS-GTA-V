#!/usr/bin/env python3
"""Exercise actual before/after trace-gated methods with recording drivers."""
from pathlib import Path
import hashlib
import json
import os
import subprocess

root = Path(__file__).resolve().parents[1]
evidence = root.parent / 'native-platform/evidence/trace-preparation-gate'
after = {'gpu': (root / 'src/gpu.rs').read_text(),
         'command': (root / 'vendor/wgpu-hal/src/vulkan/command.rs').read_text(),
         'device': (root / 'vendor/wgpu-hal/src/vulkan/device.rs').read_text()}
before = {k: (evidence / (k + '.rs.before')).read_text() for k in after}


def body(source, token):
    start = source.index('{', source.index(token)) + 1
    depth = 1
    for end in range(start, len(source)):
        depth += (source[end] == '{') - (source[end] == '}')
        if depth == 0:
            return source[start:end]
    raise ValueError(token)


def bookkeeping(source):
    at = source.index('            let mut counts = COPY_TRACE_COUNTS.lock().unwrap();', source.index('unsafe fn begin_encoding('))
    start = source.rfind('        #[cfg(target_env = "ohos")]', 0, at)
    end = source.index('\n\n        Ok(())', at)
    return source[start:end]


source = r'''
#![allow(dead_code,unused_imports)]
extern crate alloc;
use ash::vk;
use ash::vk::Handle;
use std::{collections::HashMap,sync::{Arc,Mutex,atomic::{AtomicU64,Ordering}}};
static FORMATS:AtomicU64=AtomicU64::new(0);
#[derive(Clone,Copy)] struct Number(u64);
impl std::fmt::Display for Number {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{FORMATS.fetch_add(1,Ordering::Relaxed);self.0.fmt(f)}}
impl std::fmt::Debug for Number {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{FORMATS.fetch_add(1,Ordering::Relaxed);self.0.fmt(f)}}
#[derive(Clone)]struct Allocation {memory:vk::DeviceMemory,offset:u64,size:u64}
impl Allocation {fn memory(&self)->vk::DeviceMemory{self.memory}fn offset(&self)->u64{self.offset}fn size(&self)->u64{self.size}}
#[derive(Clone)]struct Backing {allocation:Allocation,locks:Arc<AtomicU64>}
impl Backing {fn lock(&self)->&Allocation{self.locks.fetch_add(1,Ordering::Relaxed);&self.allocation}}
#[derive(Clone)]enum BufferOwnership {Managed(Backing),External}
#[derive(Clone)]struct Buffer {raw:vk::Buffer,size:u64,usage:Number,ownership:BufferOwnership}
#[derive(Debug,PartialEq,Eq,Clone)]enum Event {Flush,Upload(u64,u64,Vec<u8>),Copy(u64,u64,u64,Vec<(u64,u64,u64)>)}
struct Queue {events:Arc<Mutex<Vec<Event>>>}
impl Queue {fn write_buffer(&self,b:&Buffer,off:u64,data:&[u8]){self.events.lock().unwrap().push(Event::Upload(b.raw.as_raw(),off,data.to_vec()));}}
struct Buf {gpu:Option<Buffer>,bytes:u64,shadow:Option<Vec<u8>>,version:u32}
struct Gpu {memory:Vec<u8>,buffers:HashMap<u32,Buf>,queue:Queue,presents:Number,submits:Number,enc_epoch:Number}
impl Gpu {
 fn bytes(&self,a:u64,n:u64)->&[u8]{&self.memory[a as usize..(a+n)as usize]}
 fn used(&self,_:u32)->bool{true}
 fn buf_mut(&mut self,id:u32)->Option<&mut Buf>{self.buffers.get_mut(&id)}
 fn flush(&mut self){self.queue.events.lock().unwrap().push(Event::Flush);}
 fn upload_before(&mut self,id:u32,offset:u64,size:u64,data_addr:u64,no_overwrite:bool){UPLOAD_BEFORE}
 fn upload_after(&mut self,id:u32,offset:u64,size:u64,data_addr:u64,no_overwrite:bool){UPLOAD_AFTER}
}
static COPY_TRACE_COUNTS:Mutex<Vec<(u64,u64,u64)>>=Mutex::new(Vec::new());
static COPY_TRACE_SEQUENCE:AtomicU64=AtomicU64::new(0);
struct BufferCopy {src_offset:u64,dst_offset:u64,size:std::num::NonZeroU64}
mod smallvec {
 pub trait Array {type Item;}impl<T,const N:usize>Array for [T;N]{type Item=T;}
 pub type SmallVec<A>=Vec<<A as Array>::Item>;
}
struct Journal;impl Journal{fn event(&mut self,_:std::fmt::Arguments<'_>){}}
struct Raw {events:Arc<Mutex<Vec<Event>>>}
impl Raw {unsafe fn cmd_copy_buffer(&self,cmd:vk::CommandBuffer,src:vk::Buffer,dst:vk::Buffer,regions:&[vk::BufferCopy]){self.events.lock().unwrap().push(Event::Copy(cmd.as_raw(),src.as_raw(),dst.as_raw(),regions.iter().map(|r|(r.src_offset,r.dst_offset,r.size)).collect()));}}
struct Device {raw:Raw}
struct Encoder {raw:vk::CommandPool,active:vk::CommandBuffer,pool_generation:u64,device:Device,submit_trace:Option<Journal>}
mod command {
 use super::*;
 impl Encoder {
  pub(super) fn begin_after(&mut self,raw:vk::CommandBuffer){BEGIN_AFTER}
  pub(super) unsafe fn copy_before<T:Iterator<Item=crate::BufferCopy>>(&mut self,src:&super::Buffer,dst:&super::Buffer,regions:T){COPY_BEFORE}
  pub(super) unsafe fn copy_after<T:Iterator<Item=crate::BufferCopy>>(&mut self,src:&super::Buffer,dst:&super::Buffer,regions:T){COPY_AFTER}
 }
}
type Label<'a>=Option<&'a str>;
mod spirv_capture{pub fn pipeline_sequence()->Option<u64>{Some(71)}}
fn pipeline_trace_thread_dir()->Option<std::path::PathBuf>{PIPELINE_DIR}
fn pipeline_trace_begin(kind:&str,label:Label<'_>,config:impl FnOnce()->String)->Option<(std::path::PathBuf,String)>{PIPELINE_BEGIN}
fn pipeline_trace_end(trace:&Option<(std::path::PathBuf,String)>,result:impl FnOnce()->String){PIPELINE_END}
fn make_buffer(raw:u64)->(Buffer,Arc<AtomicU64>){let locks=Arc::new(AtomicU64::new(0));(Buffer{raw:vk::Buffer::from_raw(raw),size:128,usage:Number(3),ownership:BufferOwnership::Managed(Backing{allocation:Allocation{memory:vk::DeviceMemory::from_raw(66),offset:256,size:128},locks:locks.clone()})},locks)}
fn gpu()->Gpu{let(b,_)=make_buffer(19);Gpu{memory:(0..128).collect(),buffers:HashMap::from([(1,Buf{gpu:Some(b),bytes:128,shadow:Some(vec![0;128]),version:0}),(2,Buf{gpu:None,bytes:0,shadow:Some(vec![0;128]),version:0})]),queue:Queue{events:Arc::new(Mutex::new(Vec::new()))},presents:Number(12),submits:Number(34),enc_epoch:Number(56)}}
fn encoder()->Encoder{Encoder{raw:vk::CommandPool::from_raw(10),active:vk::CommandBuffer::from_raw(11),pool_generation:2,device:Device{raw:Raw{events:Arc::new(Mutex::new(Vec::new()))}},submit_trace:None}}
fn directory()->std::path::PathBuf{let p=std::env::temp_dir().join(format!("mg-trace-gate-{}",std::process::id()));std::fs::create_dir_all(&p).unwrap();p}
fn region()->BufferCopy{BufferCopy{src_offset:4,dst_offset:12,size:std::num::NonZeroU64::new(8).unwrap()}}
#[test]fn upload_off_skips_trace_format_but_preserves_shadow_flush_and_exact_writes(){
 std::env::remove_var("MUGUET_GPU_UPLOAD_TRACE");let(mut a,mut b)=(gpu(),gpu());
 a.upload_before(1,4,6,8,false);let expected=a.queue.events.lock().unwrap().clone();
 FORMATS.store(0,Ordering::Relaxed);b.upload_after(1,4,6,8,false);
 assert_eq!(FORMATS.load(Ordering::Relaxed),0);assert_eq!(*b.queue.events.lock().unwrap(),expected);
 assert_eq!(b.buffers[&1].shadow,a.buffers[&1].shadow);assert_eq!(b.buffers[&1].version,a.buffers[&1].version);
 assert_eq!(expected,vec![Event::Flush,Event::Upload(19,4,vec![8,9,10,11]),Event::Upload(19,8,vec![12,13,0,0])]);
}
#[test]fn upload_on_preserves_trace_fields_and_sequence_including_early_returns(){
 let dir=directory();let path=dir.join("upload.txt");std::env::set_var("MUGUET_GPU_UPLOAD_TRACE",&path);
 let(mut a,mut b)=(gpu(),gpu());let cases=[(2,0,4,0,true),(9,0,4,0,true),(1,4,6,8,false),(1,8,4,16,true)];let mut traces=Vec::new();
 for &(i,o,n,p,no)in &cases{a.upload_before(i,o,n,p,no);traces.push(std::fs::read_to_string(&path).ok());}
 let _=std::fs::remove_file(&path);
 for ((i,o,n,p,no),expected)in cases.into_iter().zip(traces){b.upload_after(i,o,n,p,no);assert_eq!(std::fs::read_to_string(&path).ok(),expected);}
 assert_eq!(*a.queue.events.lock().unwrap(),*b.queue.events.lock().unwrap());
 let text=std::fs::read_to_string(&path).unwrap();assert!(text.contains("sequence=4 id=1"));assert!(text.contains("phase=after"));std::fs::remove_dir_all(dir).unwrap();
}
#[test]fn copy_off_skips_counts_lock_and_allocation_access_but_preserves_native_copy(){
 std::env::remove_var("MUGUET_VK_COPY_TRACE");COPY_TRACE_COUNTS.lock().unwrap().push((11,4,99));
 let mut enc=encoder();let(src,sl)=make_buffer(21);let(dst,dl)=make_buffer(22);
 // A poisoned diagnostics-only lock must be unreachable while OFF.
 let _=std::thread::spawn(||{let _guard=COPY_TRACE_COUNTS.lock().unwrap();panic!("poison diagnostic mutex");}).join();
 FORMATS.store(0,Ordering::Relaxed);enc.begin_after(enc.active);
 unsafe{enc.copy_after(&src,&dst,[region()].into_iter())};
 assert_eq!(sl.load(Ordering::Relaxed),0);assert_eq!(dl.load(Ordering::Relaxed),0);assert_eq!(FORMATS.load(Ordering::Relaxed),0);assert_eq!(COPY_TRACE_SEQUENCE.load(Ordering::Relaxed),0);
 assert_eq!(*enc.device.raw.events.lock().unwrap(),vec![Event::Copy(11,21,22,vec![(4,12,8)])]);
}
#[test]fn copy_on_preserves_trace_and_native_arguments_and_resets_begin_counters(){
 let dir=directory();let path=dir.join("copy.txt");std::env::set_var("MUGUET_VK_COPY_TRACE",&path);
 let(mut a,mut b)=(encoder(),encoder());let(src,_)=make_buffer(21);let(dst,_)=make_buffer(22);
 unsafe{a.copy_before(&src,&dst,[region()].into_iter());a.copy_before(&src,&dst,[region()].into_iter());}
 let text=std::fs::read_to_string(&path).unwrap();let events=a.device.raw.events.lock().unwrap().clone();
 COPY_TRACE_SEQUENCE.store(0,Ordering::Relaxed);COPY_TRACE_COUNTS.lock().unwrap().clear();
 unsafe{b.copy_after(&src,&dst,[region()].into_iter());b.copy_after(&src,&dst,[region()].into_iter());}
 assert_eq!(std::fs::read_to_string(&path).unwrap(),text);assert!(text.contains("commandCopies=2 commandBytes=16"));assert_eq!(*b.device.raw.events.lock().unwrap(),events);
 b.begin_after(b.active);assert_eq!(*COPY_TRACE_COUNTS.lock().unwrap(),vec![(11,0,0)]);std::fs::remove_dir_all(dir).unwrap();
}
#[test]fn pipeline_off_never_evaluates_debug_closures_or_creates_trace(){
 std::env::remove_var("MUGUET_PIPELINE_TRACE_DIR");let trace=pipeline_trace_begin("graphics",Some("label"),||panic!("descriptor formatted while OFF"));assert!(trace.is_none());pipeline_trace_end(&trace,||panic!("result formatted while OFF"));
}
#[test]fn pipeline_on_evaluates_each_closure_once_and_keeps_snapshots(){
 let dir=directory();std::env::set_var("MUGUET_PIPELINE_TRACE_DIR",&dir);let calls=std::cell::Cell::new(0);
 let trace=pipeline_trace_begin("graphics",Some("label"),||{calls.set(calls.get()+1);"exact descriptor".into()});
 assert_eq!(calls.get(),1);let thread=std::thread::current();let name=thread.name().unwrap();let snapshot=std::fs::read_to_string(dir.join(name).join("pipeline-config-last.txt")).unwrap();assert_eq!(snapshot,"exact descriptor");
 pipeline_trace_end(&trace,||{calls.set(calls.get()+1);"Ok([nativePipeline])".into()});assert_eq!(calls.get(),2);let text=std::fs::read_to_string(dir.join("last-pipeline.txt")).unwrap();assert!(text.contains("phase=after"));assert!(text.contains("label=label"));assert!(text.contains("result=Ok([nativePipeline])"));std::fs::remove_dir_all(dir).unwrap();
}
'''
for key in ['before', 'after']:
    source = source.replace('UPLOAD_' + key.upper(), body(locals()[key]['gpu'], 'fn upload_buffer('))
    source = source.replace('COPY_' + key.upper(), body(locals()[key]['command'], 'unsafe fn copy_buffer_to_buffer<T>'))
source = source.replace('BEGIN_AFTER', bookkeeping(after['command']))
for key, token in [('PIPELINE_DIR', 'fn pipeline_trace_thread_dir('), ('PIPELINE_BEGIN', 'fn pipeline_trace_begin('), ('PIPELINE_END', 'fn pipeline_trace_end(')]:
    source = source.replace(key, body(after['device'], token))
out = root / 'logs/trace-preparation-gate-model'
out.mkdir(exist_ok=True)
rs = out / 'actual-trace-method-tests.rs'
rs.write_text(source)
deps = root / 'logs/device-fault-collector-model/target/debug/deps'
ash = next(deps.glob('libash-*.rlib'))
env = dict(os.environ)
env['CARGO_HOME'] = str(root.parent / 'runtime-probe/.toolchain/cargo')
env['RUSTUP_HOME'] = str(root.parent / 'runtime-probe/.toolchain/rustup')
env['PATH'] = env['CARGO_HOME'] + '/bin:' + env.get('PATH', '')
binary = out / 'tests'
subprocess.run(['rustc', '--edition=2021', '--test', '-Aexplicit_builtin_cfgs_in_flags', '--cfg', 'target_env="ohos"', str(rs), '--extern', 'ash=' + str(ash), '-L', 'dependency=' + str(deps), '-o', str(binary)], env=env, check=True)
names = ['upload_off_skips_trace_format_but_preserves_shadow_flush_and_exact_writes', 'upload_on_preserves_trace_fields_and_sequence_including_early_returns', 'copy_off_skips_counts_lock_and_allocation_access_but_preserves_native_copy', 'copy_on_preserves_trace_and_native_arguments_and_resets_begin_counters', 'pipeline_off_never_evaluates_debug_closures_or_creates_trace', 'pipeline_on_evaluates_each_closure_once_and_keeps_snapshots']
cases, logs = [], []
for name in names:
    run = subprocess.run([str(binary), '--exact', name], text=True, capture_output=True)
    logs += [run.stdout, run.stderr]
    cases.append({'case': name, 'exitCode': run.returncode})
(root / 'logs/trace-preparation-gate-tests.log').write_text(''.join(logs))
report = {'method': 'Actual production before/after upload_buffer and HAL copy method bodies, actual current begin diagnostic block and pipeline helpers. Real ash structs with recording upload/copy drivers; each test isolated in its own process. No GPU execution.',
          'sourceSHA256': {k: hashlib.sha256(v.encode()).hexdigest() for k, v in after.items()}, 'cases': cases, 'allPassed': all(c['exitCode'] == 0 for c in cases)}
(out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
raise SystemExit(0 if report['allPassed'] else 1)
