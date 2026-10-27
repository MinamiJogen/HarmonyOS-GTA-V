"""Apply the experiment ONLY to disposable source copies; fail on source drift."""
from pathlib import Path
import re

HERE = Path(__file__).resolve().parent


def replace_once(text, before, after):
    if text.count(before) != 1:
        raise ValueError('integration source drift: ' + before[:100])
    return text.replace(before, after, 1)


def method_bodies(text):
    """Frozen NOOP impl has no strings containing braces in these bodies."""
    for m in re.finditer(r'    unsafe fn (\w+)', text):
        opening = text.index('{', m.end())
        depth, end = 1, opening + 1
        while depth:
            depth += (text[end] == '{') - (text[end] == '}')
            end += 1
        yield m[1], m.start(), opening, end


def integrate(core: Path, hal: Path):
    policy = core / 'src/command/render_submit_prototype.rs'
    policy.write_text((HERE / 'policy.rs').read_text())
    path = core / 'src/command/render.rs'
    text = path.read_text()
    anchor = '    // We automatically keep extending command buffers over time, and because\n'
    text = replace_once(text, anchor, '''    // OFFLINE EXPERIMENT: classifies commands; never validates them twice.
    #[cfg(all(test, feature = "noop"))]
    let prototype_selected = super::render_submit_prototype::selected(
        &base.commands,
        device.backend() == wgt::Backend::Noop
            && timestamp_writes.is_none() && occlusion_query_set.is_none()
            && multiview_mask.is_none()
            && color_attachments.iter().flatten().all(|a| a.resolve_target.is_none()
                && a.store_op == StoreOp::Store)
            && depth_stencil_attachment.as_ref().is_none_or(|a| {
                [&a.depth].into_iter().all(|c| match c {
                    ResolvedPassChannel::ReadOnly => true,
                    ResolvedPassChannel::Operational(op) => op.store == StoreOp::Store,
                }) && match &a.stencil {
                    ResolvedPassChannel::ReadOnly => true,
                    ResolvedPassChannel::Operational(op) => op.store == StoreOp::Store,
                }
            }),
    );
    #[cfg(not(all(test, feature = "noop")))]
    let prototype_selected = false;
    #[cfg(all(test, feature = "noop"))]
    let mut verified = hal::noop::render_submit_recorder::Recorder::default();

''' + anchor)
    old = '''    parent_state
        .raw_encoder
        .close_if_open()
        .map_pass_err(pass_scope)?;
    let raw_encoder = parent_state
        .raw_encoder
        .open_pass(base.label.as_deref())
        .map_pass_err(pass_scope)?;'''
    new = '''    parent_state
        .raw_encoder
        .close_if_open()
        .map_pass_err(pass_scope)?;
    #[cfg(all(test, feature = "noop"))]
    let raw_encoder: &mut dyn hal::DynCommandEncoder = if prototype_selected {
        &mut verified
    } else {
        parent_state.raw_encoder.open_pass(base.label.as_deref()).map_pass_err(pass_scope)?
    };
    #[cfg(not(all(test, feature = "noop")))]
    let raw_encoder = parent_state.raw_encoder.open_pass(base.label.as_deref()).map_pass_err(pass_scope)?;'''
    text = replace_once(text, old, new)
    text = replace_once(text, '        for command in base.commands.drain(..) {\n',
                        '''        for command in base.commands.drain(..) {
            #[cfg(all(test, feature = "noop"))]
            super::render_submit_prototype::validated_command();
''')
    text = replace_once(text, '        if device.backend() == wgt::Backend::Metal\n',
                        '        if !prototype_selected && device.backend() == wgt::Backend::Metal\n')
    old = '''        parent_state.raw_encoder.close().map_pass_err(pass_scope)?;
        (trackers, pending_discard_init_fixups, pending_query_resets)'''
    new = '''        if !prototype_selected {
            parent_state.raw_encoder.close().map_pass_err(pass_scope)?;
        }
        (trackers, pending_discard_init_fixups, pending_query_resets)'''
    text = replace_once(text, old, new)
    anchor = '''    // OHOS Vulkan only; all pass validation, pass finish and render-CB close
'''
    text = replace_once(text, anchor, '''    #[cfg(all(test, feature = "noop"))]
    if prototype_selected {
        assert!(pending_query_resets.is_empty() && indirect_draw_validation_batcher.is_empty());
        let raw = encoder.open_pass(hal_label(base.label.as_deref(), device.instance_flags))
            .map_pass_err(pass_scope)?;
        fixup_discarded_surfaces(pending_discard_init_fixups.into_iter(), raw,
            &mut tracker.textures, device, parent_state.snatch_guard);
        CommandEncoder::insert_barriers_from_scope(raw, tracker, &scope, parent_state.snatch_guard);
        // No ArcRenderCommand/state helper is visited here. All draw arguments
        // and borrowed slices were copied by the one existing validation walk.
        unsafe { verified.replay(raw) }.map_err(|e| device.handle_hal_error(e)).map_pass_err(pass_scope)?;
        encoder.close().map_pass_err(pass_scope)?;
        return Ok(());
    }

''' + anchor)
    path.write_text(text)

    # Record resolved HAL operations using safe owned NOOP handles. No production
    # trait changes, raw pointers, transmute, or reference-lifetime erasure.
    command = hal / 'src/noop/command.rs'
    original = command.read_text()
    impl_start = original.index('impl crate::CommandEncoder for CommandBuffer {')
    impl_end = original.index('\nimpl Command {', impl_start)
    trait_impl = original[impl_start:impl_end]
    supported = {
        'begin_render_pass': 'self.ops.push(VerifiedOp::Begin(Pass::copy(desc))); Ok(())',
        'end_render_pass': 'self.ops.push(VerifiedOp::End);',
        'set_render_pipeline': 'self.ops.push(VerifiedOp::Pipeline(pipeline.clone()));',
        'set_bind_group': 'self.ops.push(VerifiedOp::Bind(layout.clone(), index, group.clone(), dynamic_offsets.to_vec()));',
        'set_immediates': 'self.ops.push(VerifiedOp::Immediates(layout.clone(), offset_bytes, data.to_vec()));',
        'set_index_buffer': 'self.ops.push(VerifiedOp::Index(binding.buffer.clone(), binding.offset, binding.size, format));',
        'set_vertex_buffer': 'self.ops.push(VerifiedOp::Vertex(index, binding.buffer.clone(), binding.offset, binding.size));',
        'set_viewport': 'self.ops.push(VerifiedOp::Viewport(rect.clone(), depth_range));',
        'set_scissor_rect': 'self.ops.push(VerifiedOp::Scissor(rect.clone()));',
        'set_stencil_reference': 'self.ops.push(VerifiedOp::Stencil(value));',
        'set_blend_constants': 'self.ops.push(VerifiedOp::Blend(*color));',
        'draw': 'self.ops.push(VerifiedOp::Draw(first_vertex, vertex_count, first_instance, instance_count));',
        'draw_indexed': 'self.ops.push(VerifiedOp::Indexed(first_index, index_count, base_vertex, first_instance, instance_count));',
        'insert_debug_marker': 'self.ops.push(VerifiedOp::Marker(label.to_string()));',
        'begin_debug_marker': 'self.ops.push(VerifiedOp::Push(group_label.to_string()));',
        'end_debug_marker': 'self.ops.push(VerifiedOp::Pop);',
    }
    generated = trait_impl.replace('for CommandBuffer {', 'for Recorder {', 1)
    for name, start, opening, end in reversed(list(method_bodies(generated))):
        body = supported.get(name, 'panic!("unclassified HAL operation: ' + name + '")')
        generated = generated[:opening+1] + '\n        ' + body + '\n    }' + generated[end:]
    recorder = (HERE / 'recorder.rs').read_text().replace('use super::{Api, Buffer, DeviceResult, Resource};',
                'use super::{Api, Buffer, CommandBuffer, DeviceResult, Resource};')
    recorder = recorder.replace('// GENERATED_COMMAND_ENCODER_IMPLEMENTATION', generated)
    (hal / 'src/noop/render_submit_recorder.rs').write_text('#![allow(unused_variables)]\n' + recorder)

    # All constructors retain unique, clone-stable identities. No allocation of
    # these fixture resources is included asymmetrically in A/B timings.
    path = hal / 'src/noop/mod.rs'
    text = path.read_text()
    text = replace_once(text, '#[derive(Debug)]\npub struct Resource;', '''#[derive(Clone, Debug)]
pub struct Resource(Arc<u64>);
impl Resource {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(Arc::new(NEXT.fetch_add(1, Ordering::Relaxed)))
    }
}
pub mod render_submit_recorder;
pub mod audit;
''')
    text = text.replace('Ok(Resource)', 'Ok(Resource::new())')
    path.write_text(text)
    (hal / 'src/noop/audit.rs').write_text('''//! Offline inspection only. Disabled in all timed trials.
extern crate std;
use alloc::{string::String, vec::Vec};
use core::cell::{Cell, RefCell};
self::std::thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}
pub fn set_enabled(v: bool) { ENABLED.set(v); EVENTS.with_borrow_mut(Vec::clear); }
pub fn take() -> Vec<String> { EVENTS.with_borrow_mut(core::mem::take) }
pub fn lifecycle(s: &str) { if ENABLED.get() { EVENTS.with_borrow_mut(|v| v.push(s.into())); } }
pub fn record<F: FnOnce() -> String>(cb: &mut Vec<String>, f: F) {
    if ENABLED.get() {
        let s = f();
        EVENTS.with_borrow_mut(|v| v.push(s.clone()));
        cb.push(s);
    }
}
''')
    # Avoid Debug printing buffer contents and make clone identity observable.
    path = hal / 'src/noop/buffer.rs'
    text = replace_once(path.read_text(), '#[derive(Clone, Debug)]', '#[derive(Clone)]')
    text += '''
impl core::fmt::Debug for Buffer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Buffer").field("storage", &Arc::as_ptr(&self.storage))
            .field("size", &self.size).finish()
    }
}
'''
    path.write_text(text)
    # Add a per-CB semantic trace, inspect after core has performed close/swap.
    text = replace_once(original, '    commands: Vec<Command>,\n',
                        '    commands: Vec<Command>,\n    audit_commands: Vec<alloc::string::String>,\n')
    text = text.replace('            commands: Vec::new(),',
                        '            commands: Vec::new(),\n            audit_commands: Vec::new(),')
    text = text.replace('            commands: mem::take(&mut self.commands),',
                        '            commands: mem::take(&mut self.commands),\n            audit_commands: mem::take(&mut self.audit_commands),')
    text = replace_once(text, 'impl CommandBuffer {', '''impl CommandBuffer {
    pub fn audit_events(&self) -> &[alloc::string::String] { &self.audit_commands }
''')
    audit_args = {
        'begin_render_pass': 'desc', 'end_render_pass': '()',
        'set_render_pipeline': 'pipeline', 'set_bind_group': '(layout,index,group,dynamic_offsets)',
        'set_immediates': '(layout,offset_bytes,data)',
        'set_index_buffer': '(&binding,format)', 'set_vertex_buffer': '(index,&binding)',
        'set_viewport': '(rect,&depth_range)', 'set_scissor_rect': 'rect',
        'set_stencil_reference': 'value', 'set_blend_constants': 'color',
        'draw': '(first_vertex,vertex_count,first_instance,instance_count)',
        'draw_indexed': '(first_index,index_count,base_vertex,first_instance,instance_count)',
        'insert_debug_marker': 'label', 'begin_debug_marker': 'group_label', 'end_debug_marker': '()',
        'draw_indirect': '(buffer,offset,draw_count)', 'draw_indexed_indirect': '(buffer,offset,draw_count)',
        'begin_query': '(set,index)', 'end_query': '(set,index)',
        'reset_queries': '(set,&range)', 'write_timestamp': '(set,index)',
        'clear_buffer': '(buffer,&range)',
        'begin_compute_pass': 'desc', 'end_compute_pass': '()',
        'set_compute_pipeline': 'pipeline', 'dispatch_workgroups': 'count',
        'dispatch_workgroups_indirect': '(buffer,offset)',
        'draw_indirect_count': '(buffer,offset,count_buffer,count_offset,max_count)',
        'draw_indexed_indirect_count': '(buffer,offset,count_buffer,count_offset,max_count)',
    }
    for name, start, opening, end in reversed(list(method_bodies(text))):
        addition = ''
        if name in ('begin_encoding', 'end_encoding', 'discard_encoding', 'reset_all'):
            addition = 'super::audit::lifecycle("' + name + '");'
            if name in ('discard_encoding', 'reset_all'):
                addition += ' self.audit_commands.clear();'
        elif name in ('transition_buffers', 'transition_textures'):
            addition = 'super::audit::record(&mut self.audit_commands, || alloc::format!("' + name + ':{:?}", barriers.collect::<Vec<_>>()));'
        elif name in audit_args:
            addition = 'super::audit::record(&mut self.audit_commands, || alloc::format!("' + name + ':{:?}", ' + audit_args[name] + '));'
        if addition:
            text = text[:opening+1] + '\n        ' + addition + text[opening+1:]
    command.write_text(text)
