// GPU version of the look pipeline in edit.rs / film.rs / spectral.rs.
//
// Every entry point mirrors one CPU stage, step for step and with the same
// constants, so a preview rendered here matches the CPU render (and so the
// export) to within rounding. When a CPU stage changes, change it here too:
// the `gpu_matches_cpu` test compares the two on every film.
//
// All image data lives in one storage buffer (`work`) at offsets given per
// pass: colour images as 4 floats per pixel (RGB + unused), planes as one.

struct Params {
    // w, h of the image; sw, sh of the reduced grid.
    dims: vec4<u32>,
    // Offsets (in floats) of the buffers a pass reads and writes.
    offs: vec4<u32>,
    // Per-pass integers (channel, radius, flags, ...).
    ints: vec4<u32>,
    ints2: vec4<u32>,
    f: array<vec4<f32>, 32>,
}

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var<storage, read_write> work: array<f32>;
@group(0) @binding(2) var<storage, read> lut: array<f32>;
@group(0) @binding(3) var<storage, read> cam: array<vec4<f32>>;

const LN10: f32 = 2.302585093;
const LOG2_10: f32 = 3.321928095;

fn pf(i: u32) -> f32 {
    return P.f[i / 4u][i % 4u];
}

fn pixel_index(gid: vec3<u32>, nwg: vec3<u32>) -> u32 {
    return gid.x + gid.y * nwg.x * 256u;
}

fn log10_(x: f32) -> f32 {
    return log(x) / LN10;
}

fn exp10(x: f32) -> f32 {
    return exp2(x * LOG2_10);
}

fn load3(off: u32, i: u32) -> vec3<f32> {
    let b = off + 4u * i;
    return vec3<f32>(work[b], work[b + 1u], work[b + 2u]);
}

fn store3(off: u32, i: u32, v: vec3<f32>) {
    let b = off + 4u * i;
    work[b] = v.x;
    work[b + 1u] = v.y;
    work[b + 2u] = v.z;
}

fn mat_at(base: u32) -> mat3x3<f32> {
    // Row-major 3x3 at f[base..base+9]; WGSL matrices are column-major.
    return transpose(mat3x3<f32>(
        vec3<f32>(pf(base), pf(base + 1u), pf(base + 2u)),
        vec3<f32>(pf(base + 3u), pf(base + 4u), pf(base + 5u)),
        vec3<f32>(pf(base + 6u), pf(base + 7u), pf(base + 8u)),
    ));
}

// Row-major matrix times vector, summed in the CPU's order.
fn mulv(m: mat3x3<f32>, v: vec3<f32>) -> vec3<f32> {
    let t = transpose(m);
    return vec3<f32>(
        t[0].x * v.x + t[0].y * v.y + t[0].z * v.z,
        t[1].x * v.x + t[1].y * v.y + t[1].z * v.z,
        t[2].x * v.x + t[2].y * v.y + t[2].z * v.z,
    );
}

// ------------------------------------------------------------ develop

// cam -> working space (develop::to_working). f[0..9] matrix. dst offs.y.
@compute @workgroup_size(256)
fn working(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = pixel_index(gid, nwg);
    if i >= P.dims.x * P.dims.y { return; }
    store3(P.offs.y, i, mulv(mat_at(0u), cam[i].xyz));
}

// Luma plane of a colour image (develop::sharpen). src offs.x, dst offs.y.
@compute @workgroup_size(256)
fn luma(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = pixel_index(gid, nwg);
    if i >= P.dims.x * P.dims.y { return; }
    let p = load3(P.offs.x, i);
    work[P.offs.y + i] = (p.x + 2.0 * p.y + p.z) * 0.25;
}

// Unsharp mask in log space. image offs.x, luma offs.y, blurred luma offs.z,
// f[0] amount.
@compute @workgroup_size(256)
fn sharpen(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = pixel_index(gid, nwg);
    if i >= P.dims.x * P.dims.y { return; }
    let y = work[P.offs.y + i];
    if y > 1e-6 {
        let b = work[P.offs.z + i];
        let f = exp(clamp((log(y) - log(max(b, 1e-6))) * pf(0u), -0.7, 0.7));
        store3(P.offs.x, i, load3(P.offs.x, i) * f);
    }
}

// ------------------------------------------------------------ blurs

// Separable Gaussian with clamped edges (film::blur). Taps in f[0..2r+1],
// r = ints.x. Horizontal: src offs.x -> dst offs.y; vertical the same.
@compute @workgroup_size(256)
fn blur_h(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let w = P.dims.x;
    let i = pixel_index(gid, nwg);
    if i >= w * P.dims.y { return; }
    let x = i32(i % w);
    let row = i - u32(x);
    let r = i32(P.ints.x);
    var acc = 0.0;
    for (var j = 0; j <= 2 * r; j++) {
        let xx = u32(clamp(x + j - r, 0, i32(w) - 1));
        acc += pf(u32(j)) * work[P.offs.x + row + xx];
    }
    work[P.offs.y + i] = acc;
}

@compute @workgroup_size(256)
fn blur_v(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let w = P.dims.x;
    let h = P.dims.y;
    let i = pixel_index(gid, nwg);
    if i >= w * h { return; }
    let x = i % w;
    let y = i32(i / w);
    let r = i32(P.ints.x);
    var acc = 0.0;
    for (var j = 0; j <= 2 * r; j++) {
        let yy = u32(clamp(y + j - r, 0, i32(h) - 1));
        acc += pf(u32(j)) * work[P.offs.x + yy * w + x];
    }
    work[P.offs.y + i] = acc;
}

// Copy channel ints.x of a colour image (offs.x) to a plane (offs.y).
@compute @workgroup_size(256)
fn extract(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = pixel_index(gid, nwg);
    if i >= P.dims.x * P.dims.y { return; }
    work[P.offs.y + i] = work[P.offs.x + 4u * i + P.ints.x];
}

// Plane copy offs.x -> offs.y.
@compute @workgroup_size(256)
fn copy_plane(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = pixel_index(gid, nwg);
    if i >= P.dims.x * P.dims.y { return; }
    work[P.offs.y + i] = work[P.offs.x + i];
}

// Box-average a plane (offs.x, w x h) onto a grid reduced by d = ints.x
// (offs.y, sw x sh), as film::smooth_blur does.
@compute @workgroup_size(256)
fn box_down(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let sw = P.dims.z;
    let i = pixel_index(gid, nwg);
    if i >= sw * P.dims.w { return; }
    let d = P.ints.x;
    let w = P.dims.x;
    let sx = i % sw;
    let sy = i / sw;
    var sum = 0.0;
    var n = 0u;
    for (var y = sy * d; y < min((sy + 1u) * d, P.dims.y); y++) {
        for (var x = sx * d; x < min((sx + 1u) * d, w); x++) {
            sum += work[P.offs.x + y * w + x];
            n++;
        }
    }
    work[P.offs.y + i] = sum / f32(max(n, 1u));
}

// Halation source on the reduced grid: box average of the light above the
// threshold (f[0]) of the layer exposures at offs.x.
@compute @workgroup_size(256)
fn halation_source(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let sw = P.dims.z;
    let i = pixel_index(gid, nwg);
    if i >= sw * P.dims.w { return; }
    let d = P.ints.x;
    let w = P.dims.x;
    let t = pf(0u);
    let sx = i % sw;
    let sy = i / sw;
    var sum = 0.0;
    var n = 0u;
    for (var y = sy * d; y < min((sy + 1u) * d, P.dims.y); y++) {
        for (var x = sx * d; x < min((sx + 1u) * d, w); x++) {
            let e = load3(P.offs.x, y * w + x);
            sum += max(0.6 * e.x + 0.3 * e.y + 0.1 * e.z - t, 0.0);
            n++;
        }
    }
    work[P.offs.y + i] = sum / f32(max(n, 1u));
}

// Bilinear sample of a reduced grid (offs.x) at full-size pixel i.
fn upsample(i: u32) -> f32 {
    let w = P.dims.x;
    let sw = P.dims.z;
    let sh = P.dims.w;
    let d = f32(P.ints.x);
    let x = i % w;
    let y = i / w;
    let fy = clamp((f32(y) + 0.5) / d - 0.5, 0.0, f32(sh - 1u));
    let y0 = u32(floor(fy));
    let ty = fy - floor(fy);
    let y1 = min(y0 + 1u, sh - 1u);
    let fx = clamp((f32(x) + 0.5) / d - 0.5, 0.0, f32(sw - 1u));
    let x0 = u32(floor(fx));
    let tx = fx - floor(fx);
    let x1 = min(x0 + 1u, sw - 1u);
    let s = P.offs.x;
    let top = work[s + y0 * sw + x0] * (1.0 - tx) + work[s + y0 * sw + x1] * tx;
    let bottom = work[s + y1 * sw + x0] * (1.0 - tx) + work[s + y1 * sw + x1] * tx;
    return top * (1.0 - ty) + bottom * ty;
}

@compute @workgroup_size(256)
fn upsample_plane(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = pixel_index(gid, nwg);
    if i >= P.dims.x * P.dims.y { return; }
    work[P.offs.y + i] = upsample(i);
}

// Add the blurred halation glow (grid offs.x) to exposures (offs.y),
// f[0..3] = tint * gain.
@compute @workgroup_size(256)
fn halation_add(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = pixel_index(gid, nwg);
    if i >= P.dims.x * P.dims.y { return; }
    let glow = upsample(i);
    let e = load3(P.offs.y, i);
    store3(P.offs.y, i, vec3<f32>(e.x + pf(0u) * glow, e.y + pf(1u) * glow, e.z + pf(2u) * glow));
}

// Film sharpness (film::apply_spread) for channel ints.x of exposures at
// offs.x: sharp plane offs.y, broad plane offs.z (if ints.y), k = f[0].
@compute @workgroup_size(256)
fn spread_combine(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = pixel_index(gid, nwg);
    if i >= P.dims.x * P.dims.y { return; }
    var s = work[P.offs.y + i];
    if P.ints.y != 0u {
        let k = pf(0u);
        s = (1.0 + k) * s - k * work[P.offs.z + i];
    }
    work[P.offs.x + 4u * i + P.ints.x] = max(s, 0.0);
}

// ------------------------------------------------------------ film

// Layer exposures (FilmModel::layer_exposure): offs.x -> offs.y, f[0..9].
@compute @workgroup_size(256)
fn layers(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = pixel_index(gid, nwg);
    if i >= P.dims.x * P.dims.y { return; }
    store3(P.offs.y, i, mulv(mat_at(0u), load3(P.offs.x, i)));
}

// film::Lut::eval for LUT number `n` (2048 entries at lut[n * 2048]).
// f[20] = x0, f[21] = step.
fn lut_eval(n: u32, x: f32) -> f32 {
    let N = 2048u;
    let t = clamp((x - pf(20u)) / pf(21u), 0.0, f32(N - 1u));
    let i = min(u32(t), N - 2u);
    let f = t - f32(i);
    let b = n * N;
    return lut[b + i] + (lut[b + i + 1u] - lut[b + i]) * f;
}

fn highlight_fit(x: f32) -> f32 {
    let clip = pf(5u);
    let k = pf(6u);
    if x <= clip {
        return x + k * x * x;
    }
    return clip + k * clip * clip + (x - clip) * pf(7u);
}

// Developed state (FilmModel::state_from_exposure + Engine::develop):
// exposures offs.x -> state offs.y.
// f[0] flare, f[1] log_offset, f[2] grey_log_e, f[3] contrast,
// f[4] highlight slope, f[5..8] fit (clip, k, end slope), f[8] interimage,
// f[9] knee, f[10..19] dye matrix (ints.x == 1: matrix engine),
// f[20..22] LUT x0, step.
@compute @workgroup_size(256)
fn state(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = pixel_index(gid, nwg);
    if i >= P.dims.x * P.dims.y { return; }
    let e = load3(P.offs.x, i);
    let grey = pf(2u);
    let knee = pf(9u);
    var le = vec3<f32>(0.0);
    for (var c = 0u; c < 3u; c++) {
        let l = log10_(max(e[c], 0.0) + pf(0u)) + pf(1u);
        var rel = max(l - grey, -3.0) * pf(3u);
        if rel > knee {
            rel = knee + highlight_fit(rel - knee) * pf(4u);
        }
        le[c] = grey + rel;
    }
    let mean = (le.x + le.y + le.z) / 3.0;
    let ii = pf(8u);
    var a = vec3<f32>(0.0);
    for (var c = 0u; c < 3u; c++) {
        a[c] = lut_eval(c, le[c] + ii * clamp(le[c] - mean, -1.0, 1.0));
    }
    if P.ints.x == 1u {
        a = mulv(mat_at(10u), a);
    }
    store3(P.offs.y, i, a);
}

// ------------------------------------------------------------ grain

// 64-bit unsigned integers as (lo, hi).
fn mul32(a: u32, b: u32) -> vec2<u32> {
    let a0 = a & 0xFFFFu;
    let a1 = a >> 16u;
    let b0 = b & 0xFFFFu;
    let b1 = b >> 16u;
    let p00 = a0 * b0;
    let p01 = a0 * b1;
    let p10 = a1 * b0;
    let p11 = a1 * b1;
    let mid = (p00 >> 16u) + (p01 & 0xFFFFu) + (p10 & 0xFFFFu);
    let lo = (p00 & 0xFFFFu) | (mid << 16u);
    let hi = p11 + (p01 >> 16u) + (p10 >> 16u) + (mid >> 16u);
    return vec2<u32>(lo, hi);
}

fn mul64(a: vec2<u32>, b: vec2<u32>) -> vec2<u32> {
    let p = mul32(a.x, b.x);
    return vec2<u32>(p.x, p.y + a.x * b.y + a.y * b.x);
}

fn add64(a: vec2<u32>, b: vec2<u32>) -> vec2<u32> {
    let lo = a.x + b.x;
    return vec2<u32>(lo, a.y + b.y + select(0u, 1u, lo < a.x));
}

fn shr64(a: vec2<u32>, s: u32) -> vec2<u32> {
    // 0 < s < 32
    return vec2<u32>((a.x >> s) | (a.y << (32u - s)), a.y >> s);
}

fn splitmix(x0: vec2<u32>) -> vec2<u32> {
    var x = add64(x0, vec2<u32>(0x7F4A7C15u, 0x9E3779B9u));
    x = mul64(x ^ shr64(x, 30u), vec2<u32>(0x1CE4E5B9u, 0xBF58476Du));
    x = mul64(x ^ shr64(x, 27u), vec2<u32>(0x133111EBu, 0x94D049BBu));
    return x ^ shr64(x, 31u);
}

// film::gaussian
fn gaussian(seed: vec2<u32>, i: u32) -> f32 {
    let h = splitmix(seed ^ mul64(vec2<u32>(i, 0u), vec2<u32>(0x6659FD93u, 0xD6E8FEB8u)));
    let sum = (h.x & 0xFFFFu) + (h.x >> 16u) + (h.y & 0xFFFFu) + (h.y >> 16u);
    return (f32(sum) / 65535.0 - 2.0) * sqrt(3.0);
}

// Noise plane (film::noise_field) at offs.y. ints.xy = seed (lo, hi),
// ints.zw = frame origin, ints2.x = frame stride.
@compute @workgroup_size(256)
fn noise(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let w = P.dims.x;
    let i = pixel_index(gid, nwg);
    if i >= w * P.dims.y { return; }
    let x = i % w;
    let y = i / w;
    let gi = (P.ints.w + y) * P.ints2.x + P.ints.z + x;
    work[P.offs.y + i] = gaussian(P.ints.xy, gi);
}

// Add one grain field (blurred noise at offs.y) to the state at offs.x:
// to every channel (ints.x == 3) or to channel ints.x. f[0] sigma,
// f[1] field weight, f[2] blur gain.
@compute @workgroup_size(256)
fn grain_add(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = pixel_index(gid, nwg);
    if i >= P.dims.x * P.dims.y { return; }
    let a = 0.6;
    let b = 0.4;
    let norm = 1.0 / sqrt(a * a + 2.0 * a * b * 1.5958 + 3.0 * b * b);
    let n0 = work[P.offs.y + i] / pf(2u);
    let n = n0 * (a + b * abs(n0)) * norm;
    let sigma = pf(0u);
    let wt = pf(1u);
    var d = load3(P.offs.x, i);
    if P.ints.x == 3u {
        let base = vec3<f32>(sigma * sqrt(max(d.x, 0.05)), sigma * sqrt(max(d.y, 0.05)), sigma * sqrt(max(d.z, 0.05)));
        d = d + base * wt * n;
    } else {
        let c = P.ints.x;
        d[c] = d[c] + sigma * sqrt(max(d[c], 0.05)) * wt * n;
    }
    store3(P.offs.x, i, d);
}

// ------------------------------------------------------------ finish

// spectral::Table::eval on the paper table: lut[base + 3 * k + ch].
fn table_eval(base: u32, n: u32, x0: f32, step: f32, ch: u32, x: f32) -> f32 {
    let t = clamp((x - x0) / step, 0.0, f32(n - 1u));
    let i = min(u32(t), n - 2u);
    let f = t - f32(i);
    let a = lut[base + 3u * i + ch];
    return a + (lut[base + 3u * (i + 1u) + ch] - a) * f;
}

// spectral::Viewer::rgb. 41^3 log-XYZ entries at lut[base], 3 floats each.
// f[30] lo, f[31] scale, f[32..41] to_rgb.
fn viewer(base: u32, a: vec3<f32>) -> vec3<f32> {
    let n = 41u;
    let top = f32(n - 1u);
    var i0 = vec3<u32>(0u);
    var f = vec3<f32>(0.0);
    for (var c = 0u; c < 3u; c++) {
        let t = clamp((a[c] - pf(30u)) * pf(31u), 0.0, top);
        i0[c] = min(u32(t), n - 2u);
        f[c] = t - f32(i0[c]);
    }
    var acc = vec3<f32>(0.0);
    for (var corner = 0u; corner < 8u; corner++) {
        let d = vec3<u32>((corner >> 2u) & 1u, (corner >> 1u) & 1u, corner & 1u);
        var w = 1.0;
        for (var c = 0u; c < 3u; c++) {
            w *= select(1.0 - f[c], f[c], d[c] == 1u);
        }
        if w == 0.0 { continue; }
        let idx = base + 3u * (((i0.x + d.x) * n + i0.y + d.y) * n + i0.z + d.z);
        acc += w * vec3<f32>(lut[idx], lut[idx + 1u], lut[idx + 2u]);
    }
    let xyz = vec3<f32>(exp10(acc.x), exp10(acc.y), exp10(acc.z));
    return mulv(mat_at(32u), xyz);
}

// Engine::finish for the spectral engine: state (offs.x) -> scanner-channel
// density, in place. ints.x: 1 lab scan, 2 printer + viewer, 3 viewer.
// Lab scan: f[0..3] gain, f[3..6] dmin, f[6..9] p_ref, f[9..12] slope,
// f[12] k, f[13] c, f[14] dmax.
// Printer: f[0..3] lights, f[3..6] gain, f[6..9] dmin, f[15] paper x0,
// f[16] paper step; ints.y paper entries, ints.z paper base; ints.w viewer base.
@compute @workgroup_size(256)
fn finish(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = pixel_index(gid, nwg);
    if i >= P.dims.x * P.dims.y { return; }
    let s = load3(P.offs.x, i);
    var rgb = vec3<f32>(0.0);
    if P.ints.x == 1u {
        for (var c = 0u; c < 3u; c++) {
            let p = pf(c) * (s[c] - pf(3u + c));
            let rel = (p - pf(6u + c)) / pf(9u + c);
            let d = pf(14u) / (1.0 + exp(pf(12u) * (rel - pf(13u))));
            rgb[c] = exp10(-d);
        }
    } else if P.ints.x == 2u {
        var paper = vec3<f32>(0.0);
        for (var c = 0u; c < 3u; c++) {
            let lh = pf(c) - pf(3u + c) * (s[c] - pf(6u + c));
            paper[c] = table_eval(P.ints.z, P.ints.y, pf(15u), pf(16u), c, lh);
        }
        rgb = viewer(P.ints.w, paper);
    } else {
        rgb = viewer(P.ints.w, s);
    }
    store3(P.offs.x, i, vec3<f32>(-log10_(max(rgb.x, 1e-6)), -log10_(max(rgb.y, 1e-6)), -log10_(max(rgb.z, 1e-6))));
}

// ------------------------------------------------------------ output

fn srgb8(v: f32) -> u32 {
    let c = clamp(v, 0.0, 1.0);
    var e = 12.92 * c;
    if c > 0.0031308 {
        e = 1.055 * pow(c, 1.0 / 2.4) - 0.055;
    }
    return min(u32(e * 255.0 + 0.5), 255u);
}

fn pack(v: vec3<f32>) -> f32 {
    return bitcast<f32>(srgb8(v.x) | (srgb8(v.y) << 8u) | (srgb8(v.z) << 16u) | 0xFF000000u);
}

// output::to_srgb8: working space (offs.x) -> packed sRGB (offs.y), f[0..9].
@compute @workgroup_size(256)
fn neutral_out(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = pixel_index(gid, nwg);
    if i >= P.dims.x * P.dims.y { return; }
    work[P.offs.y + i] = pack(mulv(mat_at(0u), load3(P.offs.x, i)));
}

fn into_gamut(c: vec3<f32>) -> vec3<f32> {
    let m = min(c.x, min(c.y, c.z));
    if m >= 0.0 { return c; }
    let y = max(0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z, 0.0);
    let t = y / (y - m);
    return vec3<f32>(y + t * (c.x - y), y + t * (c.y - y), y + t * (c.z - y));
}

// FilmModel::scan / project + output::display_to_srgb8: densities (offs.x)
// -> packed sRGB (offs.y).
// f[0..3] balance offsets, f[3..6] scales, f[6] t_white, f[7] t_black,
// f[8] 1/gamma, f[9..12] scan wb, f[12..21] scan matrix (ints.x == 1),
// ints.y == 1: projection with f[21] flare, f[22..25] wb, f[25] falloff;
// ints.zw frame origin, ints2.xy frame size.
@compute @workgroup_size(256)
fn film_out(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let w = P.dims.x;
    let i = pixel_index(gid, nwg);
    if i >= w * P.dims.y { return; }
    let d = load3(P.offs.x, i);
    var out = vec3<f32>(0.0);
    if P.ints.y == 1u {
        let u = f32(P.ints.z + i % w) / f32(P.ints2.x);
        let v = f32(P.ints.w + i / w) / f32(P.ints2.y);
        let x = 2.0 * u - 1.0;
        let y = 2.0 * v - 1.0;
        let light = exp2(-pf(25u) * (x * x + y * y) * 0.5);
        let flare = pf(21u);
        for (var c = 0u; c < 3u; c++) {
            let t = exp10(-(d[c] * pf(3u + c) + pf(c))) / pf(6u);
            out[c] = clamp((t * light + flare) / (1.0 + flare) * pf(22u + c), 0.0, 1.0);
        }
    } else {
        var v = vec3<f32>(0.0);
        for (var c = 0u; c < 3u; c++) {
            let t = exp10(-(d[c] * pf(3u + c) + pf(c)));
            v[c] = (t - pf(7u)) / (pf(6u) - pf(7u));
        }
        if P.ints.x == 1u {
            v = into_gamut(mulv(mat_at(12u), v));
        }
        for (var c = 0u; c < 3u; c++) {
            let x = clamp(v[c], 0.0, 1.0);
            var g = 0.0;
            if x > 0.0 { g = pow(x, pf(8u)); }
            out[c] = clamp(g * pf(9u + c), 0.0, 1.0);
        }
    }
    work[P.offs.y + i] = pack(out);
}
