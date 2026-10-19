//! Marker-only original/optimized compute comparison; never enters normal game.
use anyhow::{Context,Result};
use serde_json::{json,Value};
use sha2::{Digest,Sha256};
use std::{cell::RefCell,ffi::{CStr,CString},os::raw::c_char,path::Path,
    fs::File,io::Write,sync::{Arc,Mutex,mpsc,atomic::{AtomicBool,Ordering}},time::{Duration,Instant}};
use wgpu::*;
#[path="compute_depth_model.rs"] mod model;
#[path="depth_downsample.rs"] mod transform;

thread_local!{static RESULT:RefCell<CString>=RefCell::new(CString::new("{}").unwrap());}
static RUNNING:AtomicBool=AtomicBool::new(false);
struct RunGuard;
impl Drop for RunGuard{fn drop(&mut self){RUNNING.store(false,Ordering::Release);}}
struct Trace{file:File,start:Instant,cases:u32,words:u64,sentinels:u64}
impl Trace{
    fn new(root:&Path)->Result<Self>{std::fs::create_dir_all(root)?;Ok(Self{file:File::create(root.join("depth-downsample-probe-trace.jsonl"))?,start:Instant::now(),cases:0,words:0,sentinels:0})}
    fn step(&mut self,stage:&str,detail:Value)->Result<()>{serde_json::to_writer(&mut self.file,&json!({"stage":stage,"detail":detail,"elapsedMs":self.start.elapsed().as_millis(),"casesCompleted":self.cases,"u32WordsCompared":self.words}))?;self.file.write_all(b"\n")?;self.file.sync_all()?;Ok(())}
}
fn errors_seen(errors:&Mutex<Vec<String>>)->Result<()>{let e=errors.lock().map_err(|_|anyhow::anyhow!("error lock poisoned"))?;anyhow::ensure!(e.is_empty(),"wgpu error: {}",e.join("; "));Ok(())}
fn buffer(device:&Device,queue:&Queue,label:&str,bytes:&[u8])->Buffer{
    let b=device.create_buffer(&BufferDescriptor{label:Some(label),size:bytes.len()as u64,usage:BufferUsages::UNIFORM|BufferUsages::COPY_DST,mapped_at_creation:false});queue.write_buffer(&b,0,bytes);b
}
fn texture(device:&Device,label:&str,width:u32,height:u32,format:TextureFormat,usage:TextureUsages)->Texture{
    device.create_texture(&TextureDescriptor{label:Some(label),size:Extent3d{width,height,depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:TextureDimension::D2,format,usage,view_formats:&[]})
}
fn upload(queue:&Queue,tex:&Texture,width:u32,height:u32,bpp:u32,bytes:&[u8]){
    queue.write_texture(TexelCopyTextureInfo{texture:tex,mip_level:0,origin:Origin3d::ZERO,aspect:TextureAspect::All},bytes,TexelCopyBufferLayout{offset:0,bytes_per_row:Some(width*bpp),rows_per_image:Some(height)},Extent3d{width,height,depth_or_array_layers:1});
}
struct Pipelines{original:ComputePipeline,optimized:ComputePipeline,layout:BindGroupLayout,format:TextureFormat,bpp:u32,name:&'static str}
fn pipelines(device:&Device,original:&str,optimized:&str,format:TextureFormat,bpp:u32,name:&'static str)->Pipelines{
    let entries=[
        BindGroupLayoutEntry{binding:10,visibility:ShaderStages::COMPUTE,ty:BindingType::Buffer{ty:BufferBindingType::Uniform,has_dynamic_offset:false,min_binding_size:None},count:None},
        BindGroupLayoutEntry{binding:11,visibility:ShaderStages::COMPUTE,ty:BindingType::Buffer{ty:BufferBindingType::Uniform,has_dynamic_offset:false,min_binding_size:None},count:None},
        BindGroupLayoutEntry{binding:15,visibility:ShaderStages::COMPUTE,ty:BindingType::Buffer{ty:BufferBindingType::Uniform,has_dynamic_offset:false,min_binding_size:None},count:None},
        BindGroupLayoutEntry{binding:39,visibility:ShaderStages::COMPUTE,ty:BindingType::Texture{sample_type:TextureSampleType::Float{filterable:false},view_dimension:TextureViewDimension::D2,multisampled:false},count:None},
        BindGroupLayoutEntry{binding:40,visibility:ShaderStages::COMPUTE,ty:BindingType::Texture{sample_type:TextureSampleType::Float{filterable:false},view_dimension:TextureViewDimension::D2,multisampled:false},count:None},
        BindGroupLayoutEntry{binding:161,visibility:ShaderStages::COMPUTE,ty:BindingType::StorageTexture{access:StorageTextureAccess::WriteOnly,format,view_dimension:TextureViewDimension::D2},count:None},
    ];
    let layout=device.create_bind_group_layout(&BindGroupLayoutDescriptor{label:Some("depth-diff-identical-layout"),entries:&entries});
    let pl=device.create_pipeline_layout(&PipelineLayoutDescriptor{label:Some("depth-diff-layout"),bind_group_layouts:&[Some(&layout)],immediate_size:0});
    let build=|source:&str,label:&str,zero_init:bool|{
        let text=source.replace("texture_storage_2d<rgba32float, write>",&format!("texture_storage_2d<{name}, write>"));
        let module=device.create_shader_module(ShaderModuleDescriptor{label:Some(label),source:ShaderSource::Wgsl(text.into())});
        device.create_compute_pipeline(&ComputePipelineDescriptor{label:Some(label),layout:Some(&pl),module:&module,entry_point:Some("main"),compilation_options:PipelineCompilationOptions{zero_initialize_workgroup_memory:zero_init,..Default::default()},cache:None})
    };
    let original=build(original,"depth-diff-v2-zero-init-on",true);let optimized=build(optimized,"depth-diff-v2-zero-init-off",false);
    Pipelines{original,optimized,layout,format,bpp,name}
}
#[allow(clippy::too_many_arguments)]
fn run_case(device:&Device,queue:&Queue,pipe:&Pipelines,width:u32,height:u32,pattern:usize,repeat:u32,trace:&mut Trace,errors:&Mutex<Vec<String>>)->Result<Value>{
    let(gx,gy)=model::groups(width,height);let(ow,oh)=(gx+1,gy+1);let bpp=pipe.bpp;
    let(depth_bytes,aux_bytes)=model::input_words(width,height,pattern,repeat);
    let(depth,aux)=(texture(device,"depth-diff-input-depth",width,height,TextureFormat::R32Float,TextureUsages::TEXTURE_BINDING|TextureUsages::COPY_DST),texture(device,"depth-diff-input-aux",width,height,TextureFormat::Rgba32Float,TextureUsages::TEXTURE_BINDING|TextureUsages::COPY_DST));
    upload(queue,&depth,width,height,4,&depth_bytes);upload(queue,&aux,width,height,16,&aux_bytes);
    let dv=depth.create_view(&TextureViewDescriptor::default());let av=aux.create_view(&TextureViewDescriptor::default());
    let(cb10,cb11,constants)=model::uniform_words(width,height);
    let cb10=buffer(device,queue,"depth-diff-cb10",&cb10);let cb11=buffer(device,queue,"depth-diff-cb11",&cb11);let constants=buffer(device,queue,"depth-diff-fixed-binding15",&constants);
    let sentinel=model::sentinel(bpp);let output_initial:Vec<u8>=sentinel.iter().copied().cycle().take((ow*oh*bpp)as usize).collect();
    let pitch=(ow*bpp).div_ceil(256)*256;let prefix=256u64;let size=prefix+u64::from(pitch*oh)+256;
    let mut readbacks=Vec::new();let mut outputs=Vec::new();let mut walls=Vec::new();
    for (which,pipeline) in [("original",&pipe.original),("optimized",&pipe.optimized)]{
        let output=texture(device,"depth-diff-tile-output",ow,oh,pipe.format,TextureUsages::STORAGE_BINDING|TextureUsages::COPY_SRC|TextureUsages::COPY_DST);
        upload(queue,&output,ow,oh,bpp,&output_initial);let view=output.create_view(&TextureViewDescriptor::default());
        let group=device.create_bind_group(&BindGroupDescriptor{label:Some("depth-diff-bindings"),layout:&pipe.layout,entries:&[
            BindGroupEntry{binding:10,resource:cb10.as_entire_binding()},BindGroupEntry{binding:11,resource:cb11.as_entire_binding()},BindGroupEntry{binding:15,resource:constants.as_entire_binding()},
            BindGroupEntry{binding:39,resource:BindingResource::TextureView(&dv)},BindGroupEntry{binding:40,resource:BindingResource::TextureView(&av)},BindGroupEntry{binding:161,resource:BindingResource::TextureView(&view)},
        ]});
        let read=device.create_buffer(&BufferDescriptor{label:Some("depth-diff-readback-with-guards"),size,usage:BufferUsages::MAP_READ|BufferUsages::COPY_DST,mapped_at_creation:false});queue.write_buffer(&read,0,&vec![0xcdu8;size as usize]);
        let mut enc=device.create_command_encoder(&CommandEncoderDescriptor{label:Some("depth-diff-dispatch")});
        {let mut cp=enc.begin_compute_pass(&ComputePassDescriptor{label:Some(which),timestamp_writes:None});cp.set_pipeline(pipeline);cp.set_bind_group(0,&group,&[]);cp.dispatch_workgroups(gx,gy,1);}
        enc.copy_texture_to_buffer(TexelCopyTextureInfo{texture:&output,mip_level:0,origin:Origin3d::ZERO,aspect:TextureAspect::All},TexelCopyBufferInfo{buffer:&read,layout:TexelCopyBufferLayout{offset:prefix,bytes_per_row:Some(pitch),rows_per_image:Some(oh)}},Extent3d{width:ow,height:oh,depth_or_array_layers:1});
        trace.step("before-submit",json!({"variant":which,"resolution":[width,height],"groups":[gx,gy,1],"format":pipe.name,"pattern":model::PATTERNS[pattern],"repeat":repeat,"zeroInitializeWorkgroupMemory":which=="original"}))?;
        let start=Instant::now();let idx=queue.submit([enc.finish()]);let(tx,rx)=mpsc::channel();read.slice(..).map_async(MapMode::Read,move|r|{let _=tx.send(r.map_err(|e|e.to_string()));});
        device.poll(PollType::Wait{submission_index:Some(idx),timeout:Some(Duration::from_secs(30))})?;
        rx.recv_timeout(Duration::from_secs(5)).context("depth readback callback timeout")?.map_err(anyhow::Error::msg)?;errors_seen(errors)?;
        walls.push(start.elapsed().as_secs_f64()*1000.0);
        let bytes={let mapped=read.slice(..).get_mapped_range().context("depth readback range")?;mapped.to_vec()};read.unmap();
        anyhow::ensure!(bytes[..prefix as usize].iter().chain(bytes[(prefix+u64::from(pitch*oh))as usize..].iter()).all(|&b|b==0xcd),"{which} readback prefix/tail sentinel changed");
        let mut tight=Vec::with_capacity((ow*oh*bpp)as usize);
        for y in 0..oh as usize{let off=prefix as usize+y*pitch as usize;tight.extend_from_slice(&bytes[off..off+(ow*bpp)as usize]);anyhow::ensure!(bytes[off+(ow*bpp)as usize..off+pitch as usize].iter().all(|&b|b==0xcd),"{which} readback row padding changed");}
        for y in 0..oh{for x in 0..ow{if x<gx&&y<gy{continue;}let off=((y*ow+x)*bpp)as usize;anyhow::ensure!(tight[off..off+bpp as usize]==sentinel,"{which} unwritten output border changed x={x} y={y}");trace.sentinels+=u64::from(bpp/4);}}
        readbacks.push(tight);outputs.push(output);
        trace.step("after-readback",json!({"variant":which,"resolution":[width,height],"format":pipe.name,"pattern":model::PATTERNS[pattern]}))?;
    }
    let mut changed=0u64;
    for (word,(a,b)) in readbacks[0].chunks_exact(4).zip(readbacks[1].chunks_exact(4)).enumerate(){
        let(a,b)=(u32::from_le_bytes(a.try_into().unwrap()),u32::from_le_bytes(b.try_into().unwrap()));
        if a!=b{let byte=word*4;let pixel=byte/bpp as usize;let detail=json!({"resolution":[width,height],"format":pipe.name,"pattern":model::PATTERNS[pattern],"repeat":repeat,"x":pixel%ow as usize,"y":pixel/ow as usize,"wordWithinTexel":byte%bpp as usize/4,"original":format!("0x{a:08x}"),"optimized":format!("0x{b:08x}")});trace.step("counterexample",detail.clone())?;anyhow::bail!("raw u32 mismatch {detail}");}
        let pixel=(word*4/bpp as usize)as u32;if pixel%ow<gx&&pixel/ow<gy{let off=word*4%bpp as usize;if a!=u32::from_le_bytes(sentinel[off..off+4].try_into().unwrap()){changed+=1;}}
        trace.words+=1;
    }
    anyhow::ensure!(changed>0,"both outputs retained only initialization sentinel; no effective shader output");
    let non_sentinel_tiles=(0..gy).flat_map(|y|(0..gx).map(move|x|(x,y))).filter(|&(x,y)|{
        let off=((y*ow+x)*bpp)as usize;readbacks[0][off..off+bpp as usize]!=sentinel
    }).count();
    trace.cases+=1;let result=json!({"resolution":[width,height],"groups":[gx,gy,1],"outputWithSentinelBorder":[ow,oh],"pattern":model::PATTERNS[pattern],"repeat":repeat,"format":pipe.name,"u32WordsCompared":ow*oh*bpp/4,"nominalDispatchTileTexels":gx*gy,"nonSentinelTileCount":non_sentinel_tiles,"originalWallMs":walls[0],"optimizedWallMs":walls[1],"originalZeroInitializeWorkgroupMemory":true,"optimizedZeroInitializeWorkgroupMemory":false,"timingScope":"CPU submit-to-readback wall including explicit probe wait; not GPU timestamps"});
    trace.step("case-pass",result.clone())?;drop(outputs);Ok(result)
}
fn run(root:&Path,iterations:u32,trace:&mut Trace)->Result<Value>{
    anyhow::ensure!((1..=8).contains(&iterations),"iterations must be1..8 full fixture repeats");
    anyhow::ensure!(crate::RT.get().is_none()&&!crate::gpu::STARTED.load(Ordering::SeqCst),"depth probe requires independent process before game startup");
    trace.step("begin",json!({"probeKind":"depth_downsample","iterations":iterations,"sourceSHA256":transform::SOURCE_SHA256,"constants":transform::CONSTANTS,"productionSwitchChanged":false,"comparisonKind":"same-v2-wgsl-zero-init-on-versus-off","originalZeroInitializeWorkgroupMemory":true,"optimizedZeroInitializeWorkgroupMemory":false}))?;
    crate::config::configure_sync_submit(false,&root.join("depth-downsample-submit-trace"));
    std::env::set_var("MUGUET_DEVICE_FAULT_DIR",root.join("depth-downsample-device-fault"));
    std::env::set_var("MUGUET_PIPELINE_TRACE_DIR",root.join("depth-downsample-pipeline-trace"));
    std::env::set_var("MUGUET_SUPPORT",root);
    let game_root=crate::config::game_dir();let build=crate::config::build_dir(&game_root).with_context(||format!("no shader build under {}",game_root.join("b").display()))?;
    let source_path=build.join("shaders").join(transform::SHADER_FILE);
    let original=std::fs::read_to_string(&source_path).with_context(||format!("read private depth shader {}",source_path.display()))?;
    let optimized=transform::rewrite(transform::LABEL,&original,&transform::CONSTANTS).map_err(anyhow::Error::msg)?.with_context(||format!("pinned depth shader SHA/constants mismatch: {}",source_path.display()))?;
    std::fs::write(root.join("depth-downsample-pinned-source.wgsl"),&original)?;std::fs::write(root.join("depth-downsample-original.wgsl"),&optimized)?;std::fs::write(root.join("depth-downsample-optimized.wgsl"),&optimized)?;
    let mut desc=InstanceDescriptor::new_without_display_handle();desc.backends=Backends::VULKAN;let instance=Instance::new(desc);
    let adapter=pollster::block_on(instance.request_adapter(&RequestAdapterOptions{power_preference:PowerPreference::HighPerformance,compatible_surface:None,force_fallback_adapter:false,apply_limit_buckets:false}))?;
    let info=adapter.get_info();let adapter_info=json!({"name":info.name,"vendor":info.vendor,"device":info.device,"driver":info.driver,"driverInfo":info.driver_info,"backend":format!("{:?}",info.backend)});
    let features=adapter.features()&Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
    let(device,queue)=pollster::block_on(adapter.request_device(&DeviceDescriptor{label:Some("depth-downsample-differential-probe"),required_features:features,required_limits:adapter.limits(),..Default::default()}))?;
    let errors=Arc::new(Mutex::new(Vec::<String>::new()));let e=errors.clone();device.on_uncaptured_error(Arc::new(move|error:Error|{if let Ok(mut e)=e.lock(){if e.len()<32{e.push(error.to_string());}}}));let e=errors.clone();device.set_device_lost_callback(move|r,m|{if let Ok(mut e)=e.lock(){if e.len()<32{e.push(format!("device lost {r:?}: {m}"));}}});
    trace.step("adapter",adapter_info.clone())?;let mut all=Vec::new();
    for(format,bpp,name)in[(TextureFormat::Rgba16Float,8,"rgba16float"),(TextureFormat::Rgba32Float,16,"rgba32float")]{
        anyhow::ensure!(adapter.get_texture_format_features(format).allowed_usages.contains(TextureUsages::STORAGE_BINDING|TextureUsages::COPY_SRC|TextureUsages::COPY_DST),"required {name} storage/readback unsupported");
        trace.step("before-pipelines",json!({"format":name}))?;let pipe=pipelines(&device,&optimized,&optimized,format,bpp,name);errors_seen(&errors)?;
        for repeat in 0..iterations{for(width,height)in[(1202,800),(450,298)]{for pattern in 0..model::PATTERNS.len(){all.push(run_case(&device,&queue,&pipe,width,height,pattern,repeat,trace,&errors)?);}}}
        drop(pipe);
    }
    trace.step("before-final-teardown",json!({}))?;device.poll(PollType::Wait{submission_index:None,timeout:Some(Duration::from_secs(30))})?;errors_seen(&errors)?;
    drop(queue);drop(device);drop(adapter);drop(instance);trace.step("after-final-teardown",json!({}))?;
    Ok(json!({"probeKind":"depth_downsample","status":"pass","pass":true,"iterationsRequested":iterations,"iterationsCompleted":iterations,"casesCompleted":trace.cases,"expectedCases":iterations*32,"u32WordsCompared":trace.words,"sentinelU32WordsChecked":trace.sentinels,"sourceSHA256":transform::SOURCE_SHA256,"baselineShaderSHA256":format!("{:x}",Sha256::digest(optimized.as_bytes())),"optimizedSHA256":format!("{:x}",Sha256::digest(optimized.as_bytes())),"adapter":adapter_info,"cases":all,"productionSwitchChanged":false,"comparisonKind":"same-v2-wgsl-zero-init-on-versus-off","originalZeroInitializeWorkgroupMemory":true,"optimizedZeroInitializeWorkgroupMemory":false,"scope":"Independent offscreen same-V2-WGSL bitwise differential over all output texels and unchanged border; only compilation_options.zero_initialize_workgroup_memory differs (original true vs optimized false). Same dispatch, bindings, input textures, uniform bytes, formats and patched Naga guards. Includes final device teardown. Does not manufacture logical-size greater than physical input texture dimensions or establish normal-game equivalence by itself."}))
}
#[no_mangle]
pub unsafe extern "C" fn mg_depth_downsample_probe_json(files_dir:*const c_char,iterations:u32)->*const c_char{
    let result=(||->Result<Value>{anyhow::ensure!(!files_dir.is_null(),"null files directory");let root=Path::new(unsafe{CStr::from_ptr(files_dir)}.to_str()?);anyhow::ensure!(!RUNNING.swap(true,Ordering::AcqRel),"depth probe already running");let _guard=RunGuard;let mut trace=Trace::new(root)?;match run(root,iterations,&mut trace){Ok(v)=>Ok(v),Err(e)=>{let error=format!("{e:#}");let _=trace.step("fail",json!({"error":error}));Ok(json!({"probeKind":"depth_downsample","status":"fail","pass":false,"iterationsRequested":iterations,"iterationsCompleted":0,"casesCompleted":trace.cases,"u32WordsCompared":trace.words,"error":error,"productionSwitchChanged":false,"comparisonKind":"same-v2-wgsl-zero-init-on-versus-off","originalZeroInitializeWorkgroupMemory":true,"optimizedZeroInitializeWorkgroupMemory":false}))}}})().unwrap_or_else(|e|json!({"probeKind":"depth_downsample","status":"fail","pass":false,"iterationsRequested":iterations,"iterationsCompleted":0,"casesCompleted":0,"u32WordsCompared":0,"error":format!("{e:#}"),"productionSwitchChanged":false,"comparisonKind":"same-v2-wgsl-zero-init-on-versus-off","originalZeroInitializeWorkgroupMemory":true,"optimizedZeroInitializeWorkgroupMemory":false}));
    RESULT.with(|slot|{let mut slot=slot.borrow_mut();*slot=CString::new(result.to_string()).expect("JSON contains no NUL");slot.as_ptr()})
}
