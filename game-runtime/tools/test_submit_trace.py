#!/usr/bin/env python3
"""Exercise the real submit journal with fresh-process OnceLock configurations."""
from pathlib import Path
import os, json, hashlib, subprocess

root=Path(__file__).resolve().parents[1]
source=root/'vendor/wgpu-hal/src/vulkan/submit_trace.rs'
actual=source.read_text()
out=root/'logs/submit-trace-model';(out/'src').mkdir(parents=True,exist_ok=True)
extra=r'''
pub(super) fn test_case(case:&str, directory:&std::path::Path) {
 match case {
 "off" => {
  assert!(!sync_enabled());assert!(pipeline_label(Some("off")).is_none());
  assert!(Journal::begin(1,1,2,Some("off")).is_none());
  assert!(Submission::begin(1,0,0,"","",[(2,None)].into_iter()).is_none());
  assert!(!directory.exists());assert_eq!(LABEL_BYTES.load(Ordering::Relaxed),0);assert_eq!(LIVE_JOURNALS.load(Ordering::Relaxed),0);
 },
 "snapshot"=>{
  let mut a=Journal::begin(10,1,99,Some("encoder A")).unwrap();let first=a.epoch;
  let label=pipeline_label(Some("original old pipeline")).unwrap();a.pipeline("graphics",77,Some(&label));drop(label);
  let mut b=Journal::begin(10,1,99,Some("same raw new epoch")).unwrap();assert_ne!(b.epoch,first);
  let replacement=pipeline_label(Some("replacement pipeline")).unwrap();b.pipeline("graphics",77,Some(&replacement));drop(replacement);
  assert!(a.bytes.contains("original old pipeline")&&!a.bytes.contains("replacement"));
  assert!(b.bytes.contains("replacement")&&!b.bytes.contains("original"));assert_eq!(LABEL_BYTES.load(Ordering::Relaxed),0);
 },
 "budgets"=>{
  let text="x".repeat(MAX_LABEL_BYTES);let mut labels=Vec::new();
  for _ in 0..LABEL_BUDGET/MAX_LABEL_BYTES{let v=pipeline_label(Some(&text)).unwrap();assert!(!v.missing);labels.push(v);}
  let absent=pipeline_label(Some("exhausted")).unwrap();assert!(absent.missing);drop(absent);drop(labels);assert_eq!(LABEL_BYTES.load(Ordering::Relaxed),0);
  let capped=pipeline_label(Some(&"你".repeat(MAX_LABEL_BYTES))).unwrap();assert!(capped.truncated);assert!(capped.value.len()<=MAX_LABEL_BYTES);drop(capped);
  let mut journals=Vec::new();for i in 0..MAX_JOURNALS{let j=Journal::begin(1,1,i as u64,None).unwrap();assert!(j.slot);journals.push(j);}
  let mut missing=Journal::begin(1,1,100,None).unwrap();assert!(!missing.slot);missing.event(format_args!("not retained"));assert_eq!(missing.dropped,1);
  let sub=Submission::begin(1,1,0,"","",[(100,Some(&missing))].into_iter()).unwrap();assert!(sub.metadata.contains("journalMissing=true"));
  drop(sub);drop(missing);drop(journals);assert_eq!(LIVE_JOURNALS.load(Ordering::Relaxed),0);
  let mut j=Journal::begin(1,1,2,None).unwrap();for _ in 0..MAX_EVENTS+1{j.event(format_args!("event"));}assert_eq!(j.dropped,1);assert!(j.bytes.len()<=MAX_JOURNAL_BYTES);drop(j);
  let mut j=Journal::begin(1,1,2,None).unwrap();let big="x".repeat(8000);for _ in 0..300{j.event(format_args!("{big}"));}assert!(j.dropped>0);assert!(j.bytes.len()<=MAX_JOURNAL_BYTES);
 },
 "order_truncate"=>{
  let mut journals=Vec::new();let payload="x".repeat(8000);
  for i in 0..5{let mut j=Journal::begin(5,1,10+i,None).unwrap();for _ in 0..131{j.event(format_args!("{payload}"));}journals.push(j);}
  let sub=Submission::begin(9,123,55,"wait0","signal0",[(10,Some(&journals[0])),(999,None),(11,Some(&journals[1])),(12,Some(&journals[2])),(13,Some(&journals[3])),(14,Some(&journals[4]))].into_iter()).unwrap();
  assert_eq!(sub.missing,1);assert!(sub.file_truncated);assert!(sub.metadata.len()<MAX_SUBMIT_BYTES);
  let p0=sub.metadata.find("command[0] raw=0xa").unwrap();let p1=sub.metadata.find("command[1] raw=0x3e7").unwrap();let p2=sub.metadata.find("command[2] raw=0xb").unwrap();let events=sub.metadata.find("commandEvents index=0").unwrap();assert!(p0<p1&&p1<p2&&p2<events);
  assert!(sub.metadata.contains("commandCount=6")&&sub.metadata.contains("fileTruncated=true"));
 },
 "submit_failed"|"wait_failed"|"completed"=>{
  assert!(sync_enabled());let j=Journal::begin(1,1,2,None).unwrap();let sub=Submission::begin(9,1,0,"","",[(2,Some(&j))].into_iter()).unwrap();
  match case{ "submit_failed"=>sub.phase("after-vkQueueSubmit-error","Err(ERROR_DEVICE_LOST)"),"wait_failed"=>sub.phase("after-vkQueueWaitIdle-error","Err(ERROR_DEVICE_LOST)"),_=>sub.completed() }
  assert_eq!(directory.join("last-completed-submit.txt").exists(),case=="completed");
  let state=std::fs::read_to_string(directory.join("last-submit-state.txt")).unwrap();assert!(state.contains(if case=="completed"{"after-vkQueueWaitIdle-success"}else if case=="wait_failed"{"after-vkQueueWaitIdle-error"}else{"after-vkQueueSubmit-error"}));
 },
 _=>panic!("unknown test case"),
 }
}
'''
(out/'src/actual_submit_trace.rs').write_text(actual+extra)
(out/'src/main.rs').write_text('extern crate alloc;\nmod actual_submit_trace;\nfn main(){let a:Vec<_>=std::env::args().collect();actual_submit_trace::test_case(&a[1],std::path::Path::new(&a[2]));println!("PASS {}",a[1]);}\n')
registry=root.parent/'runtime-probe/.toolchain/cargo/registry/src/index.crates.io-1949cf8c6b5b557f'
log=next(registry.glob('log-0.4.34'))
(out/'Cargo.toml').write_text(f'[package]\nname="muguet-submit-trace-model"\nversion="0.0.0"\nedition="2021"\n[workspace]\n[dependencies]\nlog={{path={json.dumps(str(log))}}}\n')
env=dict(os.environ);env['CARGO_HOME']=str(root.parent/'runtime-probe/.toolchain/cargo');env['RUSTUP_HOME']=str(root.parent/'runtime-probe/.toolchain/rustup');env['PATH']=env['CARGO_HOME']+'/bin:'+env.get('PATH','')
build=subprocess.run(['cargo','build','--offline','--manifest-path',str(out/'Cargo.toml')],env=env,text=True,capture_output=True)
logtext=build.stdout+build.stderr;results=[]
if build.returncode==0:
 for case in ['off','snapshot','budgets','order_truncate','submit_failed','wait_failed','completed']:
  directory=out/('case-'+case)
  import shutil
  if directory.exists():shutil.rmtree(directory)
  caseenv=dict(env);caseenv.pop('MUGUET_SYNC_SUBMIT',None);caseenv.pop('MUGUET_SUBMIT_TRACE_DIR',None)
  if case!='off':caseenv.update(MUGUET_SYNC_SUBMIT='1',MUGUET_SUBMIT_TRACE_DIR=str(directory))
  run=subprocess.run([str(out/'target/debug/muguet-submit-trace-model'),case,str(directory)],env=caseenv,text=True,capture_output=True)
  results.append({'case':case,'exit':run.returncode});logtext+=run.stdout+run.stderr
(root/'logs/submit-trace-tests.log').write_text(logtext)
report={'method':'Entire real submit_trace.rs copied verbatim, appended private-state assertions only. Each case runs a fresh process to exercise OnceLock configuration; real bounded files/fsync and owned labels/journals. No Vulkan queue call mocked as real execution.','sourceSHA256':hashlib.sha256(actual.encode()).hexdigest(),'buildExit':build.returncode,'cases':results,'allPassed':build.returncode==0 and len(results)==7 and all(r['exit']==0 for r in results)}
(out/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
raise SystemExit(0 if report['allPassed'] else 1)
