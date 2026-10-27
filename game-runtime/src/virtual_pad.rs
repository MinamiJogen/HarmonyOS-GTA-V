//! Five Star Port's engine pad bridge, adapted from kevinerikjs/gta5-ios-port
//! 8e2e46281159d09005fea4748b5b9926e919fc0a. Original copyright (c) 2026
//! GTA V iOS port contributors, MIT (upstream/fivestar-input-8e2e4628/LICENSE).
//! This HarmonyOS adaptation is distributed under GPL-3.0-or-later.
//! The complete controller sample is packed exactly as upstream. UI threads only
//! publish samples; the ioPad::Update hook owns all engine memory writes.
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;
use wasmtime::SharedMemory;

pub const PAD_ZERO: u64 = 12_840_656;
const NEUTRAL: u64 = 0x0000_8080_8080_0000;
const SELECTION_WHEEL: u64 = (1 << 2) | (1 << 14);

struct State {
    packed: u64,
    active: bool,
    swipe_x: f32,
    swipe_y: f32,
    last_poll: Option<Instant>,
    radio: [f32; 2],
    radio_active: bool,
    radio_dirty: bool,
    pause_frames: u8,
    next_aircraft: bool,
    targeting_mode: Option<i32>,
}
impl State {
    const fn new() -> Self {
        Self { packed: NEUTRAL, active: false, swipe_x: 0.0, swipe_y: 0.0,
            last_poll: None, radio: [0.0; 2], radio_active: false,
            radio_dirty: false, pause_frames: 0, next_aircraft: false,
            targeting_mode: None }
    }
    fn release(&mut self) {
        // Explicit engine requests survive a later UI focus-loss notification.
        let targeting_mode = self.targeting_mode;
        let pause_frames = self.pause_frames;
        let radio_dirty = self.radio_dirty || self.radio_active;
        *self = Self::new();
        self.targeting_mode = targeting_mode;
        self.pause_frames = pause_frames;
        self.radio_dirty = radio_dirty;
    }
    fn sample(&mut self, now: Instant) -> (Option<u64>, Option<[f32; 2]>) {
        let radio = if self.radio_dirty || self.radio_active {
            self.radio_dirty = false;
            Some(if self.radio_active { self.radio } else { [0.0; 2] })
        } else { None };
        let elapsed = self.last_poll.map(|last| now.saturating_duration_since(last).as_secs_f32())
            .unwrap_or(1.0 / 60.0).clamp(1.0 / 240.0, 0.0667);
        self.last_poll = Some(now);
        let (dx, dy) = (self.swipe_x, self.swipe_y);
        self.swipe_x = 0.0; self.swipe_y = 0.0;
        // An explicit pause request belongs to the engine, independently of
        // foreground/touch focus. It survives subsequent focus-loss releases.
        if !self.active && self.pause_frames == 0 { return (None, radio); }
        let mut packed = if self.active { self.packed } else { NEUTRAL };
        // A held selection wheel owns the right stick, including when another
        // finger is still looking around. Impulses are consumed, never delayed.
        if packed & SELECTION_WHEEL == 0 && (dx != 0.0 || dy != 0.0) {
            let base_x = (((packed >> 32) & 255) as f32 - 127.5) / 127.5;
            let base_y = (127.5 - ((packed >> 40) & 255) as f32) / 127.5;
            let x = (base_x + swipe_axis(dx, elapsed)).clamp(-1.0, 1.0);
            let y = (base_y + swipe_axis(dy, elapsed)).clamp(-1.0, 1.0);
            packed = (packed & !(0xffff << 32)) |
                ((axis(x, false) as u64) << 32) | ((axis(y, true) as u64) << 40);
        }
        if self.pause_frames > 0 {
            packed |= 1 << 11;
            self.pause_frames -= 1;
        }
        (Some(packed), radio)
    }
}
static STATE: Mutex<State> = Mutex::new(State::new());
static UPDATES: AtomicU64 = AtomicU64::new(0);
static HOOKS: AtomicU64 = AtomicU64::new(0);
static APPLIED: AtomicU64 = AtomicU64::new(0);
static LAST_PACKED: AtomicU64 = AtomicU64::new(NEUTRAL);

fn axis(value: f32, up: bool) -> u8 {
    if !value.is_finite() { return 128; }
    let value = value.clamp(-1.0, 1.0) * if up { -1.0 } else { 1.0 };
    (127.5 + value * 127.5).round() as u8
}
fn trigger(value: f32) -> u8 {
    if value.is_finite() { (value.clamp(0.0, 1.0) * 255.0).round() as u8 } else { 0 }
}
fn pack(buttons: u32, lx: f32, ly: f32, rx: f32, ry: f32, lt: f32, rt: f32) -> u64 {
    (buttons as u64 & 0xffff) | ((axis(lx, false) as u64) << 16) |
        ((axis(ly, true) as u64) << 24) | ((axis(rx, false) as u64) << 32) |
        ((axis(ry, true) as u64) << 40) | ((trigger(lt) as u64) << 48) |
        ((trigger(rt) as u64) << 56)
}
fn swipe_axis(delta: f32, elapsed: f32) -> f32 {
    let velocity = delta.abs() / elapsed.clamp(1.0 / 240.0, 1.0 / 15.0);
    let normalized = (velocity / 1100.0).min(1.0);
    if normalized <= 0.025 { 0.0 } else { (0.245 + 0.755 * normalized.powf(0.85)).copysign(delta) }
}

pub fn update(buttons: u32, lx: f32, ly: f32, rx: f32, ry: f32, lt: f32, rt: f32, active: bool) -> bool {
    if !active { release(); return true; }
    let mut state = STATE.lock().unwrap();
    state.packed = pack(buttons, lx, ly, rx, ry, lt, rt);
    state.active = true;
    UPDATES.fetch_add(1, Ordering::Relaxed);
    true
}
pub fn release() {
    STATE.lock().unwrap().release();
}
pub fn queue_camera_swipe(x: f32, y: f32) {
    if !x.is_finite() || !y.is_finite() { return; }
    let mut state = STATE.lock().unwrap();
    state.swipe_x = (state.swipe_x + x.clamp(-1000.0, 1000.0)).clamp(-1000.0, 1000.0);
    state.swipe_y = (state.swipe_y + y.clamp(-1000.0, 1000.0)).clamp(-1000.0, 1000.0);
}
pub fn clear_camera_swipe() {
    let mut state = STATE.lock().unwrap(); state.swipe_x = 0.0; state.swipe_y = 0.0;
}
pub fn set_radio(x: f32, y: f32, active: bool) {
    let mut state = STATE.lock().unwrap();
    state.radio = [if x.is_finite() { x.clamp(-1.0, 1.0) } else { 0.0 },
        if y.is_finite() { y.clamp(-1.0, 1.0) } else { 0.0 }];
    state.radio_active = active; state.radio_dirty = true;
}
pub fn request_pause() {
    STATE.lock().unwrap().pause_frames = 3;
}
pub fn request_next_aircraft_weapon() { STATE.lock().unwrap().next_aircraft = true; }
pub fn set_targeting_mode(mode: i32) -> bool {
    if !(0..=3).contains(&mode) { return false; }
    STATE.lock().unwrap().targeting_mode = Some(mode); true
}
pub fn take_requests() -> (bool, Option<i32>) {
    let mut state = STATE.lock().unwrap();
    (std::mem::take(&mut state.next_aircraft), state.targeting_mode.take())
}

/// Called immediately before ioPad::UpdateDebug, after the engine clears and
/// fills its pad. Other pads are never touched. Every address is checked.
pub fn apply(mem: &SharedMemory, pad: u64) {
    if pad != PAD_ZERO { return; }
    HOOKS.fetch_add(1, Ordering::Relaxed);
    let (packed, radio) = STATE.lock().unwrap().sample(Instant::now());
    let (base, len) = crate::mem_base_len(mem);
    if let Some(packed) = packed {
        if let Some(range) = crate::host_memory::span(len, pad, 136, 4, "virtual_pad.apply") {
            let base = unsafe { base.add(range.start) };
            let l2 = (packed >> 48) as u8; let r2 = (packed >> 56) as u8;
            let bits = (packed & 0xffff) as u32 | if l2 > 30 { 1 } else { 0 } | if r2 > 30 { 2 } else { 0 };
            unsafe {
                // Upstream merges with the engine's digital buttons and
                // replaces six analog bytes; the engine owns button history.
                (&*(base.add(12) as *const std::sync::atomic::AtomicU32)).fetch_or(bits, Ordering::SeqCst);
                for index in 0..6 {
                    (&*(base.add(20 + index) as *const std::sync::atomic::AtomicU8))
                        .store((packed >> (16 + 8 * index)) as u8, Ordering::SeqCst);
                }
            }
            APPLIED.fetch_add(1, Ordering::Relaxed); LAST_PACKED.store(packed, Ordering::Relaxed);
        }
    }
    if let Some([x, y]) = radio {
        // Radio axes 332/333 have no controller binding in this pinned build.
        // As upstream, write ioValue[1] in both frontend controls. This runs
        // only on the input thread and also writes zero after release.
        for control in [24_660_320u64, 24_488_864] {
            for (index, value) in [-y, x].into_iter().enumerate() {
                let at = control + 33_704 + 72 * (332 + index as u64) + 4;
                if let Some(range) = crate::host_memory::span(len, at, 4, 4, "virtual_pad.radio") {
                    unsafe { (&*(base.add(range.start) as *const std::sync::atomic::AtomicU32))
                        .store(value.to_bits(), Ordering::SeqCst); }
                }
            }
        }
    }
    crate::fivestar_state::publish(mem);
}
pub fn health_json() -> Value {
    let state = STATE.lock().unwrap();
    json!({"source":"kevinerikjs/gta5-ios-port@8e2e4628", "patchVersion":12,
        "hookReady":HOOKS.load(Ordering::Relaxed)>0, "padAddress":PAD_ZERO,
        "active":state.active,"pauseFrames":state.pause_frames,"updates":UPDATES.load(Ordering::Relaxed),
        "hookCalls":HOOKS.load(Ordering::Relaxed),"appliedSamples":APPLIED.load(Ordering::Relaxed),
        "lastPacked":format!("0x{:016x}",LAST_PACKED.load(Ordering::Relaxed)),
        "scope":"ioPad transport only; gameplay acceptance requires device verification"})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn upstream_packing_centres_axes_inverts_y_and_clamps() {
        assert_eq!(pack(0,0.0,0.0,0.0,0.0,0.0,0.0), NEUTRAL);
        let frame=pack(u32::MAX,-1.0,1.0,2.0,-2.0,0.5,1.2);
        assert_eq!(frame & 0xffff,0xffff);assert_eq!((frame>>16)&0xffff,0);
        assert_eq!((frame>>32)&0xffff,0xffff);assert_eq!((frame>>48)&255,128);assert_eq!(frame>>56,255);
        assert_eq!(axis(f32::NAN,false),128);assert_eq!(trigger(f32::INFINITY),0);
    }
    #[test] fn swipe_is_consumed_once_and_selection_wheels_discard_it() {
        let now=Instant::now();let mut s=State::new();s.active=true;s.swipe_x=12.0;
        let first=s.sample(now).0.unwrap();assert_ne!(first,NEUTRAL);
        assert_eq!(s.sample(now).0,Some(NEUTRAL));
        s.packed|=1<<2;s.swipe_x=50.0;
        assert_eq!(s.sample(now).0,Some(NEUTRAL|(1<<2)));
        s.packed=NEUTRAL;assert_eq!(s.sample(now).0,Some(NEUTRAL));
    }
    #[test] fn pause_uses_three_engine_samples_and_radio_release_is_delivered() {
        let mut s=State::new();s.active=true;s.pause_frames=3;s.radio=[0.5,1.0];s.radio_active=true;
        for _ in 0..3 { let (p,r)=s.sample(Instant::now());assert_ne!(p.unwrap()&(1<<11),0);assert_eq!(r,Some([0.5,1.0])); }
        assert_eq!(s.sample(Instant::now()).0.unwrap()&(1<<11),0);
        s.radio_active=false;s.radio_dirty=true;
        assert_eq!(s.sample(Instant::now()).1,Some([0.0,0.0]));assert_eq!(s.sample(Instant::now()).1,None);
    }
    #[test] fn focus_loss_release_does_not_cancel_an_explicit_pause_request() {
        // Exercise the production release policy without global-test coupling.
        let mut state = State::new();state.pause_frames=3;
        state.release();
        assert!(!state.active);
        for _ in 0..3 {
            let frame=state.sample(Instant::now()).0.expect("pause runs without touch focus");
            assert_eq!(frame & 0xffff,1<<11);
        }
        assert_eq!(state.sample(Instant::now()).0,None);
        assert!(!state.active);
    }
}
