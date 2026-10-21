// The built-in `reel` module: what scene files import. It wraps the native functions reel
// installs as `__reel` (src/script.rs). Its public surface is documented in reel.d.ts; keep
// the two in step.

const N: any = (globalThis as any).__reel;

// ── the video ─────────────────────────────────────────────────────────────────────────────
let current: any = null;

export const params: Record<string, string> = Object.freeze({ ...N.params });

export function env(name: string): string | undefined {
  return N.env(name) ?? undefined;
}

// seconds() called before video() (in its config) assumes 60 fps; video() checks that held
let earlySeconds = false;

export function seconds(s: number, fps?: number): number {
  if (fps === undefined && !current) earlySeconds = true;
  return s * (fps ?? current?.fps ?? 60);
}

/** A time in a config: frames, or "4s", "1:30". */
function framesOf(t: any, fps: number, what: string): number {
  if (typeof t === "number") return t;
  const s = String(t).trim();
  let m = s.match(/^(-?[\d.]+)s$/);
  if (m) return Math.round(parseFloat(m[1]) * fps);
  m = s.match(/^(\d+):([\d.]+)$/);
  if (m) return Math.round((parseInt(m[1]) * 60 + parseFloat(m[2])) * fps);
  throw new TypeError(`${what}: ${JSON.stringify(t)} is not a time (frames, "4s" or "1:30")`);
}

class CanvasImpl {
  frame = 0;
  width: number;
  height: number;
  fps: number;

  constructor(cfg: any) {
    this.width = cfg.width;
    this.height = cfg.height;
    this.fps = cfg.fps;
  }

  rect(x: number, y: number, w: number, h: number, look: any): void {
    N.rect(x, y, w, h, look);
  }

  circle(cx: number, cy: number, r: number, look: any): void {
    if (!(r > 0)) return;
    const l = typeof look === "string" || Array.isArray(look) ? { fill: look } : look;
    N.rect(cx - r, cy - r, 2 * r, 2 * r, { ...l, radius: r });
  }

  image(src: string, x: number, y: number, w: number, h: number, look?: any): void {
    let uv = look?.uv;
    const fit = look?.fit ?? "fill";
    if (fit !== "fill") {
      const s = imageSize(src);
      const ia = s.width / s.height;
      const ba = w / h;
      if (fit === "cover") {
        uv = ia > ba ? [(1 - ba / ia) / 2, 0, ba / ia, 1] : [0, (1 - ia / ba) / 2, 1, ia / ba];
      } else if (fit === "contain") {
        if (ia > ba) {
          const h2 = w / ia;
          y += (h - h2) / 2;
          h = h2;
        } else {
          const w2 = h * ia;
          x += (w - w2) / 2;
          w = w2;
        }
      } else {
        throw new Error(`image(): fit must be "fill", "cover" or "contain", got ${JSON.stringify(fit)}`);
      }
    }
    N.image(src, [x, y, w, h], look, uv);
  }

  text(s: string, x: number, y: number, style?: any): any {
    const m = N.text(typeof s === "string" || Array.isArray(s) ? s : String(s), x, y, style);
    return { width: m[0], height: m[1], lines: m[2], baseline: m[3] };
  }

  path(d: any, style: any): void {
    N.path(typeof d === "string" ? d : d.d, style ?? {});
  }

  shader(src: string, opts?: any): void {
    N.shader(src, opts?.rect, opts?.params);
  }

  layout(el: any, x: number, y: number): any {
    const r = N.layout(el);
    let i = 0;
    const walk = (e: any) => {
      const k = i * 5;
      i++;
      const X = x + r[k];
      const Y = y + r[k + 1];
      const w = r[k + 2];
      const h = r[k + 3];
      if ((e.kind === "row" || e.kind === "col") && e.style?.look) this.rect(X, Y, w, h, e.style.look);
      if (e.kind === "text") this.text(e.s, X, Y, { ...e.style, width: r[k + 4] ? w : undefined, anchor: "start" });
      if (e.kind === "box" && e.draw) e.draw(this, [X, Y, w, h]);
      for (const c of e.children ?? []) walk(c);
    };
    walk(el);
    return { width: r[2], height: r[3] };
  }

  group(style: any, draw: () => void): void {
    N.pushGroup(style ?? {});
    try {
      draw();
    } finally {
      N.popGroup();
    }
  }

  fade(opacity: number, draw: () => void): void {
    this.group({ opacity }, draw);
  }

  transform(m: number[], draw: () => void): void {
    N.pushTransform(m);
    try {
      draw();
    } finally {
      N.popTransform();
    }
  }

  translate(x: number, y: number, draw: () => void): void {
    this.transform(matrix.translate(x, y), draw);
  }

  scale(s: number, draw: () => void, origin?: [number, number]): void {
    this.transform(matrix.scale(s, s, origin), draw);
  }

  rotate(deg: number, draw: () => void, origin?: [number, number]): void {
    this.transform(matrix.rotate(deg, origin), draw);
  }

  clip(rect: number[], radius: any, draw: () => void): void {
    N.pushClip(rect, radius ?? 0);
    try {
      draw();
    } finally {
      N.popClip();
    }
  }

  clipHalf(a: number, b: number, c: number, draw: () => void): void {
    N.pushHalf(a, b, c);
    try {
      draw();
    } finally {
      N.popHalf();
    }
  }

  during(from: number, to: number, draw: (local: number) => void): void {
    if (this.frame >= from && this.frame < to) draw(this.frame - from);
  }
}

export function video(config: any, draw: (cv: any, f: number) => void): any {
  if (typeof draw !== "function") throw new TypeError("video(config, draw): draw must be a function (cv, f) => { … }");
  const c = { width: 1920, height: 1080, fps: 60, ...config };
  if (earlySeconds && c.fps !== 60) {
    throw new Error(`seconds() ran before video() knew the fps (${c.fps}) and assumed 60: write duration: "10s", or seconds(10, ${c.fps})`);
  }
  c.duration = framesOf(c.duration, c.fps, "duration");
  if (c.markers) c.markers = Object.fromEntries(Object.entries(c.markers).map(([k, v]) => [k, framesOf(v, c.fps, `markers.${k}`)]));
  N.configure(c);
  current = c;
  const cv = new CanvasImpl(c);
  return Object.freeze({
    config: c,
    draw: (f: number) => {
      cv.frame = f;
      draw(cv, f);
    },
  });
}

// ── layout ────────────────────────────────────────────────────────────────────────────────
export function row(children: any[], style?: any): any {
  return { kind: "row", children: children.filter(Boolean), style };
}

export function col(children: any[], style?: any): any {
  return { kind: "col", children: children.filter(Boolean), style };
}

export function text(s: any, style?: any): any {
  return { kind: "text", s: typeof s === "string" || Array.isArray(s) ? s : String(s), style };
}

export function box(w: number, h: number, draw?: (cv: any, rect: number[]) => void): any {
  return { kind: "box", w, h, draw };
}

export function measure(el: any): any {
  const r = N.layout(el);
  return { width: r[2], height: r[3] };
}

export function measureText(s: any, style?: any): any {
  const m = N.measure(typeof s === "string" || Array.isArray(s) ? s : String(s), style);
  return { width: m[0], height: m[1], lines: m[2], baseline: m[3] };
}

// ── animation (Remotion's, so its scenes port frame for frame) ─────────────────────────────
export function interpolate(x: number, input: number[], output: number[], opts?: any): number {
  if (input.length !== output.length) throw new Error(`interpolate(): input has ${input.length} values, output ${output.length}`);
  if (input.length < 2) throw new Error("interpolate(): needs at least 2 points");
  for (let i = 1; i < input.length; i++) {
    if (!(input[i] > input[i - 1])) throw new Error(`interpolate(): input must be strictly increasing, got [${input.join(", ")}]`);
  }
  if (typeof x !== "number" || Number.isNaN(x)) throw new TypeError(`interpolate(): x is ${x}`);
  const easing = opts?.easing ?? ((t: number) => t);
  const left = opts?.extrapolateLeft ?? "extend";
  const right = opts?.extrapolateRight ?? "extend";
  let i = 1;
  for (; i < input.length - 1; ++i) if (input[i] >= x) break;
  i -= 1;
  const [a, b] = [input[i], input[i + 1]];
  const [ya, yb] = [output[i], output[i + 1]];
  let v = x;
  if (v < a) {
    if (left === "identity") return v;
    if (left === "clamp") v = a;
  }
  if (v > b) {
    if (right === "identity") return v;
    if (right === "clamp") v = b;
  }
  if (ya === yb) return ya;
  return easing((v - a) / (b - a)) * (yb - ya) + ya;
}

export function lerp(x: number, input: number[], output: number[], easing?: (t: number) => number): number {
  return interpolate(x, input, output, { easing, extrapolateLeft: "clamp", extrapolateRight: "clamp" });
}

export function spring(o: any): number {
  const fps = o.fps ?? current?.fps ?? 60;
  const c = o.config ?? {};
  const d = c.damping ?? 10;
  const m = c.mass ?? 1;
  const k = c.stiffness ?? 100;
  const from = o.from ?? 0;
  const to = o.to ?? 1;
  const f = o.frame - (o.delay ?? 0);
  let v: number;
  if (o.durationInFrames !== undefined) {
    if (f > o.durationInFrames) return to;
    v = N.spring(f / (o.durationInFrames / N.springNatural(d, m, k, fps)), d, m, k, fps);
  } else {
    v = N.spring(f, d, m, k, fps);
  }
  if (c.overshootClamping) v = Math.min(v, 1);
  return from === 0 && to === 1 ? v : from + (to - from) * v;
}

// bezier-easing (Gaëtan Renaudeau), as Remotion's Easing.bezier
function bezierEasing(x1: number, y1: number, x2: number, y2: number): (t: number) => number {
  if (x1 === y1 && x2 === y2) return (t) => t;
  const A = (a1: number, a2: number) => 1 - 3 * a2 + 3 * a1;
  const B = (a1: number, a2: number) => 3 * a2 - 6 * a1;
  const C = (a1: number) => 3 * a1;
  const calc = (t: number, a1: number, a2: number) => ((A(a1, a2) * t + B(a1, a2)) * t + C(a1)) * t;
  const slope = (t: number, a1: number, a2: number) => 3 * A(a1, a2) * t * t + 2 * B(a1, a2) * t + C(a1);
  const n = 11;
  const step = 1 / (n - 1);
  const samples = new Float64Array(n);
  for (let i = 0; i < n; ++i) samples[i] = calc(i * step, x1, x2);
  const tForX = (x: number) => {
    let start = 0;
    let cur = 1;
    const last = n - 1;
    for (; cur !== last && samples[cur] <= x; ++cur) start += step;
    --cur;
    const dist = (x - samples[cur]) / (samples[cur + 1] - samples[cur]);
    let g = start + dist * step;
    const s0 = slope(g, x1, x2);
    if (s0 >= 0.001) {
      for (let i = 0; i < 4; ++i) {
        const s = slope(g, x1, x2);
        if (s === 0) return g;
        g -= (calc(g, x1, x2) - x) / s;
      }
      return g;
    }
    if (s0 === 0) return g;
    let a = start;
    let b = start + step;
    let t = 0;
    let i = 0;
    let cx = 0;
    do {
      t = a + (b - a) / 2;
      cx = calc(t, x1, x2) - x;
      if (cx > 0) b = t;
      else a = t;
    } while (Math.abs(cx) > 1e-7 && ++i < 10);
    return t;
  };
  return (x: number) => (x === 0 || x === 1 ? x : calc(tForX(x), y1, y2));
}

export const Easing = {
  linear: (t: number) => t,
  ease: bezierEasing(0.42, 0, 1, 1),
  quad: (t: number) => t * t,
  cubic: (t: number) => t * t * t,
  sin: (t: number) => 1 - Math.cos((t * Math.PI) / 2),
  circle: (t: number) => 1 - Math.sqrt(1 - t * t),
  exp: (t: number) => Math.pow(2, 10 * (t - 1)),
  poly: (n: number) => (t: number) => Math.pow(t, n),
  back:
    (s = 1.70158) =>
    (t: number) =>
      t * t * ((s + 1) * t - s),
  elastic: (bounciness = 1) => {
    const p = bounciness * Math.PI;
    return (t: number) => 1 - Math.pow(Math.cos((t * Math.PI) / 2), 3) * Math.cos(t * p);
  },
  bounce: (t: number) => {
    if (t < 1 / 2.75) return 7.5625 * t * t;
    if (t < 2 / 2.75) {
      const u = t - 1.5 / 2.75;
      return 7.5625 * u * u + 0.75;
    }
    if (t < 2.5 / 2.75) {
      const u = t - 2.25 / 2.75;
      return 7.5625 * u * u + 0.9375;
    }
    const u = t - 2.625 / 2.75;
    return 7.5625 * u * u + 0.984375;
  },
  bezier: bezierEasing,
  in: (f: (t: number) => number) => f,
  out: (f: (t: number) => number) => (t: number) => 1 - f(1 - t),
  inOut: (f: (t: number) => number) => (t: number) => (t < 0.5 ? f(t * 2) / 2 : 1 - f((1 - t) * 2) / 2),
};

export function random(seed: number, salt = 0): number {
  // (a hash, not a generator: the same seed gives the same number on every thread and frame)
  let h = Math.imul((seed * 1000003) | 0, 0x9e3779b1) ^ Math.imul((salt * 7919 + 13) | 0, 0x85ebca77);
  h ^= h >>> 15;
  h = Math.imul(h, 0x2c1b3c6d);
  h ^= h >>> 12;
  h = Math.imul(h, 0x297a2d39);
  h ^= h >>> 15;
  return (h >>> 0) / 4294967296;
}

export function clamp(x: number, lo = 0, hi = 1): number {
  return Math.min(hi, Math.max(lo, x));
}

export function mix(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}

// ── colours ───────────────────────────────────────────────────────────────────────────────
function parseColor(c: any): number[] {
  if (Array.isArray(c)) return [c[0], c[1], c[2], c[3] ?? 1];
  const s = String(c).trim();
  if (s === "transparent") return [0, 0, 0, 0];
  if (s === "white") return [1, 1, 1, 1];
  if (s === "black") return [0, 0, 0, 1];
  if (s[0] === "#") {
    const h = s.slice(1);
    if (h.length === 3 || h.length === 4) {
      const v = [...h].map((x) => parseInt(x, 16) / 15);
      return [v[0], v[1], v[2], v[3] ?? 1];
    }
    if (h.length === 6 || h.length === 8) {
      const v = [0, 2, 4, 6].map((i) => (i < h.length ? parseInt(h.slice(i, i + 2), 16) / 255 : 1));
      return v;
    }
  }
  const m = s.match(/^rgba?\(([^)]*)\)$/);
  if (m) {
    const p = m[1].split(/[\s,/]+/).filter(Boolean);
    const ch = (x: string) => (x.endsWith("%") ? parseFloat(x) / 100 : parseFloat(x) / 255);
    const a = p[3] === undefined ? 1 : p[3].endsWith("%") ? parseFloat(p[3]) / 100 : parseFloat(p[3]);
    return [ch(p[0]), ch(p[1]), ch(p[2]), a];
  }
  throw new TypeError(`not a colour: ${JSON.stringify(c)}`);
}

export function alpha(c: any, a: number): number[] {
  const v = parseColor(c);
  return [v[0], v[1], v[2], v[3] * a];
}

export function mixColor(a: any, b: any, t: number): number[] {
  const x = parseColor(a);
  const y = parseColor(b);
  return x.map((v, i) => v + (y[i] - v) * t);
}

export function rgba(r: number, g: number, b: number, a = 1): number[] {
  return [r / 255, g / 255, b / 255, a];
}

// ── paths and transforms ──────────────────────────────────────────────────────────────────
const paths = new Map<string, any>();

export function path(d: string): any {
  let p = paths.get(d);
  if (p) return p;
  const info = N.pathInfo(d);
  p = Object.freeze({
    d,
    length: info[0],
    bbox: [info[1], info[2], info[3], info[4]],
    pointAt: (s: number) => N.pathPoint(d, s),
  });
  if (paths.size > 2048) paths.clear();
  paths.set(d, p);
  return p;
}

export function polyline(points: [number, number][], close = false): string {
  if (points.length === 0) return "";
  const parts = points.map((p, i) => `${i === 0 ? "M" : "L"} ${p[0]} ${p[1]}`);
  if (close) parts.push("Z");
  return parts.join(" ");
}

function mul(a: number[], b: number[]): number[] {
  const m = new Array(9);
  for (let i = 0; i < 3; i++) {
    for (let j = 0; j < 3; j++) m[i * 3 + j] = a[i * 3] * b[j] + a[i * 3 + 1] * b[3 + j] + a[i * 3 + 2] * b[6 + j];
  }
  return m;
}

export const matrix = {
  identity: () => [1, 0, 0, 0, 1, 0, 0, 0, 1],
  translate: (x: number, y: number) => [1, 0, x, 0, 1, y, 0, 0, 1],
  scale: (sx: number, sy?: number, origin?: [number, number]) => {
    const s2 = sy ?? sx;
    const [ox, oy] = origin ?? [0, 0];
    return [sx, 0, ox - sx * ox, 0, s2, oy - s2 * oy, 0, 0, 1];
  },
  rotate: (deg: number, origin?: [number, number]) => {
    const r = (deg * Math.PI) / 180;
    const [c, s] = [Math.cos(r), Math.sin(r)];
    const [ox, oy] = origin ?? [0, 0];
    return mul(mul([1, 0, ox, 0, 1, oy, 0, 0, 1], [c, -s, 0, s, c, 0, 0, 0, 1]), [1, 0, -ox, 0, 1, -oy, 0, 0, 1]);
  },
  perspective: (o: any) => {
    const ax = ((o.rotateX ?? 0) * Math.PI) / 180;
    const ay = ((o.rotateY ?? 0) * Math.PI) / 180;
    const [ox, oy] = o.origin ?? [0, 0];
    const d = o.distance ?? 1000;
    const rx = [1, 0, 0, 0, Math.cos(ax), -Math.sin(ax), 0, Math.sin(ax), Math.cos(ax)];
    const ry = [Math.cos(ay), 0, Math.sin(ay), 0, 1, 0, -Math.sin(ay), 0, Math.cos(ay)];
    const m = mul(rx, ry);
    // q = M (x - ox, y - oy, 0); w = 1 - q.z / d; screen = o + q.xy / w
    const a = new Array(9).fill(0);
    for (let i = 0; i < 2; i++) {
      a[i * 3] = m[i * 3];
      a[i * 3 + 1] = m[i * 3 + 1];
      a[i * 3 + 2] = -m[i * 3] * ox - m[i * 3 + 1] * oy;
    }
    a[6] = -m[6] / d;
    a[7] = -m[7] / d;
    a[8] = 1 + (m[6] * ox + m[7] * oy) / d;
    for (let j = 0; j < 3; j++) {
      a[j] += ox * a[6 + j];
      a[3 + j] += oy * a[6 + j];
    }
    return a;
  },
  multiply: mul,
  apply: (m: number[], x: number, y: number): [number, number] => {
    const w = m[6] * x + m[7] * y + m[8];
    return [(m[0] * x + m[1] * y + m[2]) / w, (m[3] * x + m[4] * y + m[5]) / w];
  },
  invert: (m: number[]) => {
    const [a, b, c, d, e, f, g, h, i] = m;
    const A = e * i - f * h;
    const B = -(d * i - f * g);
    const C = d * h - e * g;
    const det = a * A + b * B + c * C;
    const k = det === 0 ? 0 : 1 / det;
    return [A * k, -(b * i - c * h) * k, (b * f - c * e) * k, B * k, (a * i - c * g) * k, -(a * f - c * d) * k, C * k, -(a * h - b * g) * k, (a * e - b * d) * k];
  },
};

// ── files ─────────────────────────────────────────────────────────────────────────────────
export function resolve(p: string): string {
  return N.resolve(p);
}

export function exists(p: string): boolean {
  return N.exists(p);
}

export function readText(p: string): string {
  return N.readText(p);
}

const json = new Map<string, any>();

export function readJson(p: string): any {
  const full = N.resolve(p);
  if (!json.has(full)) json.set(full, JSON.parse(N.readText(full)));
  return json.get(full);
}

export function listDir(p: string, pattern?: string): string[] {
  return N.listDir(p, pattern);
}

const sizes = new Map<string, any>();

export function imageSize(p: string): any {
  let s = sizes.get(p);
  if (!s) {
    const v = N.imageSize(p);
    s = { width: v[0], height: v[1] };
    sizes.set(p, s);
  }
  return s;
}

export function frames(dir: string): string[] {
  const base = dir.endsWith("/") ? dir.slice(0, -1) : dir;
  return N.listDir(base)
    .filter((n: string) => /\.(jpe?g|png)$/i.test(n))
    .map((n: string) => `${base}/${n}`);
}
