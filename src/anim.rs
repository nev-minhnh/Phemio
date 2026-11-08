//! Critically-damped-ish springs. Animations only tick while something moves;
//! once every spring settles the timer is killed, so idle CPU is zero.

#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub v: f32,
    pub vel: f32,
    pub target: f32,
    k: f32,
    d: f32,
    eps: f32,
}

impl Spring {
    pub fn new(v: f32, stiffness: f32, damping: f32, eps: f32) -> Self {
        Spring { v, vel: 0.0, target: v, k: stiffness, d: damping, eps }
    }
    /// Size-like values (DIPs).
    pub fn size(v: f32) -> Self {
        Spring::new(v, 340.0, 30.0, 0.05)
    }
    /// 0..1 values (opacity, progress).
    pub fn unit(v: f32) -> Self {
        Spring::new(v, 260.0, 30.0, 0.002)
    }
    pub fn set(&mut self, t: f32) {
        // (A bad number must never get stuck in an animation.)
        if t.is_finite() {
            self.target = t;
        }
    }
    pub fn snap(&mut self, v: f32) {
        self.v = v;
        self.target = v;
        self.vel = 0.0;
    }
    pub fn settled(&self) -> bool {
        (self.v - self.target).abs() <= self.eps && self.vel.abs() <= self.eps * 10.0
    }
    /// Advance by `dt` seconds. Returns true while still moving.
    pub fn step(&mut self, dt: f32) -> bool {
        if !self.v.is_finite() || !self.vel.is_finite() || !self.target.is_finite() {
            let t = if self.target.is_finite() { self.target } else { 0.0 };
            self.snap(t);
            return false;
        }
        if self.settled() {
            self.v = self.target;
            self.vel = 0.0;
            return false;
        }
        let mut t = dt.min(0.05);
        while t > 0.0 {
            let h = t.min(0.004);
            let a = self.k * (self.target - self.v) - self.d * self.vel;
            self.vel += a * h;
            self.v += self.vel * h;
            t -= h;
        }
        true
    }
}

pub fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}
