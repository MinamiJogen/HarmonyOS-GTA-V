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
    pub error_invalidations: u64,
    pub error_invalidated_buffers: u64,
}

pub struct Pool<T> { idle: Vec<(Key, T)>, stats: Stats, error_generation: u64 }
impl<T> Pool<T> {
    pub fn new() -> Self { Self { idle: Vec::new(), stats: Stats::default(), error_generation: 0 } }

    /// Error generations are the monotonic, process-local uncaptured-error
    /// count. In-flight leases live outside this pool and are not invalidated
    /// by dropping idle entries; their completion must check its own generation.
    pub fn observe_error_generation(&mut self, generation: u64) {
        if generation == self.error_generation { return; }
        self.clear_idle();
        self.error_generation = generation;
        self.stats.error_invalidations = self.stats.error_invalidations.saturating_add(1);
    }

    pub fn clear_idle(&mut self) {
        self.stats.error_invalidated_buffers = self.stats.error_invalidated_buffers.saturating_add(self.idle.len() as u64);
        self.idle.clear();
        self.stats.cached_buffers = 0;
        self.stats.cached_bytes = 0;
    }

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

/// An error before a lease began is not a lifetime ban. An error during its
/// submission/map/CPU-copy lifetime is: that lease must never return to idle.
pub fn clean_completion(start_generation: u64, current_generation: u64,
    fully_written: bool, map_succeeded: bool, guest_copies_complete: bool) -> bool {
    fully_written && map_succeeded && guest_copies_complete && start_generation == current_generation
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

    #[test]
    fn an_error_retires_idle_and_inflight_leases_but_not_future_clean_work() {
        let mut p = Pool::new();
        let key = Key { size: 256, usage: 9 };
        p.return_unmapped(key, "old idle".to_string());
        p.return_unmapped(key, "old in-flight".to_string());
        let old = p.take(key).unwrap();
        let old_generation = 0;
        p.observe_error_generation(2); // two validation errors, neither cleared
        assert!(p.take(key).is_none());
        assert!(!clean_completion(old_generation, 2, true, true, true));
        drop(old); // the owner drops this lease rather than returning it
        assert_eq!(p.stats().error_invalidations, 1);
        assert_eq!(p.stats().error_invalidated_buffers, 1);
        assert_eq!(p.stats().cached_buffers, 0);
        assert_eq!(p.stats().cached_bytes, 0);
        assert_eq!(p.stats().returned, 2); // cumulative counts were not reset

        // A full overwrite that begins and completes after those old errors
        // is safe to pool; a nonzero process lifetime count is not a ban.
        assert!(clean_completion(2, 2, true, true, true));
        p.return_unmapped(key, "new clean".to_string());
        p.observe_error_generation(2); // no repeated invalidation
        assert_eq!(p.take(key).as_deref(), Some("new clean"));
        assert_eq!(p.stats().error_invalidations, 1);
        assert_eq!(p.stats().reused, 2);
    }

    #[test]
    fn mapping_cpu_copy_and_late_errors_all_prevent_return() {
        assert!(!clean_completion(9, 9, true, false, true)); // failed mapping
        assert!(!clean_completion(9, 9, true, true, false)); // truncated CPU rows
        assert!(!clean_completion(9, 9, false, true, true)); // partial GPU write
        assert!(!clean_completion(9, 10, true, true, true)); // error during copy
        assert!(!clean_completion(0, 1, true, true, true));
        assert!(clean_completion(10, 10, true, true, true));
        assert!(clean_completion(u64::MAX, u64::MAX, true, true, true));
    }

    #[test]
    fn errors_with_multiple_out_of_order_inflight_leases_never_reintroduce_old_idle() {
        let mut p = Pool::new();
        let key = Key { size: 256, usage: 9 };
        let leases = [(0, "very old"), (1, "old"), (2, "current")];
        p.observe_error_generation(2);
        for (generation, bytes) in leases.into_iter().rev() {
            if clean_completion(generation, 2, true, true, true) {
                p.return_unmapped(key, bytes);
            }
        }
        assert_eq!(p.stats().cached_buffers, 1);
        assert_eq!(p.take(key), Some("current"));
        assert!(p.take(key).is_none());
        // An error after a successful return also invalidates that idle entry
        // before the next checkout, without touching any in-flight owner.
        p.return_unmapped(key, "returned before error");
        p.observe_error_generation(3);
        assert!(p.take(key).is_none());
        assert_eq!(p.stats().error_invalidated_buffers, 1);
    }
}
