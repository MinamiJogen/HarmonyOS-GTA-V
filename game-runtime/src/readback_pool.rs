//! Idle, unmapped readback buffers only. GPU/callback ownership stays in gpu.rs.
//! A caller may return a buffer only after its mapped view has been dropped,
//! all guest copies finished, and unmap completed. Every byte must have been
//! overwritten by the current GPU copy, so old cached contents are unobservable.

pub const PER_KEY_LIMIT: usize = 4;
pub const BUFFER_LIMIT: usize = 16;
pub const BYTE_LIMIT: u64 = 16 << 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Key { pub size: u64, pub usage: u32 }

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub created: u64,
    pub reused: u64,
    pub returned: u64,
    pub rejected: u64,
    pub bypassed: u64,
    pub cached_buffers: usize,
    pub cached_bytes: u64,
    pub peak_buffers: usize,
    pub peak_bytes: u64,
}

pub struct Pool<T> { idle: Vec<(Key, T)>, stats: Stats }
impl<T> Pool<T> {
    pub fn new() -> Self { Self { idle: Vec::new(), stats: Stats::default() } }

    pub fn take(&mut self, key: Key) -> Option<T> {
        let i = self.idle.iter().position(|(k, _)| *k == key)?;
        let (_, buffer) = self.idle.swap_remove(i);
        self.stats.cached_buffers -= 1;
        self.stats.cached_bytes -= key.size;
        self.stats.reused = self.stats.reused.saturating_add(1);
        Some(buffer)
    }

    pub fn created(&mut self, eligible: bool) {
        self.stats.created = self.stats.created.saturating_add(1);
        if !eligible { self.stats.bypassed = self.stats.bypassed.saturating_add(1); }
    }

    /// Takes ownership of a completed and unmapped buffer. Rejected entries drop.
    pub fn return_unmapped(&mut self, key: Key, buffer: T) {
        let next = self.stats.cached_bytes.checked_add(key.size);
        if key.size == 0 || self.idle.len() >= BUFFER_LIMIT
            || self.idle.iter().filter(|(k, _)| *k == key).count() >= PER_KEY_LIMIT
            || next.is_none_or(|n| n > BYTE_LIMIT) {
            self.stats.rejected = self.stats.rejected.saturating_add(1);
            return;
        }
        self.idle.push((key, buffer));
        self.stats.returned = self.stats.returned.saturating_add(1);
        self.stats.cached_buffers += 1;
        self.stats.cached_bytes = next.unwrap();
        self.stats.peak_buffers = self.stats.peak_buffers.max(self.stats.cached_buffers);
        self.stats.peak_bytes = self.stats.peak_bytes.max(self.stats.cached_bytes);
    }

    pub fn stats(&self) -> Stats { self.stats }
}

/// Strong eligibility: the entire allocation, not only exported guest rows,
/// must be overwritten. A partial/empty copy keeps the original fresh buffer.
pub fn full_buffer_copy(allocation: u64, copied: u64, source_offset: u64, source_size: u64) -> bool {
    allocation != 0 && copied == allocation && source_offset % 4 == 0
        && source_offset.checked_add(copied).is_some_and(|end| end <= source_size)
}

pub fn source_region_valid(origin: [u64; 3], extent: [u64; 3], source: [u64; 3]) -> bool {
    (0..3).all(|i| extent[i] != 0
        && origin[i].checked_add(extent[i]).is_some_and(|end| end <= source[i]))
}

pub fn full_texture_copy(allocation: u64, row_bytes: u64, row_stride: u64,
    block_rows: u64, requested_depth: u64, copied_depth: u64) -> bool {
    allocation != 0 && row_bytes != 0 && row_bytes == row_stride
        && requested_depth != 0 && requested_depth == copied_depth
        && row_stride.checked_mul(block_rows).and_then(|n| n.checked_mul(copied_depth)) == Some(allocation)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_size_and_usage_are_required_and_checked_out_objects_are_not_idle() {
        let mut p = Pool::new();
        let a = Key { size: 262144, usage: 9 };
        p.return_unmapped(a, vec![0x71u8; 4]);
        assert!(p.take(Key { size: 524288, ..a }).is_none());
        assert!(p.take(Key { usage: 8, ..a }).is_none());
        let mut borrowed = p.take(a).unwrap();
        assert!(p.take(a).is_none()); // cannot acquire an in-flight/mapped lease
        borrowed.fill(0x32); // a full overwrite before the lease can be returned
        p.return_unmapped(a, borrowed);
        assert_eq!(p.take(a).unwrap(), vec![0x32; 4]);
        assert_eq!(p.stats().cached_bytes, 0);
        assert_eq!(p.stats().reused, 2);
    }

    #[test]
    fn per_key_global_count_byte_and_overflow_limits_are_hard() {
        let a = Key { size: 262144, usage: 9 };
        let mut p = Pool::new();
        for i in 0..8 { p.return_unmapped(a, i); }
        assert_eq!(p.stats().cached_buffers, PER_KEY_LIMIT);
        assert_eq!(p.stats().rejected, 4);
        for i in 1..32 { p.return_unmapped(Key { size: i, ..a }, i); }
        assert_eq!(p.stats().cached_buffers, BUFFER_LIMIT);
        assert!(p.stats().cached_bytes <= BYTE_LIMIT);
        let mut p = Pool::new();
        p.return_unmapped(Key { size: BYTE_LIMIT, ..a }, 1);
        p.return_unmapped(a, 2);
        p.return_unmapped(Key { size: u64::MAX, ..a }, 3);
        p.return_unmapped(Key { size: 0, ..a }, 4);
        assert_eq!(p.stats().cached_buffers, 1);
        assert_eq!(p.stats().peak_bytes, BYTE_LIMIT);
        assert_eq!(p.stats().rejected, 3);
    }

    #[test]
    fn partial_copies_padding_and_uncertain_texture_layers_cannot_reuse_old_bytes() {
        assert!(full_buffer_copy(262144, 262144, 0, 262144));
        assert!(!full_buffer_copy(8, 4, 0, 4)); // fresh-buffer zero tail must be retained
        assert!(!full_buffer_copy(4, 0, 0, 4));
        assert!(!full_buffer_copy(0, 0, 0, 0));
        assert!(!full_buffer_copy(4, 4, 1, 8)); // rejected unaligned GPU copy
        assert!(!full_buffer_copy(8, 8, 4, 8)); // rejected source end
        assert!(!full_buffer_copy(4, 4, u64::MAX - 3, u64::MAX));
        assert!(source_region_valid([0, 0, 0], [256, 256, 1], [256, 256, 1]));
        assert!(!source_region_valid([1, 0, 0], [256, 256, 1], [256, 256, 1]));
        assert!(!source_region_valid([0, 0, 1], [256, 256, 1], [256, 256, 1]));
        assert!(!source_region_valid([0, 0, 0], [0, 256, 1], [256, 256, 1]));
        assert!(!source_region_valid([u64::MAX, 0, 0], [1, 1, 1], [u64::MAX, 1, 1]));
        assert!(full_texture_copy(262144, 1024, 1024, 256, 1, 1));
        assert!(full_texture_copy(524288, 2048, 2048, 256, 1, 1));
        assert!(!full_texture_copy(256, 4, 256, 1, 1, 1)); // row padding
        assert!(!full_texture_copy(524288, 1024, 1024, 256, 2, 1)); // D2 copies one layer
        assert!(!full_texture_copy(4, 0, 0, 0, 1, 1));
        assert!(!full_texture_copy(16, u64::MAX, u64::MAX, 2, 1, 1));
    }

    #[test]
    fn rejected_and_retained_objects_are_really_destroyed_without_leaks() {
        use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
        struct Owned(Arc<AtomicUsize>);
        impl Drop for Owned { fn drop(&mut self) { self.0.fetch_add(1, Ordering::Relaxed); } }
        let drops = Arc::new(AtomicUsize::new(0));
        {
            let mut p = Pool::new(); let key = Key { size: 262144, usage: 9 };
            for _ in 0..5 { p.return_unmapped(key, Owned(drops.clone())); }
            assert_eq!(drops.load(Ordering::Relaxed), 1);
            drop(p.take(key));
            assert_eq!(drops.load(Ordering::Relaxed), 2);
        }
        assert_eq!(drops.load(Ordering::Relaxed), 5);
    }
}
