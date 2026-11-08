//! The Link page: every screen of both PCs on one map (drag them to match
//! your desk), pairing, and how the link is doing.

use super::*;
use crate::link::{self, Pairing};

/// How often the page redraws while it's open (pointer dot, delay).
pub(super) const T_LINKUI: usize = 20;

/// Height of the screen map's header strip.
const HEAD: f32 = 30.0;

impl Island {
    /// A pairing request arrived (or the link changed): show the page when
    /// there's something to answer.
    pub fn on_link(&mut self) {
        // While linked, note the closed island's state now and then (to find
        // out why it can look blank on the PC being controlled).
        {
            static LAST: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
            let mut g = LAST.lock().unwrap_or_else(|e| e.into_inner());
            if g.map(|t| t.elapsed().as_secs() >= 30).unwrap_or(true) && link::view().map(|v| v.session.is_some()).unwrap_or(false) {
                *g = Some(Instant::now());
                util::log(&format!(
                    "island: exp={} peek={} search={} trans={} supp={} anim={} col={:.2}->{:.2} pk={:.2} ex={:.2} w={:.0} h={:.0} mascot={:?} fade={:.2}",
                    self.expanded,
                    self.peek.is_some(),
                    self.search.is_some(),
                    self.trans.is_some(),
                    self.suppressed,
                    self.animating,
                    self.a_col.v,
                    self.a_col.target,
                    self.a_peek.v,
                    self.a_exp.v,
                    self.w.v,
                    self.h.v,
                    self.mascot,
                    self.masc_fade.v
                ));
            }
        }
        let wants = link::view().map(|v| matches!(v.pairing, Pairing::Code { mine: None, .. })).unwrap_or(false);
        if wants && enabled_pages().contains(&Page::Link) && (self.page != Page::Link || !self.expanded) {
            if self.mascot != Mascot::Off {
                self.set_mascot(Mascot::Off);
            }
            self.go_page(Page::Link, 0);
            self.slide.snap(0.0);
            self.set_expanded(true);
        }
        if self.expanded && matches!(self.page, Page::Link | Page::Shelf) {
            self.render();
        }
    }

    pub(super) fn link_drag_start(&mut self, i: usize, x: f32, y: f32) {
        let Some(v) = link::view() else { return };
        let Some(s) = v.geo.screens.get(i) else { return };
        self.link_drag = Some((i, (x, y), s.g, s.g));
    }

    pub(super) fn link_drag_move(&mut self, x: f32, y: f32) {
        let Some((i, start, orig, _)) = self.link_drag else { return };
        let Some(v) = link::view() else { return };
        // Map units per screen DIP: the map's own scale times the page's fit.
        let k = (self.link_fit.2 * self.page_fit).max(1e-3);
        let g = (orig.0 + (x - start.0) / k, orig.1 + (y - start.1) / k);
        // Edges snap within about 14 pixels on the page.
        let snapped = v.geo.snap(i, g, 14.0 / k);
        self.link_drag = Some((i, start, orig, snapped));
        self.render();
    }

    pub(super) fn link_drag_end(&mut self) {
        let Some((i, _, orig, now)) = self.link_drag.take() else { return };
        // A click (no move): that screen's display settings on the right
        // (again: back to the Link status).
        if (now.0 - orig.0).abs() <= 0.5 && (now.1 - orig.1).abs() <= 0.5 {
            if let Some(uid) = link::view().and_then(|v| v.geo.screens.get(i).map(|s| s.uid())) {
                self.link_sel = if self.link_sel.as_deref() == Some(uid.as_str()) { None } else { Some(uid) };
                self.disp = None;
            }
        }
        if (now.0 - orig.0).abs() > 0.5 || (now.1 - orig.1).abs() > 0.5 {
            if let Some((uid, mine)) = link::view().and_then(|v| v.geo.screens.get(i).map(|s| (s.uid(), s.machine == v.me))) {
                if mine && link::alone() {
                    // Not linked: move the screen in Windows itself, as
                    // Settings → Display does.
                    if let Err(e) = link::arrange_local(&uid, now) {
                        crate::util::log(&format!("link: couldn't rearrange the displays: {e}"));
                    }
                } else {
                    link::place(&uid, now);
                }
            }
        }
        self.render();
    }

    pub(super) fn page_link(&mut self, bx: f32, by: f32, bw: f32, bh: f32, a: f32) {
        let g = gfx::g();
        let c = config::cur();
        unsafe { SetTimer(Some(self.hwnd), T_LINKUI, 120, None) };
        if !c.b("link_on") {
            self.surf.text(tr("Link is off"), &self.f.title, rf(bx, by + bh / 2.0 - 40.0, bw, 24.0), self.fg(0.9 * a));
            self.surf.text(tr("One mouse and keyboard for all your PCs. Each PC needs Phemio."), &self.f.tiny, rf(bx, by + bh / 2.0 - 14.0, bw, 16.0), self.fg(0.5 * a));
            let label = tr("Turn Link on");
            let w = g.measure(label, &self.f.body) + 36.0;
            let r = rf(bx, by + bh / 2.0 + 12.0, w, 30.0);
            let accent = self.ic(pal::INDIGO, 1.0);
            self.surf.fill_rr(r, 15.0, with_alpha(accent, if self.hot == Some(Hit::LinkOn) { 1.0 } else { 0.85 } * a));
            self.surf.text(label, &self.f.body_c, r, Island::text_on(accent, a));
            self.hits.push((Hit::LinkOn, r));
            return;
        }
        let Some(v) = link::view() else {
            self.surf.text(tr("Starting…"), &self.f.body, rf(bx, by, bw, bh), self.fg(0.5 * a));
            return;
        };
        let me_c = self.ic(pal::BLUE, 1.0);
        let them_c = self.ic(pal::ORANGE, 1.0);
        // ---- The map (left) ----
        let mw = (bw * 0.62).round();
        let map = rf(bx, by, mw, bh);
        self.surf.fill_rr(map, 12.0, self.fg(0.04 * a));
        let mut screens = v.geo.screens.clone();
        if let Some((i, _, _, now)) = self.link_drag {
            if let Some(s) = screens.get_mut(i) {
                s.g = now;
            }
        }
        if !screens.is_empty() {
            let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for s in &screens {
                let (x0, y0, x1, y1) = s.grect();
                l = l.min(x0);
                t = t.min(y0);
                r = r.max(x1);
                b = b.max(y1);
            }
            // While dragging, keep the scale steady so the map doesn't jump.
            // The top strip holds the title and the Link switch; screens go below.
            let (sy, sh) = (by + HEAD, bh - HEAD);
            let k = if self.link_drag.is_some() { self.link_fit.2 } else { ((mw - 24.0) / (r - l).max(1.0)).min((sh - 14.0) / (b - t).max(1.0)) };
            let (ox, oy) = if self.link_drag.is_some() {
                (self.link_fit.0, self.link_fit.1)
            } else {
                (bx + (mw - (r - l) * k) / 2.0, sy + (sh - 4.0 - (b - t) * k) / 2.0)
            };
            let origin = if self.link_drag.is_some() { self.link_fit.3 } else { (l, t) };
            self.link_fit = (ox, oy, k, origin);
            let to = |p: (f32, f32)| (ox + (p.0 - origin.0) * k, oy + (p.1 - origin.1) * k);
            self.surf.clip(rf(bx, by + HEAD + 1.0, mw, bh - HEAD - 1.0));
            // The same numbers Identify shows on the screens.
            let geo_now = link::geom::Geo { me: v.me, screens: screens.clone() };
            let nums = geo_now.numbers();
            for (i, s) in screens.iter().enumerate() {
                let (x0, y0) = to((s.g.0, s.g.1));
                let rr = rf(x0 + 1.0, y0 + 1.0, s.gw() * k - 2.0, s.gh() * k - 2.0);
                let mine = s.machine == v.me;
                let col = if mine { me_c } else { them_c };
                let dragging = self.link_drag.map(|d| d.0 == i).unwrap_or(false) || self.link_sel.as_deref() == Some(s.uid().as_str());
                let hot = self.hot == Some(Hit::LinkScreen(i));
                self.surf.fill_rr(rr, 5.0, with_alpha(col, if dragging { 0.42 } else if hot { 0.3 } else { 0.2 } * a));
                self.surf.stroke_rr(rr, 5.0, with_alpha(col, if dragging { 1.0 } else { 0.75 } * a), if dragging { 2.0 } else { 1.2 });
                if let Some(n) = nums.get(i) {
                    let nr = rf(rr.left + 3.0, rr.top + 2.0, 16.0, 15.0);
                    self.surf.fill_rr(nr, 4.0, with_alpha(col, 0.85 * a));
                    self.surf.text(&n.to_string(), &self.f.tiny_c, nr, Island::text_on(col, a));
                }
                if rr.bottom - rr.top > 26.0 && rr.right - rr.left > 40.0 {
                    let label = ellipsize(g, &s.label, &self.f.tiny_c, rr.right - rr.left - 8.0);
                    let mid = (rr.top + rr.bottom) / 2.0;
                    self.surf.text(&label, &self.f.tiny_c, rf(rr.left + 2.0, mid - 15.0, rr.right - rr.left - 4.0, 15.0), self.fg(0.9 * a));
                    let res = format!("{}×{}", s.px.2, s.px.3);
                    self.surf.text(&res, &self.f.tiny_c, rf(rr.left + 2.0, mid + 1.0, rr.right - rr.left - 4.0, 14.0), self.fg(0.45 * a));
                }
                if a > 0.5 {
                    self.hits.push((Hit::LinkScreen(i), rr));
                }
                // Each PC's main screen says so; point at another to make it
                // the main one (taskbar, new windows) — as Settings → Display does.
                let main = s.px.0 == 0 && s.px.1 == 0;
                let hot_main = self.hot == Some(Hit::LinkMain(i));
                if rr.bottom - rr.top > 34.0 && (main || hot || hot_main) && !dragging {
                    let label = if main { format!("\u{2605} {}", tr("Main")) } else { tr("Make main").to_string() };
                    let lw = (g.measure(&label, &self.f.tiny) + 14.0).min(rr.right - rr.left - 6.0);
                    let lr = rf(rr.left + (rr.right - rr.left - lw) / 2.0, rr.bottom - 19.0, lw, 16.0);
                    if main {
                        self.surf.fill_rr(lr, 8.0, with_alpha(col, 0.9 * a));
                        self.surf.text(&label, &self.f.tiny_c, lr, Island::text_on(col, a));
                    } else {
                        self.surf.fill_rr(lr, 8.0, if hot_main { with_alpha(col, 0.95 * a) } else { self.fg(0.16 * a) });
                        self.surf.text(&label, &self.f.tiny_c, lr, if hot_main { Island::text_on(col, a) } else { self.fg(0.9 * a) });
                        if a > 0.5 {
                            self.hits.push((Hit::LinkMain(i), lr));
                        }
                    }
                }
            }
            // Where the pointer is.
            if let Some((p, over)) = v.pointer {
                // Glide towards the pointer and keep drawing while it moves
                // (smooth on both PCs, not just at the page's slow refresh).
                let shown = match self.link_dot {
                    Some(d) if !self.link_drag.is_some() => {
                        let k = 0.5;
                        let n = (d.0 + (p.0 - d.0) * k, d.1 + (p.1 - d.1) * k);
                        if (n.0 - p.0).abs() > 0.5 || (n.1 - p.1).abs() > 0.5 {
                            self.keep_anim = true;
                        } else if self.link_dot_target != Some(p) {
                            self.keep_anim = true;
                        }
                        // Jumped across the map (crossing): no slide.
                        if (p.0 - d.0).abs() + (p.1 - d.1).abs() > 2000.0 { p } else { n }
                    }
                    _ => p,
                };
                if self.link_dot_target != Some(p) {
                    self.keep_anim = true;
                }
                self.link_dot = Some(shown);
                self.link_dot_target = Some(p);
                let p = shown;
                let (px, py) = to(p);
                self.surf.fill_circle(px, py, 5.0, with_alpha(if over { them_c } else { me_c }, a));
                self.surf.fill_circle(px, py, 2.4, rgb(0xffffff, a));
            }
            self.surf.unclip();
        }
        // The map's header strip: what it is on the left, the Link on / off
        // switch on the right (off: no looking for other PCs at all).
        {
            let sw = rf(bx + mw - 48.0, by + (HEAD - 20.0) / 2.0, 38.0, 20.0);
            let col = self.ic(pal::INDIGO, 1.0);
            let hot = self.hot == Some(Hit::LinkOff);
            self.surf.text("\u{E7F4}", &self.f.icon_s, rf(bx + 12.0, by, 18.0, HEAD), self.fg(0.5 * a));
            self.surf.text(tr("Screens"), &self.f.tiny, rf(bx + 32.0, by, mw - 140.0, HEAD), self.fg(0.6 * a));
            // Identify: each screen's number on it (this PC's, and the linked one's).
            {
                let lab = tr("Identify");
                let iw = g.measure(tr("Screens"), &self.f.tiny);
                let w = g.measure(lab, &self.f.tiny) + 18.0;
                let r = rf(bx + 32.0 + iw + 10.0, by + (HEAD - 20.0) / 2.0, w, 20.0);
                let hot = self.hot == Some(Hit::LinkIdentify);
                self.surf.fill_rr(r, 10.0, self.fg(if hot { 0.16 } else { 0.08 } * a));
                self.surf.text(lab, &self.f.tiny_c, r, self.fg(0.85 * a));
                self.hits.push((Hit::LinkIdentify, r));
            }
            let lbl = tr("Link");
            let lw_ = g.measure(lbl, &self.f.tiny);
            self.surf.text(lbl, &self.f.tiny, rf(sw.left - lw_ - 8.0, by, lw_ + 4.0, HEAD), self.fg(0.75 * a));
            self.surf.fill_rr(sw, 10.0, with_alpha(col, if hot { 1.0 } else { 0.85 } * a));
            self.surf.fill_circle(sw.right - 10.0, sw.top + 10.0, 7.0, Island::text_on(col, a));
            self.surf.fill_rect(rf(bx + 10.0, by + HEAD, mw - 20.0, 1.0), self.fg(0.07 * a));
            self.hits.push((Hit::LinkOff, rf(sw.left - lw_ - 12.0, by, lw_ + 60.0, HEAD)));
        }
        // ---- Status (right) ----
        let px0 = bx + mw + 16.0;
        let pw = bx + bw - px0;
        let mut y = by + 2.0;
        let dot = |isl: &mut Island, y: f32, col: D2D1_COLOR_F| isl.surf.fill_circle(px0 + 5.0, y + 9.0, 4.0, col);
        let right = bx + bw;
        // Buttons in a row that wraps: (x, y) moves along.
        let flow = |isl: &mut Island, at: &mut (f32, f32), label: &str, hit: Hit, strong: bool| {
            let w = gfx::g().measure(label, &isl.f.tiny) + 22.0;
            if at.0 > px0 && at.0 + w > right {
                at.0 = px0;
                at.1 += 30.0;
            }
            let r = rf(at.0, at.1, w, 24.0);
            let accent = isl.ic(pal::INDIGO, 1.0);
            let hot = isl.hot == Some(hit);
            if strong {
                isl.surf.fill_rr(r, 12.0, with_alpha(accent, if hot { 1.0 } else { 0.85 } * a));
                isl.surf.text(label, &isl.f.tiny_c, r, Island::text_on(accent, a));
            } else {
                isl.surf.fill_rr(r, 12.0, isl.fg(if hot { 0.16 } else { 0.08 } * a));
                isl.surf.text(label, &isl.f.tiny_c, r, isl.fg(0.85 * a));
            }
            isl.hits.push((hit, r));
            at.0 += w + 6.0;
        };
        let button = |isl: &mut Island, x: f32, y: f32, label: &str, hit: Hit, strong: bool| -> f32 {
            let w = gfx::g().measure(label, &isl.f.tiny) + 22.0;
            let r = rf(x, y, w, 24.0);
            let accent = isl.ic(pal::INDIGO, 1.0);
            let hot = isl.hot == Some(hit);
            if strong {
                isl.surf.fill_rr(r, 12.0, with_alpha(accent, if hot { 1.0 } else { 0.85 } * a));
                isl.surf.text(label, &isl.f.tiny_c, r, Island::text_on(accent, a));
            } else {
                isl.surf.fill_rr(r, 12.0, isl.fg(if hot { 0.16 } else { 0.08 } * a));
                isl.surf.text(label, &isl.f.tiny_c, r, isl.fg(0.85 * a));
            }
            isl.hits.push((hit, r));
            w
        };
        // A screen picked on the map: its display settings here instead.
        let sel = self.link_sel.clone().and_then(|u| v.geo.screens.iter().position(|s| s.uid() == u));
        if sel.is_none() {
            self.link_sel = None;
        }
        if let Some(i) = sel {
            self.display_panel(&v, i, px0, y, pw, by + bh - y, a);
        } else {
        match (&v.pairing, &v.session) {
            (Pairing::Code { name, code, mine, .. }, _) => {
                self.surf.text(&format!("{} {name}", tr("Pair with")), &self.f.body, rf(px0, y, pw, 18.0), self.fg(0.9 * a));
                y += 22.0;
                let code = format!("{:03} {:03}", code / 1000, code % 1000);
                self.surf.text(&code, &self.f.big_c, rf(px0 - 4.0, y, pw, 38.0), with_alpha(self.ic(pal::INDIGO, 1.0), a));
                y += 42.0;
                match mine {
                    None => {
                        self.surf.text(tr("Same number on the other PC?"), &self.f.tiny, rf(px0, y, pw, 16.0), self.fg(0.55 * a));
                        y += 22.0;
                        let w = button(self, px0, y, tr("Codes match"), Hit::LinkDecide(true), true);
                        button(self, px0 + w + 6.0, y, tr("Cancel"), Hit::LinkDecide(false), false);
                    }
                    Some(_) => {
                        self.thinking_text(&format!("{} {name}…", tr("Waiting for")), rf(px0, y, pw, 18.0), a);
                    }
                }
            }
            (Pairing::Asking { name }, _) => {
                self.thinking_dots(px0 + 8.0, y + 10.0, 8.0, a);
                self.thinking_text(&format!("{} {name}…", tr("Asking")), rf(px0 + 20.0, y, pw - 20.0, 20.0), a);
            }
            (Pairing::Failed(e), _) => {
                self.surf.text(tr("Couldn't pair"), &self.f.body, rf(px0, y, pw, 18.0), rgb(pal::RED, a));
                y += 20.0;
                self.surf.text_wrap(e, &self.f.wrap_src, rf(px0, y, pw, 40.0), self.fg(0.6 * a));
                y += 42.0;
                button(self, px0, y, "OK", Hit::LinkDismiss, false);
            }
            (_, Some((name, delay))) => {
                dot(self, y, self.ic(pal::GREEN, a));
                self.surf.text(&format!("{} {name}", tr("Linked with")), &self.f.body, rf(px0 + 14.0, y, pw - 14.0, 18.0), self.fg(0.92 * a));
                y += 22.0;
                let d = match delay {
                    Some((last, worst)) => format!("{} {:.1} ms · {} {:.1} ms", tr("Delay"), last, tr("worst"), worst),
                    None => tr("Measuring the delay…").to_string(),
                };
                self.surf.text(&d, &self.f.tiny, rf(px0 + 14.0, y, pw - 14.0, 16.0), self.fg(0.6 * a));
                y += 24.0;
                let id = v.paired.iter().find(|p| &p.1 == name).map(|p| p.0).unwrap_or(0);
                let mut at = (px0, y);
                flow(self, &mut at, &format!("{} {name}", tr("Go to")), Hit::LinkJump, true);
                flow(self, &mut at, tr("Disconnect"), Hit::LinkDisconnect, false);
                flow(self, &mut at, tr("Identify"), Hit::LinkIdentify, false);
                flow(self, &mut at, tr("Unpair"), Hit::LinkUnpair(id), false);
                y = at.1 + 32.0;
                // Copy & paste between the PCs.
                if let Some(x) = v.xfer.clone() {
                    self.xfer_bar(&x, px0, y, pw - 4.0, a);
                } else if let Some(m) = &v.xfer_msg {
                    self.surf.text_wrap(m, &self.f.wrap_src, rf(px0, y, pw, 32.0), self.fg(0.6 * a));
                } else {
                    self.surf.text_wrap(tr("Copy here, paste there: text, pictures and files."), &self.f.wrap_src, rf(px0, y, pw, 32.0), self.fg(0.4 * a));
                }
            }
            _ if !v.paired.is_empty() => {
                let (id, name) = v.paired[0].clone();
                dot(self, y, self.ic(pal::ORANGE, a));
                self.surf.text(&format!("{name}"), &self.f.body, rf(px0 + 14.0, y, pw - 14.0, 18.0), self.fg(0.9 * a));
                y += 22.0;
                if v.hold || v.quiet {
                    let why = if v.hold { tr("Disconnected") } else { tr("It disconnected · Connect links again") };
                    self.surf.text(why, &self.f.tiny, rf(px0 + 14.0, y, pw - 14.0, 16.0), self.fg(0.55 * a));
                } else {
                    self.thinking_text(tr("Waiting for it on the network…"), rf(px0 + 14.0, y, pw - 14.0, 16.0), a * 0.8);
                }
                y += 24.0;
                let mut at = (px0, y);
                flow(self, &mut at, tr("Connect"), Hit::LinkConnect, true);
                flow(self, &mut at, tr("Identify"), Hit::LinkIdentify, false);
                if v.peer_ip.is_some() {
                    }
                flow(self, &mut at, tr("Unpair"), Hit::LinkUnpair(id), false);
            }
            _ => {
                self.surf.text(tr("PCs on your network"), &self.f.label, rf(px0, y, pw, 16.0), self.fg(0.5 * a));
                y += 22.0;
                if v.nearby.is_empty() {
                    self.thinking_text(tr("Looking…"), rf(px0, y, pw, 18.0), a);
                    y += 20.0;
                    self.surf.text_wrap(tr("Open Phemio on the other PC, on the same network."), &self.f.wrap_src, rf(px0, y, pw, 40.0), self.fg(0.5 * a));
                } else {
                    for (id, name) in v.nearby.iter().take(3) {
                        dot(self, y + 2.0, with_alpha(them_c, a));
                        let nw = (pw - 70.0).max(40.0);
                        let shown = ellipsize(g, name, &self.f.body, nw - 14.0);
                        self.surf.text(&shown, &self.f.body, rf(px0 + 14.0, y + 2.0, nw - 14.0, 20.0), self.fg(0.9 * a));
                        button(self, px0 + nw, y, tr("Pair"), Hit::LinkPair(*id), true);
                        y += 30.0;
                    }
                }
            }
        }
        }
        // Connect by address (when finding each other doesn't work).
        let manual = sel.is_none() && v.session.is_none() && matches!(v.pairing, Pairing::None | Pairing::Failed(_));
        let mut low = by + bh - 34.0;
        if manual {
            let fy = by + bh - 66.0;
            let go_w = g.measure(tr("Connect"), &self.f.tiny) + 22.0;
            let fr = rf(px0, fy, (pw - go_w - 6.0).max(60.0), 28.0);
            let ph = if v.manual.is_empty() { tr("Other PC's IP address").to_string() } else { v.manual.clone() };
            self.draw_field(fr, EditTarget::LinkAddr, &ph, a, Hit::LinkAddr);
            let gr = rf(fr.right + 6.0, fy + 2.0, go_w, 24.0);
            let accent = self.ic(pal::INDIGO, 1.0);
            self.surf.fill_rr(gr, 12.0, with_alpha(accent, if self.hot == Some(Hit::LinkAddrGo) { 1.0 } else { 0.85 } * a));
            self.surf.text(tr("Connect"), &self.f.tiny_c, gr, Island::text_on(accent, a));
            self.hits.push((Hit::LinkAddrGo, gr));
            low = fy - 2.0;
        }
        if !v.problem.is_empty() {
            self.surf.text_wrap(&v.problem, &self.f.wrap_src, rf(px0, low - 30.0, pw, 30.0), rgb(pal::RED, 0.85 * a));
        } else if manual && v.session.is_none() {
            // No firewall rule for this Phemio.exe (new name or folder): other
            // PCs' broadcasts don't get in, so finding each other is slow.
            use crate::link::firewall::{busy, state, Fw};
            let fw = state();
            if matches!(fw, Fw::Missing | Fw::Blocked) {
                let t = if busy() { tr("Asking Windows…") } else { tr("Windows Firewall is slowing down finding other PCs") };
                let go = tr("Allow");
                let go_w = g.measure(go, &self.f.tiny) + 20.0;
                let ty = low - 26.0;
                let tw = (pw - go_w - 8.0).max(40.0);
                self.surf.text(&ellipsize(g, t, &self.f.tiny, tw), &self.f.tiny, rf(px0, ty + 3.0, tw, 16.0), rgb(pal::ORANGE, 0.9 * a));
                if !busy() {
                    let gr = rf(px0 + pw - go_w, ty, go_w, 22.0);
                    let hot = self.hot == Some(Hit::LinkFirewall);
                    self.surf.fill_rr(gr, 11.0, rgb(pal::ORANGE, if hot { 0.95 } else { 0.8 } * a));
                    self.surf.text(go, &self.f.tiny_c, gr, Island::text_on(rgb(pal::ORANGE, 1.0), a));
                    self.hits.push((Hit::LinkFirewall, gr));
                }
            }
        }
        // Legend and tips at the bottom (not under a screen's settings: they
        // need the room).
        if sel.is_some() {
            return;
        }
        let ly = by + bh - 30.0;
        self.surf.fill_rr(rf(px0, ly + 4.0, 9.0, 9.0), 2.5, with_alpha(me_c, 0.8 * a));
        let me_w = g.measure(&v.name, &self.f.tiny).min(pw * 0.45);
        self.surf.text(&ellipsize(g, &v.name, &self.f.tiny, me_w), &self.f.tiny, rf(px0 + 13.0, ly, me_w + 4.0, 16.0), self.fg(0.55 * a));
        if let Some((name, _)) = &v.session {
            let x2 = px0 + 13.0 + me_w + 12.0;
            self.surf.fill_rr(rf(x2, ly + 4.0, 9.0, 9.0), 2.5, with_alpha(them_c, 0.8 * a));
            let w2 = (bx + bw - x2 - 14.0).max(10.0);
            self.surf.text(&ellipsize(g, name, &self.f.tiny, w2), &self.f.tiny, rf(x2 + 13.0, ly, w2, 16.0), self.fg(0.55 * a));
        }
        self.surf.text(tr("Drag screens to arrange them · click one for its settings"), &self.f.tiny, rf(px0, ly + 15.0, pw + 8.0, 15.0), self.fg(0.38 * a));
    }
}

impl Island {
    /// A screen's display settings (Settings → Display's, for that screen):
    /// main display, scale, resolution, orientation; with a "keep these?"
    /// after a resolution or orientation change.
    fn display_panel(&mut self, v: &link::View, i: usize, x: f32, y: f32, w: f32, h: f32, a: f32) {
        let g = gfx::g();
        let Some(s) = v.geo.screens.get(i).cloned() else { return };
        let mine = s.machine == v.me;
        let accent = self.ic(pal::INDIGO, 1.0);
        let num = v.geo.numbers().get(i).copied().unwrap_or(i + 1);
        // Title: its number and name, and a close.
        let cr = rf(x + w - 22.0, y, 22.0, 22.0);
        self.surf.fill_rr(cr, 11.0, self.fg(if self.hot == Some(Hit::DispClose) { 0.16 } else { 0.07 } * a));
        self.surf.text("\u{E711}", &self.f.icon_s, cr, self.fg(0.7 * a));
        self.hits.push((Hit::DispClose, cr));
        self.surf.text(&format!("{num} · {}", s.label), &self.f.body, rf(x, y, w - 28.0, 22.0), self.fg(0.92 * a));
        let mut yy = y + 26.0;
        let row_h = 28.0;
        let lab_w = (w * 0.33).min(70.0);
        let main = s.px.0 == 0 && s.px.1 == 0;
        // Main display.
        if main {
            self.surf.text(&format!("\u{2605} {}", tr("Main display")), &self.f.tiny, rf(x, yy, w - 4.0, 22.0), with_alpha(accent, a));
        } else {
            let lab = tr("Make this my main display");
            let bw = (g.measure(lab, &self.f.tiny) + 22.0).min(w);
            let r = rf(x, yy, bw, 22.0);
            let hot = self.hot == Some(Hit::DispMain);
            self.surf.fill_rr(r, 11.0, with_alpha(accent, if hot { 1.0 } else { 0.85 } * a));
            self.surf.text(lab, &self.f.tiny_c, r, Island::text_on(accent, a));
            self.hits.push((Hit::DispMain, r));
        }
        yy += row_h;
        // What the display is set to: this PC's asked here every couple of
        // seconds; the other PC's asked over the link.
        let (inf, pending) = if mine {
            let fresh = self.disp.as_ref().map(|(k, _, t)| *k == s.key && t.elapsed().as_secs_f32() < 2.0).unwrap_or(false);
            if !fresh {
                match crate::display::info(&s.key) {
                    Some(inf) => self.disp = Some((s.key.clone(), inf, Instant::now())),
                    None => {
                        self.surf.text(tr("Windows didn't say this display's settings"), &self.f.tiny, rf(x, yy, w, 18.0), self.fg(0.5 * a));
                        return;
                    }
                }
            }
            let Some((_, inf, _)) = self.disp.clone() else { return };
            (inf, crate::display::pending().filter(|p| p.0 == s.key).map(|p| p.1))
        } else {
            match link::remote_display(&s.key) {
                Some((inf, left)) => (inf, (left > 0).then_some(left)),
                None => {
                    self.thinking_text(tr("Asking the other PC…"), rf(x, yy, w, 18.0), a);
                    return;
                }
            }
        };
        // (✓: what Windows recommends.)
        let rec = |on: bool| if on { " \u{2713}".to_string() } else { String::new() };
        let rows: [(u8, &str, String); 3] = [
            (0, "Scale", format!("{}%{}", inf.scale, rec(inf.scale == inf.scale_rec))),
            (1, "Resolution", format!("{}×{}{}", inf.res.0, inf.res.1, rec(inf.res == inf.res_rec))),
            (2, "Orientation", tr(crate::display::ORIENTS[(inf.orient as usize).min(3)]).to_string()),
        ];
        for (f, label, value) in rows {
            self.surf.text(tr(label), &self.f.tiny, rf(x, yy, lab_w, 24.0), self.fg(0.55 * a));
            // ‹ value ›
            let sx = x + lab_w;
            let sw = w - lab_w;
            let box_ = rf(sx, yy, sw, 24.0);
            self.surf.fill_rr(box_, 12.0, self.fg(0.07 * a));
            for (next, glyph, bx) in [(false, "\u{E76B}", box_.left), (true, "\u{E76C}", box_.right - 20.0)] {
                let r = rf(bx, yy, 20.0, 24.0);
                let hot = self.hot == Some(Hit::DispStep(f, next));
                if hot {
                    self.surf.fill_rr(r, 12.0, with_alpha(accent, 0.25 * a));
                }
                self.surf.text(glyph, &self.f.icon_s, r, self.fg(0.8 * a));
                self.hits.push((Hit::DispStep(f, next), r));
            }
            self.surf.text(&value, &self.f.tiny_c, rf(box_.left + 20.0, yy, sw - 40.0, 24.0), self.fg(0.92 * a));
            yy += row_h;
        }
        // Kept, or back in a few seconds.
        if let Some(left) = pending {
            {
                self.surf.text(&format!("{} · {} {left} s", tr("Keep these display settings?"), tr("Going back in")), &self.f.tiny, rf(x, yy, w, 18.0), with_alpha(self.ic(pal::ORANGE, 1.0), a));
                yy += 19.0;
                let lab = tr("Keep changes");
                let kw = g.measure(lab, &self.f.tiny) + 22.0;
                let r = rf(x, yy, kw, 22.0);
                self.surf.fill_rr(r, 11.0, with_alpha(accent, if self.hot == Some(Hit::DispKeep) { 1.0 } else { 0.85 } * a));
                self.surf.text(lab, &self.f.tiny_c, r, Island::text_on(accent, a));
                self.hits.push((Hit::DispKeep, r));
                let lab2 = tr("Revert");
                let rw = g.measure(lab2, &self.f.tiny) + 22.0;
                let r2 = rf(r.right + 6.0, yy, rw, 22.0);
                self.surf.fill_rr(r2, 11.0, self.fg(if self.hot == Some(Hit::DispRevert) { 0.16 } else { 0.08 } * a));
                self.surf.text(lab2, &self.f.tiny_c, r2, self.fg(0.85 * a));
                self.hits.push((Hit::DispRevert, r2));
                yy += 26.0;
            }
        }
        if let Some((m, t)) = &self.disp_msg {
            if t.elapsed().as_secs() < 8 && yy + 16.0 <= y + h + 20.0 {
                self.surf.text_wrap(m, &self.f.wrap_src, rf(x, yy, w, 32.0), rgb(pal::RED, 0.85 * a));
            }
        }
    }

    /// The display panel's buttons.
    pub(super) fn display_click(&mut self, h: Hit) {
        let Some(v) = link::view() else { return };
        let Some(s) = self.link_sel.clone().and_then(|u| v.geo.screens.iter().find(|s| s.uid() == u).cloned()) else { return };
        let res: std::result::Result<(), String> = match h {
            Hit::DispClose => {
                self.link_sel = None;
                Ok(())
            }
            Hit::DispMain => {
                if s.machine == v.me {
                    link::set_main_local(&s.key)
                } else {
                    link::set_main(s.machine, &s.key);
                    Ok(())
                }
            }
            Hit::DispKeep if s.machine != v.me => {
                link::remote_display_set(&s.key, 3, 0, 0, 0);
                Ok(())
            }
            Hit::DispRevert if s.machine != v.me => {
                link::remote_display_set(&s.key, 4, 0, 0, 0);
                Ok(())
            }
            Hit::DispKeep => {
                crate::display::keep();
                Ok(())
            }
            Hit::DispRevert => crate::display::revert(),
            Hit::DispStep(f, next) => {
                let remote = s.machine != v.me;
                let inf = if remote {
                    link::remote_display(&s.key).map(|r| r.0)
                } else {
                    self.disp.as_ref().filter(|d| d.0 == s.key).map(|d| d.1.clone()).or_else(|| crate::display::info(&s.key))
                };
                // This PC's display: changed here; the other PC's: asked over the link.
                let set_scale = |pct: u32| -> std::result::Result<(), String> {
                    if remote {
                        link::remote_display_set(&s.key, 1, pct as i32, 0, 0);
                        Ok(())
                    } else {
                        crate::display::set_scale(&s.key, pct)
                    }
                };
                let set_mode = |res: (u32, u32), o: u32| -> std::result::Result<(), String> {
                    if remote {
                        link::remote_display_set(&s.key, 2, res.0 as i32, res.1 as i32, o as i32);
                        Ok(())
                    } else {
                        crate::display::set_mode(&s.key, res, o)
                    }
                };
                match inf {
                    None => Err("display not found".into()),
                    Some(inf) => {
                        let step = |n: usize, at: usize| if next { (at + 1).min(n.saturating_sub(1)) } else { at.saturating_sub(1) };
                        match f {
                            0 => {
                                let at = inf.scales.iter().position(|x| *x == inf.scale).unwrap_or(0);
                                let to = inf.scales[step(inf.scales.len(), at)];
                                if to != inf.scale { set_scale(to) } else { Ok(()) }
                            }
                            1 => {
                                // (Modes run largest first: "next" goes smaller.)
                                let at = inf.modes.iter().position(|x| *x == inf.res).unwrap_or(0);
                                let to = inf.modes[step(inf.modes.len(), at)];
                                if to != inf.res { set_mode(to, inf.orient) } else { Ok(()) }
                            }
                            _ => {
                                let to = if next { (inf.orient + 1) % 4 } else { (inf.orient + 3) % 4 };
                                set_mode(inf.res, to)
                            }
                        }
                    }
                }
            }
            _ => Ok(()),
        };
        self.disp = None;
        match res {
            Ok(()) => self.disp_msg = None,
            Err(e) => {
                crate::util::log(&format!("display: {e}"));
                self.disp_msg = Some((e, Instant::now()));
            }
        }
        self.render();
    }
}

/// "Receiving big.bin · 45% · 82 MB/s · 9 s left" and a bar under it.
impl Island {
    pub(super) fn xfer_bar(&mut self, x: &crate::link::Xfer, px: f32, y: f32, w: f32, a: f32) {
        let g = gfx::g();
        let frac = if x.total > 0 { (x.done as f32 / x.total as f32).clamp(0.0, 1.0) } else { 0.0 };
        let secs = x.started.elapsed().as_secs_f32().max(0.2);
        let bps = x.done as f32 / secs;
        let speed = if bps >= 1_048_576.0 { format!("{:.0} MB/s", bps / 1_048_576.0) } else { format!("{:.0} KB/s", bps / 1024.0) };
        let left = if bps > 1.0 { (x.total.saturating_sub(x.done)) as f32 / bps } else { 0.0 };
        let eta = if left >= 3600.0 {
            format!("{}h {:02}m", (left / 3600.0) as u32, ((left % 3600.0) / 60.0) as u32)
        } else if left >= 60.0 {
            format!("{}m {:02}s", (left / 60.0) as u32, (left % 60.0) as u32)
        } else {
            format!("{:.0}s", left.ceil())
        };
        let head = format!("{} {} · {:.0}% · {speed} · {eta} {}", tr(if x.out { "Sending" } else { "Receiving" }), x.what, frac * 100.0, tr("left"));
        self.surf.text(&ellipsize(g, &head, &self.f.tiny, w), &self.f.tiny, rf(px, y, w, 16.0), self.fg(0.75 * a));
        let bar = rf(px, y + 18.0, w, 4.0);
        self.surf.fill_rr(bar, 2.0, self.fg(0.1 * a));
        self.surf.fill_rr(rf(bar.left, bar.top, ((bar.right - bar.left) * frac).max(4.0), 4.0), 2.0, with_alpha(self.el(pal::INDIGO, 1.0), a));
    }
}

fn ellipsize(g: &gfx::Gfx, s: &str, f: &IDWriteTextFormat, w: f32) -> String {
    if g.measure(s, f) <= w {
        return s.to_string();
    }
    let mut t: String = s.to_string();
    while !t.is_empty() && g.measure(&format!("{t}…"), f) > w {
        t.pop();
    }
    format!("{t}…")
}
