//! reel: videos from TypeScript scene files, drawn on the GPU.
//!
//! A scene file default-exports `video(config, draw)`; `draw(cv, f)` records frame f's shapes
//! (boxes, images, text, paths, custom shaders, layers) on a canvas; reel draws them with
//! wgpu (motion blur where things move) and encodes with ffmpeg. See `reel docs`.
mod anim;
mod canvas;
mod check;
mod color;
mod config;
mod docs;
mod gpu;
mod image;
mod layout;
mod paths;
mod preview;
mod render;
mod script;
mod shader;
mod text;

use std::path::PathBuf;

use anyhow::{Context as _, bail};
use clap::{Args, Parser, Subcommand};

use render::Scene;

#[derive(Parser)]
#[command(name = "reel", version, about = "Videos from TypeScript scene files, drawn on the GPU.", long_about = None)]
#[command(after_help = "Start with `reel docs` (the guide), `reel docs api` (every function), `reel init` (a new project).\nTimes (--at, --from, --to, --every) take a frame (120), seconds (2.5s), minutes:seconds (1:05.5), `end`, or a marker's name.")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone)]
struct SceneArgs {
    /// The scene file (.ts)
    scene: PathBuf,
    /// A value for the scene's `params` (repeatable): --set lang=pt-BR
    #[arg(long = "set", value_name = "KEY=VALUE")]
    set: Vec<String>,
}

impl SceneArgs {
    fn scene(&self) -> anyhow::Result<Scene> {
        let mut params = Vec::new();
        for kv in &self.set {
            let (k, v) = kv.split_once('=').with_context(|| format!("--set {kv}: KEY=VALUE"))?;
            params.push((k.to_string(), v.to_string()));
        }
        if !self.scene.is_file() {
            bail!("no scene file {}", self.scene.display());
        }
        Ok(Scene { entry: self.scene.clone(), params })
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Render the video to a file (H.264 MP4, with the scene's audio)
    Render {
        #[command(flatten)]
        s: SceneArgs,
        /// Output file (default: the scene's name, .mp4, in the current directory)
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Output size as a fraction of the design size (0.5: a half-size draft)
        #[arg(long, default_value_t = 1.0)]
        scale: f64,
        /// First frame (a time)
        #[arg(long)]
        from: Option<String>,
        /// Frame to stop before (a time)
        #[arg(long)]
        to: Option<String>,
        /// x264 (CPU, default) or nvenc (NVIDIA GPU, much faster)
        #[arg(long, default_value = "x264")]
        encoder: String,
        /// Quality: x264's CRF (default 16) or nvenc's CQ (default 25); lower is better
        #[arg(long)]
        crf: Option<String>,
        /// x264 preset
        #[arg(long, default_value = "slow")]
        preset: String,
        /// Draw each frame once (faster drafts)
        #[arg(long)]
        no_motion_blur: bool,
        /// Leave the soundtrack out
        #[arg(long)]
        no_audio: bool,
        /// Threads running the scene (default: cores − 2)
        #[arg(long)]
        workers: Option<usize>,
        /// Draw everything but write nothing (timing)
        #[arg(long)]
        null: bool,
    },
    /// Save frames as PNG images
    Still {
        #[command(flatten)]
        s: SceneArgs,
        /// Which frames (times, comma-separated): --at 0,2.5s,end
        #[arg(long, required = true)]
        at: String,
        /// A .png file (one frame) or a directory (default: stills/)
        #[arg(short, long)]
        out: Option<PathBuf>,
        #[arg(long, default_value_t = 1.0)]
        scale: f64,
        #[arg(long)]
        no_motion_blur: bool,
    },
    /// A contact sheet: frames across the video in one labelled image
    Sheet {
        #[command(flatten)]
        s: SceneArgs,
        /// One frame every this long (a time; default: 24 frames spread over the video)
        #[arg(long)]
        every: Option<String>,
        /// Or: this many frames, spread evenly
        #[arg(long)]
        count: Option<usize>,
        /// Only from / to (times)
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        to: Option<String>,
        /// Columns
        #[arg(long, default_value_t = 6)]
        cols: usize,
        /// The sheet's width in px
        #[arg(long, default_value_t = 1920)]
        width: u32,
        /// Output file (default: sheet.png)
        #[arg(short, long)]
        out: Option<PathBuf>,
        #[arg(long)]
        no_motion_blur: bool,
    },
    /// Play the video in a window; reloads when a file it uses changes
    Preview {
        #[command(flatten)]
        s: SceneArgs,
        /// Size as a fraction of the design size
        #[arg(long, default_value_t = 0.5)]
        scale: f64,
        /// Start here (a time)
        #[arg(long)]
        at: Option<String>,
        /// Start paused
        #[arg(long)]
        paused: bool,
    },
    /// Run the scene on every frame and report errors (no GPU needed)
    Check {
        #[command(flatten)]
        s: SceneArgs,
        /// Only every nth frame
        #[arg(long, default_value_t = 1)]
        every: usize,
        /// Only from / to (times)
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        to: Option<String>,
    },
    /// Start a project: reel.d.ts (types for editors), tsconfig.json, a first scene
    Init {
        /// The project directory (default: here)
        dir: Option<PathBuf>,
    },
    /// Print documentation: guide (default), api, shaders, examples
    Docs {
        topic: Option<String>,
    },
}

fn workers(n: Option<usize>) -> usize {
    n.unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8).saturating_sub(2).max(2))
}

fn blur(cfg: &config::Config, off: bool) -> Option<(f64, usize)> {
    if off { None } else { cfg.motion_blur }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Render { s, out, scale, from, to, encoder, crf, preset, no_motion_blur, no_audio, workers: w, null } => {
            let scene = s.scene()?;
            let script = scene.load()?;
            let cfg = script.config.clone();
            let a = from.map(|t| cfg.frame_of(&t)).transpose()?.unwrap_or(0.0).max(0.0) as usize;
            let b = to.map(|t| cfg.frame_of(&t)).transpose()?.map(|v| v as usize).unwrap_or(cfg.duration).min(cfg.duration);
            if a >= b {
                bail!("nothing to render: frames {a}..{b}");
            }
            drop(script);
            let out = if null {
                None
            } else {
                Some(out.unwrap_or_else(|| PathBuf::from(format!("{}.mp4", s.scene.file_stem().unwrap().to_string_lossy()))))
            };
            let o = render::Opts { out, frames: (a, b), scale, crf, preset, encoder, workers: workers(w), blur: blur(&cfg, no_motion_blur), audio: !no_audio };
            render::render(&scene, &cfg, &o)
        }
        Cmd::Still { s, at, out, scale, no_motion_blur } => {
            let scene = s.scene()?;
            let script = scene.load()?;
            let cfg = script.config.clone();
            let frames: Vec<usize> = at.split(',').map(|t| cfg.frame_of(t).map(|f| (f.max(0.0) as usize).min(cfg.duration - 1))).collect::<anyhow::Result<_>>()?;
            let out = out.unwrap_or_else(|| PathBuf::from("stills"));
            for p in render::stills(&script, &frames, scale, blur(&cfg, no_motion_blur), &out)? {
                println!("{}", p.display());
            }
            Ok(())
        }
        Cmd::Sheet { s, every, count, from, to, cols, width, out, no_motion_blur } => {
            let scene = s.scene()?;
            let script = scene.load()?;
            let cfg = script.config.clone();
            let a = from.map(|t| cfg.frame_of(&t)).transpose()?.unwrap_or(0.0).max(0.0) as usize;
            let b = to.map(|t| cfg.frame_of(&t)).transpose()?.map(|v| v as usize).unwrap_or(cfg.duration).min(cfg.duration);
            let frames: Vec<usize> = match (every, count) {
                (Some(e), _) => {
                    let step = cfg.frame_of(&e)?.max(1.0) as usize;
                    (a..b).step_by(step).collect()
                }
                (None, n) => {
                    let n = n.unwrap_or(24).max(1);
                    (0..n).map(|i| a + ((b - a - 1) as f64 * i as f64 / (n.max(2) - 1) as f64).round() as usize).collect()
                }
            };
            let out = out.unwrap_or_else(|| PathBuf::from("sheet.png"));
            render::sheet(&script, &frames, cols, width, blur(&cfg, no_motion_blur), &out)?;
            println!("{} ({} frames)", out.display(), frames.len());
            Ok(())
        }
        Cmd::Preview { s, scale, at, paused } => {
            let scene = s.scene()?;
            let script = scene.load()?;
            let start = at.map(|t| script.config.frame_of(&t)).transpose()?.unwrap_or(0.0) as usize;
            preview::run(scene, script, scale, start, !paused)
        }
        Cmd::Check { s, every, from, to } => {
            let scene = s.scene()?;
            if !check::check(&scene, every, from.as_deref(), to.as_deref(), workers(None))? {
                std::process::exit(1);
            }
            Ok(())
        }
        Cmd::Init { dir } => docs::init(&dir.unwrap_or_else(|| PathBuf::from("."))),
        Cmd::Docs { topic } => docs::print(topic.as_deref().unwrap_or("guide")),
    }
}
