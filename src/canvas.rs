//! What a scene draws on. A Canvas only records: a display list of shapes, each with the
//! homography that puts its plane on the frame. The GPU executes the list (gpu.rs).
//! Recording first lets the renderer look ahead (decode the frames it will need) and compare
//! a frame's list at both ends of the shutter (does anything move? then blur it).
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use crate::paths::Path;
use crate::script::Ts;
use crate::text as T;

pub type Rect = [f64; 4];
pub type M3 = [[f64; 3]; 3];
/// Straight (not premultiplied) RGBA, 0…1.
pub type Col = [f64; 4];

pub const I3: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub fn mul(a: &M3, b: &M3) -> M3 {
    let mut m = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    m
}

pub fn inv3(m: &M3) -> M3 {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let co = [e * i - f * h, -(d * i - f * g), d * h - e * g];
    let det = a * co[0] + b * co[1] + c * co[2];
    let k = if det.abs() < 1e-12 { 0.0 } else { 1.0 / det };
    [
        [co[0] * k, -(b * i - c * h) * k, (b * f - c * e) * k],
        [co[1] * k, (a * i - c * g) * k, -(a * f - c * d) * k],
        [co[2] * k, -(a * h - b * g) * k, (a * e - b * d) * k],
    ]
}

fn same_plane(a: &M3, b: &M3) -> bool {
    a.iter().flatten().zip(b.iter().flatten()).all(|(x, y)| (x - y).abs() < 1e-9)
}

/// Two rounded rects' overlap as one: the overlap rect, each corner rounded as the rect whose
/// corner it is (both: the larger).
fn intersect(a: (Rect, [f64; 4]), b: (Rect, [f64; 4])) -> (Rect, [f64; 4]) {
    let (ra, rb) = (a.0, b.0);
    let x0 = ra[0].max(rb[0]);
    let y0 = ra[1].max(rb[1]);
    let x1 = (ra[0] + ra[2]).min(rb[0] + rb[2]);
    let y1 = (ra[1] + ra[3]).min(rb[1] + rb[3]);
    let r = [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)];
    // corners: top-left, top-right, bottom-right, bottom-left
    let corners = |q: Rect| [(q[0], q[1]), (q[0] + q[2], q[1]), (q[0] + q[2], q[1] + q[3]), (q[0], q[1] + q[3])];
    let (ca, cb, cr) = (corners(ra), corners(rb), corners(r));
    let near = |p: (f64, f64), q: (f64, f64)| (p.0 - q.0).abs() < 1e-6 && (p.1 - q.1).abs() < 1e-6;
    let mut radius = [0.0; 4];
    for i in 0..4 {
        let from_a = if near(cr[i], ca[i]) { a.1[i] } else { 0.0 };
        let from_b = if near(cr[i], cb[i]) { b.1[i] } else { 0.0 };
        radius[i] = from_a.max(from_b);
    }
    (r, radius)
}

pub fn translate(dx: f64, dy: f64) -> M3 {
    [[1.0, 0.0, dx], [0.0, 1.0, dy], [0.0, 0.0, 1.0]]
}

// ── colours ──────────────────────────────────────────────────────────────────────────────────

pub fn premul(c: Col, opacity: f64) -> [f32; 4] {
    let a = c[3] * opacity;
    [(c[0] * a) as f32, (c[1] * a) as f32, (c[2] * a) as f32, a as f32]
}

// ── the display list ─────────────────────────────────────────────────────────────────────────
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TexKey {
    /// a file, decoded at 1/k
    Img(Arc<str>, u32),
    Text { para: T::ParaKey, color: [u8; 3], alpha: u32 },
    /// a 3D model, rendered on the GPU (model.rs)
    Model(Arc<crate::model::ModelReq>),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Blend {
    Normal,
    Multiply,
}

#[derive(Clone)]
pub enum Fill {
    Solid([f32; 4]),
    Grad { angle: f64, stops: Vec<([f32; 4], f64)> },
    Tex { key: TexKey, uv: Rect },
}

/// A clip: a rounded rect in the plane it was set in, and the matrix from the plane of the
/// shape it clips into that one.
pub type Clip = Option<(Rect, [f64; 4], M3)>;

#[derive(Clone)]
pub struct BoxOp {
    pub h: M3,
    pub quad: Rect,
    pub rect: Rect,
    pub radius: [f64; 4],
    pub opacity: f64,
    pub feather: f64,
    pub border: f64,
    pub bcolor: [f32; 4],
    pub blend: Blend,
    pub fill: Fill,
    /// dx, dy, sigma, spread; premultiplied colour
    pub insets: Vec<([f64; 4], [f32; 4])>,
    pub clip: Clip,
    /// half-planes a·x + b·y + c ≥ 0 that are kept
    pub hp: Vec<[f64; 3]>,
}

#[derive(Clone)]
pub struct ShadowOp {
    pub h: M3,
    pub quad: Rect,
    pub rect: Rect,
    pub radius: f64,
    pub sigma: f64,
    pub color: [f32; 4],
    /// the element's own box: no shadow under it
    pub cut: Option<(Rect, [f64; 4])>,
    pub clip: Clip,
}

#[derive(Clone)]
pub struct StrokeOp {
    pub h: M3,
    pub path: Arc<Path>,
    pub a: f64,
    pub b: f64,
    pub hw: f64,
    pub color: [f32; 4],
    pub fill: bool,
    pub evenodd: bool,
    pub quad: Rect,
    pub clip: Clip,
}

/// A custom shader filling a rect (shader.rs).
#[derive(Clone)]
pub struct ShaderOp {
    pub h: M3,
    pub quad: Rect,
    pub rect: Rect,
    pub code: Arc<crate::shader::Code>,
    pub params: Vec<f32>,
    pub f: f64,
    pub clip: Clip,
}

#[derive(Clone)]
pub enum Op {
    Shader(Box<ShaderOp>),
    Box(Box<BoxOp>),
    Shadow(ShadowOp),
    Stroke(StrokeOp),
    /// a layer, composited onto what is under it when popped
    Push(Box<Layer>),
    Pop,
}

/// What an op is and where, for comparing two lists: its kind, its quad's corners on the
/// frame, and what it shows.
#[derive(Clone, PartialEq)]
pub enum Extra {
    None,
    Img,
    Text(String),
    Range(f64, f64),
}

#[derive(Clone, PartialEq)]
pub struct Sig {
    kind: u8,
    pts: Vec<f64>,
    extra: Extra,
}

// ── box styles ───────────────────────────────────────────────────────────────────────────────
/// A box-shadow: dx, dy, blur, spread, colour.
#[derive(Clone, Copy)]
pub struct Sh(pub f64, pub f64, pub f64, pub f64, pub Col);

/// A CSS box's look.
#[derive(Clone)]
pub struct Bx {
    pub fill: Option<Col>,
    pub grad: Option<(f64, Vec<(Col, f64)>)>,
    pub tex: Option<(TexKey, Rect)>,
    pub radius: [f64; 4],
    pub border: f64,
    pub bcolor: Option<Col>,
    pub shadows: Vec<Sh>,
    pub insets: Vec<Sh>,
    pub opacity: f64,
    pub feather: f64,
    pub hp: Vec<[f64; 3]>,
    pub blend: Blend,
    pub cut_shadows: bool,
}

impl Default for Bx {
    fn default() -> Self {
        Bx {
            fill: None,
            grad: None,
            tex: None,
            radius: [0.0; 4],
            border: 0.0,
            bcolor: None,
            shadows: Vec::new(),
            insets: Vec::new(),
            opacity: 1.0,
            feather: 0.0,
            hp: Vec::new(),
            blend: Blend::Normal,
            cut_shadows: false,
        }
    }
}

impl Bx {
    pub fn none() -> Bx {
        Bx::default()
    }
    pub fn multiply(mut self) -> Bx {
        self.blend = Blend::Multiply;
        self
    }
    pub fn cut_shadows(mut self) -> Bx {
        self.cut_shadows = true;
        self
    }
}

/// A layer's mask, in frame (design) coordinates.
#[derive(Clone, Debug, PartialEq)]
pub enum Mask {
    /// (cx, cy, rx, ry): transparent inside `from`, opaque beyond `to` (fractions of the radii)
    Ellipse([f64; 4], [f64; 2]),
    /// along (x0, y0) → (x1, y1): (alpha, position 0…1) stops, up to 4
    Linear([f64; 4], Vec<(f64, f64)>),
}

/// How a layer is composited: opacity, blur (x, y), a mask, a drop shadow (dx, dy, blur;
/// premultiplied colour).
#[derive(Clone, Debug)]
pub struct Layer {
    pub opacity: f64,
    pub blur: (f64, f64),
    pub mask: Option<Mask>,
    pub shadow: Option<([f64; 3], [f32; 4])>,
}

/// A group's settings (CSS opacity / filter: blur() drop-shadow() / mask-image / a translate).
#[derive(Clone)]
pub struct Group {
    pub opacity: f64,
    pub blur: (f64, f64),
    pub dx: f64,
    pub dy: f64,
    pub mask: Option<Mask>,
    pub shadow: Option<([f64; 3], [f32; 4])>,
}

impl Group {
    pub fn opacity(o: f64) -> Group {
        Group { opacity: o, blur: (0.0, 0.0), dx: 0.0, dy: 0.0, mask: None, shadow: None }
    }
    pub fn blur(mut self, b: f64) -> Group {
        self.blur = (b, b);
        self
    }
    pub fn mask(mut self, m: Mask) -> Group {
        self.mask = Some(m);
        self
    }
}

type Sizes = Mutex<HashMap<Arc<str>, (u32, u32)>>;
static IMAGE_SIZES: LazyLock<Sizes> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// A JPEG's or PNG's size, from its header.
pub fn image_size(p: &Arc<str>) -> Result<(u32, u32), String> {
    if let Some(s) = IMAGE_SIZES.lock().unwrap().get(p) {
        return Ok(*s);
    }
    if !std::path::Path::new(&**p).is_file() {
        return Err("no such file".into());
    }
    let s = read_size(p).ok_or("not a JPEG or PNG")?;
    IMAGE_SIZES.lock().unwrap().insert(p.clone(), s);
    Ok(s)
}

fn read_size(p: &str) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let mut buf = vec![0u8; 1 << 16];
    let n = f.read(&mut buf).ok()?;
    let b = &buf[..n];
    if b.starts_with(b"\x89PNG") {
        let w = u32::from_be_bytes(b[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(b[20..24].try_into().ok()?);
        return Some((w, h));
    }
    let mut i = 2;
    while i + 9 < b.len() {
        if b[i] != 0xFF {
            i += 1;
            continue;
        }
        let m = b[i + 1];
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if (0xC0..=0xCF).contains(&m) && m != 0xC4 && m != 0xC8 && m != 0xCC {
            let h = u16::from_be_bytes([b[i + 5], b[i + 6]]) as u32;
            let w = u16::from_be_bytes([b[i + 7], b[i + 8]]) as u32;
            return Some((w, h));
        }
        i += 2 + len;
    }
    None
}

pub struct Canvas {
    pub f: f64,
    pub scale: f64,
    /// the design size
    pub size: (f64, f64),
    pub ops: Vec<Op>,
    pub sigs: Vec<Sig>,
    h: Vec<M3>,
    /// open clips: rect, radii, and the transform they were set under
    clip: Vec<Option<(Rect, [f64; 4], M3)>>,
    /// open groups: (drawn into a layer, translated)
    groups: Vec<(bool, bool)>,
    /// open half-plane clips: (first op inside, the half-plane)
    halves: Vec<(usize, [f64; 3])>,
}

impl Canvas {
    pub fn new(f: f64, scale: f64, size: (f64, f64)) -> Canvas {
        Canvas { f, scale, size, ops: Vec::new(), sigs: Vec::new(), h: vec![I3], clip: vec![None], groups: Vec::new(), halves: Vec::new() }
    }

    fn push_op(&mut self, op: Op, sig: Sig) {
        self.ops.push(op);
        self.sigs.push(sig);
    }

    // ── state ────────────────────────────────────────────────────────────────────────────────
    pub fn hm(&self) -> M3 {
        *self.h.last().unwrap()
    }

    fn cur_clip(&self) -> Clip {
        self.clip.last().unwrap().map(|(r, rr, hc)| (r, rr, mul(&inv3(&hc), &self.hm())))
    }

    pub fn push_transform(&mut self, m: M3) {
        let h = mul(&self.hm(), &m);
        self.h.push(h);
    }

    pub fn pop_transform(&mut self) {
        if self.h.len() > 1 {
            self.h.pop();
        }
    }

    /// Clips to a rounded rect, inside any clip already set: in the same plane the two
    /// intersect (the corners keep whichever rounding reaches them); a clip set under another
    /// transform replaces the outer one.
    pub fn push_clip(&mut self, rect: Rect, radius: [f64; 4]) {
        let h = self.hm();
        let clip = match *self.clip.last().unwrap() {
            Some((outer, oradius, oh)) if same_plane(&oh, &h) => intersect((outer, oradius), (rect, radius)),
            _ => (rect, radius),
        };
        self.clip.push(Some((clip.0, clip.1, h)));
    }

    pub fn pop_clip(&mut self) {
        if self.clip.len() > 1 {
            self.clip.pop();
        }
    }

    pub fn push_half(&mut self, hp: [f64; 3]) {
        self.halves.push((self.ops.len(), hp));
    }

    /// Adds the half-plane to every box drawn since its push.
    pub fn pop_half(&mut self) {
        if let Some((start, hp)) = self.halves.pop() {
            for op in &mut self.ops[start..] {
                if let Op::Box(b) = op {
                    b.hp.push(hp);
                }
            }
        }
    }

    /// CSS opacity / filter: blur() on a group: drawn into a layer of its own when either
    /// applies, so overlaps inside it fade and blur as one.
    pub fn begin_group(&mut self, g: Group) {
        let (bx, by) = g.blur;
        let layered = g.opacity < 0.999 || bx > 0.3 || by > 0.3 || g.mask.is_some() || g.shadow.is_some();
        if layered {
            let layer = Layer { opacity: g.opacity.max(0.0), blur: (bx, by), mask: g.mask, shadow: g.shadow };
            self.push_op(Op::Push(Box::new(layer)), sig_of(b'u'));
        }
        let moved = g.dx != 0.0 || g.dy != 0.0;
        if moved {
            self.push_transform(translate(g.dx, g.dy));
        }
        self.groups.push((layered, moved));
    }

    pub fn end_group(&mut self) {
        let Some((layered, moved)) = self.groups.pop() else { return };
        if moved {
            self.pop_transform();
        }
        if layered {
            self.push_op(Op::Pop, sig_of(b'o'));
        }
    }

    // ── geometry helpers ─────────────────────────────────────────────────────────────────────
    fn screen_scale(&self) -> f64 {
        let h = self.hm();
        (h[0][0] * h[1][1] - h[0][1] * h[1][0]).abs().sqrt() / h[2][2].abs().max(1e-6)
    }

    fn sig(&self, kind: u8, r: Rect, extra: Extra) -> Sig {
        let h = self.hm();
        let [x, y, w, hh] = r;
        let mut pts = Vec::with_capacity(8);
        for (px, py) in [(x, y), (x + w, y), (x, y + hh), (x + w, y + hh)] {
            let sx = h[0][0] * px + h[0][1] * py + h[0][2];
            let sy = h[1][0] * px + h[1][1] * py + h[1][2];
            let sw = h[2][0] * px + h[2][1] * py + h[2][2];
            pts.push((sx / sw * 100.0).round() / 100.0);
            pts.push((sy / sw * 100.0).round() / 100.0);
        }
        Sig { kind, pts, extra }
    }

    // ── shapes ───────────────────────────────────────────────────────────────────────────────
    #[allow(clippy::too_many_arguments)]
    pub fn shadow(&mut self, r: Rect, radius: f64, s: Sh, opacity: f64, cut: Option<(Rect, [f64; 4])>) {
        let Sh(dx, dy, blur, spread, c) = s;
        let col = premul(c, opacity);
        if col[3] <= 0.002 {
            return;
        }
        let [x, y, w, h] = r;
        let r2 = if radius > 0.0 { (radius + spread).max(0.0) } else { 0.0 };
        let rect = [x + dx - spread, y + dy - spread, w + 2.0 * spread, h + 2.0 * spread];
        let sigma = blur / 2.0;
        let m = 3.0 * sigma + 3.0 / self.screen_scale().max(1e-3);
        let quad = [rect[0] - m, rect[1] - m, rect[2] + 2.0 * m, rect[3] + 2.0 * m];
        let sig = self.sig(b's', quad, Extra::None);
        let clip = self.cur_clip();
        self.push_op(Op::Shadow(ShadowOp { h: self.hm(), quad, rect, radius: r2, sigma, color: col, cut, clip }), sig);
    }

    /// A CSS box whose border box is (x, y, w, h).
    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, b: Bx) {
        if b.opacity <= 0.002 || w <= 0.0 || h <= 0.0 {
            return;
        }
        let r = [x, y, w, h];
        for s in b.shadows.iter().rev() {
            let cut = if b.cut_shadows { Some((r, b.radius)) } else { None };
            self.shadow(r, b.radius[0], *s, b.opacity, cut);
        }
        let (fill, extra) = if let Some((key, uv)) = b.tex {
            let extra = match &key {
                TexKey::Img(..) | TexKey::Model(..) => Extra::Img,
                TexKey::Text { para, .. } => Extra::Text(para.s.clone()),
            };
            (Fill::Tex { key, uv }, extra)
        } else if let Some((angle, stops)) = b.grad {
            (Fill::Grad { angle, stops: stops.iter().map(|(c, p)| (premul(*c, 1.0), *p)).collect() }, Extra::None)
        } else {
            (Fill::Solid(b.fill.map(|c| premul(c, 1.0)).unwrap_or([0.0; 4])), Extra::None)
        };
        let insets = b.insets.iter().take(2).map(|s| ([s.0, s.1, s.2 / 2.0, s.3], premul(s.4, 1.0))).collect();
        let m = 2.0 / self.screen_scale().max(1e-3) + b.feather;
        let quad = [x - m, y - m, w + 2.0 * m, h + 2.0 * m];
        let sig = self.sig(b'b', r, extra);
        let op = BoxOp {
            h: self.hm(),
            quad,
            rect: r,
            radius: b.radius,
            opacity: b.opacity,
            feather: b.feather,
            border: b.border,
            bcolor: b.bcolor.map(|c| premul(c, 1.0)).unwrap_or([0.0; 4]),
            blend: b.blend,
            fill,
            insets,
            clip: self.cur_clip(),
            hp: b.hp.iter().take(2).copied().collect(),
        };
        self.push_op(Op::Box(Box::new(op)), sig);
    }

    /// A file (a filmed frame, a prop) shown in a box; decoded at the smallest JPEG scale that
    /// still has ≥ 1.3 source pixels per output pixel.
    #[allow(clippy::too_many_arguments)]
    pub fn image(&mut self, p: &Arc<str>, x: f64, y: f64, w: f64, h: f64, uv: Rect, b: Bx) {
        let (iw, _) = image_size(p).unwrap_or((1, 1));
        let on_screen = w / uv[2].max(1e-6) * self.screen_scale() * self.scale;
        let mut k = 1u32;
        while k < 8 && iw as f64 / (k * 2) as f64 >= 1.3 * on_screen {
            k *= 2;
        }
        self.rect(x, y, w, h, Bx { tex: Some((TexKey::Img(p.clone(), k), uv)), ..b });
    }

    /// A 3D model rendered into `rect` (model.rs); `hull`: its box's corners on the rect's
    /// plane, so turning it in place counts as motion.
    pub fn model(&mut self, mut req: crate::model::ModelReq, rect: Rect, hull: &[[f64; 2]], supersample: f64, max: u32, b: Bx) {
        let k = self.screen_scale() * self.scale * supersample;
        let (mut w, mut h) = ((rect[2] * k).round().max(1.0), (rect[3] * k).round().max(1.0));
        let over = w.max(h) / max as f64;
        if over > 1.0 {
            w = (w / over).floor().max(1.0);
            h = (h / over).floor().max(1.0);
        }
        req.size = (w as u32, h as u32);
        let n = self.ops.len();
        self.rect(rect[0], rect[1], rect[2], rect[3], Bx { tex: Some((TexKey::Model(Arc::new(req)), [0.0, 0.0, 1.0, 1.0])), ..b });
        if self.ops.len() > n {
            let hm = self.hm();
            let mut pts = Vec::with_capacity(hull.len() * 2);
            for p in hull {
                let sx = hm[0][0] * p[0] + hm[0][1] * p[1] + hm[0][2];
                let sy = hm[1][0] * p[0] + hm[1][1] * p[1] + hm[1][2];
                let sw = hm[2][0] * p[0] + hm[2][1] * p[1] + hm[2][2];
                pts.push((sx / sw * 100.0).round() / 100.0);
                pts.push((sy / sw * 100.0).round() / 100.0);
            }
            let last = self.sigs.len() - 1;
            self.sigs[last] = Sig { kind: b'm', pts, extra: Extra::None };
        }
    }

    /// One line of text whose line box's top-left is (x, y); `lh` its line-height in px
    /// (None: normal). Returns the advance.
    #[allow(clippy::too_many_arguments)]
    pub fn text(&mut self, s: &str, family: &Arc<str>, x: f64, y: f64, size: f64, weight: u32, c: Col, ls: f64, lh: Option<f64>, opacity: f64, spans: &[(u32, u32, [u8; 4])]) -> f64 {
        let (_, base) = T::line_metrics(family, size, lh);
        if s.is_empty() {
            return 0.0;
        }
        let para = T::ParaKey::new(s, family, size, weight, ls, None, None, T::Align::Start).with(spans, false);
        let adv = T::shape(&para).width;
        let (wid, hgt, pad, ibase) = T::image_box(&para);
        self.text_image(para, x - pad, y + base - ibase, wid, hgt, c, opacity);
        adv
    }

    /// A paragraph wrapped at `width`, its lines aligned in it; (x, y) its top-left. Returns
    /// its (width, height, lines, first baseline).
    #[allow(clippy::too_many_arguments)]
    pub fn paragraph(&mut self, s: &str, x: f64, y: f64, width: f64, t: &Ts) -> [f64; 4] {
        if s.is_empty() {
            return [0.0; 4];
        }
        let (family, size, weight, c, ls, lh, align, opacity) = (&t.font, t.size, t.weight, t.color, t.ls, t.lh, t.align, t.opacity);
        let para = T::ParaKey::new(s, family, size, weight, ls, lh, Some(width), align).with(&t.spans, t.balance);
        let p = T::shape(&para);
        let (wid, hgt, pad, ibase) = T::image_box(&para);
        let first = p.baselines.first().copied().unwrap_or(0.0);
        self.text_image(para, x - pad, y + first - ibase, wid, hgt, c, opacity);
        [p.width, p.height, p.baselines.len() as f64, first]
    }

    /// cv.text(): one line, or wrapped at `width`; x is the start, centre or end (`anchor`).
    /// Returns [width, height, lines, first baseline].
    pub fn styled_text(&mut self, s: &str, x: f64, y: f64, t: &Ts) -> [f64; 4] {
        let k = match t.anchor {
            T::Align::Start => 0.0,
            T::Align::Center => 0.5,
            T::Align::End => 1.0,
        };
        match t.width {
            None => {
                let (line, base) = T::line_metrics(&t.font, t.size, t.lh);
                let adv = if k > 0.0 { T::measure(s, &t.font, t.size, t.weight, t.ls) } else { 0.0 };
                let adv2 = self.text(s, &t.font, x - k * adv, y, t.size, t.weight, t.color, t.ls, t.lh, t.opacity, &t.spans);
                [adv2, line, 1.0, base]
            }
            Some(w) => self.paragraph(s, x - k * w, y, w, t),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn text_image(&mut self, para: T::ParaKey, x: f64, y: f64, w: f64, h: f64, c: Col, opacity: f64) {
        let color = [0, 1, 2].map(|i| (c[i] * 255.0).round() as u8);
        let key = TexKey::Text { para, color, alpha: (c[3] as f32).to_bits() };
        self.rect(x, y, w, h, Bx { tex: Some((key, [0.0, 0.0, 1.0, 1.0])), opacity, ..Default::default() });
    }

    /// The stretch [a, b] (lengths) of a path, round caps and joins; or its fill.
    #[allow(clippy::too_many_arguments)]
    pub fn stroke_path(&mut self, p: Arc<Path>, width: f64, c: Col, a: f64, b: Option<f64>, opacity: f64, fill: bool, evenodd: bool) {
        if p.segs.is_empty() {
            return;
        }
        let b = b.unwrap_or(p.length);
        let col = premul(c, opacity);
        if col[3] <= 0.002 {
            return;
        }
        let hw = width / 2.0;
        let m = hw + 3.0 / self.screen_scale().max(1e-3);
        let [x0, y0, x1, y1] = p.bbox;
        let quad = [x0 - m, y0 - m, x1 - x0 + 2.0 * m, y1 - y0 + 2.0 * m];
        let sig = self.sig(b't', quad, Extra::Range((a * 10.0).round() / 10.0, (b * 10.0).round() / 10.0));
        let op = StrokeOp { h: self.hm(), path: p, a, b, hw, color: col, fill, evenodd, quad, clip: self.cur_clip() };
        self.push_op(Op::Stroke(op), sig);
    }

    /// A custom shader over `rect` (default: the whole design frame).
    pub fn shader(&mut self, code: Arc<crate::shader::Code>, rect: Option<Rect>, params: Vec<f32>) {
        let rect = rect.unwrap_or([0.0, 0.0, self.size.0, self.size.1]);
        let m = 2.0 / self.screen_scale().max(1e-3);
        let quad = [rect[0] - m, rect[1] - m, rect[2] + 2.0 * m, rect[3] + 2.0 * m];
        // (what a shader shows changes as it likes, as a filmed frame does: only where it is
        // counts as motion)
        let sig = self.sig(b'g', rect, Extra::None);
        let op = ShaderOp { h: self.hm(), quad, rect, code, params, f: self.f, clip: self.cur_clip() };
        self.push_op(Op::Shader(Box::new(op)), sig);
    }
}

fn sig_of(kind: u8) -> Sig {
    Sig { kind, pts: Vec::new(), extra: Extra::None }
}

/// How far (frame px) anything moves between two display lists, or None when they differ in
/// what they hold (something appears, a filmed frame changes).
pub fn motion(a: &[Sig], b: &[Sig]) -> Option<f64> {
    if a.len() != b.len() {
        return None;
    }
    let mut worst: f64 = 0.0;
    for (x, y) in a.iter().zip(b) {
        if x.kind != y.kind || x.pts.len() != y.pts.len() {
            return None;
        }
        if x.extra != y.extra {
            match (&x.extra, &y.extra) {
                // a thread drawing on: its head moves by the length it gains
                (Extra::Range(a0, a1), Extra::Range(b0, b1)) if x.kind == b't' => {
                    worst = worst.max((a0 - b0).abs()).max((a1 - b1).abs());
                }
                _ => return None,
            }
        }
        // (a filmed frame giving way to the next is the app's own motion, not ours: Img
        // extras compare equal whatever the frame)
        for (p, q) in x.pts.iter().zip(&y.pts) {
            worst = worst.max((p - q).abs());
        }
    }
    Some(worst)
}
