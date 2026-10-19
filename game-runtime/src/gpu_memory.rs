//! Checked arithmetic for GPU host access to the Wasm shared-memory allocation.
use std::ops::Range;

pub fn span(total: usize, addr: u64, size: u64, alignment: u64) -> Result<Range<usize>, String> {
    if alignment == 0 || !alignment.is_power_of_two() || addr % alignment != 0 {
        return Err(format!("unaligned address=0x{addr:x} size={size} alignment={alignment} memoryBytes={total}"));
    }
    let end = addr.checked_add(size).ok_or_else(|| format!("address overflow address=0x{addr:x} size={size} memoryBytes={total}"))?;
    let start = usize::try_from(addr).map_err(|_| format!("address exceeds native usize: 0x{addr:x}"))?;
    let end = usize::try_from(end).map_err(|_| format!("end exceeds native usize: 0x{end:x}"))?;
    if end > total || size > isize::MAX as u64 {
        return Err(format!("out of bounds address=0x{addr:x} size={size} end=0x{end:x} memoryBytes={total}"));
    }
    Ok(start..end)
}

pub fn add(a: u64, b: u64) -> Result<u64, String> {
    a.checked_add(b).ok_or_else(|| format!("addition overflow {a}+{b}"))
}
pub fn mul(a: u64, b: u64) -> Result<u64, String> {
    a.checked_mul(b).ok_or_else(|| format!("multiplication overflow {a}*{b}"))
}
pub fn sub(a: u64, b: u64) -> Result<u64, String> {
    a.checked_sub(b).ok_or_else(|| format!("reversed range {a}-{b}"))
}
pub fn word(base: u64, relative: u64) -> Result<u64, String> {
    mul(add(base, relative)?, 4)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn legal_spans_preserve_exact_range() {
        assert_eq!(span(4096, 1024, 3072, 4).unwrap(), 1024..4096);
        // The earlier driver fault's non-16-byte-aligned copy is still legal.
        assert_eq!(span(20 << 20, 9004776, 3240, 4).unwrap(), 9004776..9008016);
        assert_eq!(span(4096, 4096, 0, 1).unwrap(), 4096..4096);
        assert_eq!(span(4096, 7, 3, 1).unwrap(), 7..10);
    }
    #[test] fn rejects_one_past_end_and_wraparound() {
        assert!(span(4096, 4096, 1, 1).is_err());
        assert!(span(4096, 4097, 0, 1).is_err());
        assert!(span(4096, u64::MAX - 3, 8, 1).is_err());
    }
    #[test] fn atomic_alignment_is_enforced_without_padding() {
        assert!(span(32, 2, 4, 4).is_err());
        assert_eq!(span(32, 4, 4, 4).unwrap(), 4..8);
        assert!(span(32, 0, 4, 0).is_err());
    }
    #[test] fn command_word_add_and_multiply_are_checked() {
        assert_eq!(word(20, 3).unwrap(), 92);
        assert!(word(u64::MAX, 1).is_err());
        assert!(word(u64::MAX / 4 + 1, 0).is_err());
        assert!(sub(3, 4).is_err());
    }
    #[test] fn full_query_record_is_checked_before_any_store() {
        assert_eq!(span(32, 16, 16, 4).unwrap(), 16..32);
        assert!(span(32, 20, 16, 4).is_err());
    }
}
