//! Paths → polylines, for the stroke and fill shaders: SVG path data (M L H V C S Q T A Z and
//! their relative forms, with implicit repeats). A path
//! flattens to subpaths of points; its length and points along it are measured on the
//! flattened line, finely enough to match @remotion/paths.
use std::cell::RefCell;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub enum Cmd {
    M(f64, f64),
    L(f64, f64),
    C(f64, f64, f64, f64, f64, f64),
    /// rx, ry, x-axis rotation (degrees), large arc, sweep, x, y
    A(f64, f64, f64, bool, bool, f64, f64),
    Z,
}

/// Absolute path commands.
#[derive(Clone, Default)]
pub struct PathData(pub Vec<Cmd>);

impl PathData {
    /// SVG path data → absolute commands (H/V become L; S, Q, T become C).
    pub fn parse(d: &str) -> Result<PathData, String> {
        let toks = tokenize(d)?;
        let mut out = Vec::new();
        let mut pos = (0.0, 0.0);
        let mut start = pos;
        // the last control point, for S and T's reflections: (cubic's second, quadratic's)
        let mut last_c: Option<P> = None;
        let mut last_q: Option<P> = None;
        let mut i = 0;
        let mut cmd = ' ';
        while i < toks.len() {
            if let Tok::Cmd(c) = toks[i] {
                cmd = c;
                i += 1;
                if c == 'Z' || c == 'z' {
                    out.push(Cmd::Z);
                    pos = start;
                    last_c = None;
                    last_q = None;
                    continue;
                }
            } else if cmd == ' ' {
                return Err("path data must start with a command (M)".into());
            }
            let up = cmd.to_ascii_uppercase();
            let rel = cmd.is_ascii_lowercase();
            let n = match up {
                'M' | 'L' | 'T' => 2,
                'H' | 'V' => 1,
                'C' => 6,
                'S' | 'Q' => 4,
                'A' => 7,
                _ => return Err(format!("unknown command {cmd}")),
            };
            if i + n > toks.len() {
                return Err(format!("{cmd} needs {n} numbers"));
            }
            let mut a = Vec::with_capacity(n);
            for t in &toks[i..i + n] {
                match t {
                    Tok::Num(v) => a.push(*v),
                    Tok::Cmd(c) => return Err(format!("{cmd} needs {n} numbers, found {c}")),
                }
            }
            i += n;
            let (ox, oy) = if rel { pos } else { (0.0, 0.0) };
            let (mut nc, mut nq) = (None, None);
            match up {
                'M' => {
                    pos = (a[0] + ox, a[1] + oy);
                    start = pos;
                    out.push(Cmd::M(pos.0, pos.1));
                    cmd = if rel { 'l' } else { 'L' };
                }
                'L' => {
                    pos = (a[0] + ox, a[1] + oy);
                    out.push(Cmd::L(pos.0, pos.1));
                }
                'H' => {
                    pos = (a[0] + if rel { pos.0 } else { 0.0 }, pos.1);
                    out.push(Cmd::L(pos.0, pos.1));
                }
                'V' => {
                    pos = (pos.0, a[0] + if rel { pos.1 } else { 0.0 });
                    out.push(Cmd::L(pos.0, pos.1));
                }
                'C' | 'S' => {
                    let (c1, c2, p1) = if up == 'C' {
                        ((a[0] + ox, a[1] + oy), (a[2] + ox, a[3] + oy), (a[4] + ox, a[5] + oy))
                    } else {
                        let c1 = last_c.map(|c| (2.0 * pos.0 - c.0, 2.0 * pos.1 - c.1)).unwrap_or(pos);
                        (c1, (a[0] + ox, a[1] + oy), (a[2] + ox, a[3] + oy))
                    };
                    out.push(Cmd::C(c1.0, c1.1, c2.0, c2.1, p1.0, p1.1));
                    nc = Some(c2);
                    pos = p1;
                }
                'Q' | 'T' => {
                    let (q, p1) = if up == 'Q' {
                        ((a[0] + ox, a[1] + oy), (a[2] + ox, a[3] + oy))
                    } else {
                        (last_q.map(|c| (2.0 * pos.0 - c.0, 2.0 * pos.1 - c.1)).unwrap_or(pos), (a[0] + ox, a[1] + oy))
                    };
                    // the quadratic as a cubic
                    let c1 = (pos.0 + 2.0 / 3.0 * (q.0 - pos.0), pos.1 + 2.0 / 3.0 * (q.1 - pos.1));
                    let c2 = (p1.0 + 2.0 / 3.0 * (q.0 - p1.0), p1.1 + 2.0 / 3.0 * (q.1 - p1.1));
                    out.push(Cmd::C(c1.0, c1.1, c2.0, c2.1, p1.0, p1.1));
                    nq = Some(q);
                    pos = p1;
                }
                'A' => {
                    let p1 = (a[5] + ox, a[6] + oy);
                    out.push(Cmd::A(a[0], a[1], a[2], a[3] != 0.0, a[4] != 0.0, p1.0, p1.1));
                    pos = p1;
                }
                _ => unreachable!(),
            }
            last_c = nc;
            last_q = nq;
        }
        Ok(PathData(out))
    }

    fn key(&self, step: f64) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        step.to_bits().hash(&mut h);
        for c in &self.0 {
            let v: [f64; 7] = match *c {
                Cmd::M(x, y) => [0.0, x, y, 0.0, 0.0, 0.0, 0.0],
                Cmd::L(x, y) => [1.0, x, y, 0.0, 0.0, 0.0, 0.0],
                Cmd::C(a, b, c, d, e, f) => [2.0, a, b, c, d, e, f],
                Cmd::A(rx, ry, r, l, s, x, y) => [3.0 + 2.0 * l as u8 as f64 + 4.0 * s as u8 as f64, rx, ry, r, x, y, 0.0],
                Cmd::Z => [9.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            };
            for x in v {
                x.to_bits().hash(&mut h);
            }
        }
        h.finish()
    }
}

enum Tok {
    Cmd(char),
    Num(f64),
}

fn tokenize(d: &str) -> Result<Vec<Tok>, String> {
    let b = d.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if "MmLlHhVvCcSsQqTtAaZz".contains(c) {
            out.push(Tok::Cmd(c));
            i += 1;
        } else if c == '-' || c == '+' || c == '.' || c.is_ascii_digit() {
            let s = i;
            i += 1;
            let mut dot = c == '.';
            while i < b.len() {
                let ch = b[i] as char;
                if ch.is_ascii_digit() {
                    i += 1;
                } else if ch == '.' && !dot {
                    dot = true;
                    i += 1;
                } else if (ch == 'e' || ch == 'E') && i + 1 < b.len() {
                    i += 1;
                    if b[i] == b'-' || b[i] == b'+' {
                        i += 1;
                    }
                } else {
                    break;
                }
            }
            out.push(Tok::Num(d[s..i].parse().map_err(|_| format!("bad number {}", &d[s..i]))?));
        } else if c == ',' || c.is_ascii_whitespace() {
            i += 1;
        } else {
            return Err(format!("unexpected {c:?}"));
        }
    }
    Ok(out)
}

type P = (f64, f64);

fn dist(a: P, b: P) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Endpoint arc → points (SVG implementation notes, F.6.5).
#[allow(clippy::too_many_arguments)]
fn arc(p0: P, rx: f64, ry: f64, phi: f64, large: bool, sweep: bool, p1: P, n: f64, out: &mut Vec<P>) {
    let (x1, y1) = p0;
    let (x2, y2) = p1;
    if rx == 0.0 || ry == 0.0 {
        out.push(p1);
        return;
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    let (c, s) = (phi.cos(), phi.sin());
    let (dx, dy) = ((x1 - x2) / 2.0, (y1 - y2) / 2.0);
    let (xp, yp) = (c * dx + s * dy, -s * dx + c * dy);
    let lam = xp * xp / (rx * rx) + yp * yp / (ry * ry);
    if lam > 1.0 {
        rx *= lam.sqrt();
        ry *= lam.sqrt();
    }
    let num = rx * rx * ry * ry - rx * rx * yp * yp - ry * ry * xp * xp;
    let den = rx * rx * yp * yp + ry * ry * xp * xp;
    let mut k = if den != 0.0 { (num / den).max(0.0).sqrt() } else { 0.0 };
    if large == sweep {
        k = -k;
    }
    let (cxp, cyp) = (k * rx * yp / ry, -k * ry * xp / rx);
    let cx = c * cxp - s * cyp + (x1 + x2) / 2.0;
    let cy = s * cxp + c * cyp + (y1 + y2) / 2.0;
    let ang = |ux: f64, uy: f64, vx: f64, vy: f64| (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    let t1 = ang(1.0, 0.0, (xp - cxp) / rx, (yp - cyp) / ry);
    let mut dt = ang((xp - cxp) / rx, (yp - cyp) / ry, (-xp - cxp) / rx, (-yp - cyp) / ry);
    if !sweep && dt > 0.0 {
        dt -= 2.0 * PI;
    } else if sweep && dt < 0.0 {
        dt += 2.0 * PI;
    }
    let steps = 4.max((dt.abs() * rx.max(ry) / n) as usize + 1);
    for i in 1..=steps {
        let t = t1 + dt * i as f64 / steps as f64;
        let (ex, ey) = (rx * t.cos(), ry * t.sin());
        out.push(if i == steps { p1 } else { (c * ex - s * ey + cx, s * ex + c * ey + cy) });
    }
}

fn cubic(p0: P, c1: P, c2: P, p1: P, n: f64, out: &mut Vec<P>) {
    let est = dist(p0, c1) + dist(c1, c2) + dist(c2, p1);
    let steps = 4.max((est / n) as usize + 1);
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let u = 1.0 - t;
        out.push((
            u.powi(3) * p0.0 + 3.0 * u * u * t * c1.0 + 3.0 * u * t * t * c2.0 + t.powi(3) * p1.0,
            u.powi(3) * p0.1 + 3.0 * u * u * t * c1.1 + 3.0 * u * t * t * c2.1 + t.powi(3) * p1.1,
        ));
    }
}

/// Commands → subpaths of points; `step` is the flattening length in path units.
fn flatten(d: &PathData, step: f64) -> Vec<Vec<P>> {
    let mut subs = Vec::new();
    let mut cur: Vec<P> = Vec::new();
    let mut pos = (0.0, 0.0);
    let mut start = pos;
    for c in &d.0 {
        match *c {
            Cmd::Z => {
                if !cur.is_empty() {
                    cur.push(start);
                    subs.push(std::mem::take(&mut cur));
                }
                pos = start;
            }
            Cmd::M(x, y) => {
                if !cur.is_empty() {
                    subs.push(std::mem::take(&mut cur));
                }
                pos = (x, y);
                start = pos;
                cur.push(pos);
            }
            Cmd::L(x, y) => {
                pos = (x, y);
                cur.push(pos);
            }
            Cmd::C(a, b, c2, d2, x, y) => {
                cubic(pos, (a, b), (c2, d2), (x, y), step, &mut cur);
                pos = (x, y);
            }
            Cmd::A(rx, ry, rot, large, sweep, x, y) => {
                arc(pos, rx, ry, rot.to_radians(), large, sweep, (x, y), step, &mut cur);
                pos = (x, y);
            }
        }
    }
    if !cur.is_empty() {
        subs.push(cur);
    }
    subs
}

/// A flattened path: segments (x0, y0, x1, y1) with the length along the path where each
/// starts, for stroking a [from, to] stretch of it.
pub struct Path {
    pub id: u64,
    pub segs: Vec<[f32; 4]>,
    pub starts: Vec<f32>,
    pub length: f64,
    pub bbox: [f64; 4],
}

impl Path {
    fn build(d: &PathData, step: f64, id: u64) -> Path {
        let mut segs = Vec::new();
        let mut starts = Vec::new();
        let mut total = 0.0;
        for sub in flatten(d, step) {
            for w in sub.windows(2) {
                let (p, q) = (w[0], w[1]);
                let l = dist(p, q);
                if l < 1e-9 {
                    continue;
                }
                segs.push([p.0 as f32, p.1 as f32, q.0 as f32, q.1 as f32]);
                starts.push(total as f32);
                total += l;
            }
        }
        let mut bbox = [0.0; 4];
        if !segs.is_empty() {
            bbox = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
            for s in &segs {
                for (x, y) in [(s[0], s[1]), (s[2], s[3])] {
                    bbox[0] = bbox[0].min(x as f64);
                    bbox[1] = bbox[1].min(y as f64);
                    bbox[2] = bbox[2].max(x as f64);
                    bbox[3] = bbox[3].max(y as f64);
                }
            }
        }
        Path { id, segs, starts, length: total, bbox }
    }

    /// The point at length s along the path.
    pub fn point_at(&self, s: f64) -> (f64, f64) {
        if self.segs.is_empty() {
            return (0.0, 0.0);
        }
        let s = s.clamp(0.0, self.length);
        let i = self.starts.partition_point(|&v| (v as f64) <= s) as isize - 1;
        let i = i.clamp(0, self.segs.len() as isize - 1) as usize;
        let [x0, y0, x1, y1] = self.segs[i].map(|v| v as f64);
        let l = (x1 - x0).hypot(y1 - y0);
        let t = if l > 0.0 { (s - self.starts[i] as f64) / l } else { 0.0 };
        (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t)
    }
}

thread_local! {
    static CACHE: RefCell<HashMap<(String, u64), Arc<Path>>> = RefCell::new(HashMap::new());
}

fn cached(d: &str, step: f64) -> Result<Arc<Path>, String> {
    let key = (d.to_string(), step.to_bits());
    if let Some(p) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return Ok(p);
    }
    let data = PathData::parse(d)?;
    let p = Arc::new(Path::build(&data, step, data.key(step)));
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 4096 {
            c.clear();
        }
        c.insert(key, p.clone());
    });
    Ok(p)
}

/// The path as drawn (flattened every 1.5 px).
pub fn parsed(d: &str) -> Result<Arc<Path>, String> {
    cached(d, 1.5)
}

/// The path flattened finely (every 0.3 px): its true length and points along it.
pub fn fine(d: &str) -> Result<Arc<Path>, String> {
    cached(d, 0.3)
}

pub fn true_length(d: &str) -> Option<f64> {
    fine(d).ok().map(|p| p.length)
}
