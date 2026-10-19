struct Params { viewProj: mat4x4<f32>, translation: vec4<f32> }
var<immediate> params: Params;
struct Output { @builtin(position) clip: vec4<f32>, @location(0) color: vec4<f32> }
@vertex
fn main(@location(0) position: vec3<f32>, @location(1) color: vec4<f32>) -> Output {
    var output: Output;
    output.clip = params.viewProj * vec4<f32>(position + params.translation.xyz, 1.0);
    output.color = color;
    return output;
}
