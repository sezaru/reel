//! The video's config (the object given to `video()`), and the times the command line takes.
use std::path::PathBuf;

use anyhow::{Context as _, anyhow, bail};
use rquickjs::{Array, Object, Value};

use crate::canvas::Col;
use crate::script::resolve;

#[derive(Clone, Debug)]
pub struct Audio {
    pub src: PathBuf,
    /// video frame it starts at
    pub at: f64,
    /// seconds into the file
    pub from: f64,
    pub duration: Option<f64>,
    /// dB
    pub volume: f64,
    pub fade_in: f64,
    pub fade_out: f64,
    pub voice: bool,
    pub duck: bool,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: usize,
    pub background: Col,
    pub fonts: Vec<PathBuf>,
    pub font: Option<String>,
    pub lang: Option<String>,
    /// (shutter, most sub-frames), or None: off
    pub motion_blur: Option<(f64, usize)>,
    pub audio: Vec<Audio>,
    pub loudness: Option<f64>,
    /// named frames, in order
    pub markers: Vec<(String, f64)>,
}

fn num(o: &Object, k: &str) -> anyhow::Result<Option<f64>> {
    let v: Value = o.get(k).map_err(|e| anyhow!("{k}: {e}"))?;
    if v.is_undefined() || v.is_null() {
        return Ok(None);
    }
    match v.as_number() {
        Some(n) if n.is_finite() => Ok(Some(n)),
        _ => bail!("{k} must be a number, got {v:?}"),
    }
}

fn string(o: &Object, k: &str) -> anyhow::Result<Option<String>> {
    let v: Value = o.get(k).map_err(|e| anyhow!("{k}: {e}"))?;
    if v.is_undefined() || v.is_null() {
        return Ok(None);
    }
    Ok(Some(v.as_string().ok_or_else(|| anyhow!("{k} must be a string"))?.to_string()?))
}

/// Font paths and globs → files.
fn font_files(pats: &[String]) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for p in pats {
        let full = resolve(p);
        let s = full.display().to_string();
        if s.contains(['*', '?', '[']) {
            let mut found: Vec<PathBuf> = glob::glob(&s)?.filter_map(Result::ok).collect();
            found.sort();
            if found.is_empty() {
                bail!("fonts: nothing matches {p}");
            }
            out.extend(found);
        } else {
            if !full.is_file() {
                bail!("fonts: no file {p}");
            }
            out.push(full);
        }
    }
    Ok(out)
}

impl Config {
    pub fn from_js(o: &Object) -> anyhow::Result<Config> {
        let width = num(o, "width")?.unwrap_or(1920.0);
        let height = num(o, "height")?.unwrap_or(1080.0);
        let fps = num(o, "fps")?.unwrap_or(60.0);
        let duration = num(o, "duration")?.ok_or_else(|| anyhow!("duration (in frames) is required: `duration: seconds(10)`"))?;
        if width < 2.0 || height < 2.0 || fps <= 0.0 || duration < 1.0 {
            bail!("width, height, fps and duration must be positive (got {width}×{height}, {fps} fps, {duration} frames)");
        }
        let background = match string(o, "background")? {
            Some(s) => crate::color::parse(&s).ok_or_else(|| anyhow!("background: not a colour: {s}"))?,
            None => [0.0, 0.0, 0.0, 1.0],
        };
        let fonts: Vec<String> = match o.get::<_, Option<Array>>("fonts").map_err(|e| anyhow!("fonts: {e}"))? {
            Some(a) => a.iter::<String>().collect::<Result<_, _>>().map_err(|e| anyhow!("fonts: {e}"))?,
            None => vec![],
        };
        let mb: Value = o.get("motionBlur").map_err(|e| anyhow!("motionBlur: {e}"))?;
        let motion_blur = if let Some(b) = mb.as_bool() {
            b.then_some((0.5, 16))
        } else if let Some(m) = mb.as_object() {
            Some((num(m, "shutter")?.unwrap_or(0.5), num(m, "maxSamples")?.unwrap_or(16.0) as usize))
        } else {
            Some((0.5, 16))
        };
        let mut audio = Vec::new();
        if let Some(a) = o.get::<_, Option<Array>>("audio").map_err(|e| anyhow!("audio: {e}"))? {
            for (i, t) in a.iter::<Object>().enumerate() {
                let t = t.map_err(|e| anyhow!("audio[{i}]: {e}"))?;
                let src = string(&t, "src")?.ok_or_else(|| anyhow!("audio[{i}]: src is required"))?;
                audio.push(Audio {
                    src: resolve(&src),
                    at: num(&t, "at")?.unwrap_or(0.0),
                    from: num(&t, "from")?.unwrap_or(0.0),
                    duration: num(&t, "duration")?,
                    volume: num(&t, "volume")?.unwrap_or(0.0),
                    fade_in: num(&t, "fadeIn")?.unwrap_or(0.0),
                    fade_out: num(&t, "fadeOut")?.unwrap_or(0.0),
                    voice: t.get::<_, Option<bool>>("voice").ok().flatten().unwrap_or(false),
                    duck: t.get::<_, Option<bool>>("duck").ok().flatten().unwrap_or(false),
                });
            }
        }
        let mut markers = Vec::new();
        if let Some(m) = o.get::<_, Option<Object>>("markers").map_err(|e| anyhow!("markers: {e}"))? {
            for k in m.keys::<String>() {
                let k = k.map_err(|e| anyhow!("markers: {e}"))?;
                markers.push((k.clone(), num(&m, &k)?.unwrap_or(0.0)));
            }
            markers.sort_by(|a, b| a.1.total_cmp(&b.1));
        }
        // NV12 wants even sizes
        let even = |v: f64| ((v.round() as u32) / 2 * 2).max(2);
        Ok(Config {
            width: even(width),
            height: even(height),
            fps,
            duration: duration.round() as usize,
            background,
            fonts: font_files(&fonts)?,
            font: string(o, "font")?,
            lang: string(o, "lang")?,
            motion_blur,
            audio,
            loudness: num(o, "loudness")?,
            markers,
        })
    }

    /// A time as the command line writes it → a frame: `120` (a frame), `2.5s`, `1:05.5`
    /// (minutes:seconds), a marker's name, `end` (the last frame).
    pub fn frame_of(&self, t: &str) -> anyhow::Result<f64> {
        let t = t.trim();
        if t == "end" {
            return Ok(self.duration as f64 - 1.0);
        }
        if let Some((_, f)) = self.markers.iter().find(|(k, _)| k == t) {
            return Ok(*f);
        }
        if let Some(s) = t.strip_suffix('s') {
            return Ok((s.parse::<f64>().with_context(|| format!("not a time: {t}"))? * self.fps).round());
        }
        if let Some((m, s)) = t.split_once(':') {
            let secs = m.parse::<f64>().with_context(|| format!("not a time: {t}"))? * 60.0 + s.parse::<f64>().with_context(|| format!("not a time: {t}"))?;
            return Ok((secs * self.fps).round());
        }
        t.parse::<f64>().map_err(|_| {
            let names: Vec<&str> = self.markers.iter().map(|m| m.0.as_str()).collect();
            anyhow!("not a time: {t:?} (a frame, 2.5s, 1:05.5, end, or a marker: {})", if names.is_empty() { "none defined".into() } else { names.join(", ") })
        })
    }
}
