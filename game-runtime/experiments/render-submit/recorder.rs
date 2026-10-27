//! Offline NOOP experiment only. Typed operations own cloned HAL handles.
//! No borrowed handles, erased lifetimes, second validation, or Vulkan claim.
use alloc::{string::{String, ToString}, vec::Vec};
use core::{num::NonZeroU32, ops::Range};
use super::{Api, Buffer, DeviceResult, Resource};

#[derive(Debug)]
struct Target { view: Resource, usage: wgt::TextureUses }
impl Target {
    fn copy(a: &crate::Attachment<'_, Resource>) -> Self {
        Self { view: a.view.clone(), usage: a.usage }
    }
    fn borrowed(&self) -> crate::Attachment<'_, dyn crate::DynTextureView> {
        crate::Attachment { view: &self.view, usage: self.usage }
    }
}
#[derive(Debug)]
struct Color {
    target: Target, depth_slice: Option<u32>, resolve: Option<Target>,
    ops: crate::AttachmentOps, clear: wgt::Color,
}
#[derive(Debug)]
struct Depth {
    target: Target, depth: crate::AttachmentOps, stencil: crate::AttachmentOps,
    clear: (f32, u32),
}
#[derive(Debug)]
struct Pass {
    label: Option<String>, extent: wgt::Extent3d, samples: u32,
    colors: Vec<Option<Color>>, depth: Option<Depth>, multiview: Option<NonZeroU32>,
}
impl Pass {
    fn copy(d: &crate::RenderPassDescriptor<'_, Resource, Resource>) -> Self {
        assert!(d.timestamp_writes.is_none() && d.occlusion_query_set.is_none());
        Self {
            label: d.label.map(ToString::to_string), extent: d.extent,
            samples: d.sample_count, multiview: d.multiview_mask,
            colors: d.color_attachments.iter().map(|c| c.as_ref().map(|c| Color {
                target: Target::copy(&c.target), depth_slice: c.depth_slice,
                resolve: c.resolve_target.as_ref().map(Target::copy),
                ops: c.ops, clear: c.clear_value,
            })).collect(),
            depth: d.depth_stencil_attachment.as_ref().map(|d| Depth {
                target: Target::copy(&d.target), depth: d.depth_ops,
                stencil: d.stencil_ops, clear: d.clear_value,
            }),
        }
    }
    unsafe fn emit(self, out: &mut dyn crate::DynCommandEncoder) -> DeviceResult<()> {
        let colors: Vec<_> = self.colors.iter().map(|c| c.as_ref().map(|c| crate::ColorAttachment {
            target: c.target.borrowed(), depth_slice: c.depth_slice,
            resolve_target: c.resolve.as_ref().map(Target::borrowed),
            ops: c.ops, clear_value: c.clear,
        })).collect();
        let depth = self.depth.as_ref().map(|d| crate::DepthStencilAttachment {
            target: d.target.borrowed(), depth_ops: d.depth,
            stencil_ops: d.stencil, clear_value: d.clear,
        });
        unsafe { out.begin_render_pass(&crate::RenderPassDescriptor {
            label: self.label.as_deref(), extent: self.extent, sample_count: self.samples,
            color_attachments: &colors, depth_stencil_attachment: depth,
            multiview_mask: self.multiview, timestamp_writes: None, occlusion_query_set: None,
        }) }
    }
}

// Separate verified HAL operation type; ArcRenderCommand is never replayed.
#[derive(Debug)]
enum VerifiedOp {
    Begin(Pass), End, Pipeline(Resource),
    Bind(Resource, u32, Resource, Vec<u32>), Immediates(Resource, u32, Vec<u32>),
    Index(Buffer, u64, Option<wgt::BufferSize>, wgt::IndexFormat),
    Vertex(u32, Buffer, u64, Option<wgt::BufferSize>),
    Viewport(crate::Rect<f32>, Range<f32>), Scissor(crate::Rect<u32>),
    Stencil(u32), Blend([f32; 4]), Draw(u32, u32, u32, u32),
    Indexed(u32, u32, i32, u32, u32),
    Marker(String), Push(String), Pop,
}

#[derive(Debug, Default)]
pub struct Recorder { ops: Vec<VerifiedOp> }
impl Recorder {
    /// Caller has completed real core validation and emitted the real barriers.
    /// Resource handles and all call-scoped parameter slices are owned by ops.
    pub unsafe fn replay(self, out: &mut dyn crate::DynCommandEncoder) -> DeviceResult<()> {
        for op in self.ops {
            unsafe { match op {
                VerifiedOp::Begin(d) => d.emit(out)?,
                VerifiedOp::End => out.end_render_pass(),
                VerifiedOp::Pipeline(p) => out.set_render_pipeline(&p),
                VerifiedOp::Bind(l, i, g, offsets) => out.set_bind_group(&l, i, &g, &offsets),
                VerifiedOp::Immediates(l, offset, data) => out.set_immediates(&l, offset, &data),
                VerifiedOp::Index(b, offset, size, format) => out.set_index_buffer(
                    crate::BufferBinding::new_unchecked(&b as &dyn crate::DynBuffer, offset, size), format),
                VerifiedOp::Vertex(i, b, offset, size) => out.set_vertex_buffer(i,
                    crate::BufferBinding::new_unchecked(&b as &dyn crate::DynBuffer, offset, size)),
                VerifiedOp::Viewport(rect, depth) => out.set_viewport(&rect, depth),
                VerifiedOp::Scissor(rect) => out.set_scissor_rect(&rect),
                VerifiedOp::Stencil(v) => out.set_stencil_reference(v),
                VerifiedOp::Blend(v) => out.set_blend_constants(&v),
                VerifiedOp::Draw(a,b,c,d) => out.draw(a,b,c,d),
                VerifiedOp::Indexed(a,b,c,d,e) => out.draw_indexed(a,b,c,d,e),
                VerifiedOp::Marker(s) => out.insert_debug_marker(&s),
                VerifiedOp::Push(s) => out.begin_debug_marker(&s),
                VerifiedOp::Pop => out.end_debug_marker(),
            } }
        }
        Ok(())
    }
}
impl crate::DynResource for Recorder {
    fn as_any(&self) -> &dyn core::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any { self }
}

// prepare.py generates the complete CommandEncoder trait implementation from
// the frozen real NOOP signatures. Supported methods append VerifiedOp; every
// unsupported method panics, making a classifier omission fail the experiment.
// GENERATED_COMMAND_ENCODER_IMPLEMENTATION
