// muguet - bc1_astc.wgsl

struct Job { first_block: u32, blocks_x: u32, blocks_y: u32, layers: u32, src_word: u32, dst_word: u32, dst_row_words: u32, pad: u32 }

@group(0) @binding(0) var<storage, read> src: array<u32>;
@group(0) @binding(1) var<storage, read_write> dst: array<u32>;
@group(0) @binding(2) var<storage, read> jobs: array<Job>;

var<private> blk: array<u32, 4>;

fn put(pos: u32, n: u32, v: u32) {
    let v2 = v & ((1u << n) - 1u);
    let w = pos / 32u;
    let o = pos % 32u;
    blk[w] = blk[w] | (v2 << o);
    if (o + n > 32u) {
        blk[w + 1u] = blk[w + 1u] | (v2 >> (32u - o));
    }
}

fn put_weight(stream_index: u32, bits: u32, v: u32) {
    for (var t = 0u; t < bits; t = t + 1u) {
        let j = stream_index * bits + t;
        put(127u - j, 1u, (v >> t) & 1u);
    }
}

fn expand565(c: u32) -> vec3<u32> {
    let r = (c >> 11u) & 31u;
    let g = (c >> 5u) & 63u;
    let b = c & 31u;
    return vec3<u32>((r << 3u) | (r >> 2u), (g << 2u) | (g >> 4u), (b << 3u) | (b >> 2u));
}

fn put_rgb_endpoints(e0: vec3<u32>, e1: vec3<u32>) {
    put(17u, 8u, e0.r);
    put(25u, 8u, e1.r);
    put(33u, 8u, e0.g);
    put(41u, 8u, e1.g);
    put(49u, 8u, e0.b);
    put(57u, 8u, e1.b);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let n = arrayLength(&jobs);
    let g = gid.y * groups.x * 64u + gid.x;
    var lo = 0u;
    var hi = n - 1u;
    while (lo < hi) {
        let mid = (lo + hi + 1u) / 2u;
        if (jobs[mid].first_block <= g) { lo = mid; } else { hi = mid - 1u; }
    }
    let job = jobs[lo];
    let per_layer = job.blocks_x * job.blocks_y;
    let i = g - job.first_block;
    if (i >= per_layer * job.layers) { return; }
    let layer = i / per_layer;
    let rem = i % per_layer;
    let y = rem / job.blocks_x;
    let x = rem % job.blocks_x;

    let w0 = src[job.src_word + i * 2u];
    let idx = src[job.src_word + i * 2u + 1u];
    let c0 = w0 & 0xffffu;
    let c1 = w0 >> 16u;
    let a = expand565(c0);
    let b = expand565(c1);
    let swap = (a.r + a.g + a.b) > (b.r + b.g + b.b);
    let e0 = select(a, b, swap);
    let e1 = select(b, a, swap);
    blk = array<u32, 4>(0u, 0u, 0u, 0u);

    if (c0 > c1) {
        put(0u, 11u, 66u);   // 4x4 grid, 1 plane, 4 weight levels
        put(13u, 4u, 8u);    // CEM 8: LDR RGB direct
        put_rgb_endpoints(e0, e1);
        for (var k = 0u; k < 16u; k = k + 1u) {
            let id = (idx >> (2u * k)) & 3u;
            var t = select(select(select(2u, 1u, id == 2u), 3u, id == 1u), 0u, id == 0u);
            if (swap) { t = 3u - t; }
            put_weight(k, 2u, t);
        }
    } else {
        var transparent = false;
        for (var k = 0u; k < 16u; k = k + 1u) {
            if (((idx >> (2u * k)) & 3u) == 3u) { transparent = true; }
        }
        if (!transparent) {
            put(0u, 11u, 83u);   // 4x4 grid, 1 plane, 8 weight levels
            put(13u, 4u, 8u);
            put_rgb_endpoints(e0, e1);
            for (var k = 0u; k < 16u; k = k + 1u) {
                let id = (idx >> (2u * k)) & 3u;
                var t = select(select(3u, 7u, id == 1u), 0u, id == 0u);
                if (swap) { t = 7u - t; }
                put_weight(k, 3u, t);
            }
        } else {
            put(0u, 11u, 1089u); // 4x4 grid, 2 planes, 2 weight levels
            put(13u, 4u, 12u);   // CEM 12: LDR RGBA direct
            put_rgb_endpoints(e0, e1);
            put(65u, 8u, 0u);    // e0 alpha
            put(73u, 8u, 255u);  // e1 alpha
            put(94u, 2u, 3u);    // second plane drives alpha (sits just below the 32 weight bits)
            for (var k = 0u; k < 16u; k = k + 1u) {
                let id = (idx >> (2u * k)) & 3u;
                var t = select(1u, 0u, id == 0u);
                if (swap) { t = 1u - t; }
                if (id == 3u) { t = 0u; }
                put_weight(2u * k, 1u, t);
                put_weight(2u * k + 1u, 1u, select(1u, 0u, id == 3u));
            }
        }
    }

    let o = job.dst_word + (layer * job.blocks_y + y) * job.dst_row_words + x * 4u;
    dst[o] = blk[0];
    dst[o + 1u] = blk[1];
    dst[o + 2u] = blk[2];
    dst[o + 3u] = blk[3];
}
