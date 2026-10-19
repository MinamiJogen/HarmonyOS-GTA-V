//! Pure accounting shared by the optional GPU profiler and its host tests.
pub const QUERIES: u32 = 512;
pub const SLOTS: usize = 4;
pub const SAMPLE_EVERY: u64 = 30;
pub const MAX_OUTPUT_BYTES: u64 = 8 * 1024 * 1024;

pub fn reserve_pair(next: &mut u32) -> Option<(u32, u32)> {
    let end = next.checked_add(1)?;
    if end >= QUERIES { return None; }
    let start = *next;
    *next = end + 1;
    Some((start, end))
}

pub fn ticks(start: u64, end: u64, valid_bits: u32) -> Option<u64> {
    match valid_bits {
        1..=63 => Some(end.wrapping_sub(start) & ((1u64 << valid_bits) - 1)),
        64 => Some(end.wrapping_sub(start)),
        _ => None,
    }
}

pub fn wrap_ambiguous(host_ns: u128, valid_bits: u32, period: f32) -> bool {
    if !(1..=64).contains(&valid_bits) || !period.is_finite() || period <= 0.0 { return true; }
    host_ns as f64 >= (2f64.powi(valid_bits as i32) * period as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn pairs_never_overlap_frame_or_overrun() {
        let mut n = 2; let mut pairs = Vec::new();
        while let Some(pair) = reserve_pair(&mut n) { pairs.push(pair); }
        assert_eq!(pairs.len(), 255); assert_eq!(pairs[0], (2,3));
        assert_eq!(pairs[254], (510,511)); assert_eq!(n, QUERIES);
        assert_eq!(reserve_pair(&mut n), None); assert_eq!(n, QUERIES);
    }
    #[test] fn counter_wrap_uses_actual_valid_bits() {
        assert_eq!(ticks((1<<36)-3, 5, 36), Some(8));
        assert_eq!(ticks(u64::MAX-2, 4, 64), Some(7));
        assert_eq!(ticks(0,1,0), None); assert_eq!(ticks(0,1,65), None);
        assert!(!wrap_ambiguous(100_000_000,36,1.0));
        assert!(wrap_ambiguous(1u128<<36,36,1.0));
        assert!(wrap_ambiguous(1,64,0.0));
    }
}
