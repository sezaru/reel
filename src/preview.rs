//! A window that plays the video in real time, for judging timing and look without
//! rendering a file. Frames are recorded and drawn as they're shown (a late frame is
//! skipped, not slowed); the timeline bar along the bottom shows where you are. When a file
//! the scene loaded changes, it loads again (an error keeps the last good version).
//!
//!   Space        play / pause
//!   ← →          one frame (with Shift: one second)
//!   ↑ ↓          the next / previous marker (without markers: five seconds)
//!   Home End     start, end
//!   R            load the scene again
//!   click / drag on the window: seek to that point of the timeline
//!   Esc, Q       quit
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::canvas::TexKey;
use crate::gpu::{Decoded, Gpu};
use crate::render::{Scene, plan};
use crate::script::Script;

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
}

struct App {
    scene: Scene,
    script: Script,
    /// the files the scene loaded, and when each last changed
    watched: Vec<(PathBuf, Option<SystemTime>)>,
    last_poll: Instant,
    /// the last draw failed with this (printed once)
    failed: Option<String>,
    scale: f64,
    frames: (usize, usize),
    f: f64,
    playing: bool,
    /// when play started, and from which frame
    anchor: (Instant, f64),
    state: Option<State>,
    shift: bool,
    dragging: bool,
    cursor: (f64, f64),
    shown: u64,
    fps_since: (Instant, u64),
    fps: f64,
    /// a paused frame needs drawing again (seek, resize)
    dirty: bool,
}

fn mtime(p: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn watch_list(script: &Script) -> Vec<(PathBuf, Option<SystemTime>)> {
    let mut files: Vec<PathBuf> = script.files.lock().unwrap().clone();
    files.extend(crate::shader::watched());
    files.sort();
    files.dedup();
    files.into_iter().map(|p| {
        let t = mtime(&p);
        (p, t)
    }).collect()
}

impl App {
    /// Loads the scene again (keeps the old one if the new one fails).
    fn reload(&mut self) {
        match self.scene.load() {
            Ok(s) => {
                self.frames = (0, s.config.duration);
                self.script = s;
                self.failed = None;
                eprintln!("preview: reloaded");
            }
            Err(e) => eprintln!("preview: the scene failed to load, showing the last good one:\n{e:#}"),
        }
        self.watched = watch_list(&self.script);
        self.dirty = true;
    }

    fn poll_files(&mut self) {
        if self.last_poll.elapsed().as_millis() < 300 {
            return;
        }
        self.last_poll = Instant::now();
        let changed = self.watched.iter().any(|(p, t)| mtime(p) != *t);
        if changed {
            self.reload();
        }
    }

    fn jump(&mut self, dir: f64) {
        let fps = self.script.config.fps;
        let marks: Vec<f64> = self.script.config.markers.iter().map(|m| m.1).collect();
        let f = self.f;
        let target = if marks.is_empty() {
            f + dir * 5.0 * fps
        } else if dir > 0.0 {
            marks.iter().copied().find(|m| *m > f + 0.5).unwrap_or(self.frames.1 as f64)
        } else {
            marks.iter().rev().copied().find(|m| *m < f - 0.5).unwrap_or(0.0)
        };
        self.seek(target);
    }

    fn seek(&mut self, f: f64) {
        self.f = f.clamp(self.frames.0 as f64, (self.frames.1 - 1) as f64).round();
        self.anchor = (Instant::now(), self.f);
        self.dirty = true;
    }

    fn seek_to_cursor(&mut self) {
        if let Some(s) = &self.state {
            let w = s.config.width as f64;
            let t = (self.cursor.0 / w).clamp(0.0, 1.0);
            let (a, b) = self.frames;
            self.seek(a as f64 + t * (b - a) as f64);
        }
    }

    fn draw(&mut self) {
        self.dirty = false;
        if self.state.is_none() {
            return;
        }
        if self.playing {
            self.f = self.anchor.1 + self.anchor.0.elapsed().as_secs_f64() * self.script.config.fps;
            if self.f >= self.frames.1 as f64 {
                self.f = self.frames.0 as f64;
                self.anchor = (Instant::now(), self.f);
            }
        }
        let f = self.f.floor() as usize;
        let blur = self.script.config.motion_blur;
        let samples = match plan(&self.script, f, self.scale, blur) {
            Ok(s) => {
                self.failed = None;
                s
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if self.failed.as_deref() != Some(msg.as_str()) {
                    eprintln!("preview: {msg}");
                    self.failed = Some(msg);
                }
                vec![]
            }
        };
        let Some(s) = &mut self.state else { return };
        let frame = match s.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => {
                s.surface.configure(s.gpu.device(), &s.config);
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let mut get = |k: &TexKey| Arc::new(crate::image::decode(k)) as Arc<Decoded>;
        let (a, b) = self.frames;
        let progress = ((f - a) as f32 / (b - a).max(1) as f32).clamp(0.0, 1.0);
        s.gpu.present(&samples, &mut get, &view, s.config.format, (s.config.width, s.config.height), progress);
        s.window.pre_present_notify();
        s.gpu.queue().present(frame);
        self.shown += 1;
        let el = self.fps_since.0.elapsed().as_secs_f64();
        if el > 0.5 {
            self.fps = (self.shown - self.fps_since.1) as f64 / el;
            self.fps_since = (Instant::now(), self.shown);
        }
        let secs = f as f64 / self.script.config.fps;
        let name = self.scene.entry.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        s.window.set_title(&format!(
            "{name} — frame {f} / {} — {}:{:05.2} — {:.0} fps{}{}",
            b,
            (secs / 60.0).floor(),
            secs % 60.0,
            self.fps,
            if self.playing { "" } else { " — paused" },
            if self.failed.is_some() { " — ERROR (see the terminal)" } else { "" }
        ));
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let (dw, dh) = (self.script.config.width as f64, self.script.config.height as f64);
        let k = (1280.0 / dw).min(720.0 / dh);
        let attrs = Window::default_attributes().with_title("reel").with_inner_size(winit::dpi::LogicalSize::new(dw * k, dh * k + 8.0));
        let window = Arc::new(el.create_window(attrs).expect("window"));
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_with_display_handle(Box::new(el.owned_display_handle()))
        });
        let surface = instance.create_surface(window.clone()).expect("surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("adapter");
        let cfg = &self.script.config;
        let gpu = Gpu::from_adapter(&adapter, (cfg.width, cfg.height), self.scale, 1 << 30, cfg.background).expect("gpu");
        let caps = surface.get_capabilities(&adapter);
        // the frame's values are display values already: a non-sRGB surface shows them as is
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            color_space: Default::default(),
        };
        surface.configure(gpu.device(), &config);
        eprintln!("preview: {}; frame {}×{}; Space plays, ←/→ step, ↑/↓ markers, R reloads, click the window to seek", gpu.adapter, gpu.w, gpu.h);
        self.anchor = (Instant::now(), self.f);
        self.state = Some(State { window, surface, config, gpu });
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => {
                if let Some(s) = &mut self.state {
                    s.config.width = size.width.max(1);
                    s.config.height = size.height.max(1);
                    s.surface.configure(s.gpu.device(), &s.config);
                }
                self.dirty = true;
            }
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::ModifiersChanged(m) => self.shift = m.state().shift_key(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                if self.dragging {
                    self.seek_to_cursor();
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                self.dragging = state == ElementState::Pressed;
                if self.dragging {
                    self.seek_to_cursor();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let step = if self.shift { self.script.config.fps.round() } else { 1.0 };
                match event.logical_key {
                    Key::Named(NamedKey::Space) => {
                        self.playing = !self.playing;
                        let f = self.f;
                        self.seek(f);
                    }
                    Key::Named(NamedKey::ArrowRight) => self.seek(self.f + step),
                    Key::Named(NamedKey::ArrowLeft) => self.seek(self.f - step),
                    Key::Named(NamedKey::ArrowUp) => self.jump(1.0),
                    Key::Named(NamedKey::ArrowDown) => self.jump(-1.0),
                    Key::Named(NamedKey::Home) => self.seek(0.0),
                    Key::Named(NamedKey::End) => self.seek(self.frames.1 as f64),
                    Key::Named(NamedKey::Escape) => el.exit(),
                    Key::Character(c) if c.as_str() == "q" => el.exit(),
                    Key::Character(c) if c.as_str() == "r" => self.reload(),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.poll_files();
        el.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(Instant::now() + std::time::Duration::from_millis(300)));
        if let Some(s) = &self.state
            && (self.playing || self.dirty)
        {
            s.window.request_redraw();
        }
    }
}

pub fn run(scene: Scene, script: Script, scale: f64, start: usize, play: bool) -> anyhow::Result<()> {
    let event_loop = EventLoop::new()?;
    let frames = (0, script.config.duration);
    let watched = watch_list(&script);
    let mut app = App {
        scene,
        script,
        watched,
        last_poll: Instant::now(),
        failed: None,
        scale,
        frames,
        f: start as f64,
        playing: play,
        anchor: (Instant::now(), start as f64),
        state: None,
        shift: false,
        dragging: false,
        cursor: (0.0, 0.0),
        shown: 0,
        fps_since: (Instant::now(), 0),
        fps: 0.0,
        dirty: true,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
