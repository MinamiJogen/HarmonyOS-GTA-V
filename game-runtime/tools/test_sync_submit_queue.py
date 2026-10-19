#!/usr/bin/env python3
"""Compile the actual Queue.submit native-call tail against a recording driver."""
from pathlib import Path
import hashlib,json,subprocess,os
root=Path(__file__).resolve().parents[1]
path=root/'vendor/wgpu-hal/src/vulkan/mod.rs';actual=path.read_text()
start=actual.index('        let submit_result = unsafe {')
end=actual.index('\n        Ok(())\n    }\n\n    unsafe fn present',start)+len('\n        Ok(())')
tail=actual[start:end]
source=r'''
#![allow(dead_code)]
use std::{cell::RefCell,sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}}};
type Events=Arc<Mutex<Vec<String>>>;
thread_local!{static EVENTS:RefCell<Option<Events>>=RefCell::new(None);}
fn record(s:String){EVENTS.with(|v|v.borrow().as_ref().unwrap().lock().unwrap().push(s));}
#[derive(Copy,Clone,Debug,PartialEq,Eq)]enum Error{Lost,OutOfMemory}
fn map_host_device_oom_and_lost_err(error:Error)->Error{record(format!("map:{error:?}"));error}
struct Raw{submit_error:Option<Error>,wait_error:Option<Error>}
impl Raw{
 unsafe fn queue_submit(&self,queue:u64,infos:&[u64],fence:u64)->Result<(),Error>{
  assert_eq!((queue,infos,fence),(9,&[77][..],11));record("submit:q9:info77:fence11".into());
  self.submit_error.map_or(Ok(()),Err)
 }
 unsafe fn queue_wait_idle(&self,queue:u64)->Result<(),Error>{assert_eq!(queue,9);record("wait:q9".into());self.wait_error.map_or(Ok(()),Err)}
}
struct Device{raw:Raw}
impl Device{fn capture_device_fault_if_lost(&self,error:Error,reason:&str){if error==Error::Lost{record(format!("capture:{reason}"));}}}
struct Trace;
impl Trace{fn phase(&self,phase:&str,result:&str){record(format!("phase:{phase}:{result}"));}fn completed(&self){record("completed".into());}}
mod submit_trace{use super::*;pub static SYNC:AtomicBool=AtomicBool::new(false);pub fn sync_enabled()->bool{SYNC.load(Ordering::Relaxed)}}
struct Queue{device:Device,raw:u64}
impl Queue{
 unsafe fn actual_submit_tail(&self,diagnostic:Option<Trace>)->Result<(),Error>{
  let vk_info=77;let fence_raw=11;
ACTUAL_TAIL
 }
}
fn exercise(sync:bool,submit_error:Option<Error>,wait_error:Option<Error>)->(Result<(),Error>,Vec<String>){
 let events=Arc::new(Mutex::new(Vec::new()));EVENTS.with(|v|*v.borrow_mut()=Some(events.clone()));
 submit_trace::SYNC.store(sync,Ordering::Relaxed);
 let q=Queue{device:Device{raw:Raw{submit_error,wait_error}},raw:9};
 let result=unsafe{q.actual_submit_tail(sync.then_some(Trace))};let log=events.lock().unwrap().clone();(result,log)
}
fn position(log:&[String],prefix:&str)->usize{log.iter().position(|s|s.starts_with(prefix)).unwrap()}
#[test]fn actual_queue_tail_preserves_parameters_order_and_failure_without_false_completion(){
 let(result,log)=exercise(false,None,None);assert_eq!(result,Ok(()));assert_eq!(log,["submit:q9:info77:fence11"]);
 let(result,log)=exercise(true,None,None);assert_eq!(result,Ok(()));
 assert!(position(&log,"submit:")<position(&log,"wait:")&&position(&log,"wait:")<position(&log,"completed"));
 assert_eq!(log.iter().filter(|s|s.starts_with("submit:")).count(),1);assert_eq!(log.iter().filter(|s|s.starts_with("wait:")).count(),1);
 let(result,log)=exercise(true,Some(Error::Lost),None);assert_eq!(result,Err(Error::Lost));
 assert!(position(&log,"submit:")<position(&log,"capture:vkQueueSubmit")&&position(&log,"capture:")<position(&log,"map:"));
 assert!(!log.iter().any(|s|s.starts_with("wait:")||s=="completed"));
 let(result,log)=exercise(true,None,Some(Error::Lost));assert_eq!(result,Err(Error::Lost));
 assert!(position(&log,"submit:")<position(&log,"wait:")&&position(&log,"wait:")<position(&log,"capture:sync_submit vkQueueWaitIdle")&&position(&log,"capture:")<position(&log,"map:"));
 assert!(!log.iter().any(|s|s=="completed"));
 let(result,log)=exercise(true,Some(Error::OutOfMemory),None);assert_eq!(result,Err(Error::OutOfMemory));assert!(log.iter().any(|s|s=="map:OutOfMemory"));assert!(!log.iter().any(|s|s.starts_with("capture:")||s.starts_with("wait:")||s=="completed"));
}
'''.replace('ACTUAL_TAIL',tail)
out=root/'logs/sync-submit-queue-model';out.mkdir(exist_ok=True);rs=out/'actual-queue-submit-tests.rs';rs.write_text(source)
env=dict(os.environ);env['CARGO_HOME']=str(root.parent/'runtime-probe/.toolchain/cargo');env['RUSTUP_HOME']=str(root.parent/'runtime-probe/.toolchain/rustup');env['PATH']=env['CARGO_HOME']+'/bin:'+env.get('PATH','')
binary=out/'queue-submit-tests'
build=subprocess.run(['rustc','--edition=2021','--test','-Aexplicit_builtin_cfgs_in_flags','--cfg','target_env="ohos"',str(rs),'-o',str(binary)],env=env,text=True,capture_output=True)
log=build.stdout+build.stderr
run=None
if build.returncode==0:
 run=subprocess.run([str(binary)],text=True,capture_output=True);log+=run.stdout+run.stderr
(root/'logs/sync-submit-queue-tests.log').write_text(log)
report={'method':'Actual Queue.submit tail from let submit_result through Ok(()) extracted verbatim and compiled under OHOS cfg against recording Vulkan stub. Validates control flow, not real GPU completion.','sourceSHA256':hashlib.sha256(actual.encode()).hexdigest(),'extractedSHA256':hashlib.sha256(tail.encode()).hexdigest(),'buildExit':build.returncode,'testExit':None if run is None else run.returncode,'scenarios':5}
(out/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(log);raise SystemExit(0 if run is not None and run.returncode==0 else 1)
