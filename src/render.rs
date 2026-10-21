//! Rendering: worker threads run the scene (each its own copy) and record each frame's
//! display lists, decoding the images they will need ahead of the GPU; the GPU draws the
//! frame (several sub-frames averaged where something moves: motion blur) straight into
//! BT.709 NV12 planes; ffmpeg encodes them and mixes the soundtrack.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow, bail};

use crate::canvas::{Fill, Op, TexKey, motion};
use crate::config::Config;
use crate::gpu::{Decoded, Gpu};
use crate::image::decode;
use crate::script::Script;

const LOOKAHEAD: usize = 24;
const RING: usize = 3;

/// What a scene is run with.
#[derive(Clone)]
pub struct Scene {
    pub entry: PathBuf,
    pub params: Vec<(String, String)>,
}

impl Scene {
    pub fn load(&self) -> anyhow::Result<Script> {
        Script::load(&self.entry, &self.params)
    }
}

/// The frame's sub-frames: one display list, or several across the shutter if anything moves
/// more than a third of a pixel in it.
pub fn plan(script: &Script, f: usize, scale: f64, blur: Option<(f64, usize)>) -> anyhow::Result<Vec<Vec<Op>>> {
    let f = f as f64;
    let a = script.draw(f, scale)?;
    let Some((shutter, most)) = blur else { return Ok(vec![a.ops]) };
    let b = script.draw(f + shutter, scale)?;
    let n = match motion(&a.sigs, &b.sigs) {
        None => 4.min(most),
        Some(d) if d * scale < 0.35 => return Ok(vec![a.ops]),
        Some(d) => most.min((d * scale).ceil() as usize + 1),
    };
    let mut out = vec![a.ops];
    for i in 1..n.max(1) {
        out.push(script.draw(f + shutter * i as f64 / n as f64, scale)?.ops);
    }
    Ok(out)
}

pub fn tex_keys(samples: &[Vec<Op>]) -> impl Iterator<Item = &TexKey> {
    samples.iter().flatten().filter_map(|op| match op {
        Op::Box(b) => match &b.fill {
            Fill::Tex { key, .. } => Some(key),
            _ => None,
        },
        _ => None,
    })
}

pub struct Opts {
    pub out: Option<PathBuf>,
    pub frames: (usize, usize),
    pub scale: f64,
    pub crf: Option<String>,
    pub preset: String,
    pub encoder: String,
    pub workers: usize,
    pub blur: Option<(f64, usize)>,
    pub audio: bool,
}

/// What the workers share: the next frame to plan, how far the GPU has got, the plans, and
/// the images decoded ahead.
struct Shared {
    next: AtomicUsize,
    consumed: AtomicUsize,
    plans: Mutex<BTreeMap<usize, anyhow::Result<Vec<Vec<Op>>>>>,
    ready: Condvar,
    decoded: Mutex<HashMap<TexKey, Arc<Decoded>>>,
    resident: RwLock<HashSet<TexKey>>,
    stop: AtomicBool,
    plan_ns: AtomicUsize,
    decode_ns: AtomicUsize,
}

fn worker(sh: Arc<Shared>, scene: Scene, end: usize, scale: f64, blur: Option<(f64, usize)>) {
    let script = match scene.load() {
        Ok(s) => s,
        Err(e) => {
            let f = sh.next.load(Ordering::SeqCst);
            sh.plans.lock().unwrap().insert(f, Err(e));
            sh.ready.notify_all();
            return;
        }
    };
    loop {
        let f = sh.next.fetch_add(1, Ordering::SeqCst);
        if f >= end || sh.stop.load(Ordering::SeqCst) {
            return;
        }
        while f >= sh.consumed.load(Ordering::SeqCst) + LOOKAHEAD {
            if sh.stop.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_micros(500));
        }
        let t = Instant::now();
        let p = plan(&script, f, scale, blur);
        sh.plan_ns.fetch_add(t.elapsed().as_nanos() as usize, Ordering::Relaxed);
        // decode what the GPU doesn't hold yet
        let t = Instant::now();
        if let Ok(p) = &p {
            let mut seen = HashSet::new();
            for k in tex_keys(p) {
                if !seen.insert(k) || sh.resident.read().unwrap().contains(k) || sh.decoded.lock().unwrap().contains_key(k) {
                    continue;
                }
                let d = Arc::new(decode(k));
                sh.decoded.lock().unwrap().insert(k.clone(), d);
            }
        }
        sh.decode_ns.fetch_add(t.elapsed().as_nanos() as usize, Ordering::Relaxed);
        sh.plans.lock().unwrap().insert(f, p);
        sh.ready.notify_all();
    }
}

/// ffmpeg's arguments for the soundtrack: inputs, and the filter graph ending in [a].
fn audio_graph(cfg: &Config, first_input: usize) -> anyhow::Result<(Vec<String>, String)> {
    let total = cfg.duration as f64 / cfg.fps;
    let mut inputs = Vec::new();
    let mut filters = Vec::new();
    let (mut voices, mut ducked, mut plain) = (Vec::new(), Vec::new(), Vec::new());
    for (i, t) in cfg.audio.iter().enumerate() {
        if !t.src.is_file() {
            bail!("audio: no file {}", t.src.display());
        }
        let start = t.at / cfg.fps;
        let len = t.duration.unwrap_or(total - start).min(total - start).max(0.0);
        inputs.extend(["-ss".into(), format!("{:.4}", t.from), "-t".into(), format!("{len:.4}"), "-i".into(), t.src.display().to_string()]);
        let mut f = format!("[{}:a]aresample=48000,aformat=channel_layouts=stereo,volume={}dB", first_input + i, t.volume);
        if t.fade_in > 0.0 {
            f += &format!(",afade=t=in:d={}", t.fade_in);
        }
        if t.fade_out > 0.0 {
            f += &format!(",afade=t=out:st={:.4}:d={}", (len - t.fade_out).max(0.0), t.fade_out);
        }
        let ms = (start * 1000.0).round() as i64;
        if ms > 0 {
            f += &format!(",adelay={ms}|{ms}");
        }
        f += &format!("[t{i}]");
        filters.push(f);
        let label = format!("[t{i}]");
        if t.voice {
            voices.push(label);
        } else if t.duck {
            ducked.push(label);
        } else {
            plain.push(label);
        }
    }
    let mut mix = plain;
    // (padded with silence: a mix that ends early would end the ducked bed with it)
    let mix_of = |labels: &[String], out: &str, filters: &mut Vec<String>| {
        if labels.len() == 1 {
            filters.push(format!("{}apad[{out}]", labels[0]));
        } else {
            filters.push(format!("{}amix=inputs={}:normalize=0,apad[{out}]", labels.concat(), labels.len()));
        }
    };
    if !voices.is_empty() {
        mix_of(&voices, "vo", &mut filters);
        if ducked.is_empty() {
            mix.push("[vo]".into());
        } else {
            // the beds dip while the voice speaks
            filters.push("[vo]asplit=2[vo_mix][vo_key]".into());
            mix_of(&ducked, "bed", &mut filters);
            filters.push("[bed][vo_key]sidechaincompress=threshold=0.02:ratio=6:attack=25:release=500:makeup=1[duck]".into());
            mix.push("[vo_mix]".into());
            mix.push("[duck]".into());
        }
    } else {
        mix.extend(ducked);
    }
    let tail = match cfg.loudness {
        Some(l) => format!(",loudnorm=I={l}:TP=-1.5:LRA=11"),
        None => String::new(),
    };
    if mix.len() == 1 {
        filters.push(format!("{}apad{tail}[a]", mix[0]));
    } else {
        filters.push(format!("{}amix=inputs={}:normalize=0,apad{tail}[a]", mix.concat(), mix.len()));
    }
    Ok((inputs, filters.join(";")))
}

/// Renders frames [a, b) to a video file (or nowhere: `out` None times the drawing alone).
pub fn render(scene: &Scene, cfg: &Config, o: &Opts) -> anyhow::Result<()> {
    let t_start = Instant::now();
    let mut gpu = Gpu::new((cfg.width, cfg.height), o.scale, 1500 << 20, cfg.background)?;
    let (w, h) = (gpu.w, gpu.h);
    let (start, end) = o.frames;
    eprintln!(
        "reel: {} frames ({start}–{end}) at {w}×{h}, {} fps; {}; {} workers; {}",
        end - start,
        cfg.fps,
        if o.blur.is_some() { "motion blur" } else { "no motion blur" },
        o.workers,
        gpu.adapter
    );

    // the encoder: ffmpeg reading raw NV12, fed from its own thread
    let (tx, rx) = crossbeam_channel::bounded::<Vec<u8>>(8);
    let (back_tx, back_rx) = crossbeam_channel::unbounded::<Vec<u8>>();
    let mut enc = None;
    if let Some(out) = &o.out {
        if let Some(d) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(d)?;
        }
        let tags = ["-color_range", "tv", "-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709"];
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "nv12", "-s", &format!("{w}x{h}"), "-r", &cfg.fps.to_string()])
            .args(tags)
            .args(["-i", "-"]);
        let with_audio = o.audio && !cfg.audio.is_empty();
        if with_audio {
            let (inputs, graph) = audio_graph(cfg, 1)?;
            // (the soundtrack starts where the rendered range does)
            let offset = start as f64 / cfg.fps;
            if offset > 0.0 {
                cmd.args(["-filter_complex", &format!("{graph};[a]atrim=start={offset:.4},asetpts=PTS-STARTPTS[a2]")]);
            } else {
                cmd.args(["-filter_complex", &graph]);
            }
            cmd.args(&inputs);
            cmd.args(["-map", "0:v", "-map", if offset > 0.0 { "[a2]" } else { "[a]" }, "-c:a", "aac", "-b:a", "160k", "-ar", "48000"]);
            cmd.args(["-t", &format!("{:.4}", (end - start) as f64 / cfg.fps)]);
        }
        // (nvenc's -cq 25 makes about the file x264's CRF 16 does, at SSIM 0.994 to it)
        match o.encoder.as_str() {
            "x264" => cmd.args(["-c:v", "libx264", "-preset", &o.preset, "-crf", o.crf.as_deref().unwrap_or("16"), "-pix_fmt", "yuv420p"]),
            "nvenc" => cmd.args(["-c:v", "h264_nvenc", "-preset", "p7", "-tune", "hq", "-rc", "vbr", "-cq", o.crf.as_deref().unwrap_or("25"), "-b:v", "0", "-profile:v", "high"]),
            e => bail!("unknown encoder {e} (x264 or nvenc)"),
        };
        cmd.args(tags).args(["-movflags", "+faststart"]).arg(out).stdin(Stdio::piped());
        let mut child = cmd.spawn().context("starting ffmpeg (is it installed?)")?;
        let mut stdin = child.stdin.take().unwrap();
        let back = back_tx.clone();
        let writer = std::thread::spawn(move || -> std::io::Result<()> {
            for buf in rx {
                stdin.write_all(&buf)?;
                let _ = back.send(buf);
            }
            Ok(())
        });
        enc = Some((child, writer));
    }

    let sh = Arc::new(Shared {
        next: AtomicUsize::new(start),
        consumed: AtomicUsize::new(start),
        plans: Mutex::new(BTreeMap::new()),
        ready: Condvar::new(),
        decoded: Mutex::new(HashMap::new()),
        resident: RwLock::new(HashSet::new()),
        stop: AtomicBool::new(false),
        plan_ns: AtomicUsize::new(0),
        decode_ns: AtomicUsize::new(0),
    });
    let workers: Vec<_> = (0..o.workers)
        .map(|_| {
            let (sh, scene, scale, blur) = (sh.clone(), scene.clone(), o.scale, o.blur);
            std::thread::spawn(move || worker(sh, scene, end, scale, blur))
        })
        .collect();

    let mut timing: BTreeMap<&str, Duration> = BTreeMap::new();
    let mut hist: BTreeMap<usize, usize> = BTreeMap::new();
    let add = |k: &'static str, d: Duration, timing: &mut BTreeMap<&str, Duration>| *timing.entry(k).or_default() += d;
    let t_loop = Instant::now();
    let mut pending: Option<usize> = None;
    let mut done = 0;
    let result = (|| -> anyhow::Result<()> {
        for f in start..end {
            let t = Instant::now();
            let samples = {
                let mut plans = sh.plans.lock().unwrap();
                loop {
                    // (a worker that failed to load reports under the first frame it would draw)
                    if let Some(k) = plans.keys().next().copied()
                        && plans[&k].is_err()
                        && let Some(Err(e)) = plans.remove(&k)
                    {
                        return Err(e);
                    }
                    if let Some(p) = plans.remove(&f) {
                        break p?;
                    }
                    plans = sh.ready.wait_timeout(plans, Duration::from_millis(200)).unwrap().0;
                }
            };
            add("wait for plans", t.elapsed(), &mut timing);
            *hist.entry(samples.len()).or_default() += 1;

            let t = Instant::now();
            let slot = (f - start) % RING;
            let mut uploaded = Vec::new();
            let evicted = {
                let shc = sh.clone();
                let mut get = |k: &TexKey| {
                    uploaded.push(k.clone());
                    if let Some(d) = shc.decoded.lock().unwrap().remove(k) {
                        return d;
                    }
                    Arc::new(decode(k))
                };
                gpu.submit_nv12(&samples, f as u64, slot, &mut get)
            };
            {
                let mut r = sh.resident.write().unwrap();
                for k in uploaded {
                    r.insert(k);
                }
                for k in &evicted {
                    r.remove(k);
                }
                // (decoded ahead, but the GPU already had it)
                sh.decoded.lock().unwrap().retain(|k, _| !r.contains(k));
            }
            add("record + submit", t.elapsed(), &mut timing);
            sh.consumed.store(f + 1, Ordering::SeqCst);

            if let Some(prev) = pending.replace(slot) {
                let t = Instant::now();
                let mut buf = back_rx.try_recv().unwrap_or_default();
                gpu.collect_nv12(prev, &mut buf);
                add("gpu wait + readback", t.elapsed(), &mut timing);
                if enc.is_some() {
                    let t = Instant::now();
                    tx.send(buf).map_err(|_| anyhow!("ffmpeg stopped reading"))?;
                    add("encoder wait", t.elapsed(), &mut timing);
                }
            }
            done += 1;
            if done % 240 == 0 {
                eprintln!("frame {f} ({done}/{}) {:.1} fps", end - start, done as f64 / t_loop.elapsed().as_secs_f64());
            }
        }
        if let Some(prev) = pending {
            let mut buf = Vec::new();
            gpu.collect_nv12(prev, &mut buf);
            if enc.is_some() {
                tx.send(buf).map_err(|_| anyhow!("ffmpeg stopped reading"))?;
            }
        }
        Ok(())
    })();
    sh.stop.store(true, Ordering::SeqCst);
    for wk in workers {
        let _ = wk.join();
    }
    let loop_s = t_loop.elapsed().as_secs_f64();
    drop(tx);
    if let Some((mut child, writer)) = enc {
        let t = Instant::now();
        let w = writer.join().unwrap();
        let st = child.wait()?;
        if result.is_ok() {
            w.context("writing to ffmpeg")?;
            anyhow::ensure!(st.success(), "ffmpeg failed: {st}");
        }
        add("encoder drain", t.elapsed(), &mut timing);
    }
    result?;
    let total = t_start.elapsed().as_secs_f64();
    eprintln!("done: {done} frames in {total:.1} s ({:.1} fps drawing; setup {:.1} s)", done as f64 / loop_s, (t_loop - t_start).as_secs_f64());
    let mut parts: Vec<String> = timing.iter().map(|(k, v)| format!("{k} {:.1} s", v.as_secs_f64())).collect();
    parts.push(format!(
        "workers: run {:.1} s, decode {:.1} s (summed over threads)",
        sh.plan_ns.load(Ordering::Relaxed) as f64 / 1e9,
        sh.decode_ns.load(Ordering::Relaxed) as f64 / 1e9
    ));
    eprintln!("time: {}", parts.join(", "));
    eprintln!("sub-frames: {}", hist.iter().map(|(n, c)| format!("{n}×: {c}")).collect::<Vec<_>>().join(", "));
    if let Some(out) = &o.out {
        eprintln!("wrote {}", out.display());
    }
    Ok(())
}

pub fn write_png(path: &Path, w: u32, h: u32, rgb: &[u8]) -> anyhow::Result<()> {
    if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d)?;
    }
    let file = std::fs::File::create(path).with_context(|| format!("{}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgb)?;
    Ok(())
}

/// Single frames as PNGs: `out` is the file (one frame) or a directory (several, named by
/// frame number).
pub fn stills(script: &Script, frames: &[usize], scale: f64, blur: Option<(f64, usize)>, out: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let cfg = &script.config;
    let mut gpu = Gpu::new((cfg.width, cfg.height), scale, 1500 << 20, cfg.background)?;
    let mut get = |k: &TexKey| Arc::new(decode(k));
    let one = frames.len() == 1 && out.extension().is_some_and(|e| e == "png");
    let mut written = Vec::new();
    for &f in frames {
        let rgb = gpu.still_rgb(&plan(script, f, scale, blur)?, &mut get);
        let path = if one { out.to_path_buf() } else { out.join(format!("{f:05}.png")) };
        write_png(&path, gpu.w, gpu.h, &rgb)?;
        written.push(path);
    }
    Ok(written)
}

/// A contact sheet: frames in a grid, each labelled with its frame and time.
pub fn sheet(script: &Script, frames: &[usize], cols: usize, width: u32, blur: Option<(f64, usize)>, out: &Path) -> anyhow::Result<()> {
    let cfg = &script.config;
    let cols = cols.max(1).min(frames.len().max(1));
    let gap = 8u32;
    let label_h = 26u32;
    let tw = ((width - gap * (cols as u32 + 1)) / cols as u32).max(16);
    let scale = tw as f64 / cfg.width as f64;
    let mut gpu = Gpu::new((cfg.width, cfg.height), scale, 1500 << 20, cfg.background)?;
    let (gw, gh) = (gpu.w, gpu.h);
    let rows = frames.len().div_ceil(cols);
    let (sw, sh) = (gap + cols as u32 * (gw + gap), gap + rows as u32 * (gh + label_h + gap));
    let mut img = vec![24u8; (sw * sh * 3) as usize];
    let mut get = |k: &TexKey| Arc::new(decode(k));
    let font = crate::text::default_family();
    for (i, &f) in frames.iter().enumerate() {
        let rgb = gpu.still_rgb(&plan(script, f, scale, blur)?, &mut get);
        let (cx, cy) = (gap + (i % cols) as u32 * (gw + gap), gap + (i / cols) as u32 * (gh + label_h + gap));
        for y in 0..gh {
            let src = &rgb[(y * gw * 3) as usize..((y + 1) * gw * 3) as usize];
            let o = (((cy + y) * sw + cx) * 3) as usize;
            img[o..o + src.len()].copy_from_slice(src);
        }
        // the label, drawn with reel's own text
        let s = format!("{f}  ·  {:.2}s", f as f64 / cfg.fps);
        let key = crate::text::ParaKey::new(&s, &font, 15.0, 600, 0.0, None, None, crate::text::Align::Start);
        let r = crate::text::raster(&key, [230, 230, 230], 1.0);
        let pad = crate::text::pad(15.0) as i64;
        for y in 0..r.h as i64 {
            for x in 0..r.w as i64 {
                let (px, py) = (cx as i64 + x - pad + 2, (cy + gh) as i64 + y - pad + 5);
                if px < 0 || py < 0 || px >= sw as i64 || py >= sh as i64 {
                    continue;
                }
                let a = r.data[((y * r.w as i64 + x) * 4 + 3) as usize] as u32;
                let o = ((py as u32 * sw + px as u32) * 3) as usize;
                for c in 0..3 {
                    let s = r.data[((y * r.w as i64 + x) * 4 + c) as usize] as u32;
                    img[o + c as usize] = (s + img[o + c as usize] as u32 * (255 - a) / 255).min(255) as u8;
                }
            }
        }
    }
    write_png(out, sw, sh, &img)
}
