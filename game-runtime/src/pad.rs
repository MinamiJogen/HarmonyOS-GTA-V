// muguet - pad.rs
// Constantin Clerc (c22dev) - 0.1

use crate::input;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub static ACTIVE: AtomicBool = AtomicBool::new(false);
pub static FOCUSED: AtomicBool = AtomicBool::new(true);

pub static IN_VEHICLE: AtomicBool = AtomicBool::new(false);

pub const MODE_FOOT: u8 = 0;
pub const MODE_CAR: u8 = 1;
pub const MODE_BIKE: u8 = 2;
pub const MODE_HELI: u8 = 3;
pub const MODE_PLANE: u8 = 4;
pub const MODE_BOAT: u8 = 5;
pub const MODE_SUB: u8 = 6;
pub const MODE_BICYCLE: u8 = 7;
pub const MODE_PASSENGER: u8 = 8;
pub static VEH_MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(MODE_FOOT);

pub const CTX_TOWING: u32 = 1;
pub const CTX_TRAILER: u32 = 2;
pub const CTX_CARGOBOB: u32 = 4;
pub const CTX_ROPE: u32 = 8;
pub const CTX_CONVERTIBLE: u32 = 16;
pub const CTX_VTOL: u32 = 32;
pub const CTX_GUN: u32 = 64;
pub const CTX_MELEE: u32 = 128;
pub const CTX_THROWN: u32 = 256;
pub const CTX_SWIM: u32 = 512;
pub const CTX_SNIPER: u32 = 1024;
pub static CONTEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub static INJECT: Mutex<Vec<(i32, f32)>> = Mutex::new(Vec::new());
pub static RADIO_WHEEL: Mutex<Option<(f32, f32)>> = Mutex::new(None);
pub const FRONTEND_CONTROL: i32 = 2;
pub const RADIO_WHEEL_UD: i32 = 332;
pub const RADIO_WHEEL_LR: i32 = 333;

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub enum Request {
    DropTow,
    DropTrailer,
    Rope,
}
pub static REQUESTS: Mutex<Vec<Request>> = Mutex::new(Vec::new());
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub fn request(r: Request) {
    REQUESTS.lock().unwrap().push(r);
}

static MENU: AtomicBool = AtomicBool::new(false);
static SCRIPT_TICK: Mutex<Option<Instant>> = Mutex::new(None);
pub fn script_tick(menu: bool) {
    MENU.store(menu, Ordering::Relaxed);
    *SCRIPT_TICK.lock().unwrap() = Some(Instant::now());
}
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub fn in_menu() -> bool {
    MENU.load(Ordering::Relaxed) || SCRIPT_TICK.lock().unwrap().is_some_and(|t| t.elapsed() > Duration::from_millis(500))
}

pub static HORN_PAD: AtomicBool = AtomicBool::new(false);
pub static HORN_TOUCH: AtomicBool = AtomicBool::new(false);
pub fn horn_held() -> bool {
    HORN_PAD.load(Ordering::Relaxed) || HORN_TOUCH.load(Ordering::Relaxed)
}

struct State {
    held: HashSet<&'static str>,
    buttons: u8,
    last: Instant,
    down_since: Option<Instant>,
}
static STATE: Mutex<Option<State>> = Mutex::new(None);

const SWITCH_HOLD: Duration = Duration::from_millis(300);
const STICK_DEADZONE: f32 = 0.3;
const LOOK_DEADZONE: f32 = 0.12;

const LOOK_SPEED: f64 = 700.0;

// OHOS input arrives through mg_game_key/mg_game_control. No Apple controller dependency.
pub fn poll() -> bool { ACTIVE.load(Ordering::Relaxed) }

fn release(st: State) {
    for k in st.held { input::key(k, false, false, None, false); }
    if st.buttons & 1 != 0 { input::button(0, false); }
    if st.buttons & 2 != 0 { input::button(2, false); }
}

pub fn release_all() {
    HORN_PAD.store(false, Ordering::Relaxed);
    if let Some(st) = STATE.lock().unwrap().take() { release(st); }
}
