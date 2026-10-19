//! The game's existing 48 kHz stereo float ring, consumed by OHAudio.
use crate::RT;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};

static STARTED: AtomicBool = AtomicBool::new(false);
static UNDERRUN_FRAMES: AtomicU64 = AtomicU64::new(0);
static CALLBACKS: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" {
    fn mg_audio_start(callback: extern "C" fn(*mut c_void, *mut f32, u32), user: *mut c_void) -> i32;
    fn mg_audio_pause(paused: i32);
    fn mg_audio_stop();
}

struct Ring { base: usize, address: u64, capacity: u32 }

impl Ring {
    fn header(&self, i: usize) -> &AtomicI32 {
        unsafe { &*((self.base + self.address as usize + 4 * i) as *const AtomicI32) }
    }
}

extern "C" fn fill(user: *mut c_void, samples: *mut f32, frames: u32) {
    let ring = unsafe { &*(user as *const Ring) };
    let out = unsafe { std::slice::from_raw_parts_mut(samples, frames as usize * 2) };
    let read = ring.header(1).load(Ordering::SeqCst) as u32;
    let write = ring.header(0).load(Ordering::SeqCst) as u32;
    let take = write.wrapping_sub(read).min(frames).min(ring.capacity);
    let mask = ring.capacity - 1;
    let data = (ring.base + ring.address as usize + 64) as *const f32;
    for i in 0..take {
        let at = ((read.wrapping_add(i) & mask) * 2) as usize;
        unsafe {
            out[2 * i as usize] = *data.add(at);
            out[2 * i as usize + 1] = *data.add(at + 1);
        }
    }
    out[take as usize * 2..].fill(0.0);
    UNDERRUN_FRAMES.fetch_add((frames - take) as u64, Ordering::Relaxed);
    CALLBACKS.fetch_add(1, Ordering::Relaxed);
    ring.header(1).store(read.wrapping_add(take) as i32, Ordering::SeqCst);
    ring.header(3).fetch_add(1, Ordering::SeqCst);
}

pub fn start(address: u64, capacity: u32) {
    if STARTED.swap(true, Ordering::SeqCst) || std::env::var_os("MUGUET_MUTE").is_some() { return; }
    let Some(rt) = RT.get() else { eprintln!("[audio] runtime is not ready"); return; };
    let (base, length) = crate::mem_base_len(&rt.mem);
    let bytes = 64u64 + capacity as u64 * 2 * 4;
    if !capacity.is_power_of_two() || capacity == 0 || address.checked_add(bytes).is_none_or(|end| end > length as u64) {
        eprintln!("[audio] invalid ring: 0x{address:x}, {capacity} frames, memory {length}");
        return;
    }
    let ring = Box::into_raw(Box::new(Ring { base: base as usize, address, capacity }));
    let result = unsafe { mg_audio_start(fill, ring.cast()) };
    if result != 0 {
        unsafe { drop(Box::from_raw(ring)); }
        eprintln!("[audio] OHAudio output failed: {result}");
        return;
    }
    // The renderer owns callback lifetime; the process owns the runtime/ring.
    eprintln!("[audio] OHAudio 48000 Hz stereo F32 started (ring 0x{address:x}, {capacity} frames)");
}

pub fn suspend(paused: bool) { unsafe { mg_audio_pause(paused as i32); } }
pub fn stop() { unsafe { mg_audio_stop(); } }
