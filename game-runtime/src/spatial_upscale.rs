//! Explicit, default-OFF public XEngine spatial upscaling. Raw recording uses
//! its own encoder because wgpu30 forbids mixing Raw and Wgpu encoding APIs.
use anyhow::{Context, Result};
use ash::vk::Handle;
use std::{ffi::{c_char, c_void, CStr, OsString}, ptr::NonNull, sync::{Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering}}};
use wgpu::*;
use std::time::{Duration, Instant};

// Byte-identical shader shared by ordinary presentation, conversion and probe.
pub(crate) const BLIT_SHADER: &str = "@group(0) @binding(0) var src: texture_2d<f32>;
                     @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f { var p = array<vec2f, 3>(vec2f(-1, -1), vec2f(3, -1), vec2f(-1, 3)); return vec4f(p[i], 0, 1); }
                     @fragment fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f { return textureLoad(src, vec2i(pos.xy), 0); }";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode { Off, NativeQuality, Performance }
impl Mode {
    pub(crate) fn name(self) -> &'static str { match self {
        Self::Off => "off", Self::NativeQuality => "native_quality", Self::Performance => "performance" } }
    pub(crate) fn sizes(self) -> Option<((u32,u32),(u32,u32))> { match self {
        Self::Off => None, Self::NativeQuality => Some(((1202,800),(1802,1200))),
        Self::Performance => Some(((802,534),(1202,800))) } }
}
#[derive(Clone, Copy)]
struct Settings {
    enabled: bool, mode: Mode, sharpness: f32, sizes: Option<((u32,u32),(u32,u32))>,
    match_display: bool, short_edges: Option<(u32,u32)>, generation: u64,
}
impl Settings {
    fn requested(self) -> bool { self.enabled && self.mode != Mode::Off }
    fn display_sizes(self, display:(u32,u32)) -> Option<((u32,u32),(u32,u32))> {
        if let Some((input,output))=self.short_edges {
            crate::spatial_dimensions::short_edge_sizes(input,output,display)
        } else {
            self.mode.sizes().map(|(input,output)|
                (crate::spatial_dimensions::fit_aspect(input,display),
                 crate::spatial_dimensions::fit_aspect(output,display)))
        }
    }
    fn target(self) -> Option<crate::spatial_resize_policy::ResizeTarget> {
        if !self.requested() { return None; }
        self.sizes.map(|(input, output)| crate::spatial_resize_policy::ResizeTarget {
            input, output, generation: self.generation,
        })
    }
}
static SETTINGS: Mutex<Settings> = Mutex::new(Settings {
    enabled: true, mode: Mode::Off, sharpness: 0.2, sizes: None, match_display: false, short_edges: None, generation: 0,
});
static STATUS: Mutex<Option<serde_json::Value>> = Mutex::new(None);
static RECORDED: AtomicU64 = AtomicU64::new(0);
static RESIZES: AtomicU64 = AtomicU64::new(0);
static PENDING_DISPLAY: Mutex<Option<((u32,u32), Instant)>> = Mutex::new(None);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static INPUT_PATH: AtomicU8 = AtomicU8::new(0);
static ORIGINAL_UNIQUE_HANDLES: OnceLock<Option<OsString>> = OnceLock::new();

/// Cheap immutable identity for cached output pixels. This includes processing
/// switches and all pixel-affecting settings, not just the configured extent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OutputCacheSignature {
    pub enabled: bool, pub mode: Mode, pub generation: u64,
    pub sizes: Option<((u32,u32),(u32,u32))>, pub sharpness_bits: u32,
}
impl OutputCacheSignature {
    pub(crate) fn requested(self) -> bool { self.enabled && self.mode != Mode::Off }
}
pub(crate) fn output_cache_signature() -> OutputCacheSignature {
    let s = *SETTINGS.lock().unwrap();
    OutputCacheSignature { enabled:s.enabled, mode:s.mode, generation:s.generation,
        sizes:s.sizes, sharpness_bits:s.sharpness.to_bits() }
}

pub(crate) fn parse_mode(value: Option<&serde_json::Value>) -> Mode {
    match value.and_then(|v| v.as_str()) {
        Some("native_quality") => Mode::NativeQuality,
        Some("performance") => Mode::Performance,
        _ => Mode::Off,
    }
}
pub(crate) fn configure(cfg: &serde_json::Value) {
    let enabled = crate::config::processing_enabled(cfg, "spatial_upscale_enabled");
    let mut mode = parse_mode(cfg.get("spatial_upscale_mode"));
    let sharpness = cfg.get("spatial_upscale_sharpness").and_then(|v| v.as_f64())
        .filter(|v| v.is_finite() && (0.0..=1.0).contains(v)).unwrap_or(0.2) as f32;
    let mut match_display = cfg.get("spatial_upscale_match_display").and_then(|v| v.as_bool()).unwrap_or(false);
    let display = crate::ffi::physical_size();
    // GPU probes omit match_display and retain their original exact fixtures.
    // The game's own resize protocol requires at least 320x240 input pixels.
    let mut sizes = mode.sizes().map(|(input, output)| if match_display {
        let fitted_input = crate::spatial_dimensions::fit_aspect(input, display);
        if crate::spatial_resize_policy::valid_input(fitted_input) {
            (fitted_input, crate::spatial_dimensions::fit_aspect(output, display))
        } else { (input, output) }
    } else { (input, output) });
    let explicit_requested = cfg.get("spatial_upscale_input_size").is_some() || cfg.get("spatial_upscale_output_size").is_some();
    let short_requested = cfg.get("spatial_upscale_native_short_edge").is_some() || cfg.get("spatial_upscale_output_short_edge").is_some();
    let mut short_edges = None;
    let mut explicit_error = None;
    if short_requested && mode != Mode::Off {
        let edge=|key:&str|cfg.get(key).and_then(|v|v.as_u64()).and_then(|v|u32::try_from(v).ok());
        let requested=edge("spatial_upscale_native_short_edge").zip(edge("spatial_upscale_output_short_edge"));
        if let Some((edges,selected))=requested.and_then(|edges|
            (!explicit_requested).then(|| crate::spatial_dimensions::short_edge_sizes(edges.0,edges.1,display))
                .flatten().map(|selected|(edges,selected))) {
            short_edges=Some(edges); sizes=Some(selected); match_display=true;
        } else {
            explicit_error=Some("invalid adaptive SR sizes: require two bounded even short edges, valid display aspect and no explicit size pairs");
            eprintln!("[spatial-upscale] {}",explicit_error.unwrap());
            mode=Mode::Off; sizes=None; match_display=false;
        }
    } else if explicit_requested && mode != Mode::Off {
        let pair = |key: &str| -> Option<(u32,u32)> {
            let values = cfg.get(key)?.as_array()?;
            if values.len() != 2 { return None; }
            Some((u32::try_from(values[0].as_u64()?).ok()?, u32::try_from(values[1].as_u64()?).ok()?))
        };
        let requested = pair("spatial_upscale_input_size").zip(pair("spatial_upscale_output_size"));
        if let Some((input,output)) = requested.filter(|&(input,output)| crate::spatial_dimensions::valid_explicit_sizes(input,output)) {
            sizes = Some((input,output)); match_display = false;
        } else {
            explicit_error = Some("invalid explicit SR sizes: require both even input/output pairs, input >=320x240, output >=input, matching aspect and extents <=8192");
            eprintln!("[spatial-upscale] {}", explicit_error.unwrap());
            mode = Mode::Off; sizes = None; match_display = false;
        }
    }
    // Input resolution belongs to the guest, independently of whether XEngine
    // runs afterward. Retain the validated dimensions when SR is switched off.
    let settings = Settings { enabled, mode, sharpness, sizes, match_display, short_edges, generation: 1 };
    *SETTINGS.lock().unwrap() = settings;
    RECORDED.store(0, Ordering::Relaxed);
    RESIZES.store(0, Ordering::Relaxed);
    ACTIVE.store(false, Ordering::Relaxed);
    INPUT_PATH.store(0, Ordering::Relaxed);
    // The public vendor SDK cannot accept VVL-wrapped non-dispatchable handles.
    // This documented setting disables only wrapping, preserving validation.
    // Applied before prepare_surface/Gpu::new creates the instance; OFF restores
    // the process's original setting rather than erasing another caller's value.
    if settings.requested() {
        ORIGINAL_UNIQUE_HANDLES.get_or_init(|| std::env::var_os("VK_LAYER_UNIQUE_HANDLES"));
        std::env::set_var("VK_LAYER_UNIQUE_HANDLES", "0");
        eprintln!("[spatial-upscale] public VVL setting VK_LAYER_UNIQUE_HANDLES=0 before instance; core/shader/layout validation retained; legacy disables must not override this setting");
    } else if let Some(original) = ORIGINAL_UNIQUE_HANDLES.get() {
        match original { Some(value) => std::env::set_var("VK_LAYER_UNIQUE_HANDLES", value),
            None => std::env::remove_var("VK_LAYER_UNIQUE_HANDLES") }
    }
    *STATUS.lock().unwrap() = Some(serde_json::json!({"mode":if settings.requested(){mode.name()}else{"off"},
        "enabled":enabled,"configuredMode":mode.name(),"requested":settings.requested(),
        "sharpness":sharpness,"requestedSizes":sizes,"matchDisplay":match_display,
        "adaptiveShortEdges":short_edges,"adaptiveShortEdgeRequest":short_requested,
        "explicitSizeRequest":explicit_requested,"explicitSizeError":explicit_error,
        "startupDisplaySize":display,"requestedDisplaySize":display,"resizeGeneration":1,
        "resizePending":false,"nativeCommandsEncoded":false,
        "gpuOutputVerified":false,"frameGeneration":false}));
    if let Some((render, output)) = sizes {
        eprintln!("[spatial-upscale] enabled={} mode={} gameRender={}x{} configuredOutput={}x{} sharpness={} (guest dimensions retained with SR disabled)",
            settings.requested(), mode.name(), render.0, render.1, output.0, output.1, sharpness);
    }
}
/// XComponent callbacks only queue the latest physical window. They must never
/// overwrite launch input/settings while AOT or the first GPU graph is loading.
pub(crate) fn resize_display(display: (u32,u32)) {
    if display.0 < 2 || display.1 < 2 { return; }
    *PENDING_DISPLAY.lock().unwrap() = Some((display,Instant::now()));
}
/// Executed only at a submitted-frame boundary, after Spatial::new has copied
/// the same immutable launch dimensions as the guest CLI. Wait for a playable
/// world and stable layout before requesting the guest's own resize protocol.
pub(crate) fn process_pending_resize(before_publish: impl FnOnce() -> std::result::Result<(), String>) {
    if !crate::ffi::world_player_observed() { return; }
    let request = {
        let mut pending = PENDING_DISPLAY.lock().unwrap();
        if pending.as_ref().is_some_and(|(_,since)| since.elapsed() >= Duration::from_millis(150)) {
            pending.take()
        } else { None }
    };
    let Some((display,_)) = request else { return; };
    // This ability is landscape-only. Ignore an intermediate portrait layout
    // while the system rotates during a fold/display transition.
    if display.0 <= display.1 { return; }
    // A newer native callback may have stored physical size just before queuing
    // its request. Leave that newer request to its own stabilization interval.
    if display != crate::ffi::physical_size() { return; }
    let snapshot = *SETTINGS.lock().unwrap();
    let (input, output) = if !snapshot.requested() {
        (crate::config::render_size(display.0,display.1), None)
    } else {
        if !snapshot.match_display { return; }
        let Some((input,output))=snapshot.display_sizes(display) else { return; };
        (input,Some(output))
    };
    if !crate::spatial_resize_policy::valid_input(input) { return; }
    if let Some(output) = output {
        if snapshot.sizes == Some((input,output)) { return; }
    } else if crate::input::size() == (input.0 as i32,input.1 as i32) {
        return;
    }
    // No settings/input write may expose the resize until already submitted
    // GPU work AND its deferred CPU readback/query writes have completed.
    // The closure runs without borrowing a Spatial or holding global locks.
    if let Err(error) = before_publish() {
        let mut pending = PENDING_DISPLAY.lock().unwrap();
        if pending.is_none() { *pending = Some((display,Instant::now())); }
        drop(pending);
        if let Some(value) = STATUS.lock().unwrap().as_mut() {
            if value.get("resizeDrainError").and_then(|v|v.as_str()) != Some(error.as_str()) {
                eprintln!("[spatial-upscale] resize drain failed; retaining request without publishing: {error}");
            }
            value["resizeDrainError"] = error.into();
        }
        return;
    }
    // A fold can continue while the GPU wait blocks. Do not publish this old
    // request after a newer callback has supplied a different physical size.
    if display != crate::ffi::physical_size() { return; }
    let mut settings = SETTINGS.lock().unwrap();
    if settings.enabled != snapshot.enabled || settings.mode != snapshot.mode || settings.match_display != snapshot.match_display
        || settings.short_edges != snapshot.short_edges
        || settings.generation != snapshot.generation || settings.sizes != snapshot.sizes { return; }
    crate::heapcheck::request_snapshot("before-resize-publish");
    if let Some(output) = output {
        settings.sizes = Some((input, output));
        settings.generation = settings.generation.wrapping_add(1);
    }
    let generation = settings.generation;
    drop(settings);
    crate::input::set_size(input.0 as i32,input.1 as i32);
    crate::ffi::reset_touch_origin();
    if let Some(value) = STATUS.lock().unwrap().as_mut() {
        value["resizeDrainError"] = serde_json::Value::Null;
        if let Some(output) = output {
            value["requestedSizes"] = serde_json::json!((input,output));
            value["requestedDisplaySize"] = serde_json::json!(display);
            value["resizeGeneration"] = generation.into();
            value["resizePending"] = true.into();
        }
    }
    if let Some(output) = output {
        eprintln!("[spatial-upscale] resize requested generation={generation} display={}x{} input={}x{} output={}x{}; waiting for actual guest backbuffer",
            display.0,display.1,input.0,input.1,output.0,output.1);
    }
}
pub(crate) fn render_size_override() -> Option<(u32,u32)> { SETTINGS.lock().unwrap().sizes.map(|s| s.0) }
/// The output uses the combined interpolation graph, including exact repeats
/// of its previously submitted private post-SR pixels. This is content scope,
/// not a claim that a new SR dispatch or neural inference ran for this output.
pub(crate) fn note_frame_generation(active: bool) {
    if let Some(value) = STATUS.lock().unwrap().as_mut() {
        value["frameGeneration"] = active.into();
        value["frameGenerationScope"] = "combined graph output content, including private cached repeats; new inference/SR work and display cadence require separate counters and pixel verification".into();
    }
}
pub(crate) fn status() -> serde_json::Value {
    let settings = SETTINGS.lock().unwrap();
    let mut value = STATUS.lock().unwrap().clone().unwrap_or_else(|| serde_json::json!({"mode":"off","requested":false}));
    value["requestedSizes"] = serde_json::json!(settings.sizes);
    value["resizeGeneration"] = settings.generation.into();
    if let Some(active_generation) = value.get("activeResizeGeneration").and_then(|v|v.as_u64()) {
        value["resizePending"] = (active_generation != settings.generation).into();
    }
    value["nativeRecordedFrames"] = RECORDED.load(Ordering::Relaxed).into();
    value["completedResizes"] = RESIZES.load(Ordering::Relaxed).into(); value
}
fn fallback(reason: &str, game: (u32,u32)) {
    ACTIVE.store(false, Ordering::Relaxed);
    let mut status = STATUS.lock().unwrap();
    let value = status.get_or_insert_with(|| serde_json::json!({}));
    if value.get("fallbackReason").and_then(|s| s.as_str()) != Some(reason) {
        eprintln!("[spatial-upscale] ordinary-blit fallback gameRender={}x{} output={}x{} reason={reason}", game.0,game.1,game.0,game.1);
    }
    value["fallbackReason"] = reason.into(); value["activeGameRenderSize"] = serde_json::json!(game);
    value["activeOutputSize"] = serde_json::json!(game);
    value["nativeCommandsEncoded"] = false.into();
}
pub(crate) fn validation_compatibility() -> Result<()> {
    anyhow::ensure!(std::env::var("VK_LAYER_UNIQUE_HANDLES").as_deref() == Ok("0"),
        "SR requires public VK_LAYER_UNIQUE_HANDLES=0 before instance creation");
    anyhow::ensure!(std::env::var_os("VK_LAYER_DISABLES").is_none_or(|v| v.is_empty()),
        "legacy VK_LAYER_DISABLES can override the per-setting handle-wrapping control");
    Ok(())
}

unsafe extern "C" {
    fn muguet_xeg_spatial_create(physical: u64, device: u64, iw: u32, ih: u32, ow: u32, oh: u32, sharpness: f32) -> *mut c_void;
    fn muguet_xeg_spatial_record(context: *mut c_void, command: u64, input_view: u64, output_view: u64, output_image: u64) -> bool;
    fn muguet_xeg_spatial_destroy(context: *mut c_void);
    fn muguet_xeg_spatial_last_error() -> *const c_char;
}
fn bridge_error() -> String {
    let p = unsafe { muguet_xeg_spatial_last_error() };
    if p.is_null() { "XEngine bridge returned no error".into() }
    else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}
struct ContextHandle(NonNull<c_void>);
impl Drop for ContextHandle { fn drop(&mut self) { unsafe { muguet_xeg_spatial_destroy(self.0.as_ptr()); } } }

// Drop context while borrowed texture/view handles and the logical device live.
// The owner only replaces this slot after the existing UBO submission fence.
struct Slot {
    context: ContextHandle,
    output: Texture,
    output_view: TextureView,
    output_bg: BindGroup,
    blit_pipeline: RenderPipeline,
    source: Texture,
    source_view: TextureView,
    conversion: Option<Conversion>,
}
struct Conversion {
    texture: Texture,
    view: TextureView,
    source_bg: BindGroup,
}
pub(crate) fn blit_pipeline(device: &Device, format: TextureFormat) -> RenderPipeline {
    let module = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("xeg-shared-textureLoad-blit"), source: ShaderSource::Wgsl(BLIT_SHADER.into()) });
    device.create_render_pipeline(&RenderPipelineDescriptor { label: Some("xeg-rgba-conversion"), layout: None,
        vertex: VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        primitive: PrimitiveState::default(), depth_stencil: None, multisample: MultisampleState::default(),
        fragment: Some(FragmentState { module: &module, entry_point: Some("fs"), compilation_options: Default::default(),
            targets: &[Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })] }),
        multiview_mask: None, cache: None })
}
fn source_binding(device: &Device, view: &TextureView, pipeline: &RenderPipeline) -> BindGroup {
    device.create_bind_group(&BindGroupDescriptor { label: Some("xeg-bgra-source"),
        layout: &pipeline.get_bind_group_layout(0), entries: &[BindGroupEntry { binding: 0,
            resource: BindingResource::TextureView(view) }] })
}
fn conversion_texture(device: &Device, input: (u32,u32), source_view: &TextureView,
    pipeline: &RenderPipeline) -> Conversion {
    let texture = device.create_texture(&TextureDescriptor { label: Some("xeg-bgra-to-rgba-input"),
        size: Extent3d { width: input.0, height: input.1, depth_or_array_layers: 1 }, mip_level_count: 1,
        sample_count: 1, dimension: TextureDimension::D2, format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC,
        view_formats: &[] });
    let view = texture.create_view(&TextureViewDescriptor::default());
    Conversion { texture, view, source_bg: source_binding(device,source_view,pipeline) }
}
pub(crate) struct Spatial {
    settings: Settings,
    slots: Vec<Option<Slot>>,
    // Retains VkDevice through vendor-context destruction. Not a second device.
    device: Device,
    conversion_pipeline: Option<RenderPipeline>,
    blocked_resize_generation: Option<u64>,
}
impl Spatial {
    pub(crate) fn new(device: &Device, slot_count: usize) -> Option<Self> {
        let settings = *SETTINGS.lock().unwrap();
        settings.requested().then(|| Self { settings,
            slots: (0..slot_count).map(|_| None).collect(), device: device.clone(), conversion_pipeline: None,
            blocked_resize_generation: None })
    }
    pub(crate) fn output_size(&self) -> (u32,u32) { self.settings.sizes.unwrap().1 }
    pub(crate) fn resize_ready_for_source(&self, observed: (u32,u32)) -> bool {
        let desired = *SETTINGS.lock().unwrap();
        match (self.settings.target(), desired.target()) {
            (Some(active), Some(desired)) => crate::spatial_resize_policy::ready(
                active, desired, observed, self.blocked_resize_generation),
            _ => false,
        }
    }
    /// The caller has submitted every encoder and successfully waited ALL
    /// outstanding submissions, including the other two retained frame slots.
    pub(crate) fn resize_completed(&mut self, observed: (u32,u32)) -> bool {
        let desired = *SETTINGS.lock().unwrap();
        let ready = match (self.settings.target(), desired.target()) {
            (Some(active), Some(target)) => crate::spatial_resize_policy::ready(
                active, target, observed, self.blocked_resize_generation),
            _ => false,
        };
        if !ready { return false; }
        // ContextHandle is the first Slot field: retire vendor contexts before
        // their borrowed texture/view handles, while the same device lives.
        for slot in &mut self.slots { *slot = None; }
        self.settings = desired;
        self.blocked_resize_generation = None;
        ACTIVE.store(false,Ordering::Relaxed);
        RESIZES.fetch_add(1,Ordering::Relaxed);
        eprintln!("[spatial-upscale] resize GPU graph retired generation={} input={:?} output={:?}; same game/device",
            desired.generation,observed,self.output_size());
        true
    }
    pub(crate) fn resize_wait_failed(&mut self, reason: &str) {
        let generation = SETTINGS.lock().unwrap().generation;
        self.blocked_resize_generation = Some(generation);
        ACTIVE.store(false,Ordering::Relaxed);
        if let Some(value) = STATUS.lock().unwrap().as_mut() {
            value["resizeWaitError"] = reason.into();
            value["nativeCommandsEncoded"] = false.into();
        }
        eprintln!("[spatial-upscale] resize wait failed generation={generation}; retaining old GPU graph: {reason}");
    }
    fn prepare(&mut self, source: &Texture, slot: usize, blit: &RenderPipeline) -> Result<()> {
        validation_compatibility()?;
        let desired = *SETTINGS.lock().unwrap();
        anyhow::ensure!(desired.requested(), "SR disabled by configuration");
        anyhow::ensure!(self.blocked_resize_generation != Some(desired.generation),
            "SR resize wait failed; old resources retained until a new request or restart");
        // A folding animation can return to an identical target before the
        // guest resizes. Adopt that generation without retiring an unchanged graph.
        if self.settings.sizes == desired.sizes && self.settings.generation != desired.generation {
            self.settings = desired;
            self.blocked_resize_generation = None;
            ACTIVE.store(false,Ordering::Relaxed);
        }
        let (input, output) = self.settings.sizes.context("SR mode OFF")?;
        anyhow::ensure!(slot < self.slots.len(), "invalid completed frame slot");
        anyhow::ensure!((source.width(),source.height()) == input && source.depth_or_array_layers() == 1
            && source.dimension() == TextureDimension::D2 && source.sample_count() == 1
            && source.mip_level_count() == 1
            && matches!(source.format(),TextureFormat::Rgba8Unorm | TextureFormat::Bgra8Unorm)
            && source.usage().contains(TextureUsages::TEXTURE_BINDING),
            "requires pinned RGBA8_UNORM or explicitly converted BGRA8_UNORM 2D single-mip/layer/sample source: required={}x{}, actual={}x{}, layers={}, dimension={:?}, samples={}, mips={}, format={:?}, usage={:?}",
            input.0,input.1,source.width(),source.height(),source.depth_or_array_layers(),
            source.dimension(),source.sample_count(),source.mip_level_count(),source.format(),source.usage());
        let convert = source.format() == TextureFormat::Bgra8Unorm;
        if convert && self.conversion_pipeline.is_none() {
            self.conversion_pipeline = Some(blit_pipeline(&self.device,TextureFormat::Rgba8Unorm));
        }
        if let Some(current) = self.slots[slot].as_mut() {
            if current.blit_pipeline != *blit {
                current.output_bg = source_binding(&self.device,&current.output_view,blit);
                current.blit_pipeline = blit.clone();
            }
            // Public WGPU identity survives native-handle recycling after an
            // explicit Texture::destroy; raw VkImage equality does not.
            if current.source != *source {
                // The caller has already waited this slot's previous complete
                // submission; never replace a view while its raw CB is pending.
                current.source_view = source.create_view(&TextureViewDescriptor {
                    label: Some("xeg-source"), dimension: Some(TextureViewDimension::D2),
                    base_mip_level: 0, mip_level_count: Some(1), base_array_layer: 0,
                    array_layer_count: Some(1), ..Default::default() });
                current.source = source.clone();
                if convert {
                    let pipeline = self.conversion_pipeline.as_ref().unwrap();
                    if let Some(conversion) = current.conversion.as_mut() {
                        conversion.source_bg = source_binding(&self.device,&current.source_view,pipeline);
                    } else {
                        current.conversion = Some(conversion_texture(&self.device,input,&current.source_view,pipeline));
                    }
                } else { current.conversion = None; }
            }
            return Ok(());
        }
        let texture = self.device.create_texture(&TextureDescriptor { label: Some("xeg-spatial-output"),
            size: Extent3d { width: output.0, height: output.1, depth_or_array_layers: 1 }, mip_level_count: 1,
            sample_count: 1, dimension: TextureDimension::D2, format: TextureFormat::Rgba8Unorm,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC,
            view_formats: &[] });
        let output_view = texture.create_view(&TextureViewDescriptor::default());
        let source_view = source.create_view(&TextureViewDescriptor { label: Some("xeg-source"),
            dimension: Some(TextureViewDimension::D2), base_mip_level: 0, mip_level_count: Some(1),
            base_array_layer: 0, array_layer_count: Some(1), ..Default::default() });
        let conversion = convert.then(|| conversion_texture(&self.device,input,&source_view,
            self.conversion_pipeline.as_ref().unwrap()));
        let output_bg = self.device.create_bind_group(&BindGroupDescriptor { label: Some("xeg-output-blit"),
            layout: &blit.get_bind_group_layout(0), entries: &[BindGroupEntry { binding: 0,
                resource: BindingResource::TextureView(&output_view) }] });
        let pointer = {
            let raw = unsafe { self.device.as_hal::<hal::api::Vulkan>() }.context("device is not Vulkan")?;
            unsafe { muguet_xeg_spatial_create(raw.raw_physical_device().as_raw(),raw.raw_device().handle().as_raw(),
                input.0,input.1,output.0,output.1,self.settings.sharpness) }
        }; // Release all HAL destruction guards before dropping/creating WGPU objects.
        let context = ContextHandle(NonNull::new(pointer).ok_or_else(|| anyhow::anyhow!(bridge_error()))?);
        self.slots[slot] = Some(Slot { context, output: texture, output_view, output_bg, blit_pipeline: blit.clone(),
            source: source.clone(), source_view, conversion });
        eprintln!("[spatial-upscale] slot={slot} context created mode={} gameRender={}x{} output={}x{} (existing completed UBO slot)",
            self.settings.mode.name(),input.0,input.1,output.0,output.1);
        Ok(())
    }
    /// Append tracked initialization/transitions to WG1; return a distinct Raw
    /// CB for the SAME existing queue.submit, then the caller records WG3 blit.
    pub(crate) fn record(&mut self, source: &Texture, slot: usize, blit: &RenderPipeline,
        before: &mut CommandEncoder) -> Result<(CommandBuffer, BindGroup, (u32,u32))> {
        self.prepare(source, slot, blit)?;
        let s = self.slots[slot].as_ref().unwrap();
        if let Some(conversion) = s.conversion.as_ref() {
            // Actual format conversion: BGRA sampling returns logical RGBA,
            // which is written to a distinct RGBA8_UNORM color attachment.
            // No raw image/view reinterpretation and no CPU channel swapping.
            let mut pass = before.begin_render_pass(&RenderPassDescriptor { label: Some("xeg-BGRA8-to-RGBA8"),
                color_attachments: &[Some(RenderPassColorAttachment { view: &conversion.view,
                    depth_slice: None, resolve_target: None, ops: Operations { load: LoadOp::Clear(Color::BLACK), store: StoreOp::Store } })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
            pass.set_pipeline(self.conversion_pipeline.as_ref().unwrap());
            pass.set_bind_group(0,&conversion.source_bg,&[]);
            pass.draw(0..3,0..1);
        }
        let (native_input,native_view) = match s.conversion.as_ref() {
            Some(conversion) => (&conversion.texture,&conversion.view),
            None => (&s.source,&s.source_view),
        };
        {
            let _clear = before.begin_render_pass(&RenderPassDescriptor { label: Some("xeg-output-initialize"),
                color_attachments: &[Some(RenderPassColorAttachment { view: &s.output_view,
                    depth_slice: None, resolve_target: None, ops: Operations { load: LoadOp::Clear(Color::BLACK), store: StoreOp::Store } })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
        }
        before.transition_resources(std::iter::empty(), std::iter::once(TextureTransition {
            texture: native_input, selector: None, state: TextureUses::RESOURCE }));
        // Directly create a RAW-only encoder. self.encoder()/profile writes and
        // transition_resources MUST NOT be called on this encoder (API panic).
        let mut raw_encoder = self.device.create_command_encoder(&CommandEncoderDescriptor { label: Some("xeg-native-only") });
        let recorded = {
            let input = unsafe { native_view.as_hal::<hal::api::Vulkan>() }.context("input view is not Vulkan")?;
            let output = unsafe { s.output_view.as_hal::<hal::api::Vulkan>() }.context("output view is not Vulkan")?;
            let image = unsafe { s.output.as_hal::<hal::api::Vulkan>() }.context("output image is not Vulkan")?;
            unsafe { raw_encoder.as_hal_mut::<hal::api::Vulkan,_,_>(|encoder| {
                encoder.is_some_and(|encoder| muguet_xeg_spatial_record(s.context.0.as_ptr(),
                    encoder.raw_handle().as_raw(),input.raw_handle().as_raw(),output.raw_handle().as_raw(),image.raw_handle().as_raw()))
            }) }
        };
        anyhow::ensure!(recorded, "SR CPU-side encode rejected before commands: {}", bridge_error());
        let command = raw_encoder.finish();
        RECORDED.fetch_add(1, Ordering::Relaxed);
        let path = if s.conversion.is_some() { 2 } else { 1 };
        let previous_path = INPUT_PATH.swap(path,Ordering::Relaxed);
        if !ACTIVE.swap(true, Ordering::Relaxed) || previous_path != path {
            let desired_generation = SETTINGS.lock().unwrap().generation;
            let mut status = STATUS.lock().unwrap(); let v = status.as_mut().unwrap();
            v["nativeCommandsEncoded"] = true.into(); v["activeGameRenderSize"] = serde_json::json!((source.width(),source.height()));
            v["activeOutputSize"] = serde_json::json!(self.output_size()); v["fallbackReason"] = serde_json::Value::Null;
            v["sourceFormat"] = format!("{:?}",source.format()).into();
            v["sampledInputFormat"] = "Rgba8Unorm".into();
            v["convertedBGRAtoRGBA"] = (path == 2).into();
            v["activeResizeGeneration"] = self.settings.generation.into();
            v["resizePending"] = (self.settings.generation != desired_generation).into();
            v["resizeWaitError"] = serde_json::Value::Null;
            eprintln!("[spatial-upscale] encoded mode={} source={:?} gameRender={}x{} sampledInput=Rgba8Unorm converted={} output={}x{} (CPU encoding; GPU image verification pending)",
                self.settings.mode.name(),source.format(),source.width(),source.height(),path==2,self.output_size().0,self.output_size().1);
        }
        Ok((command,s.output_bg.clone(),self.output_size()))
    }
    pub(crate) fn report_fallback(&self, reason: &str, game: (u32,u32)) { fallback(reason,game); }
    /// Read-only diagnostic handles. The caller must uphold the same complete
    /// submission fence before slot reuse; cloning never makes early reuse safe.
    pub(crate) fn output_texture(&self, slot: usize) -> Option<Texture> {
        Some(self.slots.get(slot)?.as_ref()?.output.clone())
    }
    pub(crate) fn sampled_input_texture(&self, slot: usize) -> Option<Texture> {
        let s = self.slots.get(slot)?.as_ref()?;
        Some(s.conversion.as_ref().map(|c|c.texture.clone()).unwrap_or_else(||s.source.clone()))
    }
}
impl Drop for Spatial {
    fn drop(&mut self) {
        // Shutdown only, never per frame or ordinary fallback. Complete all
        // retained vendor contexts before views and the underlying device drop.
        let _ = self.device.poll(PollType::Wait { submission_index: None, timeout: None });
        self.slots.clear();
    }
}
