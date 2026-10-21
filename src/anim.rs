//! Remotion's spring(), ported so scenes move on exactly the frames a Remotion composition
//! would: the same stepped integrator (each frame advances the last), cached per thread.
//! (interpolate() and Easing are in reel.ts.)
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

// ── spring ───────────────────────────────────────────────────────────────────────────────────
#[derive(Clone, Copy)]
struct State {
    cur: f64,
    vel: f64,
    last: f64,
}

fn advance(s: State, now: f64, k: f64, c: f64, m: f64) -> State {
    let dt = (now - s.last).min(64.0);
    let v0 = -s.vel;
    let x0 = 1.0 - s.cur;
    let zeta = c / (2.0 * (k * m).sqrt());
    let w0 = (k / m).sqrt();
    let t = dt / 1000.0;
    let (pos, v) = if zeta < 1.0 {
        let w1 = w0 * (1.0 - zeta * zeta).sqrt();
        let (sn, co) = ((w1 * t).sin(), (w1 * t).cos());
        let env = (-zeta * w0 * t).exp();
        let frag = env * (sn * ((v0 + zeta * w0 * x0) / w1) + x0 * co);
        (1.0 - frag, zeta * w0 * frag - env * (co * (v0 + zeta * w0 * x0) - w1 * x0 * sn))
    } else {
        let env = (-w0 * t).exp();
        (1.0 - env * (x0 + (v0 + w0 * x0) * t), env * (v0 * (t * w0 - 1.0) + t * x0 * w0 * w0))
    };
    State { cur: pos, vel: v, last: now }
}

struct Table {
    fps: f64,
    k: f64,
    c: f64,
    m: f64,
    states: Vec<State>,
}

impl Table {
    fn new(damping: f64, mass: f64, stiffness: f64, fps: f64) -> Self {
        let first = advance(State { cur: 0.0, vel: 0.0, last: 0.0 }, 0.0, stiffness, damping, mass);
        Table { fps, k: stiffness, c: damping, m: mass, states: vec![first] }
    }

    fn at(&mut self, frame: f64) -> f64 {
        // (settled long before: a far frame costs no more than this)
        let frame = frame.clamp(0.0, 20000.0);
        let whole = frame.floor() as usize;
        while self.states.len() <= whole {
            let f = self.states.len() as f64;
            let s = *self.states.last().unwrap();
            self.states.push(advance(s, f / self.fps * 1000.0, self.k, self.c, self.m));
        }
        let mut s = self.states[whole];
        if frame - whole as f64 > 0.0 {
            s = advance(s, frame / self.fps * 1000.0, self.k, self.c, self.m);
        }
        s.cur
    }
}

type Key = (u64, u64, u64, u64);

thread_local! {
    static TABLES: RefCell<HashMap<Key, Rc<RefCell<Table>>>> = RefCell::new(HashMap::new());
    static NATURAL: RefCell<HashMap<Key, f64>> = RefCell::new(HashMap::new());
}

fn table(damping: f64, mass: f64, stiffness: f64, fps: f64) -> Rc<RefCell<Table>> {
    let key = (damping.to_bits(), mass.to_bits(), stiffness.to_bits(), fps.to_bits());
    TABLES.with(|t| {
        t.borrow_mut().entry(key).or_insert_with(|| Rc::new(RefCell::new(Table::new(damping, mass, stiffness, fps)))).clone()
    })
}

/// measureSpring: the frame the spring settles (threshold 0.005, then 20 calm frames).
pub fn natural(damping: f64, mass: f64, stiffness: f64, fps: f64) -> f64 {
    let key = (damping.to_bits(), mass.to_bits(), stiffness.to_bits(), fps.to_bits());
    if let Some(v) = NATURAL.with(|n| n.borrow().get(&key).copied()) {
        return v;
    }
    let t = table(damping, mass, stiffness, fps);
    let mut t = t.borrow_mut();
    let threshold = 0.005;
    let mut frame = 0.0;
    let mut diff = (t.at(frame) - 1.0).abs();
    while diff >= threshold {
        frame += 1.0;
        diff = (t.at(frame) - 1.0).abs();
    }
    let mut finished = frame;
    let mut i = 0;
    while i < 20 {
        frame += 1.0;
        diff = (t.at(frame) - 1.0).abs();
        if diff >= threshold {
            i = 0;
            finished = frame + 1.0;
        }
        i += 1;
    }
    NATURAL.with(|n| n.borrow_mut().insert(key, finished));
    finished
}


/// spring() at `frame` (fractional frames step from the whole one before), 0 → 1.
pub fn spring_at(frame: f64, damping: f64, mass: f64, stiffness: f64, fps: f64) -> f64 {
    if !(frame.is_finite() && damping > 0.0 && mass > 0.0 && stiffness > 0.0 && fps > 0.0) {
        return if frame.is_finite() { 0.0 } else { f64::NAN };
    }
    table(damping, mass, stiffness, fps).borrow_mut().at(frame)
}
