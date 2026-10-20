#!/usr/bin/env python3
"""Exercise production readback epochs and render-attachment admission on host.

This uses the actual Pool implementation and extracts render_target_format from
gpu.rs into a small fake-adapter harness. It does not emulate GPU execution or
establish that an unsupported format was rendered successfully.
"""
from pathlib import Path
import os
import shutil
import subprocess
import tempfile


def function(source: str, name: str) -> str:
    begin = source.index(f"    fn {name}(")
    opening = source.index("{", begin)
    level = 1
    cursor = opening + 1
    # These audited functions have balanced braces in comments/string literals.
    while level:
        level += (source[cursor] == "{") - (source[cursor] == "}")
        cursor += 1
    return source[begin:cursor]


def main() -> None:
    root = Path(__file__).resolve().parents[1]
    gpu = (root / "game-runtime/src/gpu.rs").read_text()
    pool = root / "game-runtime/src/readback_pool.rs"
    rustc = shutil.which("rustc")
    if rustc is None:
        raise SystemExit("rustc unavailable: source game-runtime/scripts/env.sh first")

    # Integration guards against accidentally restoring the old lifetime ban,
    # testing only a helper while omitting it from either readback producer, or
    # bypassing admission through cached views / a partial MRT render pass.
    readback = function(gpu, "readback")
    assert "self.errors.load(Ordering::Relaxed) == 0" not in readback
    assert readback.count("let error_generation = self.errors.load(Ordering::Acquire);") == 2
    assert readback.count("recycle, error_generation }") == 2
    checkout = function(gpu, "readback_buffer")
    assert checkout.index("observe_error_generation(error_generation)") < checkout.index("pool.take(")
    completion = function(gpu, "finish_ready_maps")
    assert completion.index("pm.failed.load(") < completion.index("get_mapped_range()")
    assert completion.index("pm.buf.unmap()") < completion.index("readback_pool::clean_completion(")
    assert completion.index("observe_error_generation(current_generation)") < completion.index("pool.return_unmapped(")
    target = function(gpu, "target_view")
    assert target.index("render_target_format(") < target.index("t.views.get(")
    assert "None => return false" in function(gpu, "ensure_pass")
    assert "} else { return false; }" in function(gpu, "ensure_pass")
    for name in ("clear_view", "resolve", "generate_mips"):
        assert "render_target_format(" in function(gpu, name)

    adapter_test = r'''
use std::sync::atomic::{AtomicU64, Ordering};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum TextureFormat { Rgba8Unorm, Rgba8Snorm, Rgba16Float, Depth32Float }
#[derive(Clone, Copy, Debug)]
struct TextureUsages(u32);
impl TextureUsages {
    const RENDER_ATTACHMENT: Self = Self(16);
    fn contains(self, bit: Self) -> bool { self.0 & bit.0 == bit.0 }
}
struct Features { allowed_usages: TextureUsages }
struct Adapter { snorm_rt: bool, queries: Cell<usize> }
impl Adapter {
    fn get_texture_format_features(&self, fmt: TextureFormat) -> Features {
        self.queries.set(self.queries.get() + 1);
        let render = fmt != TextureFormat::Rgba8Snorm || self.snorm_rt;
        Features { allowed_usages: TextureUsages(if render { 31 } else { 15 }) }
    }
}
struct Texture { usage: TextureUsages }
impl Texture { fn usage(&self) -> TextureUsages { self.usage } }
struct Tex { tex: Texture, format: TextureFormat, half: usize }
struct Gpu { tex: Tex, adapter: Adapter, render_attachment_usages: RefCell<HashMap<TextureFormat, TextureUsages>> }
struct Format { format: TextureFormat }
fn map_format(dxgi: u32, _: u32) -> Format {
    Format { format: match dxgi {31 => TextureFormat::Rgba8Snorm, 10 => TextureFormat::Rgba16Float,
        40 => TextureFormat::Depth32Float, _ => TextureFormat::Rgba8Unorm} }
}
fn is_depth(f: TextureFormat) -> bool { f == TextureFormat::Depth32Float }
fn log_once(_: &str, _: &str) {}
static UNSUPPORTED_RENDER_ATTACHMENTS: AtomicU64 = AtomicU64::new(0);
impl Gpu {
    fn tex(&self, id: u32) -> Option<&Tex> { (id == 1).then_some(&self.tex) }
RENDER_TARGET_FORMAT
}
fn gpu(format: TextureFormat, usage: u32, supported: bool, half: usize) -> Gpu {
    Gpu { tex: Tex { tex: Texture {usage: TextureUsages(usage)}, format, half },
        adapter: Adapter {snorm_rt: supported, queries: Cell::new(0)}, render_attachment_usages: RefCell::new(HashMap::new()) }
}
#[test]
fn snorm_sampling_storage_does_not_imply_renderability() {
    assert!(gpu(TextureFormat::Rgba8Snorm, 15, false, 0).render_target_format(1, 31).is_none());
    assert!(gpu(TextureFormat::Rgba8Snorm, 31, false, 0).render_target_format(1, 31).is_none());
}
#[test]
fn adapter_support_does_not_repair_a_texture_created_without_render_usage() {
    assert!(gpu(TextureFormat::Rgba8Snorm, 15, true, 0).render_target_format(1, 31).is_none());
    assert_eq!(gpu(TextureFormat::Rgba8Snorm, 31, true, 0).render_target_format(1, 31), Some(TextureFormat::Rgba8Snorm));
}
#[test]
fn ordinary_targets_and_existing_depth_half_overrides_keep_their_actual_formats() {
    assert_eq!(gpu(TextureFormat::Rgba8Unorm, 31, false, 0).render_target_format(1, 0), Some(TextureFormat::Rgba8Unorm));
    assert_eq!(gpu(TextureFormat::Rgba16Float, 31, false, 4).render_target_format(1, 31), Some(TextureFormat::Rgba16Float));
    assert_eq!(gpu(TextureFormat::Depth32Float, 31, false, 0).render_target_format(1, 31), Some(TextureFormat::Depth32Float));
    assert!(gpu(TextureFormat::Rgba8Unorm, 31, false, 0).render_target_format(99, 0).is_none());
}
#[test]
fn repeated_targets_query_the_driver_only_once_per_format() {
    let mut g = gpu(TextureFormat::Rgba8Unorm, 31, false, 0);
    for _ in 0..100 { assert!(g.render_target_format(1, 0).is_some()); }
    assert_eq!(g.adapter.queries.get(), 1);
    g.tex.format = TextureFormat::Rgba8Snorm;
    for _ in 0..100 { assert!(g.render_target_format(1, 0).is_none()); }
    assert_eq!(g.adapter.queries.get(), 2); // rejected formats are cached too
    g.tex.format = TextureFormat::Rgba8Unorm;
    assert!(g.render_target_format(1, 0).is_some());
    assert_eq!(g.adapter.queries.get(), 2);
}
'''.replace("RENDER_TARGET_FORMAT", function(gpu, "render_target_format"))
    with tempfile.TemporaryDirectory(prefix="gpu-readback-epochs-") as temporary:
        for name, text in (("pool", pool.read_text()), ("attachment", adapter_test)):
            source = Path(temporary) / f"{name}.rs"
            binary = Path(temporary) / f"{name}-test"
            source.write_text(text)
            subprocess.run([rustc, "--edition=2021", "--test", str(source), "-o", str(binary)], check=True)
            subprocess.run([str(binary)], check=True, env=os.environ.copy())
    print("production integration guards passed; no GPU/device rendering claim")


if __name__ == "__main__":
    main()
