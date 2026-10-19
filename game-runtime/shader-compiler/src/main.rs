use std::{collections::BTreeMap, path::Path};
fn pack(text:&str)->String {
 let decl=regex::Regex::new(r"@group\((\d+)u?\)\s*@binding\((\d+)u?\)\s*var<uniform>\s+(\w+)\s*:\s*(\w+)\s*;").unwrap();
 let mut members=Vec::new();let mut group=0;
 let out=decl.replace_all(text,|c:&regex::Captures|{let b:u32=c[2].parse().unwrap();if b==15{return c[0].to_string();}group=c[1].parse().unwrap();members.push((b,c[3].to_string(),c[4].to_string()));String::new()});
 if members.is_empty(){return text.into();}
 members.sort_by_key(|m|m.0);let mut out=out.into_owned();
 for (_,name,_) in &members {out=regex::Regex::new(&format!(r"\b{}\b",regex::escape(name))).unwrap().replace_all(&out,format!("cbpack_v.{name}").as_str()).into_owned();}
 let st=format!("struct cbpack_t {{\n{}\n}}\n@group({group}u) @binding(0u) var<uniform> cbpack_v : cbpack_t;\n",members.iter().map(|(_,n,t)|format!("  {n} : {t},")).collect::<Vec<_>>().join("\n"));
 let first=regex::Regex::new(r"\n(?:@\w+|fn)\s").unwrap();match first.find(&out){Some(at)=>format!("{}\n{}{}",&out[..at.start()],st,&out[at.start()..]),None=>out+&st}
}
fn main()->Result<(),Box<dyn std::error::Error>>{
 let a:Vec<_>=std::env::args().collect();if a.len()<5{return Err("usage: shader-review INPUT OUTPUT STAGE VERSION [pack|roundtrip]".into());}
 let input=Path::new(&a[1]);let stage=match a[3].as_str(){"Vertex"=>naga::ShaderStage::Vertex,"Fragment"=>naga::ShaderStage::Fragment,"Compute"=>naga::ShaderStage::Compute,_=>return Err("unknown stage".into())};
 let version=match a[4].as_str(){"1.3"=>(1,3),"1.4"=>(1,4),"1.6"=>(1,6),_=>return Err("unknown version".into())};
 let module={
  let raw=std::fs::read_to_string(input)?;let source=if a.get(5).is_some_and(|s|s=="pack"){pack(&raw)}else{raw};
  std::fs::write(Path::new(&a[2]).with_extension("wgsl"),&source)?;
  naga::front::wgsl::parse_str(&source).map_err(|e|e.emit_to_string(&source))?
 };
 let info=naga::valid::Validator::new(naga::valid::ValidationFlags::all(),naga::valid::Capabilities::all()).validate(&module)?;
 let (module,info)=naga::back::pipeline_constants::process_overrides(&module,&info,Some((stage,"main")),&Default::default())?;
 let mut options=naga::back::spv::Options::default();options.lang_version=version;options.flags=naga::back::spv::WriterFlags::LABEL_VARYINGS|naga::back::spv::WriterFlags::FORCE_POINT_SIZE;
 options.bounds_check_policies=naga::proc::BoundsCheckPolicies{index:naga::proc::BoundsCheckPolicy::Restrict,buffer:naga::proc::BoundsCheckPolicy::Restrict,image_load:naga::proc::BoundsCheckPolicy::Restrict,binding_array:naga::proc::BoundsCheckPolicy::Unchecked};
 options.zero_initialize_workgroup_memory=naga::back::spv::ZeroInitializeWorkgroupMemoryMode::Native;options.fake_missing_bindings=false;
 let mut groups:BTreeMap<u32,Vec<u32>>=BTreeMap::new();for (_,g) in module.global_variables.iter(){if let Some(b)=g.binding{groups.entry(b.group).or_default().push(b.binding);}}
 for(group,mut bindings)in groups{bindings.sort();bindings.dedup();for(i,binding)in bindings.into_iter().enumerate(){options.binding_map.insert(naga::ResourceBinding{group,binding},naga::back::spv::BindingInfo{descriptor_set:group,binding:i as u32,binding_array_size:None});}}
 let pipeline=naga::back::spv::PipelineOptions{entry_point:"main".into(),shader_stage:stage};let words=naga::back::spv::write_vec(&module,&info,&options,Some(&pipeline))?;
 std::fs::write(&a[2],words.iter().flat_map(|w|w.to_le_bytes()).collect::<Vec<_>>())?;println!("{} words; Naga write_vec lang_version={version:?}; stage={stage:?}",words.len());Ok(())
}
