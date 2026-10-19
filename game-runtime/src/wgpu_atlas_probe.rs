//! Offscreen upload/sample/readback control through the same patched wgpu HAL.
//! It is invoked only by the independent diagnostic worker, never game startup.
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{cell::RefCell, ffi::{CStr, CString}, fs::File, io::Write,
          os::raw::c_char, path::Path, sync::{Arc, Mutex, mpsc}, time::{Duration, Instant}};
use wgpu::*;

const ATLAS_W: u32 = 2048;
const ATLAS_H: u32 = 1024;
const READ_W: u32 = 16;
const READ_H: u32 = 16;
const ORIGIN_X: u32 = 32;
const ORIGIN_Y: u32 = 64;
const ROW_PITCH: u32 = 256;
const SHADER: &str = r#"
@group(0) @binding(0) var atlas: texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    var p = array<vec2f, 3>(vec2f(-1, -1), vec2f(3, -1), vec2f(-1, 3));
    return vec4f(p[i], 0, 1);
}
@fragment fn fs(@builtin(position) p: vec4f) -> @location(0) vec4f {
    return textureLoad(atlas, vec2i(p.xy) + vec2i(32, 64), 0);
}
"#;

thread_local! { static RESULT: RefCell<CString> = RefCell::new(CString::new("{}").unwrap()); }

struct Trace { file: File, start: Instant, completed: u32, checked: u64 }
impl Trace {
    fn new(root: &Path, batched: bool) -> Result<Self> {
        std::fs::create_dir_all(root)?;
        let file = File::create(root.join(if batched { "wgpu-atlas-batched-probe-trace.jsonl" } else { "wgpu-atlas-probe-trace.jsonl" }))?;
        Ok(Self { file, start: Instant::now(), completed: 0, checked: 0 })
    }
    fn step(&mut self, stage: &str, detail: Value) -> Result<()> {
        serde_json::to_writer(&mut self.file, &json!({"stage":stage,"detail":detail,
            "elapsedMs":self.start.elapsed().as_millis(),"iterationsCompleted":self.completed,
            "pixelBytesChecked":self.checked}))?;
        self.file.write_all(b"\n")?;
        self.file.sync_all()?;
        Ok(())
    }
}

fn base_pixel(x: u32, y: u32) -> [u8; 4] {
    [((x * 13 + y * 7) & 255) as u8, ((x * 3 + y * 17) & 255) as u8,
     ((x ^ y) & 255) as u8, 255]
}

// BEGIN PURE BATCH MODEL
mod batch_model {
    pub fn batched_request(bytes: &[u8]) -> bool {
        serde_json::from_slice::<serde_json::Value>(bytes).ok()
            .and_then(|v| v.get("batched").and_then(|b| b.as_bool())).unwrap_or(false)
    }

    pub fn groups(iterations: u32) -> Vec<std::ops::Range<u32>> {
        (0..iterations).step_by(3).map(|first| first..(first + 3).min(iterations)).collect()
    }

    pub struct Glyph { pub x: u32, pub y: u32, pub width: u32, pub height: u32, pub bytes: Vec<u8> }
    pub fn glyph(iteration: u32) -> Glyph {
        let (x, y) = ((iteration * 3) % 8, (iteration * 5) % 8);
        let (width, height) = (3 + iteration % 5, 3 + (iteration / 3) % 5);
        let mut bytes = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height { for column in 0..width {
            bytes.extend_from_slice(&[((iteration*37+column*11+row*17)&255) as u8,
                ((iteration*53+column*19+row*7)&255) as u8, ((iteration*29+column*3+row*23)&255) as u8,255]);
        } }
        Glyph { x, y, width, height, bytes }
    }

    pub fn apply(expected: &mut [u8], glyph: &Glyph) {
        for y in 0..glyph.height as usize {
            let start = (((glyph.y as usize+y)*16+glyph.x as usize)*4) as usize;
            let source = y*glyph.width as usize*4;
            expected[start..start+glyph.width as usize*4].copy_from_slice(&glyph.bytes[source..source+glyph.width as usize*4]);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test] fn explicit_boolean_only_and_invalid_json_falls_back() {
            for request in [b"bad".as_slice(), b"{}", b"{\"batched\":false}", b"{\"batched\":1}", b"{\"batched\":\"true\"}", b"null"] { assert!(!batched_request(request)); }
            assert!(batched_request(br#"{"probe_kind":"wgpu_atlas","iterations":64,"batched":true}"#));
        }
        #[test] fn three_submission_groups_cover_exact_requested_iterations() {
            for count in 3..=256 {
                let plan = groups(count);
                assert_eq!(plan.iter().cloned().flatten().collect::<Vec<_>>(),(0..count).collect::<Vec<_>>());
                assert_eq!(plan[0].len(),3);
                assert!(plan.iter().all(|group|(1..=3).contains(&group.len())));
                assert!(plan.iter().take(plan.len()-1).all(|group|group.len()==3));
            }
        }
        #[test] fn glyph_regions_never_touch_unmodified_tile_border() {
            for i in 0..256 { let g=glyph(i);assert!(g.x+g.width<16);assert!(g.y+g.height<16);assert_eq!(g.bytes.len(),(g.width*g.height*4)as usize); }
        }
        #[test] fn cumulative_expected_matches_existing_device_control_checksum() {
            let mut tile=Vec::new();for y in 0..16 {for x in 0..16 {let (x,y)=(x+32,y+64);tile.extend_from_slice(&[((x*13+y*7)&255)as u8,((x*3+y*17)&255)as u8,((x^y)&255)as u8,255]);}}
            let mut checksum=0xcbf29ce484222325u64;
            for i in 0..64 {apply(&mut tile,&glyph(i));for &byte in &tile {checksum=(checksum^u64::from(byte)).wrapping_mul(0x100000001b3);}}
            // Independently captured by the original 64-round device probe.
            assert_eq!(checksum,0x65a56f7dc0365160);
        }
        #[test] fn expected_snapshots_do_not_change_with_later_uploads() {
            let mut tile=vec![99u8;16*16*4];apply(&mut tile,&glyph(0));let snapshot=tile.clone();apply(&mut tile,&glyph(1));
            assert_ne!(tile,snapshot);assert_eq!(snapshot[4*16*15..],vec![99u8;16*4]);
            assert_eq!(&snapshot[..4],&[0,0,0,255]);
        }
    }
}
// END PURE BATCH MODEL

struct BatchSlot { output: Texture, view: TextureView, readback: Buffer }

fn encode_sample(encoder: &mut CommandEncoder, view: &TextureView, pipeline: &RenderPipeline, group: &BindGroup) {
    let mut pass=encoder.begin_render_pass(&RenderPassDescriptor {
        label:Some("wgpu-atlas-batched-sample"),
        color_attachments:&[Some(RenderPassColorAttachment {view,depth_slice:None,resolve_target:None,
            ops:Operations {load:LoadOp::Clear(Color::BLACK),store:StoreOp::Store}})],
        depth_stencil_attachment:None,timestamp_writes:None,occlusion_query_set:None,multiview_mask:None,
    });
    pass.set_pipeline(pipeline);pass.set_bind_group(0,group,&[]);pass.draw(0..3,0..1);
}

#[allow(clippy::too_many_arguments)]
fn run_batched(device: &Device, queue: &Queue, atlas: &Texture, pipeline: &RenderPipeline,
    group: &BindGroup, first_output: &Texture, first_view: &TextureView, first_readback: &Buffer,
    errors: &Mutex<Vec<String>>, iterations: u32, expected: &mut [u8], trace: &mut Trace) -> Result<(u64,Value)> {
    let mut slots=vec![BatchSlot { output:first_output.clone(), view:first_view.clone(), readback:first_readback.clone() }];
    for _ in 1..3 {
        let output=device.create_texture(&TextureDescriptor {label:Some("wgpu-atlas-batched-output"),
            size:Extent3d {width:READ_W,height:READ_H,depth_or_array_layers:1},mip_level_count:1,sample_count:1,
            dimension:TextureDimension::D2,format:TextureFormat::Rgba8Unorm,
            usage:TextureUsages::RENDER_ATTACHMENT|TextureUsages::COPY_SRC,view_formats:&[]});
        let view=output.create_view(&TextureViewDescriptor::default());
        let readback=device.create_buffer(&BufferDescriptor {label:Some("wgpu-atlas-batched-readback"),
            size:u64::from(ROW_PITCH*READ_H),usage:BufferUsages::MAP_READ|BufferUsages::COPY_DST,mapped_at_creation:false});
        slots.push(BatchSlot {output,view,readback});
    }
    let mut checksum=0xcbf29ce484222325u64;
    let mut submitted_batches=0u32;let mut polls=0u32;let mut peak_outstanding=0usize;
    for planned in batch_model::groups(iterations) {
        let mut pending=Vec::new();let mut last_submission=None;
        for iteration in planned {
            let slot=&slots[pending.len()];
            // Record a real pass before the glyph write, but leave this encoder
            // open. Wgpu pending queue writes are prepended to the next submit,
            // so both recorded sample passes must see this batch's updated tile.
            let mut first=device.create_command_encoder(&CommandEncoderDescriptor {label:Some("wgpu-atlas-batched-open")});
            encode_sample(&mut first,&slot.view,pipeline,group);
            let glyph=batch_model::glyph(iteration);batch_model::apply(expected,&glyph);
            trace.step("batched-open-encoder-before-glyph-upload",json!({"iteration":iteration,"slot":pending.len(),
                "origin":[ORIGIN_X+glyph.x,ORIGIN_Y+glyph.y],"size":[glyph.width,glyph.height],"recordedSamplePasses":1}))?;
            queue.write_texture(TexelCopyTextureInfo {texture:atlas,mip_level:0,
                origin:Origin3d {x:ORIGIN_X+glyph.x,y:ORIGIN_Y+glyph.y,z:0},aspect:TextureAspect::All},&glyph.bytes,
                TexelCopyBufferLayout {offset:0,bytes_per_row:Some(glyph.width*4),rows_per_image:Some(glyph.height)},
                Extent3d {width:glyph.width,height:glyph.height,depth_or_array_layers:1});
            let mut second=device.create_command_encoder(&CommandEncoderDescriptor {label:Some("wgpu-atlas-batched-second")});
            encode_sample(&mut second,&slot.view,pipeline,group);
            let mut copy=device.create_command_encoder(&CommandEncoderDescriptor {label:Some("wgpu-atlas-batched-readback")});
            copy.copy_texture_to_buffer(TexelCopyTextureInfo {texture:&slot.output,mip_level:0,origin:Origin3d::ZERO,aspect:TextureAspect::All},
                TexelCopyBufferInfo {buffer:&slot.readback,layout:TexelCopyBufferLayout {offset:0,bytes_per_row:Some(ROW_PITCH),rows_per_image:Some(READ_H)}},
                Extent3d {width:READ_W,height:READ_H,depth_or_array_layers:1});
            trace.step("batched-before-submit",json!({"iteration":iteration,"commandBuffers":3,"outstandingBeforeSubmit":pending.len()}))?;
            last_submission=Some(queue.submit([first.finish(),second.finish(),copy.finish()]));
            submitted_batches+=1;pending.push((iteration,expected.to_vec()));
            peak_outstanding=peak_outstanding.max(pending.len());
            trace.step("batched-after-submit",json!({"iteration":iteration,"outstandingWithoutExplicitPoll":pending.len()}))?;
        }
        let mut receivers=Vec::new();
        for slot in slots.iter().take(pending.len()) {
            let (tx,rx)=mpsc::channel();slot.readback.slice(..).map_async(MapMode::Read,move|status|{let _=tx.send(status.map_err(|e|e.to_string()));});receivers.push(rx);
        }
        trace.step("batched-before-poll",json!({"submittedBeforePoll":pending.len(),"tailGroup":pending.len()<3}))?;
        device.poll(PollType::Wait {submission_index:last_submission,timeout:Some(Duration::from_secs(30))})?;
        polls+=1;check_errors(errors)?;
        trace.step("batched-after-poll",json!({"submittedBeforePoll":pending.len()}))?;
        for (index,(iteration,want)) in pending.into_iter().enumerate() {
            receivers[index].recv_timeout(Duration::from_secs(5)).context("batched readback callback timed out")?.map_err(anyhow::Error::msg)?;
            {
                let mapped=slots[index].readback.slice(..).get_mapped_range().context("batched readback mapped range")?;
                for y in 0..READ_H as usize {for x in 0..(READ_W*4)as usize {
                    let actual=mapped[y*ROW_PITCH as usize+x];let expected=want[y*(READ_W*4)as usize+x];
                    anyhow::ensure!(actual==expected,"batched pixel mismatch iteration={iteration} x={} y={y} channel={} actual={actual} expected={expected}",x/4,x%4);
                    checksum=(checksum^u64::from(actual)).wrapping_mul(0x100000001b3);trace.checked+=1;
                }}
            }
            slots[index].readback.unmap();trace.completed+=1;
            trace.step("batched-round-verified",json!({"iteration":iteration,"bytes":READ_W*READ_H*4}))?;
        }
    }
    anyhow::ensure!(peak_outstanding>=3,"batched probe did not reach 3 submissions before explicit poll");
    trace.step("before-batched-resource-drop",json!({"peakOutstandingSubmissions":peak_outstanding}))?;
    drop(slots);
    trace.step("after-batched-resource-drop",json!({}))?;
    Ok((checksum,json!({"submittedBatches":submitted_batches,"commandBuffersPerSubmit":3,
        "explicitBatchPolls":polls,"peakOutstandingSubmissionsBeforePoll":peak_outstanding,
        "encoderOpenDuringPartialUpload":true,"scope":"Outstanding means submitted without an explicit probe device.poll; queue.submit may perform internal maintenance and GPU completion can occur independently."})))
}

fn check_errors(errors: &Mutex<Vec<String>>) -> Result<()> {
    let errors = errors.lock().map_err(|_| anyhow::anyhow!("GPU error lock poisoned"))?;
    anyhow::ensure!(errors.is_empty(), "wgpu validation/device error: {}", errors.join("; "));
    Ok(())
}

fn run(root: &Path, iterations: u32, batched: bool, trace: &mut Trace) -> Result<Value> {
    anyhow::ensure!((1..=256).contains(&iterations), "iterations must be 1..256");
    anyhow::ensure!(!batched || iterations >= 3, "batched mode requires at least 3 iterations");
    anyhow::ensure!(crate::RT.get().is_none(), "atlas probe requires an independent process before game startup");
    trace.step("begin", json!({"probeKind":"wgpu_atlas","iterations":iterations,"batched":batched,
        "atlas":[ATLAS_W,ATLAS_H],"readback":[READ_W,READ_H],"astc":false,
        "scope":"wgpu offscreen queue upload, fragment textureLoad, readback and completed command-pool retirement"}))?;
    // The worker has not started a game/native surface. Give the shared HAL's
    // existing bounded diagnostics independent paths for this control run.
    let prefix = if batched { "wgpu-atlas-batched" } else { "wgpu-atlas" };
    let sync_submit = std::fs::read(root.join("vulkan-buffer-probe-request.json")).ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|request| request.get("sync_submit").and_then(Value::as_bool)).unwrap_or(false);
    crate::config::configure_sync_submit(sync_submit, &root.join(format!("{prefix}-submit-trace")));
    std::env::set_var("MUGUET_VK_POOL_TRACE", root.join(format!("{prefix}-pool-last.txt")));
    std::env::set_var("MUGUET_DEVICE_FAULT_DIR", root.join(format!("{prefix}-device-fault")));
    std::env::set_var("MUGUET_IMAGE_SYNC_TRACE", root.join(format!("{prefix}-image-sync-first.txt")));
    std::env::set_var("MUGUET_ATLAS_SYNC_TRACE", root.join(format!("{prefix}-sync.txt")));
    std::env::set_var("MUGUET_PIPELINE_TRACE_DIR", root.join(format!("{prefix}-pipeline-trace")));
    let mut desc = InstanceDescriptor::new_without_display_handle();
    desc.backends = Backends::VULKAN;
    let instance = Instance::new(desc);
    trace.step("before-adapter", json!({}))?;
    let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
        power_preference: PowerPreference::HighPerformance, compatible_surface: None,
        force_fallback_adapter: false, apply_limit_buckets: false,
    }))?;
    let info = adapter.get_info();
    let adapter_info = json!({"name":info.name,"vendor":info.vendor,"device":info.device,
        "driver":info.driver,"driverInfo":info.driver_info,"backend":format!("{:?}",info.backend)});
    trace.step("adapter", adapter_info.clone())?;
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("wgpu-atlas-probe"), required_features: Features::empty(),
        required_limits: adapter.limits(), ..Default::default()
    }))?;
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = errors.clone();
    device.on_uncaptured_error(Arc::new(move |e: Error| {
        if let Ok(mut list) = seen.lock() { if list.len() < 16 { list.push(e.to_string()); } }
    }));
    let seen = errors.clone();
    device.set_device_lost_callback(move |reason, message| {
        if let Ok(mut list) = seen.lock() { if list.len() < 16 { list.push(format!("device lost {reason:?}: {message}")); } }
    });
    let atlas = device.create_texture(&TextureDescriptor {
        label: Some("wgpu-atlas-probe-2048x1024"),
        size: Extent3d { width: ATLAS_W, height: ATLAS_H, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST, view_formats: &[],
    });
    let output = device.create_texture(&TextureDescriptor {
        label: Some("wgpu-atlas-probe-output"),
        size: Extent3d { width: READ_W, height: READ_H, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC, view_formats: &[],
    });
    let output_view = output.create_view(&TextureViewDescriptor::default());
    let atlas_view = atlas.create_view(&TextureViewDescriptor::default());
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("wgpu-atlas-probe-readback"), size: u64::from(ROW_PITCH * READ_H),
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST, mapped_at_creation: false,
    });
    trace.step("before-pipeline", json!({"shader":"textureLoad RGBA8; no ASTC, surface or guest"}))?;
    let module = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("wgpu-atlas-probe"), source: ShaderSource::Wgsl(SHADER.into()) });
    let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("wgpu-atlas-probe"), layout: None,
        vertex: VertexState { module: &module, entry_point: Some("vs"),
            compilation_options: Default::default(), buffers: &[] },
        primitive: PrimitiveState::default(), depth_stencil: None,
        multisample: MultisampleState::default(),
        fragment: Some(FragmentState { module: &module, entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(ColorTargetState { format: TextureFormat::Rgba8Unorm,
                blend: None, write_mask: ColorWrites::ALL })] }),
        multiview_mask: None, cache: None,
    });
    let bind_group = device.create_bind_group(&BindGroupDescriptor {
        label: Some("wgpu-atlas-probe"), layout: &pipeline.get_bind_group_layout(0),
        entries: &[BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&atlas_view) }],
    });
    check_errors(&errors)?;
    let mut full = Vec::with_capacity((ATLAS_W * ATLAS_H * 4) as usize);
    for y in 0..ATLAS_H { for x in 0..ATLAS_W { full.extend_from_slice(&base_pixel(x,y)); } }
    let mut expected = Vec::with_capacity((READ_W * READ_H * 4) as usize);
    for y in 0..READ_H { for x in 0..READ_W { expected.extend_from_slice(&base_pixel(ORIGIN_X+x,ORIGIN_Y+y)); } }
    trace.step("before-full-upload", json!({"bytes":full.len()}))?;
    queue.write_texture(TexelCopyTextureInfo { texture:&atlas, mip_level:0,
        origin:Origin3d::ZERO, aspect:TextureAspect::All }, &full,
        TexelCopyBufferLayout { offset:0, bytes_per_row:Some(ATLAS_W*4), rows_per_image:Some(ATLAS_H) },
        Extent3d { width:ATLAS_W,height:ATLAS_H,depth_or_array_layers:1 });
    // Complete the full upload separately, just as initial atlas allocation can
    // precede later partial glyph uploads in a different queue submission.
    let upload = queue.submit([]);
    trace.step("before-full-upload-poll", json!({}))?;
    device.poll(PollType::Wait { submission_index:Some(upload), timeout:Some(Duration::from_secs(30)) })?;
    check_errors(&errors)?;
    trace.step("after-full-upload-poll", json!({}))?;
    drop(full);
    let mut checksum = 0xcbf29ce484222325u64;
    let mut batch_stats = None;
    if batched {
        let (value, stats) = run_batched(&device, &queue, &atlas, &pipeline, &bind_group, &output, &output_view, &readback, &errors, iterations, &mut expected, trace)?;
        checksum = value;
        batch_stats = Some(stats);
    } else {
    for iteration in 0..iterations {
        // Vary offset and row width, retain unmodified pixels from previous
        // iterations, and verify the entire sampled tile including neighbors.
        let gx = (iteration * 3) % 8;
        let gy = (iteration * 5) % 8;
        let gw = 3 + iteration % 5;
        let gh = 3 + (iteration / 3) % 5;
        let mut glyph = Vec::with_capacity((gw*gh*4) as usize);
        for y in 0..gh { for x in 0..gw {
            let value = [((iteration*37+x*11+y*17)&255) as u8,
                ((iteration*53+x*19+y*7)&255) as u8, ((iteration*29+x*3+y*23)&255) as u8, 255];
            glyph.extend_from_slice(&value);
            let offset = (((gy+y)*READ_W+gx+x)*4) as usize;
            expected[offset..offset+4].copy_from_slice(&value);
        } }
        trace.step("before-glyph-upload", json!({"iteration":iteration,"origin":[ORIGIN_X+gx,ORIGIN_Y+gy],"size":[gw,gh]}))?;
        queue.write_texture(TexelCopyTextureInfo { texture:&atlas,mip_level:0,
            origin:Origin3d {x:ORIGIN_X+gx,y:ORIGIN_Y+gy,z:0},aspect:TextureAspect::All }, &glyph,
            TexelCopyBufferLayout {offset:0,bytes_per_row:Some(gw*4),rows_per_image:Some(gh)},
            Extent3d {width:gw,height:gh,depth_or_array_layers:1});
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {label:Some("wgpu-atlas-probe-round")});
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label:Some("wgpu-atlas-probe-sample"),
                color_attachments:&[Some(RenderPassColorAttachment {view:&output_view,depth_slice:None,
                    resolve_target:None,ops:Operations {load:LoadOp::Clear(Color::BLACK),store:StoreOp::Store}})],
                depth_stencil_attachment:None,timestamp_writes:None,occlusion_query_set:None,multiview_mask:None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0,&bind_group,&[]);
            pass.draw(0..3,0..1);
        }
        encoder.copy_texture_to_buffer(TexelCopyTextureInfo {texture:&output,mip_level:0,origin:Origin3d::ZERO,aspect:TextureAspect::All},
            TexelCopyBufferInfo {buffer:&readback,layout:TexelCopyBufferLayout {offset:0,bytes_per_row:Some(ROW_PITCH),rows_per_image:Some(READ_H)}},
            Extent3d {width:READ_W,height:READ_H,depth_or_array_layers:1});
        trace.step("before-submit", json!({"iteration":iteration}))?;
        let submitted = queue.submit([encoder.finish()]);
        let (tx,rx) = mpsc::channel();
        readback.slice(..).map_async(MapMode::Read,move |status| {let _=tx.send(status.map_err(|e|e.to_string()));});
        trace.step("before-poll", json!({"iteration":iteration}))?;
        device.poll(PollType::Wait {submission_index:Some(submitted),timeout:Some(Duration::from_secs(30))})?;
        rx.recv_timeout(Duration::from_secs(5)).context("readback callback timed out")?
            .map_err(anyhow::Error::msg)?;
        check_errors(&errors)?;
        trace.step("after-poll", json!({"iteration":iteration}))?;
        {
            let mapped = readback.slice(..).get_mapped_range().context("readback mapped range")?;
            for y in 0..READ_H as usize { for x in 0..(READ_W*4) as usize {
                let actual=mapped[y*ROW_PITCH as usize+x];
                let want=expected[y*(READ_W*4)as usize+x];
                anyhow::ensure!(actual==want,"pixel mismatch iteration={iteration} x={} y={y} channel={} actual={actual} expected={want}",x/4,x%4);
                checksum=(checksum^u64::from(actual)).wrapping_mul(0x100000001b3);
                trace.checked+=1;
            } }
        }
        readback.unmap();
        trace.completed+=1;
        trace.step("round-verified", json!({"iteration":iteration,"bytes":READ_W*READ_H*4}))?;
    }
    }
    trace.step("before-resource-drop", json!({}))?;
    drop(bind_group);drop(pipeline);drop(module);drop(atlas_view);drop(output_view);
    drop(readback);drop(output);drop(atlas);
    device.poll(PollType::Wait {submission_index:None,timeout:Some(Duration::from_secs(30))})?;
    check_errors(&errors)?;
    trace.step("after-resource-drop-poll", json!({}))?;
    // Include Device/Queue teardown in the control instead of returning pass
    // while a later destructor could still fault in the driver.
    drop(queue);drop(device);drop(adapter);drop(instance);
    trace.step("pass", json!({"checksumFNV1a64":format!("{checksum:016x}")}))?;
    Ok(json!({"probeKind":"wgpu_atlas","pass":true,"status":"pass",
        "iterationsRequested":iterations,"iterationsCompleted":trace.completed,"batched":batched,"batchStats":batch_stats,
        "pixelBytesChecked":trace.checked,"bytesPerRound":READ_W*READ_H*4,
        "checksumFNV1a64":format!("{checksum:016x}"),"adapter":adapter_info,
        "atlasWidth":ATLAS_W,"atlasHeight":ATLAS_H,"astc":false,
        "scope":if batched { "Offscreen wgpu open-encoder partial uploads, 3 command buffers per submit, 3 submissions before explicit poll, byte-exact per-submission readback including neighbors and full teardown. Does not establish game or surface execution." } else { "Offscreen wgpu queue full/partial upload, fragment textureLoad, byte-exact readback, per-round submit/poll and full teardown; does not establish game or surface execution." }}))
}

/// Called synchronously by the separate Harmony diagnostic worker.
#[no_mangle]
pub unsafe extern "C" fn mg_wgpu_atlas_probe_json(files_dir:*const c_char,iterations:u32)->*const c_char {
    let result=(||->Result<Value>{
        anyhow::ensure!(!files_dir.is_null(),"null files directory");
        let root=Path::new(unsafe{CStr::from_ptr(files_dir)}.to_str()?);
        let batched=std::fs::read(root.join("vulkan-buffer-probe-request.json")).ok().is_some_and(|bytes|batch_model::batched_request(&bytes));
        let mut trace=Trace::new(root,batched)?;
        match run(root,iterations,batched,&mut trace) {
            Ok(value)=>Ok(value),
            Err(error)=>{
                let message=format!("{error:#}");
                let _=trace.step("fail",json!({"error":message}));
                Ok(json!({"probeKind":"wgpu_atlas","pass":false,"status":"fail","error":message,
                    "iterationsRequested":iterations,"iterationsCompleted":trace.completed,"pixelBytesChecked":trace.checked,"batched":batched}))
            }
        }
    })().unwrap_or_else(|e|json!({"probeKind":"wgpu_atlas","pass":false,"status":"fail",
        "iterationsRequested":iterations,"iterationsCompleted":0,"pixelBytesChecked":0,"error":format!("{e:#}")}));
    RESULT.with(|slot|{let mut slot=slot.borrow_mut();*slot=CString::new(result.to_string()).expect("serialized JSON contains no NUL");slot.as_ptr()})
}
