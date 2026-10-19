#!/usr/bin/env python3
"""Test the production VkCreateInfo construction verbatim with actual ash structs."""
from pathlib import Path
import hashlib,json,os,subprocess
r=Path(__file__).resolve().parents[1];p=r/'vendor/wgpu-hal/src/vulkan/device.rs';s=p.read_text()
start=s.index('        let vk_info = vk::DescriptorSetLayoutCreateInfo::default()',s.index('unsafe fn create_bind_group_layout('))
end=s.index('        let raw = unsafe {',start);actual=s[start:end]
body=r'''
use ash::vk;
fn verify(vk_bindings:Vec<vk::DescriptorSetLayoutBinding<'static>>,binding_flags:Vec<vk::DescriptorBindingFlags>,contains_binding_arrays:bool){
ACTUAL_CONSTRUCTION
 assert_eq!(vk_info.s_type,vk::StructureType::DESCRIPTOR_SET_LAYOUT_CREATE_INFO);
 assert_eq!(vk_info.binding_count,vk_bindings.len()as u32);assert_eq!(vk_info.p_bindings,vk_bindings.as_ptr());
 assert_eq!(vk_info.flags,if contains_binding_arrays{vk::DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL}else{vk::DescriptorSetLayoutCreateFlags::empty()});
 if cfg!(target_env="ohos")&&binding_flags.is_empty(){assert!(vk_info.p_next.is_null());}
 else{unsafe{
  assert!(!vk_info.p_next.is_null());let ext=&*(vk_info.p_next as *const vk::DescriptorSetLayoutBindingFlagsCreateInfo);
  assert_eq!(ext.s_type,vk::StructureType::DESCRIPTOR_SET_LAYOUT_BINDING_FLAGS_CREATE_INFO);assert!(ext.p_next.is_null());
  assert_eq!(ext.binding_count,binding_flags.len()as u32);assert_eq!(ext.p_binding_flags,binding_flags.as_ptr());
  if ext.binding_count!=0{assert_eq!(std::slice::from_raw_parts(ext.p_binding_flags,ext.binding_count as usize),binding_flags.as_slice());}
 }}
 if vk_info.binding_count!=0{unsafe{
  let copied=std::slice::from_raw_parts(vk_info.p_bindings,vk_info.binding_count as usize);
  for(a,b)in copied.iter().zip(&vk_bindings){assert_eq!((a.binding,a.descriptor_type,a.descriptor_count,a.stage_flags,a.p_immutable_samplers),(b.binding,b.descriptor_type,b.descriptor_count,b.stage_flags,b.p_immutable_samplers));}
 }}
}
#[test]fn empty_layout_omits_only_the_ohos_empty_flags_extension(){
 let b=Vec::<vk::DescriptorSetLayoutBinding>::new();let f=Vec::<vk::DescriptorBindingFlags>::new();
 assert!(!b.as_ptr().is_null()&&!f.as_ptr().is_null());assert_eq!(b.as_ptr()as usize,std::mem::align_of::<vk::DescriptorSetLayoutBinding>());assert_eq!(f.as_ptr()as usize,std::mem::align_of::<vk::DescriptorBindingFlags>());
 verify(b,f,false);
}
#[test]fn nonempty_zero_flags_chain_keeps_all_fields_and_pointers(){
 let bindings=vec![vk::DescriptorSetLayoutBinding::default().binding(0).descriptor_type(vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC).descriptor_count(1).stage_flags(vk::ShaderStageFlags::VERTEX),vk::DescriptorSetLayoutBinding::default().binding(1).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT)];
 verify(bindings,vec![vk::DescriptorBindingFlags::empty();2],false);
}
#[test]fn nonempty_array_flags_and_update_after_bind_pool_are_unchanged(){
 let bindings=vec![vk::DescriptorSetLayoutBinding::default().binding(0).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).descriptor_count(4).stage_flags(vk::ShaderStageFlags::COMPUTE)];
 verify(bindings,vec![vk::DescriptorBindingFlags::UPDATE_AFTER_BIND|vk::DescriptorBindingFlags::PARTIALLY_BOUND],true);
}
'''.replace('ACTUAL_CONSTRUCTION',actual)
out=r/'logs/empty-bind-group-layout-model';out.mkdir(exist_ok=True);rs=out/'actual-create-info-tests.rs';rs.write_text(body)
deps=r/'logs/device-fault-collector-model/target/debug/deps';ash=next(deps.glob('libash-*.rlib'))
env=dict(os.environ);env['CARGO_HOME']=str(r.parent/'runtime-probe/.toolchain/cargo');env['RUSTUP_HOME']=str(r.parent/'runtime-probe/.toolchain/rustup');env['PATH']=env['CARGO_HOME']+'/bin:'+env.get('PATH','')
logs=[];cases=[]
for mode in ['ohos','host']:
 binary=out/('tests-'+mode)
 cmd=['rustc','--edition=2021','--test',str(rs),'--extern','ash='+str(ash),'-L','dependency='+str(deps),'-o',str(binary)]
 if mode=='ohos':cmd+=['-Aexplicit_builtin_cfgs_in_flags','--cfg','target_env="ohos"']
 build=subprocess.run(cmd,env=env,text=True,capture_output=True);logs+=[build.stdout,build.stderr]
 if build.returncode:cases.append({'mode':mode,'buildExit':build.returncode});continue
 run=subprocess.run([str(binary)],text=True,capture_output=True);logs+=[run.stdout,run.stderr];cases.append({'mode':mode,'buildExit':0,'testsExit':run.returncode})
(r/'logs/empty-bind-group-layout-tests.log').write_text(''.join(logs))
report={'method':'Production create-info construction extracted verbatim, real cached ash0.38 structs. Three shape tests in OHOS-cfg and host-cfg; no driver call. Empty Vec pointers are checked nonnull and equal native type alignment.','sourceSHA256':hashlib.sha256(s.encode()).hexdigest(),'extractedSHA256':hashlib.sha256(actual.encode()).hexdigest(),'cases':cases,'allPassed':len(cases)==2 and all(c.get('testsExit')==0 for c in cases)}
(out/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2));raise SystemExit(0 if report['allPassed'] else 1)
