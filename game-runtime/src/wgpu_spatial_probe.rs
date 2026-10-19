//! Marker-only WGPU qualification of the actual production XEngine path.
//! It is independent of normal game startup and does not create a surface.
use anyhow::{Context, Result};
use ash::{ext::debug_utils, vk};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, ffi::{c_char, c_void, CStr, CString}, fs::File, io::Write,
    path::Path, sync::{Arc, Mutex, mpsc, atomic::{AtomicBool, Ordering}},
    time::{Duration, Instant}};
use wgpu::*;
use crate::spatial_upscale::{self, Spatial};

thread_local! { static RESULT: RefCell<CString> = RefCell::new(CString::new("{}").unwrap()); }
static RUNNING: AtomicBool = AtomicBool::new(false);
const GUARD: usize = 256;
const SENTINEL: u8 = 0xa5;
struct RunGuard;
impl Drop for RunGuard { fn drop(&mut self) {
    spatial_upscale::configure(&json!({"spatial_upscale_mode":"off"}));
    RUNNING.store(false, Ordering::Release);
} }
struct PoolEventTrace { previous: Option<std::ffi::OsString> }
impl PoolEventTrace {
    fn new(root: &Path) -> Result<Self> {
        let path=root.join("wgpu-spatial-pool-events.log");
        let mut file=File::create(&path)?;
        file.write_all(b"phase=probe-begin coverage=own-HAL-command-pools limitBytes=32768\n")?;
        file.sync_all()?;
        let previous=std::env::var_os("MUGUET_VK_POOL_EVENT_TRACE");
        // Marker-only independent startup; no game GPU instance exists here.
        unsafe { std::env::set_var("MUGUET_VK_POOL_EVENT_TRACE",path); }
        Ok(Self{previous})
    }
}
impl Drop for PoolEventTrace { fn drop(&mut self) {
    unsafe { match &self.previous {
        Some(value)=>std::env::set_var("MUGUET_VK_POOL_EVENT_TRACE",value),
        None=>std::env::remove_var("MUGUET_VK_POOL_EVENT_TRACE"),
    } }
} }
fn pool_event_report(root: &Path) -> Value {
    let path=root.join("wgpu-spatial-pool-events.log");
    match std::fs::read_to_string(&path) {
        Ok(events)=>json!({"file":path,"bytes":events.len(),"limitBytes":32768,
            "truncated":events.contains("phase=trace-truncated"),
            "afterDeviceDestroyRecorded":events.contains("phase=after-device-destroy "),
            "scope":"Only successful public command-pool creates and actual destroys performed by the app HAL, with actual VkDevice and pool handles. SDK internal operations are not hooked. Truncated/missing events cannot exclude an owner."}),
        Err(error)=>json!({"file":path,"error":error.to_string(),"complete":false}),
    }
}
struct Trace { file: File, start: Instant, fixtures: u32, submits: u32, phase: String }
impl Trace {
    fn new(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root)?;
        Ok(Self { file: File::create(root.join("wgpu-spatial-probe-trace.jsonl"))?,
            start: Instant::now(), fixtures: 0, submits: 0, phase: "begin".into() })
    }
    fn step(&mut self, phase: &str, detail: Value) -> Result<()> {
        self.phase = phase.into();
        serde_json::to_writer(&mut self.file, &json!({"phase":phase,"detail":detail,
            "elapsedMs":self.start.elapsed().as_millis(),"fixturesCompleted":self.fixtures,
            "submissions":self.submits}))?;
        self.file.write_all(b"\n")?; self.file.sync_all()?; Ok(())
    }
}

// An additional public Vulkan messenger observes all errors, including errors
// which the HAL's own log callback may filter. It must die before its instance.
#[derive(Default)]
struct ValidationState {
    negative_active: AtomicBool, negative_observed: AtomicBool,
    errors: Mutex<Vec<String>>, warnings: Mutex<Vec<String>>,
}
unsafe extern "system" fn validation_callback(severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _kind: vk::DebugUtilsMessageTypeFlagsEXT, data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    user: *mut c_void) -> vk::Bool32 {
    if data.is_null() || user.is_null() { return vk::FALSE; }
    let state = unsafe { &*user.cast::<ValidationState>() };
    let data = unsafe { &*data };
    let message = if data.p_message.is_null() { String::new() }
        else { unsafe { CStr::from_ptr(data.p_message) }.to_string_lossy().into_owned() };
    let id = if data.p_message_id_name.is_null() { String::new() }
        else { unsafe { CStr::from_ptr(data.p_message_id_name) }.to_string_lossy().into_owned() };
    let text = format!("{id}: {message}");
    if state.negative_active.load(Ordering::Relaxed)
        && text.contains("VUID-vkSubmitDebugUtilsMessageEXT-messageSeverity-parameter") {
        state.negative_observed.store(true, Ordering::Release);
    } else if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
        if let Ok(mut errors) = state.errors.lock() { if errors.len() < 64 { errors.push(text); } }
    } else if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING) {
        if let Ok(mut warnings) = state.warnings.lock() { if warnings.len() < 32 { warnings.push(text); } }
    }
    vk::FALSE
}
struct Validation {
    loader: debug_utils::Instance, messenger: vk::DebugUtilsMessengerEXT,
    state: Box<ValidationState>, available: bool,
}
impl Validation {
    fn new(instance: &Instance) -> Result<Self> {
        let hal = unsafe { instance.as_hal::<hal::api::Vulkan>() }.context("instance is not Vulkan")?;
        let shared = hal.shared_instance();
        let available = unsafe { shared.entry().enumerate_instance_layer_properties() }?
            .iter().any(|p| p.layer_name_as_c_str().is_ok_and(|n| n == c"VK_LAYER_KHRONOS_validation"));
        anyhow::ensure!(available, "public Vulkan validation layer unavailable; cannot qualify WGPU interop");
        anyhow::ensure!(shared.extensions().contains(&debug_utils::NAME), "public VK_EXT_debug_utils not enabled");
        let loader = debug_utils::Instance::new(shared.entry(), shared.raw_instance());
        let mut state = Box::<ValidationState>::default();
        let info = vk::DebugUtilsMessengerCreateInfoEXT::default()
            .message_severity(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING)
            .message_type(vk::DebugUtilsMessageTypeFlagsEXT::GENERAL | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE)
            .pfn_user_callback(Some(validation_callback))
            .user_data((&mut *state as *mut ValidationState).cast());
        let messenger = unsafe { loader.create_debug_utils_messenger(&info, None) }?;
        let observer = Self { loader, messenger, state, available };
        observer.state.negative_active.store(true, Ordering::Release);
        let message = vk::DebugUtilsMessengerCallbackDataEXT::default()
            .message_id_name(c"MuguetWgpuSpatialNegativeControl")
            .message(c"Intentional invalid message severity; no GPU command or resource");
        unsafe { observer.loader.submit_debug_utils_message(vk::DebugUtilsMessageSeverityFlagsEXT::empty(),
            vk::DebugUtilsMessageTypeFlagsEXT::GENERAL, &message); }
        observer.state.negative_active.store(false, Ordering::Release);
        anyhow::ensure!(observer.state.negative_observed.load(Ordering::Acquire),
            "validation negative control was not observed; layer execution not established");
        observer.check()?; Ok(observer)
    }
    fn check(&self) -> Result<()> {
        let errors = self.state.errors.lock().unwrap();
        anyhow::ensure!(errors.is_empty(), "Vulkan validation error: {}", errors.join("; ")); Ok(())
    }
    fn report(&self) -> Value {
        json!({"layerAvailable":self.available,"validationRequested":true,
            "layerExecutionConfirmedByNegativeControl":self.state.negative_observed.load(Ordering::Acquire),
            "handleWrapping":false,"publicSetting":"VK_LAYER_UNIQUE_HANDLES=0",
            "otherValidationDisabledByProbe":false,
            "unexpectedErrors":self.state.errors.lock().unwrap().clone(),
            "warnings":self.state.warnings.lock().unwrap().clone(),
            "scope":"All explicitly called app/HAL Vulkan operations observed by an additional unfiltered public messenger; opaque XEngine internal draw calls are not claimed as intercepted."})
    }
}
impl Drop for Validation { fn drop(&mut self) {
    unsafe { self.loader.destroy_debug_utils_messenger(self.messenger, None); }
} }
fn errors_seen(errors: &Mutex<Vec<String>>, validation: &Validation) -> Result<()> {
    validation.check()?;
    let errors = errors.lock().unwrap();
    anyhow::ensure!(errors.is_empty(), "wgpu/device error: {}", errors.join("; ")); Ok(())
}

#[derive(Clone, Copy, Debug)]
enum Pattern { Constant, Gradient, Quadrants }
impl Pattern { fn name(self) -> &'static str { match self {
    Self::Constant => "constant", Self::Gradient => "gradient_checker", Self::Quadrants => "quadrants" } } }
fn pixel(pattern: Pattern, x: u32, y: u32, w: u32, h: u32, serial: u32) -> [u8;4] {
    let rgb = match pattern {
        Pattern::Constant => if serial % 2 == 0 { [37,119,211] } else { [213,83,29] },
        Pattern::Gradient => [16 + (223*x/(w-1)) as u8,16 + (223*y/(h-1)) as u8,
            48 + 128*((x/64+y/64+serial)%2) as u8],
        Pattern::Quadrants => [[224,32,48],[32,224,48],[32,48,224],[224,208,32]]
            [((x>=w/2) as usize)+2*((y>=h/2) as usize)],
    };
    [rgb[0],rgb[1],rgb[2],255]
}
fn texture(device: &Device, size: (u32,u32), format: TextureFormat, label: &'static str) -> Texture {
    device.create_texture(&TextureDescriptor { label: Some(label),
        size: Extent3d { width: size.0,height: size.1,depth_or_array_layers: 1 },
        mip_level_count: 1,sample_count: 1,dimension: TextureDimension::D2,format,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT
            | TextureUsages::COPY_SRC | TextureUsages::COPY_DST, view_formats: &[] })
}
fn source(device: &Device, queue: &Queue, size: (u32,u32), format: TextureFormat,
    pattern: Pattern, serial: u32) -> (Texture, Vec<u8>) {
    let texture = texture(device,size,format,"wgpu-spatial-source");
    let mut logical = Vec::with_capacity((size.0*size.1*4) as usize);
    for y in 0..size.1 { for x in 0..size.0 { logical.extend_from_slice(&pixel(pattern,x,y,size.0,size.1,serial)); } }
    let mut physical = logical.clone();
    if format == TextureFormat::Bgra8Unorm { for p in physical.chunks_exact_mut(4) { p.swap(0,2); } }
    queue.write_texture(TexelCopyTextureInfo { texture: &texture,mip_level: 0,origin: Origin3d::ZERO,
        aspect: TextureAspect::All }, &physical,
        TexelCopyBufferLayout { offset: 0,bytes_per_row: Some(size.0*4),rows_per_image: Some(size.1) },
        texture.size());
    (texture,logical)
}
struct Readback { buffer: Buffer, width: u32, height: u32, pitch: usize }
impl Readback {
    fn record(device: &Device, queue: &Queue, encoder: &mut CommandEncoder, image: &Texture) -> Self {
        let (width,height) = (image.width(),image.height());
        let pitch = ((width as usize*4).div_ceil(256))*256;
        let bytes = GUARD + pitch*height as usize + GUARD;
        let buffer = device.create_buffer(&BufferDescriptor { label: Some("wgpu-spatial-guarded-readback"),
            size: bytes as u64,usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,mapped_at_creation: false });
        queue.write_buffer(&buffer,0,&vec![SENTINEL;bytes]);
        encoder.copy_texture_to_buffer(TexelCopyTextureInfo { texture: image,mip_level: 0,origin: Origin3d::ZERO,
            aspect: TextureAspect::All }, TexelCopyBufferInfo { buffer: &buffer,
            layout: TexelCopyBufferLayout { offset: GUARD as u64,bytes_per_row: Some(pitch as u32),rows_per_image: Some(height) } }, image.size());
        Self { buffer,width,height,pitch }
    }
    fn read(&self, device: &Device) -> Result<(Vec<u8>,usize)> {
        let (tx,rx) = mpsc::channel();
        self.buffer.slice(..).map_async(MapMode::Read,move|r| {let _=tx.send(r.map_err(|e|e.to_string()));});
        device.poll(PollType::Wait {submission_index: None,timeout: Some(Duration::from_secs(30))})?;
        rx.recv_timeout(Duration::from_secs(5)).context("map callback timeout")?.map_err(anyhow::Error::msg)?;
        let result = (|| -> Result<(Vec<u8>,usize)> {
            let mapped = self.buffer.slice(..).get_mapped_range()?;
            let payload_end = GUARD+self.pitch*self.height as usize;
            anyhow::ensure!(mapped[..GUARD].iter().chain(&mapped[payload_end..]).all(|b|*b==SENTINEL),
                "prefix/suffix readback guard changed");
            let mut pixels=Vec::with_capacity((self.width*self.height*4)as usize);
            for row in 0..self.height as usize {
                let start=GUARD+row*self.pitch;let end=start+self.width as usize*4;
                pixels.extend_from_slice(&mapped[start..end]);
                anyhow::ensure!(mapped[end..start+self.pitch].iter().all(|b|*b==SENTINEL),
                    "readback row padding guard changed at row {row}");
            }
            Ok((pixels,GUARD*2+(self.pitch-self.width as usize*4)*self.height as usize))
        })();
        self.buffer.unmap();result
    }
}
struct Pending {
    commands: Option<[CommandBuffer;3]>,
    source: Texture, sampled: Texture, raw_output: Texture, present: Texture, present_view: TextureView,
    present_bg: BindGroup, input_readback: Readback, raw_readback: Readback, present_readback: Readback,
    logical: Vec<u8>, pattern: Pattern, serial: u32, slot: usize, format: TextureFormat,
    input: (u32,u32), output: (u32,u32),
}
fn encode(device: &Device, queue: &Queue, spatial: &mut Spatial, pipeline: &RenderPipeline,
    input: (u32,u32), format: TextureFormat, pattern: Pattern, serial: u32, slot: usize,
    trace: &mut Trace) -> Result<Pending> {
    let (source,logical)=source(device,queue,input,format,pattern,serial);
    let mut before=device.create_command_encoder(&CommandEncoderDescriptor {label:Some("wgpu-spatial-WG1")});
    // This is the actual production method, including its BGRA->RGBA pass,
    // output initialization, tracked transitions and raw-only SDK encoder.
    let (raw,present_bg,output)=spatial.record(&source,slot,pipeline,&mut before)?;
    let sampled=spatial.sampled_input_texture(slot).context("missing production sampled input")?;
    let raw_output=spatial.output_texture(slot).context("missing production output")?;
    anyhow::ensure!(sampled.format()==TextureFormat::Rgba8Unorm && raw_output.format()==TextureFormat::Rgba8Unorm,
        "production sampled input/output is not RGBA8");
    anyhow::ensure!((sampled.width(),sampled.height())==input,"converted input extent changed");
    let present=texture(device,output,format,"wgpu-spatial-present-target");
    let present_view=present.create_view(&TextureViewDescriptor::default());
    let mut after=device.create_command_encoder(&CommandEncoderDescriptor {label:Some("wgpu-spatial-WG3")});
    {
        let mut pass=after.begin_render_pass(&RenderPassDescriptor {label:Some("wgpu-spatial-present-style"),
            color_attachments:&[Some(RenderPassColorAttachment {view:&present_view,depth_slice:None,resolve_target:None,
                ops:Operations {load:LoadOp::Clear(Color::BLACK),store:StoreOp::Store}})],
            depth_stencil_attachment:None,timestamp_writes:None,occlusion_query_set:None,multiview_mask:None});
        pass.set_pipeline(pipeline);pass.set_bind_group(0,&present_bg,&[]);pass.draw(0..3,0..1);
    }
    let input_readback=Readback::record(device,queue,&mut after,&sampled);
    let raw_readback=Readback::record(device,queue,&mut after,&raw_output);
    let present_readback=Readback::record(device,queue,&mut after,&present);
    trace.step("three-CB-prepared",json!({"slot":slot,"sourceFormat":format!("{format:?}"),
        "pattern":pattern.name(),"input":input,"output":output,"production":spatial_upscale::status()}))?;
    let commands=Some([before.finish(),raw,after.finish()]);
    Ok(Pending {commands,source,sampled,raw_output,present,present_view,present_bg,input_readback,raw_readback,
        present_readback,logical,pattern,serial,slot,format,input,output})
}
fn verify(pending: Pending, device: &Device, trace: &mut Trace, mode: &str, root: &Path) -> Result<Value> {
    // Every cloned resource, view and bind group stays retained through map and
    // verification. Spatial slot reuse occurs only after the batch fence.
    let (converted,ig)=pending.input_readback.read(device)?;
    anyhow::ensure!(converted==pending.logical,"BGRA/RGBA sampled input differs from logical RGBA (channel order/alpha)");
    let (raw,rg)=pending.raw_readback.read(device)?;
    let (mut present,pg)=pending.present_readback.read(device)?;
    let physical_present_hash=format!("{:x}",Sha256::digest(&present));
    // CPU verification decodes the target's documented physical channel order;
    // GPU conversion is exclusively the actual production textureLoad pass.
    if pending.format==TextureFormat::Bgra8Unorm {for p in present.chunks_exact_mut(4){p.swap(0,2);}}
    anyhow::ensure!(raw==present,"logical present textureLoad blit differs from native RGBA output");
    let mut compared=0u64;let mut bad=0u64;let mut max_error=0u32;let mut alpha_errors=0u64;
    for y in 0..pending.output.1 { for x in 0..pending.output.0 {
        let offset=((y*pending.output.0+x)*4)as usize;let p=&present[offset..offset+4];
        if p[3]<252 {alpha_errors+=1;}
        let sx=((x as f64+0.5)*pending.input.0 as f64/pending.output.0 as f64-0.5).clamp(0.0,(pending.input.0-1)as f64);
        let sy=((y as f64+0.5)*pending.input.1 as f64/pending.output.1 as f64-0.5).clamp(0.0,(pending.input.1-1)as f64);
        let expected=pixel(pending.pattern,(sx+0.5)as u32,(sy+0.5)as u32,pending.input.0,pending.input.1,pending.serial);
        for c in 0..3 {
            let boundary=match pending.pattern {
                Pattern::Quadrants=>(sx-pending.input.0 as f64/2.0).abs()<4.0 || (sy-pending.input.1 as f64/2.0).abs()<4.0,
                Pattern::Gradient if c==2=>sx%64.0<4.0 || sx%64.0>60.0 || sy%64.0<4.0 || sy%64.0>60.0,
                _=>false,
            };
            if boundary {continue;}
            let difference=u32::from(p[c].abs_diff(expected[c]));compared+=1;max_error=max_error.max(difference);
            let tolerance=if matches!(pending.pattern,Pattern::Gradient)&&c<2 {8}else{3};
            if difference>tolerance {bad+=1;}
        }
    } }
    anyhow::ensure!(bad==0&&alpha_errors==0,"SR output mismatch: badChannels={bad}, alphaErrors={alpha_errors}, maxError={max_error}");
    let output_file=if matches!(pending.pattern,Pattern::Quadrants) {
        let name=format!("wgpu-spatial-{mode}-{:?}-quadrants.ppm",pending.format);
        let mut f=File::create(root.join(&name))?;write!(f,"P6\n{} {}\n255\n",pending.output.0,pending.output.1)?;
        for p in present.chunks_exact(4) {f.write_all(&p[..3])?;}f.sync_all()?;Some(name)
    }else{None};
    let result=json!({"mode":mode,"sourceFormat":format!("{:?}",pending.format),"input":pending.input,
        "output":pending.output,"pattern":pending.pattern.name(),"serial":pending.serial,"slot":pending.slot,
        "convertedBGRAtoRGBA":pending.format==TextureFormat::Bgra8Unorm,"inputLogicalRGBAExact":true,
        "presentFormat":format!("{:?}",pending.format),"presentLogicalRGBAEqualsRawOutputExact":true,
        "guardBytesChecked":ig+rg+pg,"badChannels":bad,
        "alphaErrors":alpha_errors,"alphaScope":"opaque source alpha255; output alpha>=252",
        "comparedSRChannels":compared,"maximumSRChannelError":max_error,
        "inputSHA256":format!("{:x}",Sha256::digest(&converted)),
        "outputLogicalRGBASHA256":format!("{:x}",Sha256::digest(&present)),
        "presentPhysicalBytesSHA256":physical_present_hash,"outputImage":output_file,"pass":true});
    // Explicitly consume retained handles only after all readbacks completed.
    drop((pending.source,pending.sampled,pending.raw_output,pending.present,pending.present_view,pending.present_bg));
    trace.fixtures+=1;trace.step("fixture-pass",result.clone())?;Ok(result)
}
fn run(root: &Path, iterations: u32, trace: &mut Trace) -> Result<Value> {
    anyhow::ensure!((1..=4).contains(&iterations),"iterations must be1..4");
    anyhow::ensure!(crate::RT.get().is_none()&&!crate::gpu::STARTED.load(Ordering::SeqCst),"requires independent process before game startup");
    let _pool_event_trace=PoolEventTrace::new(root)?;
    spatial_upscale::configure(&json!({"spatial_upscale_mode":"native_quality","spatial_upscale_sharpness":0.2}));
    spatial_upscale::validation_compatibility()?;
    trace.step("begin",json!({"iterations":iterations,"realProductionMethods":true,"slots":3,
        "framesPerIteration":12,"ordinaryGameStarted":false,"physicalSurfaceCreated":false,
        "ownHalPoolEventFile":"wgpu-spatial-pool-events.log","poolEventLimitBytes":32768}))?;
    let mut descriptor=InstanceDescriptor::new_without_display_handle();descriptor.backends=Backends::VULKAN;
    descriptor.flags=InstanceFlags::DEBUG|InstanceFlags::VALIDATION;
    let instance=Instance::new(descriptor);let validation=Validation::new(&instance)?;
    trace.step("validation-negative-control-pass",validation.report())?;
    let adapter=pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
        power_preference:PowerPreference::HighPerformance,compatible_surface:None,
        force_fallback_adapter:false,apply_limit_buckets:false}))?;
    let info=adapter.get_info();
    let adapter_info=json!({"name":info.name,"vendor":info.vendor,"device":info.device,
        "driver":info.driver,"driverInfo":info.driver_info,"backend":format!("{:?}",info.backend)});
    let (device,queue)=pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label:Some("production-spatial-independent-probe"),required_features:Features::empty(),
        required_limits:adapter.limits(),..Default::default()}))?;
    let errors=Arc::new(Mutex::new(Vec::<String>::new()));let e=errors.clone();
    device.on_uncaptured_error(Arc::new(move|error:Error|{if let Ok(mut e)=e.lock(){if e.len()<32{e.push(error.to_string());}}}));
    let e=errors.clone();device.set_device_lost_callback(move|reason,message|{if let Ok(mut e)=e.lock(){if e.len()<32{e.push(format!("device lost {reason:?}: {message}"));}}});
    let mut fixtures=Vec::new();let mut serial=0;
    for mode in ["native_quality","performance"] {
        spatial_upscale::configure(&json!({"spatial_upscale_mode":mode,"spatial_upscale_sharpness":0.2}));
        let mut spatial=Spatial::new(&device,3).context("production Spatial unexpectedly off")?;
        let input=if mode=="native_quality"{(1202,800)}else{(802,534)};
        for _ in 0..iterations { for format in [TextureFormat::Rgba8Unorm,TextureFormat::Bgra8Unorm] {
            let pipeline=spatial_upscale::blit_pipeline(&device,format);
            let mut pending=Vec::new();
            for (slot,pattern) in [Pattern::Constant,Pattern::Gradient,Pattern::Quadrants].into_iter().enumerate() {
                serial+=1;pending.push(encode(&device,&queue,&mut spatial,&pipeline,input,format,pattern,serial,slot,trace)?);
            }
            trace.step("before-three-slot-submits",json!({"mode":mode,"sourceFormat":format!("{format:?}")}))?;
            // Prepare all three first, then submit three ordinary production
            // batches without an intervening completion wait. Hardware may
            // advance independently; three retained pending owners are proven,
            // not a measured simultaneous GPU execution count.
            for fixture in &mut pending {
                queue.submit(fixture.commands.take().context("fixture commands already submitted")?);
                trace.submits+=1;
            }
            trace.step("before-three-slot-fence",json!({"mode":mode,"sourceFormat":format!("{format:?}")}))?;
            device.poll(PollType::Wait {submission_index:None,timeout:Some(Duration::from_secs(30))})?;
            errors_seen(&errors,&validation)?;
            for fixture in pending {fixtures.push(verify(fixture,&device,trace,mode,root)?);}
            errors_seen(&errors,&validation)?;
        } }
        trace.step("before-mode-context-destroy",json!({"mode":mode,"productionStatus":spatial_upscale::status()}))?;
        drop(spatial);errors_seen(&errors,&validation)?;
        trace.step("after-mode-context-destroy",json!({"mode":mode}))?;
    }
    trace.step("before-final-teardown",json!({}))?;
    device.poll(PollType::Wait {submission_index:None,timeout:Some(Duration::from_secs(30))})?;
    drop(queue);drop(device);drop(adapter);
    errors_seen(&errors,&validation)?;let validation_report=validation.report();drop(validation);drop(instance);
    trace.step("after-final-teardown",json!({}))?;
    Ok(json!({"probeKind":"wgpu_spatial","status":"pass","pass":true,"adapter":adapter_info,
        "iterationsCompleted":iterations,"fixturesCompleted":trace.fixtures,"expectedFixtures":iterations*12,
        "submissions":trace.submits,"commandBuffersPerSubmission":3,"maximumRetainedSlots":3,
        "submissionsBeforeExplicitCompletionWait":3,"simultaneousGpuExecutionCountMeasured":false,
        "actualProductionConversionAndSdkRecord":true,"sharedProductionTextureLoadShader":true,
        "presentPipelineBuilder":"spatial_upscale::blit_pipeline (also builds production conversion)",
        "ordinaryGamePresentMethodInvoked":false,"presentTargetFormats":["Rgba8Unorm","Bgra8Unorm"],
        "validation":validation_report,"wgpuErrors":errors.lock().unwrap().clone(),"fixtures":fixtures,
        "finalDeviceTeardownComplete":true,"ordinaryGameStarted":false,"physicalSurfaceCreated":false,
        "scope":"Actual production Spatial::record with direct RGBA and BGRA-to-RGBA paths, shared RGBA/BGRA present pipelines, three tracked/raw/tracked command buffers in each queue submit, three separately retained context/texture/view slots submitted before a completion wait, exact converted input and logical present-copy checks plus tolerant spatial fixtures and guarded readback. No physical-game presentation, long-play behavior, render FPS or generated-frame qualification claimed."}))
}

#[no_mangle]
pub unsafe extern "C" fn mg_wgpu_spatial_probe_json(files_dir: *const c_char, iterations: u32) -> *const c_char {
    let result=(||->Result<Value>{
        anyhow::ensure!(!files_dir.is_null(),"null files dir");
        let root=Path::new(unsafe {CStr::from_ptr(files_dir)}.to_str()?);
        anyhow::ensure!(!RUNNING.swap(true,Ordering::AcqRel),"WGPU spatial probe already running");
        let _guard=RunGuard;let mut trace=Trace::new(root)?;
        let mut result=match run(root,iterations,&mut trace) {
            Ok(value)=>value,Err(error)=>{
                let error=format!("{error:#}");let failed_phase=trace.phase.clone();
                let _=trace.step("fail",json!({"error":error,"failedPhase":failed_phase}));
                json!({"probeKind":"wgpu_spatial","status":"fail","pass":false,"error":error,
                    "failedPhase":failed_phase,"fixturesCompleted":trace.fixtures,"submissions":trace.submits,
                    "finalDeviceTeardownComplete":false})
            }
        };
        result["ownHalCommandPoolEvents"]=pool_event_report(root);
        Ok(result)
    })().unwrap_or_else(|e|json!({"probeKind":"wgpu_spatial","status":"fail","pass":false,"error":format!("{e:#}")}));
    RESULT.with(|slot| { let mut slot = slot.borrow_mut();
        *slot = CString::new(result.to_string()).expect("JSON has no raw NUL");
        slot.as_ptr()
    })
}
