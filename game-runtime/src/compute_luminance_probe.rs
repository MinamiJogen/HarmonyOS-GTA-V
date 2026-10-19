//! Marker-only original/optimized compute comparison; never enters normal game.
use anyhow::{Context,Result};
use serde_json::{json,Value};
use sha2::{Digest,Sha256};
use std::{cell::RefCell,ffi::{CStr,CString},os::raw::c_char,path::Path,
    fs::File,io::Write,sync::{Arc,Mutex,mpsc,atomic::{AtomicBool,Ordering}},time::{Duration,Instant}};
use wgpu::*;
#[path="compute_depth_model.rs"] mod model;
#[path="luminance_init.rs"] mod transform;

thread_local!{static RESULT:RefCell<CString>=RefCell::new(CString::new("{}").unwrap());}
static RUNNING:AtomicBool=AtomicBool::new(false);
struct RunGuard;
impl Drop for RunGuard{fn drop(&mut self){RUNNING.store(false,Ordering::Release);}}
struct Trace{file:File,start:Instant,cases:u32,words:u64,sentinels:u64}
impl Trace{
    fn new(root:&Path)->Result<Self>{std::fs::create_dir_all(root)?;Ok(Self{file:File::create(root.join("luminance-probe-trace.jsonl"))?,start:Instant::now(),cases:0,words:0,sentinels:0})}
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

fn pipelines(device:&Device,source:&str)->(BindGroupLayout,ComputePipeline,ComputePipeline){
    let layout=device.create_bind_group_layout(&BindGroupLayoutDescriptor{label:Some("luminance-diff-layout"),entries:&[
        BindGroupLayoutEntry{binding:12,visibility:ShaderStages::COMPUTE,ty:BindingType::Buffer{ty:BufferBindingType::Uniform,has_dynamic_offset:false,min_binding_size:None},count:None},
        BindGroupLayoutEntry{binding:35,visibility:ShaderStages::COMPUTE,ty:BindingType::Texture{sample_type:TextureSampleType::Float{filterable:false},view_dimension:TextureViewDimension::D2,multisampled:false},count:None},
        BindGroupLayoutEntry{binding:161,visibility:ShaderStages::COMPUTE,ty:BindingType::StorageTexture{access:StorageTextureAccess::WriteOnly,format:TextureFormat::R32Float,view_dimension:TextureViewDimension::D2},count:None},
    ]});
    let pl=device.create_pipeline_layout(&PipelineLayoutDescriptor{label:Some("luminance-diff-layout"),bind_group_layouts:&[Some(&layout)],immediate_size:0});
    let module=device.create_shader_module(ShaderModuleDescriptor{label:Some(transform::LABEL),source:ShaderSource::Wgsl(source.into())});
    let make=|zero_init,label|device.create_compute_pipeline(&ComputePipelineDescriptor{label:Some(label),layout:Some(&pl),module:&module,entry_point:Some("main"),compilation_options:PipelineCompilationOptions{zero_initialize_workgroup_memory:zero_init,..Default::default()},cache:None});
    let on=make(true,"luminance-zero-init-on");let off=make(false,"luminance-zero-init-off");(layout,on,off)
}
#[allow(clippy::too_many_arguments)]
fn run_case(device:&Device,queue:&Queue,layout:&BindGroupLayout,on:&ComputePipeline,off:&ComputePipeline,width:u32,height:u32,pattern:usize,repeat:u32,trace:&mut Trace,errors:&Mutex<Vec<String>>)->Result<Value>{
    let(gx,gy)=model::groups(width,height);let(ow,oh)=(gx+1,gy+1);
    let logical=if pattern==7 {(0,0)} else if pattern>=4 {(width.saturating_sub(1),height.saturating_sub(1))} else {(width,height)};
    let(bits,_)=model::input_words(width,height,pattern,repeat);let mut input_bytes=Vec::with_capacity((width*height*16)as usize);
    for word in bits.chunks_exact(4){for _ in 0..3{input_bytes.extend_from_slice(word);}input_bytes.extend_from_slice(&1.0f32.to_le_bytes());}
    let input=texture(device,"luminance-input",width,height,TextureFormat::Rgba32Float,TextureUsages::TEXTURE_BINDING|TextureUsages::COPY_DST);upload(queue,&input,width,height,16,&input_bytes);let iv=input.create_view(&TextureViewDescriptor::default());
    let cb_bytes:Vec<u8>=[0.0f32,0.0,logical.0 as f32,logical.1 as f32].into_iter().flat_map(f32::to_le_bytes).collect();let cb=buffer(device,queue,"luminance-cb12",&cb_bytes);
    let sentinel=(-7.25f32).to_le_bytes();let initial:Vec<u8>=sentinel.into_iter().cycle().take((ow*oh*4)as usize).collect();
    let pitch=(ow*4).div_ceil(256)*256;let prefix=256u64;let size=prefix+u64::from(pitch*oh)+256;let mut results=Vec::new();let mut walls=Vec::new();let mut hashes=Vec::new();
    // Complete shared input/CB uploads before either variant's wall timer.
    trace.step("before-input-upload",json!({"physicalInput":[width,height],"pattern":model::PATTERNS[pattern]}))?;
    let upload_index=queue.submit(std::iter::empty::<CommandBuffer>());
    device.poll(PollType::Wait{submission_index:Some(upload_index),timeout:Some(Duration::from_secs(30))})?;errors_seen(errors)?;
    trace.step("after-input-upload",json!({"physicalInput":[width,height],"pattern":model::PATTERNS[pattern]}))?;
    for(which,pipeline)in[("zero-init-on",on),("zero-init-off",off)]{
        let output=texture(device,"luminance-output",ow,oh,TextureFormat::R32Float,TextureUsages::STORAGE_BINDING|TextureUsages::COPY_SRC|TextureUsages::COPY_DST);upload(queue,&output,ow,oh,4,&initial);let ov=output.create_view(&TextureViewDescriptor::default());
        let bg=device.create_bind_group(&BindGroupDescriptor{label:Some("luminance-bindings"),layout,entries:&[BindGroupEntry{binding:12,resource:cb.as_entire_binding()},BindGroupEntry{binding:35,resource:BindingResource::TextureView(&iv)},BindGroupEntry{binding:161,resource:BindingResource::TextureView(&ov)}]});
        let read=device.create_buffer(&BufferDescriptor{label:Some("luminance-readback-guards"),size,usage:BufferUsages::MAP_READ|BufferUsages::COPY_DST,mapped_at_creation:false});queue.write_buffer(&read,0,&vec![0xcd;size as usize]);
        let mut enc=device.create_command_encoder(&CommandEncoderDescriptor{label:Some(which)});
        {let mut pass=enc.begin_compute_pass(&ComputePassDescriptor{label:Some(which),timestamp_writes:None});pass.set_pipeline(pipeline);pass.set_bind_group(0,&bg,&[]);pass.dispatch_workgroups(gx,gy,1);}
        enc.copy_texture_to_buffer(TexelCopyTextureInfo{texture:&output,mip_level:0,origin:Origin3d::ZERO,aspect:TextureAspect::All},TexelCopyBufferInfo{buffer:&read,layout:TexelCopyBufferLayout{offset:prefix,bytes_per_row:Some(pitch),rows_per_image:Some(oh)}},Extent3d{width:ow,height:oh,depth_or_array_layers:1});
        trace.step("before-submit",json!({"variant":which,"physicalInput":[width,height],"logicalInput":logical,"dispatch":[gx,gy,1],"pattern":model::PATTERNS[pattern],"repeat":repeat}))?;
        let start=Instant::now();let index=queue.submit([enc.finish()]);let(tx,rx)=mpsc::channel();read.slice(..).map_async(MapMode::Read,move|r|{let _=tx.send(r.map_err(|e|e.to_string()));});
        device.poll(PollType::Wait{submission_index:Some(index),timeout:Some(Duration::from_secs(30))})?;rx.recv_timeout(Duration::from_secs(5)).context("luminance readback callback timeout")?.map_err(anyhow::Error::msg)?;errors_seen(errors)?;walls.push(start.elapsed().as_secs_f64()*1000.0);
        let bytes={let r=read.slice(..).get_mapped_range().context("luminance readback mapping")?;r.to_vec()};read.unmap();
        anyhow::ensure!(bytes[..prefix as usize].iter().chain(bytes[(prefix+u64::from(pitch*oh))as usize..].iter()).all(|&b|b==0xcd),"{which} prefix/tail changed");trace.sentinels+=128;
        let mut tight=Vec::new();
        for y in 0..oh as usize{let start=prefix as usize+y*pitch as usize;tight.extend_from_slice(&bytes[start..start+(ow*4)as usize]);let padding=&bytes[start+(ow*4)as usize..start+pitch as usize];anyhow::ensure!(padding.iter().all(|&b|b==0xcd),"{which} row padding changed");trace.sentinels+=(padding.len()/4)as u64;}
        for y in 0..oh{for x in 0..ow{if x<gx&&y<gy{continue;}let at=((y*ow+x)*4)as usize;anyhow::ensure!(tight[at..at+4]==sentinel,"{which} output border changed");trace.sentinels+=1;}}
        hashes.push(format!("{:x}",Sha256::digest(&tight)));results.push(tight);trace.step("after-readback",json!({"variant":which,"physicalInput":[width,height],"pattern":model::PATTERNS[pattern]}))?;
    }
    let mut effective=0u64;
    for(i,(a,b))in results[0].chunks_exact(4).zip(results[1].chunks_exact(4)).enumerate(){anyhow::ensure!(a==b,"luminance bit mismatch physical={width}x{height} pattern={pattern} word={i} on={a:?} off={b:?}");trace.words+=1;let pixel=i as u32;if pixel%ow<gx&&pixel/ow<gy&&a!=sentinel{effective+=1;}}
    anyhow::ensure!(effective==u64::from(gx*gy),"not all nominal output tiles written");trace.cases+=1;
    let result=json!({"physicalInput":[width,height],"logicalInput":logical,"groups":[gx,gy,1],"outputWithSentinelBorder":[ow,oh],"pattern":model::PATTERNS[pattern],"repeat":repeat,"inputFormat":"rgba32float","outputFormat":"r32float","u32WordsCompared":ow*oh,"effectiveOutputTiles":effective,"onWallMs":walls[0],"offWallMs":walls[1],"onOutputSHA256":hashes[0],"offOutputSHA256":hashes[1],"timingScope":"CPU submit-to-readback wall, not isolated GPU timestamps"});trace.step("case-pass",result.clone())?;Ok(result)
}
fn run(root:&Path,iterations:u32,trace:&mut Trace)->Result<Value>{
    anyhow::ensure!((1..=8).contains(&iterations),"iterations must be1..8");anyhow::ensure!(crate::RT.get().is_none()&&!crate::gpu::STARTED.load(Ordering::SeqCst),"requires independent process before game startup");
    std::env::set_var("MUGUET_SUPPORT",root);
    let game_root=crate::config::game_dir();let build=crate::config::build_dir(&game_root).with_context(||format!("no shader build under {}",game_root.join("b").display()))?;
    let source_path=build.join("shaders").join(transform::SHADER_FILE);
    let source=std::fs::read_to_string(&source_path).with_context(||format!("read private luminance shader {}",source_path.display()))?;
    anyhow::ensure!(transform::verified(transform::LABEL,&source,&[]),"pinned luminance shader SHA mismatch: {}",source_path.display());trace.step("begin",json!({"probeKind":"luminance","sourceSHA256":transform::SOURCE_SHA256,"onlyDifference":"workgroup zero initialization true versus false","productionSwitchChanged":false}))?;
    crate::config::configure_sync_submit(false,&root.join("luminance-submit-trace"));std::env::set_var("MUGUET_DEVICE_FAULT_DIR",root.join("luminance-device-fault"));std::env::set_var("MUGUET_PIPELINE_TRACE_DIR",root.join("luminance-pipeline-trace"));std::fs::write(root.join("luminance-original.wgsl"),&source)?;
    let mut desc=InstanceDescriptor::new_without_display_handle();desc.backends=Backends::VULKAN;let instance=Instance::new(desc);
    let adapter=pollster::block_on(instance.request_adapter(&RequestAdapterOptions{power_preference:PowerPreference::HighPerformance,compatible_surface:None,force_fallback_adapter:false,apply_limit_buckets:false}))?;
    let info=adapter.get_info();let info=json!({"name":info.name,"vendor":info.vendor,"device":info.device,"driver":info.driver,"driverInfo":info.driver_info,"backend":format!("{:?}",info.backend)});
    let(device,queue)=pollster::block_on(adapter.request_device(&DeviceDescriptor{label:Some("luminance-differential-probe"),required_features:adapter.features()&Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES,required_limits:adapter.limits(),..Default::default()}))?;
    let errors=Arc::new(Mutex::new(Vec::<String>::new()));let e=errors.clone();device.on_uncaptured_error(Arc::new(move|error:Error|{if let Ok(mut e)=e.lock(){if e.len()<32{e.push(error.to_string());}}}));let e=errors.clone();device.set_device_lost_callback(move|r,m|{if let Ok(mut e)=e.lock(){if e.len()<32{e.push(format!("device lost {r:?}:{m}"));}}});trace.step("adapter",info.clone())?;
    let(layout,on,off)=pipelines(&device,&source);errors_seen(&errors)?;let mut cases=Vec::new();
    for repeat in 0..iterations{for(w,h)in[(301,200),(113,75),(17,19),(1,1),(1202,800)]{for p in 0..model::PATTERNS.len(){cases.push(run_case(&device,&queue,&layout,&on,&off,w,h,p,repeat,trace,&errors)?);}}}
    drop(off);drop(on);drop(layout);trace.step("before-final-teardown",json!({}))?;device.poll(PollType::Wait{submission_index:None,timeout:Some(Duration::from_secs(30))})?;errors_seen(&errors)?;drop(queue);drop(device);drop(adapter);drop(instance);trace.step("after-final-teardown",json!({}))?;
    Ok(json!({"probeKind":"luminance","status":"pass","pass":true,"casesCompleted":trace.cases,"expectedCases":iterations*40,"iterationsCompleted":iterations,"u32WordsCompared":trace.words,"sentinelU32WordsChecked":trace.sentinels,"sourceSHA256":transform::SOURCE_SHA256,"adapter":info,"cases":cases,"productionSwitchChanged":false,"scope":"Same exact WGSL and bindings, only zero_initialize_workgroup_memory differs; full raw-u32 output comparison, guarded prefix/row padding/tail and extra output border, final resource/device teardown. Logical uniform extents never exceed physical textures. These fixtures do not establish whole-game equivalence or FPS."}))
}
#[no_mangle]
pub unsafe extern "C" fn mg_luminance_probe_json(files_dir:*const c_char,iterations:u32)->*const c_char{
    let result=(||->Result<Value>{anyhow::ensure!(!files_dir.is_null(),"null files dir");let root=Path::new(unsafe{CStr::from_ptr(files_dir)}.to_str()?);anyhow::ensure!(!RUNNING.swap(true,Ordering::AcqRel),"luminance probe already running");let _guard=RunGuard;let mut trace=Trace::new(root)?;match run(root,iterations,&mut trace){Ok(v)=>Ok(v),Err(e)=>{let error=format!("{e:#}");let _=trace.step("fail",json!({"error":error}));Ok(json!({"probeKind":"luminance","status":"fail","pass":false,"casesCompleted":trace.cases,"u32WordsCompared":trace.words,"error":error}))}}})().unwrap_or_else(|e|json!({"probeKind":"luminance","status":"fail","pass":false,"error":format!("{e:#}")}));
    RESULT.with(|slot|{let mut slot=slot.borrow_mut();*slot=CString::new(result.to_string()).expect("JSON no NUL");slot.as_ptr()})
}
