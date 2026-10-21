//! `reel check`: runs the scene on every frame (or every nth) on all cores without drawing
//! anything, and reports what would go wrong in a render: exceptions (grouped, with the
//! frames they happen on), shaders that don't compile, missing audio, and a summary.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::canvas::{Op, TexKey};
use crate::render::{Scene, tex_keys};

#[derive(Default)]
struct Found {
    /// message → frames it happened on
    errors: BTreeMap<String, Vec<usize>>,
    max_ops: (usize, usize),
    images: HashSet<std::sync::Arc<str>>,
    shaders: HashMap<u64, Arc<crate::shader::Code>>,
    frames: usize,
}

/// Frame lists as ranges: "3, 10–20, 31".
fn ranges(v: &[usize], step: usize) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < v.len() && out.len() < 8 {
        let mut j = i;
        while j + 1 < v.len() && v[j + 1] == v[j] + step {
            j += 1;
        }
        out.push(if j > i { format!("{}–{}", v[i], v[j]) } else { v[i].to_string() });
        i = j + 1;
    }
    if i < v.len() {
        out.push("…".into());
    }
    out.join(", ")
}

/// Returns whether the scene passed.
pub fn check(scene: &Scene, every: usize, from: Option<&str>, to: Option<&str>, workers: usize) -> anyhow::Result<bool> {
    let script = match scene.load() {
        Ok(s) => s,
        Err(e) => {
            println!("error: the scene does not load:\n{e:#}");
            return Ok(false);
        }
    };
    let cfg = script.config.clone();
    drop(script);
    let every = every.max(1);
    let a = from.map(|t| cfg.frame_of(t)).transpose()?.unwrap_or(0.0).max(0.0) as usize;
    let b = to.map(|t| cfg.frame_of(t)).transpose()?.map(|v| v as usize).unwrap_or(cfg.duration).min(cfg.duration);
    let frames: Vec<usize> = (a..b).step_by(every).collect();
    let next = Arc::new(AtomicUsize::new(0));
    let found = Arc::new(Mutex::new(Found::default()));
    let threads: Vec<_> = (0..workers.max(1))
        .map(|_| {
            let (next, found, scene, frames) = (next.clone(), found.clone(), scene.clone(), frames.clone());
            std::thread::spawn(move || {
                let script = match scene.load() {
                    Ok(s) => s,
                    Err(e) => {
                        found.lock().unwrap().errors.entry(format!("{e:#}")).or_default().push(0);
                        return;
                    }
                };
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(&f) = frames.get(i) else { return };
                    let r = script.draw(f as f64, 1.0);
                    let mut fd = found.lock().unwrap();
                    fd.frames += 1;
                    match r {
                        Ok(cv) => {
                            if cv.ops.len() > fd.max_ops.0 {
                                fd.max_ops = (cv.ops.len(), f);
                            }
                            let ops = vec![cv.ops];
                            for k in tex_keys(&ops) {
                                if let TexKey::Img(p, _) = k
                                    && !fd.images.contains(p)
                                {
                                    fd.images.insert(p.clone());
                                }
                            }
                            for op in &ops[0] {
                                if let Op::Shader(s) = op {
                                    fd.shaders.entry(s.code.id).or_insert_with(|| s.code.clone());
                                }
                            }
                        }
                        Err(e) => {
                            // (the frame is in the message's first line: group without it)
                            let msg = format!("{e:#}");
                            let msg = msg.split_once(": ").map(|x| x.1.to_string()).unwrap_or(msg);
                            fd.errors.entry(msg).or_default().push(f);
                        }
                    }
                }
            })
        })
        .collect();
    for t in threads {
        let _ = t.join();
    }
    let mut fd = Arc::try_unwrap(found).ok().unwrap().into_inner().unwrap();
    let mut ok = true;
    for (msg, mut fs) in std::mem::take(&mut fd.errors) {
        ok = false;
        fs.sort();
        println!("error on {} frame{} ({}):\n{}\n", fs.len(), if fs.len() == 1 { "" } else { "s" }, ranges(&fs, every), msg.trim_end());
    }
    for code in fd.shaders.values() {
        if let Err(e) = crate::shader::validate(code) {
            ok = false;
            println!("error: shader {} does not compile:\n{}", code.name, e.trim_end());
        }
    }
    for t in &cfg.audio {
        if !t.src.is_file() {
            ok = false;
            println!("error: audio file missing: {}", t.src.display());
        }
    }
    println!(
        "{}: {} frames checked of {} ({:.1} s at {} fps), {}×{}; up to {} shapes a frame (frame {}); {} images; {} shaders; {} audio tracks",
        if ok { "ok" } else { "FAILED" },
        fd.frames,
        cfg.duration,
        cfg.duration as f64 / cfg.fps,
        cfg.fps,
        cfg.width,
        cfg.height,
        fd.max_ops.0,
        fd.max_ops.1,
        fd.images.len(),
        fd.shaders.len(),
        cfg.audio.len()
    );
    Ok(ok)
}
