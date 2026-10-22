// A model's surfaces, shaded as three.js r137 shades a MeshStandardMaterial (the renderer the
// store-screenshot skill used): metal/rough GGX, two directional lights and an ambient one in
// three's legacy units, image-based light from its RoomEnvironment as its PMREM blurs it,
// ACES filmic tone mapping, sRGB out. The screen is unlit: its image as it is.
diagnostic(off, derivative_uniformity);

const PI: f32 = 3.141592653589793;

struct MU {
    vp: mat4x4f,       // world → clip
    eye: vec4f,        // the camera; w = exposure
    base: vec4f,       // base colour (linear) and alpha
    mr: vec4f,         // metalness, roughness, normal scale, alpha cutoff
    emissive: vec4f,
    has: vec4f,        // textures: base colour, metal/rough, normal, emissive
    mode: vec4f,       // x: 1 the screen; y: alpha 0 opaque, 1 mask, 2 blend; z: flat; w: double-sided
    uv0: vec4f,        // the screen image's UV transform (rows); uv0.w: u wraps
    uv1: vec4f,
}

@group(0) @binding(0) var<uniform> mu: MU;
@group(1) @binding(0) var t_base: texture_2d<f32>;
@group(1) @binding(1) var t_mr: texture_2d<f32>;
@group(1) @binding(2) var t_nrm: texture_2d<f32>;
@group(1) @binding(3) var t_em: texture_2d<f32>;
@group(1) @binding(4) var smp: sampler;
@group(1) @binding(5) var smp_clamp: sampler;
@group(2) @binding(0) var env: texture_2d_array<f32>;
@group(2) @binding(1) var env_smp: sampler;

struct VIn {
    @location(0) p: vec3f,
    @location(1) n: vec3f,
    @location(2) uv: vec2f,
}

struct VOut {
    @builtin(position) pos: vec4f,
    @location(0) w: vec3f,
    @location(1) n: vec3f,
    @location(2) uv: vec2f,
}

@vertex
fn vs(v: VIn) -> VOut {
    var o: VOut;
    o.pos = mu.vp * vec4f(v.p, 1.0);
    o.w = v.p;
    o.n = v.n;
    o.uv = v.uv;
    return o;
}

fn to_linear(c: vec3f) -> vec3f {
    return select(pow((c + 0.055) / 1.055, vec3f(2.4)), c / 12.92, c <= vec3f(0.04045));
}
fn to_srgb(c: vec3f) -> vec3f {
    return select(1.055 * pow(c, vec3f(0.41666)) - 0.055, c * 12.92, c <= vec3f(0.0031308));
}

fn aces(c0: vec3f, exposure: f32) -> vec3f {
    let input = mat3x3f(vec3f(0.59719, 0.07600, 0.02840), vec3f(0.35458, 0.90834, 0.13383), vec3f(0.04823, 0.01566, 0.83777));
    let output = mat3x3f(vec3f(1.60475, -0.10208, -0.00327), vec3f(-0.53108, 1.10813, -0.07276), vec3f(-0.07367, -0.00605, 1.07602));
    var c = c0 * exposure / 0.6;
    c = input * c;
    let a = c * (c + 0.0245786) - 0.000090537;
    let b = c * (0.983729 * c + 0.4329510) + 0.238081;
    c = output * (a / b);
    return clamp(c, vec3f(0.0), vec3f(1.0));
}

// ── three's BRDF ──────────────────────────────────────────────────────────────────────────────
fn f_schlick(f0: vec3f, f90: f32, vh: f32) -> vec3f {
    let fr = exp2((-5.55473 * vh - 6.98316) * vh);
    return f0 * (1.0 - fr) + f90 * fr;
}
fn ggx(l: vec3f, v: vec3f, n: vec3f, f0: vec3f, rough: f32) -> vec3f {
    let alpha = rough * rough;
    let h = normalize(l + v);
    let nl = clamp(dot(n, l), 0.0, 1.0);
    let nv = clamp(dot(n, v), 0.0, 1.0);
    let nh = clamp(dot(n, h), 0.0, 1.0);
    let vh = clamp(dot(v, h), 0.0, 1.0);
    let f = f_schlick(f0, 1.0, vh);
    let a2 = alpha * alpha;
    let gv = nl * sqrt(a2 + (1.0 - a2) * nv * nv);
    let gl = nv * sqrt(a2 + (1.0 - a2) * nl * nl);
    let vis = 0.5 / max(gv + gl, 1e-6);
    let den = nh * nh * (a2 - 1.0) + 1.0;
    let d = a2 / (PI * den * den);
    return f * (vis * d);
}
fn dfg(n: vec3f, v: vec3f, rough: f32) -> vec2f {
    let nv = clamp(dot(n, v), 0.0, 1.0);
    let c0 = vec4f(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4f(1.0, 0.0425, 1.04, -0.04);
    let r = rough * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * nv)) * r.x + r.y;
    return vec2f(-1.04, 1.04) * a004 + r.zw;
}

// ── the environment: three's PMREM levels as an equirectangular array ────────────────────────
fn env_uv(d: vec3f) -> vec2f {
    return vec2f(atan2(d.x, -d.z) / (2.0 * PI) + 0.5, acos(clamp(d.y, -1.0, 1.0)) / PI);
}
fn env_level(d: vec3f, i: f32) -> vec3f {
    return textureSampleLevel(env, env_smp, env_uv(d), i32(clamp(i, 0.0, 10.0)), 0.0).rgb;
}
fn rough_to_mip(r: f32) -> f32 {
    if (r >= 0.8) { return (1.0 - r) * (-1.0 - -2.0) / (1.0 - 0.8) + -2.0; }
    if (r >= 0.4) { return (0.8 - r) * (2.0 - -1.0) / (0.8 - 0.4) + -1.0; }
    if (r >= 0.305) { return (0.4 - r) * (3.0 - 2.0) / (0.4 - 0.305) + 2.0; }
    if (r >= 0.21) { return (0.305 - r) * (4.0 - 3.0) / (0.305 - 0.21) + 3.0; }
    return -2.0 * log2(1.16 * r);
}
// textureCubeUV: "mip" 8 is the sharpest level (index 0), −2 the blurriest (index 10)
fn env_at(d: vec3f, rough: f32) -> vec3f {
    let mip = clamp(rough_to_mip(rough), -2.0, 8.0);
    let mi = floor(mip);
    let mf = mip - mi;
    let c0 = env_level(d, 8.0 - mi);
    if (mf == 0.0) { return c0; }
    let c1 = env_level(d, 8.0 - (mi + 1.0));
    return mix(c0, c1, mf);
}

@fragment
fn fs(v: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    if (mu.mode.x > 0.5) {
        var uv = vec2f(dot(mu.uv0.xyz, vec3f(v.uv, 1.0)), dot(mu.uv1.xyz, vec3f(v.uv, 1.0)));
        if (mu.uv0.w > 0.5) { uv.x = fract(uv.x); }
        let c = textureSample(t_base, smp_clamp, uv);
        return vec4f(c.rgb, 1.0);
    }
    var base = mu.base;
    if (mu.has.x > 0.5) {
        let t = textureSample(t_base, smp, v.uv);
        base *= vec4f(to_linear(t.rgb), t.a);
    }
    let alpha_mode = mu.mode.y;
    if (alpha_mode > 0.5 && alpha_mode < 1.5 && base.a < mu.mr.w) { discard; }
    var metal = mu.mr.x;
    var rough = mu.mr.y;
    if (mu.has.y > 0.5) {
        let t = textureSample(t_mr, smp, v.uv);
        rough *= t.g;
        metal *= t.b;
    }
    let face = select(-1.0, 1.0, front);
    var gn: vec3f;
    if (mu.mode.z > 0.5) {
        // (WGSL's y runs down: the other order than GL's)
        gn = normalize(cross(dpdy(v.w), dpdx(v.w)));
    } else {
        gn = normalize(v.n);
        if (mu.mode.w > 0.5) { gn *= face; }
    }
    let dxy = max(abs(dpdx(gn)), abs(dpdy(gn)));
    let geo_rough = max(max(dxy.x, dxy.y), dxy.z);
    var n = gn;
    if (mu.has.z > 0.5) {
        // three's perturbNormal2Arb (with glTF's normal scale (s, −s); y down negates T and B)
        var mn = textureSample(t_nrm, smp, v.uv).xyz * 2.0 - 1.0;
        mn = vec3f(mn.x * mu.mr.z, -mn.y * mu.mr.z, mn.z);
        let q0 = dpdx(v.w);
        let q1 = dpdy(v.w);
        let st0 = dpdx(v.uv);
        let st1 = dpdy(v.uv);
        let q1perp = cross(q1, gn);
        let q0perp = cross(gn, q0);
        let t = q1perp * st0.x + q0perp * st1.x;
        let b = q1perp * st0.y + q0perp * st1.y;
        let det = max(dot(t, t), dot(b, b));
        var s = 0.0;
        if (det > 0.0) { s = -face * inverseSqrt(det); }
        n = normalize(t * (mn.x * s) + b * (mn.y * s) + gn * mn.z);
    }
    rough = min(max(rough, 0.0525) + geo_rough, 1.0);
    let view = normalize(mu.eye.xyz - v.w);
    let diffuse = base.rgb * (1.0 - metal);
    let spec = mix(vec3f(0.04), base.rgb, metal);

    // direct: three's key and fill (legacy units: irradiance × π)
    var dd = vec3f(0.0);
    var ds = vec3f(0.0);
    let lights = array<vec4f, 2>(vec4f(normalize(vec3f(4.0, 6.0, 6.0)), 2.4), vec4f(normalize(vec3f(-5.0, 2.0, 3.0)), 0.8));
    for (var i = 0; i < 2; i++) {
        let l = lights[i].xyz;
        let irr = clamp(dot(n, l), 0.0, 1.0) * lights[i].w * PI;
        dd += irr * diffuse / PI;
        ds += irr * ggx(l, view, n, spec, rough);
    }
    // ambient 0.3
    var id = 0.3 * diffuse;
    // the room
    let ibl_irr = PI * env_at(n, 1.0);
    var r = reflect(-view, n);
    r = normalize(mix(r, n, rough * rough));
    let radiance = env_at(r, rough);
    let fab = dfg(n, view, rough);
    let fss = spec * fab.x + fab.y;
    let ess = fab.x + fab.y;
    let ems = 1.0 - ess;
    let favg = spec + (1.0 - spec) * 0.047619;
    let fms = fss * favg / (1.0 - ems * favg);
    let multi = fms * ems;
    let cos_irr = ibl_irr / PI;
    let is = radiance * fss + multi * cos_irr;
    id += diffuse * (1.0 - (fss + multi)) * cos_irr;
    var em = mu.emissive.rgb;
    if (mu.has.w > 0.5) { em *= to_linear(textureSample(t_em, smp, v.uv).rgb); }
    let out = dd + id + ds + is + em;
    let c = to_srgb(aces(out, mu.eye.w));
    var a = 1.0;
    if (alpha_mode > 1.5) { a = base.a; }
    return vec4f(c, a);
}
