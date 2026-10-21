// The shaders. Every shape is a signed distance evaluated per pixel in the shape's own plane
// (a 3×3 homography maps that plane onto the frame, so the phone's tilt is a real
// perspective, and edges stay anti-aliased under it). Colours are premultiplied throughout.
diagnostic(off, derivative_uniformity);

// One struct for every draw; each pipeline reads the fields it needs.
struct U {
    h0: vec4f, h1: vec4f, h2: vec4f,   // the homography's rows
    quad: vec4f,                       // the quad drawn, in the shape's plane
    rect: vec4f,
    radius: vec4f,                     // top-left, top-right, bottom-right, bottom-left
    color: vec4f,
    stops: array<vec4f, 4>,
    pos: vec4f,
    uv: vec4f,
    bcolor: vec4f,
    in0: vec4f, in0c: vec4f, in1: vec4f, in1c: vec4f,
    clip: vec4f, clipr: vec4f,
    hp0: vec4f, hp1: vec4f,
    p0: vec4f,
    p1: vec4f,
    view: vec4f,                       // half the design size; scale
    cm0: vec4f, cm1: vec4f, cm2: vec4f, // the shape's plane → the clip's plane
    cut: vec4f, cutr: vec4f,           // a shadow's own box (no shadow under it)
}

@group(0) @binding(0) var<uniform> u: U;

struct VOut {
    @builtin(position) pos: vec4f,
    @location(0) p: vec2f,
}

// Every shape: a quad in its local plane, mapped through the homography. Interpolating the
// local point with w = the homography's w keeps it perspective-correct.
@vertex
fn vs_shape(@builtin(vertex_index) vi: u32) -> VOut {
    let unit = vec2f(f32(vi & 1u), f32(vi >> 1u));
    let p = u.quad.xy + unit * u.quad.zw;
    let q = vec3f(p, 1.0);
    let s = vec3f(dot(u.h0.xyz, q), dot(u.h1.xyz, q), dot(u.h2.xyz, q));
    var o: VOut;
    o.pos = vec4f(s.x / u.view.x - s.z, s.z - s.y / u.view.y, 0.0, s.z);
    o.p = p;
    return o;
}

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4f {
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    return vec4f(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
}

// Rounded box distance; r = (top-left, top-right, bottom-right, bottom-left), y down.
fn sdRoundBox(p: vec2f, b: vec2f, r: vec4f) -> f32 {
    let left = select(r.w, r.x, p.y < 0.0);
    let right = select(r.z, r.y, p.y < 0.0);
    var rr = select(right, left, p.x < 0.0);
    rr = min(rr, min(b.x, b.y));
    let q = abs(p) - b + rr;
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2f(0.0))) - rr;
}
fn sdRect(p: vec2f, rect: vec4f, r: vec4f) -> f32 {
    return sdRoundBox(p - (rect.xy + rect.zw * 0.5), rect.zw * 0.5, r);
}
// Pixel size in local units (for anti-aliasing under any scale or tilt).
fn pixelSize(p: vec2f) -> f32 {
    return max(1e-4, 0.5 * (length(dpdx(p)) + length(dpdy(p))));
}
fn coverage(d: f32, px: f32) -> f32 { return clamp(0.5 - d / px, 0.0, 1.0); }

// Gaussian-blurred rounded box (Evan Wallace, "Fast rounded rectangle shadows").
fn gaussian(x: f32, sigma: f32) -> f32 {
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (2.5066282746 * sigma);
}
fn erf2(x0: vec2f) -> vec2f {
    let s = sign(x0);
    let a = abs(x0);
    var x = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    x *= x;
    return s - s / (x * x);
}
fn boxShadowX(x: f32, y: f32, sigma: f32, corner: f32, halfSize: vec2f) -> f32 {
    let delta = min(halfSize.y - corner - abs(y), 0.0);
    let curved = halfSize.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
    let integral = 0.5 + 0.5 * erf2((x + vec2f(-curved, curved)) * (0.70710678 / sigma));
    return integral.y - integral.x;
}
fn blurredBox(rect: vec4f, corner0: f32, point0: vec2f, sigma: f32) -> f32 {
    let center = rect.xy + rect.zw * 0.5;
    let halfSize = rect.zw * 0.5;
    let corner = min(corner0, min(halfSize.x, halfSize.y));
    let point = point0 - center;
    let low = point.y - halfSize.y;
    let high = point.y + halfSize.y;
    let start = clamp(-3.0 * sigma, low, high);
    let end = clamp(3.0 * sigma, low, high);
    let step = (end - start) / 6.0;
    var y = start + step * 0.5;
    var value = 0.0;
    for (var i = 0; i < 6; i++) {
        value += boxShadowX(point.x, point.y - y, sigma, corner, halfSize) * gaussian(y, sigma) * step;
        y += step;
    }
    return value;
}
fn boxMask(rect: vec4f, corner: f32, p: vec2f, sigma: f32, px: f32) -> f32 {
    if (sigma < 0.35) { return coverage(sdRect(p, rect, vec4f(corner)), px); }
    return blurredBox(rect, corner, p, sigma);
}
fn hash(p0: vec2f) -> f32 {
    var p = fract(p0 * vec2f(443.897, 441.423));
    p += dot(p, p.yx + 19.19);
    return fract((p.x + p.y) * p.x);
}
// The clip (a rounded rect in the plane it was set in), anti-aliased.
fn clipCoverage(p: vec2f) -> f32 {
    if (u.clip.z <= 0.0) { return 1.0; }
    let q = vec3f(p, 1.0);
    let c = vec3f(dot(u.cm0.xyz, q), dot(u.cm1.xyz, q), dot(u.cm2.xyz, q));
    let cp = c.xy / c.z;
    return coverage(sdRect(cp, u.clip, u.clipr), pixelSize(cp));
}
fn halfPlane(hp: vec4f, p: vec2f, px: f32) -> f32 {
    if (hp.x == 0.0 && hp.y == 0.0) { return 1.0; }
    return clamp(0.5 + dot(hp.xyz, vec3f(p, 1.0)) / (px * length(hp.xy)), 0.0, 1.0);
}

//#section box
// ── boxes ────────────────────────────────────────────────────────────────────────────────
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var smp: sampler;

fn gradient(p: vec2f) -> vec4f {
    let a = radians(u.p0.z);
    let dir = vec2f(sin(a), -cos(a));
    let len = abs(u.rect.z * sin(a)) + abs(u.rect.w * cos(a));
    let t = dot(p - (u.rect.xy + u.rect.zw * 0.5), dir) / len + 0.5;
    let n = i32(u.p0.y);
    var c = u.stops[0];
    for (var i = 1; i < 4; i++) {
        if (i >= n) { break; }
        let p0 = u.pos[i - 1];
        let p1 = u.pos[i];
        let k = clamp((t - p0) / max(1e-5, p1 - p0), 0.0, 1.0);
        if (t > p0) { c = mix(u.stops[i - 1], u.stops[i], k); }
    }
    return c;
}

fn inset(c: vec4f, ins: vec4f, col: vec4f, p: vec2f, px: f32) -> vec4f {
    // CSS: over the background, under the border: what the shifted, shrunk, blurred box
    // leaves uncovered.
    let r = vec4f(u.rect.xy + ins.xy + ins.w, u.rect.zw - 2.0 * ins.w);
    var hole = 0.0;
    if (r.z > 0.0 && r.w > 0.0) { hole = boxMask(r, max(0.0, u.radius.x - ins.w), p, ins.z, px); }
    let s = col * (1.0 - hole);
    return s + c * (1.0 - s.a);
}

@fragment
fn fs_box(v: VOut) -> @location(0) vec4f {
    let p = v.p;
    let px = pixelSize(p);
    let uv = u.uv.xy + (p - u.rect.xy) / u.rect.zw * u.uv.zw;
    let dx = dpdx(uv);
    let dy = dpdy(uv);
    let d = sdRect(p, u.rect, u.radius);
    let feather = u.p1.y;
    var cov = select(coverage(d, px), clamp(-d / feather, 0.0, 1.0), feather > 0.0);
    cov *= clipCoverage(p);
    cov *= halfPlane(u.hp0, p, px);
    cov *= halfPlane(u.hp1, p, px);
    if (cov <= 0.0) { discard; }
    var c: vec4f;
    let fill = i32(u.p0.x);
    if (fill == 2) {
        // rotated-grid supersampling over the pixel's footprint: sharper than trilinear alone
        c = textureSampleGrad(tex, smp, uv + 0.125 * dx + 0.375 * dy, dx * 0.5, dy * 0.5);
        c += textureSampleGrad(tex, smp, uv + 0.375 * dx - 0.125 * dy, dx * 0.5, dy * 0.5);
        c += textureSampleGrad(tex, smp, uv - 0.125 * dx - 0.375 * dy, dx * 0.5, dy * 0.5);
        c += textureSampleGrad(tex, smp, uv - 0.375 * dx + 0.125 * dy, dx * 0.5, dy * 0.5);
        c *= 0.25;
    } else if (fill == 1) {
        c = gradient(p);
    } else {
        c = u.color;
    }
    if (u.in0c.a > 0.0) { c = inset(c, u.in0, u.in0c, p, px); }
    if (u.in1c.a > 0.0) { c = inset(c, u.in1, u.in1c, p, px); }
    let border = u.p0.w;
    if (border > 0.0) {
        let b = clamp(0.5 + (d + border) / px, 0.0, 1.0);
        let s = u.bcolor * b;
        c = s + c * (1.0 - s.a);
    }
    return c * cov * u.p1.x;
}

// Separable Gaussian: p0.xy = one texel along the axis (uv), p0.z = sigma in texels.
@fragment
fn fs_blur(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let uv = pos.xy / vec2f(textureDimensions(tex));
    let sigma = u.p0.z;
    if (sigma < 0.3) { return textureSampleLevel(tex, smp, uv, 0.0); }
    let r = i32(ceil(3.0 * sigma));
    var acc = vec4f(0.0);
    var wsum = 0.0;
    // pairs of taps through the bilinear filter: half the fetches
    for (var i = -r; i <= r; i += 2) {
        let x0 = f32(i);
        let x1 = f32(i + 1);
        let w0 = exp(-x0 * x0 / (2.0 * sigma * sigma));
        var w1 = 0.0;
        if (i + 1 <= r) { w1 = exp(-x1 * x1 / (2.0 * sigma * sigma)); }
        let w = w0 + w1;
        let off = x0 + w1 / w;
        acc += textureSampleLevel(tex, smp, uv + u.p0.xy * off, 0.0) * w;
        wsum += w;
    }
    return acc / wsum;
}

// A mip level from the one above (2×2 box through the bilinear filter).
@fragment
fn fs_mip(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let uv = pos.xy * 2.0 / vec2f(textureDimensions(tex));
    return textureSampleLevel(tex, smp, uv, 0.0);
}

//#section shadow
// ── shadows: p0 = (sigma, radius); cut = the element's own box (no shadow under it) ──────
@fragment
fn fs_shadow(v: VOut) -> @location(0) vec4f {
    let px = pixelSize(v.p);
    var m = boxMask(u.rect, u.p0.y, v.p, max(u.p0.x, 0.0), px);
    if (u.cut.z > 0.0) { m *= 1.0 - coverage(sdRect(v.p, u.cut, u.cutr), px); }
    m *= clipCoverage(v.p);
    return u.color * m;
}

//#section stroke
// ── strokes: a stretch [a, b] (lengths along the path) of a polyline with round caps and
// joins, or the path's fill (nonzero winding). p0 = (a, b, half width, n), p1.x = mode ──
@group(1) @binding(0) var<storage, read> segs: array<vec4f>;
@group(1) @binding(1) var<storage, read> starts: array<f32>;

@fragment
fn fs_stroke(v: VOut) -> @location(0) vec4f {
    let p = v.p;
    let px = pixelSize(p);
    let a = u.p0.x;
    let b = u.p0.y;
    let hw = u.p0.z;
    let n = u32(u.p0.w);
    let fill = u.p1.x > 0.5;
    let evenodd = u.p1.y > 0.5;
    var d = 1e9;
    var wind = 0;
    for (var i = 0u; i < n; i++) {
        let s = segs[i];
        let p0 = s.xy;
        let p1 = s.zw;
        if (fill) {
            let cr = (p1.x - p0.x) * (p.y - p0.y) - (p.x - p0.x) * (p1.y - p0.y);
            if (p0.y <= p.y) {
                if (p1.y > p.y && cr > 0.0) { wind++; }
            } else if (p1.y <= p.y && cr < 0.0) { wind--; }
            let e0 = p1 - p0;
            let t0 = clamp(dot(p - p0, e0) / dot(e0, e0), 0.0, 1.0);
            d = min(d, length(p - p0 - e0 * t0));
            continue;
        }
        let s0 = starts[i];
        let L = length(p1 - p0);
        if (b < s0 || a > s0 + L) { continue; }
        let ta = clamp((a - s0) / L, 0.0, 1.0);
        let tb = clamp((b - s0) / L, 0.0, 1.0);
        let q0 = mix(p0, p1, ta);
        let q1 = mix(p0, p1, tb);
        let e = q1 - q0;
        let ee = dot(e, e);
        var t = 0.0;
        if (ee > 0.0) { t = clamp(dot(p - q0, e) / ee, 0.0, 1.0); }
        d = min(d, length(p - q0 - e * t));
    }
    var cov: f32;
    if (fill) {
        let inside = select(wind != 0, (wind & 1) != 0, evenodd);
        cov = coverage(select(d, -d, inside), px);
    } else {
        cov = clamp(0.5 + (hw - d) / px, 0.0, 1.0);
    }
    cov *= clipCoverage(p);
    if (cov <= 0.0) { discard; }
    return u.color * cov;
}

//#section src
// ── layers ───────────────────────────────────────────────────────────────────────────────
@group(1) @binding(0) var src: texture_2d<f32>;

// A layer composited onto its parent: p0.x opacity, p0.y the mask (0 none, 1 an ellipse,
// 2 linear), in frame px (p1.z = scale).
//   ellipse: rect = centre x, y, radii x, y; p1.xy = transparent at, opaque at
//   linear: rect = from x, y, to x, y; color = the stops' alphas, pos their places, p1.w how many
@fragment
fn fs_comp(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    var c = textureLoad(src, vec2i(pos.xy), 0) * u.p0.x;
    let px = pos.xy / u.p1.z;
    if (u.p0.y > 0.5 && u.p0.y < 1.5) {
        let t = length((px - u.rect.xy) / u.rect.zw);
        c *= clamp((t - u.p1.x) / (u.p1.y - u.p1.x), 0.0, 1.0);
    } else if (u.p0.y > 1.5) {
        let d = u.rect.zw - u.rect.xy;
        let t = dot(px - u.rect.xy, d) / max(1e-6, dot(d, d));
        let n = i32(u.p1.w);
        var a = u.color[0];
        for (var i = 1; i < 4; i++) {
            if (i >= n) { break; }
            if (t > u.pos[i - 1]) {
                a = mix(u.color[i - 1], u.color[i], clamp((t - u.pos[i - 1]) / max(1e-6, u.pos[i] - u.pos[i - 1]), 0.0, 1.0));
            }
        }
        c *= clamp(a, 0.0, 1.0);
    }
    return c;
}

// A drop shadow's start: the layer's alpha, moved by p0.xy px, in u.color (premultiplied).
@fragment
fn fs_tint(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let q = vec2i(floor(pos.xy - u.p0.xy));
    let size = vec2i(textureDimensions(src));
    if (q.x < 0 || q.y < 0 || q.x >= size.x || q.y >= size.y) { return vec4f(0.0); }
    return u.color * textureLoad(src, q, 0).a;
}

@fragment
fn fs_accum(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    return textureLoad(src, vec2i(pos.xy), 0) * u.p0.x;
}

// BT.709 limited range, written straight as NV12 planes, with a triangular dither so the
// 8-bit steps never band. p0.x = seed.
fn dither(fc: vec2f) -> f32 {
    return (hash(fc + u.p0.x) + hash(fc * 1.37 + u.p0.x + 3.1) - 1.0) / 255.0;
}

@fragment
fn fs_y(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let c = clamp(textureLoad(src, vec2i(pos.xy), 0).rgb, vec3f(0.0), vec3f(1.0));
    let y = dot(c, vec3f(0.2126, 0.7152, 0.0722));
    return vec4f((16.0 + 219.0 * y) / 255.0 + dither(pos.xy), 0.0, 0.0, 1.0);
}

@fragment
fn fs_uv(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let b = vec2i(pos.xy) * 2;
    var c = textureLoad(src, b, 0).rgb + textureLoad(src, b + vec2i(1, 0), 0).rgb;
    c += textureLoad(src, b + vec2i(0, 1), 0).rgb + textureLoad(src, b + vec2i(1, 1), 0).rgb;
    c = clamp(c * 0.25, vec3f(0.0), vec3f(1.0));
    let y = dot(c, vec3f(0.2126, 0.7152, 0.0722));
    let cb = (c.b - y) / 1.8556;
    let cr = (c.r - y) / 1.5748;
    let n = dither(pos.xy);
    return vec4f((128.0 + 224.0 * cb) / 255.0 + n, (128.0 + 224.0 * cr) / 255.0 + n, 0.0, 1.0);
}

@fragment
fn fs_rgb(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    return vec4f(clamp(textureLoad(src, vec2i(pos.xy), 0).rgb, vec3f(0.0), vec3f(1.0)), 1.0);
}

// The preview window: the frame fitted into it (p0.x window px → frame px, p0.yz its
// offset), a progress bar (p0.w) along the bottom 8 px; p1 = frame w, h, window w, h.
@fragment
fn fs_present(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    if (pos.y > u.p1.w - 8.0) {
        let done = pos.x < u.p0.w * u.p1.z;
        return select(vec4f(0.16, 0.14, 0.12, 1.0), vec4f(0.76, 0.34, 0.18, 1.0), done);
    }
    let fp = (pos.xy - u.p0.yz) * u.p0.x;
    if (fp.x < 0.0 || fp.y < 0.0 || fp.x >= u.p1.x || fp.y >= u.p1.y) { return vec4f(0.08, 0.07, 0.06, 1.0); }
    return vec4f(clamp(textureLoad(src, vec2i(fp), 0).rgb, vec3f(0.0), vec3f(1.0)), 1.0);
}
