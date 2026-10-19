struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}
@vertex
fn main(@builtin(vertex_index) vertex: u32) -> Output {
    let p = vec2<f32>(f32((vertex << 1u) & 2u), f32(vertex & 2u));
    var out: Output;
    out.position = vec4<f32>(p * 2.0 - vec2<f32>(1.0), 0.0, 1.0);
    out.uv = p;
    return out;
}
