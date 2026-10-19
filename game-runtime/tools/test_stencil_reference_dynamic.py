#!/usr/bin/env python3
"""Exercise exact HAL dynamic-state construction with real wgpu-types/ash enums."""
from pathlib import Path
import hashlib,json,os,subprocess
root=Path(__file__).resolve().parents[1]
p=root/'vendor/wgpu-hal/src/vulkan/device.rs';s=p.read_text()
a=s.index('        let dynamic_states = [',s.index('unsafe fn create_render_pipeline'))
b=s.index('        let mut compatible_rp_key',a);actual=s[a:b]
# These unchanged consumers are required: value-sensitive pipelines must still
# receive reference updates, including when switching from a static pipeline.
core=(root/'vendor/wgpu-core/src/device/resource.rs').read_text()
render=(root/'vendor/wgpu-core/src/command/render.rs').read_text()
command=(root/'vendor/wgpu-hal/src/vulkan/command.rs').read_text()
conv=(root/'vendor/wgpu-hal/src/vulkan/conv.rs').read_text()
assert 'if ds.stencil.is_enabled() && ds.stencil.needs_ref_value() {' in core
assert 'if pipeline.flags.contains(PipelineFlags::STENCIL_REFERENCE) {' in render
assert '.set_stencil_reference(state.stencil_reference)' in render
assert 'state.pass.base.raw_encoder.set_stencil_reference(value)' in render
hal_set=command[command.index('    unsafe fn set_stencil_reference('):command.index('    unsafe fn set_blend_constants(')]
assert 'cmd_set_stencil_reference(' in hal_set and 'value,' in hal_set and 'if ' not in hal_set
mapped=conv[conv.index('pub fn map_stencil_face('):conv.index('fn map_blend_factor(')]
assert 'reference: 0,' in mapped
out=root/'logs/stencil-reference-model';(out/'src').mkdir(parents=True,exist_ok=True)
(out/'Cargo.toml').write_text('''[package]
name="stencil-reference-model"
version="0.0.0"
edition="2021"
[workspace]
[dependencies]
ash={version="=0.38.0",default-features=false}
wgpu-types={version="=30.0.1",default-features=false,features=["std"]}
''')
body=r'''
use ash::vk;
use wgpu_types as wgt;
struct Desc { depth_stencil: Option<wgt::DepthStencilState> }
fn select(stencil:Option<wgt::StencilState>)->Vec<vk::DynamicState>{
 let desc=Desc{depth_stencil:stencil.map(|stencil|wgt::DepthStencilState{
 format:wgt::TextureFormat::Depth24PlusStencil8,depth_write_enabled:Some(false),
 depth_compare:Some(wgt::CompareFunction::Always),stencil,bias:Default::default()})};
 ACTUAL
 let vk_info=vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
 assert_eq!(vk_info.dynamic_state_count,dynamic_states.len()as u32);
 assert!(vk_info.p_dynamic_states==dynamic_states.as_ptr());
 unsafe{std::slice::from_raw_parts(vk_info.p_dynamic_states,vk_info.dynamic_state_count as usize).to_vec()}
}
fn semantic_ref(f:wgt::StencilFaceState)->bool{
 use wgt::{CompareFunction as C,StencilOperation as O};
 !matches!(f.compare,C::Never|C::Always)||f.fail_op==O::Replace||f.depth_fail_op==O::Replace||f.pass_op==O::Replace
}
fn verify(s:Option<wgt::StencilState>){
 let required=s.as_ref().is_some_and(|s|s.is_enabled()&&(semantic_ref(s.front)||semantic_ref(s.back)));
 let selected=select(s);
 let expected=[vk::DynamicState::VIEWPORT,vk::DynamicState::SCISSOR,vk::DynamicState::BLEND_CONSTANTS,vk::DynamicState::STENCIL_REFERENCE];
 let n=if cfg!(target_env="ohos")&&!required{3}else{4};
 assert!(selected==expected[..n]);
}
#[test]fn no_depth_and_disabled_stencil(){verify(None);verify(Some(wgt::StencilState::default()));
 let s=wgt::StencilState{front:wgt::StencilFaceState{pass_op:wgt::StencilOperation::Replace,..wgt::StencilFaceState::IGNORE},back:wgt::StencilFaceState::IGNORE,read_mask:0,write_mask:0};verify(Some(s));}
#[test]fn all_compare_and_operation_positions_both_faces_masks(){
 use wgt::{CompareFunction as C,StencilOperation as O};
 let comps=[C::Never,C::Less,C::Equal,C::LessEqual,C::Greater,C::NotEqual,C::GreaterEqual,C::Always];
 let ops=[O::Keep,O::Zero,O::Replace,O::Invert,O::IncrementClamp,O::DecrementClamp,O::IncrementWrap,O::DecrementWrap];
 let mut count=0;
 for compare in comps{for fail_op in ops{for depth_fail_op in ops{for pass_op in ops{
  let face=wgt::StencilFaceState{compare,fail_op,depth_fail_op,pass_op};
  for(read_mask,write_mask)in[(0,0),(0xff,0),(0,0xff),(0xff,0xff)]{
   for back in[false,true]{let(sfront,sback)=if back{(wgt::StencilFaceState::IGNORE,face)}else{(face,wgt::StencilFaceState::IGNORE)};
    verify(Some(wgt::StencilState{front:sfront,back:sback,read_mask,write_mask}));count+=1;}
  }
 }}}}assert_eq!(count,32768);
}
#[test]fn reference_sensitive_compare_or_replace_on_either_face_is_dynamic(){
 use wgt::{StencilFaceState as F,StencilOperation as O,CompareFunction as C};
 let free=F{compare:C::Always,pass_op:O::IncrementClamp,..F::IGNORE};
 let sensitive=[F{compare:C::Less,..F::IGNORE},F{fail_op:O::Replace,..F::IGNORE},F{depth_fail_op:O::Replace,..F::IGNORE},F{pass_op:O::Replace,..F::IGNORE}];
 for f in sensitive{for(front,back)in[(f,free),(free,f),(f,f)]{verify(Some(wgt::StencilState{front,back,read_mask:255,write_mask:255}));}}
}
'''.replace('ACTUAL',actual)
(out/'src/lib.rs').write_text(body)
env=dict(os.environ);env['CARGO_HOME']=str(root.parent/'runtime-probe/.toolchain/cargo');env['RUSTUP_HOME']=str(root.parent/'runtime-probe/.toolchain/rustup');env['PATH']=env['CARGO_HOME']+'/bin:'+env.get('PATH','')
logs=[];cases=[]
for mode in ['host','ohos']:
 env['RUSTFLAGS']='--cfg target_env="ohos" -Aexplicit_builtin_cfgs_in_flags' if mode=='ohos' else ''
 r=subprocess.run(['cargo','test','--offline','--manifest-path',str(out/'Cargo.toml'),'--target','aarch64-apple-darwin'],env=env,capture_output=True,text=True)
 logs.extend([r.stdout,r.stderr]);cases.append({'mode':mode,'exitCode':r.returncode})
(root/'logs/stencil-reference-tests.log').write_text(''.join(logs))
report={'status':'pass'if all(c['exitCode']==0 for c in cases)else'fail','sourceSha256':hashlib.sha256(p.read_bytes()).hexdigest(),'actualSelectionSha256':hashlib.sha256(actual.encode()).hexdigest(),'cases':cases,'faceConfigurationsPerMode':32768,'testsPerMode':3,'scope':'Exact production HAL array/gate and VkPipelineDynamicStateCreateInfo, actual wgpu-types30.0.1/ash0.38.0. No driver execution. Existing core predicate/pipeline-bind/set-reference commands and static mappedzero asserted intact.','halSetReferenceSha256':hashlib.sha256(hal_set.encode()).hexdigest(),'mapStencilFaceSha256':hashlib.sha256(mapped.encode()).hexdigest()}
(out/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2));raise SystemExit(0 if report['status']=='pass' else 1)
