//! Temporary-core integration harness. Never compiled into the shipped runtime.
//! Real core API recording, validation, tracking, initialization and destruction;
//! NOOP HAL supplies no driver cost, GPU work, synchronization validation or FPS.
use alloc::{borrow::Cow, format, string::String, sync::Arc, vec, vec::Vec};
use core::num::NonZeroU64;
use std::time::Instant;

use crate::{binding_model as bm, global::Global, id, pipeline, resource};
use super::{render_submit_prototype, CommandEncoderStatus,
    PassChannel, RenderPassColorAttachment, RenderPassDepthStencilAttachment,
    RenderPassDescriptor};

struct Scene {
    global: Global,
    device: id::DeviceId,
    queue: id::QueueId,
    pipeline: id::RenderPipelineId,
    alternate_pipeline: id::RenderPipelineId,
    vertex: id::BufferId,
    index: id::BufferId,
    uniform: id::BufferId,
    indirect: id::BufferId,
    color: [id::TextureId; 2],
    views: [id::TextureViewId; 2],
    depth: id::TextureId,
    depth_view: id::TextureViewId,
    groups: [id::BindGroupId; 2],
    queries: id::QuerySetId,
    immediates: bool,
    mappable: bool,
}

fn buffer(global: &Global, device: id::DeviceId, size: u64, usage: wgt::BufferUsages) -> id::BufferId {
    let (id, error) = global.device_create_buffer(device, &resource::BufferDescriptor {
        label: None, size, usage, mapped_at_creation: false,
    }, None);
    assert!(error.is_none(), "buffer: {error:?}");
    id
}

fn texture(global: &Global, device: id::DeviceId, format: wgt::TextureFormat,
           usage: wgt::TextureUsages) -> (id::TextureId, id::TextureViewId) {
    let (texture, error) = global.device_create_texture(device, &resource::TextureDescriptor {
        label: None, size: wgt::Extent3d { width: 32, height: 32, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgt::TextureDimension::D2,
        format, usage, view_formats: vec![],
    }, None);
    assert!(error.is_none(), "texture: {error:?}");
    let (view, error) = global.texture_create_view(texture, &Default::default(), None);
    assert!(error.is_none(), "view: {error:?}");
    (texture, view)
}

impl Scene {
    fn new() -> Self {
        let mut instance = wgt::InstanceDescriptor::new_without_display_handle();
        instance.backends = wgt::Backends::NOOP;
        instance.backend_options.noop.enable = true;
        // NOOP otherwise advertises an unlimited alignment of 1, while the
        // unchanged real indirect validator requires a supported Vulkan-style
        // storage alignment. Advertise 256 equally for both branches.
        let mut limits = wgt::Limits::unlimited();
        limits.min_storage_buffer_offset_alignment = 256;
        limits.min_uniform_buffer_offset_alignment = 256;
        instance.backend_options.noop.limits = Some(limits);
        let global = Global::new("render-submit-offline-prototype", instance, None);
        let adapter = global.enumerate_adapters(wgt::Backends::NOOP, false)[0];
        let features = global.adapter_features(adapter);
        let immediates = features.contains(wgt::Features::IMMEDIATES);
        let mappable = features.contains(wgt::Features::MAPPABLE_PRIMARY_BUFFERS);
        let mut descriptor = wgt::DeviceDescriptor::default();
        if immediates {
            descriptor.required_features |= wgt::Features::IMMEDIATES;
            descriptor.required_limits.max_immediate_size = 16;
        }
        if mappable { descriptor.required_features |= wgt::Features::MAPPABLE_PRIMARY_BUFFERS; }
        let (device, queue) = global.adapter_request_device(adapter, &descriptor, None, None).unwrap();
        let vertex = buffer(&global, device, 8192, wgt::BufferUsages::VERTEX | wgt::BufferUsages::COPY_DST |
            if mappable { wgt::BufferUsages::MAP_READ } else { wgt::BufferUsages::empty() });
        let index = buffer(&global, device, 1024, wgt::BufferUsages::INDEX | wgt::BufferUsages::COPY_DST);
        let uniform = buffer(&global, device, 1024, wgt::BufferUsages::UNIFORM | wgt::BufferUsages::COPY_DST);
        let indirect = buffer(&global, device, 32, wgt::BufferUsages::INDIRECT | wgt::BufferUsages::COPY_DST);
        let usage = wgt::TextureUsages::RENDER_ATTACHMENT | wgt::TextureUsages::TEXTURE_BINDING |
            wgt::TextureUsages::COPY_SRC | wgt::TextureUsages::COPY_DST;
        let (a, av) = texture(&global, device, wgt::TextureFormat::Rgba8Unorm, usage);
        let (b, bv) = texture(&global, device, wgt::TextureFormat::Rgba8Unorm, usage);
        let (depth, depth_view) = texture(&global, device, wgt::TextureFormat::Depth32Float,
            wgt::TextureUsages::RENDER_ATTACHMENT);
        let (layout, error) = global.device_create_bind_group_layout(device, &bm::BindGroupLayoutDescriptor {
            label: None,
            entries: Cow::Owned(vec![
                wgt::BindGroupLayoutEntry {
                    binding: 0, visibility: wgt::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgt::BindingType::Buffer { ty: wgt::BufferBindingType::Uniform,
                        has_dynamic_offset: true, min_binding_size: NonZeroU64::new(16) },
                    count: None,
                },
                wgt::BindGroupLayoutEntry {
                    binding: 1, visibility: wgt::ShaderStages::FRAGMENT,
                    ty: wgt::BindingType::Texture { sample_type: wgt::TextureSampleType::Float { filterable: false },
                        view_dimension: wgt::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
            ]),
        }, None);
        assert!(error.is_none(), "bind layout: {error:?}");
        let mut groups = Vec::new();
        // Group i samples texture i. An ordinary pass renders texture 0 and samples 1.
        for view in [av, bv] {
            let (group, error) = global.device_create_bind_group(device, &bm::BindGroupDescriptor {
                label: None, layout,
                entries: Cow::Owned(vec![
                    bm::BindGroupEntry { binding: 0, resource: bm::BindingResource::Buffer(bm::BufferBinding {
                        buffer: uniform, offset: 0, size: Some(16),
                    }) },
                    bm::BindGroupEntry { binding: 1, resource: bm::BindingResource::TextureView(view) },
                ]),
            }, None);
            assert!(error.is_none(), "bind group: {error:?}");
            groups.push(group);
        }
        let (pipeline_layout, error) = global.device_create_pipeline_layout(device, &bm::PipelineLayoutDescriptor {
            label: None, bind_group_layouts: Cow::Owned(vec![Some(layout)]),
            immediate_size: if immediates { 16 } else { 0 },
        }, None);
        assert!(error.is_none(), "pipeline layout: {error:?}");
        let shader_text = r#"
            @group(0) @binding(0) var<uniform> tint: vec4<f32>;
            @group(0) @binding(1) var source_image: texture_2d<f32>;
            @vertex fn vertex_main(@location(0) position: vec2<f32>) -> @builtin(position) vec4<f32> {
                return vec4<f32>(position + tint.xy * 0.001, 0.0, 1.0);
            }
            @fragment fn fragment_main() -> @location(0) vec4<f32> {
                return textureLoad(source_image, vec2<i32>(0, 0), 0) + tint;
            }
        "#;
        let (shader, error) = global.device_create_shader_module(device, &pipeline::ShaderModuleDescriptor {
            label: None, runtime_checks: Default::default(),
        }, pipeline::ShaderModuleSource::Wgsl(Cow::Borrowed(shader_text)), None);
        assert!(error.is_none(), "shader: {error:?}");
        let stage = |entry: &'static str| pipeline::ProgrammableStageDescriptor {
            module: shader, entry_point: Some(Cow::Borrowed(entry)), constants: Default::default(),
            zero_initialize_workgroup_memory: true,
        };
        let mut pipelines = Vec::new();
        for constant_blend in [false, true] {
            let blend = constant_blend.then_some(wgt::BlendState {
                color: wgt::BlendComponent { src_factor: wgt::BlendFactor::Constant,
                    dst_factor: wgt::BlendFactor::OneMinusConstant, operation: wgt::BlendOperation::Add },
                alpha: wgt::BlendComponent::REPLACE,
            });
            let (pipeline, error) = global.device_create_render_pipeline(device, &pipeline::RenderPipelineDescriptor {
                label: None, layout: Some(pipeline_layout),
                vertex: pipeline::VertexState {
                    stage: stage("vertex_main"),
                    buffers: Cow::Owned(vec![Some(pipeline::VertexBufferLayout {
                        array_stride: 8, step_mode: wgt::VertexStepMode::Vertex,
                        attributes: Cow::Owned(vec![wgt::VertexAttribute {
                            format: wgt::VertexFormat::Float32x2, offset: 0, shader_location: 0,
                        }]),
                    })]),
                },
                primitive: Default::default(),
                depth_stencil: Some(wgt::DepthStencilState {
                    format: wgt::TextureFormat::Depth32Float, depth_write_enabled: Some(true),
                    depth_compare: Some(wgt::CompareFunction::LessEqual), stencil: Default::default(), bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(pipeline::FragmentState {
                    stage: stage("fragment_main"), targets: Cow::Owned(vec![Some(wgt::ColorTargetState {
                        format: wgt::TextureFormat::Rgba8Unorm, blend, write_mask: wgt::ColorWrites::ALL,
                    })]),
                }),
                multiview_mask: None, cache: None,
            }, None);
            assert!(error.is_none(), "render pipeline: {error:?}");
            pipelines.push(pipeline);
        }
        let (queries, error) = global.device_create_query_set(device, &resource::QuerySetDescriptor {
            label: None, ty: wgt::QueryType::Occlusion, count: 4,
        }, None);
        assert!(error.is_none(), "queries: {error:?}");
        Self { global, device, queue, pipeline: pipelines[0], alternate_pipeline: pipelines[1],
            vertex, index, uniform, indirect, color: [a, b], views: [av, bv], depth, depth_view,
            groups: [groups[0], groups[1]], queries, immediates, mappable }
    }

    fn reference_counts(&self) -> Vec<usize> {
        let g = &self.global;
        vec![
            Arc::strong_count(&g.hub.buffers.get(self.vertex).get().unwrap()),
            Arc::strong_count(&g.hub.buffers.get(self.index).get().unwrap()),
            Arc::strong_count(&g.hub.buffers.get(self.uniform).get().unwrap()),
            Arc::strong_count(&g.hub.buffers.get(self.indirect).get().unwrap()),
            Arc::strong_count(&g.hub.textures.get(self.color[0])),
            Arc::strong_count(&g.hub.textures.get(self.color[1])),
            Arc::strong_count(&g.hub.textures.get(self.depth)),
            Arc::strong_count(&g.hub.bind_groups.get(self.groups[0]).get().unwrap()),
            Arc::strong_count(&g.hub.bind_groups.get(self.groups[1]).get().unwrap()),
            Arc::strong_count(&g.hub.render_pipelines.get(self.pipeline)),
            Arc::strong_count(&g.hub.render_pipelines.get(self.alternate_pipeline)),
        ]
    }
}

#[derive(Clone, Copy, Debug)]
enum Fixture {
    Direct, Indexed, StateChanges, ZeroCount, Hazards, DiscardFixup,
    IndirectFallback, QueryFallback, MixedFallback,
    InvalidVertexRange, InvalidIndexRange, InvalidScissor, InvalidDynamicOffset,
    MissingPipeline, AttachmentConflict,
}

impl Fixture {
    fn invalid(self) -> bool {
        matches!(self, Self::InvalidVertexRange | Self::InvalidIndexRange | Self::InvalidScissor |
            Self::InvalidDynamicOffset | Self::MissingPipeline | Self::AttachmentConflict)
    }
    fn pass_count(self) -> usize {
        match self { Self::Hazards | Self::DiscardFixup => 2, Self::MixedFallback => 4, _ => 1 }
    }
    fn selected_passes(self) -> usize {
        match self {
            Self::IndirectFallback | Self::QueryFallback => 0,
            Self::MixedFallback | Self::Hazards => 2,
            Self::DiscardFixup => 1,
            _ => 1,
        }
    }
}

fn record(scene: &Scene, fixture: Fixture, draw_count: usize) -> id::CommandEncoderId {
    let g = &scene.global;
    let (encoder, error) = g.device_create_command_encoder(scene.device,
        &wgt::CommandEncoderDescriptor { label: Some(Cow::Borrowed("render-submit-fixture")) }, None);
    assert!(error.is_none(), "encoder: {error:?}");
    for pass_index in 0..fixture.pass_count() {
        let ping_pong = matches!(fixture, Fixture::Hazards | Fixture::DiscardFixup);
        let target = usize::from(ping_pong && pass_index == 1);
        let sampled = if matches!(fixture, Fixture::AttachmentConflict) { target } else { 1 - target };
        let with_query = matches!(fixture, Fixture::QueryFallback) ||
            matches!(fixture, Fixture::MixedFallback) && pass_index == 1;
        let with_indirect = matches!(fixture, Fixture::IndirectFallback) ||
            matches!(fixture, Fixture::MixedFallback) && pass_index == 3;
        let (mut pass, error) = g.command_encoder_begin_render_pass(encoder, &RenderPassDescriptor {
            label: Some(Cow::Borrowed("actual-core-pass")),
            color_attachments: Cow::Owned(vec![Some(RenderPassColorAttachment {
                view: scene.views[target], depth_slice: None, resolve_target: None,
                load_op: wgt::LoadOp::Clear(wgt::Color { r: 0.1, g: 0.2, b: 0.3, a: 1.0 }),
                store_op: if matches!(fixture, Fixture::DiscardFixup) && pass_index == 0 {
                    wgt::StoreOp::Discard
                } else { wgt::StoreOp::Store },
            })]),
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: scene.depth_view,
                depth: PassChannel { load_op: Some(wgt::LoadOp::Clear(Some(1.0))),
                    store_op: Some(wgt::StoreOp::Store), read_only: false },
                stencil: PassChannel { load_op: None, store_op: None, read_only: true },
            }),
            timestamp_writes: None, occlusion_query_set: with_query.then_some(scene.queries),
            multiview_mask: None,
        });
        assert!(error.is_none(), "pass: {error:?}");
        if !matches!(fixture, Fixture::MissingPipeline) {
            g.render_pass_set_pipeline(&mut pass, scene.pipeline).unwrap();
        }
        g.render_pass_set_bind_group(&mut pass, 0, Some(scene.groups[sampled]),
            &[if matches!(fixture, Fixture::InvalidDynamicOffset) { 1 } else { 0 }]).unwrap();
        g.render_pass_set_vertex_buffer(&mut pass, 0, Some(scene.vertex), 8, NonZeroU64::new(4096)).unwrap();
        g.render_pass_set_index_buffer(&mut pass, scene.index, wgt::IndexFormat::Uint16,
            4, NonZeroU64::new(128)).unwrap();
        g.render_pass_set_viewport(&mut pass, 1.0, 2.0, 24.0, 24.0, 0.1, 0.9).unwrap();
        g.render_pass_set_scissor_rect(&mut pass, 1, 2,
            if matches!(fixture, Fixture::InvalidScissor) { 64 } else { 24 }, 24).unwrap();
        g.render_pass_set_blend_constant(&mut pass, wgt::Color { r: 0.25, g: 0.5, b: 0.75, a: 1.0 }).unwrap();
        g.render_pass_set_stencil_reference(&mut pass, 7).unwrap();
        if scene.immediates && !matches!(fixture, Fixture::MissingPipeline) {
            g.render_pass_set_immediates(&mut pass, 0, &[1, 2, 3, 4]).unwrap();
        }
        if with_query { g.render_pass_begin_occlusion_query(&mut pass, 1).unwrap(); }
        for draw_index in 0..draw_count {
            if matches!(fixture, Fixture::StateChanges) {
                g.render_pass_set_pipeline(&mut pass,
                    if draw_index % 2 == 0 { scene.alternate_pipeline } else { scene.pipeline }).unwrap();
                g.render_pass_set_bind_group(&mut pass, 0, Some(scene.groups[sampled]),
                    &[if draw_index % 2 == 0 { 256 } else { 0 }]).unwrap();
                g.render_pass_set_vertex_buffer(&mut pass, 0, Some(scene.vertex),
                    if draw_index % 2 == 0 { 16 } else { 8 }, NonZeroU64::new(4096)).unwrap();
                if scene.immediates { g.render_pass_set_immediates(&mut pass, 0, &[5, 6, 7, 8]).unwrap(); }
            }
            if matches!(fixture, Fixture::Indexed | Fixture::InvalidIndexRange) {
                g.render_pass_draw_indexed(&mut pass, 6, 2,
                    if matches!(fixture, Fixture::InvalidIndexRange) { 63 } else { 3 }, -2, 1).unwrap();
            } else {
                g.render_pass_draw(&mut pass,
                    if matches!(fixture, Fixture::ZeroCount) { 0 } else { 3 },
                    if matches!(fixture, Fixture::ZeroCount) { 0 } else { 2 },
                    if matches!(fixture, Fixture::InvalidVertexRange) { 1024 } else { 2 }, 1).unwrap();
            }
        }
        if with_indirect { g.render_pass_draw_indirect(&mut pass, scene.indirect, 0).unwrap(); }
        if with_query { g.render_pass_end_occlusion_query(&mut pass).unwrap(); }
        g.render_pass_end(&mut pass).unwrap();
        drop(pass);
    }
    encoder
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    executed_events: Vec<String>,
    buffer_tracker: String,
    texture_tracker: String,
    buffer_init: Vec<String>,
    texture_init: String,
    bind_groups: Vec<usize>,
    pipelines: Vec<usize>,
    views: Vec<usize>,
    query_count: usize,
    temp_count: usize,
    as_count: usize,
}

fn snapshot(scene: &Scene, command: id::CommandBufferId) -> (Snapshot, usize) {
    let command = scene.global.hub.command_buffers.get(command);
    let locked = command.data.lock();
    let CommandEncoderStatus::Finished(data) = &*locked else { panic!("expected finished CB") };
    let executed_events = data.encoder.list.iter().flat_map(|raw| {
        raw.as_any().downcast_ref::<hal::noop::CommandBuffer>().unwrap().audit_events().iter().cloned()
    }).collect();
    (Snapshot {
        executed_events,
        buffer_tracker: data.trackers.buffers.host_fingerprint(),
        texture_tracker: data.trackers.textures.host_fingerprint(),
        buffer_init: data.buffer_memory_init_actions.iter().map(|action| format!("{}|{:?}|{:?}",
            Arc::as_ptr(&action.buffer) as usize, action.range, action.kind)).collect(),
        texture_init: data.texture_memory_actions.host_fingerprint(),
        bind_groups: (&data.trackers.bind_groups).into_iter().map(|r| Arc::as_ptr(r) as usize).collect(),
        pipelines: (&data.trackers.render_pipelines).into_iter().map(|r| Arc::as_ptr(r) as usize).collect(),
        views: (&data.trackers.views).into_iter().map(|r| Arc::as_ptr(r) as usize).collect(),
        query_count: data.trackers.query_sets.used_resources().count(),
        temp_count: data.temp_resources.len(), as_count: data.as_actions.len(),
    }, data.encoder.list.len())
}

fn finish(scene: &Scene, encoder: id::CommandEncoderId) -> (id::CommandBufferId, Option<String>) {
    let (command, error) = scene.global.command_encoder_finish(encoder,
        &wgt::CommandBufferDescriptor { label: Some(Cow::Borrowed("render-submit-result")) }, None);
    scene.global.command_encoder_drop(encoder);
    (command, error.map(|(_, error)| format!("{error:?}")))
}

fn execute_cycle(scene: &Scene, fixture: Fixture, draws: usize, mode: bool) {
    render_submit_prototype::set_mode(mode);
    let encoder = record(scene, fixture, draws);
    let (command, error) = finish(scene, encoder);
    assert!(error.is_none(), "timed fixture {fixture:?}: {error:?}");
    scene.global.command_buffer_drop(command);
}

fn compare(scene: &Scene, fixture: Fixture, draws: usize) -> String {
    let before = scene.reference_counts();
    let mut results = Vec::new();
    for mode in [false, true] {
        render_submit_prototype::set_mode(mode);
        hal::noop::audit::set_enabled(true);
        let _ = hal::noop::audit::take();
        let encoder = record(scene, fixture, draws);
        let (command, error) = finish(scene, encoder);
        let captured = error.is_none().then(|| snapshot(scene, command));
        let calls = hal::noop::audit::take();
        scene.global.command_buffer_drop(command);
        hal::noop::audit::set_enabled(false);
        assert_eq!(scene.reference_counts(), before, "retained resource after {fixture:?}, mode={mode}");
        results.push((error, captured, calls.len(), render_submit_prototype::validations(),
            render_submit_prototype::counts()));
    }
    assert_eq!(results[0].0, results[1].0, "validation error differs: {fixture:?}");
    assert_eq!(results[0].0.is_some(), fixture.invalid(), "fixture validity: {fixture:?}");
    assert_eq!(results[0].3, results[1].3, "original validation repeated or skipped: {fixture:?}");
    assert_eq!(results[0].4, (0, 0), "legacy must bypass prototype selection: {fixture:?}");
    assert_eq!(results[1].4, (fixture.pass_count() as u64, fixture.selected_passes() as u64),
        "prototype selection coverage: {fixture:?}");
    let (legacy_buffers, candidate_buffers, event_count) = match (&results[0].1, &results[1].1) {
        (Some((a, ac)), Some((b, bc))) => {
            assert_eq!(a, b, "submitted HAL trace/tracker/init/owners differ: {fixture:?}");
            assert_eq!(*ac, fixture.pass_count() * 2, "legacy command-buffer count: {fixture:?}");
            assert_eq!(*bc, *ac - fixture.selected_passes(), "candidate command-buffer reduction: {fixture:?}");
            (*ac, *bc, a.executed_events.len())
        },
        (None, None) => (0, 0, 0),
        _ => unreachable!(),
    };
    format!("{{\"fixture\":\"{fixture:?}\",\"draws_per_pass\":{draws},\"passes\":{},\"valid\":{},\"exact_match\":true,\"candidate_observed_passes\":{},\"candidate_selected_passes\":{},\"legacy_command_buffers\":{legacy_buffers},\"candidate_command_buffers\":{candidate_buffers},\"submitted_semantic_events\":{event_count},\"legacy_recording_events\":{},\"candidate_recording_events\":{},\"legacy_validation_commands\":{},\"candidate_validation_commands\":{}}}",
        fixture.pass_count(), !fixture.invalid(), results[1].4.0, results[1].4.1,
        results[0].2, results[1].2, results[0].3, results[1].3)
}

fn lifecycle(mode: bool) {
    render_submit_prototype::set_mode(mode);
    hal::noop::audit::set_enabled(false);
    // Dropping all client resource handles after recording must not invalidate
    // the retained pass, and the CB must release its ownership when dropped.
    let scene = Scene::new();
    let vertex = scene.global.hub.buffers.get(scene.vertex).get().unwrap();
    let weak_vertex = Arc::downgrade(&vertex);
    drop(vertex);
    let pipeline = scene.global.hub.render_pipelines.get(scene.pipeline);
    let weak_pipeline = Arc::downgrade(&pipeline);
    drop(pipeline);
    let encoder = record(&scene, Fixture::Direct, 2);
    scene.global.buffer_drop(scene.vertex);
    scene.global.render_pipeline_drop(scene.pipeline);
    assert!(weak_vertex.upgrade().is_some());
    assert!(weak_pipeline.upgrade().is_some());
    let (command, error) = finish(&scene, encoder);
    assert!(error.is_none(), "client drop: {error:?}");
    assert!(weak_vertex.upgrade().is_some());
    assert!(weak_pipeline.upgrade().is_some());
    scene.global.command_buffer_drop(command);
    assert!(weak_vertex.upgrade().is_none(), "vertex remains after CB drop");
    assert!(weak_pipeline.upgrade().is_none(), "pipeline remains after CB drop");

    // Destroy after finish retains submission-time destruction rejection.
    let scene = Scene::new();
    let encoder = record(&scene, Fixture::Direct, 2);
    let (command, error) = finish(&scene, encoder);
    assert!(error.is_none());
    scene.global.buffer_destroy(scene.vertex);
    let error = scene.global.queue_submit(scene.queue, &[command]).unwrap_err();
    assert!(format!("{error:?}").contains("Destroyed"), "destroyed vertex submit: {error:?}");
    scene.global.command_buffer_drop(command);

    // Core intentionally defers destruction errors found during finish until
    // queue submission. The prototype must preserve this API timing.
    let scene = Scene::new();
    let encoder = record(&scene, Fixture::Direct, 2);
    scene.global.buffer_destroy(scene.vertex);
    let (command, error) = finish(&scene, encoder);
    assert!(error.is_none(), "destruction error must be deferred until submit");
    let error = scene.global.queue_submit(scene.queue, &[command]).unwrap_err();
    assert!(format!("{error:?}").contains("Destroyed"), "destroyed-before-finish submit: {error:?}");
    scene.global.command_buffer_drop(command);

    // Mapping after finish must still fail queue submission. Pending mapping
    // is sufficient for the core's non-idle mapping-state check.
    let scene = Scene::new();
    assert!(scene.mappable, "NOOP must support primary buffer mapping for this fixture");
    let encoder = record(&scene, Fixture::Direct, 2);
    let (command, error) = finish(&scene, encoder);
    assert!(error.is_none());
    scene.global.buffer_map_async(scene.vertex, 0, Some(16), resource::BufferMapOperation {
        host: crate::device::HostMap::Read, callback: None,
    }).unwrap();
    let error = scene.global.queue_submit(scene.queue, &[command]).unwrap_err();
    assert!(format!("{error:?}").contains("BufferStillMapped"), "mapped submit: {error:?}");
    scene.global.command_buffer_drop(command);

    // An unused/replaced bind group must remain subject to the queue check.
    let scene = Scene::new();
    let unused = buffer(&scene.global, scene.device, 1024, wgt::BufferUsages::UNIFORM);
    // Build an equivalent layout through the public APIs, then replace the
    // group before any draw. The buffer must still be checked at submission.
    let (unused_layout, layout_error) = scene.global.device_create_bind_group_layout(scene.device,
        &bm::BindGroupLayoutDescriptor { label: None, entries: Cow::Owned(vec![
            wgt::BindGroupLayoutEntry { binding: 0, visibility: wgt::ShaderStages::VERTEX_FRAGMENT,
                ty: wgt::BindingType::Buffer { ty: wgt::BufferBindingType::Uniform,
                    has_dynamic_offset: true, min_binding_size: NonZeroU64::new(16) }, count: None },
            wgt::BindGroupLayoutEntry { binding: 1, visibility: wgt::ShaderStages::FRAGMENT,
                ty: wgt::BindingType::Texture { sample_type: wgt::TextureSampleType::Float { filterable: false },
                    view_dimension: wgt::TextureViewDimension::D2, multisampled: false }, count: None },
        ]) }, None);
    assert!(layout_error.is_none());
    let (unused_group, group_error) = scene.global.device_create_bind_group(scene.device, &bm::BindGroupDescriptor {
        label: None, layout: unused_layout, entries: Cow::Owned(vec![
            bm::BindGroupEntry { binding: 0, resource: bm::BindingResource::Buffer(bm::BufferBinding {
                buffer: unused, offset: 0, size: Some(16),
            }) },
            bm::BindGroupEntry { binding: 1, resource: bm::BindingResource::TextureView(scene.views[1]) },
        ]),
    }, None);
    assert!(group_error.is_none());
    render_submit_prototype::set_mode(mode);
    let (encoder, error) = scene.global.device_create_command_encoder(scene.device, &Default::default(), None);
    assert!(error.is_none());
    let (mut pass, error) = scene.global.command_encoder_begin_render_pass(encoder, &RenderPassDescriptor {
        color_attachments: Cow::Owned(vec![Some(RenderPassColorAttachment {
            view: scene.views[0], depth_slice: None, resolve_target: None,
            load_op: wgt::LoadOp::Clear(wgt::Color::BLACK), store_op: wgt::StoreOp::Store,
        })]),
        depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
            view: scene.depth_view,
            depth: PassChannel { load_op: Some(wgt::LoadOp::Clear(Some(1.0))),
                store_op: Some(wgt::StoreOp::Store), read_only: false },
            stencil: PassChannel { load_op: None, store_op: None, read_only: true },
        }),
        ..Default::default()
    });
    assert!(error.is_none());
    scene.global.render_pass_set_bind_group(&mut pass, 0, Some(unused_group), &[0]).unwrap();
    scene.global.render_pass_set_bind_group(&mut pass, 0, Some(scene.groups[1]), &[0]).unwrap();
    scene.global.render_pass_set_pipeline(&mut pass, scene.pipeline).unwrap();
    scene.global.render_pass_set_vertex_buffer(&mut pass, 0, Some(scene.vertex), 0, NonZeroU64::new(4096)).unwrap();
    scene.global.render_pass_draw(&mut pass, 3, 1, 0, 0).unwrap();
    scene.global.render_pass_end(&mut pass).unwrap();
    drop(pass);
    let (command, error) = finish(&scene, encoder);
    assert!(error.is_none(), "replaced group finish: {error:?}");
    assert_eq!(render_submit_prototype::counts(), if mode { (1, 1) } else { (0, 0) },
        "replaced-group test must actually exercise candidate selection");
    scene.global.buffer_destroy(unused);
    let error = scene.global.queue_submit(scene.queue, &[command]).unwrap_err();
    assert!(format!("{error:?}").contains("Destroyed"), "replaced group submit: {error:?}");
    scene.global.command_buffer_drop(command);

    // A normal multi-pass submit exercises the real core memory initialization
    // and submission machinery after the exact encoding/tracker comparison.
    let scene = Scene::new();
    let encoder = record(&scene, Fixture::Hazards, 2);
    let (command, error) = finish(&scene, encoder);
    assert!(error.is_none());
    scene.global.queue_submit(scene.queue, &[command]).unwrap();
    scene.global.device_poll(scene.device, wgt::PollType::Poll).unwrap();
    scene.global.command_buffer_drop(command);
}

fn percentile(values: &mut [u128], percentile: usize) -> u128 {
    values.sort_unstable();
    values[((values.len() - 1) * percentile + 99) / 100]
}

fn benchmark(scene: &Scene, fixture: Fixture, draws: usize) -> String {
    const WARMUP: usize = 8;
    const SAMPLES: usize = 41;
    const ITERATIONS: usize = 8;
    hal::noop::audit::set_enabled(false);
    let before = scene.reference_counts();
    for _ in 0..WARMUP { execute_cycle(scene, fixture, draws, false); execute_cycle(scene, fixture, draws, true); }
    let mut legacy = Vec::new();
    let mut candidate = Vec::new();
    let mut paired = Vec::new();
    // Fixed-seed order randomization prevents a permanent A-first/B-second bias.
    let mut rng = 0x4b1d_ae39_6837_27c5u64;
    for _ in 0..SAMPLES {
        rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17;
        let modes = if rng & 1 == 0 { [false, true] } else { [true, false] };
        let mut pair = [0; 2];
        for mode in modes {
            let started = Instant::now();
            for _ in 0..ITERATIONS { execute_cycle(scene, fixture, draws, mode); }
            let elapsed = started.elapsed().as_nanos() / ITERATIONS as u128;
            pair[usize::from(mode)] = elapsed;
        }
        legacy.push(pair[0]); candidate.push(pair[1]);
        paired.push(pair[1] as i128 - pair[0] as i128);
    }
    assert_eq!(before, scene.reference_counts(), "benchmark retained resources");
    paired.sort_unstable();
    let legacy_median = percentile(&mut legacy, 50);
    let legacy_p95 = percentile(&mut legacy, 95);
    let candidate_median = percentile(&mut candidate, 50);
    let candidate_p95 = percentile(&mut candidate, 95);
    let paired_delta = paired[paired.len() / 2];
    let percent = (candidate_median as f64 / legacy_median as f64 - 1.0) * 100.0;
    format!("{{\"fixture\":\"{fixture:?}\",\"draws_per_pass\":{draws},\"passes\":{},\"samples\":{SAMPLES},\"iterations_per_sample\":{ITERATIONS},\"legacy_median_ns\":{legacy_median},\"candidate_median_ns\":{candidate_median},\"legacy_p95_ns\":{legacy_p95},\"candidate_p95_ns\":{candidate_p95},\"paired_median_delta_ns\":{paired_delta},\"candidate_median_change_percent\":{percent:.3},\"candidate_faster_median\":{}}}",
        fixture.pass_count(), candidate_median < legacy_median)
}

#[test]
fn host_render_harness_validate_and_measure_complete_processing() {
    let scene = Scene::new();
    let mut checks = Vec::new();
    for fixture in [Fixture::Direct, Fixture::Indexed, Fixture::StateChanges, Fixture::ZeroCount,
        Fixture::Hazards, Fixture::DiscardFixup, Fixture::IndirectFallback, Fixture::QueryFallback,
        Fixture::MixedFallback, Fixture::InvalidVertexRange, Fixture::InvalidIndexRange,
        Fixture::InvalidScissor, Fixture::InvalidDynamicOffset, Fixture::MissingPipeline,
        Fixture::AttachmentConflict] {
        checks.push(compare(&scene, fixture, if fixture.invalid() { 1 } else { 4 }));
    }
    lifecycle(false); lifecycle(true);
    let mut timings = Vec::new();
    for draws in [1, 32, 256] {
        for fixture in [Fixture::Direct, Fixture::Indexed, Fixture::StateChanges] {
            timings.push(benchmark(&scene, fixture, draws));
        }
    }
    timings.push(benchmark(&scene, Fixture::MixedFallback, 32));
    render_submit_prototype::set_mode(false);
    std::println!("RENDER_SUBMIT_RESULT={{\"schema_version\":1,\"backend\":\"core-noop\",\"validation\":\"actual unchanged core validator\",\"gpu_or_driver_claim\":false,\"representative_game_coverage_known\":false,\"cost_scope\":\"create encoder + API record + pass end + original validation + scope/init tracking + eligibility + tape allocation/copy/replay + finish + encoder/CB/tape drop\",\"fixture_construction_timed\":false,\"audit_enabled_during_timing\":false,\"immediates_exercised\":{},\"lifecycle_modes_passed\":2,\"differential\":[{}],\"benchmarks\":[{}]}}", scene.immediates, checks.join(","), timings.join(","));
}
