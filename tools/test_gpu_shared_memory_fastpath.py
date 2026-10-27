#!/usr/bin/env python3
"""Test the production GPU reader against real Wasmtime 49 shared memory.

The small host crate avoids initializing the OHOS GPU backend. It extracts the
self-contained production module, includes the production boundary checker,
and tests concurrent growth as well as exact-check call counts. No device FPS
or Vulkan performance conclusion follows from these checks.
"""
from pathlib import Path
import json
import os
import shutil
import subprocess
import tempfile


TESTS = r'''
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::{AtomicU32, AtomicUsize, Ordering}, Arc, Barrier};
    use wasmtime::{Config, Engine, MemoryType};
    const PAGE: usize = 65536;

    fn memory(memory64: bool) -> SharedMemory {
        let mut config = Config::new();
        config.shared_memory(true);
        let engine = Engine::new(&config).unwrap();
        let ty = MemoryType::builder().min(1).max(Some(64)).shared(true)
            .memory64(memory64).build().unwrap();
        SharedMemory::new(&engine, ty).unwrap()
    }

    #[test]
    fn valid_cached_accesses_never_query_the_locked_size() {
        let bounds = Bounds { confirmed_len: Cell::new(PAGE), fast: true };
        for word in 0..(PAGE / 4) {
            assert_eq!(bounds.span((word * 4) as u64, 4, 4,
                || panic!("hot path queried the exact size")).unwrap(), word * 4..word * 4 + 4);
        }
        let calls = Cell::new(0);
        let mut exact = || { calls.set(calls.get() + 1); 2 * PAGE };
        assert_eq!(bounds.span(PAGE as u64, 4, 4, &mut exact).unwrap(), PAGE..PAGE + 4);
        assert_eq!(calls.get(), 1);
        assert_eq!(bounds.confirmed_len.get(), 2 * PAGE);
        assert!(bounds.span(PAGE as u64 + 4, 4, 4,
            || panic!("refreshed cached range queried size")).is_ok());
    }

    #[test]
    fn stale_zero_length_and_end_boundary_are_rechecked() {
        let bounds = Bounds { confirmed_len: Cell::new(0), fast: true };
        assert_eq!(bounds.span(PAGE as u64, 0, 1, || PAGE).unwrap(), PAGE..PAGE);
        assert_eq!(bounds.span(PAGE as u64 - 4, 4, 4, || panic!("cached end")).unwrap(), PAGE - 4..PAGE);
        let error = bounds.span(PAGE as u64, 1, 1, || PAGE).unwrap_err();
        assert_eq!(error, memory_checks::span(PAGE, PAGE as u64, 1, 1).unwrap_err());
        assert!(bounds.span(PAGE as u64 + 1, 0, 1, || PAGE).is_err());
    }

    #[test]
    fn failure_reports_precise_current_size_after_growth() {
        let bounds = Bounds { confirmed_len: Cell::new(PAGE), fast: true };
        let calls = Cell::new(0);
        let error = bounds.span((2 * PAGE) as u64, 4, 4, || {
            calls.set(calls.get() + 1); 2 * PAGE
        }).unwrap_err();
        assert_eq!(calls.get(), 1);
        assert_eq!(error, memory_checks::span(2 * PAGE, (2 * PAGE) as u64, 4, 4).unwrap_err());
        assert_eq!(bounds.confirmed_len.get(), 2 * PAGE);
    }

    #[test]
    fn alignment_overflow_and_slice_limits_survive_a_large_cache() {
        let cases = [(2, 4, 4), (0, 4, 0), (0, 4, 3),
            (u64::MAX - 3, 8, 1), (0, u64::MAX, 1),
            (0, isize::MAX as u64 + 1, 1)];
        for (addr, size, alignment) in cases {
            let bounds = Bounds { confirmed_len: Cell::new(usize::MAX), fast: true };
            let calls = Cell::new(0);
            assert!(bounds.span(addr, size, alignment, || {
                calls.set(calls.get() + 1); usize::MAX
            }).is_err(), "accepted {addr}:{size}:{alignment}");
            assert_eq!(calls.get(), 1);
        }
        assert!(memory_checks::word(u64::MAX, 1).is_err());
        assert!(memory_checks::word(u64::MAX / 4 + 1, 0).is_err());
    }

    #[test]
    fn memory64_addresses_are_not_truncated_to_u32() {
        if usize::BITS < 64 { return; }
        let address = (1u64 << 32) + 4;
        let total = usize::try_from(address + 4).unwrap();
        let bounds = Bounds { confirmed_len: Cell::new(total), fast: true };
        assert_eq!(bounds.span(address, 4, 4, || panic!("cached memory64 span")).unwrap(),
            address as usize..address as usize + 4);
        let tiny = Bounds { confirmed_len: Cell::new(PAGE), fast: true };
        assert!(tiny.span(address, 4, 4, || PAGE).is_err());
    }

    #[test]
    fn fast_and_exact_modes_have_identical_span_results() {
        let mut random = 0x392d_4401_2d9e_5c19u64;
        for current in [0, 4, PAGE, 2 * PAGE] {
            for cached in [0, current / 2, current] {
                let fast = Bounds { confirmed_len: Cell::new(cached), fast: true };
                let slow = Bounds { confirmed_len: Cell::new(cached), fast: false };
                for _ in 0..4096 {
                    random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let addr = if random & 7 == 0 { u64::MAX - (random & 31) }
                        else { random % (3 * PAGE as u64) };
                    let size = (random >> 17) & 127;
                    let alignment = [0, 1, 2, 3, 4, 8][(random >> 29) as usize % 6];
                    assert_eq!(fast.span(addr, size, alignment, || current),
                        slow.span(addr, size, alignment, || current));
                }
            }
        }
    }

    #[test]
    fn rollback_always_uses_exact_size_even_for_valid_cached_addresses() {
        let bounds = Bounds { confirmed_len: Cell::new(PAGE), fast: false };
        let calls = Cell::new(0);
        for _ in 0..128 {
            assert_eq!(bounds.span(0, 4, 4, || { calls.set(calls.get() + 1); PAGE }).unwrap(), 0..4);
        }
        assert_eq!(calls.get(), 128);
    }

    #[test]
    fn real_reader_preserves_pointer_and_reads_newly_grown_memory() {
        for memory64 in [false, true] {
            for fast in [false, true] {
                let memory = memory(memory64);
                let reader = SharedReader::new(&memory, fast);
                let original = reader.base;
                reader.atomic(0).store(1234, Ordering::SeqCst);
                assert_eq!(reader.word(0, 0, "test initial"), 1234);
                memory.grow(1).unwrap();
                assert_eq!(memory.data().as_ptr() as *mut u8, original);
                let new_word = unsafe { &*(original.add(PAGE) as *const AtomicU32) };
                new_word.store(0x12345678, Ordering::SeqCst);
                assert_eq!(reader.word(PAGE as u64 / 4, 0, "test grown"), 0x12345678);
                assert_eq!(reader.span((2 * PAGE) as u64, 0, 1, "test endpoint"), 2 * PAGE..2 * PAGE);
                drop(memory); // The reader's owning handle still pins memory.
                assert_eq!(reader.word(PAGE as u64 / 4, 0, "test pinned"), 0x12345678);
            }
        }
    }

    #[test]
    fn real_reader_rechecks_concurrent_growth_without_false_rejection() {
        for fast in [false, true] {
            let memory = memory(true);
            let reader = SharedReader::new(&memory, fast);
            let writer = memory.clone();
            let published = Arc::new(AtomicUsize::new(0));
            let marker = published.clone();
            let barrier = Arc::new(Barrier::new(2));
            let start = barrier.clone();
            let thread = std::thread::spawn(move || {
                start.wait();
                for page in 1..32 {
                    writer.grow(1).unwrap();
                    let base = writer.data().as_ptr() as *mut u8;
                    let word = unsafe { &*(base.add(page * PAGE) as *const AtomicU32) };
                    word.store(page as u32, Ordering::Release);
                    marker.store(page, Ordering::Release);
                    std::thread::yield_now();
                }
            });
            barrier.wait();
            let mut last = 0;
            while last < 31 {
                assert_eq!(reader.word(0, 0, "old memory during grow"), 0);
                let page = published.load(Ordering::Acquire);
                if page > last {
                    for n in last + 1..=page {
                        assert_eq!(reader.word((n * PAGE / 4) as u64, 0, "committed concurrent grow"), n as u32);
                    }
                    last = page;
                }
                std::thread::yield_now();
            }
            thread.join().unwrap();
            assert_eq!(reader.span((32 * PAGE) as u64, 0, 1, "grown end"), 32 * PAGE..32 * PAGE);
            assert!(reader.bounds.span((32 * PAGE) as u64, 1, 1, || memory.data_size()).is_err());
        }
    }
}
'''


def main() -> None:
    root = Path(__file__).resolve().parents[1]
    gpu = (root / "game-runtime/src/gpu.rs").read_text()
    begin = gpu.index("mod shared_memory_reader {")
    end = gpu.index("\nuse shared_memory_reader::SharedReader;", begin)
    module = gpu[begin:end]
    assert module.rstrip().endswith("}")
    module = module.rstrip()[:-1] + TESTS + "\n}"
    # Verify that tests exercise the reader actually used by all hot parsers.
    for context in ("exec command field", "draw command field", "readback command field"):
        assert f'reader.word(p, w, "{context}")' in gpu
    assert 'self.shared_reader.word(w, 0, "command u32")' in gpu
    assert 'self.shared_reader.span(command_addr, command_bytes, 4, "ring command payload")' in gpu
    assert 'self.shared_reader.atomic(self.ring + 4 * w)' in gpu
    assert 'var_os("MUGUET_SHARED_MEMORY_FASTPATH")' in gpu
    assert "let memory_len = self.mem.data_size();" not in gpu

    cargo = shutil.which("cargo")
    if cargo is None:
        raise SystemExit("cargo unavailable: source game-runtime/scripts/env.sh first")
    with tempfile.TemporaryDirectory(prefix="gpu-shared-memory-contract-") as temporary:
        project = Path(temporary)
        (project / "src").mkdir()
        (project / "Cargo.toml").write_text('''[package]
name = "gpu-shared-memory-contract"
version = "0.0.0"
edition = "2021"
[dependencies]
wasmtime = { version = "=49.0.0", default-features = false, features = ["std", "runtime", "threads", "anyhow"] }
[workspace]
''')
        helpers = f'''#[path = {json.dumps(str(root / "game-runtime/src/gpu_memory.rs"), ensure_ascii=False)}]
mod memory_checks;
fn memory_violation(context: &str, detail: impl std::fmt::Display) -> ! {{
    panic!("GPU memory access rejected: {{context}}: {{detail}}")
}}
fn checked_value<T>(value: Result<T, String>, context: &str) -> T {{
    value.unwrap_or_else(|error| memory_violation(context, error))
}}
'''
        (project / "src/lib.rs").write_text(helpers + module)
        env = os.environ.copy()
        env["CARGO_TARGET_DIR"] = env.get("GPU_SHARED_MEMORY_TEST_TARGET", "/private/tmp/gpu-shared-memory-contract-target")
        subprocess.run([cargo, "test", "--offline", "--manifest-path", str(project / "Cargo.toml")],
            check=True, env=env, cwd=project)
    print("production parser integration guards and real Wasmtime growth checks passed; no device performance claim")


if __name__ == "__main__":
    main()
