struct Params { width: u32, height: u32, rowWords: u32, baseWords: u32 }
var<immediate> params: Params;
@group(0) @binding(0) var depth: texture_depth_2d;
@group(0) @binding(1) var<storage, read_write> output: array<u32>;
@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(depth, 0);
    if gid.x >= params.width || gid.y >= params.height || gid.x >= size.x || gid.y >= size.y { return; }
    let index = params.baseWords + gid.y * params.rowWords + gid.x;
    if index >= arrayLength(&output) { return; }
    output[index] = bitcast<u32>(textureLoad(depth, vec2<i32>(gid.xy), 0));
}
