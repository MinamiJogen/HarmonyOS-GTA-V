//! Opt-in native XComponent clear/present control before Wasmtime or game AOT.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{fs::File, io::Write, path::Path, sync::{Arc, Mutex}, time::{Duration, Instant}};
use wgpu::*;

#[path = "surface_probe_plan.rs"]
mod plan;

const MAX_TRACE_RECORDS: u32 = 65_536;
const MAX_TRACE_BYTES: u64 = 16 << 20;

pub(crate) fn requested(root: &Path) -> bool { root.join("surface-probe-request.json").is_file() }

struct Trace {
    file: File,
    start: Instant,
    records: u32,
    bytes: u64,
    truncated: bool,
    submitted: u32,
    presented: u32,
    completed: u32,
    polls: u32,
}

impl Trace {
    fn new(root: &Path) -> Result<Self> {
        Ok(Self { file: File::create(root.join("surface-probe-trace.jsonl"))?, start: Instant::now(),
            records: 0, bytes: 0, truncated: false, submitted: 0, presented: 0, completed: 0, polls: 0 })
    }

    fn step(&mut self, stage: &str, detail: Value) -> Result<()> {
        if self.truncated { return Ok(()); }
        let mut record = serde_json::to_vec(&json!({"stage":stage,"detail":detail,
            "elapsedMs":self.start.elapsed().as_millis(),"framesSubmitted":self.submitted,
            "presentCalls":self.presented,"framesCompletedAtExplicitPoll":self.completed,"explicitPolls":self.polls}))?;
        record.push(b'\n');
        if self.records >= MAX_TRACE_RECORDS || self.bytes + record.len() as u64 + 256 > MAX_TRACE_BYTES {
            self.truncated = true;
            record = serde_json::to_vec(&json!({"stage":"TRUNCATED","maxRecords":MAX_TRACE_RECORDS,"maxBytes":MAX_TRACE_BYTES}))?;
            record.push(b'\n');
        }
        self.file.write_all(&record)?;
        self.file.sync_all()?;
        self.records += 1;
        self.bytes += record.len() as u64;
        Ok(())
    }
}

fn checkpoint(root: &Path, value: &Value) -> Result<()> {
    let mut file = File::create(root.join("surface-probe-result.json"))?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn check_errors(errors: &Mutex<Vec<String>>) -> Result<()> {
    let errors = errors.lock().map_err(|_| anyhow::anyhow!("surface probe error lock poisoned"))?;
    anyhow::ensure!(errors.is_empty(), "surface probe wgpu/device error: {}", errors.join("; "));
    Ok(())
}

pub(crate) fn run_requested(root: &Path, width: u32, height: u32) -> Result<()> {
    let mut trace = Trace::new(root)?;
    checkpoint(root, &json!({"probe":"wgpu_native_surface_clear_present","status":"running",
        "framesSubmitted":0,"presentCalls":0,"framesCompletedAtExplicitPoll":0}))?;
    let result = run(root, width, height, &mut trace);
    match result {
        Ok(value) => checkpoint(root, &value),
        Err(error) => {
            let _ = trace.step("failed", json!({"error":format!("{error:#}")}));
            checkpoint(root, &json!({"probe":"wgpu_native_surface_clear_present","status":"fail",
                "error":format!("{error:#}"),"framesSubmitted":trace.submitted,"presentCalls":trace.presented,
                "framesCompletedAtExplicitPoll":trace.completed,"explicitPolls":trace.polls,
                "traceTruncated":trace.truncated,"gameAotLoaded":false,"scope":"Native surface diagnostic only; not game playability."}))?;
            Err(error)
        }
    }
}

fn run(root: &Path, width: u32, height: u32, trace: &mut Trace) -> Result<Value> {
    anyhow::ensure!(crate::RT.get().is_none(), "surface probe must run before Wasmtime/AOT startup");
    let request: Value = serde_json::from_slice(&std::fs::read(root.join("surface-probe-request.json"))?)
        .context("surface probe request JSON")?;
    anyhow::ensure!(request.is_object(), "surface probe request must be a JSON object");
    let frames = match request.get("frames") {
        None => plan::DEFAULT_FRAMES,
        Some(value) => u32::try_from(value.as_u64().context("frames must be an unsigned integer")?)?,
    };
    let groups = plan::groups(frames).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(width > 0 && height > 0, "surface probe dimensions must be positive");
    std::env::set_var("MUGUET_VK_POOL_TRACE", root.join("surface-probe-pool-last.txt"));
    std::env::set_var("MUGUET_DEVICE_FAULT_DIR", root.join("surface-probe-device-fault"));
    let sync_submit = request.get("sync_submit").and_then(Value::as_bool).unwrap_or(false);
    crate::config::configure_sync_submit(sync_submit, &root.join("surface-probe-submit-trace"));
    std::env::set_var("MUGUET_IMAGE_SYNC_TRACE", root.join("surface-probe-image-sync-first.txt"));
    std::env::set_var("MUGUET_ATLAS_SYNC_TRACE", root.join("surface-probe-atlas-sync.txt"));
    std::env::set_var("MUGUET_GPU_UPLOAD_TRACE", root.join("surface-probe-buffer-upload-last.txt"));
    std::env::set_var("MUGUET_VK_COPY_TRACE", root.join("surface-probe-vk-copy-last.txt"));
    std::env::set_var("MUGUET_PIPELINE_TRACE_DIR", root.join("surface-probe-pipeline-trace"));
    trace.step("begin", json!({"requestedFrames":frames,"width":width,"height":height,
        "submissionsPerExplicitPoll":plan::SUBMISSIONS_PER_POLL,"gameAotLoaded":false,
        "scope":"Clear/present through the existing XComponent surface and patched wgpu, no application shaders, textures, audio, Wasmtime or game assets."}))?;
    let crate::gpu::SurfaceHandoff { instance, surface, window } = crate::gpu::SURFACE.lock().unwrap()
        .take().context("surface probe has no prepared XComponent surface")?;
    trace.step("before-adapter", json!({"compatibleSurface":true}))?;
    let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
        power_preference: PowerPreference::HighPerformance, compatible_surface: Some(&surface),
        force_fallback_adapter: false, apply_limit_buckets: false,
    }))?;
    let info = adapter.get_info();
    let adapter_info = json!({"name":info.name,"vendor":info.vendor,"device":info.device,
        "driver":info.driver,"driverInfo":info.driver_info,"backend":format!("{:?}",info.backend)});
    // Keep the same requested feature intersection and limits as Gpu::new.
    let want = Features::TEXTURE_COMPRESSION_BC | Features::TEXTURE_COMPRESSION_BC_SLICED_3D | Features::DEPTH32FLOAT_STENCIL8
        | Features::FLOAT32_FILTERABLE | Features::INDIRECT_FIRST_INSTANCE | Features::RG11B10UFLOAT_RENDERABLE
        | Features::BGRA8UNORM_STORAGE | Features::DEPTH_CLIP_CONTROL | Features::CLIP_DISTANCES | Features::DUAL_SOURCE_BLENDING
        | Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES | Features::TEXTURE_COMPRESSION_ASTC;
    let features = want & adapter.features();
    trace.step("before-device", json!({"adapter":adapter_info,"requiredFeatures":format!("{features:?}"),"limits":"adapter.limits()"}))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("muguet-surface-probe"), required_features: features,
        required_limits: adapter.limits(), ..Default::default()
    }))?;
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = errors.clone();
    device.on_uncaptured_error(Arc::new(move |error: Error| {
        if let Ok(mut values) = seen.lock() { if values.len() < 32 { values.push(error.to_string()); } }
    }));
    let seen = errors.clone();
    device.set_device_lost_callback(move |reason, message| {
        if let Ok(mut values) = seen.lock() { if values.len() < 32 { values.push(format!("device lost {reason:?}: {message}")); } }
    });
    let caps = surface.get_capabilities(&adapter);
    let format = caps.formats.iter().copied().find(|format| !format.is_srgb())
        .or_else(|| caps.formats.first().copied()).context("surface has no supported formats")?;
    let alpha_mode = caps.alpha_modes.first().copied().context("surface has no alpha modes")?;
    let present_mode = if caps.present_modes.contains(&PresentMode::Mailbox) { PresentMode::Mailbox } else { PresentMode::Fifo };
    window.set_buffer_geometry(width, height)?;
    trace.step("before-surface-configure", json!({"format":format!("{format:?}"),"presentMode":format!("{present_mode:?}"),
        "alphaMode":format!("{alpha_mode:?}"),"desiredMaximumFrameLatency":2,"width":width,"height":height}))?;
    surface.configure(&device, &SurfaceConfiguration {
        usage: TextureUsages::RENDER_ATTACHMENT, format, width, height, present_mode,
        desired_maximum_frame_latency: 2, alpha_mode, view_formats: vec![], color_space: SurfaceColorSpace::Auto,
    });
    check_errors(&errors)?;
    let mut peak_outstanding_without_explicit_poll = 0;
    for group in groups {
        let group_len = group.len() as u32;
        let mut last_submission = None;
        for frame_index in group {
            anyhow::ensure!(!crate::ffi::stop_requested(), "surface probe interrupted by stop request");
            trace.step("before-acquire", json!({"frame":frame_index}))?;
            let frame = match surface.get_current_texture() {
                CurrentSurfaceTexture::Success(frame) | CurrentSurfaceTexture::Suboptimal(frame) => frame,
                other => anyhow::bail!("surface probe acquire failed at frame{frame_index}: {other:?}"),
            };
            let view = frame.texture.create_view(&TextureViewDescriptor::default());
            let rgba = plan::clear_color(frame_index);
            let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("surface-probe-clear") });
            {
                let _pass = encoder.begin_render_pass(&RenderPassDescriptor {
                    label: Some("surface-probe-clear"),
                    color_attachments: &[Some(RenderPassColorAttachment {
                        view: &view, depth_slice: None, resolve_target: None,
                        ops: Operations { load: LoadOp::Clear(Color { r: rgba[0], g: rgba[1], b: rgba[2], a: rgba[3] }), store: StoreOp::Store },
                    })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
                });
            }
            trace.step("before-submit", json!({"frame":frame_index,"clearRgba":rgba,"commandBuffers":1}))?;
            last_submission = Some(queue.submit([encoder.finish()]));
            trace.submitted += 1;
            trace.step("before-present", json!({"frame":frame_index}))?;
            queue.present(frame);
            trace.presented += 1;
            drop(view);
            trace.step("after-present", json!({"frame":frame_index}))?;
        }
        peak_outstanding_without_explicit_poll = peak_outstanding_without_explicit_poll.max(group_len);
        trace.step("before-batch-poll", json!({"submittedSinceLastExplicitPoll":group_len}))?;
        device.poll(PollType::Wait { submission_index: last_submission, timeout: Some(Duration::from_secs(30)) })?;
        trace.polls += 1;
        trace.completed += group_len;
        check_errors(&errors)?;
        trace.step("after-batch-poll", json!({"submittedSinceLastExplicitPoll":group_len}))?;
        checkpoint(root, &json!({"probe":"wgpu_native_surface_clear_present","status":"running","requestedFrames":frames,
            "framesSubmitted":trace.submitted,"presentCalls":trace.presented,"framesCompletedAtExplicitPoll":trace.completed,
            "explicitPolls":trace.polls,"traceTruncated":trace.truncated,"gameAotLoaded":false}))?;
    }
    anyhow::ensure!(trace.presented == frames && trace.completed == frames, "surface probe did not complete requested frames");
    anyhow::ensure!(peak_outstanding_without_explicit_poll == 3, "surface probe did not reach three submissions before poll");
    trace.step("before-surface-drop", json!({}))?;
    drop(surface);
    trace.step("after-surface-drop-before-final-poll", json!({}))?;
    device.poll(PollType::Wait { submission_index: None, timeout: Some(Duration::from_secs(30)) })?;
    check_errors(&errors)?;
    trace.step("before-wgpu-device-teardown", json!({}))?;
    drop(queue); drop(device); drop(adapter); drop(instance); drop(window);
    trace.step("after-wgpu-surface-device-teardown", json!({}))?;
    check_errors(&errors)?;
    Ok(json!({"probe":"wgpu_native_surface_clear_present","status":"pass","requestedFrames":frames,
        "framesSubmitted":trace.submitted,"presentCalls":trace.presented,"framesCompletedAtExplicitPoll":trace.completed,
        "explicitBatchPolls":trace.polls,"peakSubmissionsWithoutExplicitPoll":peak_outstanding_without_explicit_poll,
        "adapter":adapter_info,"requiredFeatures":format!("{features:?}"),"format":format!("{format:?}"),
        "presentMode":format!("{present_mode:?}"),"alphaMode":format!("{alpha_mode:?}"),"renderSize":[width,height],
        "desiredMaximumFrameLatency":2,"elapsedMs":trace.start.elapsed().as_millis(),"wgpuErrors":errors.lock().unwrap().clone(),
        "wgpuSurfaceDeviceTeardownCompleted":true,"gameAotLoaded":false,"traceTruncated":trace.truncated,
        "scope":"Native surface clear/submit/present control, no game world or pixel readback proof. Outstanding means no explicit probe poll; submit/acquire can maintain or wait internally. NativeWindow remains owned by the XComponent/platform globals until process exit. Wgpu internal validation shaders may be initialized."}))
}
