// muguet - mouse.rs
// Constantin Clerc (c22dev) - 0.1

use crate::input;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, Imp, Sel};
use objc2::{msg_send, sel};
use objc2_game_controller::{GCControllerDirectionPad, GCMouse, GCMouseInput};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Mutex;
use winit::window::Window;

static HOOKED: Mutex<Vec<usize>> = Mutex::new(Vec::new());
static BUTTONS: AtomicU8 = AtomicU8::new(0);
static LOCK: AtomicBool = AtomicBool::new(false);
static OVERRIDDEN: AtomicBool = AtomicBool::new(false);
static WHEEL: Mutex<f64> = Mutex::new(0.0);

extern "C-unwind" fn prefers_pointer_locked(_this: *mut AnyObject, _cmd: Sel) -> Bool {
    Bool::new(LOCK.load(Ordering::Relaxed))
}

fn lock_pointer(window: &Window, on: bool) {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    LOCK.store(on, Ordering::Relaxed);
    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::UiKit(h) = handle.as_raw() else { return };
    let Some(vc) = h.ui_view_controller else { return };
    let vc = unsafe { &*(vc.as_ptr() as *const AnyObject) };
    if !OVERRIDDEN.swap(true, Ordering::Relaxed) {
        let imp: Imp = unsafe { std::mem::transmute(prefers_pointer_locked as extern "C-unwind" fn(*mut AnyObject, Sel) -> Bool) };
        unsafe { objc2::ffi::class_addMethod(vc.class() as *const _ as *mut _, sel!(prefersPointerLocked), imp, c"B@:".as_ptr()) };
    }
    unsafe { let _: () = msg_send![vc, setNeedsUpdateOfPrefersPointerLocked]; }
}

fn set_buttons(now: u8) {
    let before = BUTTONS.swap(now, Ordering::Relaxed);
    for (bit, dom) in [(1u8, 0usize), (2, 2), (4, 1)] {
        if (now ^ before) & bit != 0 { input::button(dom, now & bit != 0); }
    }
}

pub fn poll(window: &Window) -> bool {
    let mice: Vec<_> = unsafe { GCMouse::mice().iter().filter_map(|m| m.mouseInput()).collect() };
    let mut hooked = HOOKED.lock().unwrap();
    if mice.is_empty() {
        if !hooked.is_empty() {
            hooked.clear();
            eprintln!("[mouse] mouse disconnected");
            set_buttons(0);
            lock_pointer(window, false);
        }
        return false;
    }
    let ids: Vec<usize> = mice.iter().map(|m| Retained::as_ptr(m) as usize).collect();
    for (mouse, id) in mice.iter().zip(&ids) {
        if hooked.contains(id) { continue; }
        let moved = RcBlock::new(|_: NonNull<GCMouseInput>, dx: f32, dy: f32| input::mouse_delta_f(dx as f64, -(dy as f64)));
        let scrolled = RcBlock::new(|_: NonNull<GCControllerDirectionPad>, _x: f32, y: f32| {
            let mut accum = WHEEL.lock().unwrap();
            *accum += y as f64;
            let n = accum.trunc();
            *accum -= n;
            input::wheel(n as i32);
        });
        unsafe {
            mouse.setMouseMovedHandler(RcBlock::as_ptr(&moved));
            mouse.scroll().setValueChangedHandler(RcBlock::as_ptr(&scrolled));
        }
        if hooked.is_empty() { lock_pointer(window, true); }
        eprintln!("[mouse] mouse connected");
    }
    *hooked = ids;
    if !crate::pad::FOCUSED.load(Ordering::Relaxed) {
        BUTTONS.store(0, Ordering::Relaxed);
        return false;
    }
    input::set_locked(true);
    let mut buttons = 0u8;
    for mouse in &mice {
        unsafe {
            if mouse.leftButton().isPressed() { buttons |= 1; }
            if mouse.rightButton().is_some_and(|b| b.isPressed()) { buttons |= 2; }
            if mouse.middleButton().is_some_and(|b| b.isPressed()) { buttons |= 4; }
        }
    }
    set_buttons(buttons);
    true
}
