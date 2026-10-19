diagnostic(off, derivative_uniformity);

var<private> r0 : vec4<f32>;

var<private> r1 : vec4<f32>;

var<private> r2 : vec4<f32>;

struct cb4_struct {
  tint_symbol : array<vec4<f32>, 4u>,
}

@group(1u) @binding(5u) var<uniform> cb5_5 : cb4_struct;

@group(1u) @binding(18u) var s2 : sampler;

@group(1u) @binding(34u) var t2 : texture_2d<f32>;

var<private> o0 : vec4<f32>;

fn main_inner(v1 : vec4<f32>, v2 : vec4<f32>) {
  r0 = fma(cb5_5.tint_symbol[0u].xyxy, vec4<f32>(-0.5f, -0.5f, 0.5f, -0.5f), v2.xy.xyxy);
  r1 = textureSample(t2, s2, r0.xyxx.xy);
  r0 = textureSample(t2, s2, r0.zwzz.xy);
  r0 = (r0 + r1);
  r1 = fma(cb5_5.tint_symbol[0u].xyxy, vec4<f32>(-0.5f, 0.5f, 0.5f, 0.5f), v2.xy.xyxy);
  r2 = textureSample(t2, s2, r1.xyxx.xy);
  r1 = textureSample(t2, s2, r1.zwzz.xy);
  r0 = (r0 + r2);
  r0 = (r1 + r0);
  r1 = textureSample(t2, s2, v2.xy.xyxx.xy);
  r0 = (r0 + r1);
  r0 = (r0 * vec4<f32>(0.20000000298023223877f));
  r0.x = dot(r0, cb5_5.tint_symbol[2u]);
  r0.y = ((-(cb5_5.tint_symbol[3u].xxxx)).y + cb5_5.tint_symbol[3u].y);
  r0.x = fma(r0.x, r0.y, cb5_5.tint_symbol[3u].x);
  o0 = (r0.xxxx * v1);
}

@fragment
fn main(@location(1u) v1 : vec4<f32>, @location(2u) v2 : vec4<f32>) -> @location(0u) vec4<f32> {
  main_inner(v1, v2);
  return o0;
}
