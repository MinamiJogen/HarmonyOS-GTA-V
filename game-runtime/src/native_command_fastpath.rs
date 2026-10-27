//! Opt-in host allocation/diagnostic guards. No GPU calls or changed pass fields.
use std::ops::Deref;
use std::sync::OnceLock;

pub(crate) const RENDER_TARGETS: usize = 8;
pub(crate) const PASS_WORDS: usize = RENDER_TARGETS * 4 + 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StartupOptions {
    pub enabled: bool,
    pub vertex_diagnostics: bool,
}

impl StartupOptions {
    pub fn requested() -> Self {
        static OPTIONS: OnceLock<StartupOptions> = OnceLock::new();
        *OPTIONS.get_or_init(|| Self {
            enabled: std::env::var_os("MUGUET_NATIVE_COMMAND_FASTPATH").is_some_and(|v| v == "1"),
            vertex_diagnostics: std::env::var_os("MUGUET_GPU_DIAGNOSTICS").is_some_and(|v| v == "1"),
        })
    }

    /// Feature OFF always enters the original trace function. When ON, only a
    /// diagnostics-OFF startup can bypass it; diagnostics ON uses the old trace.
    pub fn should_trace_vertex_bind(self) -> bool {
        !self.enabled || self.vertex_diagnostics
    }
}

#[derive(Debug)]
pub(crate) enum PassKey {
    Inline([u32; PASS_WORDS]),
    Heap(Vec<u32>),
}

#[derive(Default)]
pub(crate) struct Counters {
    pub inline_pass_keys: u64,
    pub heap_pass_keys: u64,
    pub pass_key_requested_bytes_avoided: u64,
    pub geometry_clones_avoided: u64,
    pub disabled_vertex_trace_calls_avoided: u64,
}

impl PassKey {
    /// Preserve the exact original ordering: four words per target, then all
    /// five depth/stencil words. Unexpected target counts retain the heap path.
    pub fn new(enabled: bool, targets: &[[u32; 4]], depth: [u32; 5]) -> Self {
        if enabled && targets.len() == RENDER_TARGETS {
            let mut words = [0; PASS_WORDS];
            for (output, target) in words[..RENDER_TARGETS * 4].chunks_exact_mut(4).zip(targets) {
                output.copy_from_slice(target);
            }
            words[RENDER_TARGETS * 4..].copy_from_slice(&depth);
            Self::Inline(words)
        } else {
            let mut words = Vec::with_capacity(targets.len().saturating_mul(4).saturating_add(5));
            for target in targets { words.extend_from_slice(target); }
            words.extend_from_slice(&depth);
            Self::Heap(words)
        }
    }

    pub fn as_slice(&self) -> &[u32] {
        match self { Self::Inline(words) => words, Self::Heap(words) => words }
    }
}

impl Deref for PassKey {
    type Target = [u32];
    fn deref(&self) -> &Self::Target { self.as_slice() }
}
impl PartialEq for PassKey {
    fn eq(&self, other: &Self) -> bool { self.as_slice() == other.as_slice() }
}
impl Eq for PassKey {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    #[derive(Clone, Copy, Default)]
    struct Allocations { calls: usize, bytes: usize }
    thread_local! {
        static ALLOCATIONS: Cell<Option<Allocations>> = const { Cell::new(None) };
    }
    struct MeasuredAllocator;
    #[global_allocator]
    static ALLOCATOR: MeasuredAllocator = MeasuredAllocator;
    unsafe impl GlobalAlloc for MeasuredAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let _ = ALLOCATIONS.try_with(|state| if let Some(mut count) = state.get() {
                count.calls += 1; count.bytes += layout.size(); state.set(Some(count));
            });
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) { unsafe { System.dealloc(ptr, layout) } }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let _ = ALLOCATIONS.try_with(|state| if let Some(mut count) = state.get() {
                count.calls += 1; count.bytes += new_size; state.set(Some(count));
            });
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }

    fn measured<T>(work: impl FnOnce() -> T) -> (T, Allocations) {
        ALLOCATIONS.with(|state| { assert!(state.get().is_none()); state.set(Some(Allocations::default())); });
        let value = work();
        let count = ALLOCATIONS.with(|state| state.take().unwrap());
        (value, count)
    }
    fn targets() -> [[u32; 4]; 8] {
        std::array::from_fn(|i| std::array::from_fn(|j| (i * 4 + j) as u32))
    }
    fn depth() -> [u32; 5] { [32, 33, 34, 35, 36] }
    fn legacy(targets: &[[u32; 4]], depth: [u32; 5]) -> Vec<u32> {
        let mut words = Vec::with_capacity(37);
        for target in targets { words.extend(*target); }
        words.extend(depth); words
    }

    #[test]
    fn eight_targets_keep_all_37_words_and_old_indexing() {
        let key = PassKey::new(true, &targets(), depth());
        assert!(matches!(key, PassKey::Inline(_)));
        assert_eq!(key.as_slice(), &(0..37).collect::<Vec<_>>());
        assert_eq!(key.len(), 37);
        for i in 0..9 { assert_eq!(key[i * 4], (i * 4) as u32); }
    }

    #[test]
    fn each_target_mip_layer_format_and_all_depth_fields_change_equality() {
        let base = PassKey::new(true, &targets(), depth());
        for i in 0..PASS_WORDS {
            let (mut targets, mut depth) = (targets(), depth());
            if i < 32 { targets[i / 4][i % 4] ^= 0x8000_0000; }
            else { depth[i - 32] ^= 0x8000_0000; }
            let changed = PassKey::new(true, &targets, depth);
            assert_ne!(base, changed, "pass word {i} must remain significant");
            assert_eq!(base.iter().zip(changed.iter()).filter(|(a,b)| a != b).count(), 1);
        }
    }

    #[test]
    fn off_and_on_compare_equally_to_reference_and_each_other() {
        for value in [0, 1, u32::MAX, 0x8000_0000] {
            let targets = [[value; 4]; 8]; let depth = [value.wrapping_add(1); 5];
            let reference = legacy(&targets, depth);
            let off = PassKey::new(false, &targets, depth);
            let on = PassKey::new(true, &targets, depth);
            assert!(matches!(off, PassKey::Heap(_)));
            assert_eq!(off.as_slice(), reference); assert_eq!(on.as_slice(), reference);
            assert_eq!(off, on);
        }
    }

    #[test]
    fn unexpected_target_counts_use_full_heap_fallback_without_truncation() {
        for n in [0, 1, 7, 9, 16, 257] {
            let targets: Vec<_> = (0..n).map(|i| [i as u32, 17, u32::MAX, 0]).collect();
            for enabled in [false, true] {
                let key = PassKey::new(enabled, &targets, depth());
                assert!(matches!(key, PassKey::Heap(_)));
                assert_eq!(key.as_slice(), legacy(&targets, depth()));
                assert_eq!(key.len(), n * 4 + 5);
                assert_eq!(&key[n * 4..], &depth());
            }
        }
    }

    #[test]
    fn actual_inline_constructor_allocates_zero_and_off_requests_148_bytes() {
        let targets = targets(); let depth = depth();
        let (inline, a) = measured(|| PassKey::new(true, &targets, depth));
        let (heap, b) = measured(|| PassKey::new(false, &targets, depth));
        assert_eq!((a.calls, a.bytes), (0, 0));
        assert_eq!((b.calls, b.bytes), (1, 148));
        assert_eq!(inline, heap);
    }

    #[test]
    fn startup_guard_keeps_reference_trace_and_diagnostic_on_path() {
        for enabled in [false, true] {
            for vertex_diagnostics in [false, true] {
                let options = StartupOptions { enabled, vertex_diagnostics };
                assert_eq!(options.should_trace_vertex_bind(), !enabled || vertex_diagnostics);
            }
        }
    }
}
