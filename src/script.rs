//! The scene: a TypeScript module run in QuickJS. Types are stripped (swc, which keeps every
//! line and column where it was, so a stack trace points into the file as written), imports
//! resolve to files next to the importer, JSON imports become their value, and `reel` is the
//! built-in module (js/reel.ts) over the native functions installed here.
//!
//! Each thread that draws frames loads its own copy of the module: a runtime is not shared
//! across threads, and a scene's `draw` depends only on the frame, so copies agree.
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::SystemTime;

use anyhow::{Context as _, anyhow, bail};
use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::module::Declared;
use rquickjs::{Array, Context, Ctx, Exception, Function, Module, Object, Persistent, Runtime, Value};

use crate::canvas::{Bx, Canvas, Col, Group, Mask, Sh, image_size};
use crate::config::Config;
use crate::text::{self as T, Align};

const PRELUDE: &str = include_str!("../js/reel.ts");

/// Where relative paths in a scene point: the directory of the scene file.
pub static ROOT: RwLock<PathBuf> = RwLock::new(PathBuf::new());

pub fn resolve(p: &str) -> PathBuf {
    let p = Path::new(p);
    if p.is_absolute() { p.to_path_buf() } else { ROOT.read().unwrap().join(p) }
}

/// A path as written in messages: relative to the scene's directory when inside it.
pub fn shown(p: &Path) -> String {
    let root = ROOT.read().unwrap();
    p.strip_prefix(&*root).map(|r| r.display().to_string()).unwrap_or_else(|_| p.display().to_string())
}

// ── TypeScript ───────────────────────────────────────────────────────────────────────────────
#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// TypeScript → JavaScript with the types blanked out (positions kept).
pub fn strip(name: &str, src: &str) -> Result<String, String> {
    use swc_common::{GLOBALS, Globals, SourceMap, errors::Handler, sync::Lrc};
    GLOBALS.set(&Globals::new(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let buf = Buf::default();
        let handler = Handler::with_emitter_writer(Box::new(buf.clone()), Some(cm.clone()));
        let opts = swc_ts_fast_strip::Options { filename: Some(name.into()), module: Some(true), ..Default::default() };
        match swc_ts_fast_strip::operate(&cm, &handler, src.to_string(), opts) {
            Ok(o) => Ok(o.code),
            Err(e) => {
                let detail = String::from_utf8_lossy(&buf.0.lock().unwrap()).trim().to_string();
                let hint = if matches!(e.code, swc_ts_fast_strip::ErrorCode::UnsupportedSyntax) {
                    "\n(reel strips types only: no enums, namespaces or parameter properties; use unions and plain objects)"
                } else {
                    ""
                };
                Err(format!("{detail}{hint}"))
            }
        }
    })
}

/// Stripped modules, by path and modification time (each thread's runtime loads them again).
type Stripped = Mutex<HashMap<PathBuf, (SystemTime, Arc<str>)>>;
static STRIPPED: LazyLock<Stripped> = LazyLock::new(Default::default);

fn module_source(path: &Path) -> Result<Arc<str>, String> {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some((t, s)) = STRIPPED.lock().unwrap().get(path)
        && *t == mtime
    {
        return Ok(s.clone());
    }
    let src = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path.display().to_string();
    let js: Arc<str> = match path.extension().and_then(|e| e.to_str()) {
        Some("json") => format!("export default {};", src.trim()).into(),
        Some("js" | "mjs") => src.into(),
        _ => strip(&name, &src)?.into(),
    };
    STRIPPED.lock().unwrap().insert(path.to_path_buf(), (mtime, js.clone()));
    Ok(js)
}

// ── modules ──────────────────────────────────────────────────────────────────────────────────
/// Every file a script loaded (the preview watches them).
pub type Loaded = Arc<Mutex<Vec<PathBuf>>>;

struct Files(Loaded);

impl Resolver for Files {
    fn resolve<'js>(&mut self, _ctx: &Ctx<'js>, base: &str, name: &str, _a: Option<ImportAttributes<'js>>) -> rquickjs::Result<String> {
        if name == "reel" {
            return Ok("reel".into());
        }
        let dir = if base == "reel" || base.is_empty() { ROOT.read().unwrap().clone() } else { Path::new(base).parent().map(Path::to_path_buf).unwrap_or_default() };
        let p = if Path::new(name).is_absolute() { PathBuf::from(name) } else { dir.join(name) };
        let mut tries = vec![p.clone()];
        if p.extension().is_none() {
            for ext in ["ts", "js", "json"] {
                tries.push(p.with_extension(ext));
            }
            tries.push(p.join("index.ts"));
        }
        for t in tries {
            if t.is_file() {
                let t = t.canonicalize().unwrap_or(t);
                return Ok(t.display().to_string());
            }
        }
        Err(rquickjs::Error::new_resolving_message(base, name, "no such file (imports are relative to the importing file; \"reel\" is the only package)"))
    }
}

impl Loader for Files {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str, _a: Option<ImportAttributes<'js>>) -> rquickjs::Result<Module<'js, Declared>> {
        if name == "reel" {
            static JS: LazyLock<String> = LazyLock::new(|| strip("reel.ts", PRELUDE).expect("the built-in reel module"));
            return Module::declare(ctx.clone(), "reel", JS.as_str());
        }
        let path = PathBuf::from(name);
        self.0.lock().unwrap().push(path.clone());
        let js = module_source(&path).map_err(|e| rquickjs::Error::new_loading_message(name, e))?;
        let m = Module::declare(ctx.clone(), name, js.as_bytes())?;
        // import.meta as Node has it: the module's own file and directory
        let meta = m.meta()?;
        meta.set("url", format!("file://{name}"))?;
        meta.set("filename", name)?;
        meta.set("dirname", path.parent().map(|p| p.display().to_string()).unwrap_or_default())?;
        Ok(m)
    }
}

// ── the script ───────────────────────────────────────────────────────────────────────────────
pub struct Script {
    // (field order: the function and context go before the runtime)
    draw: Persistent<Function<'static>>,
    ctx: Context,
    _rt: Runtime,
    pub config: Config,
    pub files: Loaded,
}

thread_local! {
    /// The canvas a running `draw` records into.
    static CV: RefCell<Option<Canvas>> = const { RefCell::new(None) };
}

/// A JS exception (or other failure) as text: message and stack, paths shown relative.
fn describe(ctx: &Ctx, e: rquickjs::Error) -> String {
    if !matches!(e, rquickjs::Error::Exception) {
        return e.to_string();
    }
    let v = ctx.catch();
    let root = format!("{}/", ROOT.read().unwrap().display());
    let tidy = |s: String| s.replace(&root, "");
    if let Some(ex) = v.as_exception() {
        let msg = ex.message().unwrap_or_default();
        let stack = ex.stack().unwrap_or_default();
        let name: String = ex.get("name").unwrap_or_else(|_| "Error".into());
        return tidy(format!("{name}: {msg}\n{}", stack.trim_end()));
    }
    tidy(format!("thrown: {v:?}"))
}

impl Script {
    /// Loads the scene at `entry` (sets the root for relative paths).
    pub fn load(entry: &Path, params: &[(String, String)]) -> anyhow::Result<Script> {
        let entry = entry.canonicalize().with_context(|| format!("{}", entry.display()))?;
        *ROOT.write().unwrap() = entry.parent().unwrap().to_path_buf();
        let rt = Runtime::new()?;
        rt.set_max_stack_size(4 << 20);
        let files: Loaded = Arc::new(Mutex::new(Vec::new()));
        rt.set_loader(Files(files.clone()), Files(files.clone()));
        let ctx = Context::full(&rt)?;
        let (draw, config) = ctx.with(|ctx| -> anyhow::Result<_> {
            install(&ctx, params).map_err(|e| anyhow!("installing natives: {e}"))?;
            let ns = Module::import(&ctx, entry.display().to_string())
                .and_then(|p| p.finish::<Object>())
                .map_err(|e| anyhow!("{}", describe(&ctx, e)))?;
            let v: Value = ns.get("default").map_err(|e| anyhow!("{}", describe(&ctx, e)))?;
            let Some(video) = v.as_object() else {
                bail!("{}: the default export must be the video: `export default video({{ duration: … }}, (cv, f) => {{ … }})`", shown(&entry));
            };
            let draw: Function = video.get("draw").map_err(|_| anyhow!("the default export is not a video() (no draw function)"))?;
            let cfg: Object = video.get("config").map_err(|_| anyhow!("the default export is not a video() (no config)"))?;
            let config = Config::from_js(&cfg).map_err(|e| anyhow!("video config: {e}"))?;
            Ok((Persistent::save(&ctx, draw), config))
        })?;
        Ok(Script { draw, ctx, _rt: rt, config, files })
    }

    /// Records frame `f` (fractional for motion-blur sub-frames).
    pub fn draw(&self, f: f64, scale: f64) -> anyhow::Result<Canvas> {
        let size = (self.config.width as f64, self.config.height as f64);
        CV.with(|c| *c.borrow_mut() = Some(Canvas::new(f, scale, size)));
        let r = self.ctx.with(|ctx| -> Result<(), String> {
            let draw = self.draw.clone().restore(&ctx).map_err(|e| e.to_string())?;
            draw.call::<_, ()>((f,)).map_err(|e| describe(&ctx, e))
        });
        let cv = CV.with(|c| c.borrow_mut().take()).unwrap();
        r.map_err(|e| anyhow!("frame {f}: {e}"))?;
        Ok(cv)
    }
}

// ── natives ──────────────────────────────────────────────────────────────────────────────────
type R<T> = rquickjs::Result<T>;

fn with_cv<T>(ctx: &Ctx, f: impl FnOnce(&mut Canvas) -> T) -> R<T> {
    CV.with(|c| match c.borrow_mut().as_mut() {
        Some(cv) => Ok(f(cv)),
        None => Err(Exception::throw_message(ctx, "drawing outside draw(): canvas methods only work while a frame is drawn")),
    })
}

fn finite(ctx: &Ctx, what: &str, vals: &[f64]) -> R<()> {
    if vals.iter().all(|v| v.is_finite()) {
        return Ok(());
    }
    Err(Exception::throw_type(ctx, &format!("{what}: got a non-finite number in {vals:?} (NaN or Infinity: an undefined value in arithmetic?)")))
}

fn get_f(o: &Object, k: &str) -> R<Option<f64>> {
    let v: Value = o.get(k)?;
    if v.is_undefined() || v.is_null() {
        return Ok(None);
    }
    Ok(v.as_number())
}

fn get_s(o: &Object, k: &str) -> R<Option<String>> {
    let v: Value = o.get(k)?;
    v.as_string().map(|s| s.to_string()).transpose()
}

/// A colour: CSS text or [r, g, b, a] in 0…1.
pub fn color(ctx: &Ctx, v: &Value) -> R<Col> {
    if let Some(s) = v.as_string() {
        let s = s.to_string()?;
        return crate::color::parse(&s).ok_or_else(|| Exception::throw_type(ctx, &format!("not a colour: {s:?} (use \"#rrggbb\", \"#rrggbbaa\", \"rgba(r, g, b, a)\" or [r, g, b, a] in 0…1)")));
    }
    if let Some(a) = v.as_array() {
        let n: Vec<f64> = a.iter::<f64>().collect::<R<_>>()?;
        if n.len() == 3 || n.len() == 4 {
            return Ok([n[0], n[1], n[2], n.get(3).copied().unwrap_or(1.0)]);
        }
    }
    Err(Exception::throw_type(ctx, &format!("not a colour: {v:?}")))
}

fn color_at(ctx: &Ctx, o: &Object, k: &str) -> R<Option<Col>> {
    let v: Value = o.get(k)?;
    if v.is_undefined() || v.is_null() {
        return Ok(None);
    }
    color(ctx, &v).map(Some)
}

fn radius(v: &Value) -> R<[f64; 4]> {
    if let Some(n) = v.as_number() {
        return Ok([n; 4]);
    }
    if let Some(a) = v.as_array() {
        let n: Vec<f64> = a.iter::<f64>().collect::<R<_>>()?;
        if n.len() == 4 {
            return Ok([n[0], n[1], n[2], n[3]]);
        }
    }
    Ok([0.0; 4])
}

fn shadows(ctx: &Ctx, v: &Value) -> R<Vec<Sh>> {
    let one = |o: &Object| -> R<Sh> {
        Ok(Sh(
            get_f(o, "x")?.unwrap_or(0.0),
            get_f(o, "y")?.unwrap_or(0.0),
            get_f(o, "blur")?.unwrap_or(0.0),
            get_f(o, "spread")?.unwrap_or(0.0),
            color_at(ctx, o, "color")?.unwrap_or([0.0, 0.0, 0.0, 1.0]),
        ))
    };
    if let Some(a) = v.as_array() {
        return a.iter::<Object>().map(|o| one(&o?)).collect();
    }
    if let Some(o) = v.as_object() {
        return Ok(vec![one(o)?]);
    }
    Ok(vec![])
}

/// A Look object → a box style.
pub fn look(ctx: &Ctx, v: Option<&Object>) -> R<Bx> {
    let mut b = Bx::none();
    let Some(o) = v else { return Ok(b) };
    if let Some(c) = color_at(ctx, o, "fill")? {
        b.fill = Some(c);
    }
    let g: Value = o.get("gradient")?;
    if let Some(g) = g.as_object() {
        let angle = get_f(g, "angle")?.unwrap_or(180.0);
        let stops: Array = g.get("stops")?;
        let mut st = Vec::new();
        for s in stops.iter::<Array>() {
            let s = s?;
            st.push((color(ctx, &s.get(0)?)?, s.get::<f64>(1)?));
        }
        if st.is_empty() || st.len() > 4 {
            return Err(Exception::throw_range(ctx, "gradient: 1 to 4 stops"));
        }
        b.grad = Some((angle, st));
    }
    b.radius = radius(&o.get("radius")?)?;
    let border: Value = o.get("border")?;
    if let Some(bo) = border.as_object() {
        b.border = get_f(bo, "width")?.unwrap_or(1.0);
        b.bcolor = color_at(ctx, bo, "color")?;
    }
    b.shadows = shadows(ctx, &o.get("shadow")?)?;
    b.insets = shadows(ctx, &o.get("inset")?)?;
    if let Some(v) = get_f(o, "opacity")? {
        b.opacity = v;
    }
    if let Some(v) = get_f(o, "feather")? {
        b.feather = v;
    }
    if get_s(o, "blend")?.as_deref() == Some("multiply") {
        b = b.multiply();
    }
    if o.get::<_, Option<bool>>("clipShadows")?.unwrap_or(false) {
        b = b.cut_shadows();
    }
    Ok(b)
}

/// Byte ranges of a text drawn in their own colours (RGBA8).
pub type Spans = Vec<(u32, u32, [u8; 4])>;

/// A TextStyle object (with the video's defaults).
pub struct Ts {
    pub size: f64,
    pub weight: u32,
    pub color: Col,
    pub font: Arc<str>,
    pub ls: f64,
    pub lh: Option<f64>,
    pub width: Option<f64>,
    pub align: Align,
    pub anchor: Align,
    pub opacity: f64,
    /// byte ranges in their own colours
    pub spans: Spans,
    pub balance: bool,
}

fn align_of(s: Option<String>) -> Align {
    match s.as_deref() {
        Some("center") => Align::Center,
        Some("end") => Align::End,
        _ => Align::Start,
    }
}

pub fn text_style(ctx: &Ctx, v: Option<&Object>) -> R<Ts> {
    let mut t = Ts {
        size: 32.0,
        weight: 400,
        color: [0.0, 0.0, 0.0, 1.0],
        font: T::default_family(),
        ls: 0.0,
        lh: None,
        width: None,
        align: Align::Start,
        anchor: Align::Start,
        opacity: 1.0,
        spans: Vec::new(),
        balance: false,
    };
    let Some(o) = v else { return Ok(t) };
    if let Some(v) = get_f(o, "size")? {
        t.size = v;
    }
    if let Some(v) = get_f(o, "weight")? {
        t.weight = v as u32;
    }
    if let Some(c) = color_at(ctx, o, "color")? {
        t.color = c;
    }
    if let Some(f) = get_s(o, "font")? {
        t.font = f.into();
    }
    t.ls = get_f(o, "letterSpacing")?.unwrap_or(0.0);
    t.lh = get_f(o, "lineHeight")?;
    t.width = get_f(o, "width")?;
    t.align = align_of(get_s(o, "align")?);
    t.anchor = align_of(get_s(o, "anchor")?);
    t.opacity = get_f(o, "opacity")?.unwrap_or(1.0);
    t.balance = o.get::<_, Option<bool>>("balance")?.unwrap_or(false);
    if !(t.size > 0.0 && t.size.is_finite()) {
        return Err(Exception::throw_range(ctx, &format!("text size must be > 0, got {}", t.size)));
    }
    Ok(t)
}

/// Text: a string, or spans `[{ text, color? }, …]` (a span without a colour takes the
/// style's). Returns the whole string and the coloured byte ranges.
pub fn text_arg<'js>(ctx: &Ctx<'js>, v: &Value<'js>) -> R<(String, Spans)> {
    if let Some(a) = v.as_array() {
        let mut s = String::new();
        let mut spans = Vec::new();
        for o in a.iter::<Value>() {
            let o = o?;
            let (t, c) = if let Some(o) = o.as_object() {
                (o.get::<_, Option<String>>("text")?.unwrap_or_default(), color_at(ctx, o, "color")?)
            } else {
                (o.as_string().map(|x| x.to_string()).transpose()?.unwrap_or_default(), None)
            };
            let start = s.len() as u32;
            s.push_str(&t);
            if let Some(c) = c {
                // (alpha 0 would read as "no colour": the smallest alpha instead)
                let px = [c[0], c[1], c[2]].map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
                let a = ((c[3].clamp(0.0, 1.0) * 255.0).round() as u8).max(1);
                spans.push((start, s.len() as u32, [px[0], px[1], px[2], a]));
            }
        }
        return Ok((s, spans));
    }
    if let Some(st) = v.as_string() {
        return Ok((st.to_string()?, Vec::new()));
    }
    if v.is_undefined() || v.is_null() {
        return Ok((String::new(), Vec::new()));
    }
    // numbers and the like, as JS would print them
    let s: rquickjs::String = <rquickjs::convert::Coerced<rquickjs::String> as rquickjs::FromJs>::from_js(ctx, v.clone())?.0;
    Ok((s.to_string()?, Vec::new()))
}

fn group_style(ctx: &Ctx, o: &Object) -> R<Group> {
    let mut g = Group::opacity(get_f(o, "opacity")?.unwrap_or(1.0));
    let blur: Value = o.get("blur")?;
    if let Some(b) = blur.as_number() {
        g = g.blur(b);
    } else if let Some(a) = blur.as_array() {
        g.blur = (a.get(0)?, a.get(1)?);
    }
    g.dx = get_f(o, "x")?.unwrap_or(0.0);
    g.dy = get_f(o, "y")?.unwrap_or(0.0);
    let m: Value = o.get("mask")?;
    if let Some(m) = m.as_object() {
        if let Some(e) = m.get::<_, Option<Array>>("ellipse")? {
            let e: Vec<f64> = e.iter::<f64>().collect::<R<_>>()?;
            if e.len() != 4 {
                return Err(Exception::throw_type(ctx, "mask.ellipse: [cx, cy, rx, ry]"));
            }
            g = g.mask(Mask::Ellipse([e[0], e[1], e[2], e[3]], [get_f(m, "from")?.unwrap_or(0.0), get_f(m, "to")?.unwrap_or(1.0)]));
        } else if let Some(l) = m.get::<_, Option<Array>>("linear")? {
            let l: Vec<f64> = l.iter::<f64>().collect::<R<_>>()?;
            if l.len() != 4 {
                return Err(Exception::throw_type(ctx, "mask.linear: [x0, y0, x1, y1]"));
            }
            let stops: Array = m.get("stops")?;
            let mut st = Vec::new();
            for s in stops.iter::<Array>() {
                let s = s?;
                st.push((s.get::<f64>(0)?, s.get::<f64>(1)?));
            }
            if st.is_empty() || st.len() > 4 {
                return Err(Exception::throw_range(ctx, "mask.stops: 1 to 4 [alpha, position] pairs"));
            }
            g = g.mask(Mask::Linear([l[0], l[1], l[2], l[3]], st));
        } else {
            return Err(Exception::throw_type(ctx, "mask: { ellipse: [cx, cy, rx, ry], from, to } or { linear: [x0, y0, x1, y1], stops: [[alpha, position], …] }"));
        }
    }
    let sh: Value = o.get("shadow")?;
    if let Some(sh) = sh.as_object() {
        let c = color_at(ctx, sh, "color")?.unwrap_or([0.0, 0.0, 0.0, 0.5]);
        let a = c[3];
        g.shadow = Some((
            [get_f(sh, "x")?.unwrap_or(0.0), get_f(sh, "y")?.unwrap_or(0.0), get_f(sh, "blur")?.unwrap_or(0.0)],
            [(c[0] * a) as f32, (c[1] * a) as f32, (c[2] * a) as f32, a as f32],
        ));
    }
    finite(ctx, "group", &[g.opacity, g.blur.0, g.blur.1, g.dx, g.dy])?;
    Ok(g)
}

fn matrix(ctx: &Ctx, a: &Array) -> R<crate::canvas::M3> {
    let v: Vec<f64> = a.iter::<f64>().collect::<R<_>>()?;
    if v.len() != 9 {
        return Err(Exception::throw_type(ctx, "a matrix is 9 numbers, row-major"));
    }
    finite(ctx, "transform", &v)?;
    Ok([[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]])
}

fn nums<'js>(ctx: &Ctx<'js>, v: &[f64]) -> R<Array<'js>> {
    let a = Array::new(ctx.clone())?;
    for (i, x) in v.iter().enumerate() {
        a.set(i, *x)?;
    }
    Ok(a)
}

fn io_err(ctx: &Ctx, p: &Path, e: impl std::fmt::Display) -> rquickjs::Error {
    Exception::throw_message(ctx, &format!("{}: {e}", shown(p)))
}

fn install<'js>(ctx: &Ctx<'js>, params: &[(String, String)]) -> R<()> {
    let n = Object::new(ctx.clone())?;
    let set = |name: &str, f: Function<'js>| n.set(name, f);

    let p = Object::new(ctx.clone())?;
    for (k, v) in params {
        p.set(k.as_str(), v.as_str())?;
    }
    n.set("params", p)?;
    set("env", Function::new(ctx.clone(), |k: String| std::env::var(k).ok())?)?;
    set(
        "log",
        Function::new(ctx.clone(), |level: String, msg: String| {
            let f = CV.with(|c| c.borrow().as_ref().map(|cv| cv.f));
            let at = f.map(|f| format!("[frame {f}] ")).unwrap_or_default();
            eprintln!("{}{at}{msg}", if level == "log" { String::new() } else { format!("{level}: ") });
        })?,
    )?;
    set(
        "configure",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, cfg: Object<'js>| -> R<()> {
            let c = Config::from_js(&cfg).map_err(|e| Exception::throw_type(&ctx, &format!("video(): {e}")))?;
            T::configure(&c.fonts, c.font.as_deref(), c.lang.as_deref()).map_err(|e| Exception::throw_message(&ctx, &format!("video(): {e}")))
        })?,
    )?;

    // drawing
    set(
        "rect",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, x: f64, y: f64, w: f64, h: f64, l: Option<Object<'js>>| -> R<()> {
            finite(&ctx, "rect", &[x, y, w, h])?;
            let b = look(&ctx, l.as_ref())?;
            with_cv(&ctx, |cv| cv.rect(x, y, w, h, b))
        })?,
    )?;
    set(
        "image",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, src: String, r: Array<'js>, l: Option<Object<'js>>, uv: Option<Array<'js>>| -> R<()> {
            let r: Vec<f64> = r.iter::<f64>().collect::<R<_>>()?;
            let [x, y, w, h] = [r[0], r[1], r[2], r[3]];
            finite(&ctx, "image", &[x, y, w, h])?;
            let p = resolve(&src);
            let key: Arc<str> = Arc::from(p.display().to_string());
            image_size(&key).map_err(|e| Exception::throw_message(&ctx, &format!("image {}: {e}", shown(&p))))?;
            let b = look(&ctx, l.as_ref())?;
            let uv = match uv {
                Some(a) => {
                    let v: Vec<f64> = a.iter::<f64>().collect::<R<_>>()?;
                    [v[0], v[1], v[2], v[3]]
                }
                None => [0.0, 0.0, 1.0, 1.0],
            };
            with_cv(&ctx, |cv| cv.image(&key, x, y, w, h, uv, b))
        })?,
    )?;
    set(
        "text",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, s: Value<'js>, x: f64, y: f64, st: Option<Object<'js>>| -> R<Array<'js>> {
            finite(&ctx, "text", &[x, y])?;
            let mut t = text_style(&ctx, st.as_ref())?;
            let (s, spans) = text_arg(&ctx, &s)?;
            t.spans = spans;
            let m = with_cv(&ctx, |cv| cv.styled_text(&s, x, y, &t))?;
            nums(&ctx, &m)
        })?,
    )?;
    set(
        "measure",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, s: Value<'js>, st: Option<Object<'js>>| -> R<Array<'js>> {
            let mut t = text_style(&ctx, st.as_ref())?;
            let (s, spans) = text_arg(&ctx, &s)?;
            t.spans = spans;
            nums(&ctx, &T::metrics(&s, &t))
        })?,
    )?;
    set(
        "path",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, d: String, st: Object<'js>| -> R<()> {
            let p = crate::paths::parsed(&d).map_err(|e| Exception::throw_type(&ctx, &format!("path {d:?}: {e}")))?;
            let opacity = get_f(&st, "opacity")?.unwrap_or(1.0);
            let full = crate::paths::true_length(&d).unwrap_or(p.length);
            // lengths along the true curve, onto the flattened one
            let k = if full > 0.0 { p.length / full } else { 1.0 };
            let progress = get_f(&st, "progress")?;
            let a = get_f(&st, "from")?.unwrap_or(0.0) * k;
            let b = match (get_f(&st, "to")?, progress) {
                (Some(t), _) => Some(t * k),
                (None, Some(pr)) => Some(pr.clamp(0.0, 1.0) * p.length),
                _ => None,
            };
            finite(&ctx, "path", &[a, b.unwrap_or(0.0), opacity])?;
            let fill = color_at(&ctx, &st, "fill")?;
            let stroke = color_at(&ctx, &st, "stroke")?;
            let width = get_f(&st, "width")?.unwrap_or(2.0);
            let evenodd = get_s(&st, "fillRule")?.as_deref() == Some("evenodd");
            with_cv(&ctx, |cv| {
                if let Some(c) = fill {
                    cv.stroke_path(p.clone(), 0.0, c, 0.0, None, opacity, true, evenodd);
                }
                if let Some(c) = stroke {
                    cv.stroke_path(p.clone(), width, c, a, b, opacity, false, false);
                }
            })
        })?,
    )?;
    set(
        "pathInfo",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, d: String| -> R<Array<'js>> {
            let p = crate::paths::parsed(&d).map_err(|e| Exception::throw_type(&ctx, &format!("path {d:?}: {e}")))?;
            let len = crate::paths::true_length(&d).unwrap_or(p.length);
            nums(&ctx, &[len, p.bbox[0], p.bbox[1], p.bbox[2], p.bbox[3]])
        })?,
    )?;
    set(
        "pathPoint",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, d: String, s: f64| -> R<Array<'js>> {
            let p = crate::paths::fine(&d).map_err(|e| Exception::throw_type(&ctx, &format!("path {d:?}: {e}")))?;
            let (x, y) = p.point_at(s);
            nums(&ctx, &[x, y])
        })?,
    )?;
    set(
        "shader",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, src: String, rect: Option<Array<'js>>, params: Option<Array<'js>>| -> R<()> {
            let code = crate::shader::source(&src).map_err(|e| Exception::throw_message(&ctx, &e))?;
            let rect = match rect {
                Some(a) => {
                    let v: Vec<f64> = a.iter::<f64>().collect::<R<_>>()?;
                    Some([v[0], v[1], v[2], v[3]])
                }
                None => None,
            };
            let ps: Vec<f32> = match params {
                Some(a) => a.iter::<f64>().map(|v| v.map(|v| v as f32)).collect::<R<_>>()?,
                None => vec![],
            };
            if ps.len() > 64 {
                return Err(Exception::throw_range(&ctx, "shader: at most 64 params"));
            }
            with_cv(&ctx, |cv| cv.shader(code, rect, ps))
        })?,
    )?;
    set(
        "pushGroup",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, o: Object<'js>| -> R<()> {
            let g = group_style(&ctx, &o)?;
            with_cv(&ctx, |cv| cv.begin_group(g))
        })?,
    )?;
    set("popGroup", Function::new(ctx.clone(), |ctx: Ctx<'js>| with_cv(&ctx, |cv| cv.end_group()))?)?;
    set(
        "pushTransform",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, m: Array<'js>| -> R<()> {
            let m = matrix(&ctx, &m)?;
            with_cv(&ctx, |cv| cv.push_transform(m))
        })?,
    )?;
    set("popTransform", Function::new(ctx.clone(), |ctx: Ctx<'js>| with_cv(&ctx, |cv| cv.pop_transform()))?)?;
    set(
        "pushClip",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, r: Array<'js>, rad: Value<'js>| -> R<()> {
            let v: Vec<f64> = r.iter::<f64>().collect::<R<_>>()?;
            finite(&ctx, "clip", &v)?;
            let rr = radius(&rad)?;
            with_cv(&ctx, |cv| cv.push_clip([v[0], v[1], v[2], v[3]], rr))
        })?,
    )?;
    set("popClip", Function::new(ctx.clone(), |ctx: Ctx<'js>| with_cv(&ctx, |cv| cv.pop_clip()))?)?;
    set(
        "pushHalf",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, a: f64, b: f64, c: f64| -> R<()> {
            finite(&ctx, "clipHalf", &[a, b, c])?;
            with_cv(&ctx, |cv| cv.push_half([a, b, c]))
        })?,
    )?;
    set("popHalf", Function::new(ctx.clone(), |ctx: Ctx<'js>| with_cv(&ctx, |cv| cv.pop_half()))?)?;
    set(
        "layout",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, el: Object<'js>| -> R<Array<'js>> {
            let node = crate::layout::from_js(&ctx, &el)?;
            nums(&ctx, &crate::layout::compute(&node))
        })?,
    )?;

    // animation
    set("spring", Function::new(ctx.clone(), |f: f64, d: f64, m: f64, k: f64, fps: f64| crate::anim::spring_at(f, d, m, k, fps))?)?;
    set("springNatural", Function::new(ctx.clone(), |d: f64, m: f64, k: f64, fps: f64| crate::anim::natural(d, m, k, fps))?)?;

    // files
    set("resolve", Function::new(ctx.clone(), |p: String| resolve(&p).display().to_string())?)?;
    set("exists", Function::new(ctx.clone(), |p: String| resolve(&p).exists())?)?;
    set(
        "readText",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, p: String| -> R<String> {
            let p = resolve(&p);
            std::fs::read_to_string(&p).map_err(|e| io_err(&ctx, &p, e))
        })?,
    )?;
    set(
        "listDir",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, p: String, pat: Option<String>| -> R<Vec<String>> {
            let p = resolve(&p);
            let rd = std::fs::read_dir(&p).map_err(|e| io_err(&ctx, &p, e))?;
            let pat = match pat {
                Some(s) => Some(glob::Pattern::new(&s).map_err(|e| Exception::throw_type(&ctx, &format!("listDir pattern: {e}")))?),
                None => None,
            };
            let mut names: Vec<String> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| pat.as_ref().is_none_or(|g| g.matches(n)))
                .collect();
            names.sort();
            Ok(names)
        })?,
    )?;
    set(
        "imageSize",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, p: String| -> R<Array<'js>> {
            let p = resolve(&p);
            let key: Arc<str> = Arc::from(p.display().to_string());
            let (w, h) = image_size(&key).map_err(|e| io_err(&ctx, &p, e))?;
            nums(&ctx, &[w as f64, h as f64])
        })?,
    )?;

    ctx.globals().set("__reel", n)?;
    let console = Object::new(ctx.clone())?;
    let g = ctx.globals();
    ctx.eval::<(), _>(
        r#"globalThis.console = {
  log: (...a) => __reel.log("log", a.map(x => typeof x === "string" ? x : JSON.stringify(x)).join(" ")),
  warn: (...a) => __reel.log("warning", a.map(x => typeof x === "string" ? x : JSON.stringify(x)).join(" ")),
  error: (...a) => __reel.log("error", a.map(x => typeof x === "string" ? x : JSON.stringify(x)).join(" ")),
};"#,
    )?;
    let _ = (console, g);
    Ok(())
}
