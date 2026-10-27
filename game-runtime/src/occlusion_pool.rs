//! Paired resolve/map leases using the existing bounded idle readback cache.
//! Each entry owns two buffers and is charged for both allocations. The caller
//! retains both handles until map completion, result processing and unmap.
use super::readback_pool;

pub const BUFFER_LIMIT: usize = readback_pool::BUFFER_LIMIT;
pub const BYTE_LIMIT: u64 = readback_pool::BYTE_LIMIT;
const PAIR_LIMIT: usize = BUFFER_LIMIT / 2;

pub struct Pair<T> { pub resolve: T, pub map: T }
pub struct Pool<T> { idle: readback_pool::Pool<Pair<T>>, pair_limit_rejections: u64 }

impl<T> Pool<T> {
    pub fn new() -> Self { Self { idle: readback_pool::Pool::new(), pair_limit_rejections: 0 } }

    fn key(size: u64) -> Option<readback_pool::Key> {
        // All pairs have fixed resolve/map usages checked by the GPU caller.
        // `size` is the exact allocation of EACH buffer, with no bucket tail.
        (size != 0).then_some(())?;
        Some(readback_pool::Key { size: size.checked_mul(2)?, usage: 0 })
    }

    pub fn take(&mut self, size: u64, generation: u64) -> Option<Pair<T>> {
        self.idle.observe_error_generation(generation);
        self.idle.take(Self::key(size)?)
    }

    pub fn created(&mut self) { self.idle.created(true); }

    pub fn invalidate_failed_map(&mut self) { self.idle.clear_idle(); }

    pub fn return_completed(&mut self, size: u64, pair: Pair<T>, start_generation: u64,
        current_generation: u64, fully_written: bool, results_complete: bool) {
        self.idle.observe_error_generation(current_generation);
        if !readback_pool::clean_completion(start_generation, current_generation,
            fully_written, true, results_complete) { return; }
        if self.idle.stats().cached_buffers >= PAIR_LIMIT {
            self.pair_limit_rejections = self.pair_limit_rejections.saturating_add(1);
            return;
        }
        // Invalid/overflowing sizes take the existing cache's rejection path.
        let key = Self::key(size).unwrap_or(readback_pool::Key { size: 0, usage: 0 });
        self.idle.return_unmapped(key, pair);
    }

    /// Created/reused/returned count PAIRS; capacity and invalidation counters
    /// count individual BUFFERS, and cached bytes include both allocations.
    pub fn stats(&self) -> readback_pool::Stats {
        let mut stats = self.idle.stats();
        stats.cached_buffers *= 2;
        stats.peak_buffers *= 2;
        stats.error_invalidated_buffers = stats.error_invalidated_buffers.saturating_mul(2);
        stats.rejected = stats.rejected.saturating_add(self.pair_limit_rejections);
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

    struct Buffer { id: usize, drops: Arc<AtomicUsize> }
    impl Drop for Buffer { fn drop(&mut self) { self.drops.fetch_add(1, Ordering::Relaxed); } }
    fn pair(id: usize, drops: &Arc<AtomicUsize>) -> Pair<Buffer> {
        Pair { resolve: Buffer { id: id * 2, drops: drops.clone() }, map: Buffer { id: id * 2 + 1, drops: drops.clone() } }
    }

    #[test]
    fn paired_checkout_is_exact_and_removes_both_buffers_from_idle() {
        let drops = Arc::new(AtomicUsize::new(0)); let mut pool = Pool::new();
        pool.return_completed(256, pair(0, &drops), 0, 0, true, true);
        assert_eq!(pool.stats().cached_buffers, 2);
        assert_eq!(pool.stats().cached_bytes, 512);
        assert!(pool.take(512, 0).is_none());
        let lease = pool.take(256, 0).unwrap();
        assert_eq!((lease.resolve.id, lease.map.id), (0, 1));
        assert!(pool.take(256, 0).is_none());
        assert_eq!(pool.stats().cached_buffers, 0);
        assert_eq!(drops.load(Ordering::Relaxed), 0); // Pending owner pins BOTH.
        pool.return_completed(256, lease, 0, 0, true, true);
        drop(pool.take(256, 0));
        assert_eq!(drops.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn sixteen_buffer_and_sixteen_megabyte_limits_include_both_members() {
        let drops = Arc::new(AtomicUsize::new(0)); let mut pool = Pool::new();
        for i in 1..=20 { pool.return_completed(i as u64 * 256, pair(i, &drops), 0, 0, true, true); }
        assert_eq!(pool.stats().cached_buffers, BUFFER_LIMIT);
        assert_eq!(pool.stats().peak_buffers, BUFFER_LIMIT);
        assert_eq!(pool.stats().rejected, 12);
        assert_eq!(drops.load(Ordering::Relaxed), 24);
        let mut large = Pool::new();
        large.return_completed(BYTE_LIMIT / 2, pair(21, &drops), 0, 0, true, true);
        large.return_completed(256, pair(22, &drops), 0, 0, true, true);
        large.return_completed(u64::MAX, pair(23, &drops), 0, 0, true, true);
        large.return_completed(0, pair(24, &drops), 0, 0, true, true);
        assert_eq!(large.stats().cached_bytes, BYTE_LIMIT);
        assert_eq!(large.stats().cached_buffers, 2);
        assert_eq!(large.stats().rejected, 3);
        assert_eq!(large.stats().peak_bytes, BYTE_LIMIT);
    }

    #[test]
    fn per_size_limit_uses_the_existing_generic_cache() {
        let drops = Arc::new(AtomicUsize::new(0)); let mut pool = Pool::new();
        for i in 0..8 { pool.return_completed(256, pair(i, &drops), 4, 4, true, true); }
        assert_eq!(pool.stats().cached_buffers, readback_pool::PER_KEY_LIMIT * 2);
        assert_eq!(pool.stats().rejected, 4);
        assert_eq!(drops.load(Ordering::Relaxed), 8);
    }

    #[test]
    fn incomplete_copy_or_result_processing_cannot_return_a_lease() {
        let drops = Arc::new(AtomicUsize::new(0)); let mut pool = Pool::new();
        pool.return_completed(256, pair(0, &drops), 0, 0, false, true);
        pool.return_completed(256, pair(1, &drops), 0, 0, true, false);
        assert_eq!(pool.stats().cached_buffers, 0);
        assert_eq!(drops.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn error_and_map_failure_drop_idle_pairs_without_releasing_pending_owners() {
        let drops = Arc::new(AtomicUsize::new(0)); let mut pool = Pool::new();
        pool.return_completed(256, pair(0, &drops), 0, 0, true, true);
        pool.return_completed(256, pair(1, &drops), 0, 0, true, true);
        let pending = pool.take(256, 0).unwrap();
        assert!(pool.take(512, 1).is_none()); // new error invalidates idle only
        assert_eq!(drops.load(Ordering::Relaxed), 2);
        pool.return_completed(256, pending, 0, 1, true, true);
        assert_eq!(drops.load(Ordering::Relaxed), 4);
        assert_eq!(pool.stats().cached_buffers, 0);
        assert_eq!(pool.stats().error_invalidations, 1);
        assert_eq!(pool.stats().error_invalidated_buffers, 2);
        pool.return_completed(256, pair(2, &drops), 1, 1, true, true);
        pool.invalidate_failed_map(); // failed map may not increment error count
        assert_eq!(pool.stats().cached_buffers, 0);
        assert_eq!(drops.load(Ordering::Relaxed), 6);
        pool.return_completed(256, pair(3, &drops), 1, 1, true, true);
        assert!(pool.take(256, 1).is_some()); // later clean generation is usable
    }

    #[test]
    fn out_of_order_old_completion_never_reintroduces_a_retired_pair() {
        let drops = Arc::new(AtomicUsize::new(0)); let mut pool = Pool::new();
        for (id, started) in [(0, 2), (1, 0), (2, 1)] {
            pool.return_completed(256, pair(id, &drops), started, 2, true, true);
        }
        assert_eq!(pool.stats().cached_buffers, 2);
        assert_eq!(pool.take(256, 2).unwrap().resolve.id, 0);
        assert!(pool.take(256, 2).is_none());
        assert_eq!(drops.load(Ordering::Relaxed), 6);
    }
}
