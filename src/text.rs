//! Text → premultiplied RGBA images. Parley lays text out (HarfRust shaping, kerning, CSS
//! letter-spacing, bidi, line breaking, and per-script font fallback through fontconfig, so
//! Japanese, Hindi or emoji find a font); swash rasterises the glyphs. The scene's fonts are
//! registered on every thread; single lines sit in line boxes measured the way Chrome
//! measures them (the font's ascent and descent, each rounded to a pixel).
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use fast_image_resize as fr;
use parley::fontique::Blob;
use parley::{Alignment, AlignmentOptions, FontContext, FontData, FontFamily, FontWeight, Language, LayoutContext, LineHeight, PositionedLayoutItem, StyleProperty};
use swash::FontRef;
use swash::scale::{Render, ScaleContext, Source, StrikeWith, image::Content};
use swash::zeno::Vector;

use crate::script::Ts;

/// The scene's fonts, default family and language. `generation` changes when they do, and
/// each thread then builds its font context again.
struct FontSet {
    generation: u64,
    files: Vec<PathBuf>,
    data: Vec<Arc<Vec<u8>>>,
    family: Arc<str>,
    lang: Option<String>,
}

static FONTS: LazyLock<RwLock<FontSet>> = LazyLock::new(|| {
    RwLock::new(FontSet { generation: 0, files: vec![], data: vec![], family: Arc::from("sans-serif"), lang: None })
});
static FILE_DATA: LazyLock<Mutex<HashMap<PathBuf, Arc<Vec<u8>>>>> = LazyLock::new(Default::default);

/// Sets the scene's fonts (files), default family (None: the first file's) and language.
pub fn configure(files: &[PathBuf], family: Option<&str>, lang: Option<&str>) -> anyhow::Result<()> {
    {
        let f = FONTS.read().unwrap();
        if f.files == files && family.is_none_or(|x| *f.family == *x) && f.lang.as_deref() == lang && (f.generation > 0) {
            return Ok(());
        }
    }
    let mut data = Vec::new();
    for p in files {
        let d = {
            let mut cache = FILE_DATA.lock().unwrap();
            match cache.get(p) {
                Some(d) => d.clone(),
                None => {
                    let d = Arc::new(std::fs::read(p).map_err(|e| anyhow::anyhow!("font {}: {e}", p.display()))?);
                    cache.insert(p.clone(), d.clone());
                    d
                }
            }
        };
        data.push(d);
    }
    let family: Arc<str> = match family {
        Some(f) => Arc::from(f),
        None => match data.first() {
            Some(d) => {
                let mut c = parley::fontique::Collection::new(parley::fontique::CollectionOptions { shared: false, system_fonts: false });
                let reg = c.register_fonts(Blob::new(d.clone()), None);
                let id = reg.first().map(|r| r.0).ok_or_else(|| anyhow::anyhow!("font {}: no font in the file", files[0].display()))?;
                Arc::from(c.family_name(id).unwrap_or("sans-serif"))
            }
            None => Arc::from("sans-serif"),
        },
    };
    let mut f = FONTS.write().unwrap();
    f.generation += 1;
    f.files = files.to_vec();
    f.data = data;
    f.family = family;
    f.lang = lang.map(str::to_string);
    Ok(())
}

pub fn default_family() -> Arc<str> {
    FONTS.read().unwrap().family.clone()
}

struct Fcx {
    generation: u64,
    fcx: FontContext,
    locale: Option<Language>,
}

fn font_context() -> Fcx {
    let f = FONTS.read().unwrap();
    let mut fcx = FontContext::new();
    for data in &f.data {
        fcx.collection.register_fonts(Blob::new(data.clone()), None);
    }
    let locale = f.lang.as_deref().and_then(|l| Language::parse(l).ok());
    Fcx { generation: f.generation, fcx, locale }
}

thread_local! {
    static FCX: RefCell<Fcx> = RefCell::new(font_context());
    static LCX: RefCell<LayoutContext<[u8; 4]>> = RefCell::new(LayoutContext::new());
    static SCALER: RefCell<ScaleContext> = RefCell::new(ScaleContext::new());
    static SHAPED: RefCell<HashMap<ParaKey, Arc<Para>>> = RefCell::new(HashMap::new());
    static METRICS: RefCell<HashMap<Arc<str>, (f64, f64)>> = RefCell::new(HashMap::new());
}

/// This thread's font context, rebuilt (and the caches emptied) when the fonts changed.
fn with_fcx<R>(f: impl FnOnce(&mut Fcx) -> R) -> R {
    let current = FONTS.read().unwrap().generation;
    FCX.with(|c| {
        let mut c = c.borrow_mut();
        if c.generation != current {
            *c = font_context();
            SHAPED.with(|s| s.borrow_mut().clear());
            METRICS.with(|m| m.borrow_mut().clear());
        }
        f(&mut c)
    })
}

/// A family's ascent and descent, in em (its default face's).
fn family_metrics(family: &Arc<str>) -> (f64, f64) {
    with_fcx(|_| ());
    if let Some(m) = METRICS.with(|m| m.borrow().get(family).copied()) {
        return m;
    }
    let m = with_fcx(|c| {
        let fcx = &mut c.fcx;
        // the list's first family that exists
        let mut info = None;
        for name in parley::FontFamilyName::parse_css_list(family).flatten() {
            let fam = match name {
                parley::FontFamilyName::Named(n) => fcx.collection.family_by_name(&n),
                parley::FontFamilyName::Generic(g) => {
                    let id = fcx.collection.generic_families(g).next();
                    id.and_then(|id| fcx.collection.family(id))
                }
            };
            if let Some(f) = fam {
                info = f.default_font().cloned();
                break;
            }
        }
        info.and_then(|i| {
            let blob = i.load(Some(&mut fcx.source_cache))?;
            let font = FontRef::from_index(blob.data(), i.index() as usize)?;
            let m = font.metrics(&[]);
            let em = m.units_per_em as f64;
            Some((m.ascent as f64 / em, m.descent.abs() as f64 / em))
        })
    })
    .unwrap_or((0.9, 0.25));
    METRICS.with(|c| c.borrow_mut().insert(family.clone(), m));
    m
}

/// Horizontal alignment of a paragraph's lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Align {
    Start,
    Center,
    End,
}

/// What a paragraph is: text, family, size, weight, letter-spacing (em), line-height (px; None:
/// normal), wrap width (None: one line), alignment.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ParaKey {
    pub s: String,
    pub family: Arc<str>,
    pub size: u64,
    pub weight: u32,
    pub ls: u64,
    pub lh: Option<u64>,
    pub wrap: Option<u64>,
    pub align: Align,
    /// byte ranges drawn in their own colour (RGBA8); the rest takes the text's colour
    pub spans: Vec<(u32, u32, [u8; 4])>,
    /// wrapped lines made as even as they can be (CSS text-wrap: balance)
    pub balance: bool,
}

impl ParaKey {
    #[allow(clippy::too_many_arguments)]
    pub fn new(s: &str, family: &Arc<str>, size: f64, weight: u32, ls: f64, lh: Option<f64>, wrap: Option<f64>, align: Align) -> ParaKey {
        ParaKey {
            s: s.to_string(),
            family: family.clone(),
            size: size.to_bits(),
            weight,
            ls: ((ls * 1e4).round() / 1e4).to_bits(),
            lh: lh.map(f64::to_bits),
            wrap: wrap.map(f64::to_bits),
            align,
            spans: Vec::new(),
            balance: false,
        }
    }
    pub fn with(mut self, spans: &[(u32, u32, [u8; 4])], balance: bool) -> ParaKey {
        self.spans = spans.to_vec();
        self.balance = balance && self.wrap.is_some();
        self
    }
    pub fn size(&self) -> f64 {
        f64::from_bits(self.size)
    }
}

pub struct GlyphRun {
    /// a span's colour, or [0; 4]: the text's
    brush: [u8; 4],
    font: FontData,
    size: f32,
    coords: Vec<i16>,
    /// glyph id, x, y (y: the baseline, from the paragraph's top)
    glyphs: Vec<(u32, f32, f32)>,
}

/// A laid-out paragraph.
pub struct Para {
    pub width: f64,
    pub height: f64,
    /// each line's baseline, from the paragraph's top
    pub baselines: Vec<f64>,
    runs: Vec<GlyphRun>,
}

/// Lays a paragraph out (cached per thread).
pub fn shape(k: &ParaKey) -> Arc<Para> {
    if let Some(p) = SHAPED.with(|c| c.borrow().get(k).cloned()) {
        return p;
    }
    let p = Arc::new(shape_uncached(k));
    SHAPED.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 8192 {
            c.clear();
        }
        c.insert(k.clone(), p.clone());
    });
    p
}

fn shape_uncached(k: &ParaKey) -> Para {
    let size = k.size() as f32;
    let ls = f64::from_bits(k.ls) as f32;
    let (lh, _) = line_metrics(&k.family, k.size(), k.lh.map(f64::from_bits));
    with_fcx(|c| {
        LCX.with(|l| {
            let mut l = l.borrow_mut();
            let locale = c.locale;
            let mut b = l.ranged_builder(&mut c.fcx, &k.s, 1.0, true);
            // (a CSS font-family list: names, then generics such as sans-serif)
            b.push_default(StyleProperty::FontFamily(FontFamily::Source(std::borrow::Cow::Borrowed(&k.family))));
            b.push_default(StyleProperty::FontSize(size));
            b.push_default(StyleProperty::FontWeight(FontWeight::new(k.weight as f32)));
            b.push_default(StyleProperty::LetterSpacing(ls * size));
            b.push_default(StyleProperty::LineHeight(LineHeight::Absolute(lh as f32)));
            b.push_default(StyleProperty::Locale(locale));
            for &(a, z, col) in &k.spans {
                b.push(StyleProperty::Brush(col), a as usize..z as usize);
            }
            let mut layout = b.build(&k.s);
            let wrap = k.wrap.map(|w| f64::from_bits(w) as f32);
            layout.break_all_lines(wrap);
            // balanced: the narrowest width that keeps the line count, the block then placed
            // in the full width as the lines would be
            let mut dx = 0.0;
            if let (true, Some(w)) = (k.balance, wrap) {
                let n = layout.lines().count();
                if n > 1 {
                    let (mut lo, mut hi) = (w / n as f32 * 0.5, w);
                    for _ in 0..16 {
                        let mid = (lo + hi) / 2.0;
                        layout.break_all_lines(Some(mid));
                        if layout.lines().count() <= n { hi = mid } else { lo = mid }
                    }
                    layout.break_all_lines(Some(hi));
                    dx = match k.align {
                        Align::Start => 0.0,
                        Align::Center => (w - hi) as f64 / 2.0,
                        Align::End => (w - hi) as f64,
                    };
                }
            }
            let align = match k.align {
                Align::Start => Alignment::Start,
                Align::Center => Alignment::Center,
                Align::End => Alignment::End,
            };
            layout.align(align, AlignmentOptions::default());
            let mut runs = Vec::new();
            let mut baselines = Vec::new();
            for line in layout.lines() {
                baselines.push(line.metrics().baseline as f64);
                for item in line.items() {
                    if let PositionedLayoutItem::GlyphRun(gr) = item {
                        let run = gr.run();
                        runs.push(GlyphRun {
                            brush: gr.style().brush,
                            font: run.font().clone(),
                            size: run.font_size(),
                            coords: run.normalized_coords().iter().map(|c| c.to_bits()).collect(),
                            glyphs: gr.positioned_glyphs().map(|g| (g.id, g.x + dx as f32, g.y)).collect(),
                        });
                    }
                }
            }
            Para { width: layout.width() as f64, height: layout.height() as f64, baselines, runs }
        })
    })
}

/// One line's advance (letter-spacing after every character, as CSS).
pub fn measure(text: &str, family: &Arc<str>, size: f64, weight: u32, ls: f64) -> f64 {
    shape(&ParaKey::new(text, family, size, weight, ls, None, None, Align::Start)).width
}

fn jr(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// (line box height, baseline offset from its top), as Chrome computes them.
pub fn line_metrics(family: &Arc<str>, size: f64, line_height: Option<f64>) -> (f64, f64) {
    let (asc, desc) = family_metrics(family);
    let (a, d) = (jr(asc * size), jr(desc * size));
    let lh = line_height.unwrap_or(a + d);
    (lh, (lh - (a + d)) / 2.0 + a)
}

/// The margin around a text image (room for overhangs and swashes).
pub fn pad(size: f64) -> f64 {
    (size * 0.35).ceil() + 4.0
}

/// A paragraph's image size and where its first baseline sits in it: (w, h, pad, base).
/// One-line text keeps the Chrome line box: its baseline at pad + ascent.
pub fn image_box(k: &ParaKey) -> (f64, f64, f64, f64) {
    let p = shape(k);
    let size = k.size();
    let pad = pad(size);
    if k.wrap.is_none() && p.baselines.len() <= 1 {
        let (asc, desc) = family_metrics(&k.family);
        let w = p.width.ceil() + 2.0 * pad;
        let h = (size * (asc + desc)).ceil() + 2.0 * pad;
        return (w, h, pad, pad + size * asc);
    }
    let w = k.wrap.map(f64::from_bits).unwrap_or(p.width).max(p.width).ceil() + 2.0 * pad;
    let h = p.height.ceil() + 2.0 * pad;
    (w, h, pad, pad + p.baselines.first().copied().unwrap_or(size))
}

/// measureText(): [width, height, lines, first baseline]. Without a width, one line in its
/// line box; with one, the paragraph wrapped at it.
pub fn metrics(s: &str, t: &Ts) -> [f64; 4] {
    let (line, base) = line_metrics(&t.font, t.size, t.lh);
    match t.width {
        None => [measure(s, &t.font, t.size, t.weight, t.ls), line, 1.0, base],
        Some(w) => {
            let p = shape(&ParaKey::new(s, &t.font, t.size, t.weight, t.ls, t.lh, Some(w), t.align).with(&t.spans, t.balance));
            [p.width, p.height, p.baselines.len() as f64, p.baselines.first().copied().unwrap_or(base)]
        }
    }
}

pub struct Raster {
    pub w: usize,
    pub h: usize,
    /// premultiplied RGBA
    pub data: Vec<u8>,
}

/// The paragraph's image in one colour.
pub fn raster(k: &ParaKey, color: [u8; 3], alpha: f32) -> Raster {
    let p = shape(k);
    let (wf, hf, pad, base) = image_box(k);
    let (w, h) = (wf as usize, hf as usize);
    // glyph y is measured from the paragraph's top; move the first baseline to `base`
    let dy = base - p.baselines.first().copied().unwrap_or(0.0);
    // one coverage mask per colour (the text's first, then each span's)
    let mut masks: Vec<([u8; 4], Vec<f32>)> = vec![([0; 4], vec![0f32; w * h])];
    let mut color_px: Vec<(usize, [f32; 4])> = Vec::new();
    SCALER.with(|s| {
        let mut s = s.borrow_mut();
        for run in &p.runs {
            let Some(font) = FontRef::from_index(run.font.data.data(), run.font.index as usize) else {
                continue;
            };
            let mi = match masks.iter().position(|m| m.0 == run.brush) {
                Some(i) => i,
                None => {
                    masks.push((run.brush, vec![0f32; w * h]));
                    masks.len() - 1
                }
            };
            let mask = &mut masks[mi].1;
            let mut scaler = s.builder(font).size(run.size).normalized_coords(run.coords.iter()).hint(false).build();
            for &(id, gx, gy) in &run.glyphs {
                let px = pad + gx as f64;
                let py = dy + gy as f64;
                let (ix, iy) = (px.floor(), py.round());
                let Some(img) = Render::new(&[Source::ColorOutline(0), Source::ColorBitmap(StrikeWith::BestFit), Source::Outline])
                    .offset(Vector::new((px - ix) as f32, 0.0))
                    .render(&mut scaler, id as u16)
                else {
                    continue;
                };
                let pl = img.placement;
                let colored = matches!(img.content, Content::Color);
                for yy in 0..pl.height as i64 {
                    let oy = iy as i64 - pl.top as i64 + yy;
                    if oy < 0 || oy >= h as i64 {
                        continue;
                    }
                    for xx in 0..pl.width as i64 {
                        let ox = ix as i64 + pl.left as i64 + xx;
                        if ox < 0 || ox >= w as i64 {
                            continue;
                        }
                        let o = oy as usize * w + ox as usize;
                        let i = (yy * pl.width as i64 + xx) as usize;
                        if colored {
                            let c = &img.data[i * 4..i * 4 + 4];
                            let a = c[3] as f32 / 255.0;
                            if a > 0.0 {
                                color_px.push((o, [c[0] as f32 / 255.0 * a, c[1] as f32 / 255.0 * a, c[2] as f32 / 255.0 * a, a]));
                            }
                        } else {
                            let v = img.data[i] as f32 / 255.0;
                            mask[o] = mask[o].max(v);
                        }
                    }
                }
            }
        }
    });
    let mut out = vec![[0f32; 4]; w * h];
    for (brush, mask) in &masks {
        // (a span's colour carries its own alpha; the text's alpha applies to all)
        let (col, a) = if *brush == [0; 4] {
            (color.map(|c| c as f32 / 255.0), alpha)
        } else {
            ([brush[0], brush[1], brush[2]].map(|c| c as f32 / 255.0), alpha * brush[3] as f32 / 255.0)
        };
        for (d, m) in out.iter_mut().zip(mask) {
            if *m <= 0.0 {
                continue;
            }
            let m = m * a;
            let src = [col[0] * m, col[1] * m, col[2] * m, m];
            for i in 0..4 {
                d[i] = src[i] + d[i] * (1.0 - m);
            }
        }
    }
    // colour glyphs (emoji) over the text, in their own colours
    for (o, c) in color_px {
        let d = &mut out[o];
        let c = c.map(|v| v * alpha);
        for i in 0..4 {
            d[i] = c[i] + d[i] * (1.0 - c[3]);
        }
    }
    let data = out.iter().flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect();
    Raster { w, h, data }
}

/// Lanczos3 resize of premultiplied RGBA8.
pub fn resize_premul(data: Vec<u8>, w: u32, h: u32, dw: u32, dh: u32) -> Vec<u8> {
    let src = fr::images::Image::from_vec_u8(w, h, data, fr::PixelType::U8x4).unwrap();
    let mut dst = fr::images::Image::new(dw, dh, fr::PixelType::U8x4);
    let opts = fr::ResizeOptions::new().resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::Lanczos3)).use_alpha(false);
    fr::Resizer::new().resize(&src, &mut dst, &opts).unwrap();
    dst.into_vec()
}
