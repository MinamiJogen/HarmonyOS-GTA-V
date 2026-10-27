//! Test-only selection. Never linked into the shipped runtime.
use core::cell::Cell;
use super::render_command::ArcRenderCommand as C;
std::thread_local! {
    static MODE: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(u64,u64)> = const { Cell::new((0,0)) };
    static VALIDATIONS: Cell<u64> = const { Cell::new(0) };
}
pub(super) fn set_mode(enabled: bool) { MODE.set(enabled); COUNTS.set((0,0)); VALIDATIONS.set(0); }
pub(super) fn counts() -> (u64,u64) { COUNTS.get() }
pub(super) fn validated_command() { VALIDATIONS.set(VALIDATIONS.get()+1); }
pub(super) fn validations() -> u64 { VALIDATIONS.get() }
pub(super) fn selected(commands: &[C], simple_descriptor: bool) -> bool {
    if !MODE.get() { return false; }
    let mut direct = false;
    let accepted = simple_descriptor && commands.iter().all(|c| match c {
        C::Draw {..} | C::DrawIndexed {..} => { direct = true; true }
        C::SetBindGroup {..} | C::SetPipeline(_) | C::SetIndexBuffer {..}
        | C::SetVertexBuffer {..} | C::SetBlendConstant(_) | C::SetStencilReference(_)
        | C::SetViewport {..} | C::SetImmediate {..} | C::SetScissor(_)
        | C::PushDebugGroup {..} | C::PopDebugGroup | C::InsertDebugMarker {..} => true,
        _ => false,
    }) && direct;
    COUNTS.set({ let (n,e) = COUNTS.get(); (n+1,e+u64::from(accepted)) });
    accepted
}
