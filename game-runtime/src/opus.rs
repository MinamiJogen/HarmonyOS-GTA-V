// muguet - opus.rs
// Constantin Clerc (c22dev) - 0.1

use audiopus_sys as ffi;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

const MODE_SILK: i32 = 1000;
const MODE_HYBRID: i32 = 1001;
const MODE_CELT: i32 = 1002;
const OPUS_RESET_STATE: i32 = 4028;

struct Native { dec: *mut ffi::OpusDecoder, fs: i32, channels: i32 }
unsafe impl Send for Native {}

static DECODERS: Mutex<Option<HashMap<u64, Native>>> = Mutex::new(None);
static MODE_OFF: AtomicI64 = AtomicI64::new(-1);

fn rd(mem: &[u8], at: u64) -> i32 {
    let a = at as usize;
    mem.get(a..a + 4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0)
}

fn find_mode_off(mem: &[u8], st: u64) -> Option<i64> {
    (16..4096u64).step_by(4).find(|&o| {
        let (sc, bw, mode) = (rd(mem, st + o - 8), rd(mem, st + o - 4), rd(mem, st + o));
        (1..=2).contains(&sc) && (1101..=1105).contains(&bw) && (MODE_SILK..=MODE_CELT).contains(&mode)
    }).map(|o| o as i64)
}

fn toc_config(mode: i32, bandwidth: i32, samples: i32, fs: i32) -> Option<u8> {
    let quarter_ms = samples as i64 * 4000 / fs as i64; // duration in 0.25 ms
    let bw = bandwidth - 1101; // 0 NB, 1 MB, 2 WB, 3 SWB, 4 FB
    match mode {
        MODE_SILK => {
            let d = match quarter_ms { 40 => 0, 80 => 1, 160 => 2, 240 => 3, _ => return None };
            if bw > 2 { return None; }
            Some((bw * 4 + d) as u8)
        }
        MODE_HYBRID => {
            let d = match quarter_ms { 40 => 0, 80 => 1, _ => return None };
            Some(match bw { 3 => 12 + d, 4 => 14 + d, _ => return None } as u8)
        }
        MODE_CELT => {
            let d = match quarter_ms { 10 => 0, 20 => 1, 40 => 2, 80 => 3, _ => return None };
            let base = match bw { 0 => 16, 1 | 2 => 20, 3 => 24, _ => 28 };
            Some((base + d) as u8)
        }
        _ => None,
    }
}

pub fn decode_frame(mem: &mut [u8], st: u64, data: u64, len: i32, pcm: u64, frame_size: i32, fec: i32) -> i64 {
    let (channels, fs) = (rd(mem, st + 8), rd(mem, st + 12));
    if !(1..=2).contains(&channels) || fs <= 0 { return -1; }
    let mut off = MODE_OFF.load(Ordering::Relaxed);
    if off < 0 && data != 0 && len > 0 {
        match find_mode_off(mem, st) {
            Some(o) => { MODE_OFF.store(o, Ordering::Relaxed); off = o; eprintln!("[opus] native decoding: decoder fields at +{o}"); }
            None => return -1,
        }
    }
    if off < 0 { return -1; }
    let off = off as u64;
    let (stream_channels, bandwidth, mode) = (rd(mem, st + off - 8), rd(mem, st + off - 4), rd(mem, st + off));
    let (prev_mode, packet_frame) = (rd(mem, st + off + 4), rd(mem, st + off + 8));

    let mut guard = DECODERS.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    if map.get(&st).is_some_and(|n| n.fs != fs || n.channels != channels) {
        if let Some(n) = map.remove(&st) { unsafe { ffi::opus_decoder_destroy(n.dec) }; }
    }
    let native = match map.get(&st) {
        Some(n) => n,
        None => {
            let mut err = 0;
            let dec = unsafe { ffi::opus_decoder_create(fs, channels, &mut err) };
            if dec.is_null() || err != 0 { return -1; }
            map.entry(st).or_insert(Native { dec, fs, channels })
        }
    };
    if prev_mode == 0 { unsafe { ffi::opus_decoder_ctl(native.dec, OPUS_RESET_STATE) }; }

    let cap = frame_size.max(0) as usize;
    let out_len = cap * channels as usize;
    let mut out = vec![0f32; out_len];
    let n = if data == 0 || len <= 0 {
        unsafe { ffi::opus_decode_float(native.dec, std::ptr::null(), 0, out.as_mut_ptr(), frame_size, 0) }
    } else {
        let samples = if packet_frame > 0 { packet_frame } else { frame_size };
        let Some(config) = toc_config(mode, bandwidth, samples, fs) else { return -1 };
        let Some(frame) = mem.get(data as usize..data as usize + len as usize) else { return -1 };
        let mut packet = Vec::with_capacity(len as usize + 1);
        packet.push(config << 3 | if stream_channels == 2 { 4 } else { 0 });
        packet.extend_from_slice(frame);
        unsafe { ffi::opus_decode_float(native.dec, packet.as_ptr(), packet.len() as i32, out.as_mut_ptr(), frame_size, fec) }
    };
    if n > 0 {
        let bytes = n as usize * channels as usize * 4;
        if let Some(dst) = mem.get_mut(pcm as usize..pcm as usize + bytes) {
            for (d, s) in dst.chunks_exact_mut(4).zip(out.iter()) { d.copy_from_slice(&s.to_le_bytes()); }
        }
        if data != 0 && len > 0 {
            let at = (st + off + 4) as usize;
            if let Some(d) = mem.get_mut(at..at + 4) { d.copy_from_slice(&mode.to_le_bytes()); }
        }
    }
    n as i64
}
