//! A bounded, single-owner cache for the native resource I/O worker.
//!
//! Values are borrowed in place. Only the worker owns this cache, so no lock or
//! worker is added. Opening before LRU eviction keeps useful handles on failure.
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EvictionPolicy {
    ClearAll,
    Lru,
}

struct Entry<T> {
    value: T,
    last_used: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CacheStats {
    pub(super) hits: u64,
    pub(super) misses: u64,
    pub(super) open_attempts: u64,
    pub(super) evictions: u64,
    pub(super) clear_all: u64,
    pub(super) fd_pressure_retries: u64,
    pub(super) peak_entries: usize,
}

pub(super) struct HandleCache<T> {
    entries: HashMap<i32, Entry<T>>,
    capacity: usize,
    policy: EvictionPolicy,
    clock: u64,
    stats: CacheStats,
}

impl<T> HandleCache<T> {
    pub(super) fn new(capacity: usize, policy: EvictionPolicy) -> Self {
        assert!(capacity > 0, "I/O handle capacity must be positive");
        Self { entries: HashMap::new(), capacity, policy, clock: 0, stats: CacheStats::default() }
    }

    pub(super) fn stats(&self) -> CacheStats { self.stats }

    fn stamp(&mut self) -> u64 {
        if self.policy == EvictionPolicy::ClearAll { return 0; }
        if self.clock == u64::MAX {
            // Preserve relative recency on the practically unreachable wrap.
            let mut order: Vec<_> = self.entries.iter()
                .map(|(&id, entry)| (entry.last_used, id)).collect();
            order.sort_unstable();
            for (rank, (_, id)) in order.into_iter().enumerate() {
                self.entries.get_mut(&id).unwrap().last_used = rank as u64 + 1;
            }
            self.clock = self.entries.len() as u64;
        }
        self.clock += 1;
        self.clock
    }

    fn evict_lru(&mut self) {
        let victim = self.entries.iter().min_by_key(|(&id, entry)| (entry.last_used, id))
            .map(|(&id, _)| id).unwrap();
        self.entries.remove(&victim);
        self.stats.evictions = self.stats.evictions.saturating_add(1);
    }

    #[cfg(test)]
    fn get_or_try_insert_with<E>(
        &mut self, id: i32, open: impl FnMut() -> Result<T, E>,
    ) -> Result<&T, E> {
        self.get_or_try_insert_with_resource_retry(id, open, |_| false)
    }

    pub(super) fn get_or_try_insert_with_resource_retry<E>(
        &mut self, id: i32, mut open: impl FnMut() -> Result<T, E>,
        resource_pressure: impl FnOnce(&E) -> bool,
    ) -> Result<&T, E> {
        if self.entries.contains_key(&id) {
            self.stats.hits = self.stats.hits.saturating_add(1);
            let stamp = self.stamp();
            let entry = self.entries.get_mut(&id).unwrap();
            entry.last_used = stamp;
            return Ok(&entry.value);
        }
        self.stats.misses = self.stats.misses.saturating_add(1);
        if self.policy == EvictionPolicy::ClearAll && self.entries.len() >= self.capacity {
            // Keep the original default policy, including on failed opens.
            self.entries.clear();
            self.stats.clear_all = self.stats.clear_all.saturating_add(1);
        }
        self.stats.open_attempts = self.stats.open_attempts.saturating_add(1);
        let value = match open() {
            Ok(value) => value,
            Err(error) if self.policy == EvictionPolicy::Lru && !self.entries.is_empty()
                && resource_pressure(&error) => {
                // EMFILE/ENFILE may prevent an otherwise valid open while our
                // cache retains descriptors. Release just the coldest one and
                // retry once; a second failure leaves other handles intact.
                self.evict_lru();
                self.stats.fd_pressure_retries = self.stats.fd_pressure_retries.saturating_add(1);
                self.stats.open_attempts = self.stats.open_attempts.saturating_add(1);
                open()?
            }
            Err(error) => return Err(error),
        };
        if self.entries.len() >= self.capacity {
            self.evict_lru();
        }
        let stamp = self.stamp();
        self.entries.insert(id, Entry { value, last_used: stamp });
        self.stats.peak_entries = self.stats.peak_entries.max(self.entries.len());
        Ok(&self.entries.get(&id).unwrap().value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    fn add(cache: &mut HandleCache<i32>, id: i32) {
        assert_eq!(*cache.get_or_try_insert_with(id, || Ok::<_, ()>(id)).unwrap(), id);
    }

    #[test]
    fn lru_keeps_hot_handle_and_reopens_evicted_handle_only() {
        let mut cache = HandleCache::new(2, EvictionPolicy::Lru);
        add(&mut cache, 1); add(&mut cache, 2);
        assert_eq!(*cache.get_or_try_insert_with(1, || Err(())).unwrap(), 1);
        add(&mut cache, 3);
        assert!(!cache.entries.contains_key(&2));
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(*cache.get_or_try_insert_with(1, || Err(())).unwrap(), 1);
        add(&mut cache, 2);
        assert!(!cache.entries.contains_key(&3));
    }

    #[test]
    fn failed_lru_open_preserves_entries_recency_and_capacity() {
        let mut cache = HandleCache::new(2, EvictionPolicy::Lru);
        add(&mut cache, 1); add(&mut cache, 2);
        let clock = cache.clock;
        assert_eq!(cache.get_or_try_insert_with(3, || Err("open failed")), Err("open failed"));
        assert_eq!(cache.clock, clock);
        assert_eq!(cache.entries.len(), 2);
        add(&mut cache, 3);
        assert!(!cache.entries.contains_key(&1));
        assert!(cache.entries.contains_key(&2));
    }

    #[test]
    fn legacy_clear_all_policy_remains_default_comparable() {
        let mut cache = HandleCache::new(2, EvictionPolicy::ClearAll);
        add(&mut cache, 1); add(&mut cache, 2);
        add(&mut cache, 1); add(&mut cache, 3);
        assert_eq!(cache.entries.len(), 1);
        assert!(cache.entries.contains_key(&3));
        add(&mut cache, 4);
        assert_eq!(cache.get_or_try_insert_with(5, || Err("failed")), Err("failed"));
        assert!(cache.entries.is_empty());
        assert_eq!(cache.clock, 0);
    }

    #[test]
    fn capacity_never_exceeded_including_single_entry_and_1024() {
        for capacity in [1, 2, 1024] {
            let mut cache = HandleCache::new(capacity, EvictionPolicy::Lru);
            for id in 0..2050 {
                add(&mut cache, id);
                assert_eq!(cache.entries.len(), ((id + 1) as usize).min(capacity));
            }
        }
    }

    #[test]
    fn eviction_and_cache_drop_release_exact_owned_values() {
        struct Tracked(Rc<Cell<usize>>);
        impl Drop for Tracked { fn drop(&mut self) { self.0.set(self.0.get() + 1); } }
        let drops = Rc::new(Cell::new(0));
        {
            let mut cache = HandleCache::new(2, EvictionPolicy::Lru);
            for id in [1, 2, 1, 3] {
                cache.get_or_try_insert_with(id, || Ok::<_, ()>(Tracked(drops.clone()))).unwrap();
            }
            assert_eq!(drops.get(), 1);
            assert!(cache.get_or_try_insert_with(4, || Err(())).is_err());
            assert_eq!(drops.get(), 1);
        }
        assert_eq!(drops.get(), 3);
    }

    #[test]
    fn actual_file_borrow_reuses_open_handle_after_path_unlink_and_failure() {
        use std::os::unix::fs::FileExt;
        use std::os::fd::AsRawFd;
        let path = std::env::temp_dir().join(format!("muguet-io-lru-test-{}", std::process::id()));
        std::fs::write(&path, b"original").unwrap();
        let mut cache = HandleCache::new(1, EvictionPolicy::Lru);
        let original_fd = cache.get_or_try_insert_with(1, || std::fs::File::open(&path)).unwrap().as_raw_fd();
        std::fs::remove_file(&path).unwrap();
        assert!(cache.get_or_try_insert_with(2, || std::fs::File::open(&path)).is_err());
        let file = cache.get_or_try_insert_with(1, || std::fs::File::open(&path)).unwrap();
        assert_eq!(file.as_raw_fd(), original_fd);
        let mut bytes = [0u8; 8];
        file.read_exact_at(&mut bytes, 0).unwrap();
        assert_eq!(&bytes, b"original");
    }

    #[test]
    fn recency_counter_wrap_preserves_order() {
        let mut cache = HandleCache::new(2, EvictionPolicy::Lru);
        add(&mut cache, 1); add(&mut cache, 2);
        cache.clock = u64::MAX;
        add(&mut cache, 1); add(&mut cache, 3);
        assert!(cache.entries.contains_key(&1));
        assert!(!cache.entries.contains_key(&2));
        assert_eq!(cache.entries.len(), 2);
    }

    #[test]
    fn descriptor_pressure_evicts_only_lru_and_retries_once() {
        for code in [23, 24] { // POSIX ENFILE/EMFILE supplied by the File caller.
            let mut cache = HandleCache::new(2, EvictionPolicy::Lru);
            add(&mut cache, 1); add(&mut cache, 2); add(&mut cache, 1);
            let mut attempts = 0;
            let result = cache.get_or_try_insert_with_resource_retry(3, || {
                attempts += 1;
                if attempts == 1 { Err(std::io::Error::from_raw_os_error(code)) } else { Ok(3) }
            }, |error| matches!(error.raw_os_error(), Some(23 | 24)));
            assert_eq!(*result.unwrap(), 3);
            assert_eq!(attempts, 2);
            assert!(cache.entries.contains_key(&1));
            assert!(!cache.entries.contains_key(&2));
            assert_eq!(cache.entries.len(), 2);
        }
    }

    #[test]
    fn resource_retry_failure_is_bounded_and_ordinary_errors_never_evict() {
        for code in [2, 13, 23, 24] {
            let mut cache = HandleCache::new(2, EvictionPolicy::Lru);
            add(&mut cache, 1); add(&mut cache, 2); add(&mut cache, 1);
            let mut attempts = 0;
            let result = cache.get_or_try_insert_with_resource_retry(3, || {
                attempts += 1;
                Err(std::io::Error::from_raw_os_error(code))
            }, |error| matches!(error.raw_os_error(), Some(23 | 24)));
            assert_eq!(result.unwrap_err().raw_os_error(), Some(code));
            let pressure = code == 23 || code == 24;
            assert_eq!(attempts, if pressure { 2 } else { 1 });
            assert_eq!(cache.entries.len(), if pressure { 1 } else { 2 });
            assert!(cache.entries.contains_key(&1));
            assert_eq!(cache.entries.contains_key(&2), !pressure);
        }
        let mut empty = HandleCache::<i32>::new(2, EvictionPolicy::Lru);
        let mut attempts = 0;
        assert!(empty.get_or_try_insert_with_resource_retry(1, || {
            attempts += 1; Err(24)
        }, |_| true).is_err());
        assert_eq!(attempts, 1);
    }

    #[test]
    fn owner_local_counters_distinguish_hits_pressure_eviction_and_legacy_clear() {
        let mut cache = HandleCache::new(2, EvictionPolicy::Lru);
        add(&mut cache, 1); add(&mut cache, 2); add(&mut cache, 1); add(&mut cache, 3);
        let mut attempts = 0;
        cache.get_or_try_insert_with_resource_retry(4, || {
            attempts += 1;
            if attempts == 1 { Err(24) } else { Ok(4) }
        }, |code| *code == 24).unwrap();
        assert!(cache.get_or_try_insert_with(5, || Err(2)).is_err());
        assert_eq!(cache.stats(), CacheStats { hits: 1, misses: 5, open_attempts: 6,
            evictions: 2, clear_all: 0, fd_pressure_retries: 1, peak_entries: 2 });
        let mut legacy = HandleCache::new(2, EvictionPolicy::ClearAll);
        add(&mut legacy, 1); add(&mut legacy, 2); add(&mut legacy, 3); add(&mut legacy, 4);
        assert!(legacy.get_or_try_insert_with(5, || Err(2)).is_err());
        assert_eq!(legacy.stats(), CacheStats { hits: 0, misses: 5, open_attempts: 5,
            evictions: 0, clear_all: 2, fd_pressure_retries: 0, peak_entries: 2 });
    }

    #[test]
    #[should_panic(expected = "capacity must be positive")]
    fn zero_capacity_is_rejected() { let _ = HandleCache::<i32>::new(0, EvictionPolicy::Lru); }
}
