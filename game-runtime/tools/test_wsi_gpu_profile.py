#!/usr/bin/env python3
from pathlib import Path
import os,subprocess,json,hashlib,shutil
root=Path(__file__).resolve().parents[1];out=root/'logs/wsi-profile-model';out.mkdir(parents=True,exist_ok=True)
p=root/'vendor/wgpu-hal/src/vulkan/wsi_profile.rs';src=p.read_text()
extra=r'''
pub fn test_case(case:&str,dir:&std::path::Path){
 if case=="off"{let _a=span(Kind::ReuseSubmissionFence);let _b=span(Kind::AcquireNextImage);assert!(!dir.exists());STATS.with(|s|assert!(s.borrow().is_none()));}
 else {
  let _=config();STATS.with(|s|*s.borrow_mut()=Some(Stats{since:Instant::now()-std::time::Duration::from_secs(11),calls:[2,3],ns:[200,300],max:[100,100],windows:if case=="cap"{MAX_WINDOWS-1}else{0}}));
  drop(span(Kind::ReuseSubmissionFence));
  let text=std::fs::read_to_string(dir.join("wsi.jsonl")).unwrap();assert!(text.contains("reuseSubmissionFence")&&text.contains("vkAcquireNextImageKHR")&&text.contains(&format!("\"processId\":{}",std::process::id())));assert!(text.contains("\"calls\":3"));
  if case=="cap"{assert!(text.contains("\"truncated\":true"));STATS.with(|s|s.borrow_mut().as_mut().unwrap().since=Instant::now()-std::time::Duration::from_secs(11));drop(span(Kind::AcquireNextImage));assert_eq!(text,std::fs::read_to_string(dir.join("wsi.jsonl")).unwrap());}
 }
 println!("PASS {case}");
}
'''
(out/'actual.rs').write_text(src+extra);(out/'main.rs').write_text('mod actual;fn main(){let a:Vec<_>=std::env::args().collect();actual::test_case(&a[1],std::path::Path::new(&a[2]));}')
env=dict(os.environ);env['CARGO_HOME']=str(root.parent/'runtime-probe/.toolchain/cargo');env['RUSTUP_HOME']=str(root.parent/'runtime-probe/.toolchain/rustup');env['PATH']=env['CARGO_HOME']+'/bin:'+env.get('PATH','')
r=subprocess.run(['rustc','--edition=2021',str(out/'main.rs'),'-o',str(out/'wsi-test')],env=env,text=True,capture_output=True);log=r.stdout+r.stderr;cases=[]
if r.returncode==0:
 for case in ['off','on','cap']:
  directory=out/('case-'+case)
  if directory.exists():shutil.rmtree(directory)
  e=dict(env);e.pop('MUGUET_GPU_PROFILE',None);e.pop('MUGUET_GPU_PROFILE_DIR',None)
  if case!='off':e.update(MUGUET_GPU_PROFILE='1',MUGUET_GPU_PROFILE_DIR=str(directory))
  x=subprocess.run([str(out/'wsi-test'),case,str(directory)],env=e,text=True,capture_output=True);log+=x.stdout+x.stderr;cases.append({'case':case,'exit':x.returncode})
model=root/'src/gpu_profile_model.rs';math=subprocess.run(['rustc','--edition=2021','--test',str(model),'-o',str(out/'math-tests')],env=env,text=True,capture_output=True);log+=math.stdout+math.stderr
mathrun=subprocess.run([str(out/'math-tests')],env=env,text=True,capture_output=True) if math.returncode==0 else math;log+=mathrun.stdout+mathrun.stderr
# Verify wrappers preserve one call each, identical argument/error control flow, no idle or submit.
native=(root/'vendor/wgpu-hal/src/vulkan/swapchain/native.rs').read_text();before=(root.parent/'native-platform/evidence/gpu-profiling/native.rs.before').read_text()
a=native[native.index('        let completed = {'):native.index('        if !completed {')]
b=native[native.index('        let (index, suboptimal) = match unsafe {'):native.index('            // We treat `VK_SUBOPTIMAL')]
assert a.count('self.device.wait_for_fence(')==1 and a.count(')?')==1
assert b.count('self.functor.acquire_next_image(')==1
assert all(t not in a+b for t in ['queue_wait_idle','device_wait_idle','queue_submit','reset_fences'])
assert native.count('self.device.wait_for_fence(')==before.count('self.device.wait_for_fence(')
assert native.count('self.functor.acquire_next_image(')==before.count('self.functor.acquire_next_image(')
report={'method':'Entire production WSI module copied verbatim, private assertions appended. Three isolated processes verify OFF no directory/TLS metrics, ON two named phase counters, explicit bounded 360-window truncation. Real timestamp math source rustc --test. Native acquire source guards retain one existing wait and acquire call with original error operator. No actual device timing claimed.','sourceSHA256':hashlib.sha256(src.encode()).hexdigest(),'buildExit':r.returncode,'cases':cases,'mathExit':mathrun.returncode,'allPassed':r.returncode==0 and len(cases)==3 and all(c['exit']==0 for c in cases) and mathrun.returncode==0}
(root/'logs/wsi-gpu-profile-tests.log').write_text(log);(out/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2));raise SystemExit(0 if report['allPassed'] else 1)
