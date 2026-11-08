//! The mascot's extras: the cat's mouth and tapping paws (it drums along
//! when you type or music plays), its magic-trick goodbye, and the
//! Cat vs Robot showdown (Settings → Island → Mascot look).

use super::*;

/// Repaints the calm parts of the showdown a few times a second (the busy
/// parts run at full frame rate).
pub(super) const T_DUEL: usize = 22;

const PI: f32 = std::f32::consts::PI;
const TAU: f32 = std::f32::consts::TAU;

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Overshoots a little before settling (things that land / pop in).
fn ease_back(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let c = 1.9;
    1.0 + (c + 1.0) * (t - 1.0).powi(3) + c * (t - 1.0).powi(2)
}

/// 0 → 1 over [a, b].
fn span(u: f32, a: f32, b: f32) -> f32 {
    ((u - a) / (b - a)).clamp(0.0, 1.0)
}

fn hash(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^ (x >> 33)
}

/// How the cat looks (all in the face's own units, ~±30 wide).
#[derive(Clone, Copy)]
pub(crate) struct CatLook {
    /// 0 upright lines (`eye_h` tall), 1 happy "^", 2 asleep, 3 "x x",
    /// 4 "> <", 5 cross (slanted brows), 6 wide round.
    pub eyes: u8,
    pub eye_h: f32,
    pub look: f32,
    pub look_y: f32,
    /// See `cat_mouth`.
    pub mouth: u8,
    pub open: f32,
    /// Ears: positive droops / flattens them.
    pub ear: f32,
    pub tilt: f32,
    /// Whisker length (1 normal).
    pub whisk: f32,
    pub blush: f32,
}

impl Default for CatLook {
    fn default() -> Self {
        CatLook { eyes: 0, eye_h: 6.5, look: 0.0, look_y: 0.0, mouth: 0, open: 0.0, ear: 0.0, tilt: 0.0, whisk: 1.0, blush: 0.4 }
    }
}

/// How the robot looks.
#[derive(Clone, Copy)]
pub(crate) struct BotLook {
    /// 0 glowing pills (`eye_h` tall), 1 happy "^", 2 angry, 3 "x x",
    /// 4 hearts, 5 asleep, 6 round, 7 stars.
    pub eyes: u8,
    pub eye_h: f32,
    pub look: f32,
    /// 0 smile, 1 grin, 2 "o", 3 flat, 4 gritted teeth, 5 wavy.
    pub mouth: u8,
    pub tilt: f32,
    /// Antenna light: 0 off … 1 bright; `hue` picks red (0) or its glow (1).
    pub light: f32,
    pub red: bool,
}

impl Default for BotLook {
    fn default() -> Self {
        BotLook { eyes: 0, eye_h: 7.0, look: 0.0, mouth: 0, tilt: 0.0, light: 0.6, red: false }
    }
}

impl Island {
    // ------------------------------------------------------------------
    // Shared bits
    // ------------------------------------------------------------------

    /// The cat's nose and mouth. `mouth`: 0 "w", 1 open smile, 2 "o",
    /// 3 open (meow / singing, `open` 0…1), 4 flat, 5 hissing with fangs,
    /// 6 wobbly, 7 "w" with the tongue out (blep).
    pub(super) fn cat_mouth(&self, p: &dyn Fn(f32, f32) -> (f32, f32), k: f32, mouth: u8, open: f32, a: f32) {
        let line_c = self.fg(0.95 * a);
        let pink = self.ic(pal::RED, 1.0);
        let dark = self.fg(0.16 * a);
        // Nose: a tiny pink triangle.
        let nose = [p(-1.7, 2.4), p(1.7, 2.4), p(0.0, 4.0)];
        self.surf.fill_poly(&nose, with_alpha(pink, 0.95 * a));
        let w_pts = |dy: f32, depth: f32| -> Vec<(f32, f32)> {
            let mut v = Vec::with_capacity(20);
            for side in [-1.0f32, 1.0] {
                for i in 0..=8 {
                    let u = i as f32 / 8.0;
                    let x = side * (3.6 - 3.6 * u);
                    let y = 5.0 + dy + (PI * u).sin() * depth;
                    v.push(p(x, y));
                }
            }
            v
        };
        match mouth {
            1 => {
                // An open smile with a little tongue.
                let mut pts = vec![p(-4.0, 5.0)];
                for i in 0..=10 {
                    let u = i as f32 / 10.0;
                    pts.push(p(-4.0 + 8.0 * u, 5.0 + (PI * u).sin() * 3.6));
                }
                self.surf.fill_poly(&pts, dark);
                self.surf.stroke_poly(&pts, true, line_c, 1.6);
                let (tx, ty) = p(0.0, 7.2);
                self.surf.fill_circle(tx, ty, 1.5 * k, with_alpha(pink, 0.9 * a));
            }
            2 => {
                let (ox, oy) = p(0.0, 6.4);
                let ring: Vec<_> = (0..=16).map(|i| { let u = i as f32 / 16.0 * TAU; (ox + 1.8 * k * u.cos(), oy + 2.0 * k * u.sin()) }).collect();
                self.surf.stroke_poly(&ring, true, line_c, 1.6);
            }
            3 => {
                let h = 1.4 + 3.6 * open.clamp(0.0, 1.0);
                let w = 2.4 + 0.8 * open.clamp(0.0, 1.0);
                let (mx, my) = p(0.0, 5.0 + h / 2.0);
                let pts: Vec<_> = (0..=20).map(|i| { let u = i as f32 / 20.0 * TAU; (mx + u.cos() * w * k, my + u.sin() * h / 2.0 * k) }).collect();
                self.surf.fill_poly(&pts, dark);
                self.surf.stroke_poly(&pts, true, line_c, 1.6);
                if open > 0.35 {
                    self.surf.fill_circle(mx, my + h * 0.22 * k, w * 0.5 * k, with_alpha(pink, 0.85 * a));
                }
            }
            4 => {
                let (x0, y0) = p(-2.6, 6.0);
                let (x1, y1) = p(2.6, 6.0);
                self.surf.line(x0, y0, x1, y1, line_c, 1.6);
            }
            5 => {
                // Hiss: wide open, two little fangs.
                let pts = [p(-4.4, 4.6), p(4.4, 4.6), p(2.6, 9.2), p(-2.6, 9.2)];
                self.surf.fill_poly(&pts, dark);
                self.surf.stroke_poly(&pts, true, line_c, 1.6);
                for side in [-1.0f32, 1.0] {
                    let f = [p(side * 3.0, 4.8), p(side * 1.6, 4.8), p(side * 2.3, 6.8)];
                    self.surf.fill_poly(&f, rgb(0xffffff, a));
                }
            }
            6 => {
                let pts: Vec<_> = (0..=12).map(|i| { let u = i as f32 / 12.0; p(-3.6 + 7.2 * u, 6.0 + (u * TAU * 1.5).sin() * 0.9) }).collect();
                self.surf.stroke_poly(&pts, false, line_c, 1.5);
            }
            _ => {
                // The classic cat "w"; with the tongue out for a blep.
                if mouth == 7 {
                    let (tx, ty) = p(0.9, 7.6);
                    self.surf.fill_rr(rf(tx - 1.5 * k, ty - 2.0 * k, 3.0 * k, 4.0 * k), 1.5 * k, with_alpha(pink, 0.95 * a));
                }
                let v = w_pts(0.0, 1.6);
                self.surf.stroke_poly(&v[..9], false, line_c, 1.6);
                self.surf.stroke_poly(&v[9..], false, line_c, 1.6);
            }
        }
    }

    /// A cat paw (a mitten; toe beans show when it's raised toward you).
    fn paw(&self, x: f32, y: f32, k: f32, beans: f32, a: f32) {
        let line_c = self.fg(0.95 * a);
        let pink = self.ic(pal::RED, 1.0);
        let outline: Vec<_> = (0..=20).map(|i| { let t = i as f32 / 20.0 * TAU; (x + t.cos() * 4.4 * k, y + t.sin() * 3.8 * k) }).collect();
        self.surf.fill_poly(&outline, self.blend(0.0));
        self.surf.stroke_poly(&outline, true, line_c, 1.8);
        if beans > 0.05 {
            let c = with_alpha(pink, 0.9 * beans * a);
            for (bx, by) in [(-2.2f32, -1.6f32), (0.0, -2.4), (2.2, -1.6)] {
                self.surf.fill_circle(x + bx * k, y + by * k, 0.9 * k, c);
            }
            self.surf.fill_circle(x, y + 1.0 * k, 1.5 * k, c);
        }
    }

    /// A whole cat face (ears, eyes, cheeks, whiskers, mouth) at (cx, cy).
    pub(super) fn cat_face(&self, cx: f32, cy: f32, k: f32, l: &CatLook, a: f32) {
        let (sn, cs) = l.tilt.sin_cos();
        let p = |px: f32, py: f32| (cx + (px * cs - py * sn) * k, cy + (px * sn + py * cs) * k);
        let line_c = self.fg(0.95 * a);
        let eye_c = self.ic(pal::CYAN, a);
        let sw = 2.7;
        for side in [-1.0f32, 1.0] {
            let e = l.ear;
            let pts = [p(side * 26.0, -5.0), p(side * (22.5 + e.max(0.0) * 2.5), -14.0 + 3.5 * e), p(side * 14.0, -9.0 + e * 0.6)];
            self.surf.stroke_poly(&pts, false, line_c, sw);
        }
        let ex = |side: f32| side * 9.0 + l.look;
        let ey = 0.5 + l.look_y;
        match l.eyes {
            1 | 2 => {
                // Happy "^" arcs, or asleep (curving down).
                for side in [-1.0f32, 1.0] {
                    let up = l.eyes == 1;
                    let pts: Vec<_> = (0..=8).map(|i| { let t = i as f32 / 8.0; let b = (PI * t).sin() * 2.2; p(ex(side) - 3.5 + 7.0 * t, if up { ey + 1.5 - b } else { ey + b }) }).collect();
                    self.surf.stroke_poly(&pts, false, eye_c, sw);
                }
            }
            3 => {
                for side in [-1.0f32, 1.0] {
                    let (x0, y0) = p(ex(side) - 2.6, ey - 2.6);
                    let (x1, y1) = p(ex(side) + 2.6, ey + 2.6);
                    self.surf.line(x0, y0, x1, y1, eye_c, 2.2);
                    let (x0, y0) = p(ex(side) - 2.6, ey + 2.6);
                    let (x1, y1) = p(ex(side) + 2.6, ey - 2.6);
                    self.surf.line(x0, y0, x1, y1, eye_c, 2.2);
                }
            }
            4 => {
                for side in [-1.0f32, 1.0] {
                    let pts = [p(ex(side) - side * 3.0, ey - 2.4), p(ex(side) + side * 2.4, ey), p(ex(side) - side * 3.0, ey + 2.4)];
                    self.surf.stroke_poly(&pts, false, eye_c, sw - 0.4);
                }
            }
            5 => {
                // Cross: eyes plus slanted brows.
                for side in [-1.0f32, 1.0] {
                    let (x0, y0) = p(ex(side), ey - 2.2);
                    let (x1, y1) = p(ex(side), ey + 2.6);
                    self.surf.line(x0, y0, x1, y1, eye_c, sw + 0.3);
                    let (b0x, b0y) = p(ex(side) - side * 4.0, ey - 6.2);
                    let (b1x, b1y) = p(ex(side) + side * 2.0, ey - 3.6);
                    self.surf.line(b0x, b0y, b1x, b1y, line_c, 2.0);
                }
            }
            6 => {
                for side in [-1.0f32, 1.0] {
                    let (x0, y0) = p(ex(side), ey);
                    self.surf.fill_circle(x0, y0, 3.0 * k, eye_c);
                    self.surf.fill_circle(x0 + 0.9 * k, y0 - 1.0 * k, 0.9 * k, rgb(0xffffff, 0.9 * a));
                }
            }
            _ => {
                for side in [-1.0f32, 1.0] {
                    let hh = l.eye_h.max(1.4);
                    let (x0, y0) = p(ex(side), ey - hh / 2.0);
                    let (x1, y1) = p(ex(side), ey + hh / 2.0);
                    self.surf.line(x0, y0, x1, y1, eye_c, sw + 0.5);
                }
            }
        }
        for side in [-1.0f32, 1.0] {
            let (hx, hy) = p(side * 13.5, 5.0);
            self.surf.fill_circle(hx, hy, 2.4 * k, self.el(pal::RED, l.blush * a));
        }
        let whisk_c = self.fg(0.6 * a);
        for side in [-1.0f32, 1.0] {
            for (y0, y1) in [(2.0f32, 0.0f32), (5.0, 6.5)] {
                let (ax, ay) = p(side * 17.0, y0);
                let (mx, my) = p(side * (17.0 + 8.0 * l.whisk), (y0 + y1) / 2.0);
                let (bx, by) = p(side * (17.0 + 15.0 * l.whisk), y1);
                self.surf.stroke_poly(&[(ax, ay), (mx, my), (bx, by)], false, whisk_c, 2.0);
            }
        }
        self.cat_mouth(&p, k, l.mouth, l.open, a);
    }

    /// The robot for the showdown: a rounded head with an antenna, glowing eyes.
    pub(super) fn bot_face(&self, cx: f32, cy: f32, k: f32, l: &BotLook, a: f32) {
        let (sn, cs) = l.tilt.sin_cos();
        let p = |px: f32, py: f32| (cx + (px * cs - py * sn) * k, cy + (px * sn + py * cs) * k);
        let line_c = self.fg(0.95 * a);
        let glow = self.ic(pal::CYAN, a);
        let red = self.ic(pal::RED, 1.0);
        let eye_c = if l.eyes == 2 { with_alpha(red, a) } else { glow };
        // Head.
        let mut head = Vec::with_capacity(40);
        let (x0, y0, w, h, r) = (-16.0f32, -11.0f32, 32.0f32, 23.0f32, 7.0f32);
        for (ccx, ccy, a0) in [(x0 + w - r, y0 + r, -90.0f32), (x0 + w - r, y0 + h - r, 0.0), (x0 + r, y0 + h - r, 90.0), (x0 + r, y0 + r, 180.0)] {
            for i in 0..=6 {
                let ang = (a0 + i as f32 * 15.0).to_radians();
                head.push(p(ccx + r * ang.cos(), ccy + r * ang.sin()));
            }
        }
        self.surf.fill_poly(&head, self.blend(0.0));
        self.surf.stroke_poly(&head, true, line_c, 2.0);
        // Antenna and its light.
        let (a0x, a0y) = p(0.0, -11.0);
        let (a1x, a1y) = p(0.0, -16.0);
        self.surf.line(a0x, a0y, a1x, a1y, line_c, 1.8);
        let lc = if l.red { red } else { self.ic(pal::CYAN, 1.0) };
        self.surf.fill_circle(a1x, a1y - 1.4 * k, 4.0 * k, with_alpha(lc, 0.25 * l.light * a));
        self.surf.fill_circle(a1x, a1y - 1.4 * k, 2.1 * k, with_alpha(lc, (0.35 + 0.65 * l.light) * a));
        // Eyes.
        let ey = -2.5;
        for side in [-1.0f32, 1.0] {
            let ex = side * 6.5 + l.look;
            match l.eyes {
                1 => {
                    let pts = [p(ex - 2.8, ey + 1.4), p(ex, ey - 1.6), p(ex + 2.8, ey + 1.4)];
                    self.surf.stroke_poly(&pts, false, eye_c, 2.3);
                }
                2 => {
                    // Angry: a slanted slab of an eye.
                    let pts = [p(ex - 2.6, ey - 1.6 - side * 1.2), p(ex + 2.6, ey - 1.6 + side * 1.2), p(ex + 2.6, ey + 2.0), p(ex - 2.6, ey + 2.0)];
                    self.surf.fill_poly(&pts, eye_c);
                }
                3 => {
                    let (xa, ya) = p(ex - 2.3, ey - 2.3);
                    let (xb, yb) = p(ex + 2.3, ey + 2.3);
                    self.surf.line(xa, ya, xb, yb, eye_c, 2.0);
                    let (xa, ya) = p(ex - 2.3, ey + 2.3);
                    let (xb, yb) = p(ex + 2.3, ey - 2.3);
                    self.surf.line(xa, ya, xb, yb, eye_c, 2.0);
                }
                4 => {
                    let (hx, hy) = p(ex, ey);
                    let heart: Vec<(f32, f32)> = (0..28).map(|i| { let t = i as f32 / 28.0 * TAU; let xx = 16.0 * t.sin().powi(3); let yy = 13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos(); (hx + xx * 0.19 * k, hy - yy * 0.19 * k) }).collect();
                    self.surf.fill_poly(&heart, with_alpha(red, a));
                }
                5 => {
                    let pts: Vec<_> = (0..=8).map(|i| { let u = i as f32 / 8.0; p(ex - 2.8 + 5.6 * u, ey + 0.6 + (PI * u).sin() * 1.5) }).collect();
                    self.surf.stroke_poly(&pts, false, with_alpha(glow, 0.8 * glow.a), 2.2);
                }
                6 => {
                    let (x1, y1) = p(ex, ey);
                    self.surf.fill_circle(x1, y1, 3.0 * k, eye_c);
                }
                7 => {
                    let (sx, sy) = p(ex, ey);
                    self.star(sx, sy, 3.4 * k, 0.0, self.ic(pal::YELLOW, a));
                }
                _ => {
                    let eh = l.eye_h.max(1.2);
                    let (x1, y1) = p(ex, ey);
                    self.surf.fill_rr(rf(x1 - 2.1 * k, y1 - eh * k / 2.0, 4.2 * k, eh * k), 2.0 * k, eye_c);
                }
            }
        }
        for side in [-1.0f32, 1.0] {
            let (hx, hy) = p(side * 11.0, 3.0);
            self.surf.fill_circle(hx, hy, 2.0 * k, with_alpha(red, 0.5 * a));
        }
        // Mouth.
        let my = 5.4;
        match l.mouth {
            1 => {
                let mut pts = vec![p(-5.0, my - 0.4)];
                for i in 0..=10 {
                    let u = i as f32 / 10.0;
                    pts.push(p(-5.0 + 10.0 * u, my - 0.4 + (PI * u).sin() * 3.4));
                }
                self.surf.fill_poly(&pts, line_c);
            }
            2 => {
                let (ox, oy) = p(0.0, my + 0.6);
                let ring: Vec<_> = (0..=16).map(|i| { let u = i as f32 / 16.0 * TAU; (ox + 1.8 * k * u.cos(), oy + 2.0 * k * u.sin()) }).collect();
                self.surf.stroke_poly(&ring, true, line_c, 1.7);
            }
            3 => {
                let (x0, y0) = p(-3.6, my + 0.8);
                let (x1, y1) = p(3.6, my + 0.8);
                self.surf.line(x0, y0, x1, y1, line_c, 1.8);
            }
            4 => {
                // Gritted teeth: a little grille.
                let r0 = [p(-4.6, my - 0.6), p(4.6, my - 0.6), p(4.6, my + 2.6), p(-4.6, my + 2.6)];
                self.surf.stroke_poly(&r0, true, line_c, 1.4);
                for i in 1..4 {
                    let x = -4.6 + 2.3 * i as f32;
                    let (x0, y0) = p(x, my - 0.6);
                    let (x1, y1) = p(x, my + 2.6);
                    self.surf.line(x0, y0, x1, y1, line_c, 1.0);
                }
            }
            5 => {
                let pts: Vec<_> = (0..=12).map(|i| { let u = i as f32 / 12.0; p(-4.0 + 8.0 * u, my + 0.8 + (u * TAU * 1.5).sin() * 1.0) }).collect();
                self.surf.stroke_poly(&pts, false, line_c, 1.6);
            }
            _ => {
                let pts: Vec<_> = (0..=10).map(|i| { let u = i as f32 / 10.0; p(-4.0 + 8.0 * u, my + (PI * u).sin() * 1.9) }).collect();
                self.surf.stroke_poly(&pts, false, line_c, 1.8);
            }
        }
    }

    /// A five-point star.
    fn star(&self, cx: f32, cy: f32, r: f32, rot: f32, c: D2D1_COLOR_F) {
        let pts: Vec<_> = (0..10).map(|i| { let ang = rot - PI / 2.0 + i as f32 * PI / 5.0; let rr = if i % 2 == 0 { r } else { r * 0.45 }; (cx + rr * ang.cos(), cy + rr * ang.sin()) }).collect();
        self.surf.fill_poly(&pts, c);
    }

    /// A four-point sparkle.
    fn sparkle(&self, cx: f32, cy: f32, r: f32, c: D2D1_COLOR_F) {
        let pts: Vec<_> = (0..8).map(|i| { let ang = i as f32 * PI / 4.0; let rr = if i % 2 == 0 { r } else { r * 0.22 }; (cx + rr * ang.cos(), cy + rr * ang.sin()) }).collect();
        self.surf.fill_poly(&pts, c);
    }

    /// A little lightning bolt.
    fn bolt(&self, cx: f32, cy: f32, s: f32, c: D2D1_COLOR_F) {
        let pts = [(cx - 1.0 * s, cy - 4.0 * s), (cx + 2.2 * s, cy - 4.0 * s), (cx + 0.4 * s, cy - 0.6 * s), (cx + 2.6 * s, cy - 0.6 * s), (cx - 1.8 * s, cy + 4.6 * s), (cx - 0.2 * s, cy + 0.6 * s), (cx - 2.4 * s, cy + 0.6 * s)];
        self.surf.fill_poly(&pts, c);
    }

    // ------------------------------------------------------------------
    // The cat: paws that drum along, music notes
    // ------------------------------------------------------------------

    /// A key went down while the cat is out: a paw taps (nothing about which key).
    pub fn cat_tap(&mut self) {
        if self.mascot != Mascot::Awake && self.mascot != Mascot::Doze {
            return;
        }
        self.tap_side = -self.tap_side;
        self.tap_at = Instant::now();
        if std::env::var_os("ISLET_LOG_MASCOT").is_some() {
            util::log("mascot: tap");
        }
        self.kick();
    }

    /// Paws drumming on a little desk line, under the cat's face: on each key
    /// you press, or to the beat while music plays. Returns how much of the
    /// drumming is showing (0 none … 1).
    pub(super) fn cat_drum(&mut self, p: &dyn Fn(f32, f32) -> (f32, f32), k: f32, music: bool, a: f32) -> f32 {
        let mut since = self.tap_at.elapsed().as_secs_f32();
        // Developer aid (screenshots): ISLET_DEMO_TAPS fakes steady typing.
        if std::env::var_os("ISLET_DEMO_TAPS").is_some() {
            let tt = self.t0.elapsed().as_secs_f32() * 6.0;
            since = tt.fract() / 6.0;
            self.tap_side = if (tt.floor() as i64) % 2 == 0 { 1.0 } else { -1.0 };
        }
        let typing = (1.0 - span(since, 1.0, 1.4)).max(0.0);
        let vis = if music { 1.0f32.max(typing) } else { typing };
        if vis <= 0.001 {
            return 0.0;
        }
        self.keep_anim = true;
        let t = self.t0.elapsed().as_secs_f32();
        // Which paw is down, and how far (1 = slammed on the desk).
        let (side, down) = if typing > 0.0 {
            let d = if since < 0.05 { 1.0 } else { (1.0 - span(since, 0.05, 0.2)) * 0.7 + 0.3 * (1.0 - span(since, 0.2, 1.0)) };
            (self.tap_side, d)
        } else {
            let b = t * 2.0; // ~120 beats a minute
            (if (b.floor() as i64) % 2 == 0 { -1.0 } else { 1.0 }, (1.0 - b.fract()).powi(3))
        };
        // The paws come up either side of the face (past the whiskers) and
        // drum on a desk line near the island's bottom edge; put away, they
        // sink below it.
        let sink = (1.0 - vis) * 16.0;
        let desk = 11.5;
        for s in [-1.0f32, 1.0] {
            let (d0x, d0y) = p(s * 31.0, desk);
            let (d1x, d1y) = p(s * 47.0, desk);
            self.surf.line(d0x, d0y, d1x, d1y, self.fg(0.4 * vis * a), 1.4);
        }
        for s in [-1.0f32, 1.0] {
            let dn = if s == side { down } else { 0.0 };
            let y = -1.0 + 8.6 * dn + sink;
            let (x, yy) = p(s * 39.0, y);
            self.paw(x, yy, k, 1.0 - dn, a);
            // A tap: little lines burst out where it lands.
            if dn > 0.85 {
                let c = self.fg(0.55 * a);
                for dx in [-6.5f32, 6.5] {
                    let (bx, by) = p(s * 39.0 + dx, desk - 1.0);
                    let (ex, ey) = p(s * 39.0 + dx * 1.45, desk - 4.0);
                    self.surf.line(bx, by, ex, ey, c, 1.2);
                }
            }
        }
        vis
    }

    /// While music plays: little notes float up beside the cat.
    pub(super) fn cat_notes(&mut self, cx: f32, cy: f32, k: f32, a: f32) {
        let t = self.t0.elapsed().as_secs_f32();
        for i in 0..2 {
            let ph = (t * 0.55 + i as f32 * 0.5).fract();
            let side = if i == 0 { 1.0 } else { -1.0 };
            let x = cx + side * (34.0 + ph * 8.0) * k;
            let y = cy + (2.0 - ph * 16.0) * k;
            let al = (1.0 - ph) * span(ph, 0.0, 0.15) * a;
            self.surf.text(if i == 0 { "\u{266A}" } else { "\u{266B}" }, &self.f.tiny_b, rf(x - 6.0, y - 7.0, 14.0, 14.0), self.ic(pal::PINK, al));
        }
        self.keep_anim = true;
    }

    // ------------------------------------------------------------------
    // The cat's goodbye: a magic trick
    // ------------------------------------------------------------------

    /// Ta-da! A magician's hat drops onto the desk beside the cat, upside
    /// down. A wand taps it twice in a shower of sparkles — the cat hops in
    /// with a poof. The hat wobbles… a little paw pops out and waves, then the
    /// hat flips over, hops away and twinkles out.
    pub(super) fn cat_magic_bye(&mut self, area: D2D_RECT_F, cx: f32, cy: f32, k: f32, u: f32, a: f32) {
        if u < CAT_BYE_S {
            self.keep_anim = true;
        }
        let line_c = self.fg(0.95 * a);
        let gold = self.ic(pal::YELLOW, 1.0);
        let pink = self.ic(pal::RED, 1.0);
        // Leave room on the right for the hat to hop away: a wandering cat
        // first scoots back toward the middle.
        let lo = area.left + 50.0 * k;
        let want = cx.clamp(lo, (area.right - 108.0 * k).max(lo));
        let cx = cx + (want - cx) * ease(span(u, 0.0, 0.3));
        self.surf.clip(area);
        // ---- The hat: drops in at the right, upside down (opening up) ----
        let drop = ease_back(span(u, 0.0, 0.35));
        let flip = ease(span(u, 1.85, 2.05));
        let hop_t = span(u, 2.05, 2.6);
        let hop_x = hop_t * 60.0;
        let hop_y = -((hop_t * 2.0 * PI).sin().abs()) * 7.0;
        // Squash as the cat lands inside.
        let land = span(u, 1.12, 1.3);
        let squash = 1.0 - 0.18 * (land * PI).sin();
        let wobble = if u > 1.3 && u < 1.85 { ((u - 1.3) * 24.0).sin() * 0.16 * (1.0 - span(u, 1.3, 1.85)) } else { 0.0 };
        let rot = PI * (1.0 - flip) + wobble;
        let (hx0, hy0) = (40.0 + hop_x, 4.0 - 40.0 * (1.0 - drop) + hop_y);
        let shrink = 1.0 - 0.6 * span(u, 2.35, 2.6);
        let fade = 1.0 - span(u, 2.4, 2.6);
        let (hs, hc) = rot.sin_cos();
        let hk = k * shrink;
        // Hat-local: brim (the opening) at y = +6.5, crown top at −6.5.
        let hp = |px: f32, py: f32| {
            let py = py * squash;
            (cx + hx0 * k + (px * hc - py * hs) * hk, cy + hy0 * k + (px * hs + py * hc) * hk)
        };
        // Where the opening is (top while upside down).
        let (ox, oy) = hp(0.0, 6.5);
        // ---- The cat ----
        let jump = span(u, 0.86, 1.18);
        if jump < 1.0 {
            let mut l = CatLook { eyes: 1, mouth: 1, blush: 0.65, look: 2.5, ..Default::default() };
            if u > 0.42 && u < 0.86 {
                l.eyes = 6;
                l.mouth = 2;
            }
            if jump > 0.0 {
                l.eyes = 4;
                l.mouth = 3;
                l.open = 0.7;
                l.ear = -0.8;
            }
            // Hop in an arc into the hat, getting smaller.
            let j = ease(jump);
            let fx = cx + (ox - cx) * j;
            let fy = cy + (oy - cy) * j - (jump * PI).sin() * 10.0 * k;
            let fk = k * (1.0 - 0.75 * j);
            self.cat_face(fx, fy, fk, &l, a);
        }
        if fade > 0.0 {
            let fa = fade * a;
            // A paw pops out of the opening and waves.
            let wave = span(u, 1.35, 1.85);
            if wave > 0.0 && wave < 1.0 {
                let up = ease(span(wave, 0.0, 0.25)) * (1.0 - ease(span(wave, 0.8, 1.0)));
                let sway = (wave * 3.0 * TAU).sin() * 3.0;
                self.paw(ox + sway * k, oy - (2.0 + up * 7.0) * k, k * 0.8, 1.0, fa);
            }
            let fill = self.blend(0.0);
            let crown = [hp(-6.0, -6.5), hp(6.0, -6.5), hp(7.0, 6.0), hp(-7.0, 6.0)];
            let band = [hp(-6.8, 2.6), hp(6.8, 2.6), hp(6.95, 5.0), hp(-6.95, 5.0)];
            let brim = [hp(-11.0, 5.4), hp(11.0, 5.4), hp(11.0, 7.8), hp(-11.0, 7.8)];
            self.surf.fill_poly(&crown, fill);
            self.surf.stroke_poly(&crown, true, with_alpha(line_c, fade), 2.0);
            self.surf.fill_poly(&band, with_alpha(pink, 0.85 * fa));
            self.surf.fill_poly(&brim, fill);
            self.surf.stroke_poly(&brim, true, with_alpha(line_c, fade), 2.0);
            // A trail of sparkles as it hops off.
            if hop_t > 0.0 {
                for i in 0..4 {
                    let ht = (hop_t - i as f32 * 0.07).max(0.0);
                    let sx = cx + (40.0 + ht * 60.0) * k;
                    let sy = cy + (12.0 - (ht * 2.0 * PI).sin().abs() * 7.0) * k;
                    self.sparkle(sx - 8.0 * k, sy, (2.6 - i as f32 * 0.5) * k, with_alpha(gold, (1.0 - i as f32 * 0.22) * fa));
                }
            }
        }
        // ---- The wand, from the left: taps twice, sparkles fly ----
        let wand_in = span(u, 0.35, 0.47) * (1.0 - span(u, 0.86, 0.95));
        if wand_in > 0.0 {
            let tap = ((u - 0.5) * PI / 0.09).sin().abs() * span(u, 0.5, 0.52) * (1.0 - span(u, 0.8, 0.84));
            // The tip dips onto the hat's opening on each tap.
            let (tx, ty) = (ox - 4.0 * k, oy - (4.5 - 2.5 * tap) * k);
            let (bx, by) = (tx - 17.0 * k * wand_in, ty - 6.0 * k * wand_in);
            self.surf.line(bx, by, tx, ty, with_alpha(line_c, wand_in), 2.4);
            self.surf.line(tx - 4.0 * k, ty - 1.4 * k, tx, ty, rgb(0xffffff, wand_in * a), 2.8);
            self.sparkle(tx, ty, 3.2 * k * (0.7 + 0.5 * tap), with_alpha(gold, wand_in * a));
            for (i, t0) in [0.52f32, 0.7].iter().enumerate() {
                let st = span(u, *t0, *t0 + 0.3);
                if st > 0.0 && st < 1.0 {
                    for j in 0..5 {
                        let ang = j as f32 * TAU / 5.0 + i as f32 * 0.6;
                        let r = (5.0 + st * 13.0) * k;
                        self.sparkle(ox + ang.cos() * r, oy - 3.0 * k + ang.sin() * r * 0.6, 2.2 * k * (1.0 - st), with_alpha(gold, (1.0 - st) * a));
                    }
                }
            }
        }
        // ---- POOF as the cat drops in ----
        let poof = span(u, 1.1, 1.6);
        if poof > 0.0 && poof < 1.0 {
            let grow = ease(span(poof, 0.0, 0.35));
            let al = (1.0 - span(poof, 0.3, 1.0)) * a;
            for i in 0..7 {
                let ang = i as f32 * TAU / 7.0 + 0.3;
                let d = (3.0 + 9.0 * grow) * k;
                let r = (3.0 + 3.5 * grow + (i % 3) as f32 * 0.8) * k;
                self.surf.fill_circle(ox + ang.cos() * d, oy - 3.0 * k + ang.sin() * d * 0.6, r, self.fg(0.5 * al));
            }
            for i in 0..6 {
                let ang = i as f32 * TAU / 6.0;
                let d = (8.0 + 16.0 * poof) * k;
                self.sparkle(ox + ang.cos() * d, oy - 3.0 * k + ang.sin() * d * 0.5, 2.4 * k * (1.0 - poof), with_alpha(gold, al));
            }
        }
        self.surf.unclip();
    }

    // ------------------------------------------------------------------
    // Cat vs Robot
    // ------------------------------------------------------------------

    /// The showdown on the closed island: they stare each other down, taunt,
    /// and now and then crash into a cartoon brawl. On goodbye: the final
    /// round — VS!, the big brawl, K.O., and a random winner celebrates.
    pub(super) fn draw_duel(&mut self, x: f32, y: f32, w: f32, h: f32, a: f32) {
        let k = (h / 34.0).clamp(0.8, 1.4) * 0.84;
        let t = self.t0.elapsed().as_secs_f32();
        let cx0 = x + w / 2.0;
        let cy = y + h / 2.0 + 2.0;
        let home = (w / 2.0 - 44.0 * k).clamp(36.0, 72.0);
        let state = if self.mascot == Mascot::Off { self.masc_last } else { self.mascot };
        let area = rf(x, y, w, h);
        self.surf.clip(area);
        match state {
            Mascot::Bye => {
                let u = self.bye_at.elapsed().as_secs_f32();
                self.duel_final(cx0, cy, home, k, u, a);
            }
            Mascot::Doze => {
                // Truce: asleep, leaning on each other.
                let br = (t * 1.6).sin() * 0.6;
                let cl = CatLook { eyes: 2, mouth: 4, ear: 0.6, tilt: 0.22, blush: 0.3, whisk: 0.7, ..Default::default() };
                self.cat_face(cx0 - 27.0 * k, cy + br, k, &cl, a);
                let bl = BotLook { eyes: 5, mouth: 3, tilt: -0.2, light: 0.15, ..Default::default() };
                self.bot_face(cx0 + 24.0 * k, cy - br, k, &bl, a);
                for (i, sx) in [(0, -1.0f32), (1, 1.0)] {
                    let zt = (t * 0.5 + i as f32 * 0.5).fract();
                    let (zx, zy) = (cx0 + sx * (44.0 + zt * 6.0) * k, cy - (8.0 + zt * 10.0) * k);
                    self.surf.text("z", &self.f.tiny_b, rf(zx - 5.0, zy - 7.0, 12.0, 14.0), self.fg(0.7 * a * (1.0 - zt)));
                }
                self.duel_calm_timer();
            }
            Mascot::Hello => {
                let cl = CatLook { eyes: 6, mouth: 3, open: 0.5, ear: -0.6, ..Default::default() };
                self.cat_face(cx0 - home, cy, k, &cl, a);
                let bl = BotLook { eyes: 6, mouth: 2, light: 1.0, ..Default::default() };
                self.bot_face(cx0 + home, cy, k, &bl, a);
                self.keep_anim = true;
            }
            _ => self.duel_idle(cx0, cy, home, k, t, a),
        }
        self.surf.unclip();
    }

    /// Repaint the calm parts every ~80 ms instead of every frame.
    fn duel_calm_timer(&mut self) {
        unsafe {
            SetTimer(Some(self.hwnd), T_DUEL, 80, None);
        }
    }

    fn duel_idle(&mut self, cx0: f32, cy: f32, home: f32, k: f32, t: f32, a: f32) {
        const CYCLE: f32 = 8.0;
        let n = (t / CYCLE).floor() as u64;
        let tc = t - n as f32 * CYCLE;
        let h = hash(n ^ self.duel_seed);
        let cat_taunts = h % 2 == 0;
        let fight = (h >> 8) % 10 < 7;
        // Positions: home, a dash to the middle, the brawl, back home.
        let dash = if fight { ease(span(tc, 3.7, 4.2)) * (1.0 - ease(span(tc, 5.6, 6.2))) } else { 0.0 };
        let brawl = fight && tc >= 4.2 && tc < 5.6;
        let bob = (t * 3.1).sin() * 0.9;
        let bob2 = (t * 2.7 + 1.0).sin() * 0.9;
        let xc = cx0 - home + (home - 18.0 * k) * dash;
        let xr = cx0 + home - (home - 18.0 * k) * dash;
        let taunt = span(tc, 2.5, 2.65) * (1.0 - span(tc, 3.5, 3.6));
        let dizzy = fight && tc >= 5.6 && tc < 7.0;
        let mut cl = CatLook { look: 2.5, whisk: 0.75, ..Default::default() };
        let mut bl = BotLook { look: -2.0, light: 0.5 + 0.5 * (t * 2.0).sin().abs(), ..Default::default() };
        // A blink now and then.
        if (t * 0.9).fract() < 0.04 {
            cl.eye_h = 1.5;
        }
        if ((t + 0.4) * 0.8).fract() < 0.04 {
            bl.eye_h = 1.2;
        }
        // Ear twitch.
        if (t * 0.35).fract() < 0.06 {
            cl.ear = -0.8;
        }
        if taunt > 0.0 {
            if cat_taunts {
                cl.eyes = 5;
                cl.mouth = 5;
                cl.ear = 0.9 * taunt;
                bl.eyes = 6;
                bl.mouth = 2;
            } else {
                bl.eyes = 2;
                bl.mouth = 4;
                bl.red = true;
                bl.light = 1.0;
                cl.eyes = 6;
                cl.ear = 0.7;
            }
        } else if dash > 0.0 && !dizzy {
            cl.eyes = 5;
            cl.mouth = 5;
            cl.ear = 0.8;
            bl.eyes = 2;
            bl.mouth = 4;
            bl.red = true;
        }
        if dizzy {
            cl.eyes = 3;
            cl.mouth = 6;
            bl.eyes = 3;
            bl.mouth = 5;
        }
        let recoil = if taunt > 0.0 { 3.0 * taunt } else { 0.0 };
        if !brawl {
            self.cat_face(xc - if cat_taunts { 0.0 } else { recoil }, cy + bob, k, &cl, a);
            self.bot_face(xr + if cat_taunts { recoil } else { 0.0 }, cy + bob2, k, &bl, a);
        }
        // Taunt marks: "!" over the scared one, steam over an angry robot.
        if taunt > 0.0 {
            let (mx, sx) = if cat_taunts { (xr, 1.0) } else { (xc, -1.0) };
            self.surf.text("!", &self.f.tiny_b, rf(mx + sx * 18.0 * k - 4.0, cy - 20.0 * k, 10.0, 14.0), self.ic(pal::YELLOW, taunt * a));
            if !cat_taunts {
                for i in 0..3 {
                    let ph = (t * 1.8 + i as f32 / 3.0).fract();
                    self.surf.fill_circle(xr + (6.0 + i as f32 * 3.0 - 6.0) * k, cy - (16.0 + ph * 10.0) * k, (1.5 + ph * 2.0) * k, self.fg(0.35 * (1.0 - ph) * taunt * a));
                }
            }
        }
        if brawl {
            let bt = (tc - 4.2) / 1.4;
            self.brawl(cx0, cy, k, t, bt, 1.0, a);
        }
        if dizzy {
            // Stars circling both heads.
            for (hx, ph) in [(xc, 0.0f32), (xr, 1.3)] {
                for i in 0..3 {
                    let ang = t * 5.0 + ph + i as f32 * TAU / 3.0;
                    self.star(hx + ang.cos() * 10.0 * k, cy - 14.0 * k + ang.sin() * 3.0 * k, 2.4 * k, ang, self.ic(pal::YELLOW, a * (1.0 - span(tc, 6.6, 7.0))));
                }
            }
        }
        if taunt > 0.0 || dash > 0.0 || brawl || dizzy {
            self.keep_anim = true;
        } else {
            self.duel_calm_timer();
        }
        // The score so far, small in the middle while they stare.
        if !brawl && dash < 0.05 {
            let c = config::cur();
            let s = format!("{} : {}", c.i("duel_cat"), c.i("duel_bot"));
            if c.i("duel_cat") + c.i("duel_bot") > 0 {
                self.surf.text(&s, &self.f.cal_sub, rf(cx0 - 30.0, cy - 6.0, 60.0, 12.0), self.fg(0.35 * a));
            }
        }
    }

    /// The cartoon brawl: a churning dust cloud with stars, bolts, a paw and
    /// a robot arm popping out of it. `bt` 0…1 through it, `big` scales it.
    fn brawl(&self, cx: f32, cy: f32, k: f32, t: f32, bt: f32, big: f32, a: f32) {
        let r0 = 15.0 * k * big;
        let jx = (t * 41.0).sin() * 1.4;
        let jy = (t * 37.0).cos() * 1.0;
        let (cx, cy) = (cx + jx, cy + jy);
        let gone = span(bt, 0.92, 1.0);
        let al = a * (1.0 - gone);
        let fill = self.fg(0.82 * al);
        let edge = self.blend(0.0);
        for i in 0..9 {
            let ang = i as f32 * TAU / 9.0 + t * 3.0;
            let wob = (t * 9.0 + i as f32 * 1.7).sin();
            let d = r0 * (0.75 + 0.15 * wob);
            let r = r0 * (0.42 + 0.08 * (i % 3) as f32 + 0.06 * wob);
            self.surf.fill_circle(cx + ang.cos() * d, cy + ang.sin() * d * 0.5, r + 1.4, with_alpha(edge, al));
            self.surf.fill_circle(cx + ang.cos() * d, cy + ang.sin() * d * 0.5, r, fill);
        }
        self.surf.fill_circle(cx, cy, r0 * 0.85, fill);
        // Things flying out.
        for i in 0..7 {
            let ph = (t * 2.2 + i as f32 * 0.37).fract();
            let ang = i as f32 * 2.4 + (t * 0.7).floor() * 1.3;
            let d = r0 + ph * 16.0 * k;
            let (px, py) = (cx + ang.cos() * d, cy + ang.sin() * d * 0.55);
            let pa = (1.0 - ph) * al;
            match i % 4 {
                0 => self.star(px, py, 3.0 * k, t * 6.0, self.ic(pal::YELLOW, pa)),
                1 => self.bolt(px, py, 0.9 * k, self.ic(pal::CYAN, pa)),
                2 => self.paw(px, py, k * 0.7, 1.0, pa),
                _ => {
                    // A robot arm with a claw.
                    let (ex, ey) = (px + ang.cos() * 7.0 * k, py + ang.sin() * 4.0 * k);
                    self.surf.line(px, py, ex, ey, self.fg(0.9 * pa), 2.0);
                    self.surf.line(ex, ey, ex + 2.5 * k, ey - 2.5 * k, self.fg(0.9 * pa), 1.6);
                    self.surf.line(ex, ey, ex + 2.5 * k, ey + 2.5 * k, self.fg(0.9 * pa), 1.6);
                }
            }
        }
        // A "!@#" of cartoon swearing.
        let sym = ["#", "!", "@", "%", "*"][((t * 6.0) as usize) % 5];
        self.surf.text(sym, &self.f.tiny_b, rf(cx - 5.0 + (t * 13.0).sin() * 8.0, cy - r0 - 6.0, 12.0, 14.0), self.ic(pal::RED, al));
    }

    /// The final round (the goodbye). The winner was drawn when it started.
    fn duel_final(&mut self, cx0: f32, cy: f32, home: f32, k: f32, u: f32, a: f32) {
        if u < DUEL_BYE_S {
            self.keep_anim = true;
        }
        let t = self.t0.elapsed().as_secs_f32();
        let cat_wins = self.duel_winner == 0;
        let gold = self.ic(pal::YELLOW, 1.0);
        let fade = 1.0 - span(u, DUEL_BYE_S - 0.45, DUEL_BYE_S);
        let a = a * fade;
        // ---- 1. Face off: "VS" ----
        let dash = ease(span(u, 0.75, 1.0));
        let xc = cx0 - home + (home - 18.0 * k) * dash;
        let xr = cx0 + home - (home - 18.0 * k) * dash;
        if u < 1.0 {
            let shake = if u > 0.25 { (t * 50.0).sin() * 0.8 } else { 0.0 };
            let cl = CatLook { eyes: 5, mouth: 5, ear: 0.9, look: 2.5, whisk: 0.75, ..Default::default() };
            let bl = BotLook { eyes: 2, mouth: 4, look: -2.0, red: true, light: 1.0, ..Default::default() };
            self.cat_face(xc + shake, cy, k, &cl, a);
            self.bot_face(xr - shake, cy, k, &bl, a);
            let vs = ease_back(span(u, 0.1, 0.4)) * (1.0 - span(u, 0.65, 0.78));
            if vs > 0.0 {
                self.vs_mark(cx0, cy - 1.0, k * (0.6 + 0.6 * vs), a * vs.min(1.0));
                // A spark between them.
                for i in 0..4 {
                    let ang = i as f32 * PI / 2.0 + t * 4.0;
                    self.sparkle(cx0 + ang.cos() * 14.0 * k, cy + ang.sin() * 9.0 * k, 2.4 * k, with_alpha(gold, vs.min(1.0) * a));
                }
            }
        }
        // ---- 2. The big brawl ----
        if (1.0..2.5).contains(&u) {
            self.brawl(cx0, cy, k, t, span(u, 1.0, 2.5), 1.25, a);
        }
        // ---- 3. The cloud bursts: one standing, one flat out ----
        if u >= 2.4 {
            let burst = span(u, 2.4, 2.75);
            if burst < 1.0 {
                for i in 0..10 {
                    let ang = i as f32 * TAU / 10.0;
                    let d = (14.0 + 30.0 * ease(burst)) * k;
                    self.surf.fill_circle(cx0 + ang.cos() * d, cy + ang.sin() * d * 0.5, (6.0 - 4.0 * burst) * k, self.fg(0.6 * (1.0 - burst) * a));
                }
            }
            let party = span(u, 3.2, 4.6);
            let hop = if party > 0.0 { -((party * 3.0 * PI).sin().abs()) * 6.0 * k } else { 0.0 };
            let (wx, lx) = if cat_wins { (cx0 - 46.0 * k, cx0 + 42.0 * k) } else { (cx0 + 36.0 * k, cx0 - 52.0 * k) };
            // The loser: knocked over, dizzy.
            let ko_tilt = if cat_wins { 0.55 } else { -0.55 } * ease(span(u, 2.45, 2.7));
            let ly = cy + 2.0 * k;
            if cat_wins {
                let bl = BotLook { eyes: 3, mouth: 5, tilt: ko_tilt, light: if (t * 7.0).sin() > 0.6 { 0.8 } else { 0.0 }, ..Default::default() };
                self.bot_face(lx, ly, k * 0.95, &bl, a);
                // Smoke from its head.
                for i in 0..3 {
                    let ph = (t * 1.2 + i as f32 / 3.0).fract();
                    self.surf.fill_circle(lx + (ph * 6.0) * k, ly - (14.0 + ph * 10.0) * k, (1.5 + ph * 2.5) * k, self.fg(0.3 * (1.0 - ph) * a));
                }
            } else {
                let cl = CatLook { eyes: 3, mouth: 6, tilt: ko_tilt, ear: 0.9, whisk: 0.6, ..Default::default() };
                self.cat_face(lx, ly, k * 0.95, &cl, a);
            }
            for i in 0..3 {
                let ang = t * 5.0 + i as f32 * TAU / 3.0;
                self.star(lx + ang.cos() * 10.0 * k, ly - 14.0 * k + ang.sin() * 3.0 * k, 2.4 * k, ang, with_alpha(gold, a));
            }
            // The winner: happy, hopping, with a crown.
            if cat_wins {
                let cl = CatLook { eyes: 1, mouth: 1, blush: 0.7, ear: -0.5, whisk: 0.8, look: 0.0, ..Default::default() };
                self.cat_face(wx, cy + hop, k, &cl, a);
            } else {
                let rainbow = [pal::RED, pal::ORANGE, pal::YELLOW, pal::GREEN, pal::CYAN, pal::PURPLE];
                let _ = rainbow;
                let bl = BotLook { eyes: if party > 0.0 { 4 } else { 7 }, mouth: 1, light: 1.0, red: (t * 6.0).sin() > 0.0, ..Default::default() };
                self.bot_face(wx, cy + hop, k, &bl, a);
            }
            let crown_in = ease_back(span(u, 3.0, 3.35));
            if crown_in > 0.0 {
                let top = if cat_wins { -8.5 } else { -10.5 };
                self.crown(wx, cy + hop + (top - 22.0 * (1.0 - crown_in)) * k, k, a);
            }
            // Confetti.
            if party > 0.0 {
                let cols = [pal::RED, pal::YELLOW, pal::CYAN, pal::GREEN, pal::PINK, pal::PURPLE];
                for i in 0..16 {
                    let hsh = hash(i as u64 * 7919 + self.duel_seed);
                    let sx = (hsh % 1000) as f32 / 1000.0 * 2.0 - 1.0;
                    let speed = 0.6 + ((hsh >> 10) % 100) as f32 / 100.0 * 0.8;
                    let ph = (party * speed + (hsh >> 20) as f32 % 7.0 / 7.0).fract();
                    let px = wx + sx * 50.0 * k + (ph * 9.0 + i as f32).sin() * 3.0;
                    let py = cy - 20.0 * k + ph * 40.0 * k;
                    self.surf.fill_rr(rf(px, py, 2.6 * k, 1.6 * k), 0.6, self.el(cols[i % 6], (1.0 - ph * 0.5) * a));
                }
            }
            // K.O.!
            let ko = span(u, 2.6, 2.9);
            if ko > 0.0 {
                let s = 1.0 + 1.4 * (1.0 - ease(ko));
                let out = 1.0 - span(u, 3.5, 3.75);
                self.ko_mark(cx0 + 1.0 * k, cy - 1.0, k * s, a * ko * out);
            }
            // Who won, under it.
            let tag = span(u, 3.7, 3.95) * (1.0 - span(u, 4.6, 4.9));
            if tag > 0.0 {
                let c = config::cur();
                let name = if cat_wins { tr("Cat") } else { tr("Robot") };
                let _ = &c;
                let s = format!("{} {}!", name, tr("wins"));
                let mid = (wx + lx) / 2.0;
                self.surf.text(&s, &self.f.cal_sub, rf(mid - 40.0, cy - 6.0 + (1.0 - tag) * 4.0, 80.0, 12.0), self.ic(pal::YELLOW, tag * a));
            }
        }
    }

    /// "VS" in strokes.
    fn vs_mark(&self, cx: f32, cy: f32, k: f32, a: f32) {
        let c = self.ic(pal::YELLOW, a);
        let o = self.blend(0.0);
        let v = [(cx - 9.0 * k, cy - 6.0 * k), (cx - 5.0 * k, cy + 6.0 * k), (cx - 1.0 * k, cy - 6.0 * k)];
        let s: Vec<_> = [(3.0f32, -5.0f32), (1.0, -6.0), (-1.5, -5.6), (-3.0, -4.0), (-2.5, -1.8), (0.0, -0.5), (2.5, 0.8), (3.0, 3.0), (1.5, 5.4), (-1.0, 6.0), (-3.0, 5.0)]
            .iter()
            .map(|(px, py)| (cx + 5.5 * k + px * k, cy + py * k))
            .collect();
        for (w, col) in [(4.6, o), (2.4, c)] {
            self.surf.stroke_poly(&v, false, col, w);
            self.surf.stroke_poly(&s, false, col, w);
        }
    }

    /// "K.O." in strokes, with a dark outline so it reads over anything.
    fn ko_mark(&self, cx: f32, cy: f32, k: f32, a: f32) {
        if a <= 0.01 {
            return;
        }
        let c = self.ic(pal::YELLOW, a);
        let o = self.blend(0.0);
        let x = cx - 12.0 * k;
        let h = 6.5 * k;
        for (w, col) in [(5.0, with_alpha(o, a)), (2.6, c)] {
            // K
            self.surf.line(x, cy - h, x, cy + h, col, w);
            self.surf.stroke_poly(&[(x + 6.0 * k, cy - h), (x + 0.5 * k, cy), (x + 6.5 * k, cy + h)], false, col, w);
            // .
            self.surf.fill_circle(x + 9.5 * k, cy + h - 0.5 * k, w * 0.55, col);
            // O
            let ring: Vec<_> = (0..=24).map(|i| { let t = i as f32 / 24.0 * TAU; (x + 17.0 * k + t.cos() * 4.6 * k, cy + t.sin() * h) }).collect();
            self.surf.stroke_poly(&ring, true, col, w);
            // .
            self.surf.fill_circle(x + 24.5 * k, cy + h - 0.5 * k, w * 0.55, col);
        }
    }

    /// A little golden crown.
    fn crown(&self, cx: f32, by: f32, k: f32, a: f32) {
        let g = self.ic(pal::YELLOW, a);
        let pts = [(cx - 7.0 * k, by), (cx - 7.5 * k, by - 6.0 * k), (cx - 3.5 * k, by - 3.0 * k), (cx, by - 7.5 * k), (cx + 3.5 * k, by - 3.0 * k), (cx + 7.5 * k, by - 6.0 * k), (cx + 7.0 * k, by)];
        self.surf.fill_poly(&pts, g);
        self.surf.fill_circle(cx, by - 7.5 * k, 1.2 * k, self.el(pal::RED, a));
    }
}

/// How long each goodbye runs (seconds).
pub(super) const CAT_BYE_S: f32 = 2.65;
pub(super) const DUEL_BYE_S: f32 = 5.0;
