// The studio's light at one blur: the room (an equirectangular map with mips) convolved with
// a Gaussian on the sphere of `sigma` radians, as three's PMREM blurs each level.
const PI: f32 = 3.141592653589793;

struct EU {
    p: vec4f, // sigma (radians), the source mip to read, output width, height
}

@group(0) @binding(0) var<uniform> eu: EU;
@group(1) @binding(0) var base: texture_2d<f32>;
@group(1) @binding(1) var smp: sampler;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4f {
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    return vec4f(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
}

fn dir_of(uv: vec2f) -> vec3f {
    let phi = (uv.x - 0.5) * 2.0 * PI;
    let theta = uv.y * PI;
    return vec3f(sin(theta) * sin(phi), cos(theta), -sin(theta) * cos(phi));
}
fn uv_of(d: vec3f) -> vec2f {
    return vec2f(atan2(d.x, -d.z) / (2.0 * PI) + 0.5, acos(clamp(d.y, -1.0, 1.0)) / PI);
}

@fragment
fn fs_env(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let d = dir_of(pos.xy / eu.p.zw);
    let sigma = eu.p.x;
    // a frame around d
    var up = vec3f(0.0, 1.0, 0.0);
    if (abs(d.y) > 0.99) { up = vec3f(1.0, 0.0, 0.0); }
    let t = normalize(cross(up, d));
    let b = cross(d, t);
    let tmax = min(3.0 * sigma, PI);
    let nt = 32;
    let np = 64;
    var acc = vec3f(0.0);
    var wsum = 0.0;
    for (var i = 0; i < nt; i++) {
        let th = (f32(i) + 0.5) / f32(nt) * tmax;
        let w = exp(-th * th / (2.0 * sigma * sigma)) * sin(th);
        for (var j = 0; j < np; j++) {
            let ph = (f32(j) + 0.5 * f32(i % 2)) / f32(np) * 2.0 * PI;
            let s = cos(th) * d + sin(th) * (cos(ph) * t + sin(ph) * b);
            acc += textureSampleLevel(base, smp, uv_of(s), eu.p.y).rgb * w;
            wsum += w;
        }
    }
    return vec4f(acc / max(wsum, 1e-12), 1.0);
}
