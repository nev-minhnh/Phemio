//! The shared map of every screen on every linked PC, and where the pointer
//! goes when it reaches an edge.
//!
//! Each screen keeps its real pixels (in its own PC's desktop) and a place on
//! the map in "map units" (pixels ÷ scaling, so a 150 % laptop screen and a
//! 100 % monitor show at their true relative sizes). The map, not Windows'
//! own display arrangement, decides every crossing — even between two
//! screens of the same PC — so screens of different PCs can sit side by side
//! in any order.
//!
//! No Windows calls here: it's tested on its own.

/// Screens closer than this (map units) count as touching.
pub const TOUCH: f32 = 2.5;

#[derive(Clone, Debug, PartialEq)]
pub struct Screen {
    pub machine: u64,
    /// The display's device name on its PC (\\.\DISPLAY1).
    pub key: String,
    /// What to call it ("Laptop 1 · DELL U2723QE").
    pub label: String,
    /// x, y, w, h in its PC's desktop (physical pixels).
    pub px: (i32, i32, i32, i32),
    pub scale: f32,
    /// Top-left on the shared map.
    pub g: (f32, f32),
}

impl Screen {
    pub fn uid(&self) -> String {
        format!("{:x}/{}", self.machine, self.key)
    }
    pub fn gw(&self) -> f32 {
        self.px.2 as f32 / self.scale
    }
    pub fn gh(&self) -> f32 {
        self.px.3 as f32 / self.scale
    }
    /// (left, top, right, bottom) on the map.
    pub fn grect(&self) -> (f32, f32, f32, f32) {
        (self.g.0, self.g.1, self.g.0 + self.gw(), self.g.1 + self.gh())
    }
    pub fn g_contains(&self, p: (f32, f32), tol: f32) -> bool {
        let (l, t, r, b) = self.grect();
        p.0 >= l - tol && p.0 <= r + tol && p.1 >= t - tol && p.1 <= b + tol
    }
    pub fn px_contains(&self, x: i32, y: i32) -> bool {
        let (x0, y0, w, h) = self.px;
        x >= x0 && x < x0 + w && y >= y0 && y < y0 + h
    }
    /// A pixel → the map.
    pub fn to_g(&self, x: i32, y: i32) -> (f32, f32) {
        (self.g.0 + (x - self.px.0) as f32 / self.scale, self.g.1 + (y - self.px.1) as f32 / self.scale)
    }
    /// A map point → the nearest pixel on this screen.
    pub fn to_px(&self, p: (f32, f32)) -> (i32, i32) {
        let (x0, y0, w, h) = self.px;
        let x = x0 + ((p.0 - self.g.0) * self.scale).round() as i32;
        let y = y0 + ((p.1 - self.g.1) * self.scale).round() as i32;
        // (max/min, not clamp: never panics, even for an odd 0-pixel screen.)
        (x.min(x0 + w - 1).max(x0), y.min(y0 + h - 1).max(y0))
    }
    /// `p` pulled inside this screen, a pixel in from its edges.
    pub fn clamp_g(&self, p: (f32, f32)) -> (f32, f32) {
        let (l, t, r, b) = self.grect();
        let m = (1.0 / self.scale).min((r - l) / 2.0).min((b - t) / 2.0).max(0.0);
        let fix = |v: f32, lo: f32, hi: f32| if v.is_nan() { lo } else { v.min(hi).max(lo) };
        (fix(p.0, l + m, r - m), fix(p.1, t + m, b - m))
    }
}

/// What a pointer move on this PC should do.
#[derive(Clone, Debug, PartialEq)]
pub enum Local {
    /// Nothing special: let Windows move it.
    Pass,
    /// Put the pointer here instead (screen index, pixel) — another of this
    /// PC's screens per the map, or held at an edge with nothing beyond.
    Warp(usize, i32, i32),
    /// It goes to another PC: that screen, the map point and its pixel there.
    Enter(usize, (f32, f32), i32, i32),
}

/// Where the pointer goes while it's on another PC.
#[derive(Clone, Debug, PartialEq)]
pub enum Remote {
    /// Still on that PC: the screen, map point and its pixel.
    Move(usize, (f32, f32), i32, i32),
    /// Back on this PC: the screen and pixel.
    Exit(usize, i32, i32),
}

#[derive(Clone, Debug, Default)]
pub struct Geo {
    /// This PC.
    pub me: u64,
    pub screens: Vec<Screen>,
}

impl Geo {
    /// Each screen's number (1, 2, …) as the Link page and "Identify" show it:
    /// left to right on the shared map (top to bottom when level), so both
    /// PCs number every screen the same way.
    pub fn numbers(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.screens.len()).collect();
        order.sort_by(|&a, &b| {
            let (sa, sb) = (&self.screens[a], &self.screens[b]);
            let ca = (sa.g.0 + sa.gw() / 2.0, sa.g.1 + sa.gh() / 2.0);
            let cb = (sb.g.0 + sb.gw() / 2.0, sb.g.1 + sb.gh() / 2.0);
            ca.0.total_cmp(&cb.0).then(ca.1.total_cmp(&cb.1)).then(sa.uid().cmp(&sb.uid()))
        });
        let mut out = vec![0; self.screens.len()];
        for (n, i) in order.into_iter().enumerate() {
            out[i] = n + 1;
        }
        out
    }

    pub fn local_at_px(&self, x: i32, y: i32) -> Option<usize> {
        self.screens.iter().position(|s| s.machine == self.me && s.px_contains(x, y))
    }

    /// The screen (other than `from`) at map point `p`, nearest first.
    fn neighbor(&self, from: usize, p: (f32, f32)) -> Option<usize> {
        let mut best: Option<(f32, usize)> = None;
        for (i, s) in self.screens.iter().enumerate() {
            if i == from || !s.g_contains(p, TOUCH) {
                continue;
            }
            let (l, t, r, b) = s.grect();
            let dx = (l - p.0).max(0.0).max(p.0 - r);
            let dy = (t - p.1).max(0.0).max(p.1 - b);
            let d = dx * dx + dy * dy;
            if best.map(|b| d < b.0).unwrap_or(true) {
                best = Some((d, i));
            }
        }
        best.map(|b| b.1)
    }

    /// The pointer on this PC moved from `last` to `pt` (pixels).
    pub fn local_move(&self, last: (i32, i32), pt: (i32, i32)) -> Local {
        let Some(cur) = self.local_at_px(last.0, last.1).or_else(|| self.local_at_px(pt.0, pt.1)) else { return Local::Pass };
        let s = &self.screens[cur];
        let (x0, y0, w, h) = s.px;
        let (x1, y1) = (x0 + w - 1, y0 + h - 1);
        let (dx, dy) = (pt.0 - last.0, pt.1 - last.1);
        let inside = s.px_contains(pt.0, pt.1);
        // Pushing out through an edge (or already past it).
        let ox = if pt.0 > x1 || (pt.0 >= x1 && dx > 0) {
            1
        } else if pt.0 < x0 || (pt.0 <= x0 && dx < 0) {
            -1
        } else {
            0
        };
        let oy = if pt.1 > y1 || (pt.1 >= y1 && dy > 0) {
            1
        } else if pt.1 < y0 || (pt.1 <= y0 && dy < 0) {
            -1
        } else {
            0
        };
        if ox == 0 && oy == 0 {
            return Local::Pass;
        }
        let e = (pt.0.clamp(x0, x1), pt.1.clamp(y0, y1));
        let ge = s.to_g(e.0, e.1);
        // The far side of the edge: the exact edge line, a step beyond.
        let (l, t, r, b) = s.grect();
        let edge_x = if ox > 0 { r + 1.0 } else if ox < 0 { l - 1.0 } else { ge.0 };
        let edge_y = if oy > 0 { b + 1.0 } else if oy < 0 { t - 1.0 } else { ge.1 };
        let mut tries = vec![(edge_x, edge_y)];
        if ox != 0 && oy != 0 {
            tries.push((edge_x, ge.1));
            tries.push((ge.0, edge_y));
        }
        for p in tries {
            if let Some(ti) = self.neighbor(cur, p) {
                let ts = &self.screens[ti];
                let gp = ts.clamp_g(p);
                let (x, y) = ts.to_px(gp);
                return if ts.machine == self.me { Local::Warp(ti, x, y) } else { Local::Enter(ti, gp, x, y) };
            }
        }
        // Nothing there on the map: stay on this screen (even if Windows
        // would have moved to one of this PC's other screens).
        if inside {
            Local::Pass
        } else {
            Local::Warp(cur, e.0, e.1)
        }
    }

    /// The pointer is on another PC's screen `cur`, at map point `p`, and the
    /// mouse moved by `d` map units.
    pub fn remote_move(&self, cur: usize, p: (f32, f32), d: (f32, f32)) -> Remote {
        // The screen it was on went away (unplugged over there): back home.
        let Some(s) = self.screens.get(cur).filter(|s| s.machine != self.me) else {
            return match self.screens.iter().position(|s| s.machine == self.me) {
                Some(h) => {
                    let s = &self.screens[h];
                    Remote::Exit(h, s.px.0 + s.px.2 / 2, s.px.1 + s.px.3 / 2)
                }
                None => Remote::Exit(0, 0, 0),
            };
        };
        let np = (p.0 + d.0, p.1 + d.1);
        let (l, t, r, b) = s.grect();
        if np.0 >= l && np.0 <= r && np.1 >= t && np.1 <= b {
            let (x, y) = s.to_px(np);
            return Remote::Move(cur, np, x, y);
        }
        // Past an edge: whatever touches it there (also sliding along one axis).
        for q in [np, (np.0, np.1.clamp(t, b)), (np.0.clamp(l, r), np.1)] {
            if q.0 >= l && q.0 <= r && q.1 >= t && q.1 <= b {
                continue;
            }
            if let Some(ti) = self.neighbor(cur, q) {
                let ts = &self.screens[ti];
                let gp = ts.clamp_g(q);
                let (x, y) = ts.to_px(gp);
                return if ts.machine == self.me { Remote::Exit(ti, x, y) } else { Remote::Move(ti, gp, x, y) };
            }
        }
        let cp = (np.0.clamp(l, r), np.1.clamp(t, b));
        let (x, y) = s.to_px(cp);
        Remote::Move(cur, cp, x, y)
    }

    /// Where a screen dropped at `g` while dragging should land: never on
    /// top of another screen, and its edges pulled onto nearby screens' edges
    /// within `reach` map units.
    pub fn snap(&self, i: usize, g: (f32, f32), reach: f32) -> (f32, f32) {
        if i >= self.screens.len() {
            return g;
        }
        let g = self.separate(i, g);
        let s = &self.screens[i];
        let (w, h) = (s.gw(), s.gh());
        let (mut bx, mut by) = (g.0, g.1);
        let (mut best_x, mut best_y) = (reach, reach);
        let near = reach;
        for (j, o) in self.screens.iter().enumerate() {
            if j == i {
                continue;
            }
            let (l, t, r, b) = o.grect();
            // Only snap to screens nearby on the other axis.
            let near_y = g.1 < b + near && g.1 + h > t - near;
            let near_x = g.0 < r + near && g.0 + w > l - near;
            if near_y {
                for (cand, from) in [(r, g.0), (l - w, g.0), (l, g.0), (r - w, g.0)] {
                    let d = (cand - from).abs();
                    if d < best_x {
                        best_x = d;
                        bx = cand;
                    }
                }
            }
            if near_x {
                for (cand, from) in [(b, g.1), (t - h, g.1), (t, g.1), (b - h, g.1), (t + (b - t - h) / 2.0, g.1)] {
                    let d = (cand - from).abs();
                    if d < best_y {
                        best_y = d;
                        by = cand;
                    }
                }
            }
        }
        self.separate(i, (bx, by))
    }

    /// Push screen `i` (at `g`) out of any screen it overlaps, the shortest way.
    pub fn separate(&self, i: usize, mut g: (f32, f32)) -> (f32, f32) {
        if i >= self.screens.len() {
            return g;
        }
        let (w, h) = (self.screens[i].gw(), self.screens[i].gh());
        for _ in 0..6 {
            let mut moved = false;
            for (j, o) in self.screens.iter().enumerate() {
                if j == i {
                    continue;
                }
                let (l, t, r, b) = o.grect();
                let ox = (g.0 + w).min(r) - g.0.max(l);
                let oy = (g.1 + h).min(b) - g.1.max(t);
                if ox <= 0.5 || oy <= 0.5 {
                    continue;
                }
                let opts = [(r - g.0, 0.0), (l - (g.0 + w), 0.0), (0.0, b - g.1), (0.0, t - (g.1 + h))];
                let best = opts.iter().min_by(|a, c| (a.0.abs() + a.1.abs()).total_cmp(&(c.0.abs() + c.1.abs()))).unwrap();
                g = (g.0 + best.0, g.1 + best.1);
                moved = true;
            }
            if !moved {
                break;
            }
        }
        g
    }

    /// Lay out the screens that have no place yet: each PC's screens as
    /// Windows arranges them, that group beside what's already placed.
    /// `placed[i]` says whether screen i already has a place.
    pub fn auto_place(&mut self, placed: &[bool]) {
        let mut fixed: Vec<bool> = placed.to_vec();
        let machines: Vec<u64> = {
            let mut v: Vec<u64> = self.screens.iter().map(|s| s.machine).collect();
            v.sort_by_key(|m| (*m != self.me, *m));
            v.dedup();
            v
        };
        for m in machines {
            let idx: Vec<usize> = (0..self.screens.len()).filter(|&i| self.screens[i].machine == m && !fixed[i]).collect();
            if idx.is_empty() {
                continue;
            }
            // Within the PC: start from its left-most screen, walk Windows' adjacency.
            let first = *idx.iter().min_by_key(|&&i| (self.screens[i].px.0, self.screens[i].px.1)).unwrap();
            self.screens[first].g = (0.0, 0.0);
            let mut done = vec![first];
            let mut changed = true;
            while changed {
                changed = false;
                for &i in &idx {
                    if done.contains(&i) {
                        continue;
                    }
                    let si = self.screens[i].clone();
                    let (x0, y0, w0, h0) = si.px;
                    let mut at = None;
                    for &d in &done {
                        let sd = &self.screens[d];
                        let (x1, y1, w1, h1) = sd.px;
                        at = if x0 == x1 + w1 {
                            Some((sd.g.0 + sd.gw(), sd.g.1 + (y0 - y1) as f32 / sd.scale))
                        } else if x0 + w0 == x1 {
                            Some((sd.g.0 - si.gw(), sd.g.1 + (y0 - y1) as f32 / sd.scale))
                        } else if y0 == y1 + h1 {
                            Some((sd.g.0 + (x0 - x1) as f32 / sd.scale, sd.g.1 + sd.gh()))
                        } else if y0 + h0 == y1 {
                            Some((sd.g.0 + (x0 - x1) as f32 / sd.scale, sd.g.1 - si.gh()))
                        } else {
                            None
                        };
                        if at.is_some() {
                            break;
                        }
                    }
                    if let Some(g) = at {
                        self.screens[i].g = g;
                        done.push(i);
                        changed = true;
                    }
                }
            }
            // Anything not touching: in a row after the rest.
            for &i in &idx {
                if !done.contains(&i) {
                    let right = done.iter().map(|&d| self.screens[d].grect().2).fold(f32::MIN, f32::max);
                    self.screens[i].g = (right, 0.0);
                    done.push(i);
                }
            }
            // The whole group goes to the right of what's already on the map.
            let (min_x, min_y) = idx.iter().fold((f32::MAX, f32::MAX), |a, &i| (a.0.min(self.screens[i].g.0), a.1.min(self.screens[i].g.1)));
            let on_map: Vec<usize> = (0..self.screens.len()).filter(|&i| fixed[i]).collect();
            let (ox, oy) = if on_map.is_empty() {
                (0.0, 0.0)
            } else {
                (on_map.iter().map(|&i| self.screens[i].grect().2).fold(f32::MIN, f32::max), on_map.iter().map(|&i| self.screens[i].g.1).fold(f32::MAX, f32::min))
            };
            for &i in &idx {
                self.screens[i].g.0 += ox - min_x;
                self.screens[i].g.1 += oy - min_y;
                fixed[i] = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scr(m: u64, key: &str, px: (i32, i32, i32, i32), scale: f32, g: (f32, f32)) -> Screen {
        Screen { machine: m, key: key.into(), label: key.into(), px, scale, g }
    }

    /// Minh's desk: Laptop 1 (left) + Screen 3 (bottom middle) on PC 1;
    /// Screens 1 and 2 (top) + Laptop 2 (right) on PC 2.
    fn desk(me: u64) -> Geo {
        Geo {
            me,
            screens: vec![
                // PC 1. Windows: laptop at 0,0 (150 %), Screen 3 to its right.
                scr(1, "L1", (0, 0, 2880, 1800), 1.5, (-1920.0, 300.0)),
                scr(1, "S3", (2880, 0, 1920, 1080), 1.0, (1600.0, 1440.0)),
                // PC 2. Windows: Screen 1 at 0,0, Screen 2 right of it, laptop right of that.
                scr(2, "S1", (0, 0, 2560, 1440), 1.0, (0.0, 0.0)),
                scr(2, "S2", (2560, 0, 2560, 1440), 1.0, (2560.0, 0.0)),
                scr(2, "L2", (5120, 0, 2880, 1800), 1.5, (5120.0, 300.0)),
            ],
        }
    }

    #[test]
    fn laptop1_right_edge_goes_to_screen1() {
        let g = desk(1);
        // Laptop 1's right edge, upper part (map y ≈ 300 + 200/1.5).
        match g.local_move((2878, 200), (2879, 200)) {
            Local::Enter(t, p, x, y) => {
                assert_eq!(g.screens[t].key, "S1");
                assert!(p.0 > 0.0 && p.0 < 2.0);
                assert_eq!(x, 1);
                assert_eq!(y, (300.0 + 200.0 / 1.5f32).round() as i32);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn laptop1_right_edge_low_part_is_a_wall() {
        let g = desk(1);
        // Map y = 300 + 1700/1.5 ≈ 1433 → still beside Screen 1 (ends at 1440).
        assert!(matches!(g.local_move((2878, 1700), (2879, 1700)), Local::Enter(..)));
        // Windows would carry the pointer into Screen 3 (it's right of the
        // laptop in Display settings) — but on the map nothing is there below
        // Screen 1's bottom... the laptop's bottom is at 300+1200=1500, so
        // y=1790 maps to 1493: nothing to the right → held at the edge.
        let r = g.local_move((2879, 1790), (2885, 1790));
        assert_eq!(r, Local::Warp(0, 2879, 1790));
    }

    #[test]
    fn screen3_up_goes_to_screen1_or_2() {
        let g = desk(1);
        // Screen 3 spans map x 1600..3520; its top at 1440 meets S1 (..2560) and S2 (2560..).
        match g.local_move((2880 + 100, 1), (2880 + 100, 0)) {
            Local::Enter(t, _, x, y) => {
                assert_eq!(g.screens[t].key, "S1");
                assert_eq!((x, y), (1700, 1439));
            }
            o => panic!("{o:?}"),
        }
        match g.local_move((2880 + 1500, 1), (2880 + 1500, 0)) {
            Local::Enter(t, _, x, _) => {
                assert_eq!(g.screens[t].key, "S2");
                assert_eq!(x, 2560 + (3100 - 2560));
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn same_pc_moves_follow_the_map_not_windows() {
        let g = desk(1);
        // Screen 3's left edge: on the map nothing touches it (Laptop 1 ends at x=0).
        // Windows would move into the laptop: held instead.
        assert_eq!(g.local_move((2881, 500), (2879, 500)), Local::Warp(1, 2880, 500));
        // Moving inside a screen: untouched.
        assert_eq!(g.local_move((100, 100), (140, 120)), Local::Pass);
    }

    #[test]
    fn on_the_other_pc_and_back() {
        let g = desk(1);
        let s1 = g.screens.iter().position(|s| s.key == "S1").unwrap();
        // Across Screen 1 into Screen 2 (same PC over there).
        match g.remote_move(s1, (2559.0, 700.0), (5.0, 0.0)) {
            Remote::Move(t, p, x, y) => {
                assert_eq!(g.screens[t].key, "S2");
                assert!((p.0 - 2564.0).abs() < 0.01);
                assert_eq!((x, y), (2564, 700));
            }
            o => panic!("{o:?}"),
        }
        // Down out of Screen 1 into Screen 3 — back on this PC.
        match g.remote_move(s1, (2000.0, 1439.0), (0.0, 4.0)) {
            Remote::Exit(t, x, y) => {
                assert_eq!(g.screens[t].key, "S3");
                assert_eq!((x, y), (2880 + 400, 3));
            }
            o => panic!("{o:?}"),
        }
        // Down out of Screen 1 left of Screen 3: a wall.
        match g.remote_move(s1, (500.0, 1439.0), (0.0, 30.0)) {
            Remote::Move(t, p, ..) => {
                assert_eq!(t, s1);
                assert_eq!(p.1, 1440.0);
            }
            o => panic!("{o:?}"),
        }
        // Left out of Screen 1 → Laptop 1 (this PC).
        match g.remote_move(s1, (1.0, 600.0), (-3.0, 0.0)) {
            Remote::Exit(t, x, y) => {
                assert_eq!(g.screens[t].key, "L1");
                // The 2-unit overshoot carries over (×1.5 on the laptop).
                assert_eq!(x, 2877);
                assert_eq!(y, ((600.0 - 300.0) * 1.5f32).round() as i32);
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn from_pc2_side() {
        let g = desk(2);
        // Laptop 2's left edge low part → Screen 3? Screen 3 spans y 1440..2520,
        // laptop 2 spans y 300..1500 at x 5120: S3's right edge is 3520 — not
        // touching laptop 2, so moving left from its low part is a wall,
        // and its upper part goes into Screen 2 (same PC).
        let r = g.local_move((5121, 300), (5119, 300));
        assert_eq!(r, Local::Warp(3, 5119, 500));
        // Screen 2 → down into Screen 3 (PC 1).
        assert!(matches!(g.local_move((3000, 1438), (3000, 1439)), Local::Enter(t, ..) if g.screens[t].key == "S3"));
    }

    #[test]
    fn a_screen_that_went_away_sends_the_pointer_home() {
        let g = desk(1);
        assert!(matches!(g.remote_move(99, (10.0, 10.0), (1.0, 1.0)), Remote::Exit(..)));
        // A local screen index isn't "over there" either.
        assert!(matches!(g.remote_move(0, (10.0, 10.0), (1.0, 1.0)), Remote::Exit(..)));
        assert_eq!(g.snap(99, (1.0, 2.0), 40.0), (1.0, 2.0));
    }

    #[test]
    fn tiny_or_odd_screens_never_panic() {
        let s = scr(1, "Z", (0, 0, 0, 0), 1.0, (0.0, 0.0));
        let _ = s.to_px((5.0, 5.0));
        let _ = s.clamp_g((f32::NAN, 3.0));
    }

    #[test]
    fn numbering_is_left_to_right() {
        let g = desk(1);
        let n = g.numbers();
        // L1 (x -1920), S1 (0), S3 (1600, lower), S2 (2560), L2 (5120)
        assert_eq!(n, vec![1, 3, 2, 4, 5]);
    }

    #[test]
    fn snapping() {
        let g = desk(1);
        // Dropping Screen 3 a little off: it snaps under Screens 1/2.
        let p = g.snap(1, (1612.0, 1452.0), 40.0);
        assert_eq!(p.1, 1440.0);
    }

    #[test]
    fn dropping_on_top_pushes_it_out() {
        let g = desk(1);
        // Screen 3 dropped mostly over Screen 1: it ends up just below it.
        let p = g.snap(1, (300.0, 1200.0), 40.0);
        let s = &g.screens[1];
        for (j, o) in g.screens.iter().enumerate() {
            if j == 1 {
                continue;
            }
            let (l, t, r, b) = o.grect();
            let ox = (p.0 + s.gw()).min(r) - p.0.max(l);
            let oy = (p.1 + s.gh()).min(b) - p.1.max(t);
            assert!(ox <= 0.5 || oy <= 0.5, "overlaps {}", o.key);
        }
        assert_eq!(p.1, 1440.0);
    }

    #[test]
    fn auto_place_groups() {
        let mut g = Geo {
            me: 1,
            screens: vec![
                scr(1, "A", (0, 0, 2880, 1800), 1.5, (0.0, 0.0)),
                scr(1, "B", (2880, 0, 1920, 1080), 1.0, (0.0, 0.0)),
                scr(2, "C", (0, 0, 2560, 1440), 1.0, (0.0, 0.0)),
                scr(2, "D", (-1920, 200, 1920, 1080), 1.0, (0.0, 0.0)),
            ],
        };
        g.auto_place(&[false, false, false, false]);
        // A at 0; B right after A's 1920 map units.
        assert_eq!(g.screens[0].g, (0.0, 0.0));
        assert_eq!(g.screens[1].g, (1920.0, 0.0));
        // PC 2's group sits to the right of PC 1's (B ends at 3840).
        assert_eq!(g.screens[3].g.0, 3840.0);
        assert_eq!(g.screens[2].g.0, 3840.0 + 1920.0);
        assert_eq!(g.screens[2].g.1 - g.screens[3].g.1, -200.0);
    }
}
