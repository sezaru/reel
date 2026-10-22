//! 3D models (glTF 2.0): a phone, a tablet, a laptop, lit the way three.js lights a model in a
//! studio (its RoomEnvironment, two lights, ACES), with an image on the device's screen.
//!
//! The scene's thread loads a model to frame it: where the camera goes for a yaw and pitch,
//! where the device lands in its rect, where its screen is (so a scene can draw on it). The
//! GPU thread renders it into a texture (gpu/mesh.rs) that is then drawn like an image, so
//! transforms, clips, groups and motion blur apply to it as to anything else.
//!
//! The framing is the store-screenshot skill's three.js renderer's (`render_phone.py`), line
//! for line, so its stage layouts keep their numbers.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use crate::canvas::TexKey;
use crate::gpu::Decoded;

pub type V3 = [f64; 3];
pub type M4 = [[f64; 4]; 4];

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l > 0.0 { scale(a, 1.0 / l) } else { a }
}
/// `v` turned by `angle` (radians) about the unit `axis`, right-handed (three's applyAxisAngle).
fn rotate(v: V3, axis: V3, angle: f64) -> V3 {
    let (s, c) = angle.sin_cos();
    add(add(scale(v, c), scale(cross(axis, v), s)), scale(axis, dot(axis, v) * (1.0 - c)))
}

/// Column-major (glTF) 4×4 → row-major rows.
fn rows(m: [[f32; 4]; 4]) -> M4 {
    let mut r = [[0.0; 4]; 4];
    for (c, col) in m.iter().enumerate() {
        for (i, v) in col.iter().enumerate() {
            r[i][c] = *v as f64;
        }
    }
    r
}
fn mul4(a: &M4, b: &M4) -> M4 {
    let mut m = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            m[i][j] = (0..4).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}
fn point(m: &M4, p: V3) -> V3 {
    let w = m[3][0] * p[0] + m[3][1] * p[1] + m[3][2] * p[2] + m[3][3];
    [0, 1, 2].map(|i| (m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2] + m[i][3]) / w)
}
/// The 3×3 normal matrix (inverse transpose) of an affine 4×4, as rows.
fn normal_matrix(m: &M4) -> [[f64; 3]; 3] {
    let a = [[m[0][0], m[0][1], m[0][2]], [m[1][0], m[1][1], m[1][2]], [m[2][0], m[2][1], m[2][2]]];
    let c = |r: usize, k: usize| {
        let (r1, r2, k1, k2) = ((r + 1) % 3, (r + 2) % 3, (k + 1) % 3, (k + 2) % 3);
        a[r1][k1] * a[r2][k2] - a[r1][k2] * a[r2][k1]
    };
    // the cofactor matrix is det · inverse transpose: the scale drops out when normalised, the
    // sign must not (a mirroring transform would turn every normal inside out)
    let k = if det3(m) < 0.0 { -1.0 } else { 1.0 };
    [[c(0, 0) * k, c(0, 1) * k, c(0, 2) * k], [c(1, 0) * k, c(1, 1) * k, c(1, 2) * k], [c(2, 0) * k, c(2, 1) * k, c(2, 2) * k]]
}
fn det3(m: &M4) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

// ── the model ────────────────────────────────────────────────────────────────────────────────
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vert {
    pub p: [f32; 3],
    pub n: [f32; 3],
    pub uv: [f32; 2],
}

pub struct Prim {
    /// in world space (the node transforms baked in)
    pub verts: Vec<Vert>,
    pub idx: Vec<u32>,
    pub mat: usize,
    /// no normals in the file: shaded flat
    pub flat: bool,
    /// the centre of its box, for sorting the see-through ones
    pub center: V3,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Alpha {
    Opaque,
    Mask(f32),
    Blend,
}

pub struct Material {
    pub base: [f32; 4],
    pub base_tex: Option<usize>,
    pub metal: f32,
    pub rough: f32,
    pub mr_tex: Option<usize>,
    pub normal_tex: Option<usize>,
    pub normal_scale: f32,
    pub emissive: [f32; 3],
    pub emissive_tex: Option<usize>,
    pub alpha: Alpha,
    pub double: bool,
}

/// The device's screen: the mesh an image goes on, and its axes in the world.
pub struct Screen {
    pub prim: usize,
    /// outward
    pub normal: V3,
    pub up: V3,
    pub right: V3,
    /// width / height, portrait
    pub aspect: f64,
    /// < 0: the mesh winds the other way, so the image would come out mirrored
    pub handed: f64,
    /// the screen's corners: (right min, up max), (right max, up max), (right max, up min),
    /// (right min, up min), on its outer face
    pub corners: [V3; 4],
}

pub struct Model {
    pub path: Arc<str>,
    pub prims: Vec<Prim>,
    pub mats: Vec<Material>,
    /// straight RGBA8
    pub images: Vec<Arc<Decoded>>,
    /// three's Box3.setFromObject: each mesh's own box, its corners moved into the world
    pub min: V3,
    pub max: V3,
    pub screen: Option<Screen>,
}

type Models = Mutex<HashMap<PathBuf, (std::time::SystemTime, Arc<Model>)>>;
static MODELS: LazyLock<Models> = LazyLock::new(Default::default);

/// A model from a .gltf (with its .bin and images) or .glb file, loaded once per process.
pub fn load(path: &Path) -> Result<Arc<Model>, String> {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some((t, m)) = MODELS.lock().unwrap().get(path)
        && *t == mtime
    {
        return Ok(m.clone());
    }
    let m = Arc::new(read(path).map_err(|e| format!("{}: {e}", path.display()))?);
    MODELS.lock().unwrap().insert(path.to_path_buf(), (mtime, m.clone()));
    Ok(m)
}

/// A model already loaded (the GPU thread asks for what a scene loaded).
pub fn loaded(path: &str) -> Option<Arc<Model>> {
    MODELS.lock().unwrap().get(Path::new(path)).map(|(_, m)| m.clone())
}

fn base64(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' => continue,
            _ => return Err("bad base64 in a data URI".into()),
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

fn uri_bytes(dir: &Path, uri: &str) -> Result<Vec<u8>, String> {
    if let Some(rest) = uri.strip_prefix("data:") {
        let (_, data) = rest.split_once(";base64,").ok_or("a data URI that isn't base64")?;
        return base64(data);
    }
    // percent-decoded, relative to the model's file
    let mut name = Vec::new();
    let b = uri.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&uri[i + 1..i + 3], 16)
        {
            name.push(v);
            i += 3;
            continue;
        }
        name.push(b[i]);
        i += 1;
    }
    let p = dir.join(String::from_utf8_lossy(&name).as_ref());
    std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))
}

fn read(path: &Path) -> Result<Model, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let g = gltf::Gltf::from_slice(&bytes).map_err(|e| e.to_string())?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut bufs = Vec::new();
    for b in g.document.buffers() {
        bufs.push(match b.source() {
            gltf::buffer::Source::Bin => g.blob.clone().ok_or("a .glb without its binary chunk")?,
            gltf::buffer::Source::Uri(u) => uri_bytes(dir, u)?,
        });
    }
    let mut images = Vec::new();
    for im in g.document.images() {
        let data = match im.source() {
            gltf::image::Source::View { view, .. } => {
                let b = &bufs[view.buffer().index()];
                b[view.offset()..view.offset() + view.length()].to_vec()
            }
            gltf::image::Source::Uri { uri, .. } => uri_bytes(dir, uri)?,
        };
        let d = crate::image::bytes(&data, false).map_err(|e| format!("image {}: {e}", im.index()))?;
        images.push(Arc::new(d));
    }
    let tex = |t: Option<gltf::texture::Texture>| t.map(|t| t.source().index());
    let mut mats: Vec<Material> = g
        .document
        .materials()
        .map(|m| {
            let pbr = m.pbr_metallic_roughness();
            Material {
                base: pbr.base_color_factor(),
                base_tex: tex(pbr.base_color_texture().map(|t| t.texture())),
                metal: pbr.metallic_factor(),
                rough: pbr.roughness_factor(),
                mr_tex: tex(pbr.metallic_roughness_texture().map(|t| t.texture())),
                normal_tex: tex(m.normal_texture().map(|t| t.texture())),
                normal_scale: m.normal_texture().map(|t| t.scale()).unwrap_or(1.0),
                emissive: m.emissive_factor(),
                emissive_tex: tex(m.emissive_texture().map(|t| t.texture())),
                alpha: match m.alpha_mode() {
                    gltf::material::AlphaMode::Opaque => Alpha::Opaque,
                    gltf::material::AlphaMode::Mask => Alpha::Mask(m.alpha_cutoff().unwrap_or(0.5)),
                    gltf::material::AlphaMode::Blend => Alpha::Blend,
                },
                double: m.double_sided(),
            }
        })
        .collect();
    // glTF's default material
    let default_mat = mats.len();
    mats.push(Material {
        base: [1.0; 4],
        base_tex: None,
        metal: 1.0,
        rough: 1.0,
        mr_tex: None,
        normal_tex: None,
        normal_scale: 1.0,
        emissive: [0.0; 3],
        emissive_tex: None,
        alpha: Alpha::Opaque,
        double: false,
    });

    let scene = g.document.default_scene().or_else(|| g.document.scenes().next()).ok_or("no scene")?;
    let mut prims = Vec::new();
    let (mut min, mut max) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    let mut screen_at: Option<(usize, M4)> = None;
    let mut stack: Vec<(gltf::Node, M4)> = scene.nodes().map(|n| (n, [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]])).collect();
    stack.reverse();
    // depth first, in document order (which mesh is "the screen" when several match: the last)
    while let Some((node, parent)) = stack.pop() {
        let world = mul4(&parent, &rows(node.transform().matrix()));
        if let Some(mesh) = node.mesh() {
            let nm = normal_matrix(&world);
            let flip = det3(&world) < 0.0;
            for p in mesh.primitives() {
                if p.mode() != gltf::mesh::Mode::Triangles {
                    continue;
                }
                let r = p.reader(|b| Some(&bufs[b.index()]));
                let Some(pos) = r.read_positions() else { continue };
                let local: Vec<[f32; 3]> = pos.collect();
                let normals: Option<Vec<[f32; 3]>> = r.read_normals().map(|n| n.collect());
                let uvs: Option<Vec<[f32; 2]>> = r.read_tex_coords(0).map(|t| t.into_f32().collect());
                let mut idx: Vec<u32> = match r.read_indices() {
                    Some(i) => i.into_u32().collect(),
                    None => (0..local.len() as u32).collect(),
                };
                if flip {
                    for t in idx.chunks_mut(3) {
                        t.swap(1, 2);
                    }
                }
                // three: the mesh's own box, its 8 corners into the world
                let (mut lmin, mut lmax) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
                for v in &local {
                    for a in 0..3 {
                        lmin[a] = lmin[a].min(v[a] as f64);
                        lmax[a] = lmax[a].max(v[a] as f64);
                    }
                }
                let mut cmin = [f64::INFINITY; 3];
                let mut cmax = [f64::NEG_INFINITY; 3];
                for c in 0..8 {
                    let q = [if c & 1 == 0 { lmin[0] } else { lmax[0] }, if c & 2 == 0 { lmin[1] } else { lmax[1] }, if c & 4 == 0 { lmin[2] } else { lmax[2] }];
                    let w = point(&world, q);
                    for a in 0..3 {
                        min[a] = min[a].min(w[a]);
                        max[a] = max[a].max(w[a]);
                        cmin[a] = cmin[a].min(w[a]);
                        cmax[a] = cmax[a].max(w[a]);
                    }
                }
                let verts = local
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let w = point(&world, [v[0] as f64, v[1] as f64, v[2] as f64]);
                        let n = normals.as_ref().map(|n| {
                            let n = n[i].map(|x| x as f64);
                            norm([dot(nm[0], n), dot(nm[1], n), dot(nm[2], n)])
                        });
                        Vert {
                            p: w.map(|x| x as f32),
                            n: n.unwrap_or([0.0, 0.0, 1.0]).map(|x| x as f32),
                            uv: uvs.as_ref().map(|u| u[i]).unwrap_or([0.0, 0.0]),
                        }
                    })
                    .collect();
                let mat = p.material().index().unwrap_or(default_mat);
                let mname = p.material().name().unwrap_or("");
                let name = format!("{} {} {}", node.name().unwrap_or(""), mesh.name().unwrap_or(""), mname);
                let lower = name.to_lowercase();
                if ["screen", "display", "wallpaper"].iter().any(|k| lower.contains(k)) && !["bezel", "glass"].iter().any(|k| lower.contains(k)) {
                    screen_at = Some((prims.len(), world));
                }
                prims.push(Prim { verts, idx, mat, flat: normals.is_none(), center: scale(add(cmin, cmax), 0.5) });
            }
        }
        let kids: Vec<_> = node.children().collect();
        for k in kids.into_iter().rev() {
            stack.push((k, world));
        }
    }
    if prims.is_empty() {
        return Err("no triangles".into());
    }
    let mut m = Model { path: Arc::from(path.display().to_string()), prims, mats, images, min, max, screen: None };
    if let Some((i, world)) = screen_at {
        m.screen = Some(screen(&mut m, i, &world));
    }
    Ok(m)
}

/// three's analyzeScreen: the screen mesh's thinnest axis is its normal, its longest "up", the
/// other "right"; clean 0…1 UVs from its own geometry (the baked ones are unreliable).
fn screen(m: &mut Model, i: usize, world: &M4) -> Screen {
    let inv = invert(world);
    let local: Vec<V3> = m.prims[i].verts.iter().map(|v| point(&inv, v.p.map(|x| x as f64))).collect();
    let (mut mn, mut mx) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for p in &local {
        for a in 0..3 {
            mn[a] = mn[a].min(p[a]);
            mx[a] = mx[a].max(p[a]);
        }
    }
    let ext = [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]];
    let mut ord = [0, 1, 2];
    ord.sort_by(|a, b| ext[*a].partial_cmp(&ext[*b]).unwrap());
    let (na, upa, ra) = (ord[0], ord[2], ord[1]);
    for (v, p) in m.prims[i].verts.iter_mut().zip(&local) {
        v.uv = [((p[ra] - mn[ra]) / ext[ra]) as f32, (1.0 - (p[upa] - mn[upa]) / ext[upa]) as f32];
    }
    let nm = normal_matrix(world);
    let axis = |a: usize| norm([nm[0][a], nm[1][a], nm[2][a]]);
    let mut normal = axis(na);
    let center = point(world, scale(add(mn, mx), 0.5));
    let body = scale(add(m.min, m.max), 0.5);
    let out = dot(normal, sub(center, body)) >= 0.0;
    if !out {
        normal = scale(normal, -1.0);
    }
    let (up, right) = (axis(upa), axis(ra));
    let handed = dot(cross(right, up), normal);
    // the outer face's corners
    let face = if out { mx[na] } else { mn[na] };
    let corner = |r: f64, u: f64| {
        let mut q = [0.0; 3];
        q[na] = face;
        q[ra] = r;
        q[upa] = u;
        point(world, q)
    };
    let corners = [corner(mn[ra], mx[upa]), corner(mx[ra], mx[upa]), corner(mx[ra], mn[upa]), corner(mn[ra], mn[upa])];
    Screen { prim: i, normal, up, right, aspect: ext[ra] / ext[upa], handed, corners }
}

fn invert(m: &M4) -> M4 {
    // affine: [A t; 0 1] → [A⁻¹ −A⁻¹t; 0 1]
    let c = normal_matrix(m); // cofactors, signed as det: A⁻¹ = cofactorᵀ / |det|
    let d = det3(m).abs();
    let k = if d.abs() < 1e-300 { 0.0 } else { 1.0 / d };
    let ai = [[c[0][0] * k, c[1][0] * k, c[2][0] * k], [c[0][1] * k, c[1][1] * k, c[2][1] * k], [c[0][2] * k, c[1][2] * k, c[2][2] * k]];
    let t = [m[0][3], m[1][3], m[2][3]];
    let mut r = [[0.0; 4]; 4];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = ai[i][j];
        }
        r[i][3] = -dot(ai[i], t);
    }
    r[3][3] = 1.0;
    r
}

// ── framing ──────────────────────────────────────────────────────────────────────────────────
#[derive(Clone, Copy)]
pub struct Opts {
    /// degrees
    pub yaw: f64,
    pub pitch: f64,
    pub landscape: bool,
    /// 1: the whole device fills the rect (a hair of margin); < 1 leaves room around it
    pub scale: f64,
    /// vertical field of view, degrees
    pub fov: f64,
}

pub struct Cam {
    pub eye: V3,
    /// world → clip (wgpu: depth 0…1), rows
    pub view_proj: M4,
}

/// three's camera for the skill's phone.html: aimed down the screen's normal, orbited by yaw
/// (about the view's up) then pitch, backed off until the model's box fits the rect.
pub fn camera(m: &Model, o: &Opts, aspect: f64) -> Cam {
    let (normal, up, right) = match &m.screen {
        Some(s) => (s.normal, s.up, s.right),
        None => ([0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
    };
    let view_up = norm(if o.landscape { right } else { up });
    let orbit_right = norm(cross(view_up, normal));
    let dir = norm(rotate(rotate(normal, view_up, o.yaw.to_radians()), orbit_right, o.pitch.to_radians()));
    let center = scale(add(m.min, m.max), 0.5);
    let radius = dot(sub(m.max, m.min), sub(m.max, m.min)).sqrt() / 2.0;
    let cam_right = norm(cross(view_up, dir));
    let cam_up = norm(cross(dir, cam_right));
    let (mut hw, mut hh) = (0.0f64, 0.0f64);
    for c in 0..8 {
        let q = [if c & 1 == 0 { m.min[0] } else { m.max[0] }, if c & 2 == 0 { m.min[1] } else { m.max[1] }, if c & 4 == 0 { m.min[2] } else { m.max[2] }];
        let d = sub(q, center);
        hw = hw.max(dot(d, cam_right).abs());
        hh = hh.max(dot(d, cam_up).abs());
    }
    let t = (o.fov.to_radians() / 2.0).tan();
    let dist = (hw / (t * aspect)).max(hh / t) * 1.05 / o.scale.max(1e-3);
    let eye = add(center, scale(dir, dist));
    let near = (dist - radius * 2.0).max(dist * 0.02);
    let far = dist + radius * 2.0;
    // three's lookAt: z from the target to the eye, x = up × z, y = z × x
    let z = norm(sub(eye, center));
    let mut x = cross(view_up, z);
    if dot(x, x) < 1e-12 {
        x = cross([0.0, 0.0, 1.0], z);
    }
    let x = norm(x);
    let y = cross(z, x);
    let view = [[x[0], x[1], x[2], -dot(x, eye)], [y[0], y[1], y[2], -dot(y, eye)], [z[0], z[1], z[2], -dot(z, eye)], [0.0, 0.0, 0.0, 1.0]];
    let f = 1.0 / t;
    let proj = [[f / aspect, 0.0, 0.0, 0.0], [0.0, f, 0.0, 0.0], [0.0, 0.0, far / (near - far), near * far / (near - far)], [0.0, 0.0, -1.0, 0.0]];
    Cam { eye, view_proj: mul4(&proj, &view) }
}

/// Where a model lands in `rect` (x, y, w, h, in the scene's units).
pub struct Info {
    /// the device's box: its every vertex, projected
    pub bbox: [f64; 4],
    /// the screen's corners (top-left, top-right, bottom-right, bottom-left of the image on
    /// it), and its aspect (width / height, as the image shows)
    pub screen: Option<([[f64; 2]; 4], f64)>,
    /// the model's box's 8 corners (motion)
    pub hull: Vec<[f64; 2]>,
}

fn to_rect(cam: &Cam, rect: [f64; 4], p: V3) -> [f64; 2] {
    let q = point(&cam.view_proj, p);
    [rect[0] + (q[0] + 1.0) / 2.0 * rect[2], rect[1] + (1.0 - q[1]) / 2.0 * rect[3]]
}

/// The screen image's UV transform (three: flip a left-handed screen, turn the image back
/// upright on a landscape device): rows of a 2×3 matrix, and whether u wraps.
pub fn screen_uv(m: &Model, landscape: bool) -> ([f32; 3], [f32; 3], bool) {
    let flip = m.screen.as_ref().is_some_and(|s| s.handed < 0.0);
    let (sx, tx) = if flip { (-1.0, 1.0) } else { (1.0, 0.0) };
    let (rot, cx, cy) = if landscape { (std::f64::consts::FRAC_PI_2, 0.5, 0.5) } else { (0.0, 0.0, 0.0) };
    let (s, c) = rot.sin_cos();
    // three's Matrix3.setUvTransform(tx, 0, sx, 1, rotation, cx, cy)
    let r0 = [sx * c, sx * s, -sx * (c * cx + s * cy) + cx + tx];
    let r1 = [-s, c, -(-s * cx + c * cy) + cy];
    (r0.map(|v| v as f32), r1.map(|v| v as f32), flip)
}

pub fn info(m: &Model, cam: &Cam, rect: [f64; 4], landscape: bool) -> Info {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in &m.prims {
        for v in &p.verts {
            let q = to_rect(cam, rect, v.p.map(|x| x as f64));
            x0 = x0.min(q[0]);
            y0 = y0.min(q[1]);
            x1 = x1.max(q[0]);
            y1 = y1.max(q[1]);
        }
    }
    let screen = m.screen.as_ref().map(|s| {
        // which corner of the mesh shows which corner of the image
        let (r0, r1, wrap) = screen_uv(m, landscape);
        // (a hair inside each corner: where u wraps, the corner itself is ambiguous)
        let mesh_uv = [[0.01, 0.01], [0.99, 0.01], [0.99, 0.99], [0.01, 0.99]];
        let mut out = [[0.0; 2]; 4];
        for (k, c) in s.corners.iter().enumerate() {
            let (u, v) = (mesh_uv[k][0], mesh_uv[k][1]);
            let mut iu = r0[0] as f64 * u + r0[1] as f64 * v + r0[2] as f64;
            let iv = r1[0] as f64 * u + r1[1] as f64 * v + r1[2] as f64;
            if wrap {
                iu = iu.rem_euclid(1.0);
            }
            let slot = match (iu.round() as i32, iv.round() as i32) {
                (0, 0) => 0,
                (1, 0) => 1,
                (1, 1) => 2,
                _ => 3,
            };
            out[slot] = to_rect(cam, rect, *c);
        }
        (out, if landscape { 1.0 / s.aspect } else { s.aspect })
    });
    let hull = (0..8)
        .map(|c| {
            let q = [if c & 1 == 0 { m.min[0] } else { m.max[0] }, if c & 2 == 0 { m.min[1] } else { m.max[1] }, if c & 4 == 0 { m.min[2] } else { m.max[2] }];
            to_rect(cam, rect, q)
        })
        .collect();
    Info { bbox: [x0, y0, x1 - x0, y1 - y0], screen, hull }
}

/// A model rendered into a texture: what the GPU needs, hashable (floats as bits) so a
/// texture is reused while nothing about it changes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ModelReq {
    pub path: Arc<str>,
    /// the texture's size (supersampled)
    pub size: (u32, u32),
    /// world → clip, rows; the eye
    pub view_proj: [u32; 16],
    pub eye: [u32; 3],
    pub landscape: bool,
    pub exposure: u32,
    /// the image on the screen
    pub screen: Option<TexKey>,
}

impl ModelReq {
    pub fn view_proj(&self) -> [f32; 16] {
        self.view_proj.map(f32::from_bits)
    }
    pub fn eye(&self) -> [f32; 3] {
        self.eye.map(f32::from_bits)
    }
}

pub fn bits16(m: &M4) -> [u32; 16] {
    let mut out = [0; 16];
    for i in 0..4 {
        for j in 0..4 {
            out[i * 4 + j] = (m[i][j] as f32).to_bits();
        }
    }
    out
}

// ── the studio ───────────────────────────────────────────────────────────────────────────────
/// three's RoomEnvironment (from model-viewer): a white room lit by a point light, six grey
/// boxes on its floor, six glowing panels; seen from the origin, as an equirectangular map of
/// linear radiance (w × h, RGB).
pub fn room(w: usize, h: usize) -> Vec<[f32; 3]> {
    struct Bx {
        pos: V3,
        rot: f64,
        size: V3,
        kind: u8, // 0 room (seen from inside), 1 box, 2 light
        glow: f64,
    }
    let b = |pos: V3, rot: f64, size: V3| Bx { pos, rot, size, kind: 1, glow: 0.0 };
    let light = |pos: V3, size: V3, glow: f64| Bx { pos, rot: 0.0, size, kind: 2, glow };
    let boxes = [
        Bx { pos: [-0.757, 13.219, 0.717], rot: 0.0, size: [31.713, 28.305, 28.591], kind: 0, glow: 0.0 },
        b([-10.906, 2.009, 1.846], -0.195, [2.328, 7.905, 4.651]),
        b([-5.607, -0.754, -0.758], 0.994, [1.970, 1.534, 3.955]),
        b([6.167, 0.857, 7.803], 0.561, [3.927, 6.285, 3.687]),
        b([-2.017, 0.018, 6.124], 0.333, [2.002, 4.566, 2.064]),
        b([2.291, -0.756, -2.621], -0.286, [1.546, 1.552, 1.496]),
        b([-2.193, -0.369, -5.547], 0.516, [3.875, 3.487, 2.986]),
        light([-16.116, 14.37, 8.208], [0.1, 2.428, 2.739], 50.0),
        light([-16.109, 18.021, -8.207], [0.1, 2.425, 2.751], 50.0),
        light([14.904, 12.198, -1.832], [0.15, 4.265, 6.331], 17.0),
        light([-0.462, 8.89, 14.520], [4.38, 5.441, 0.088], 43.0),
        light([3.235, 11.486, -12.541], [2.5, 2.0, 0.1], 20.0),
        light([0.0, 20.0, 0.0], [1.0, 0.1, 1.0], 100.0),
    ];
    let lamp: V3 = [0.418, 16.199, 0.300];
    // rotation about y by a (three's Euler): local → world
    let roty = |v: V3, a: f64| {
        let (s, c) = a.sin_cos();
        [c * v[0] + s * v[2], v[1], -s * v[0] + c * v[2]]
    };
    let mut out = vec![[0.0f32; 3]; w * h];
    for j in 0..h {
        for i in 0..w {
            let u = (i as f64 + 0.5) / w as f64;
            let v = (j as f64 + 0.5) / h as f64;
            let (phi, theta) = ((u - 0.5) * std::f64::consts::TAU, v * std::f64::consts::PI);
            let d = [theta.sin() * phi.sin(), theta.cos(), -theta.sin() * phi.cos()];
            let mut best = (f64::INFINITY, 0usize, [0.0; 3]);
            for (k, bx) in boxes.iter().enumerate() {
                // into the box's unit frame
                let o = roty(sub([0.0; 3], bx.pos), -bx.rot);
                let dl = roty(d, -bx.rot);
                let (o, dl) = ([o[0] / bx.size[0], o[1] / bx.size[1], o[2] / bx.size[2]], [dl[0] / bx.size[0], dl[1] / bx.size[1], dl[2] / bx.size[2]]);
                let (mut tn, mut tf) = (f64::NEG_INFINITY, f64::INFINITY);
                let (mut an, mut af) = ((0, 0.0), (0, 0.0));
                let mut miss = false;
                for a in 0..3 {
                    if dl[a].abs() < 1e-12 {
                        if o[a].abs() > 0.5 {
                            miss = true;
                        }
                        continue;
                    }
                    let (t0, t1) = ((-0.5 - o[a]) / dl[a], (0.5 - o[a]) / dl[a]);
                    let (lo, hi, slo) = if t0 < t1 { (t0, t1, -1.0) } else { (t1, t0, 1.0) };
                    if lo > tn {
                        tn = lo;
                        an = (a, slo);
                    }
                    if hi < tf {
                        tf = hi;
                        af = (a, -slo);
                    }
                }
                if miss || tn > tf {
                    continue;
                }
                // the room from inside: where the ray leaves it, its face turned inward
                let (t, (a, s)) = if bx.kind == 0 { (tf, (af.0, -af.1)) } else { (tn, an) };
                if t <= 1e-6 || t >= best.0 {
                    continue;
                }
                let mut nl = [0.0; 3];
                nl[a] = s;
                best = (t, k, roty(nl, bx.rot));
            }
            if !best.0.is_finite() {
                continue;
            }
            let bx = &boxes[best.1];
            let c = if bx.kind == 2 {
                bx.glow
            } else {
                // white, rough, lit by the point light (three's legacy units: × π, falloff
                // (1 − d/28)²)
                let p = scale(d, best.0);
                let n = best.2;
                let lv = sub(lamp, p);
                let dist = dot(lv, lv).sqrt();
                let l = scale(lv, 1.0 / dist);
                let atten = (1.0 - dist / 28.0).clamp(0.0, 1.0).powi(2);
                let nl = dot(n, l).max(0.0);
                let irr = nl * 5.0 * atten * std::f64::consts::PI;
                let vv = norm(scale(p, -1.0));
                irr * (1.0 / std::f64::consts::PI + ggx(l, vv, n, 0.04, 1.0))
            };
            out[j * w + i] = [c as f32; 3];
        }
    }
    out
}

/// three's BRDF_GGX (a scalar Fresnel base).
fn ggx(l: V3, v: V3, n: V3, f0: f64, rough: f64) -> f64 {
    let alpha = rough * rough;
    let h = norm(add(l, v));
    let (nl, nv, nh, vh) = (dot(n, l).clamp(0.0, 1.0), dot(n, v).clamp(0.0, 1.0), dot(n, h).clamp(0.0, 1.0), dot(v, h).clamp(0.0, 1.0));
    let fr = 2f64.powf((-5.55473 * vh - 6.98316) * vh);
    let f = f0 * (1.0 - fr) + fr;
    let a2 = alpha * alpha;
    let gv = nl * (a2 + (1.0 - a2) * nv * nv).sqrt();
    let gl = nv * (a2 + (1.0 - a2) * nl * nl).sqrt();
    let vis = 0.5 / (gv + gl).max(1e-6);
    let den = nh * nh * (a2 - 1.0) + 1.0;
    let dd = a2 / (std::f64::consts::PI * den * den);
    f * vis * dd
}

/// The blur (radians) of each level of three's PMREM as `textureCubeUV` reads them: level i is
/// "mip" 8 − i; the scene's own 0.04 blur sits under every level.
pub const ENV_SIGMAS: [f64; 11] = [0.0, 1.0 / 128.0, 1.0 / 64.0, 1.0 / 32.0, 1.0 / 16.0, 0.125, 0.215, 0.35, 0.446, 0.526, 0.582];
pub const ENV_BASE_SIGMA: f64 = 0.04;
