//! Custom shaders: a scene's WGSL `fn shade(p: vec2f, px: vec2f) -> vec4f`, wrapped into a
//! module that fills a rect under the current transform (`cv.shader`). See docs/shaders.md.
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use crate::script::{resolve, shown};

/// A shader's whole module and its identity (the pipeline cache key).
pub struct Code {
    pub id: u64,
    pub wgsl: String,
    /// where it came from, for messages
    pub name: String,
}

/// The uniform block a custom shader reads (in 16-byte slots).
pub const SLOT_RECT: usize = 4;
pub const SLOT_VIEW: usize = 5;
pub const SLOT_CLIP: usize = 6;
pub const SLOT_CLIPR: usize = 7;
pub const SLOT_CM: usize = 8;
pub const SLOT_P: usize = 11;

const PREFIX: &str = r#"diagnostic(off, derivative_uniformity);
struct U {
    h0: vec4f, h1: vec4f, h2: vec4f,
    quad: vec4f,
    rect: vec4f,
    view: vec4f,          // half the design size, scale, frame
    clip: vec4f, clipr: vec4f,
    cm0: vec4f, cm1: vec4f, cm2: vec4f,
    p: array<vec4f, 16>,
}
@group(0) @binding(0) var<uniform> u: U;
struct VOut {
    @builtin(position) pos: vec4f,
    @location(0) p: vec2f,
}
@vertex
fn reel_vs(@builtin(vertex_index) vi: u32) -> VOut {
    let unit = vec2f(f32(vi & 1u), f32(vi >> 1u));
    let p = u.quad.xy + unit * u.quad.zw;
    let q = vec3f(p, 1.0);
    let s = vec3f(dot(u.h0.xyz, q), dot(u.h1.xyz, q), dot(u.h2.xyz, q));
    var o: VOut;
    o.pos = vec4f(s.x / u.view.x - s.z, s.z - s.y / u.view.y, 0.0, s.z);
    o.p = p;
    return o;
}

// ── what a shader can use ──────────────────────────────────────────────────────────────────
struct Reel {
    frame: f32,       // the frame being drawn (fractional between frames with motion blur)
    scale: f32,       // output pixels per design pixel
    size: vec2f,      // the design size
    rect: vec4f,      // the rect being filled: x, y, w, h
}
var<private> reel: Reel;

// params[4i .. 4i + 4] as given to cv.shader()
fn param(i: i32) -> vec4f { return u.p[i]; }

// 0…1 from a 2D point (white noise)
fn hash(p0: vec2f) -> f32 {
    var p = fract(p0 * vec2f(443.897, 441.423));
    p += dot(p, p.yx + 19.19);
    return fract((p.x + p.y) * p.x);
}
// 0…1 from a whole-number point, with no pattern (a PCG hash): per-pixel noise, grain
fn ihash(p: vec2f) -> f32 {
    var v = vec2u(vec2i(floor(p)) + vec2i(65536));
    v = v * 1664525u + 1013904223u;
    v.x += v.y * 1664525u;
    v.y += v.x * 1664525u;
    v ^= v >> vec2u(16u);
    v.x += v.y * 1664525u;
    v.y += v.x * 1664525u;
    v ^= v >> vec2u(16u);
    return f32(v.x) / 4294967295.0;
}
fn reel_grad(i: vec2f) -> vec2f {
    let a = 6.2831853 * hash(i + 17.0);
    return vec2f(cos(a), sin(a));
}
// gradient noise, about -0.7…0.7, smooth, one feature per unit
fn noise(p: vec2f) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let w = f * f * (3.0 - 2.0 * f);
    let a = dot(reel_grad(i), f);
    let b = dot(reel_grad(i + vec2f(1.0, 0.0)), f - vec2f(1.0, 0.0));
    let c = dot(reel_grad(i + vec2f(0.0, 1.0)), f - vec2f(0.0, 1.0));
    let d = dot(reel_grad(i + vec2f(1.0, 1.0)), f - vec2f(1.0, 1.0));
    return mix(mix(a, b, w.x), mix(c, d, w.x), w.y);
}
// fractal noise: `octaves` of noise, each twice the frequency and half the amplitude
fn fbm(p: vec2f, octaves: i32) -> f32 {
    var s = 0.0;
    var amp = 1.0;
    var q = p;
    for (var i = 0; i < octaves; i++) {
        s += amp * noise(q);
        q *= 2.0;
        amp *= 0.5;
    }
    return s;
}
// signed distance to a rounded box centred at 0 (half size b, radii r: tl, tr, br, bl)
fn sdRoundBox(p: vec2f, b: vec2f, r: vec4f) -> f32 {
    let left = select(r.w, r.x, p.y < 0.0);
    let right = select(r.z, r.y, p.y < 0.0);
    var rr = select(right, left, p.x < 0.0);
    rr = min(rr, min(b.x, b.y));
    let q = abs(p) - b + rr;
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2f(0.0))) - rr;
}
fn reel_px(p: vec2f) -> f32 { return max(1e-4, 0.5 * (length(dpdx(p)) + length(dpdy(p)))); }
fn reel_cov(p: vec2f, rect: vec4f, r: vec4f) -> f32 {
    let d = sdRoundBox(p - (rect.xy + rect.zw * 0.5), rect.zw * 0.5, r);
    return clamp(0.5 - d / reel_px(p), 0.0, 1.0);
}
// ── the scene's shader ─────────────────────────────────────────────────────────────────────
"#;

const SUFFIX: &str = r#"
// ───────────────────────────────────────────────────────────────────────────────────────────
@fragment
fn reel_fs(v: VOut) -> @location(0) vec4f {
    reel = Reel(u.view.w, u.view.z, u.view.xy * 2.0, u.rect);
    var cov = reel_cov(v.p, u.rect, vec4f(0.0));
    if (u.clip.z > 0.0) {
        let q = vec3f(v.p, 1.0);
        let c = vec3f(dot(u.cm0.xyz, q), dot(u.cm1.xyz, q), dot(u.cm2.xyz, q));
        cov *= reel_cov(c.xy / c.z, u.clip, u.clipr);
    }
    if (cov <= 0.0) { discard; }
    let c = shade(v.p, v.pos.xy);
    return vec4f(c.rgb * c.a, c.a) * cov;
}
"#;

/// The number of lines before the scene's code in the module (to point errors at its lines).
pub fn prefix_lines() -> usize {
    PREFIX.lines().count()
}

type Files = Mutex<HashMap<PathBuf, (SystemTime, Arc<Code>)>>;
static FILES: LazyLock<Files> = LazyLock::new(Default::default);
static INLINE: LazyLock<Mutex<HashMap<String, Arc<Code>>>> = LazyLock::new(Default::default);

fn build(user: &str, name: String) -> Arc<Code> {
    let wgsl = format!("{PREFIX}{user}\n{SUFFIX}");
    let mut h = std::collections::hash_map::DefaultHasher::new();
    wgsl.hash(&mut h);
    Arc::new(Code { id: h.finish(), wgsl, name })
}

/// cv.shader()'s `src`: WGSL source (it has `fn shade`) or a .wgsl file.
pub fn source(src: &str) -> Result<Arc<Code>, String> {
    if src.contains("fn shade") {
        let mut c = INLINE.lock().unwrap();
        if let Some(code) = c.get(src) {
            return Ok(code.clone());
        }
        let code = build(src, "inline shader".into());
        c.insert(src.to_string(), code.clone());
        return Ok(code);
    }
    let p = resolve(src);
    let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).map_err(|e| format!("shader {}: {e}", shown(&p)))?;
    if let Some((t, code)) = FILES.lock().unwrap().get(&p)
        && *t == mtime
    {
        return Ok(code.clone());
    }
    let text = std::fs::read_to_string(&p).map_err(|e| format!("shader {}: {e}", shown(&p)))?;
    if !text.contains("fn shade") {
        return Err(format!("shader {}: no `fn shade(p: vec2f, px: vec2f) -> vec4f` in it", shown(&p)));
    }
    let code = build(&text, shown(&p));
    FILES.lock().unwrap().insert(p, (mtime, code.clone()));
    Ok(code)
}

/// The shader files loaded so far (the preview watches them).
pub fn watched() -> Vec<PathBuf> {
    FILES.lock().unwrap().keys().cloned().collect()
}

/// Checks a shader compiles (naga), with errors pointing at the scene's own lines.
pub fn validate(code: &Code) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(&code.wgsl).map_err(|e| relines(&e.emit_to_string(&code.wgsl), &code.name))?;
    let mut v = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all());
    v.validate(&module).map_err(|e| relines(&format!("{e:?}"), &code.name))?;
    Ok(())
}

/// Rewrites "wgsl:LINE:COL" in naga's messages to the scene file's own line numbers.
fn relines(msg: &str, name: &str) -> String {
    let skip = prefix_lines();
    let mut out = String::new();
    for line in msg.lines() {
        let mut l = line.to_string();
        if let Some(i) = l.find("wgsl:") {
            let rest = &l[i + 5..];
            let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(v) = n.parse::<usize>() {
                let fixed = if v > skip { format!("{name}:{}", v - skip) } else { format!("(reel's prelude):{v}") };
                l = format!("{}{fixed}{}", &l[..i], &rest[n.len()..]);
            }
        }
        out.push_str(&l);
        out.push('\n');
    }
    out
}
