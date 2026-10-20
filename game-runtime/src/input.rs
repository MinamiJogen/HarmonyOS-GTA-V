// muguet - input.rs
// Constantin Clerc (c22dev) - 0.1

use crate::RT;
#[path = "input_ownership.rs"]
mod ownership;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicU8, Ordering};
use std::sync::{LazyLock, Mutex};

static GUEST_SCRIPT_TICK: AtomicU64 = AtomicU64::new(0);
static POINTER_LOCKED: AtomicBool = AtomicBool::new(false);
static BLOCK: AtomicU64 = AtomicU64::new(0);
// Status-only observation of the last mapped-key write attempt.
static LAST_KEY_BASE_VALID: AtomicI32 = AtomicI32::new(-1);
static LAST_KEY_VK: AtomicI32 = AtomicI32::new(-1);
static LAST_KEY_PRESSED: AtomicBool = AtomicBool::new(false);
static SIZE: Mutex<(i32, i32)> = Mutex::new((1280, 720));
static HELD: LazyLock<Mutex<ownership::Keys>> = LazyLock::new(|| Mutex::new(ownership::Keys::default()));
static BUTTON_OWNERS: LazyLock<Mutex<ownership::Buttons>> = LazyLock::new(|| Mutex::new(ownership::Buttons::default()));

const MX: u64 = 0;
const MY: u64 = 1;
const DX: u64 = 2;
const DY: u64 = 3;
const DZ: u64 = 4;
const BTN: u64 = 5;
const FOCUS: u64 = 6;
const LOCKED: u64 = 7;
const HEAD: u64 = 8;
const TAIL: u64 = 9;

fn base() -> Option<*mut u8> {
    let b = BLOCK.load(Ordering::Acquire);
    if b == 0 { return None; }
    let (p, len) = crate::mem_base_len(&RT.get()?.mem);
    let range = crate::host_memory::span(len, b, 444, 4, "input.block")?;
    Some(unsafe { p.add(range.start) })
}
fn word(i: u64) -> Option<&'static AtomicI32> {
    base().map(|p| unsafe { &*(p.add(256 + 4 * i as usize) as *const AtomicI32) })
}
fn ext(i: u64) -> Option<&'static AtomicI32> {
    base().map(|p| unsafe { &*(p.add(424 + 4 * i as usize) as *const AtomicI32) })
}
fn key_byte(vk: u8) -> Option<&'static AtomicU8> {
    base().map(|p| unsafe { &*(p.add(vk as usize) as *const AtomicU8) })
}

pub fn publish(block: u64) {
    let Some(rt) = RT.get() else { return; };
    let (_, len) = crate::mem_base_len(&rt.mem);
    if crate::host_memory::span(len, block, 444, 4, "input.publish").is_none() { return; }
    BLOCK.store(block, Ordering::Release);
    let (w, h) = *SIZE.lock().unwrap();
    if let Some(e) = ext(2) { e.store(w, Ordering::SeqCst); }
    if let Some(e) = ext(3) { e.store(h, Ordering::SeqCst); }
    if let Some(f) = word(FOCUS) { f.store(1, Ordering::SeqCst); }
    if let Some(l) = word(LOCKED) { l.store(POINTER_LOCKED.load(Ordering::Relaxed) as i32, Ordering::SeqCst); }
    if let Some(m) = word(MX) { m.store(w >> 1, Ordering::SeqCst); }
    if let Some(m) = word(MY) { m.store(h >> 1, Ordering::SeqCst); }
    if let Ok(mode) = std::env::var("MUGUET_START_MODE") {
        if let (Ok(m), Some(p)) = (mode.parse::<i32>(), base()) {
            unsafe { &*(p.add(440) as *const AtomicI32) }.store(m, Ordering::SeqCst);
        }
    }
    eprintln!("[input] engine input block at 0x{block:x}");
}

pub fn set_size(w: i32, h: i32) {
    *SIZE.lock().unwrap() = (w, h);
    if let Some(e) = ext(2) { e.store(w, Ordering::SeqCst); }
    if let Some(e) = ext(3) { e.store(h, Ordering::SeqCst); }
}
pub fn size() -> (i32, i32) {
    *SIZE.lock().unwrap()
}

/// Low-frequency status snapshot; never changes shared input bytes or words.
pub fn health_json() -> serde_json::Value {
    let block = BLOCK.load(Ordering::Acquire);
    let p = base(); // The same checked 444-byte, 4-aligned span as input writes.
    let read_word = |i: u64| p.map(|p| unsafe {
        (&*(p.add(256 + 4 * i as usize) as *const AtomicI32)).load(Ordering::SeqCst)
    });
    let read_key = |vk: u8| p.map(|p| unsafe {
        (&*(p.add(vk as usize) as *const AtomicU8)).load(Ordering::SeqCst)
    });
    let (w_held, escape_held) = {
        let held = HELD.lock().unwrap();
        (held.contains("KeyW"), held.contains("Escape"))
    };
    let last_valid = LAST_KEY_BASE_VALID.load(Ordering::Relaxed);
    let last_vk = LAST_KEY_VK.load(Ordering::Relaxed);
    serde_json::json!({
        "blockAddress": format!("0x{block:x}"), "published": block != 0,
        "checkedBaseValid": p.is_some(), "focusWord": read_word(FOCUS),
        "lockedWord": read_word(LOCKED), "wByte": read_key(0x57),
        "escapeByte": read_key(0x1B), "wHeld": w_held, "escapeHeld": escape_held,
        "lastKeyWriteBaseValid": if last_valid < 0 { None } else { Some(last_valid != 0) },
        "lastKeyVk": if last_vk < 0 { None } else { Some(last_vk) },
        "lastKeyPressed": if last_valid < 0 { None } else { Some(LAST_KEY_PRESSED.load(Ordering::Relaxed)) },
        "buttonsWord": read_word(BTN), "mouseDx": read_word(DX), "mouseDy": read_word(DY), "mouseWheel": read_word(DZ),
        "scope": "nontransactional read-only snapshot; lastKey fields describe mapped-key write attempts"
    })
}

pub fn set_focus(focused: bool) {
    crate::pad::FOCUSED.store(focused, Ordering::Relaxed);
    if !focused { crate::pad::release_all(); }
    if let Some(f) = word(FOCUS) { f.store(focused as i32, Ordering::SeqCst); }
    if !focused { release_all(); }
}
pub fn set_locked(locked: bool) {
    POINTER_LOCKED.store(locked, Ordering::Relaxed);
    if let Some(l) = word(LOCKED) { l.store(locked as i32, Ordering::SeqCst); }
}
pub fn release_all() {
    HELD.lock().unwrap().clear();
    BUTTON_OWNERS.lock().unwrap().clear();
    for i in 0..=255u8 { if let Some(k) = key_byte(i) { k.store(0, Ordering::SeqCst); } }
    if let Some(b) = word(BTN) { b.store(0, Ordering::SeqCst); }
    for i in [DX, DY, DZ] { if let Some(w) = word(i) { w.store(0, Ordering::SeqCst); } }
    *FRAC.lock().unwrap() = (0.0, 0.0);
    *WHEEL_FRAC.lock().unwrap() = 0.0;
}
pub fn is_held(code: &str) -> bool { HELD.lock().unwrap().contains(code) }

fn vk(code: &str) -> Option<u8> {
    let v = match code {
        "Backspace" => 0x08, "Tab" => 0x09, "Enter" => 0x0D, "NumpadEnter" => 0xFD, "Pause" => 0x13, "CapsLock" => 0x14, "Escape" => 0x1B,
        "Space" => 0x20, "PageUp" => 0x21, "PageDown" => 0x22, "End" => 0x23, "Home" => 0x24, "ArrowLeft" => 0x25, "ArrowUp" => 0x26,
        "ArrowRight" => 0x27, "ArrowDown" => 0x28, "PrintScreen" => 0x2C, "Insert" => 0x2D, "Delete" => 0x2E, "MetaLeft" => 0x5B,
        "MetaRight" => 0x5C, "ContextMenu" => 0x5D, "NumpadMultiply" => 0x6A, "NumpadAdd" => 0x6B, "NumpadSubtract" => 0x6D,
        "NumpadDecimal" => 0x6E, "NumpadDivide" => 0x6F, "NumLock" => 0x90, "ScrollLock" => 0x91, "ShiftLeft" => 0xA0, "ShiftRight" => 0xA1,
        "ControlLeft" => 0xA2, "ControlRight" => 0xA3, "AltLeft" => 0xA4, "AltRight" => 0xA5, "Semicolon" => 0xBA, "Equal" => 0xBB,
        "Comma" => 0xBC, "Minus" => 0xBD, "Period" => 0xBE, "Slash" => 0xBF, "Backquote" => 0xC0, "BracketLeft" => 0xDB,
        "Backslash" => 0xDC, "BracketRight" => 0xDD, "Quote" => 0xDE, "IntlBackslash" => 0xE2,
        c => {
            if let Some(d) = c.strip_prefix("Digit") { return d.parse::<u8>().ok().filter(|d| *d < 10).map(|d| 0x30 + d); }
            if let Some(d) = c.strip_prefix("Numpad") { return d.parse::<u8>().ok().filter(|d| *d < 10).map(|d| 0x60 + d); }
            if let Some(k) = c.strip_prefix("Key") { let b = k.as_bytes(); return (b.len() == 1 && b[0].is_ascii_uppercase()).then(|| b[0]); }
            if let Some(f) = c.strip_prefix('F') { return f.parse::<u8>().ok().filter(|f| (1..=24).contains(f)).map(|f| 0x6F + f); }
            return None;
        }
    };
    Some(v)
}

fn dom_code(winit_name: &str) -> &str {
    match winit_name { "SuperLeft" => "MetaLeft", "SuperRight" => "MetaRight", n => n }
}

const FLY_ALIAS: [(&str, &str); 6] = [("KeyI", "Numpad8"), ("KeyK", "Numpad2"), ("KeyJ", "Numpad4"), ("KeyL", "Numpad6"), ("KeyU", "Numpad7"), ("KeyO", "Numpad9")];

static PICKING_MAP: AtomicBool = AtomicBool::new(false);
pub fn start_mode() -> i32 {
    base().map(|p| unsafe { &*(p.add(440) as *const AtomicI32) }.load(Ordering::SeqCst)).unwrap_or(0)
}
fn start_menu_key(code: &str) {
    let Some(p) = base() else { return };
    let word = unsafe { &*(p.add(440) as *const AtomicI32) };
    if word.load(Ordering::SeqCst) != 0 { return; }
    let picking = PICKING_MAP.load(Ordering::SeqCst);
    let choice = match code {
        "Enter" | "NumpadEnter" if !picking => 1,
        "Space" => { PICKING_MAP.store(true, Ordering::SeqCst); 0 }
        "Backspace" => { PICKING_MAP.store(false, Ordering::SeqCst); 0 }
        "Digit5" | "Numpad5" => 2,
        _ => 0,
    };
    if choice != 0 {
        word.store(choice, Ordering::SeqCst);
        eprintln!("[input] start mode chosen: {}", ["", "Story Mode", "Sandbox"][choice as usize]);
    }
}

pub fn key(winit_name: &str, pressed: bool, repeat: bool, text: Option<&str>, ctrl_or_cmd: bool) -> bool {
    let code = dom_code(winit_name);
    {
        let mut held = HELD.lock().unwrap();
        if !held.legacy(code, pressed) {return false;}
        write_held_key(code, held.contains(code));
        for (src, target) in FLY_ALIAS {
            write_held_key(target, held.legacy_contains(src) || held.contains(target));
        }
    }
    if !pressed { return true; }
    if !repeat {start_menu_key(code);}
    let ch: Option<u16> = match text {
        Some(t) if t.chars().count() == 1 && !ctrl_or_cmd && !t.chars().next().unwrap().is_control() => t.encode_utf16().next(),
        _ if code == "Enter" => Some(13),
        _ if code == "Backspace" => Some(8),
        _ if code == "Space" && !ctrl_or_cmd => Some(32),
        _ => None,
    };
    if let Some(ch) = ch { queue_character(ch); }
    true
}

fn write_held_key(code: &str, pressed: bool) {
    if let Some(v) = vk(code) {
        LAST_KEY_VK.store(v as i32, Ordering::Relaxed); LAST_KEY_PRESSED.store(pressed, Ordering::Relaxed);
        if let Some(k) = key_byte(v) {
            k.store(if pressed { 0x80 } else { 0 }, Ordering::SeqCst); LAST_KEY_BASE_VALID.store(1, Ordering::Relaxed);
        } else { LAST_KEY_BASE_VALID.store(0, Ordering::Relaxed); }
    }
}
fn queue_character(ch: u16) {
    if let (Some(head), Some(tail), Some(b)) = (word(HEAD), word(TAIL), base()) {
        let h = head.load(Ordering::SeqCst);
        if (h.wrapping_sub(tail.load(Ordering::SeqCst)) as u32) < 64 {
            unsafe { std::ptr::write_volatile(b.add(296 + 2 * (h & 63) as usize) as *mut u16, ch) };
            head.store(h.wrapping_add(1), Ordering::SeqCst);
        }
    }
}
/// Literal guest keyboard: no startup choices, host shortcuts, aliases or MP commands.
/// The character queue is a separate single-ASCII path; named key identity remains intact.
pub fn raw_key(code: &str, pressed: bool, source: &str) -> bool {
    if vk(code).is_none() || source.is_empty() || source.len() > 128 { return false; }
    let mut held = HELD.lock().unwrap();
    let was_down = held.raw_event(code, pressed, source, GUEST_SCRIPT_TICK.load(Ordering::Relaxed));
    let aliased = FLY_ALIAS.iter().any(|(src, target)| *target == code && held.legacy_contains(src));
    write_held_key(code, held.contains(code) || aliased);
    if pressed && !was_down { if let Some(ch) = held.ascii(code) { queue_character(ch); } }
    true
}

/// Called once at the selected script safe point, independently of pad ACTIVE.
pub fn guest_script_tick() {
    let tick=GUEST_SCRIPT_TICK.fetch_add(1,Ordering::Relaxed).wrapping_add(1);
    let mut held=HELD.lock().unwrap();
    for code in held.guest_tick(tick) {
        let aliased=FLY_ALIAS.iter().any(|(src,target)| *target==code && held.legacy_contains(src));
        write_held_key(&code,held.contains(&code)||aliased);
    }
    drop(held);
    let mut buttons=BUTTON_OWNERS.lock().unwrap();buttons.guest_tick(tick);write_buttons(&buttons);
}

pub fn button(dom_button: usize, pressed: bool) { button_source(dom_button, pressed, "legacy"); }
pub fn button_source(dom_button: usize, pressed: bool, source: &str) {
    if dom_button>4 {return;}
    if source.is_empty() || source.len() > 128 { return; }
    let mut owners = BUTTON_OWNERS.lock().unwrap();
    owners.event(dom_button,pressed,source,GUEST_SCRIPT_TICK.load(Ordering::Relaxed));
    write_buttons(&owners);
}
fn write_buttons(owners:&ownership::Buttons) {
    if let Some(word)=word(BTN) {
        let mut bits=0;
        for (button,bit) in [0,2,1,3,4].into_iter().enumerate() {if owners.down(button) {bits|=1<<bit;}}
        word.store(bits,Ordering::SeqCst);
    }
}

pub fn mouse_absolute(x:i32,y:i32) {
    let (w,h)=size();if let Some(mx)=word(MX){mx.store(x.clamp(0,w-1),Ordering::SeqCst);}if let Some(my)=word(MY){my.store(y.clamp(0,h-1),Ordering::SeqCst);}
}

pub fn mouse_pos(x: i32, y: i32) {
    let (w, h) = size();
    let (x, y) = (x.clamp(0, w - 1), y.clamp(0, h - 1));
    if let (Some(mx), Some(my), Some(dx), Some(dy)) = (word(MX), word(MY), word(DX), word(DY)) {
        dx.fetch_add(x - mx.load(Ordering::SeqCst), Ordering::SeqCst);
        dy.fetch_add(y - my.load(Ordering::SeqCst), Ordering::SeqCst);
        mx.store(x, Ordering::SeqCst);
        my.store(y, Ordering::SeqCst);
    }
}

static SENS: LazyLock<f64> = LazyLock::new(|| std::env::var("MUGUET_MOUSE_SENS").ok().and_then(|v| v.parse().ok()).unwrap_or(2.0));
static FRAC: Mutex<(f64, f64)> = Mutex::new((0.0, 0.0));

pub fn mouse_delta_f(dx: f64, dy: f64) {
    let (ix, iy) = {
        let mut f = FRAC.lock().unwrap();
        f.0 += dx * *SENS;
        f.1 += dy * *SENS;
        let (ix, iy) = (f.0.trunc(), f.1.trunc());
        f.0 -= ix;
        f.1 -= iy;
        (ix as i32, iy as i32)
    };
    if ix != 0 || iy != 0 { mouse_delta(ix, iy); }
}

fn mouse_delta(dx_: i32, dy_: i32) {
    let (w, h) = size();
    if let (Some(mx), Some(my), Some(dx), Some(dy)) = (word(MX), word(MY), word(DX), word(DY)) {
        dx.fetch_add(dx_, Ordering::SeqCst);
        dy.fetch_add(dy_, Ordering::SeqCst);
        mx.store((mx.load(Ordering::SeqCst) + dx_).clamp(0, w - 1), Ordering::SeqCst);
        my.store((my.load(Ordering::SeqCst) + dy_).clamp(0, h - 1), Ordering::SeqCst);
    }
}
pub fn wheel(notches: i32) {
    if notches != 0 { if let Some(z) = word(DZ) { z.fetch_add(notches, Ordering::SeqCst); } }
}
static WHEEL_FRAC: Mutex<f32> = Mutex::new(0.0);
pub fn wheel_fractional(notches: f32) {
    if !notches.is_finite() {return;}
    let mut fraction = WHEEL_FRAC.lock().unwrap();
    *fraction += notches.clamp(-32.0, 32.0);
    let whole = fraction.trunc() as i32;
    *fraction -= whole as f32;
    wheel(whole);
}
