//! Default-off, six-frame evidence from the real guest's final source texture.
//! All GPU work uses tracked WGPU resources on the game's existing queue. A
//! private RGBA8 snapshot precedes readback; no guest memory or image is written.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use wgpu::*;

const LIMIT: usize = 6;
const GUARD: usize = 64;
const SENTINEL: u8 = 0x9d;
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
const MAX_PENDING_BYTES: usize = 128 * 1024 * 1024;
const MAP_TIMEOUT: Duration = Duration::from_secs(30);
const SHADER: &str = "@group(0) @binding(0) var src: texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
 var p = array<vec2f, 3>(vec2f(-1,-1),vec2f(3,-1),vec2f(-1,3)); return vec4f(p[i],0,1);
}
@fragment fn fs(@builtin(position) p: vec4f) -> @location(0) vec4f {
 return textureLoad(src,vec2i(p.xy),0);
}";

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new().create(true).truncate(true).write(true)
        .mode(0o600).custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW).open(path)
        .map_err(|e| format!("open {}: {e}",path.display()))?;
    file.write_all(bytes).map_err(|e| format!("write {}: {e}",path.display()))?;
    file.sync_all().map_err(|e| format!("sync {}: {e}",path.display()))
}
struct Pending {
    record: usize, width: u32, height: u32, pitch: usize, bytes: usize,
    buffer: Buffer, _snapshot: Texture,
    ready: Receiver<Result<(), String>>, began: Instant,
}
pub(crate) struct Capture {
    directory: PathBuf, began: Instant, pipeline: Option<RenderPipeline>,
    boundary: String, sequence_meaning: String, frame_generation_enabled: bool,
    records: Vec<Value>, pending: Vec<Pending>, errors: Vec<String>,
    attempts: usize, saved: usize, closed: bool,
}
impl Capture {
    pub(crate) fn from_env() -> Option<Self> {
        let requested = std::env::var_os("MG_FG_CAPTURE_DIR")?;
        Self::from_directory(PathBuf::from(requested),
            "real_guest_opcode_7_final_source_before_present_blit_original_texture_not_SR_output",
            "one_based_guest_opcode_7_present_count", false)
    }
    pub(crate) fn from_directory(requested: PathBuf, boundary: &str, sequence_meaning: &str,
        frame_generation_enabled: bool) -> Option<Self> {
        let initialize = || -> Result<Self,String> {
            let root = std::env::var_os("MUGUET_SUPPORT").ok_or("MUGUET_SUPPORT app files root is absent")?;
            let root = std::fs::canonicalize(root).map_err(|e| format!("app files root: {e}"))?;
            let directory = std::fs::canonicalize(&requested).map_err(|e| format!("capture directory must already exist: {e}"))?;
            if !directory.is_dir() || directory == root || !directory.starts_with(&root) {
                return Err("capture directory must already exist strictly inside app files".into());
            }
            let capture = Self { directory, began: Instant::now(), pipeline: None,
                boundary: boundary.into(), sequence_meaning: sequence_meaning.into(), frame_generation_enabled,
                records: Vec::new(),pending: Vec::new(),errors: Vec::new(),attempts: 0,saved: 0,closed: false };
            capture.persist()?;
            Ok(capture)
        };
        match initialize() {
            Ok(capture) => { eprintln!("[fg-capture] enabled for at most {LIMIT} GPU texture frames at {}: {}",capture.boundary,capture.directory.display());Some(capture) }
            Err(error) => { eprintln!("[fg-capture] rejected, ordinary game presentation retained: {error}");None }
        }
    }
    fn persist(&self) -> Result<(), String> {
        let value = json!({
            "status":if !self.errors.is_empty(){"error"}else if self.closed&&self.pending.is_empty(){"completed_capture_only"}else{"capturing"},
            "pid":std::process::id(),"directory":self.directory,"frameLimit":LIMIT,
            "sourceBoundary":self.boundary,
            "sourceSequenceMeaning":self.sequence_meaning,
            "attemptedFrames":self.attempts,"savedFrames":self.saved,"pendingFrames":self.pending.len(),
            "captureClosed":self.closed,"frames":self.records,"errors":self.errors,
            "rowAlignmentBytes":256,"readbackGuardBytesEachSide":GUARD,
            "guestMemoryWritten":false,"sourceTextureModified":false,"originalPresentationChanged":false,
            "frameGenerationEnabled":self.frame_generation_enabled,"displayFpsVerified":null,
            "scope":"Bounded GPU texture capture at the explicitly labeled caller boundary; host boundary times are not GPU timestamps or scanout times. Capture alone does not prove interpolation provenance, super-resolution quality or display FPS."
        });
        let temporary = self.directory.join(".capture-status.json.tmp");
        write_synced(&temporary,format!("{value}\n").as_bytes())?;
        std::fs::rename(&temporary,self.directory.join("capture-status.json"))
            .map_err(|e| format!("publish capture status: {e}"))
    }
    fn fail(&mut self, error: String) {
        eprintln!("[fg-capture] disabled, ordinary game presentation retained: {error}");
        if self.errors.len() < LIMIT + 2 { self.errors.push(error); }
        self.closed = true;
    }
    fn persist_or_close(&mut self) {
        if let Err(error) = self.persist() { self.fail(error); }
    }
    pub(crate) fn awaiting_gpu(&self) -> bool { !self.pending.is_empty() }
    pub(crate) fn finished(&self) -> bool { self.closed && self.pending.is_empty() }

    /// Return an isolated, fully tracked command buffer. The caller inserts it
    /// after all earlier guest/SR work and before the unchanged final blit in
    /// the SAME queue submission. Invalid capture commands never touch the
    /// game's own encoder. map_buffer_on_submit maps only after that submission.
    pub(crate) fn record(&mut self, device: &Device, source: &Texture, source_sequence: u64) -> Option<CommandBuffer> {
        if self.closed { return None; }
        self.attempts += 1;
        self.closed = self.attempts >= LIMIT;
        let (width,height) = (source.width(),source.height());
        let format = source.format();
        let elapsed_ns = self.began.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        let utc_ns = SystemTime::now().duration_since(UNIX_EPOCH).ok()
            .map(|t|t.as_nanos().min(u64::MAX as u128) as u64);
        let index = self.records.len();
        self.records.push(json!({"captureSequence":self.attempts,"sourceSequence":source_sequence,
            "boundaryTimestampUtcNs":utc_ns,"boundaryMonotonicNs":elapsed_ns,
            "gpuTimestampNs":null,"width":width,"height":height,"actualFormat":format!("{format:?}"),
            "sourceSampleCount":source.sample_count(),"sourceMip":0,"sourceArrayLayer":0,
            "normalizedFormat":"Rgba8Unorm","status":"preparing"}));
        let prepared = self.encode(device,source,index);
        match prepared {
            Ok((command,pending)) => {
                self.records[index]["status"] = "encoded_waiting_gpu".into();
                self.records[index]["readbackBytesPerRow"] = pending.pitch.into();
                self.records[index]["snapshotOwnedByCapture"] = true.into();
                self.records[index]["colorNormalization"] = "logical_RGBA_channels_preserving_encoded_UNORM_bytes_linear_view_for_sRGB_storage".into();
                self.pending.push(pending);
                self.persist_or_close();
                Some(command)
            }
            Err(error) => {
                self.records[index]["status"] = "error".into();
                self.records[index]["error"] = error.clone().into();
                self.fail(error);self.persist_or_close();None
            }
        }
    }
    fn encode(&mut self, device: &Device, source: &Texture, record: usize) -> Result<(CommandBuffer,Pending),String> {
        let (width,height) = (source.width(),source.height());
        let linear = source.format().remove_srgb_suffix();
        if !matches!(linear,TextureFormat::Rgba8Unorm|TextureFormat::Bgra8Unorm) ||
            source.dimension()!=TextureDimension::D2 || source.sample_count()!=1 ||
            !source.usage().contains(TextureUsages::TEXTURE_BINDING) || width==0 || height==0 {
            return Err(format!("unsupported final source {:?} {}x{} samples={} dimension={:?} usage={:?}",
                source.format(),width,height,source.sample_count(),source.dimension(),source.usage()));
        }
        let pitch = (width as usize).checked_mul(4).and_then(|n|n.checked_add(255)).map(|n|n&!255)
            .ok_or("capture row size overflow")?;
        let bytes = pitch.checked_mul(height as usize).and_then(|n|n.checked_add(2*GUARD)).ok_or("capture size overflow")?;
        let retained: usize = self.pending.iter().map(|p|p.bytes*2).sum();
        if bytes>MAX_FRAME_BYTES || retained.saturating_add(bytes.saturating_mul(2))>MAX_PENDING_BYTES {
            return Err(format!("bounded capture memory limit: frameBytes={bytes}, pendingApproxBytes={retained}"));
        }
        let validation = device.push_error_scope(ErrorFilter::Validation);
        let allocation = device.push_error_scope(ErrorFilter::OutOfMemory);
        let encoded = (|| -> Result<(CommandBuffer,Pending),String> {
            if self.pipeline.is_none() {
                let shader = device.create_shader_module(ShaderModuleDescriptor {label:Some("fg-capture-copy"),source:ShaderSource::Wgsl(SHADER.into())});
                self.pipeline = Some(device.create_render_pipeline(&RenderPipelineDescriptor {
                    label:Some("fg-capture-rgba8-normalization"),layout:None,
                    vertex:VertexState {module:&shader,entry_point:Some("vs"),compilation_options:Default::default(),buffers:&[]},
                    primitive:PrimitiveState::default(),depth_stencil:None,multisample:MultisampleState::default(),
                    fragment:Some(FragmentState {module:&shader,entry_point:Some("fs"),compilation_options:Default::default(),
                        targets:&[Some(ColorTargetState {format:TextureFormat::Rgba8Unorm,blend:None,write_mask:ColorWrites::ALL})]}),
                    multiview_mask:None,cache:None,
                }));
            }
            let pipeline = self.pipeline.as_ref().ok_or("capture pipeline unavailable")?;
            let snapshot = device.create_texture(&TextureDescriptor {label:Some("fg-capture-owned-rgba8"),
                size:Extent3d {width,height,depth_or_array_layers:1},mip_level_count:1,sample_count:1,
                dimension:TextureDimension::D2,format:TextureFormat::Rgba8Unorm,
                usage:TextureUsages::RENDER_ATTACHMENT|TextureUsages::COPY_SRC,view_formats:&[]});
            let snapshot_view = snapshot.create_view(&TextureViewDescriptor::default());
            // The game's create_texture declares linear/sRGB compatible views
            // for RGBA8/BGRA8. Choosing the linear view preserves stored bytes.
            let source_view = source.create_view(&TextureViewDescriptor {label:Some("fg-capture-source-linear-view"),
                format:Some(linear),dimension:Some(TextureViewDimension::D2),base_mip_level:0,mip_level_count:Some(1),
                base_array_layer:0,array_layer_count:Some(1),..Default::default()});
            let binding = device.create_bind_group(&BindGroupDescriptor {label:Some("fg-capture-source"),
                layout:&pipeline.get_bind_group_layout(0),entries:&[BindGroupEntry {binding:0,resource:BindingResource::TextureView(&source_view)}]});
            let buffer = device.create_buffer(&BufferDescriptor {label:Some("fg-capture-guarded-readback"),size:bytes as u64,
                usage:BufferUsages::COPY_DST|BufferUsages::MAP_READ,mapped_at_creation:true});
            {
                let mut mapped = buffer.slice(..).get_mapped_range_mut().map_err(|e|format!("initial capture buffer map: {e:?}"))?;
                mapped.slice(..).fill(SENTINEL);
            }
            buffer.unmap();
            let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {label:Some("fg-capture-isolated" )});
            {
                let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {label:Some("fg-capture-snapshot"),
                    color_attachments:&[Some(RenderPassColorAttachment {view:&snapshot_view,depth_slice:None,resolve_target:None,
                        ops:Operations {load:LoadOp::Clear(Color::BLACK),store:StoreOp::Store}})],
                    depth_stencil_attachment:None,timestamp_writes:None,occlusion_query_set:None,multiview_mask:None});
                pass.set_pipeline(pipeline);pass.set_bind_group(0,&binding,&[]);pass.draw(0..3,0..1);
            }
            encoder.copy_texture_to_buffer(TexelCopyTextureInfo {texture:&snapshot,mip_level:0,origin:Origin3d::ZERO,aspect:TextureAspect::All},
                TexelCopyBufferInfo {buffer:&buffer,layout:TexelCopyBufferLayout {offset:GUARD as u64,bytes_per_row:Some(pitch as u32),rows_per_image:Some(height)}},
                Extent3d {width,height,depth_or_array_layers:1});
            let (tx,ready) = mpsc::channel();
            encoder.map_buffer_on_submit(&buffer,MapMode::Read,..,move|result| {let _=tx.send(result.map_err(|e|format!("capture map callback: {e}")));});
            let command = encoder.finish();
            Ok((command,Pending {record,width,height,pitch,bytes,buffer,_snapshot:snapshot,ready,began:Instant::now()}))
        })();
        let allocation_error = pollster::block_on(allocation.pop());
        let validation_error = pollster::block_on(validation.pop());
        if let Some(error) = allocation_error { return Err(format!("capture allocation rejected: {error}")); }
        if let Some(error) = validation_error { return Err(format!("capture validation rejected: {error}")); }
        encoded
    }
    fn read_and_save(&self, pending: &Pending) -> Result<Value,String> {
        let mapped = pending.buffer.slice(..).get_mapped_range().map_err(|e|format!("capture mapped range: {e:?}"))?;
        if mapped.len()<pending.bytes { return Err("capture mapped buffer shorter than requested".into()); }
        let end = GUARD+pending.pitch*pending.height as usize;
        if !mapped[..GUARD].iter().chain(&mapped[end..pending.bytes]).all(|&b|b==SENTINEL) {
            return Err("capture readback prefix/suffix guard changed".into());
        }
        let row_bytes = pending.width as usize*4;
        let mut rgb = Vec::with_capacity(pending.width as usize*pending.height as usize*3);
        for row in 0..pending.height as usize {
            let at = GUARD+row*pending.pitch;
            if !mapped[at+row_bytes..at+pending.pitch].iter().all(|&b|b==SENTINEL) {
                return Err(format!("capture row padding guard changed at row {row}"));
            }
            for rgba in mapped[at..at+row_bytes].chunks_exact(4) { rgb.extend_from_slice(&rgba[..3]); }
        }
        drop(mapped);
        let digest = format!("{:x}",Sha256::digest(&rgb));
        let filename = format!("frame-{:02}.ppm",pending.record+1);
        let mut ppm = format!("P6\n{} {}\n255\n",pending.width,pending.height).into_bytes();ppm.extend_from_slice(&rgb);
        write_synced(&self.directory.join(&filename),&ppm)?;
        Ok(json!({"filename":filename,"ppmBytes":ppm.len(),"rgbBytes":rgb.len(),"rgbSha256":digest,
            "readbackGuardIntact":true,"rowPaddingGuardIntact":true,"alphaDroppedForPpm":true}))
    }
    pub(crate) fn poll(&mut self, device: &Device) {
        if self.pending.is_empty() { return; }
        // As in the guest's normal map path: poll, consume only completed maps,
        // copy data while mapped, drop the view, then unmap. Never wait idle.
        if let Err(error) = device.poll(PollType::Poll) {
            self.fail(format!("capture device poll failed: {error:?}"));
            for pending in self.pending.drain(..) {
                self.records[pending.record]["status"] = "error".into();
                self.records[pending.record]["error"] = "device poll failed before capture completed".into();
            }
            self.persist_or_close();return;
        }
        let mut index = 0;
        while index<self.pending.len() {
            let ready = match self.pending[index].ready.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Disconnected) => Some(Err("capture map channel disconnected".into())),
                Err(TryRecvError::Empty) if self.pending[index].began.elapsed()>=MAP_TIMEOUT => Some(Err("capture map exceeded bounded 30 second timeout".into())),
                Err(TryRecvError::Empty) => None,
            };
            let Some(ready) = ready else {index+=1;continue};
            let pending = self.pending.swap_remove(index);
            let mapped = ready.is_ok();
            let result = ready.and_then(|()|self.read_and_save(&pending));
            if mapped { pending.buffer.unmap(); }
            match result {
                Ok(value) => {self.saved+=1;self.records[pending.record]["status"]="saved".into();self.records[pending.record]["image"]=value;}
                Err(error) => {self.records[pending.record]["status"]="error".into();self.records[pending.record]["error"]=error.clone().into();self.fail(error);}
            }
            self.persist_or_close();
        }
    }
}
