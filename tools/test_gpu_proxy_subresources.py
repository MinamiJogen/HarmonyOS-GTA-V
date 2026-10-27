#!/usr/bin/env python3
"""Production view-routing/dirty-state differential model; no GPU pixel claim.

Uses the real range/dirty implementation and extracts production prepare,
srv_view and invalidation methods. Fake proxy passes copy deterministic
per-subresource values, isolating view routing, freshness and work selection
from the unchanged WGPU conversion shaders.
"""
from pathlib import Path
import json
import shutil
import subprocess
import tempfile


def function(source: str, name: str) -> str:
    begin = source.index(f"    fn {name}(")
    cursor = source.index("{", begin) + 1
    level = 1
    while level:
        level += (source[cursor] == "{") - (source[cursor] == "}")
        cursor += 1
    return source[begin:cursor]


HARNESS = r'''
use std::{collections::{HashMap, HashSet}, sync::atomic::{AtomicBool, Ordering}};
use proxy_subresources::{Dirty, Span};
#[derive(Clone, Copy, Debug, PartialEq, Eq)] enum TextureDimension { D1, D2, D3 }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] enum TextureViewDimension { D1, D2, D2Array, Cube, CubeArray, D3 }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] enum AstcBlock { B4x4 }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] enum AstcChannel { Unorm, UnormSrgb }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] enum TextureFormat {
    Depth, R32Float, R16Float, Rg8Uint, Rgba, Astc { block: AstcBlock, channel: AstcChannel }
}
impl TextureFormat {
    fn is_srgb(self) -> bool { false }
    fn remove_srgb_suffix(self) -> Self { self }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)] enum TextureAspect { All, DepthOnly, StencilOnly }
#[derive(Clone, Debug, PartialEq, Eq)] struct TextureViewDescriptor {
    dimension: Option<TextureViewDimension>, base_mip_level: u32, mip_level_count: Option<u32>,
    base_array_layer: u32, array_layer_count: Option<u32>, format: Option<TextureFormat>, aspect: TextureAspect,
}
impl Default for TextureViewDescriptor {
    fn default() -> Self { Self { dimension: None, base_mip_level: 0, mip_level_count: None,
        base_array_layer: 0, array_layer_count: None, format: None, aspect: TextureAspect::All } }
}
#[derive(Clone)] struct Texture { id: u32 }
impl Texture {
    fn create_view(&self, desc: &TextureViewDescriptor) -> TextureView { TextureView { texture: self.id, desc: desc.clone() } }
}
#[derive(Clone)] struct TextureView { texture: u32, desc: TextureViewDescriptor }
#[derive(Clone, Copy)] struct TexDesc { mips: u32, d: u32, skip: u32, dim: TextureDimension, samples: u32 }
struct Proxy { dirty: Dirty, values: Vec<u64>, tex: Texture, format: TextureFormat }
struct Tex { tex: Texture, desc: TexDesc, format: TextureFormat, astc: bool, values: Vec<u64>,
    proxy_depth: Option<Proxy>, proxy_stencil: Option<Proxy>, proxy_fdepth: Option<Proxy>, proxy_half: Option<Proxy> }
struct ViewObj { res: u32, fmt: u32, a: u32, b: u32, c: u32, d: u32, is_buf: bool, views: HashMap<String, TextureView> }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] enum Kind { Texture, Other }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] enum Sample { Float, Uint, Depth }
#[derive(Clone)] struct Binding { kind: Kind, binding: u32, sample: Sample, dim: TextureViewDimension }
struct Info { bindings: Vec<Binding> }
struct Program { uid: u64, text: String, info: Info }
struct State { srv: [Vec<u32>; 3] }
struct Gpu { textures: HashMap<u32, Tex>, views: HashMap<u32, ViewObj>, s: State,
    astc_pending: HashSet<u32>, unfilt: HashMap<u64, (u128, u128)>, pdirty: bool, last_pl: Option<()>,
    enabled: bool, converted: usize }
static F32_HALF: AtomicBool = AtomicBool::new(true);
fn is_depth(format: TextureFormat) -> bool { format == TextureFormat::Depth }
fn half_of(format: TextureFormat) -> Option<(TextureFormat, usize)> {
    (format == TextureFormat::R32Float).then_some((TextureFormat::R16Float, 1))
}
struct MappedFormat { format: TextureFormat }
fn map_format(_: u32, _: u32) -> MappedFormat { MappedFormat { format: TextureFormat::Rgba } }
fn samplers_paired_with(_: &str, _: u128) -> u128 { 0 }
fn log_once(_: &str, _: &str) {}
impl Gpu {
    fn new(enabled: bool, format: TextureFormat) -> Self {
        let texture = Tex { tex: Texture { id: 10 }, desc: TexDesc { mips: 4, d: 12, skip: 1, dim: TextureDimension::D2, samples: 1 },
            format, astc: false, values: (0..48).map(|n| n * 17 + 1).collect(),
            proxy_depth: None, proxy_stencil: None, proxy_fdepth: None, proxy_half: None };
        let view = ViewObj { res: 1, fmt: 0, a: 0, b: 0, c: 0, d: 0, is_buf: false, views: HashMap::new() };
        Self { textures: [(1, texture)].into(), views: [(1, view)].into(), s: State { srv: [vec![1], vec![1], vec![1]] },
            astc_pending: HashSet::new(), unfilt: HashMap::new(), pdirty: false, last_pl: None, enabled, converted: 0 }
    }
    fn tex(&self, id: u32) -> Option<&Tex> { self.textures.get(&id) }
    fn tex_mut(&mut self, id: u32) -> Option<&mut Tex> { self.textures.get_mut(&id) }
    fn view_obj(&self, id: u32) -> Option<&ViewObj> { self.views.get(&id) }
    fn view_obj_mut(&mut self, id: u32) -> Option<&mut ViewObj> { self.views.get_mut(&id) }
    fn flush_astc(&mut self) { self.astc_pending.clear(); }
    fn scoped<T>(&mut self, _: &str, f: impl FnOnce(&()) -> T) -> Option<T> { Some(f(&())) }
    fn proxy(&mut self, id: u32, kind: u32, requested: Option<Span>) -> Option<(Texture, TextureFormat)> {
        let enabled = self.enabled;
        let t = self.tex_mut(id)?; let desc = t.desc; let source = t.values.clone();
        let slot = match kind { 0 => &mut t.proxy_depth, 1 => &mut t.proxy_stencil,
            2 => &mut t.proxy_fdepth, _ => &mut t.proxy_half };
        let format = match kind { 0 => TextureFormat::R32Float, 1 => TextureFormat::Rg8Uint,
            2 => TextureFormat::Depth, _ => TextureFormat::R16Float };
        let px = slot.get_or_insert_with(|| Proxy { dirty: Dirty::new(desc.mips, desc.d,
            enabled && desc.dim == TextureDimension::D2 && desc.samples == 1), values: vec![u64::MAX; source.len()],
            tex: Texture { id: 11 + kind }, format });
        let pending = px.dirty.pending(source.len(), requested);
        for &pass in &pending { px.values[pass] = source[pass]; }
        px.dirty.recorded(&pending);
        let result = (px.tex.clone(), px.format); self.converted += pending.len(); Some(result)
    }
    fn depth_proxy(&mut self, id: u32, stencil: bool, requested: Option<Span>) -> Option<(Texture, TextureFormat)> { self.proxy(id, u32::from(stencil), requested) }
    fn float_to_depth_proxy(&mut self, id: u32, requested: Option<Span>) -> Option<(Texture, TextureFormat)> { self.proxy(id, 2, requested) }
    fn half_proxy(&mut self, id: u32, requested: Option<Span>) -> Option<(Texture, TextureFormat)> { self.proxy(id, 3, requested) }
    fn read(&self, view: &TextureView) -> Vec<u64> {
        let t = self.tex(1).unwrap();
        let values = if view.texture == t.tex.id { &t.values } else {
            [&t.proxy_depth, &t.proxy_stencil, &t.proxy_fdepth, &t.proxy_half].into_iter()
                .filter_map(|slot| slot.as_ref()).find(|px| px.tex.id == view.texture).map(|px| &px.values).unwrap()
        };
        (view.desc.base_mip_level..view.desc.base_mip_level + view.desc.mip_level_count.unwrap()).flat_map(|mip|
            (view.desc.base_array_layer..view.desc.base_array_layer + view.desc.array_layer_count.unwrap())
                .map(move |layer| values[(mip * t.desc.d + layer) as usize])).collect()
    }
PRODUCTION_METHODS
}
fn binding(sample: Sample, dim: TextureViewDimension) -> Binding { Binding { kind: Kind::Texture, binding: 32, sample, dim } }

#[test]
fn production_prepare_and_srv_view_match_full_conversion_reference_after_writes() {
    for (format, sample) in [(TextureFormat::Depth, Sample::Float), (TextureFormat::Depth, Sample::Uint),
        (TextureFormat::R32Float, Sample::Depth), (TextureFormat::R32Float, Sample::Float)] {
        let mut old = Gpu::new(false, format); let mut new = Gpu::new(true, format);
        let mut random = 0xd819_43a0_0bb1_7c22u64;
        for sequence in 1..4001 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let mip = (random >> 9) as u32 % 4; let layer = (random >> 23) as u32 % 12;
            let dim = [TextureViewDimension::D2, TextureViewDimension::D2Array,
                TextureViewDimension::Cube, TextureViewDimension::CubeArray][(random >> 33) as usize % 4];
            let b = binding(sample, dim);
            for gpu in [&mut old, &mut new] {
                gpu.tex_mut(1).unwrap().values[(mip * 12 + layer) as usize] = sequence;
                if is_depth(format) { gpu.mark_depth_range_dirty(1, Some(Span::one(mip, layer)), true, true); }
                else if random & 15 == 0 { gpu.mark_color_dirty(1); }
                else { gpu.mark_color_range_dirty(1, Some(Span::one(mip, layer))); }
                let v = gpu.view_obj_mut(1).unwrap();
                v.a = (random >> 11) as u32 % 5; v.b = [0, u32::MAX, 1, 2][(random >> 17) as usize % 4];
                v.c = if matches!(dim, TextureViewDimension::Cube | TextureViewDimension::CubeArray) { ((random >> 25) as u32 % 2) * 6 }
                    else { (random >> 25) as u32 % 12 };
                v.d = if dim == TextureViewDimension::CubeArray { 6 } else { (random >> 27) as u32 % 5 };
                let prog = Program { uid: 1, text: String::new(), info: Info { bindings: vec![b.clone()] } };
                gpu.prepare_depth_proxies(&prog, sequence as usize % 3);
            }
            let a = old.legacy_srv_view(1, &b).unwrap(); let b = new.srv_view(1, &b).unwrap();
            assert_eq!(a.desc, b.desc); assert_eq!(old.read(&a), new.read(&b), "stale source at sequence {sequence}");
        }
        assert!(new.converted < old.converted / 2, "test must exercise fewer conversions");
    }
}
#[test]
fn readonly_aspects_remain_independent_through_production_dirty_methods() {
    for readonly in 0..4 {
        let mut gpu = Gpu::new(true, TextureFormat::Depth);
        gpu.depth_proxy(1, false, None); gpu.depth_proxy(1, true, None);
        gpu.mark_depth_range_dirty(1, Some(Span::one(2, 3)), readonly & 1 == 0, readonly & 2 == 0);
        let t = gpu.tex(1).unwrap();
        assert_eq!(!t.proxy_depth.as_ref().unwrap().dirty.pending(48, Some(Span::one(2, 3))).is_empty(), readonly & 1 == 0);
        assert_eq!(!t.proxy_stencil.as_ref().unwrap().dirty.pending(48, Some(Span::one(2, 3))).is_empty(), readonly & 2 == 0);
        assert!(t.proxy_depth.as_ref().unwrap().dirty.pending(48, Some(Span::one(0, 0))).is_empty());
    }
}
#[test]
fn direct_depth_comparison_views_keep_the_original_texture_and_aspect() {
    let mut gpu = Gpu::new(true, TextureFormat::Depth); let b = binding(Sample::Depth, TextureViewDimension::D2);
    let view = gpu.srv_view(1, &b).unwrap(); assert_eq!(view.texture, 10);
    assert_eq!(view.desc.aspect, TextureAspect::DepthOnly); assert_eq!(gpu.converted, 0);
}
#[test]
fn invalid_bases_or_unknown_dimensions_cannot_select_incomplete_proxy_work() {
    let mut gpu = Gpu::new(true, TextureFormat::R32Float); let b = binding(Sample::Float, TextureViewDimension::D2Array);
    gpu.view_obj_mut(1).unwrap().c = u32::MAX;
    assert!(gpu.proxy_srv_range(1, &b).is_none());
    let view = gpu.srv_view(1, &b).unwrap(); assert_eq!(gpu.converted, 48);
    assert_eq!(view.desc.base_array_layer, 11); assert_eq!(view.desc.array_layer_count, Some(1));
    gpu.tex_mut(1).unwrap().desc.dim = TextureDimension::D3;
    assert!(gpu.proxy_srv_range(1, &b).is_none());
}
'''


def main() -> None:
    root = Path(__file__).resolve().parents[1]
    gpu = (root / "game-runtime/src/gpu.rs").read_text()
    for name in ("depth_proxy", "float_to_depth_proxy", "half_proxy"):
        body = function(gpu, name)
        assert "px.dirty.pending(available, requested)" in body
        assert body.index("p.draw(0..3, 0..1)") < body.index("dirty.recorded(&pending)")
        assert "pending.iter().map(|&index|" in body
        assert "desc.dim == TextureDimension::D2" in body
    assert "!ro_d, has_stencil(f) && !ro_s" in function(gpu, "ensure_pass")
    clear = function(gpu, "clear_uav")
    assert clear.rindex("self.mark_color_dirty(res)") > clear.rindex("self.queue.write_texture(")
    upload = function(gpu, "upload_texture")
    for kind in ("depth", "stencil", "fdepth", "half"):
        assert f"t.proxy_{kind}.as_mut() {{ p.dirty.invalidate(None); }}" in upload
    assert 'var_os("MUGUET_SUBRESOURCE_PROXIES").is_some_and(|v|v=="1")' in gpu
    methods = "\n".join(function(gpu, name) for name in ("proxy_srv_range", "prepare_depth_proxies", "srv_view", "needs_half",
        "mark_color_dirty", "mark_color_range_dirty", "mark_depth_dirty", "mark_depth_range_dirty"))
    # Frozen pre-candidate production code prevents a shared normalization bug
    # in ON/OFF candidates from making the differential model pass falsely.
    methods += "\n" + (root / "tools/fixtures/gpu_proxy_srv_view_baseline.rs").read_text()
    rustc = shutil.which("rustc")
    if rustc is None:
        raise SystemExit("rustc unavailable: source game-runtime/scripts/env.sh first")
    with tempfile.TemporaryDirectory(prefix="gpu-proxy-subresources-") as temporary:
        source, binary = Path(temporary) / "tests.rs", Path(temporary) / "tests"
        path = json.dumps(str(root / "game-runtime/src/proxy_subresources.rs"), ensure_ascii=False)
        source.write_text(f'#![allow(dead_code)]\n#[path = {path}]\nmod proxy_subresources;\n' + HARNESS.replace("PRODUCTION_METHODS", methods))
        subprocess.run([rustc, "--edition=2021", "--test", str(source), "-o", str(binary)], check=True)
        subprocess.run([str(binary)], check=True)
    print("production view/prepare/dirty differential model and boundaries passed; no GPU pixel/performance claim")


if __name__ == "__main__":
    main()
