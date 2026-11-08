//! Expanded pages, the mascot and small drawing helpers.

use windows::Win32::Graphics::Direct2D::Common::*;

use super::*;
use crate::tools::Phase;
use crate::weather;

/// Palette colour per kind of device.
fn kind_color(k: crate::bt::Kind) -> u32 {
    use crate::bt::Kind::*;
    match k {
        Headphones | Headset => pal::PINK,
        Speaker => pal::ORANGE,
        Mouse => pal::CYAN,
        Keyboard => pal::INDIGO,
        Gamepad => pal::GREEN,
        Pen => pal::YELLOW,
        Phone => pal::MINT,
        Computer => pal::TEAL,
        Watch => pal::PURPLE,
        Other => pal::BLUE,
    }
}

impl Island {
    // ------------------------------------------------------------------
    // Shared bits
    // ------------------------------------------------------------------

    /// A text field (to-do input, location, city). Shows a caret while editing.
    pub(super) fn draw_field(&mut self, r: D2D_RECT_F, target: EditTarget, placeholder: &str, a: f32, hit: Hit) {
        let editing = self.edit == Some(target);
        self.surf.fill_rr(r, 10.0, self.fg(if editing { 0.12 } else { 0.07 } * a));
        if editing {
            self.surf.stroke_rr(r, 10.0, with_alpha(self.accent, 0.7 * a), 1.2);
        }
        let tr = rf(r.left + 12.0, r.top, r.right - r.left - 24.0, r.bottom - r.top);
        if editing && !self.edit_buf.is_empty() {
            let masked = matches!(target, EditTarget::WifiPass | EditTarget::RdpPass).then(|| "\u{2022}".repeat(self.edit_buf.chars().count()));
            let (font, acc) = (self.f.body.clone(), self.accent);
            self.draw_edit_line(&font, rf(tr.left, tr.top, tr.right - tr.left - 4.0, tr.bottom - tr.top), self.fg(a), with_alpha(acc, a.max(0.0)), 16.0, masked.as_deref(), a);
        } else {
            if editing && self.caret {
                let cy = (r.top + r.bottom) / 2.0;
                self.surf.fill_rect(rf(tr.left, cy - 8.0, 1.5, 16.0), self.fg(a));
            }
            self.surf.text(placeholder, &self.f.body, rf(tr.left + if editing { 4.0 } else { 0.0 }, tr.top, tr.right - tr.left, tr.bottom - tr.top), self.fg(0.4 * a));
        }
        if !editing {
            self.hits.push((hit, r));
        }
    }

    fn button(&mut self, r: D2D_RECT_F, label: &str, glyph: &str, primary: bool, col: D2D1_COLOR_F, hit: Hit, a: f32) {
        let hot = self.hot == Some(hit);
        let rad = (r.bottom - r.top) / 2.0;
        if primary {
            self.surf.fill_rr(r, rad, with_alpha(col, if hot { 1.0 } else { 0.88 } * a));
        } else {
            self.surf.fill_rr(r, rad, self.fg(if hot { 0.16 } else { 0.09 } * a));
        }
        let fg = if primary { rgb(0x000000, 0.92 * a) } else { self.fg(0.92 * a) };
        if label.is_empty() {
            self.surf.text(glyph, &self.f.icon, r, fg);
        } else {
            let gw = if glyph.is_empty() { 0.0 } else { 20.0 };
            let tw = gfx::g().measure(label, &self.f.body_c) + gw;
            let sx = (r.left + r.right) / 2.0 - tw / 2.0;
            if !glyph.is_empty() {
                self.surf.text(glyph, &self.f.icon, rf(sx, r.top, 16.0, r.bottom - r.top), fg);
            }
            self.surf.text(label, &self.f.body_c, rf(sx + gw, r.top, tw - gw + 2.0, r.bottom - r.top), fg);
        }
        self.hits.push((hit, r));
    }

    /// Largest clock size that fits the width.
    fn clock_font(&self, t: &str, width: f32, max: f32) -> (f32, IDWriteTextFormat) {
        let g = gfx::g();
        self.f
            .clock
            .iter()
            .filter(|(s, _)| *s <= max)
            .find(|(_, f)| g.measure(t, f) <= width - 2.0)
            .or(self.f.clock.last())
            .map(|(s, f)| (*s, f.clone()))
            .unwrap()
    }

    // ------------------------------------------------------------------
    // Home: media + clock
    // ------------------------------------------------------------------

    /// Now Playing: a big cover on the left; on the right the title and
    /// artist, the progress bar, large controls (play in a filled circle in
    /// the cover's colour), and at the bottom where the sound goes plus a
    /// volume slider. A soft glow of the cover's colour sits behind it all.
    pub(super) fn page_home(&mut self, bx: f32, by: f32, bw: f32, bh: f32, a: f32) {
        let has = self.media.has_session && !self.media.title.is_empty();
        let ak = config::cur().i("cover_size").clamp(50, 125) as f32 / 100.0;
        let asz = (bh - 8.0).min(186.0 * ak).round().max(60.0);
        let art = rf(bx, by + (bh - asz) / 2.0, asz, asz);
        let accent = if has { self.accent } else { self.fg(1.0) };
        // The cover's colour, faintly, behind the right side.
        if has {
            self.surf.fill_rr(rf(art.right + 8.0, by, bx + bw - art.right - 8.0, bh), 16.0, with_alpha(accent, 0.07 * a));
        }
        self.draw_art(art, 18.0 * ak.max(0.8), a);
        let can_open = has && !self.media.app_id.is_empty();
        if can_open {
            // The cover opens the player too: dimmed with an arrow on hover.
            if self.hot == Some(Hit::OpenPlayer(1)) {
                self.surf.fill_rr(art, 18.0 * ak.max(0.8), rgb(0x000000, 0.35 * a));
                self.surf.fill_circle((art.left + art.right) / 2.0, (art.top + art.bottom) / 2.0, 20.0, rgb(0xffffff, 0.9 * a));
                self.surf.text("\u{E8A7}", &self.f.icon_m, rf((art.left + art.right) / 2.0 - 20.0, (art.top + art.bottom) / 2.0 - 20.0, 40.0, 40.0), rgb(0x111114, a));
            }
            self.hits.push((Hit::OpenPlayer(1), art));
        }
        let tx = art.right + 22.0;
        let tw = bx + bw - tx - 12.0;
        // Top right: open the player ("↗ Spotify").
        let ow = if can_open { self.open_player_button(tx + tw, by + 8.0, (tw * 0.42).min(170.0), a) + 10.0 } else { 0.0 };
        let (title, artist) = if has { (self.media.title.clone(), self.media.artist.clone()) } else { (tr("Not playing").into(), tr("Play something in any app").into()) };
        self.surf.text(&title, &self.f.title, rf(tx, by + 10.0, tw - ow, 24.0), self.fg(a));
        self.surf.text(&artist, &self.f.artist, rf(tx, by + 36.0, tw - ow, 20.0), self.fg(0.55 * a));
        self.progress_row(tx, by + 72.0, tw, a);
        // Big controls in the middle of the text column.
        self.big_controls(tx + tw / 2.0, by + 122.0, accent, a);
        // Bottom: where the sound goes, and the volume.
        let yb = by + bh - 22.0;
        let cw = self.output_chip(tx, yb - 15.0, tw * 0.46, a);
        self.player_volume(tx + cw + 14.0, yb, tw - cw - 14.0, a);
        self.cluster_popover(rf(tx, by, tw, bh), a);
    }

    /// "↗ Spotify": brings the playing app to the front. Returns its width.
    fn open_player_button(&mut self, right: f32, top: f32, maxw: f32, a: f32) -> f32 {
        let name = crate::player::name(&self.media.app_id).unwrap_or_else(|| tr("Open player").to_string());
        let g = gfx::g();
        let w = (g.measure(&name, &self.f.tiny) + 40.0).min(maxw).max(40.0);
        let r = rf(right - w, top, w, 28.0);
        let hot = self.hot == Some(Hit::OpenPlayer(0));
        self.surf.fill_rr(r, 14.0, self.fg(if hot { 0.16 } else { 0.08 } * a));
        self.surf.text("\u{E8A7}", &self.f.icon_s, rf(r.left + 10.0, top, 14.0, 28.0), self.fg(0.85 * a));
        self.surf.text(&ellip(g, &name, &self.f.tiny, w - 36.0), &self.f.tiny, rf(r.left + 28.0, top, w - 32.0, 28.0), self.fg(0.85 * a));
        self.hits.push((Hit::OpenPlayer(0), r));
        w
    }

    /// Previous / play-pause / next as filled round buttons (solid glyphs
    /// inside), and a repeat button beside them — outlined when off, filled
    /// when on, with a "1" when it repeats one song.
    fn big_controls(&mut self, cx: f32, cy: f32, _accent: D2D1_COLOR_F, a: f32) {
        let has = self.media.has_session;
        let ca = if has { a } else { a * 0.5 };
        // In the colours chosen in Settings → Island (colour theme / icon
        // colours: the theme's, the album's, or plain).
        let fill = self.el(pal::CYAN, 1.0);
        let ink = Island::text_on(fill, ca);
        let items = [(Hit::Prev, cx - 64.0, 19.0), (Hit::Play, cx, 25.0), (Hit::Next, cx + 64.0, 19.0)];
        for (b, x, r) in items {
            let hot = self.hot == Some(b) && has;
            let r2 = r + if hot { 1.5 } else { 0.0 };
            self.surf.fill_circle(x, cy, r2, with_alpha(fill, if hot { 1.0 } else { 0.92 } * ca));
            let k = r / 20.0;
            match b {
                Hit::Play if self.media.playing => {
                    // Pause: two bars.
                    self.surf.fill_rr(rf(x - 7.0 * k, cy - 8.5 * k, 5.0 * k, 17.0 * k), 1.5 * k, ink);
                    self.surf.fill_rr(rf(x + 2.0 * k, cy - 8.5 * k, 5.0 * k, 17.0 * k), 1.5 * k, ink);
                }
                Hit::Play => {
                    // Play: a solid triangle, nudged right to look centred.
                    self.surf.fill_poly(&[(x - 5.5 * k, cy - 9.5 * k), (x - 5.5 * k, cy + 9.5 * k), (x + 9.5 * k, cy)], ink);
                }
                Hit::Prev => {
                    self.surf.fill_rr(rf(x - 8.0 * k, cy - 7.5 * k, 3.2 * k, 15.0 * k), 1.0 * k, ink);
                    self.surf.fill_poly(&[(x + 7.5 * k, cy - 7.5 * k), (x + 7.5 * k, cy + 7.5 * k), (x - 4.5 * k, cy)], ink);
                }
                _ => {
                    self.surf.fill_rr(rf(x + 4.8 * k, cy - 7.5 * k, 3.2 * k, 15.0 * k), 1.0 * k, ink);
                    self.surf.fill_poly(&[(x - 7.5 * k, cy - 7.5 * k), (x - 7.5 * k, cy + 7.5 * k), (x + 4.5 * k, cy)], ink);
                }
            }
            if has {
                self.hits.push((b, rf(x - r, cy - r, r * 2.0, r * 2.0)));
            }
        }
        // Far left: stop after this song (outlined: the next one plays on;
        // filled: it stops when this one ends).
        let (sx, sr) = (cx - 118.0, 16.0);
        let stop = crate::media::stop_after();
        let hot = self.hot == Some(Hit::StopAfter) && has;
        let sr2 = sr + if hot { 1.0 } else { 0.0 };
        let sk = sr / 16.0;
        let col = if stop { ink } else { with_alpha(fill, 0.95 * ca) };
        if stop {
            self.surf.fill_circle(sx, cy, sr2, with_alpha(fill, 0.95 * ca));
        } else {
            self.surf.stroke_rr(rf(sx - sr2 + 0.75, cy - sr2 + 0.75, sr2 * 2.0 - 1.5, sr2 * 2.0 - 1.5), sr2, with_alpha(fill, 0.9 * ca), 1.5);
        }
        // ■ in a circle: stop when this song ends.
        self.surf.fill_rr(rf(sx - 5.5 * sk, cy - 5.5 * sk, 11.0 * sk, 11.0 * sk), 1.5 * sk, col);
        if has {
            self.hits.push((Hit::StopAfter, rf(sx - sr, cy - sr, sr * 2.0, sr * 2.0)));
        }
        // Repeat: off (outline) · the whole list · this one song ("1").
        let (rx, rr) = (cx + 118.0, 16.0);
        let mode = self.media.repeat;
        let hot = self.hot == Some(Hit::Repeat) && has;
        let rr2 = rr + if hot { 1.0 } else { 0.0 };
        let glyph = if mode == 1 { "\u{E8ED}" } else { "\u{E8EE}" };
        let gr = rf(rx - rr, cy - rr, rr * 2.0, rr * 2.0);
        if mode == 0 {
            self.surf.stroke_rr(rf(rx - rr2 + 0.75, cy - rr2 + 0.75, rr2 * 2.0 - 1.5, rr2 * 2.0 - 1.5), rr2, with_alpha(fill, 0.9 * ca), 1.5);
            self.surf.text(glyph, &self.f.icon, gr, with_alpha(fill, 0.95 * ca));
        } else {
            self.surf.fill_circle(rx, cy, rr2, with_alpha(fill, 0.95 * ca));
            self.surf.text(glyph, &self.f.icon, gr, ink);
        }
        if has {
            self.hits.push((Hit::Repeat, gr));
        }
    }

    /// The volume as a slim slider with its icon (click it to mute) and number.
    fn player_volume(&mut self, x: f32, vy: f32, w: f32, a: f32) {
        if w < 80.0 {
            return;
        }
        let (vol, muted) = self.shown_volume();
        let mr = rf(x, vy - 16.0, 32.0, 32.0);
        if self.hot == Some(Hit::Mute) {
            self.surf.fill_circle(x + 16.0, vy, 16.0, self.fg(0.1 * a));
        }
        self.speaker(x + 16.0, vy, 18.0, speaker_level(vol, muted), self.fg(0.8 * a));
        self.hits.push((Hit::Mute, mr));
        let pct = if muted { tr("Muted").to_string() } else { format!("{}%", (vol * 100.0).round() as i32) };
        let pw = gfx::g().measure("100%", &self.f.body).max(gfx::g().measure(&pct, &self.f.body)) + 6.0;
        let res = if self.mo_cluster.is_empty() { 0.0 } else { Island::CHIP_W + 6.0 };
        let (sx, sw) = (x + 42.0, w - 42.0 - pw - 10.0 - res);
        self.cluster_chip(x + w, vy, a);
        let f = if muted { 0.0 } else { vol };
        self.surf.fill_rr(rf(sx, vy - 3.0, sw, 6.0), 3.0, self.fg(0.16 * a));
        self.surf.fill_rr(rf(sx, vy - 3.0, (sw * f).max(6.0), 6.0), 3.0, self.fg(0.85 * a));
        self.surf.fill_circle(sx + sw * f, vy, if self.hot == Some(Hit::Volume) || self.vol_drag { 9.0 } else { 7.5 }, rgb(0xffffff, a));
        self.hits.push((Hit::Volume, rf(sx - 10.0, vy - 14.0, sw + 20.0, 28.0)));
        self.surf.text(&pct, &self.f.body, rf(sx + sw + 10.0, vy - 11.0, pw + 4.0, 22.0), self.fg(0.75 * a));
    }

    /// A speaker drawn in lines: the body, then 0–3 sound waves (level 0 =
    /// muted, with a cross). Whatever is showing is centred on (cx, cy), so
    /// it sits in the middle of its button at every volume.
    pub(super) fn speaker(&self, cx: f32, cy: f32, size: f32, level: u8, col: D2D1_COLOR_F) {
        let u = size / 20.0;
        let sw = 1.7 * u.max(0.8);
        let waves = level.min(3) as usize;
        // How wide the drawing really is: the body reaches 7.4, the outermost
        // wave 5 + its radius, the mute cross 17.
        let ink: f32 = if level == 0 { 17.0 } else { (8.0 + (waves as f32 - 1.0) * 3.6).max(7.4) };
        let x0 = cx - ink * u / 2.0;
        let p = |x: f32, y: f32| (x0 + x * u, cy + y * u);
        // Body: a small box and the cone opening to the right.
        let body = [p(0.0, -2.6), p(2.8, -2.6), p(7.4, -6.6), p(7.4, 6.6), p(2.8, 2.6), p(0.0, 2.6)];
        self.surf.fill_poly(&body, with_alpha(col, col.a * 0.25));
        self.surf.stroke_poly(&body, true, col, sw);
        if level == 0 {
            let (a0, b0) = p(10.0, -3.6);
            let (a1, b1) = p(17.0, 3.6);
            self.surf.line(a0, b0, a1, b1, col, sw);
            let (a0, b0) = p(10.0, 3.6);
            let (a1, b1) = p(17.0, -3.6);
            self.surf.line(a0, b0, a1, b1, col, sw);
            return;
        }
        for i in 0..waves {
            let r = 3.0 + i as f32 * 3.6;
            let pts: Vec<_> = (0..=10)
                .map(|k| {
                    let ang = (-48.0 + 96.0 * k as f32 / 10.0f32).to_radians();
                    p(5.0 + r * ang.cos(), r * ang.sin() * 1.05)
                })
                .collect();
            self.surf.stroke_poly(&pts, false, col, sw);
        }
    }

    /// "🎧 Bose Flex 2 SoundLink · 70%" — where the sound is going.
    fn output_chip(&mut self, x: f32, y: f32, maxw: f32, a: f32) -> f32 {
        let d = crate::data();
        if d.output.is_empty() {
            return 0.0;
        }
        let btd = d.output_bt().cloned();
        let name = crate::bt::inner(&d.output).to_string();
        let label = match btd.as_ref().and_then(|b| b.battery) {
            Some(b) => format!("{name}  ·  {b}%"),
            None => name,
        };
        let glyph = if btd.is_some() { "\u{E702}" } else { "\u{E7F5}" };
        let tw = (gfx::g().measure(&label, &self.f.label) + 46.0).min(maxw);
        let r = rf(x, y, tw, 30.0);
        let hot = self.hot == Some(Hit::OpenDevices);
        self.surf.fill_rr(r, 15.0, self.fg(if hot { 0.14 } else { 0.07 } * a));
        let col = if btd.is_some() { self.ic(pal::BLUE, a) } else { self.ic(pal::ORANGE, 0.85 * a) };
        self.surf.text(glyph, &self.f.icon_m, rf(x + 9.0, y, 20.0, 30.0), col);
        self.surf.text(&label, &self.f.label, rf(x + 34.0, y, tw - 40.0, 30.0), self.fg(0.8 * a));
        if enabled_pages().contains(&Page::Devices) {
            self.hits.push((Hit::OpenDevices, r));
        }
        tw
    }

    // ------------------------------------------------------------------
    // Devices: audio output, volume, Bluetooth, Wi-Fi, battery
    // ------------------------------------------------------------------

    pub(super) fn page_devices(&mut self, bx: f32, by: f32, bw: f32, bh: f32, a: f32) {
        // Left: every device as a deck of cards, the volume, and a full-width
        // button for where the sound goes.
        let lw = (bw * 0.5).round();
        self.device_deck(bx, by, lw, a);
        self.volume_row(bx, by + 134.0, lw, a);
        let ow = ((lw - 8.0) / 2.0).floor();
        self.output_button(rf(bx, by + bh - 38.0, ow, 38.0), a);
        self.input_button(rf(bx + lw - ow, by + bh - 38.0, ow, 38.0), a);
        // Right: connections & power.
        let x = bx + lw + 22.0;
        self.surf.fill_rect(rf(x - 12.0, by + 4.0, 1.0, bh - 8.0), self.fg(0.1 * a));
        // Connections and the sound-output list slide past each other.
        let t = self.out_anim.v.clamp(0.0, 1.0);
        let ease = t * t * (3.0 - 2.0 * t);
        let cw = bx + bw - x;
        let vh = bh - 4.0;
        self.zones.push((2, rf(bx, by - 4.0, lw, 124.0)));
        self.zones.push((3, rf(x - 10.0, by - 4.0, cw + 30.0, vh + 8.0)));
        self.surf.clip(rf(x - 6.0, by - 4.0, cw + 12.0, vh + 8.0));
        if ease < 0.99 {
            let n = self.hits.len();
            self.connections(x - 24.0 * ease, by, cw, a * (1.0 - ease), vh);
            if ease > 0.5 {
                self.hits.truncate(n);
            }
        }
        if ease > 0.01 {
            let n = self.hits.len();
            self.output_list(x, by, cw, a, ease, vh);
            if ease < 0.5 {
                self.hits.truncate(n);
            }
        }
        self.surf.unclip();
        self.cluster_popover(rf(bx, by, lw, bh), a);
    }

    fn volume_row(&mut self, x: f32, vy: f32, w: f32, a: f32) {
        let (vol, muted) = self.shown_volume();
        let mr = rf(x, vy - 14.0, 28.0, 28.0);
        if self.hot == Some(Hit::Mute) {
            self.surf.fill_circle(x + 14.0, vy, 14.0, self.fg(0.1 * a));
        }
        self.speaker(x + 14.0, vy, 16.0, speaker_level(vol, muted), self.ic(pal::BLUE, 0.95 * a));
        self.hits.push((Hit::Mute, mr));
        let res = if self.mo_cluster.is_empty() { 0.0 } else { Island::CHIP_W + 6.0 };
        let (tx0, tw) = (x + 38.0, w - 38.0 - 50.0 - res);
        let f = if muted { 0.0 } else { vol };
        self.cluster_chip(x + w, vy, a);
        self.surf.fill_rr(rf(tx0, vy - 3.0, tw, 6.0), 3.0, self.fg(0.16 * a));
        self.surf.fill_rr(rf(tx0, vy - 3.0, (tw * f).max(6.0), 6.0), 3.0, self.el(pal::BLUE, a));
        self.surf.fill_circle(tx0 + tw * f, vy, if self.hot == Some(Hit::Volume) || self.vol_drag { 9.0 } else { 7.5 }, rgb(0xffffff, a));
        self.hits.push((Hit::Volume, rf(tx0 - 10.0, vy - 14.0, tw + 20.0, 28.0)));
        let pct = if muted { tr("Muted").to_string() } else { format!("{}%", (vol * 100.0).round() as i32) };
        self.surf.text(&pct, &self.f.body, rf(tx0 + tw + 8.0, vy - 11.0, 44.0, 22.0), self.fg(0.75 * a));
    }

    /// Width of the "speakers playing together" button at the end of a volume row.
    pub(super) const CHIP_W: f32 = 54.0;

    /// The button at the end of a volume row that opens every speaker's own
    /// slider (only while several play together). `right`: where the row ends.
    pub(super) fn cluster_chip(&mut self, right: f32, cy: f32, a: f32) {
        if self.mo_cluster.is_empty() {
            self.cv_anchor = None;
            return;
        }
        let r = rf(right - Island::CHIP_W, cy - 14.0, Island::CHIP_W, 28.0);
        let hot = self.hot == Some(Hit::CluVol);
        let blue = self.el(pal::BLUE, 1.0);
        self.surf.fill_rr(r, 14.0, if self.cv_open { with_alpha(blue, 0.25 * a) } else { self.fg(if hot { 0.15 } else { 0.08 } * a) });
        // A speaker and how many play together.
        self.speaker(r.left + 15.0, cy, 14.0, 2, with_alpha(blue, a));
        self.surf.text(&self.mo_cluster.len().to_string(), &self.f.tiny_b, rf(r.left + 25.0, r.top, 12.0, 28.0), self.fg(0.9 * a));
        self.surf.text(if self.cv_open { "\u{E70E}" } else { "\u{E70D}" }, &self.f.icon_s, rf(r.right - 20.0, r.top, 14.0, 28.0), self.fg(0.65 * a));
        self.hits.push((Hit::CluVol, r));
        self.cv_anchor = Some(r);
    }

    /// Every speaker playing together, each with its own small slider: a card
    /// that drops from the volume row's button. Drawn last, over the page;
    /// a click anywhere else closes it.
    pub(super) fn cluster_popover(&mut self, bounds: D2D_RECT_F, a: f32) {
        let anchor = self.cv_anchor.take();
        let t = self.cv_anim.v.clamp(0.0, 1.0);
        let e = t * t * (3.0 - 2.0 * t);
        let (Some(an), false) = (anchor, self.mo_cluster.is_empty()) else { return };
        if e < 0.01 {
            return;
        }
        let rows: Vec<(usize, String, String)> = self.mo_cluster.iter().enumerate().map(|(k, (id, n))| (k, id.clone(), n.clone())).collect();
        let (head, rowh) = (28.0, 26.0);
        let h = head + rowh * rows.len() as f32 + 8.0;
        let w = (bounds.right - bounds.left).min(340.0);
        let x = (an.right - w).max(bounds.left);
        let below = an.bottom + 6.0 + h <= bounds.bottom + 10.0;
        let y0 = if below { an.bottom + 6.0 } else { (an.top - 6.0 - h).max(bounds.top - 6.0) };
        let y = y0 + if below { -10.0 } else { 10.0 } * (1.0 - e);
        let ca = a * e;
        if self.cv_open {
            self.hits.push((Hit::CluClose, rf(bounds.left - 60.0, bounds.top - 60.0, bounds.right - bounds.left + 120.0, bounds.bottom - bounds.top + 120.0)));
            self.hits.push((Hit::CluVol, an));
        }
        let card = rf(x, y, w, h);
        self.surf.shadow(card, 14.0, 0.5 * ca);
        let bg = self.blend(0.11);
        self.surf.fill_rr(card, 14.0, with_alpha(bg, ca));
        self.surf.stroke_rr(card, 14.0, self.fg(0.1 * ca), 1.0);
        if self.cv_open {
            self.hits.push((Hit::CluCard, card));
        }
        let g = gfx::g();
        self.surf.text(tr("Speakers playing together"), &self.f.tiny, rf(x + 14.0, y + 6.0, w - 28.0, 20.0), self.fg(0.55 * ca));
        let nw = (w * 0.34).min(116.0);
        for (n, (k, id, name)) in rows.iter().enumerate() {
            let ry = y + head + rowh * n as f32 + rowh / 2.0;
            let _ = id;
            let muted = self.mo_lv.get(*k).map(|l| l.1).unwrap_or(false);
            let v = if muted { 0.0 } else { self.mo_show.get(*k).copied().unwrap_or(0.0) };
            let mut label = name.clone();
            if *k == 0 {
                label = format!("{label} · {}", tr("Main"));
            }
            self.surf.text(&ellip(g, &label, &self.f.tiny, nw), &self.f.tiny, rf(x + 14.0, ry - 9.0, nw, 18.0), self.fg(0.85 * ca));
            let (sx, sw) = (x + 14.0 + nw + 8.0, w - 14.0 - nw - 8.0 - 50.0);
            let hit = Hit::MoCVol(*k);
            let hot = self.hot == Some(hit) || self.mo_drag == Some(hit);
            self.surf.fill_rr(rf(sx, ry - 3.0, sw, 6.0), 3.0, self.fg(0.14 * ca));
            self.surf.fill_rr(rf(sx, ry - 3.0, (sw * v).max(6.0), 6.0), 3.0, self.el(pal::BLUE, ca));
            self.surf.fill_circle(sx + sw * v, ry, if hot { 8.0 } else { 6.5 }, rgb(0xffffff, ca));
            if self.cv_open {
                self.hits.push((hit, rf(sx - 8.0, ry - 12.0, sw + 16.0, 24.0)));
            }
            let pct = if muted { tr("Muted").to_string() } else { format!("{}%", (v * 100.0).round() as i32) };
            self.surf.text(&pct, &self.f.tiny_r, rf(sx + sw + 4.0, ry - 9.0, 44.0, 18.0), self.fg(0.75 * ca));
        }
    }

    /// "🎤 Sound input / Microphone Array ›": opens the list of microphones.
    fn input_button(&mut self, r: D2D_RECT_F, a: f32) {
        let open = self.out_menu && self.in_mode;
        let name = self.in_list.iter().find(|i| i.default).map(|i| crate::bt::inner(&i.name).to_string()).unwrap_or_else(|| tr("No microphones").to_string());
        self.io_button(r, a, Hit::InMenu, open, "\u{E720}", tr("Sound input"), &name);
    }

    /// "🔊 Sound output / Realtek(R) Audio ›": opens the list of outputs.
    fn output_button(&mut self, r: D2D_RECT_F, a: f32) {
        let d = crate::data();
        let open = self.out_menu && !self.in_mode;
        let glyph = if d.output_bt().is_some() { "\u{E702}" } else { "\u{E7F5}" };
        let name = if d.output.is_empty() { tr("No sound devices").to_string() } else { crate::bt::inner(&d.output).to_string() };
        self.io_button(r, a, Hit::OutMenu, open, glyph, tr("Sound output"), &name);
    }

    /// The two equal buttons under the volume: icon, a small caption over the
    /// device's name, and a chevron.
    fn io_button(&mut self, r: D2D_RECT_F, a: f32, hit: Hit, open: bool, glyph: &str, caption: &str, name: &str) {
        let hot = self.hot == Some(hit);
        let blue = self.ic(pal::BLUE, 1.0);
        let h = r.bottom - r.top;
        let w = r.right - r.left;
        self.surf.fill_rr(r, h / 2.0, if open { with_alpha(blue, 0.22 * a) } else { self.fg(if hot { 0.13 } else { 0.08 } * a) });
        self.surf.text(glyph, &self.f.icon_m, rf(r.left + 12.0, r.top, 22.0, h), with_alpha(blue, a));
        let g = gfx::g();
        let tw = w - 42.0 - 28.0;
        let cy = r.top + h / 2.0;
        self.surf.text(&ellip(g, caption, &self.f.small, tw), &self.f.small, rf(r.left + 40.0, cy - 15.0, tw, 13.0), self.fg(0.5 * a));
        self.surf.text(&ellip(g, name, &self.f.label, tw), &self.f.label, rf(r.left + 40.0, cy - 3.0, tw, 17.0), self.fg(0.92 * a));
        self.surf.text(if open { "\u{E76B}" } else { "\u{E76C}" }, &self.f.icon_s, rf(r.right - 26.0, r.top, 16.0, h), self.fg(0.6 * a));
        self.hits.push((hit, r));
    }

    /// The sound-output list (right side of the Devices page).
    /// Keep the shared right-column scroll in range; returns the y offset.
    fn dev_scrolled(&mut self, content_h: f32, view_h: f32, x: f32, y: f32, a: f32) -> f32 {
        let max = (content_h - view_h).max(0.0);
        self.dev_scroll = self.dev_scroll.min(max);
        if max > 0.0 {
            let bh = (view_h * view_h / content_h).max(16.0);
            let by = y + (view_h - bh) * (self.dev_scroll / max);
            self.surf.fill_rr(rf(x, by, 3.0, bh), 1.5, self.fg(0.22 * a));
        }
        self.dev_scroll
    }

    /// The sound outputs (or, from Sound input, the microphones).
    /// Outputs: click one to open its settings — its volume, and with "Play on
    /// several" on: its delay (type it, or −/+), Auto-sync, Calibrate, Make
    /// main. The tick adds it to the speakers playing together.
    fn output_list(&mut self, x: f32, y: f32, w: f32, a: f32, t: f32, view_h: f32) {
        if self.in_mode {
            self.input_list(x, y, w, a, t, view_h);
            return;
        }
        let g = gfx::g();
        let mo = config::cur().b("mo_on");
        let blue = self.ic(pal::BLUE, 1.0);
        // Header: title, and the "Play on several" switch.
        self.surf.text(tr("SOUND OUTPUT"), &self.f.label, rf(x + 2.0 + 20.0 * (1.0 - t), y, w, 16.0), self.fg(0.45 * a * t));
        {
            let sw = rf(x + w - 34.0, y - 1.0, 34.0, 18.0);
            let lbl = tr("Play on several");
            let lw = g.measure(lbl, &self.f.tiny);
            self.surf.text(lbl, &self.f.tiny, rf(sw.left - lw - 8.0, y - 2.0, lw + 4.0, 20.0), self.fg(0.7 * a * t));
            if mo {
                self.surf.fill_rr(sw, 9.0, with_alpha(blue, a * t));
            } else {
                self.surf.stroke_rr(rf(sw.left + 0.75, sw.top + 0.75, 32.5, 16.5), 8.25, self.fg(0.45 * a * t), 1.4);
            }
            self.surf.fill_circle(if mo { sw.right - 9.0 } else { sw.left + 9.0 }, sw.top + 9.0, 6.0, if mo { Island::text_on(blue, a * t) } else { self.fg(0.75 * a * t) });
            self.hits.push((Hit::MoToggle, rf(sw.left - lw - 12.0, y - 4.0, lw + 50.0, 24.0)));
        }
        let list = self.out_list.clone();
        if list.is_empty() {
            self.surf.text(tr("No sound devices"), &self.f.body, rf(x, y + 24.0, w, 24.0), self.fg(0.5 * a * t));
            return;
        }
        let st = crate::multiout::status();
        let rh = 28.0;
        let line = 26.0;
        let tgts: Vec<crate::multiout::Target> = list.iter().map(|o| crate::multiout::target(&o.id)).collect();
        let picked = self.mo_sel.clone();
        let sel = |i: usize| picked.as_deref() == Some(list[i].id.as_str());
        let ticked = |i: usize| mo && !list[i].default && tgts[i].on;
        let behind = |id: &str| st.behind.iter().find(|b| b.0 == id).map(|b| b.1);
        // Lines under a row while its settings are open: volume; delay and the
        // buttons when it's playing along; "Make main" otherwise.
        let panel = |i: usize| -> f32 {
            if !sel(i) {
                return 0.0;
            }
            let mut h = line;
            if ticked(i) {
                h += line + 30.0;
            } else if !list[i].default {
                h += 30.0;
            }
            if behind(&list[i].id).is_some() {
                h += 16.0;
            }
            h + 4.0
        };
        let footer = if mo { 58.0 } else { 0.0 };
        let content = 24.0 + (0..list.len()).map(|i| rh + panel(i)).sum::<f32>() + footer;
        let off = self.dev_scrolled(content, view_h, x + w + 3.0, y, a * t);
        let mut yy = y + 24.0 - off;
        for (i, o) in list.iter().enumerate() {
            // Rows arrive one after another.
            let ti = ((t * 1.35) - i as f32 * 0.09).clamp(0.0, 1.0);
            let ti = ti * ti * (3.0 - 2.0 * ti);
            let a = a * ti;
            let ph = panel(i);
            let r = rf(x - 4.0 + 28.0 * (1.0 - ti), yy, w + 4.0, rh - 2.0);
            let hot = self.hot == Some(Hit::OutPick(i));
            let card = rf(r.left, r.top, r.right - r.left, rh - 2.0 + ph);
            if o.default {
                self.surf.fill_rr(card, 8.0, with_alpha(blue, 0.18 * a));
                self.surf.fill_rr(rf(r.left + 1.0, r.top + 7.0, 3.0, rh - 16.0), 1.5, with_alpha(blue, a));
            } else if sel(i) || ticked(i) {
                self.surf.fill_rr(card, 8.0, self.fg(if sel(i) { 0.08 } else { 0.05 } * a));
            } else if hot {
                self.surf.fill_rr(r, 8.0, self.fg(0.08 * a));
            }
            let glyph = match o.form {
                3 | 4 => "\u{E7F6}",
                9 => "\u{E7F4}",
                _ => "\u{E7F5}",
            };
            self.surf.text(glyph, &self.f.icon_s, rf(r.left + 8.0, r.top, 18.0, rh - 2.0), if o.default { with_alpha(blue, a) } else { self.fg(0.75 * a) });
            let right_w = if mo || o.default { 58.0 } else { 18.0 };
            self.surf.text(&ellip(g, &o.name, &self.f.body, w - 34.0 - right_w), &self.f.body, rf(r.left + 30.0, r.top, w - 34.0 - right_w, rh - 2.0), self.fg(if o.default { 1.0 } else { 0.85 } * a));
            self.hits.push((Hit::OutPick(i), rf(r.left, r.top, r.right - r.left - if mo && !o.default { 34.0 } else { 0.0 }, rh - 2.0)));
            if o.default {
                let tg = tr("Main");
                let tw = g.measure(tg, &self.f.tiny) + 14.0;
                let tr_ = rf(r.right - tw - (if mo { 6.0 } else { 22.0 }), r.top + 5.0, tw, rh - 12.0);
                self.surf.fill_rr(tr_, (rh - 12.0) / 2.0, with_alpha(blue, 0.85 * a));
                self.surf.text(tg, &self.f.tiny_c, tr_, Island::text_on(blue, a));
            } else if mo {
                // Tick: plays along.
                let on = tgts[i].on;
                let cb = rf(r.right - 28.0, r.top + 5.0, 18.0, 18.0);
                let hot_c = self.hot == Some(Hit::MoTick(i));
                if on {
                    self.surf.fill_rr(cb, 5.0, with_alpha(blue, a));
                    self.surf.text("\u{E73E}", &self.f.icon_s, cb, Island::text_on(blue, a));
                } else {
                    self.surf.stroke_rr(rf(cb.left + 0.75, cb.top + 0.75, 16.5, 16.5), 4.5, self.fg(if hot_c { 0.8 } else { 0.45 } * a), 1.5);
                }
                self.hits.push((Hit::MoTick(i), rf(cb.left - 8.0, r.top, 34.0, rh - 2.0)));
            }
            if !(mo || o.default) {
                self.surf.text(if sel(i) { "\u{E70E}" } else { "\u{E70D}" }, &self.f.icon_s, rf(r.right - 22.0, r.top, 16.0, rh - 2.0), self.fg(0.45 * a));
            }
            if sel(i) {
                let lx = x + 8.0;
                let lw_ = w - 16.0;
                let mut ly = yy + rh;
                // Its own volume.
                let (vol, _) = crate::multiout::dev_volume(&o.id).unwrap_or((0.0, false));
                let vy = ly + line / 2.0 - 2.0;
                self.speaker(lx + 8.0, vy, 14.0, speaker_level(vol, false), self.fg(0.65 * a));
                let (sx, sw_) = (lx + 24.0, lw_ - 24.0 - 44.0);
                self.surf.fill_rr(rf(sx, vy - 2.5, sw_, 5.0), 2.5, self.fg(0.16 * a));
                self.surf.fill_rr(rf(sx, vy - 2.5, (sw_ * vol).max(5.0), 5.0), 2.5, self.fg(0.85 * a));
                let dragging = self.mo_drag == Some(Hit::MoVol(i)) || self.hot == Some(Hit::MoVol(i));
                self.surf.fill_circle(sx + sw_ * vol, vy, if dragging { 7.0 } else { 5.5 }, rgb(0xffffff, a));
                self.hits.push((Hit::MoVol(i), rf(sx - 6.0, vy - 11.0, sw_ + 12.0, 22.0)));
                self.surf.text(&format!("{}%", (vol * 100.0).round() as i32), &self.f.tiny_r, rf(lx + lw_ - 42.0, vy - 9.0, 42.0, 18.0), self.fg(0.75 * a));
                ly += line;
                let btn = |isl: &mut Island, bx_: f32, by_: f32, label: &str, glyph: &str, hit: Hit, strong: bool| -> f32 {
                    let bw_ = gfx::g().measure(label, &isl.f.tiny) + if glyph.is_empty() { 20.0 } else { 34.0 };
                    let br = rf(bx_, by_, bw_, 24.0);
                    let hot = isl.hot == Some(hit);
                    isl.surf.fill_rr(br, 12.0, if strong { with_alpha(blue, if hot { 1.0 } else { 0.85 } * a) } else { isl.fg(if hot { 0.16 } else { 0.08 } * a) });
                    let tc = if strong { Island::text_on(blue, a) } else { isl.fg(0.85 * a) };
                    if !glyph.is_empty() {
                        isl.surf.text(glyph, &isl.f.icon_s, rf(br.left + 8.0, br.top, 16.0, 24.0), if strong { tc } else { with_alpha(blue, a) });
                    }
                    isl.surf.text(label, &isl.f.tiny, rf(br.left + if glyph.is_empty() { 10.0 } else { 27.0 }, br.top, bw_ - 28.0 + 18.0, 24.0), tc);
                    isl.hits.push((hit, br));
                    bw_
                };
                if ticked(i) {
                    // Delay: − [ typed ms ] +, then the slider for big moves.
                    let dy = ly + line / 2.0 - 2.0;
                    self.surf.text("\u{E916}", &self.f.icon_s, rf(lx, dy - 9.0, 16.0, 18.0), self.fg(0.6 * a));
                    let fx = lx + 22.0;
                    let nb = |isl: &mut Island, bx_: f32, glyph: &str, hit: Hit| {
                        let br = rf(bx_, dy - 11.0, 22.0, 22.0);
                        let hot = isl.hot == Some(hit);
                        isl.surf.fill_rr(br, 11.0, isl.fg(if hot { 0.18 } else { 0.08 } * a));
                        isl.surf.text(glyph, &isl.f.icon_s, br, isl.fg(0.85 * a));
                        isl.hits.push((hit, br));
                    };
                    nb(self, fx, "\u{E738}", Hit::MoNudge(i, -5));
                    let field = rf(fx + 26.0, dy - 11.0, 64.0, 22.0);
                    let editing = self.edit == Some(EditTarget::MoDelay) && self.mo_edit_id == o.id;
                    self.surf.fill_rr(field, 6.0, if editing { self.fg(0.14 * a) } else { self.fg(if self.hot == Some(Hit::MoDelayEdit(i)) { 0.12 } else { 0.06 } * a) });
                    if editing {
                        self.surf.stroke_rr(field, 6.0, with_alpha(blue, a), 1.2);
                        if self.edit_all {
                            self.surf.fill_rr(rf(field.left + 10.0, field.top + 4.0, field.right - field.left - 20.0, 14.0), 3.0, with_alpha(blue, 0.35 * a));
                        }
                    }
                    let txt = if editing { format!("{}{}", self.edit_buf, if self.caret { "|" } else { "" }) } else { format!("{} ms", tgts[i].delay) };
                    self.surf.text(&txt, &self.f.tiny_c, field, self.fg(0.95 * a));
                    self.hits.push((Hit::MoDelayEdit(i), field));
                    nb(self, field.right + 4.0, "\u{E710}", Hit::MoNudge(i, 5));
                    let sx2 = field.right + 34.0;
                    let sw2 = (x + w - 8.0 - sx2).max(30.0);
                    let f = tgts[i].delay as f32 / crate::multiout::MAX_DELAY as f32;
                    self.surf.fill_rr(rf(sx2, dy - 2.5, sw2, 5.0), 2.5, self.fg(0.16 * a));
                    self.surf.fill_rr(rf(sx2, dy - 2.5, (sw2 * f).max(5.0), 5.0), 2.5, self.el(pal::ORANGE, a));
                    let dragging = self.mo_drag == Some(Hit::MoDelay(i)) || self.hot == Some(Hit::MoDelay(i));
                    self.surf.fill_circle(sx2 + sw2 * f, dy, if dragging { 7.0 } else { 5.5 }, rgb(0xffffff, a));
                    self.hits.push((Hit::MoDelay(i), rf(sx2 - 6.0, dy - 11.0, sw2 + 12.0, 22.0)));
                    ly += line;
                    // This speaker: line it up, or make it the main one — three
                    // equal buttons across.
                    let bw3 = (lw_ - 12.0) / 3.0;
                    let busy = st.busy;
                    for (k, (label, glyph, hit)) in [(tr("Auto-sync"), "\u{E895}", Hit::MoAutoOne(i)), (tr("Calibrate"), "\u{E720}", Hit::MoCalibOne(i)), (tr("Make main"), "\u{E7F5}", Hit::MoMain(i))].into_iter().enumerate() {
                        let br = rf(lx + k as f32 * (bw3 + 6.0), ly + 2.0, bw3, 24.0);
                        let hot = self.hot == Some(hit);
                        let on = busy && hit == Hit::MoCalibOne(i);
                        self.surf.fill_rr(br, 12.0, if on { with_alpha(blue, 0.3 * a) } else { self.fg(if hot { 0.16 } else { 0.08 } * a) });
                        let lwid = g.measure(label, &self.f.tiny).min(bw3 - 30.0);
                        let x0 = br.left + (bw3 - lwid - 20.0) / 2.0;
                        self.surf.text(glyph, &self.f.icon_s, rf(x0, br.top, 16.0, 24.0), with_alpha(blue, a));
                        self.surf.text(&ellip(g, label, &self.f.tiny, bw3 - 30.0), &self.f.tiny, rf(x0 + 20.0, br.top, lwid + 6.0, 24.0), self.fg(0.85 * a));
                        self.hits.push((hit, br));
                    }
                    ly += 30.0;
                } else if !o.default {
                    btn(self, lx, ly + 2.0, tr("Make main"), "", Hit::MoMain(i), false);
                    ly += 30.0;
                }
                if let Some(ms) = behind(&o.id) {
                    let msg = format!("{} {ms} ms", tr("Slower than the main output by"));
                    self.surf.text(&ellip(g, &msg, &self.f.small, w - 12.0), &self.f.small, rf(lx, ly, w - 12.0, 14.0), self.ic(pal::ORANGE, a));
                }
            }
            yy += rh + ph;
        }
        if mo {
            // Line up all of them at once, and what happened.
            let by_ = yy + 4.0;
            let mut bx_ = x;
            for (hit, glyph, label) in [(Hit::MoAuto, "\u{E895}", tr("Auto-sync all")), (Hit::MoCalib, "\u{E720}", tr("Calibrate all (mic)"))] {
                let bw_ = g.measure(label, &self.f.tiny) + 36.0;
                let br = rf(bx_, by_, bw_, 26.0);
                let hot = self.hot == Some(hit);
                let busy = st.busy && hit == Hit::MoCalib;
                self.surf.fill_rr(br, 13.0, if busy { with_alpha(blue, 0.3 * a) } else { self.fg(if hot { 0.16 } else { 0.08 } * a) });
                self.surf.text(glyph, &self.f.icon_s, rf(br.left + 9.0, br.top, 16.0, 26.0), with_alpha(blue, a));
                self.surf.text(label, &self.f.tiny, rf(br.left + 28.0, br.top, bw_ - 30.0, 26.0), self.fg(0.85 * a));
                self.hits.push((hit, br));
                bx_ += bw_ + 6.0;
            }
            let (msg, col) = if st.busy && st.msg.is_empty() {
                (tr("Calibrating…").to_string(), self.fg(0.55 * a))
            } else if !st.msg.is_empty() {
                (st.msg.clone(), if st.bad { rgb(pal::RED, a) } else { self.fg(0.55 * a) })
            } else if st.running {
                (tr("Copying to the ticked devices").to_string(), self.ic(pal::GREEN, a))
            } else {
                (tr("Tick the devices that should play too").to_string(), self.fg(0.5 * a))
            };
            self.surf.text(&ellip(g, &msg, &self.f.small, w), &self.f.small, rf(x + 2.0, by_ + 30.0, w, 14.0), col);
            if st.busy {
                self.keep_anim = true;
            }
        }
    }

    /// The microphones: pick the one Windows (and calibration) uses.
    fn input_list(&mut self, x: f32, y: f32, w: f32, a: f32, t: f32, view_h: f32) {
        let g = gfx::g();
        let blue = self.ic(pal::BLUE, 1.0);
        self.surf.text(tr("SOUND INPUT"), &self.f.label, rf(x + 2.0 + 20.0 * (1.0 - t), y, w, 16.0), self.fg(0.45 * a * t));
        let list = self.in_list.clone();
        if list.is_empty() {
            self.surf.text(tr("No microphones"), &self.f.body, rf(x, y + 24.0, w, 24.0), self.fg(0.5 * a * t));
            return;
        }
        let rh = 30.0;
        let off = self.dev_scrolled(24.0 + list.len() as f32 * rh + 34.0, view_h, x + w + 3.0, y, a * t);
        let mut yy = y + 24.0 - off;
        for (i, m) in list.iter().enumerate() {
            let ti = ((t * 1.35) - i as f32 * 0.09).clamp(0.0, 1.0);
            let ti = ti * ti * (3.0 - 2.0 * ti);
            let a = a * ti;
            let r = rf(x - 4.0 + 28.0 * (1.0 - ti), yy, w + 4.0, rh - 3.0);
            let hot = self.hot == Some(Hit::InPick(i));
            if m.default {
                self.surf.fill_rr(r, 8.0, with_alpha(blue, 0.18 * a));
                self.surf.fill_rr(rf(r.left + 1.0, r.top + 7.0, 3.0, rh - 17.0), 1.5, with_alpha(blue, a));
            } else if hot {
                self.surf.fill_rr(r, 8.0, self.fg(0.08 * a));
            }
            let bt = crate::multiout::is_bluetooth(&m.id) || m.name.to_lowercase().contains("hands-free");
            self.surf.text("\u{E720}", &self.f.icon_s, rf(r.left + 8.0, r.top, 18.0, rh - 3.0), if m.default { with_alpha(blue, a) } else { self.fg(0.75 * a) });
            let tag = if bt { tr("Bluetooth — call quality") } else { "" };
            let tw = if tag.is_empty() { 0.0 } else { g.measure(tag, &self.f.small) + 8.0 };
            self.surf.text(&ellip(g, &m.name, &self.f.body, w - 34.0 - tw), &self.f.body, rf(r.left + 30.0, r.top, w - 34.0 - tw, rh - 3.0), self.fg(if m.default { 1.0 } else { 0.85 } * a));
            if !tag.is_empty() {
                self.surf.text(tag, &self.f.small, rf(r.right - tw - 4.0, r.top, tw, rh - 3.0), self.ic(pal::ORANGE, 0.9 * a));
            }
            self.hits.push((Hit::InPick(i), r));
            yy += rh;
        }
        self.surf.text_wrap(tr("Calibration listens with the microphone picked here. A Bluetooth speaker's own microphone switches it to call quality — use the PC's."), &self.f.wrap_src, rf(x + 2.0, yy + 4.0, w, 32.0), self.fg(0.45 * a * t));
    }

    /// Wi-Fi, Ethernet, VPN, Bluetooth radio, IP address, battery.
    fn connections(&mut self, x: f32, y: f32, w: f32, a: f32, view_h: f32) {
        let d = crate::data();
        let net = d.net.clone();
        let online = d.online.unwrap_or(true);
        let mut rows: Vec<(&'static str, D2D1_COLOR_F, String, String, f32)> = Vec::new(); // glyph, colour, title, right, alpha
        // Wi-Fi
        match &net.wifi {
            Some(wf) => {
                let right = if wf.band.is_empty() { format!("{}%", wf.signal) } else { format!("{} · {}%", wf.band, wf.signal) };
                rows.push(("\u{E701}", self.ic(pal::GREEN, a), wf.ssid.clone(), right, 1.0));
            }
            None if net.ethernet.is_empty() => {
                let off = crate::controls::state().wifi == Some(false);
                rows.push(("\u{EB5E}", self.fg(0.4 * a), tr("Wi-Fi").into(), tr(if off { "Off" } else { "Not connected" }).into(), 0.5))
            }
            None => {}
        }
        for (name, bps) in &net.ethernet {
            rows.push(("\u{E839}", self.ic(pal::TEAL, a), name.clone(), crate::netinfo::speed_label(*bps), 1.0));
        }
        if !online {
            rows.push(("\u{E783}", rgb(pal::RED, a), tr("No internet").into(), tr("Offline").into(), 1.0));
        }
        // VPN
        if net.vpn.is_empty() {
            rows.push(("\u{E72E}", self.fg(0.35 * a), tr("VPN").into(), tr("Off").into(), 0.5));
        } else {
            for v in net.vpn.iter().take(2) {
                rows.push(("\u{E72E}", self.ic(pal::PURPLE, a), v.clone(), tr("On").into(), 1.0));
            }
        }
        let ctl = crate::controls::state();
        if ctl.airplane == Some(true) {
            rows.insert(0, ("\u{E709}", self.ic(pal::ORANGE, a), tr("Airplane mode").into(), tr("On").into(), 1.0));
        }
        if let Some(m) = ctl.mic_muted {
            let name = if ctl.mic_name.is_empty() { tr("Microphone").to_string() } else { crate::bt::inner(&ctl.mic_name).to_string() };
            let (g, col) = if m { ("\u{EC54}", rgb(pal::RED, a)) } else { ("\u{E720}", self.ic(pal::GREEN, a)) };
            rows.push((g, col, name, tr(if m { "Muted" } else { "On" }).into(), if m { 0.8 } else { 1.0 }));
        }
        // Bluetooth radio
        let n_bt = d.bt.len();
        let (bt_right, bt_a) = match ctl.bt.or(net.bt_on) {
            Some(false) => (tr("Off").to_string(), 0.5),
            _ if n_bt > 0 => (format!("{n_bt} {}", tr(if n_bt == 1 { "device" } else { "devices" })), 1.0),
            Some(true) => (tr("On").to_string(), 1.0),
            None => ("—".to_string(), 0.5),
        };
        rows.push(("\u{E702}", if bt_a < 1.0 { self.fg(0.35 * a) } else { self.ic(pal::BLUE, a) }, tr("Bluetooth").into(), bt_right, bt_a));
        let mut ip_row = None;
        if let Some(ip) = &net.ip {
            // The address, and the MAC of the same (connected) adapter — click either to copy it.
            let right = net.mac.clone().unwrap_or_else(|| tr("IP address").to_string());
            ip_row = Some((rows.len(), ip.clone(), net.mac.clone()));
            rows.push(("\u{E968}", self.ic(pal::ORANGE, a), ip.clone(), right, 0.8));
        }
        // Every connected Bluetooth device gets its own row (scroll for more).
        for b in d.bt.iter() {
            let right = b.battery.map(|p| format!("{p}%")).unwrap_or_else(|| tr(b.kind.label()).to_string());
            rows.push((b.kind.glyph(), self.ic(b.kind.color(), a), b.name.clone(), right, 0.9));
        }
        let rh = 21.5;
        let total = rows.len() as f32 * rh + 2.0;
        let off = self.dev_scrolled(total, view_h, x + w + 3.0, y, a);
        let mut ry = y + 1.0 - off;
        let g = gfx::g();
        let copied = self.net_copied.filter(|c| c.1.elapsed().as_millis() < 1600).map(|c| c.0);
        let green = rgb(pal::GREEN, a);
        for (i, (glyph, col, title, right, ra)) in rows.into_iter().enumerate() {
            self.surf.text(glyph, &self.f.icon_s, rf(x, ry, 16.0, rh), col);
            let rw = if right.is_empty() { 0.0 } else { g.measure(&right, &self.f.tiny).min(w * 0.5) + 4.0 };
            let is_ip = ip_row.as_ref().map(|r| r.0 == i).unwrap_or(false);
            let tr_ = rf(x + 24.0, ry, w - 24.0 - rw - 6.0, rh);
            let rr_ = rf(x + w - rw, ry, rw, rh);
            let visible = ry >= y - 2.0 && ry + rh <= y + view_h + 2.0;
            if is_ip && visible {
                // Click the address or the MAC to copy it; it says so for a moment.
                for (k, r) in [(0u8, tr_), (1u8, rr_)] {
                    if k == 1 && ip_row.as_ref().map(|r| r.2.is_none()).unwrap_or(true) {
                        continue;
                    }
                    let hot = self.hot == Some(Hit::CopyNet(k));
                    if hot {
                        let tw = if k == 0 { g.measure(&title, &self.f.body) } else { rw - 4.0 };
                        let hx = if k == 0 { r.left - 4.0 } else { r.right - tw - 6.0 };
                        self.surf.fill_rr(rf(hx, ry + 1.0, tw + 8.0, rh - 2.0), 6.0, self.fg(0.08 * a));
                    }
                    self.hits.push((Hit::CopyNet(k), r));
                }
            }
            if is_ip && copied == Some(0) {
                self.surf.text(&format!("\u{2713} {}", tr("Copied")), &self.f.body, tr_, green);
            } else {
                self.surf.text(&title, &self.f.body, tr_, self.fg(0.92 * ra * a));
            }
            if !right.is_empty() {
                if is_ip && copied == Some(1) {
                    self.surf.text(&format!("\u{2713} {}", tr("Copied")), &self.f.tiny_r, rr_, green);
                } else {
                    self.surf.text(&right, &self.f.tiny_r, rr_, self.fg(0.5 * a));
                }
            }
            ry += rh;
        }
        if copied.is_some() {
            self.keep_anim = true;
        }
    }

    /// The cards on the Devices page: the audio output first (when it isn't a
    /// Bluetooth device), then every connected Bluetooth device.
    pub(crate) fn deck_cards() -> Vec<(crate::bt::BtDevice, bool, bool)> {
        let d = crate::data();
        let out_bt = d.output_bt().map(|b| b.name.clone());
        let mut v = Vec::new();
        if out_bt.is_none() && !d.output.is_empty() {
            let kind_word = d.output.split(" (").next().unwrap_or("").to_lowercase();
            let mut dev = crate::bt::BtDevice::simple(crate::bt::inner(&d.output), None);
            dev.kind = if kind_word.contains("head") { crate::bt::Kind::Headphones } else { crate::bt::Kind::Speaker };
            v.push((dev, true, true)); // (device, is_output, wired)
        }
        let mut bts: Vec<_> = d.bt.iter().cloned().collect();
        // The device playing the sound goes first.
        if let Some(n) = &out_bt {
            if let Some(i) = bts.iter().position(|b| &b.name == n) {
                let b = bts.remove(i);
                bts.insert(0, b);
            }
        }
        for b in bts {
            let is_out = out_bt.as_deref() == Some(b.name.as_str());
            v.push((b, is_out, false));
        }
        v
    }

    /// One card at a time: the next ones peek out underneath; the wheel (or a
    /// click) flips to the next card.
    fn device_deck(&mut self, x: f32, y: f32, w: f32, a: f32) {
        let list = Island::deck_cards();
        let n = list.len();
        self.surf.text(tr("DEVICES"), &self.f.label, rf(x + 2.0, y, 120.0, 16.0), self.fg(0.42 * a));
        if n > 1 {
            let cur = (self.dev_card.target.round() as usize).min(n - 1) + 1;
            self.surf.text(&format!("{cur} / {n}"), &self.f.tiny_r, rf(x + w - 80.0, y, 66.0, 16.0), self.fg(0.42 * a));
        }
        let top = y + 22.0;
        let ch = 82.0;
        let cw = if n > 1 { w - 14.0 } else { w };
        let stack = rf(x - 2.0, top - 2.0, cw + 4.0, ch + 24.0);
        let card_col = |isl: &Island, depth: f32, alpha: f32| {
            // Opaque greys so stacked cards don't show through each other.
            let k = (0.105 - depth * 0.03).max(0.03);
            let v = if isl.dark { k } else { 1.0 - k * 0.75 };
            D2D1_COLOR_F { r: v, g: v, b: if isl.dark { v + 0.01 } else { v }, a: alpha }
        };
        if n == 0 {
            let r = rf(x, top, w, ch);
            self.surf.fill_rr(r, 16.0, card_col(self, 0.0, a));
            self.surf.text("\u{E702}", &self.f.icon_ctl, rf(x + 14.0, top + ch / 2.0 - 18.0, 36.0, 36.0), self.fg(0.35 * a));
            self.surf.text(tr("No devices connected"), &self.f.body, rf(x + 58.0, top + 20.0, w - 66.0, 20.0), self.fg(0.75 * a));
            self.surf.text(tr("Mice, keyboards, headphones…"), &self.f.tiny, rf(x + 58.0, top + 42.0, w - 66.0, 16.0), self.fg(0.4 * a));
            return;
        }
        let pos = self.dev_card.v.clamp(-0.5, n as f32 - 0.5);
        self.surf.clip(rf(x - 4.0, top - 4.0, w + 8.0, ch + 30.0));
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|p, q| (*q as f32 - pos).partial_cmp(&(*p as f32 - pos)).unwrap_or(std::cmp::Ordering::Equal));
        for i in order {
            let rel = i as f32 - pos;
            if rel <= -1.0 || rel >= 3.0 {
                continue;
            }
            let (dy, inset, alpha, content) = if rel < 0.0 {
                (rel * 54.0, 0.0, (1.0 + rel).powf(1.6), (1.0 + rel * 1.4).max(0.0))
            } else {
                (rel.min(2.0) * 9.0, rel.min(2.0) * 9.0, if rel > 2.0 { 3.0 - rel } else { 1.0 }, (1.0 - rel * 1.6).max(0.0))
            };
            let r = rf(x + inset, top + dy, cw - inset * 2.0, ch);
            let depth = rel.max(0.0).min(2.0);
            if rel < 0.6 {
                self.surf.shadow(r, 16.0, 0.35 * alpha * a);
            }
            self.surf.fill_rr(r, 16.0, card_col(self, depth, alpha * a));
            if content > 0.01 {
                let (dev, is_out, wired) = &list[i];
                self.device_card(dev, r, *is_out, *wired, content * a);
            }
        }
        self.surf.unclip();
        self.hits.push((Hit::DevDeck, stack));
        if n > 1 {
            let shown = n.min(9);
            let cur = (self.dev_card.target.round() as usize).min(n - 1).min(shown - 1);
            let total: f32 = (0..shown).map(|j| if j == cur { 14.0 } else { 5.0 } + 4.0).sum::<f32>() - 4.0;
            let mut dy = top + ch / 2.0 - total / 2.0;
            let dx = x + w - 5.0;
            for j in 0..shown {
                let is_cur = j == cur;
                let hgt = if is_cur { 14.0 } else { 5.0 };
                let hot = self.hot == Some(Hit::DevDot(j));
                self.surf.fill_rr(rf(dx - 2.5, dy, 5.0, hgt), 2.5, if is_cur { self.el(pal::BLUE, 0.95 * a) } else { self.fg(if hot { 0.55 } else { 0.25 } * a) });
                self.hits.push((Hit::DevDot(j), rf(dx - 7.0, dy - 2.0, 14.0, hgt + 4.0)));
                dy += hgt + 4.0;
            }
        }
    }

    fn device_card(&mut self, b: &crate::bt::BtDevice, r: D2D_RECT_F, is_output: bool, wired: bool, a: f32) {
        let g = gfx::g();
        let kc = kind_color(b.kind);
        let tint = self.ic(kc, 1.0);
        let (cx, cy, rad) = (r.left + 34.0, r.top + 41.0, 21.0);
        self.surf.fill_circle(cx, cy, rad, with_alpha(tint, 0.16 * a));
        if let Some(p) = b.battery {
            let col = if p <= 20 { rgb(pal::RED, 1.0) } else { tint };
            self.surf.ring(cx, cy, rad - 1.5, 3.0, p as f32 / 100.0, with_alpha(tint, 0.18 * a), with_alpha(col, a));
        }
        self.surf.text(b.kind.glyph(), &self.f.icon_ctl, rf(cx - rad, cy - rad, rad * 2.0, rad * 2.0), with_alpha(tint, a));
        let tx = r.left + 66.0;
        let tw = r.right - tx - 12.0;
        let nf = if g.measure(&b.name, &self.f.peek_t) <= tw { &self.f.peek_t } else { &self.f.body };
        self.surf.text(&b.name, nf, rf(tx, r.top + 12.0, tw, 22.0), self.fg(0.95 * a));
        let link = if wired { tr("Audio output") } else if b.le { "Bluetooth LE" } else { "Bluetooth" };
        let sub = format!("{}  ·  {}", tr(b.kind.label()), link);
        self.surf.text(&sub, &self.f.tiny, rf(tx, r.top + 34.0, tw, 16.0), self.fg(0.55 * a));
        let ly = r.top + 58.0;
        match b.battery {
            Some(p) => {
                let col = if p <= 20 { rgb(pal::RED, 1.0) } else { tint };
                let pt = format!("{p}%");
                let pw = g.measure(&pt, &self.f.tiny_b) + 6.0;
                let bw = (tw - pw).max(30.0);
                self.surf.fill_rr(rf(tx, ly - 2.0, bw, 4.0), 2.0, self.fg(0.14 * a));
                self.surf.fill_rr(rf(tx, ly - 2.0, (bw * p as f32 / 100.0).max(4.0), 4.0), 2.0, with_alpha(col, a));
                self.surf.text(&pt, &self.f.tiny_b, rf(tx + bw + 6.0, ly - 8.0, pw, 16.0), self.fg(0.8 * a));
            }
            None => {
                let t = if is_output {
                    tr("Playing audio").to_string()
                } else if !b.address.is_empty() {
                    b.address.clone()
                } else {
                    tr("Connected").to_string()
                };
                self.surf.text(&t, &self.f.tiny, rf(tx, ly - 8.0, tw, 16.0), self.fg(0.38 * a));
            }
        }
        if is_output {
            self.surf.fill_circle(r.right - 12.0, r.top + 14.0, 3.0, self.el(pal::GREEN, a));
        }
    }

    // ------------------------------------------------------------------
    // Controls: Windows quick settings in the island
    // ------------------------------------------------------------------

    pub(super) fn page_controls(&mut self, bx: f32, by: f32, bw: f32, bh: f32, a: f32) {
        let t = self.ctl_anim.v.clamp(0.0, 1.0);
        let e = t * t * (3.0 - 2.0 * t);
        if e < 0.999 {
            let n0 = self.hits.len();
            self.controls_main(bx - 36.0 * e, by, bw, bh, a * (1.0 - e));
            self.cluster_popover(rf(bx - 36.0 * e, by, bw, bh), a * (1.0 - e));
            if e > 0.5 {
                self.hits.truncate(n0);
            }
        }
        if self.ctl_list != 0 {
            self.zones.push((1, rf(bx - 30.0, by - 6.0, bw + 60.0, bh + 12.0)));
        }
        if e > 0.001 {
            let n0 = self.hits.len();
            self.controls_list(bx + 36.0 * (1.0 - e), by, bw, bh, a * e, e);
            if e <= 0.5 {
                self.hits.truncate(n0);
            }
        }
    }

    /// Wi-Fi networks / Bluetooth devices, with a link to Windows' settings at the end.
    fn controls_list(&mut self, bx: f32, by: f32, bw: f32, bh: f32, a: f32, e: f32) {
        let which = if self.ctl_list != 0 { self.ctl_list } else { self.ctl_last };
        if which == 3 {
            self.keyboard_list(bx, by, bw, bh, a, e);
            return;
        }
        if which == 4 {
            self.power_list(bx, by, bw, bh, a, e);
            return;
        }
        let wifi = which == 1;
        let st = crate::controls::state();
        let radio_on = if wifi { st.wifi } else { st.bt };
        let lists = crate::connect::lists();
        let g = gfx::g();
        // ---- Header: back, title, status, on/off switch ----
        let back = rf(bx - 2.0, by - 3.0, 26.0, 26.0);
        if self.hot == Some(Hit::CtlBack) {
            self.surf.fill_rr(back, 8.0, self.fg(0.1 * a));
        }
        self.surf.text("\u{E72B}", &self.f.icon_s, back, self.fg(0.8 * a));
        self.hits.push((Hit::CtlBack, back));
        let title = tr(if wifi { "Wi-Fi" } else { "Bluetooth" });
        self.surf.text(title, &self.f.title, rf(bx + 30.0, by - 3.0, 160.0, 24.0), self.fg(a));
        let tw = g.measure(title, &self.f.title);
        let fresh = self.ctl_note.as_ref().filter(|n| n.2.elapsed().as_secs_f32() < 5.0).map(|n| (n.0.clone(), n.1));
        let (status, scol) = if let Some((m, ok)) = fresh {
            (m, if ok { self.ic(pal::GREEN, a) } else { rgb(pal::RED, a) })
        } else if lists.busy.is_some() {
            (tr("Connecting…").to_string(), self.fg(0.55 * a))
        } else if radio_on == Some(true) && (if wifi { lists.wifi_scanning } else { lists.bt_scanning }) {
            (tr("Searching…").to_string(), self.fg(0.45 * a))
        } else {
            (String::new(), self.fg(0.0))
        };
        if !status.is_empty() {
            self.surf.text(&status, &self.f.tiny, rf(bx + 40.0 + tw, by - 2.0, bw - tw - 110.0, 22.0), scol);
        }
        if let Some(on) = radio_on {
            let id = if wifi { 0 } else { 1 };
            let sw = rf(bx + bw - 44.0, by - 1.0, 44.0, 22.0);
            let col = self.tile_col(if wifi { pal::CYAN } else { pal::BLUE }, 1.0);
            if on {
                self.surf.fill_rr(sw, 11.0, with_alpha(col, a));
            } else {
                self.surf.stroke_rr(rf(sw.left + 0.75, sw.top + 0.75, 42.5, 20.5), 10.25, self.fg(0.45 * a), 1.5);
            }
            let kx = if on { sw.right - 11.0 } else { sw.left + 11.0 };
            self.surf.fill_circle(kx, sw.top + 11.0, 6.5, if on { Island::text_on(col, a) } else { self.fg(0.75 * a) });
            self.hits.push((Hit::Ctl(id), rf(sw.left - 6.0, sw.top - 4.0, 56.0, 30.0)));
        }
        // ---- Rows ----
        let top = by + 28.0;
        let bottom = by + bh.max(140.0);
        let rh = 30.0;
        let n = if radio_on == Some(false) { 0 } else if wifi { lists.wifi.len() } else { lists.bt.len() };
        let empty_line = n == 0;
        // Still looking: a line under what's listed so far.
        let scanning = radio_on != Some(false) && (if wifi { lists.wifi_scanning } else { lists.bt_scanning });
        let more_line = scanning && !empty_line;
        // Windows 11 24H2+: nearby networks need location on for desktop apps.
        let banner = if wifi && lists.wifi_denied && radio_on != Some(false) { 50.0 } else { 0.0 };
        let total = (if empty_line { 1 } else { n + more_line as usize }) as f32 * rh + 36.0 + banner;
        let max_scroll = (total - (bottom - top)).max(0.0);
        self.ctl_scroll = self.ctl_scroll.min(max_scroll);
        let y0 = top - self.ctl_scroll + banner;
        self.surf.clip(rf(bx - 6.0, top - 2.0, bw + 12.0, bottom - top + 2.0));
        if banner > 0.0 {
            let br = rf(bx - 4.0, y0 - banner, bw + 4.0, banner - 6.0);
            let org = self.ic(pal::ORANGE, 1.0);
            self.surf.fill_rr(br, 10.0, with_alpha(org, 0.14 * a));
            self.surf.text("\u{E81D}", &self.f.icon, rf(br.left + 8.0, br.top, 22.0, br.bottom - br.top), with_alpha(org, a));
            let bw_ = g.measure(tr("Location settings"), &self.f.tiny) + 24.0;
            self.surf.text_wrap(tr("Windows hides nearby Wi-Fi from apps until location is on: turn on Location and \"Let desktop apps access your location\". The list fills in by itself."), &self.f.wrap_src, rf(br.left + 36.0, br.top + 4.0, br.right - br.left - 50.0 - bw_, br.bottom - br.top - 6.0), self.fg(0.8 * a));
            let bb = rf(br.right - bw_ - 8.0, br.top + (br.bottom - br.top - 24.0) / 2.0, bw_, 24.0);
            let hot = self.hot == Some(Hit::LocSettings);
            self.surf.fill_rr(bb, 12.0, with_alpha(org, if hot { 0.9 } else { 0.75 } * a));
            self.surf.text(tr("Location settings"), &self.f.tiny_c, bb, Island::text_on(org, a));
            self.hits.push((Hit::LocSettings, bb));
        }
        let vis = |r: D2D_RECT_F| -> Option<D2D_RECT_F> {
            let t = r.top.max(top);
            let b = r.bottom.min(bottom);
            (b - t > 8.0).then(|| D2D_RECT_F { left: r.left, top: t, right: r.right, bottom: b })
        };
        let stagger = |i: usize| {
            let ti = ((e * 1.35) - i.min(6) as f32 * 0.07).clamp(0.0, 1.0);
            ti * ti * (3.0 - 2.0 * ti)
        };
        if empty_line {
            let msg = match (radio_on, wifi) {
                (Some(false), true) => "Wi-Fi is off",
                (Some(false), false) => "Bluetooth is off",
                (_, true) if lists.wifi_scanning => "Searching…",
                (_, false) if lists.bt_scanning => "Searching…",
                (_, true) => "No networks found",
                (_, false) => "No devices found",
            };
            self.surf.text(tr(msg), &self.f.body, rf(bx + 6.0, y0, bw, rh), self.fg(0.5 * a));
        }
        for i in 0..n {
            let ti = stagger(i);
            let a = a * ti;
            let r = rf(bx - 4.0 + 24.0 * (1.0 - ti), y0 + i as f32 * rh, bw + 4.0, rh - 3.0);
            if r.bottom < top || r.top > bottom {
                continue;
            }
            let (hit, glyph, name, right, active, accent) = if wifi {
                let w = &lists.wifi[i];
                let glyph = match w.signal {
                    0..=24 => "\u{E872}",
                    25..=49 => "\u{E873}",
                    50..=74 => "\u{E874}",
                    _ => "\u{E701}",
                };
                let right = if lists.busy.as_deref() == Some(w.ssid.as_str()) {
                    tr("Connecting…")
                } else if w.connected {
                    tr("Connected")
                } else if !w.profile.is_empty() {
                    tr("Saved")
                } else {
                    ""
                };
                (Hit::WifiNet(i), glyph, w.ssid.clone(), right, w.connected, pal::CYAN)
            } else {
                let b = &lists.bt[i];
                let right = if lists.busy.as_deref() == Some(b.name.as_str()) {
                    tr("Connecting…")
                } else if b.connected {
                    tr("Connected")
                } else if b.paired {
                    tr("Not connected")
                } else {
                    tr("Pair")
                };
                (Hit::BtItem(i), b.kind.glyph(), b.name.clone(), right, b.connected, b.kind.color())
            };
            let hot = self.hot == Some(hit);
            // Clicking a name picks the row (it never connects by itself).
            let picked = self.ctl_sel.as_deref() == Some(name.as_str());
            if active {
                self.surf.fill_rr(r, 9.0, self.el(accent, 0.2 * a));
                self.surf.fill_rr(rf(r.left + 1.0, r.top + 7.0, 3.0, rh - 17.0), 1.5, self.el(accent, a));
            } else if picked {
                self.surf.fill_rr(r, 9.0, self.fg(0.1 * a));
            } else if hot {
                self.surf.fill_rr(r, 9.0, self.fg(0.06 * a));
            }
            if picked {
                self.surf.stroke_rr(rf(r.left + 0.5, r.top + 0.5, r.right - r.left - 1.0, r.bottom - r.top - 1.0), 9.0, self.ic(accent, 0.8 * a), 1.3);
            }
            let gcol = if active { self.ic(accent, a) } else { self.fg(0.75 * a) };
            self.surf.text(glyph, &self.f.icon_s, rf(r.left + 8.0, r.top, 18.0, rh - 3.0), gcol);
            let pw_row = wifi && self.wifi_pw_for.as_deref() == Some(name.as_str());
            let name_w = if pw_row { (bw * 0.34).min(170.0) } else { bw - 150.0 };
            self.surf.text(&name, &self.f.body, rf(r.left + 32.0, r.top, name_w, rh - 3.0), self.fg(if active { 1.0 } else { 0.88 } * a));
            if let Some(v) = vis(r) {
                self.hits.push((hit, v));
            }
            if pw_row {
                let fx = r.left + 38.0 + name_w;
                let fw = r.right - fx - 40.0;
                if self.wifi_ent {
                    // Work / school: user name and password side by side.
                    let half = (fw - 6.0) / 2.0;
                    let user = self.wifi_user.clone();
                    self.rdp_field(rf(fx, r.top + 1.0, half, rh - 5.0), EditTarget::WifiUser, tr("User name"), &user, false, a, Hit::WifiUser);
                    let pass = self.wifi_pass.clone();
                    self.rdp_field(rf(fx + half + 6.0, r.top + 1.0, half, rh - 5.0), EditTarget::WifiPass, tr("Password"), &pass, true, a, Hit::WifiPass);
                } else {
                    let fr = rf(fx, r.top + 1.0, fw, rh - 5.0);
                    self.draw_field(fr, EditTarget::WifiPass, tr("Password"), a, Hit::WifiPass);
                }
                let jr = rf(r.right - 34.0, r.top + 1.0, 32.0, rh - 5.0);
                let jc = self.tile_col(pal::CYAN, 1.0);
                self.surf.fill_rr(jr, 12.0, with_alpha(jc, if self.hot == Some(Hit::WifiJoin) { 1.0 } else { 0.88 } * a));
                self.surf.text("\u{E72A}", &self.f.icon_s, jr, Island::text_on(jc, a));
                if vis(jr).is_some() {
                    self.hits.push((Hit::WifiJoin, jr));
                }
                continue;
            }
            // Right side: a Connect / Disconnect / Pair button (only it
            // connects), then the state and a lock for secured networks.
            let busy = lists.busy.as_deref() == Some(name.as_str());
            let (act, act_hit) = if wifi {
                let w = &lists.wifi[i];
                (if busy { "" } else if w.connected { tr("Disconnect") } else { tr("Connect") }, Hit::WifiAct(i))
            } else {
                let b = &lists.bt[i];
                (if busy { "" } else if !b.paired { tr("Pair") } else if b.connected { tr("Disconnect") } else { tr("Connect") }, Hit::BtAct(i))
            };
            let mut rx = r.right - 6.0;
            if !act.is_empty() {
                let bw_ = g.measure(act, &self.f.tiny) + 20.0;
                let br = rf(rx - bw_, r.top + 4.0, bw_, rh - 11.0);
                let hot_b = self.hot == Some(act_hit);
                if active {
                    self.surf.fill_rr(br, (rh - 11.0) / 2.0, self.fg(if hot_b { 0.2 } else { 0.1 } * a));
                    self.surf.text(act, &self.f.tiny_c, br, self.fg(0.9 * a));
                } else {
                    self.surf.fill_rr(br, (rh - 11.0) / 2.0, self.el(pal::BLUE, if hot_b { 0.34 } else { 0.2 } * a));
                    self.surf.text(act, &self.f.tiny_c, br, self.ic(pal::BLUE, a));
                }
                if let Some(v) = vis(br) {
                    self.hits.push((act_hit, v));
                }
                rx = br.left - 6.0;
            }
            let lock = wifi && lists.wifi[i].secure;
            if lock {
                self.surf.text("\u{E72E}", &self.f.icon_s, rf(rx - 16.0, r.top, 16.0, rh - 3.0), self.fg(0.45 * a));
                rx -= 20.0;
            }
            let failed_here = lists.failed.as_ref().map(|f| f.0 == name && f.1.elapsed().as_secs() < 6).unwrap_or(false) && !busy;
            let state = if failed_here { tr("Couldn't connect") } else if busy || active || (wifi && right == tr("Saved")) { right } else { "" };
            if busy {
                // Connecting: breathing dots where the button was.
                self.thinking_dots(rx - 14.0, (r.top + r.bottom) / 2.0 - 1.5, 5.0, a);
                rx -= 30.0;
            }
            if !state.is_empty() {
                let col = if failed_here { rgb(pal::RED, 0.95 * a) } else if active { self.ic(accent, a) } else { self.fg(0.5 * a) };
                self.surf.text(state, &self.f.tiny_r, rf(rx - 120.0, r.top, 120.0, rh - 3.0), col);
            }
        }
        if more_line {
            let ty = y0 + n as f32 * rh;
            self.thinking_dots(bx + 16.0, ty + (rh - 3.0) / 2.0, 6.0, a);
            self.thinking_text(tr(if wifi { "Looking for more networks…" } else { "Looking for more devices…" }), rf(bx + 32.0, ty, bw - 40.0, rh - 3.0), a * 0.8);
        }
        // ---- "Wi-Fi settings" / "Bluetooth settings" ----
        let i = if empty_line { 1 } else { n + more_line as usize };
        let ti = stagger(i);
        let a = a * ti;
        let label = tr(if wifi { "Wi-Fi settings" } else { "Bluetooth settings" });
        let w = g.measure(label, &self.f.tiny) + 52.0;
        let r = rf(bx + (bw - w) / 2.0, y0 + i as f32 * rh + 4.0 + 16.0 * (1.0 - ti), w, 26.0);
        let hot = self.hot == Some(Hit::CtlSettings);
        self.surf.fill_rr(r, 13.0, self.fg(if hot { 0.16 } else { 0.08 } * a));
        self.surf.text("\u{E713}", &self.f.icon_s, rf(r.left + 10.0, r.top, 16.0, 26.0), self.fg(0.85 * a));
        self.surf.text(label, &self.f.tiny, rf(r.left + 30.0, r.top, w - 34.0, 26.0), self.fg(0.85 * a));
        self.surf.text("\u{E76C}", &self.f.icon_s, rf(r.right - 20.0, r.top, 12.0, 26.0), self.fg(0.5 * a));
        if let Some(v) = vis(r) {
            self.hits.push((Hit::CtlSettings, v));
        }
        self.surf.unclip();
        // Scroll hint: a thin bar on the right.
        if max_scroll > 0.0 {
            let vh = bottom - top;
            let bh_ = (vh * vh / total).max(18.0);
            let by_ = top + (vh - bh_) * (self.ctl_scroll / max_scroll);
            self.surf.fill_rr(rf(bx + bw + 4.0, by_, 3.0, bh_), 1.5, self.fg(0.25 * a));
        }
    }

    /// Lock, sleep, shut down or restart this PC (the last two ask once more).
    fn power_list(&mut self, bx: f32, by: f32, bw: f32, bh: f32, a: f32, e: f32) {
        let back = rf(bx - 2.0, by - 3.0, 26.0, 26.0);
        if self.hot == Some(Hit::CtlBack) {
            self.surf.fill_rr(back, 8.0, self.fg(0.1 * a));
        }
        self.surf.text("\u{E72B}", &self.f.icon_s, back, self.fg(0.8 * a));
        self.hits.push((Hit::CtlBack, back));
        self.surf.text(tr("Power"), &self.f.title, rf(bx + 30.0, by - 3.0, 200.0, 24.0), self.fg(a));
        let armed = self.power_arm.filter(|(_, t)| t.elapsed() < std::time::Duration::from_secs(4)).map(|(k, _)| k);
        if armed.is_some() {
            self.keep_anim = true;
        }
        let items: [(u8, &str, &str, u32); 4] = [(0, "\u{E72E}", "Lock", pal::BLUE), (1, "\u{E708}", "Sleep", pal::INDIGO), (2, "\u{E7E8}", "Shut down", pal::RED), (3, "\u{E777}", "Restart", pal::ORANGE)];
        let top = by + 30.0;
        let th = (bh - 34.0).clamp(70.0, 120.0);
        let gap = 8.0;
        let tw = (bw - gap * 3.0) / 4.0;
        for (i, (k, glyph, label, col)) in items.iter().enumerate() {
            let ti = ((e * 1.35) - i as f32 * 0.07).clamp(0.0, 1.0);
            let ti = ti * ti * (3.0 - 2.0 * ti);
            let a = a * ti;
            let r = rf(bx + i as f32 * (tw + gap), top + 14.0 * (1.0 - ti), tw, th);
            let hot = self.hot == Some(Hit::PowerDo(*k));
            let arm = armed == Some(*k);
            if arm {
                self.surf.fill_rr(r, 14.0, self.el(*col, 0.3 * a));
                self.surf.stroke_rr(r, 14.0, self.ic(*col, 0.9 * a), 1.5);
            } else {
                self.surf.fill_rr(r, 14.0, self.fg(if hot { 0.14 } else { 0.07 } * a));
            }
            let mid = r.top + th * 0.4;
            self.surf.fill_circle((r.left + r.right) / 2.0, mid, 19.0, self.el(*col, if hot || arm { 0.9 } else { 0.22 } * a));
            self.surf.text(glyph, &self.f.icon_ctl, rf(r.left, mid - 19.0, tw, 38.0), if hot || arm { rgb(0xffffff, a) } else { self.ic(*col, a) });
            let text = if arm { tr("Click again") } else { tr(label) };
            self.surf.text(text, &self.f.tiny_c, rf(r.left + 2.0, r.bottom - 26.0, tw - 4.0, 20.0), if arm { self.ic(*col, a) } else { self.fg(0.88 * a) });
            self.hits.push((Hit::PowerDo(*k), r));
        }
    }

    /// Every installed keyboard; click one to type with it.
    fn keyboard_list(&mut self, bx: f32, by: f32, bw: f32, bh: f32, a: f32, e: f32) {
        let g = gfx::g();
        let back = rf(bx - 2.0, by - 3.0, 26.0, 26.0);
        if self.hot == Some(Hit::CtlBack) {
            self.surf.fill_rr(back, 8.0, self.fg(0.1 * a));
        }
        self.surf.text("\u{E72B}", &self.f.icon_s, back, self.fg(0.8 * a));
        self.hits.push((Hit::CtlBack, back));
        let title = tr("Keyboard");
        self.surf.text(title, &self.f.title, rf(bx + 30.0, by - 3.0, 200.0, 24.0), self.fg(a));
        let tw = g.measure(title, &self.f.title);
        self.surf.text(tr("Click one to type with it"), &self.f.tiny, rf(bx + 40.0 + tw, by - 2.0, bw - tw - 60.0, 22.0), self.fg(0.45 * a));
        let cur = crate::sysmon::current_hkl();
        let cur_lang = (cur as usize & 0xffff) as u16;
        let list = self.kb_list.clone();
        let top = by + 28.0;
        let bottom = by + bh.max(140.0);
        let rh = 30.0;
        let n = list.len();
        let total = n.max(1) as f32 * rh + 36.0;
        let max_scroll = (total - (bottom - top)).max(0.0);
        self.ctl_scroll = self.ctl_scroll.min(max_scroll);
        let y0 = top - self.ctl_scroll;
        self.surf.clip(rf(bx - 6.0, top - 2.0, bw + 12.0, bottom - top + 2.0));
        let accent = pal::INDIGO;
        for (i, k) in list.iter().enumerate() {
            let ti = ((e * 1.35) - i.min(6) as f32 * 0.07).clamp(0.0, 1.0);
            let ti = ti * ti * (3.0 - 2.0 * ti);
            let a = a * ti;
            let r = rf(bx - 4.0 + 24.0 * (1.0 - ti), y0 + i as f32 * rh, bw + 4.0, rh - 3.0);
            if r.bottom < top || r.top > bottom {
                continue;
            }
            // Same keyboard, or (fake list) same position.
            let active = k.hkl == cur || (list.iter().all(|x| x.hkl != cur) && k.lang == cur_lang);
            let hot = self.hot == Some(Hit::KbItem(i));
            if active {
                self.surf.fill_rr(r, 9.0, self.el(accent, 0.2 * a));
                self.surf.fill_rr(rf(r.left + 1.0, r.top + 7.0, 3.0, rh - 17.0), 1.5, self.el(accent, a));
            } else if hot {
                self.surf.fill_rr(r, 9.0, self.fg(0.08 * a));
            }
            let badge = rf(r.left + 8.0, r.top + 4.0, 30.0, rh - 11.0);
            self.surf.fill_rr(badge, 6.0, if active { self.el(accent, 0.9 * a) } else { self.fg(0.1 * a) });
            let bcol = if active { Island::text_on(self.ic(accent, 1.0), a) } else { self.fg(0.8 * a) };
            self.surf.text(&k.short, &self.f.tiny_c, badge, bcol);
            self.surf.text(&k.name, &self.f.body, rf(r.left + 46.0, r.top, bw * 0.55, rh - 3.0), self.fg(if active { 1.0 } else { 0.88 } * a));
            let right = if active { format!("{}  ·  {}", k.layout, tr("In use")) } else { k.layout.clone() };
            self.surf.text(&right, &self.f.tiny_r, rf(r.right - bw * 0.42 - 8.0, r.top, bw * 0.42, rh - 3.0), if active { self.ic(accent, a) } else { self.fg(0.5 * a) });
            let v = rf(r.left, r.top.max(top), r.right - r.left, r.bottom.min(bottom) - r.top.max(top));
            if v.bottom - v.top > 8.0 {
                self.hits.push((Hit::KbItem(i), v));
            }
        }
        if n == 0 {
            self.surf.text(tr("No keyboards found"), &self.f.body, rf(bx + 6.0, y0, bw, rh), self.fg(0.5 * a));
        }
        // Language settings at the end.
        let label = tr("Language settings");
        let w = g.measure(label, &self.f.tiny) + 52.0;
        let r = rf(bx + (bw - w) / 2.0, y0 + n.max(1) as f32 * rh + 4.0, w, 26.0);
        let hot = self.hot == Some(Hit::CtlSettings);
        self.surf.fill_rr(r, 13.0, self.fg(if hot { 0.16 } else { 0.08 } * a));
        self.surf.text("\u{E713}", &self.f.icon_s, rf(r.left + 10.0, r.top, 16.0, 26.0), self.fg(0.85 * a));
        self.surf.text(label, &self.f.tiny, rf(r.left + 30.0, r.top, w - 34.0, 26.0), self.fg(0.85 * a));
        if r.bottom <= bottom + 2.0 && r.top >= top - 2.0 {
            self.hits.push((Hit::CtlSettings, r));
        }
        self.surf.unclip();
        if max_scroll > 0.0 {
            let vh = bottom - top;
            let bh_ = (vh * vh / total).max(18.0);
            let by_ = top + (vh - bh_) * (self.ctl_scroll / max_scroll);
            self.surf.fill_rr(rf(bx + bw + 4.0, by_, 3.0, bh_), 1.5, self.fg(0.25 * a));
        }
    }

    /// Big tiles on top, sliders in the middle, shortcuts along the bottom —
    /// spread over the whole page height.
    fn controls_main(&mut self, bx: f32, by: f32, bw: f32, bh: f32, a: f32) {
        let st = crate::controls::state();
        let d = crate::data();
        let g = gfx::g();
        // Row of toggle tiles.
        let wifi_label = d.net.wifi.as_ref().map(|w| w.ssid.clone()).filter(|_| st.wifi == Some(true)).unwrap_or_else(|| tr("Wi-Fi").to_string());
        let tiles: [(u8, &str, String, u32, Option<bool>); 6] = [
            (0, "\u{E701}", wifi_label, pal::CYAN, st.wifi),
            (1, "\u{E702}", tr("Bluetooth").to_string(), pal::BLUE, st.bt),
            (2, "\u{E709}", if crate::i18n::vi() { "Máy bay".into() } else { tr("Airplane mode").to_string() }, pal::ORANGE, st.airplane.or(Some(false))),
            // Keyboard: the badge shows the one in use ("EN", "VI"…).
            (3, "\u{E765}", tr("Keyboard").to_string(), pal::INDIGO, Some(true)),
            // "Mute mic": on = muted, off = the microphone works normally.
            (4, if st.mic_muted == Some(true) { "\u{EC54}" } else { "\u{E720}" }, tr("Mute mic").to_string(), pal::RED, st.mic_muted),
            (5, "\u{E793}", if crate::i18n::vi() { "Chế độ tối".into() } else { tr("Dark mode").to_string() }, pal::PURPLE, st.dark),
        ];
        let gap = 10.0;
        let tw = (bw - gap * 5.0) / 6.0;
        let th = 74.0;
        for (i, (id, glyph, label, col, on)) in tiles.iter().enumerate() {
            let r = rf(bx + i as f32 * (tw + gap), by, tw, th);
            let avail = on.is_some();
            let is_on = on.unwrap_or(false);
            let hot = self.hot == Some(Hit::Ctl(*id));
            let ta = if avail { a } else { 0.35 * a };
            let bg = self.tile_col(*col, 1.0);
            if is_on {
                self.surf.fill_rr(r, 14.0, with_alpha(bg, (if hot { 1.0 } else { 0.9 }) * ta));
            } else {
                self.surf.fill_rr(r, 14.0, self.fg(if hot { 0.14 } else { 0.08 } * ta));
            }
            let fgc = if is_on { Island::text_on(bg, ta) } else { self.fg(0.9 * ta) };
            if *id == 3 {
                let (_, short) = crate::sysmon::lang_name((crate::sysmon::current_hkl() as usize & 0xffff) as u16);
                let short = if short.is_empty() { "⌨".to_string() } else { short };
                self.surf.text(&short, &self.f.small_c, rf(r.left, r.top + 10.0, tw, 32.0), fgc);
            } else {
                self.surf.text(glyph, &self.f.icon_tile, rf(r.left, r.top + 10.0, tw, 32.0), fgc);
            }
            self.surf.text(label, &self.f.tiny_c, rf(r.left + 4.0, r.top + 46.0, tw - 8.0, 20.0), if is_on { Island::text_on(bg, 0.95 * ta) } else { self.fg(0.75 * ta) });
            if avail {
                self.hits.push((Hit::Ctl(*id), r));
            }
            // Wi-Fi / Bluetooth: a chevron opens the list of networks / devices.
            if avail && *id <= 1 {
                let cr = rf(r.right - 24.0, r.top + 4.0, 20.0, 20.0);
                let hot_c = self.hot == Some(Hit::CtlList(*id));
                if hot_c {
                    self.surf.fill_rr(cr, 7.0, if is_on { Island::text_on(bg, 0.18 * ta) } else { self.fg(0.12 * ta) });
                }
                self.surf.text("\u{E76C}", &self.f.icon_s, cr, if is_on { Island::text_on(bg, 0.85 * ta) } else { self.fg(0.55 * ta) });
                self.hits.push((Hit::CtlList(*id), rf(r.right - 28.0, r.top, 28.0, 28.0)));
            }
        }
        // Sliders: brightness (built-in screens) and volume, spaced evenly
        // between the tiles and the bottom bar.
        let yb = by + bh - 30.0;
        let bright = self.bright_ui.or(st.brightness);
        let n_sl = 1 + bright.is_some() as i32;
        let step = ((yb - (by + th)) / (n_sl as f32 + 1.0)).max(26.0);
        let mut y = by + th + step - 2.0;
        // With several speakers playing together the volume row ends in a
        // button that opens their own sliders.
        let chip_w = if self.mo_cluster.is_empty() { 0.0 } else { Island::CHIP_W + 8.0 };
        let slider = |isl: &mut Island, y: f32, glyph: &str, f: f32, label: String, col: u32, hit: Hit, icon_hit: Option<Hit>, res: f32| {
            let ir = rf(bx, y - 15.0, 30.0, 30.0);
            if let Some(h) = icon_hit {
                if isl.hot == Some(h) {
                    isl.surf.fill_circle(bx + 15.0, y, 15.0, isl.fg(0.1 * a));
                }
                isl.hits.push((h, ir));
            }
            // The speaker is drawn (centred on what's visible); the rest are glyphs.
            match glyph.strip_prefix("spk").and_then(|n| n.parse::<u8>().ok()) {
                Some(lv) => isl.speaker(bx + 15.0, y, 18.0, lv, isl.ic(col, a)),
                None => isl.surf.text(glyph, &isl.f.icon_m, ir, isl.ic(col, a)),
            }
            let (tx0, tw) = (bx + 42.0, bw - 42.0 - 54.0 - res);
            isl.surf.fill_rr(rf(tx0, y - 4.0, tw, 8.0), 4.0, isl.fg(0.14 * a));
            isl.surf.fill_rr(rf(tx0, y - 4.0, (tw * f).max(8.0), 8.0), 4.0, isl.el(col, a));
            isl.surf.fill_circle(tx0 + tw * f, y, if isl.hot == Some(hit) { 10.0 } else { 8.5 }, rgb(0xffffff, a));
            isl.hits.push((hit, rf(tx0 - 10.0, y - 14.0, tw + 20.0, 28.0)));
            isl.surf.text(&label, &isl.f.tiny_r, rf(tx0 + tw + 6.0, y - 9.0, 48.0, 18.0), isl.fg(0.75 * a));
        };
        if let Some(b) = bright {
            slider(self, y, "\u{E706}", b as f32 / 100.0, format!("{b}%"), pal::YELLOW, Hit::Bright, None, chip_w);
            y += step;
        }
        let (vol, muted) = self.shown_volume();
        let vg = ["spk0", "spk1", "spk2", "spk3"][speaker_level(vol, muted) as usize];
        let vl = if muted { tr("Muted").to_string() } else { format!("{}%", (vol * 100.0).round() as i32) };
        slider(self, y, vg, if muted { 0.0 } else { vol }, vl, pal::BLUE, Hit::Volume, Some(Hit::Mute), chip_w);
        self.cluster_chip(bx + bw, y, a);
        // Bottom bar: battery, then shortcuts to Windows' own panels.
        // Battery and energy saver; click → Settings › Power & battery.
        if let Some((pct, charging)) = d.battery {
            let g = gfx::g();
            let col = battery_color(pct, charging, self.dark);
            let t = if charging { format!("{pct}%  ·  {}", tr("Charging")) } else { format!("{pct}%") };
            let tw = g.measure(&t, &self.f.tiny);
            let on = d.energy_saver;
            let sl = tr(if on { "Saver on" } else { "Saver off" });
            let sw = g.measure(sl, &self.f.tiny);
            let r = rf(bx - 4.0, yb, 26.0 + tw + 22.0 + sw + 14.0, 28.0);
            if self.hot == Some(Hit::PowerSettings) {
                self.surf.fill_rr(r, 14.0, self.fg(0.1 * a));
            }
            self.surf.ring(bx + 10.0, yb + 14.0, 8.5, 2.6, pct as f32 / 100.0, self.fg(0.16 * a), with_alpha(col, a));
            self.surf.text(&t, &self.f.tiny, rf(bx + 26.0, yb, tw + 4.0, 28.0), self.fg(0.75 * a));
            let lx = bx + 26.0 + tw + 14.0;
            let lc = if on { self.ic(pal::GREEN, a) } else { self.fg(0.4 * a) };
            draw_leaf(&self.surf, lx, yb + 14.0, 5.5, lc);
            self.surf.text(sl, &self.f.tiny, rf(lx + 10.0, yb, sw + 4.0, 28.0), if on { lc } else { self.fg(0.55 * a) });
            self.hits.push((Hit::PowerSettings, r));
        } else {
            // No battery level to read: a battery with an ✕.
            let r = rf(bx - 4.0, yb, 38.0, 28.0);
            if self.hot == Some(Hit::PowerSettings) {
                self.surf.fill_rr(r, 14.0, self.fg(0.1 * a));
            }
            self.battery_x(bx + 4.0, yb + 14.0, a);
            self.hits.push((Hit::PowerSettings, r));
        }
        let dev = if enabled_pages().contains(&Page::Devices) { (11u8, "\u{E772}", "Devices") } else { (11u8, "\u{E7E7}", "Notifications") };
        let btns: [(u8, &str, &str); 3] = [(10, "\u{E9E9}", "Quick Settings"), dev, (12, "\u{E713}", "Settings")];
        let mut x = bx + bw;
        for (id, glyph, label) in btns.iter().rev() {
            let label = tr(label);
            let w = g.measure(label, &self.f.tiny) + 40.0;
            x -= w;
            let r = rf(x, yb, w, 28.0);
            let hot = self.hot == Some(Hit::Ctl(*id));
            self.surf.fill_rr(r, 14.0, self.fg(if hot { 0.16 } else { 0.08 } * a));
            self.surf.text(glyph, &self.f.icon_s, rf(x + 10.0, yb, 16.0, 28.0), self.fg(0.85 * a));
            self.surf.text(label, &self.f.tiny, rf(x + 30.0, yb, w - 32.0, 28.0), self.fg(0.85 * a));
            self.hits.push((Hit::Ctl(*id), r));
            x -= 6.0;
        }
    }

    // ------------------------------------------------------------------
    // Calendar (Outlook / any ICS feed)
    // ------------------------------------------------------------------

    /// 3 : 7 — the time (with seconds), the date in both calendars and your
    /// city on the left; the month on the right, solar or lunar, each day
    /// showing its date in the other calendar too. Click a day to see it in
    /// full on the left.
    pub(super) fn page_calendar(&mut self, bx: f32, by: f32, bw: f32, bh: f32, a: f32) {
        let c = config::cur();
        let vi = crate::i18n::vi();
        let lunar_mode = c.i("cal_lunar") == 1;
        let now = util::unix_now();
        let today = util::local_day(now);
        let (ty, tm, _) = util::civil_from_days(today);
        let month = self.cal_month.unwrap_or(ty * 12 + tm as i64 - 1);
        let (y, m) = (month.div_euclid(12), (month.rem_euclid(12) + 1) as u32);
        let g = gfx::g();
        let accent = self.ic(pal::MINT, 1.0);
        let moon_c = self.ic(pal::ORANGE, 1.0);

        // ---- Left 32 %: time, the day in both calendars, city — or, for a
        // picked day, everything the calendar says about it ----
        let lw = (bw * 0.32).round();
        let k = c.i("home_clock_size").clamp(60, 160) as f32 / 100.0;
        let st = util::now();
        let (t, suf) = util::clock_parts(&st, c.i("clock_fmt"));
        let t = format!("{t}:{:02}", st.wSecond);
        let (sz, f) = self.clock_font(&t, lw, 40.0 * k);
        let tw = g.measure(&t, &f);
        let th = sz * 1.2;
        let show_date = c.b("home_date");
        let place = c.s("weather_place");
        let city = place.split(',').next().unwrap_or("").trim().to_string();
        let city = if city.is_empty() { c.s("weather_city").split(',').next().unwrap_or("").trim().to_string() } else { city };
        // A picked day's details fade in over the clock (and out again).
        let fe = {
            let t = self.cal_fade.v.clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        if fe > 0.01 {
            if let Some(d) = self.cal_shown {
                self.cal_detail(bx, by, lw, bh, a * fe, d);
            }
        }
        let day = today;
        let picked = false;
        let block = th + if suf.is_empty() { 0.0 } else { 16.0 } + if show_date { 22.0 + 36.0 + 20.0 } else { 36.0 } + 22.0;
        let mut yy = by + (bh - block) / 2.0 - 4.0;
        if fe < 0.99 {
            let a = a * (1.0 - fe);
            let x0 = bx + (lw - tw) / 2.0;
            self.surf.text(&t, &f, rf(x0 - 1.0, yy, tw + 6.0, th), self.fg(0.96 * a));
            yy += th;
            if !suf.is_empty() {
                self.surf.text(&suf, &self.f.tiny_c, rf(bx, yy - 4.0, lw, 16.0), self.fg(0.5 * a));
                yy += 16.0;
            }
            self.surf.fill_rect(rf(bx + lw / 2.0 - 12.0, yy + 4.0, 24.0, 1.0), self.fg(0.2 * a));
            yy += 10.0;
            if show_date || picked {
                // The (picked) day — click it to come back to today.
                let date = util::date_fmt(&util::systime(day * 86400), "dddd, d MMMM");
                let dr = rf(bx - 6.0, yy, lw + 12.0, 18.0);
                if picked {
                    if self.hot == Some(Hit::CalOpen) {
                        self.surf.fill_rr(dr, 9.0, self.fg(0.08 * a));
                    }
                    self.hits.push((Hit::CalOpen, dr));
                }
                self.surf.text(&date, &self.f.day, dr, if picked { with_alpha(accent, a) } else { self.fg(0.7 * a) });
                yy += 20.0;
            }
            // The same day in the lunar calendar.
            let l = crate::lunar::of(day);
            let lunar = format!("{} {}/{}{} · {}", tr("Lunar calendar"), l.day, l.month, if l.leap { format!(" {}", tr("(leap)")) } else { String::new() }, crate::lunar::year_name(l.year));
            let lwid = g.measure(&lunar, &self.f.date).min(lw + 8.0);
            let lx = bx + (lw - lwid - 14.0) / 2.0;
            draw_moon_glyph(&self.surf, lx + 5.0, yy + 8.0, 4.5, l.day, with_alpha(moon_c, a), self.fg(0.18 * a));
            self.surf.text(&lunar, &self.f.date, rf(lx + 14.0, yy, lwid + 4.0, 16.0), self.fg(0.75 * a));
            yy += 17.0;
            let cc = format!("{} {}", tr("Day"), crate::lunar::day_name(day));
            self.surf.text(&cc, &self.f.tiny_c, rf(bx - 6.0, yy, lw + 12.0, 15.0), self.fg(0.42 * a));
            yy += 17.0;
            // Today: hoàng đạo or hắc đạo, with its spirit.
            {
                let (god, good) = crate::lunar::GODS[crate::lunar::info(day).god];
                let t = format!("{} · {god}", tr(if good { "Lucky day" } else { "Unlucky day" }));
                let col = if good { self.ic(pal::GREEN, 1.0) } else { rgb(pal::RED, 1.0) };
                let tw = g.measure(&t, &self.f.tiny).min(lw - 24.0);
                let pw = tw + 30.0;
                let pr = rf(bx + (lw - pw) / 2.0, yy, pw, 18.0);
                self.surf.fill_rr(pr, 9.0, with_alpha(col, 0.16 * a));
                self.surf.text(if good { "\u{E735}" } else { "\u{E7BA}" }, &self.f.icon_s, rf(pr.left + 7.0, pr.top, 14.0, 18.0), with_alpha(col, a));
                self.surf.text(&ellip(g, &t, &self.f.tiny, tw + 2.0), &self.f.tiny, rf(pr.left + 23.0, pr.top, tw + 4.0, 18.0), with_alpha(col, a));
                yy += 22.0;
            }
            if show_date {
                let (glyph_a, label) = if city.is_empty() { (0.3, tr("Set your city in Settings").to_string()) } else { (0.7, city) };
                let cw = g.measure(&label, &self.f.date).min(lw - 20.0);
                let cx = bx + (lw - cw - 16.0) / 2.0;
                self.surf.text("\u{E707}", &self.f.icon_s, rf(cx, yy, 12.0, 16.0), with_alpha(accent, glyph_a * a));
                self.surf.text(&label, &self.f.date, rf(cx + 16.0, yy, cw + 4.0, 16.0), self.fg(0.55 * a));
            }
        }
        let gx = bx + lw + 16.0;
        self.surf.fill_rect(rf(gx - 8.0, by + 4.0, 1.0, bh - 8.0), self.fg(0.1 * a));

        // ---- Right 70 %: the month (slides when it changes) ----
        let gw = bx + bw - gx;
        let sl = self.cal_slide.v.clamp(-1.0, 1.0);
        let full_a = a;
        let a = a * (1.0 - sl.abs()).powf(1.5);
        let gx_static = gx;
        let gx = gx + sl * gw * 0.35;
        // Solar / lunar switch, then ‹ ›.
        let modes: [&str; 2] = [tr("Solar"), tr("Lunar")];
        let seg_w: Vec<f32> = modes.iter().map(|m| g.measure(m, &self.f.tiny) + 16.0).collect();
        let total: f32 = seg_w.iter().sum::<f32>() + 4.0;
        let sx0 = gx_static + gw - 62.0 - total;
        let pill = rf(sx0, by - 2.0, total, 22.0);
        self.surf.fill_rr(pill, 11.0, self.fg(0.07 * full_a));
        let mut sx = sx0 + 2.0;
        for (i, mname) in modes.iter().enumerate() {
            let r = rf(sx, by, seg_w[i], 18.0);
            let on = (i == 1) == lunar_mode;
            if on {
                self.surf.fill_rr(r, 9.0, with_alpha(if i == 1 { moon_c } else { accent }, 0.9 * full_a));
            } else if self.hot == Some(Hit::CalMode(i as u8)) {
                self.surf.fill_rr(r, 9.0, self.fg(0.1 * full_a));
            }
            let col = if on { Island::text_on(if i == 1 { moon_c } else { accent }, full_a) } else { self.fg(0.6 * full_a) };
            self.surf.text(mname, &self.f.tiny_c, r, col);
            self.hits.push((Hit::CalMode(i as u8), r));
            sx += seg_w[i];
        }
        for (hit, glyph, x) in [(Hit::CalPrev, "\u{E76B}", gx_static + gw - 56.0), (Hit::CalNext, "\u{E76C}", gx_static + gw - 28.0)] {
            let r = rf(x, by - 4.0, 26.0, 26.0);
            if self.hot == Some(hit) {
                self.surf.fill_rr(r, 8.0, self.fg(0.1 * full_a));
            }
            self.surf.text(glyph, &self.f.icon_s, r, self.fg(0.7 * full_a));
            self.hits.push((hit, r));
        }
        self.surf.clip(rf(gx_static - 4.0, by - 6.0, gw + 8.0, bh + 12.0));
        // Which days the grid covers.
        let (first, len, title) = if lunar_mode {
            let base = self.cal_lmonth.unwrap_or(today);
            let (start, n) = crate::lunar::month_shift(base, 0);
            let ls = crate::lunar::of(start);
            let title = format!("{}{} · {}", tr("Month {n}").replace("{n}", &ls.month.to_string()), if ls.leap { format!(" {}", tr("(leap)")) } else { String::new() }, crate::lunar::year_name(ls.year));
            (start, n, title)
        } else {
            let first = util::days_from_civil(y, m as i64, 1);
            let next = if m == 12 { util::days_from_civil(y + 1, 1, 1) } else { util::days_from_civil(y, m as i64 + 1, 1) };
            (first, next - first, util::date_fmt(&util::systime(first * 86400), "MMMM yyyy"))
        };
        let tr_ = rf(gx + 4.0, by - 3.0, (sx0 - gx - 10.0).max(40.0), 22.0);
        // The month's name opens the month / year picker.
        let ttw = g.measure(&title, &self.f.title).min(tr_.right - tr_.left - 16.0);
        let hot_t = self.hot == Some(Hit::CalTitle) || self.cal_picker.is_some();
        if hot_t {
            self.surf.fill_rr(rf(tr_.left - 6.0, tr_.top, ttw + 30.0, 22.0), 8.0, self.fg(0.08 * a));
        }
        self.surf.text(&title, &self.f.title, tr_, self.fg(a));
        self.surf.text(if self.cal_picker.is_some() { "\u{E70E}" } else { "\u{E70D}" }, &self.f.icon_s, rf(tr_.left + ttw + 4.0, tr_.top + 1.0, 14.0, 22.0), self.fg(0.55 * a));
        self.hits.push((Hit::CalTitle, rf(tr_.left - 6.0, tr_.top, ttw + 30.0, 22.0)));
        let cw = gw / 7.0;
        // Weekday names in the island's language (Monday first).
        let heads: Vec<String> = if vi {
            ["T2", "T3", "T4", "T5", "T6", "T7", "CN"].iter().map(|s| s.to_string()).collect()
        } else {
            let monday = util::days_from_civil(2024, 1, 1);
            (0..7).map(|i| util::date_fmt(&util::systime((monday + i) * 86400), "ddd")).collect()
        };
        for (i, h) in heads.iter().enumerate() {
            self.surf.text(h, &self.f.tiny_c, rf(gx + i as f32 * cw, by + 22.0, cw, 14.0), self.fg(if i >= 5 { 0.3 } else { 0.45 } * a));
        }
        // Weeks start on Monday; only as many rows as the month needs.
        let lead = ((first + 4).rem_euclid(7) + 6) % 7;
        let start = first - lead;
        let rows = ((lead + len) as f32 / 7.0).ceil() as i64;
        let grid_top = by + 37.0;
        // The rows end a little above the page's edge.
        let ch = ((by + bh - 3.0 - grid_top) / rows as f32).clamp(18.0, 34.0);
        // Each day's box fills its column (both dates fit inside it).
        let cell_w = (cw - 3.0).min(ch * 2.0).max(28.0);
        // Half the height of the two dates together.
        let k = (ch / 2.0 - 1.0).clamp(8.0, 12.5);
        for row in 0..rows {
            for col in 0..7 {
                let d = start + row * 7 + col;
                let (_, dm, dd) = util::civil_from_days(d);
                let ld = crate::lunar::of(d);
                let cx = gx + col as f32 * cw + cw / 2.0;
                let cy = grid_top + row as f32 * ch + ch / 2.0;
                let in_month = d >= first && d < first + len;
                let is_today = d == today;
                let cell = rf(cx - cell_w / 2.0, cy - ch / 2.0 + 0.5, cell_w, ch - 1.0);
                // One colour at three strengths: today 100 %, hoàng đạo (lucky)
                // days 60 %, hắc đạo (unlucky) days 15 %.
                let lucky = in_month && !is_today && crate::lunar::GODS[crate::lunar::info(d).god].1;
                if !is_today && in_month {
                    self.surf.fill_rr(cell, 8.0, with_alpha(accent, if lucky { 0.6 } else { 0.15 } * a));
                }
                // What the 60 % fill looks like over the island (for the text on it).
                let lucky_bg = {
                    let b = if self.dark { 0.0 } else { 1.0 };
                    D2D1_COLOR_F { r: accent.r * 0.6 + b * 0.4, g: accent.g * 0.6 + b * 0.4, b: accent.b * 0.6 + b * 0.4, a: 1.0 }
                };
                if is_today {
                    self.surf.fill_rr(cell, 8.0, with_alpha(accent, a));
                } else if self.cal_sel == Some(d) {
                    self.surf.stroke_rr(cell, 8.0, self.fg(0.9 * a), 1.8);
                } else if self.hot == Some(Hit::CalDay(d)) {
                    self.surf.fill_rr(cell, 8.0, self.fg(0.08 * a));
                }
                // Big: this calendar's date. Small: the other calendar's.
                let (big, small, mark) = if lunar_mode {
                    let s = if dd == 1 { format!("{dd}/{dm}") } else { dd.to_string() };
                    (ld.day.to_string(), s, dd == 1)
                } else {
                    let s = if ld.day == 1 { format!("{}/{}", ld.day, ld.month) } else { ld.day.to_string() };
                    (dd.to_string(), s, ld.day == 1 || ld.day == 15)
                };
                let base = if in_month { if col >= 5 { 0.55 } else { 0.9 } } else { 0.2 };
                let col_t = if is_today {
                    Island::text_on(accent, a)
                } else if lucky {
                    Island::text_on(lucky_bg, if col >= 5 { 0.75 } else { 1.0 } * a)
                } else {
                    self.fg(base * a)
                };
                self.surf.text(&big, &self.f.body_c, rf(cx - cw / 2.0, cy - k, cw, 16.0), col_t);
                let small_c = if is_today {
                    Island::text_on(accent, 0.75 * a)
                } else if lucky {
                    Island::text_on(lucky_bg, 0.7 * a)
                } else if mark && in_month {
                    with_alpha(if lunar_mode { accent } else { moon_c }, 0.95 * a)
                } else {
                    self.fg(if in_month { 0.42 } else { 0.15 } * a)
                };
                self.surf.text(&small, &self.f.cal_sub, rf(cx - cw / 2.0, cy + k - 11.0, cw, 11.0), small_c);
                self.hits.push((Hit::CalDay(d), cell));
            }
        }
        if let Some(py) = self.cal_picker {
            let shown = if lunar_mode {
                let l = crate::lunar::of(first);
                (l.year, l.month)
            } else {
                (y, m as i64)
            };
            self.cal_picker_draw(gx_static, by + 22.0, gw, bh - 22.0, full_a, py, lunar_mode, shown, today);
        }
        self.surf.unclip();
    }

    /// Month / year picker over the month grid: ‹ year ›, twelve months, Today.
    #[allow(clippy::too_many_arguments)]
    fn cal_picker_draw(&mut self, x: f32, y: f32, w: f32, h: f32, a: f32, year: i64, lunar_mode: bool, shown: (i64, i64), today: i64) {
        let vi = crate::i18n::vi();
        let g = gfx::g();
        let accent = self.ic(pal::MINT, 1.0);
        let r = rf(x - 4.0, y, w + 8.0, h);
        self.surf.fill_rr(r, 12.0, if self.dark { rgb(0x1c1c1e, 0.98 * a) } else { rgb(0xf2f2f7, 0.98 * a) });
        self.hits.push((Hit::CalPickBg, r));
        // Year row.
        let yr = rf(x, y + 4.0, w, 24.0);
        let label = if lunar_mode { format!("{} · {}", year, crate::lunar::year_name(year)) } else { year.to_string() };
        self.surf.text(&label, &self.f.title_c, yr, self.fg(a));
        for (hit, glyph, bx) in [(Hit::CalPickYear(-1), "\u{E76B}", x + 4.0), (Hit::CalPickYear(1), "\u{E76C}", x + w - 30.0)] {
            let br = rf(bx, y + 3.0, 26.0, 26.0);
            if self.hot == Some(hit) {
                self.surf.fill_rr(br, 8.0, self.fg(0.1 * a));
            }
            self.surf.text(glyph, &self.f.icon_s, br, self.fg(0.75 * a));
            self.hits.push((hit, br));
        }
        // Today.
        let tl = tr("Today");
        let tw = g.measure(tl, &self.f.tiny) + 20.0;
        let todr = rf(x + w - 30.0 - tw - 8.0, y + 5.0, tw, 22.0);
        self.surf.fill_rr(todr, 11.0, with_alpha(accent, if self.hot == Some(Hit::CalOpen) { 0.4 } else { 0.22 } * a));
        self.surf.text(tl, &self.f.tiny_c, todr, with_alpha(accent, a));
        self.hits.push((Hit::CalOpen, todr));
        // Months: 4 × 3.
        let (ty, tm, _) = util::civil_from_days(today);
        let lt = crate::lunar::of(today);
        let gy = y + 34.0;
        let cw = w / 4.0;
        let ch = ((h - 38.0) / 3.0).max(18.0);

        for i in 0..12 {
            let cr = rf(x + (i % 4) as f32 * cw + 3.0, gy + (i / 4) as f32 * ch + 2.0, cw - 6.0, ch - 4.0);
            let mo = i as i64 + 1;
            let is_shown = shown == (year, mo);
            let is_now = if lunar_mode { lt.year == year && lt.month == mo } else { ty == year && tm as i64 == mo };
            let hot = self.hot == Some(Hit::CalPickMonth(i as u8));
            if is_shown {
                self.surf.fill_rr(cr, 9.0, with_alpha(accent, 0.9 * a));
            } else if hot {
                self.surf.fill_rr(cr, 9.0, self.fg(0.1 * a));
            } else if is_now {
                self.surf.stroke_rr(cr, 9.0, with_alpha(accent, 0.8 * a), 1.2);
            }
            let name = if vi || lunar_mode { tr("Month {n}").replace("{n}", &mo.to_string()) } else { util::date_fmt(&util::systime(util::days_from_civil(2024, mo, 1) * 86400), "MMM") };
            self.surf.text(&name, &self.f.body_c, cr, if is_shown { Island::text_on(accent, a) } else { self.fg(0.88 * a) });
            self.hits.push((Hit::CalPickMonth(i as u8), cr));
        }
    }

    /// A day on the calendar: its details on the left (again: back to the clock).
    pub(super) fn cal_pick(&mut self, d: i64) {
        let was = self.cal_sel;
        self.cal_sel = if self.cal_sel == Some(d) { None } else { Some(d) };
        self.cal_detail_scroll = 0.0;
        match self.cal_sel {
            Some(n) => {
                // Another day while one is showing: the new one fades in afresh.
                if was.is_some() && was != Some(n) {
                    self.cal_fade.v = 0.25;
                }
                self.cal_shown = Some(n);
                self.cal_fade.set(1.0);
            }
            None => self.cal_fade.set(0.0),
        }
        self.kick();
        self.render();
    }

    /// The picked day in full: both calendars, hoàng đạo / hắc đạo, lucky
    /// and unlucky hours, trực, solar term, star sign, clashes.
    fn cal_detail(&mut self, bx: f32, by: f32, lw: f32, bh: f32, a: f32, day: i64) {
        use crate::lunar;
        let vi = crate::i18n::vi();
        let g = gfx::g();
        let accent = self.ic(pal::MINT, 1.0);
        let good_c = self.ic(pal::GREEN, 1.0);
        let bad_c = rgb(pal::RED, 1.0);
        let info = lunar::info(day);
        let l = lunar::of(day);
        // Long details scroll (wheel over the left side).
        let view = rf(bx - 6.0, by - 4.0, lw + 12.0, bh + 8.0);
        self.zones.push((8, view));
        self.surf.clip(view);
        let y0 = by - 2.0 - self.cal_detail_scroll;
        let mut y = y0;
        // The date: click to go back to the clock.
        let date = util::date_fmt(&util::systime(day * 86400), "dddd, d MMMM yyyy");
        let dr = rf(bx - 6.0, y, lw + 10.0, 18.0);
        if self.hot == Some(Hit::CalOpen) {
            self.surf.fill_rr(dr, 9.0, self.fg(0.08 * a));
        }
        self.hits.push((Hit::CalOpen, dr));
        self.surf.text(&ellip(g, &date, &self.f.body, lw), &self.f.body, rf(bx, y, lw, 18.0), with_alpha(accent, a));
        y += 19.0;
        let lunar_t = format!("{} {}/{}{} · {}", tr("Lunar"), l.day, l.month, if l.leap { format!(" {}", tr("(leap)")) } else { String::new() }, lunar::year_name(l.year));
        draw_moon_glyph(&self.surf, bx + 5.0, y + 8.0, 4.5, l.day, with_alpha(self.ic(pal::ORANGE, 1.0), a), self.fg(0.18 * a));
        self.surf.text(&ellip(g, &lunar_t, &self.f.tiny, lw - 14.0), &self.f.tiny, rf(bx + 14.0, y, lw - 14.0, 16.0), self.fg(0.8 * a));
        y += 17.0;
        let dn = format!("{} {} · Trực {}", tr("Day"), lunar::day_name(day), lunar::TRUC[info.truc]);
        let dnw = g.measure(&dn, &self.f.tiny).min(lw);
        self.surf.text(&ellip(g, &dn, &self.f.tiny, lw), &self.f.tiny, rf(bx, y, lw, 15.0), self.fg(0.55 * a));
        // Traditionally avoided days, right beside it.
        if let Some(wn) = info.warn {
            let t = format!("⚠ {wn}");
            if dnw + 8.0 + g.measure(&t, &self.f.tiny) <= lw {
                self.surf.text(&t, &self.f.tiny, rf(bx + dnw + 8.0, y, lw - dnw - 8.0, 15.0), rgb(pal::ORANGE, 0.95 * a));
            }
        }
        y += 18.0;
        // Hoàng đạo / hắc đạo, with the day's spirit.
        let (god, good) = lunar::GODS[info.god];
        let label = tr(if good { "Lucky day" } else { "Unlucky day" });
        let t = format!("{label} · {god}");
        let col = if good { good_c } else { bad_c };
        let pw = (g.measure(&t, &self.f.tiny) + 34.0).min(lw);
        let pr = rf(bx, y, pw, 19.0);
        self.surf.fill_rr(pr, 9.5, with_alpha(col, 0.18 * a));
        self.surf.text(if good { "\u{E735}" } else { "\u{E7BA}" }, &self.f.icon_s, rf(pr.left + 7.0, pr.top, 14.0, 19.0), with_alpha(col, a));
        self.surf.text(&ellip(g, &t, &self.f.tiny, pw - 30.0), &self.f.tiny, rf(pr.left + 24.0, pr.top, pw - 26.0, 19.0), with_alpha(col, a));
        y += 23.0;
        // Lucky / unlucky hours.
        let list = |want: bool| -> String { (0..12).filter(|&h| info.hours[h] == want).map(|h| format!("{} {}", lunar::CHI[h], lunar::hour_span(h))).collect::<Vec<_>>().join("  ") };
        for (want, name, c) in [(true, tr("Good hours"), good_c), (false, tr("Bad hours"), bad_c)] {
            self.surf.fill_circle(bx + 3.5, y + 7.5, 3.0, with_alpha(c, 0.9 * a));
            self.surf.text(name, &self.f.tiny, rf(bx + 11.0, y, lw - 11.0, 15.0), self.fg(0.7 * a));
            y += 15.0;
            let txt = list(want);
            let th = g.measure_h(&txt, &self.f.wrap_src, lw - 11.0).clamp(14.0, 60.0);
            self.surf.text_wrap(&txt, &self.f.wrap_src, rf(bx + 11.0, y, lw - 11.0, th + 2.0), self.fg(0.55 * a));
            y += th + 3.0;
        }
        // Star sign, solar term and the clash, on one line.
        let (sv, se, sym) = lunar::SIGNS[info.sign];
        let st = format!("{sym} {} · {} · {} {}", if vi { sv } else { tr(se) }, lunar::TERMS[info.term], tr("clashes with"), lunar::CHI[info.clash]);
        self.surf.text(&ellip(g, &st, &self.f.tiny, lw), &self.f.tiny, rf(bx, y, lw, 15.0), self.fg(0.6 * a));
        y += 16.0;
        self.surf.unclip();
        // Too long to show at once: a thin bar says there's more.
        let total = y - y0;
        let max = (total - (bh + 2.0)).max(0.0);
        self.cal_detail_max = max;
        self.cal_detail_scroll = self.cal_detail_scroll.min(max);
        if max > 0.0 {
            let vh = bh * bh / total;
            let sy = by + (bh - vh) * (self.cal_detail_scroll / max);
            self.surf.fill_rr(rf(bx + lw + 1.0, sy, 2.5, vh), 1.25, self.fg(0.25 * a));
        }
    }

    /// Solar ↔ lunar: keep looking at about the same days.
    pub(super) fn cal_mode(&mut self, m: u8) {
        let lunar = config::cur().i("cal_lunar") == 1;
        if (m == 1) == lunar {
            return;
        }
        if m == 1 {
            // The lunar month around the middle of the solar month shown.
            self.cal_lmonth = self.cal_month.map(|mo| util::days_from_civil(mo.div_euclid(12), mo.rem_euclid(12) + 1, 15));
        } else {
            self.cal_month = self.cal_lmonth.map(|d| {
                let (s, n) = crate::lunar::month_shift(d, 0);
                let (yy, mm, _) = util::civil_from_days(s + n / 2);
                yy * 12 + mm as i64 - 1
            });
        }
        config::update(|c| c.set("cal_lunar", m as i32));
        self.cal_slide.v = if m == 1 { 1.0 } else { -1.0 };
        self.cal_slide.vel = 0.0;
        self.cal_slide.set(0.0);
        self.kick();
        self.render();
    }

    fn draw_art(&mut self, r: D2D_RECT_F, rad: f32, a: f32) {
        match &self.art {
            Some(img) if self.media.has_session => {
                if let Some(fit) = &self.art_fit {
                    // Not square (a video's wide frame): the whole picture at its
                    // own shape, never stretched or cropped, on a soft blurred
                    // square made from it that fills the rest of the cover.
                    match &self.art_blur {
                        Some(bl) => self.surf.image(bl, r, a),
                        None => self.surf.image(img, r, a * 0.45),
                    }
                    self.surf.fill_rr(r, rad, rgb(0x000000, 0.28 * a));
                    let (w, h) = (r.right - r.left, r.bottom - r.top);
                    let k = (w / fit.w as f32).min(h / fit.h as f32);
                    let (fw, fh) = (fit.w as f32 * k, fit.h as f32 * k);
                    self.surf.image(fit, rf(r.left + (w - fw) / 2.0, r.top + (h - fh) / 2.0, fw, fh), a);
                } else {
                    self.surf.image(img, r, a);
                }
            }
            _ => {
                self.surf.fill_rr(r, rad, self.fg(0.08 * a));
                self.surf.text("\u{E8D6}", &self.f.icon_big, r, self.fg(0.4 * a));
            }
        }
    }

    fn progress_row(&mut self, tx: f32, y: f32, tw: f32, a: f32) {
        if !(self.media.has_session && self.media.dur_ms > 0) {
            return;
        }
        let pos = self.media.position_ms();
        let f = (pos as f32 / self.media.dur_ms as f32).clamp(0.0, 1.0);
        let hot = self.hot == Some(Hit::Seek);
        let th = if hot { 6.0 } else { 4.0 };
        self.surf.fill_rr(rf(tx, y - th / 2.0, tw, th), th / 2.0, self.fg(0.16 * a));
        let fill = if self.icon_mode != 1 { self.el(pal::PINK, a) } else { self.fg(0.9 * a) };
        self.surf.fill_rr(rf(tx, y - th / 2.0, (tw * f).max(th), th), th / 2.0, fill);
        self.hits.push((Hit::Seek, rf(tx, y - 8.0, tw, 16.0)));
        self.surf.text(&fmt_ms(pos), &self.f.tiny, rf(tx, y + 6.0, 60.0, 14.0), self.fg(0.5 * a));
        self.surf.text(&format!("-{}", fmt_ms(self.media.dur_ms - pos)), &self.f.tiny_r, rf(tx + tw - 60.0, y + 6.0, 60.0, 14.0), self.fg(0.5 * a));
    }


    // ------------------------------------------------------------------
    // World clock: local + one other city
    // ------------------------------------------------------------------

    pub(super) fn page_clock(&mut self, bx: f32, by: f32, bw: f32, bh: f32, a: f32) {
        let c = config::cur();
        let fmt = c.i("clock_fmt");
        let cw = bw / 2.0 - 10.0;
        let local_off = util::local_offset();
        let now = util::unix_now();

        // Local
        let st = util::now();
        // The place is the city name you typed (its looked-up name once found).
        let here = c.s("weather_place");
        let here = if here.is_empty() { c.s("weather_city") } else { here };
        let here = here.split(',').next().unwrap_or("").trim().to_uppercase();
        let lbl = if here.is_empty() { format!("{} · {}", tr("LOCAL"), util::utc_label(local_off)) } else { format!("{here} · {}", util::utc_label(local_off)) };
        self.clock_col(bx, by, cw, &st, fmt, &lbl, tr("Today"), a);
        {
            let d = crate::data();
            let now_w = d.weather.as_ref().map(|w| (w.temp, w.code, w.is_day));
            if now_w.is_none() && d.weather_err.as_deref() == Some(weather::NOT_FOUND) {
                self.surf.text(&format!("{} · {}", c.s("weather_city"), tr("Not found")), &self.f.body_c, rf(bx, by + 142.0, cw, 22.0), rgb(pal::RED, 0.85 * a));
            }
            self.clock_extra(bx, by, cw, now_w, None, local_off, a);
        }
        self.surf.fill_rect(rf(bx + bw / 2.0, by + 10.0, 1.0, bh - 20.0), self.fg(0.12 * a));

        // Second city
        let x2 = bx + bw / 2.0 + 14.0;
        let d = crate::data();
        if self.edit == Some(EditTarget::TzCity) {
            self.draw_field(rf(x2, by + 2.0, cw, 30.0), EditTarget::TzCity, tr("Type a city, e.g. Melbourne"), a, Hit::ClockEdit);
            self.surf.text(tr("Enter to save · Esc to cancel"), &self.f.tiny, rf(x2 + 4.0, by + 38.0, cw, 16.0), self.fg(0.45 * a));
            return;
        }
        let city = c.s("tz2_city");
        if city.is_empty() {
            let r = rf(x2 + 10.0, by + bh / 2.0 - 22.0, cw - 20.0, 44.0);
            self.button(r, tr("Add a city"), "\u{E710}", false, self.accent, Hit::ClockEdit, a);
            return;
        }
        match (&d.tz2, &d.tz2_err) {
            (Some(tz), _) => {
                let st2 = util::systime(now + tz.offset);
                let place = c.s("tz2_place");
                let name = if place.is_empty() { city.clone() } else { place.split(',').next().unwrap_or("").trim().to_string() };
                let diff = tz.offset - local_off;
                let dd = util::local_day(now) - (now + tz.offset).div_euclid(86400);
                let rel = match dd {
                    0 => tr("Today"),
                    -1 => tr("Tomorrow"),
                    1 => tr("Yesterday"),
                    _ => "",
                };
                let hours = diff as f32 / 3600.0;
                let rel = if diff == 0 {
                    format!("{rel} · {}", tr("same time"))
                } else if hours.fract() == 0.0 {
                    format!("{rel} · {:+}h", hours as i32)
                } else {
                    format!("{rel} · {:+.1}h", hours)
                };
                let lbl = format!("{} · {}", name.to_uppercase(), util::utc_label(tz.offset));
                self.clock_col(x2, by, cw, &st2, fmt, &lbl, &rel, a);
                let zone = (!tz.zone.is_empty()).then(|| tz.zone.clone());
                self.clock_extra(x2, by, cw, Some((tz.temp, tz.code, tz.is_day)), zone.as_deref().or(Some("?")), tz.offset, a);
                self.hits.push((Hit::ClockEdit, rf(x2, by, cw, 22.0)));
                self.surf.text("\u{E70F}", &self.f.icon_s, rf(x2 + cw - 18.0, by + 2.0, 18.0, 18.0), self.fg(if self.hot == Some(Hit::ClockEdit) { 0.9 } else { 0.35 } * a));
            }
            (None, Some(e)) => {
                let msg = if e == weather::NOT_FOUND { format!("“{city}” · {}", tr("Not found")) } else { e.clone() };
                self.surf.text(&msg, &self.f.body, rf(x2, by + 40.0, cw, 20.0), if e == weather::NOT_FOUND { rgb(pal::RED, 0.9 * a) } else { self.fg(0.7 * a) });
                let r = rf(x2, by + 72.0, 140.0, 32.0);
                self.button(r, tr("Change city"), "", false, self.accent, Hit::ClockEdit, a);
            }
            _ => {
                self.surf.text(&format!("{} {city}…", tr("Finding")), &self.f.body, rf(x2, by, cw, bh), self.fg(0.5 * a));
            }
        }
    }

    /// Under a city's time: its weather right now, and when its clocks next
    /// change for daylight saving (if they do). `zone`: None = this PC's zone,
    /// Some("?") = unknown.
    fn clock_extra(&mut self, x: f32, by: f32, w: f32, now: Option<(f32, i32, bool)>, zone: Option<&str>, offset: i64, a: f32) {
        let g = gfx::g();
        if let Some((temp, code, day)) = now {
            // Big enough to read at a glance: icon, temperature, sky.
            let temp_s = format!("{}°", temp.round() as i32);
            let desc = tr(weather::describe(code));
            let tw = g.measure(&temp_s, &self.f.peek_t) + 4.0;
            let dw = g.measure(desc, &self.f.body) + 4.0;
            let isz = 30.0;
            let total = isz + 8.0 + tw + 8.0 + dw;
            let x0 = x + (w - total) / 2.0;
            let y = by + 140.0;
            draw_wx(&self.surf, code, day, x0 + isz / 2.0, y + 13.0, isz, a, self.dark, self.blend(0.0), self.icon_mode == 2);
            self.surf.text(&temp_s, &self.f.peek_t, rf(x0 + isz + 8.0, y, tw + 4.0, 26.0), self.fg(0.95 * a));
            self.surf.text(desc, &self.f.body, rf(x0 + isz + 16.0 + tw, y + 1.0, dw + 4.0, 26.0), self.fg(0.7 * a));
        }
        if zone == Some("?") {
            return;
        }
        let now_s = util::unix_now();
        if let Some((when, delta)) = crate::tzinfo::next_change(zone, now_s) {
            let st = util::systime(when + offset + delta as i64);
            let date = util::date_fmt(&st, "ddd d MMM");
            let h = delta as f32 / 3600.0;
            let hs = if h.fract() == 0.0 { format!("{:+}h", h as i32) } else { format!("{:+.1}h", h) };
            let t = if delta > 0 { format!("{} {date} ({hs})", tr("Daylight saving starts")) } else { format!("{} {date} ({hs})", tr("Daylight saving ends")) };
            let tw = g.measure(&t, &self.f.cal_sub) + 22.0;
            let r = rf(x + (w - tw) / 2.0, by + 172.0, tw, 16.0);
            let col = self.ic(pal::YELLOW, 1.0);
            self.surf.fill_rr(r, 8.0, with_alpha(col, 0.14 * a));
            self.surf.text(if delta > 0 { "\u{E706}" } else { "\u{E708}" }, &self.f.icon_s, rf(r.left + 5.0, r.top, 13.0, 16.0), with_alpha(col, a));
            self.surf.text(&t, &self.f.cal_sub, rf(r.left + 18.0, r.top, tw - 18.0, 16.0), self.fg(0.75 * a));
        }
    }

    /// One city's time, centred in its half.
    fn clock_col(&mut self, x: f32, by: f32, w: f32, st: &windows::Win32::Foundation::SYSTEMTIME, fmt: i32, label: &str, rel: &str, a: f32) {
        let (t, suf) = util::clock_parts(st, fmt);
        self.surf.text(label, &self.f.label_c, rf(x + 16.0, by + 2.0, w - 32.0, 18.0), self.fg(0.5 * a));
        let k = config::cur().i("wc_size").clamp(60, 150) as f32 / 100.0;
        let sw = if suf.is_empty() { 0.0 } else { gfx::g().measure(&suf, &self.f.ampm) + 6.0 };
        let (sz, f) = self.clock_font(&t, w - sw, 52.0 * k);
        let tw = gfx::g().measure(&t, &f);
        let tx = x + (w - tw - sw) / 2.0;
        let mid = by + 58.0 + (sz - 52.0).max(0.0) * 0.12;
        self.surf.text(&t, &f, rf(tx - 2.0, mid - 32.0, tw + 8.0, 64.0), self.fg(0.95 * a));
        if !suf.is_empty() {
            self.surf.text(&suf, &self.f.ampm, rf(tx + tw + 6.0, mid + sz * 0.16 - 9.0, sw + 4.0, 18.0), self.fg(0.5 * a));
        }
        let date = util::date_fmt(st, "dddd, d MMMM");
        self.surf.text(&date, &self.f.body_c, rf(x, by + 98.0, w, 20.0), self.fg(0.75 * a));
        self.surf.text(rel, &self.f.artist_c, rf(x, by + 120.0, w, 18.0), self.fg(0.5 * a));
    }

    // ------------------------------------------------------------------
    // Weather
    // ------------------------------------------------------------------

    pub(super) fn page_weather(&mut self, bx: f32, by: f32, bw: f32, bh: f32, a: f32) {
        let c = config::cur();
        let d = crate::data();
        let city = c.s("weather_city");
        if self.edit == Some(EditTarget::WeatherCity) || city.is_empty() {
            let editing = self.edit == Some(EditTarget::WeatherCity);
            self.surf.text(tr("Where are you?"), &self.f.title, rf(bx + 60.0, by + 18.0, bw - 120.0, 26.0), self.fg(a));
            self.surf.text(tr("Weather and the world clock use this place"), &self.f.artist, rf(bx + 60.0, by + 44.0, bw - 120.0, 20.0), self.fg(0.5 * a));
            self.draw_field(rf(bx + 60.0, by + 74.0, bw - 120.0, 34.0), EditTarget::WeatherCity, tr("City, e.g. Hanoi or Melbourne, Australia"), a, Hit::WeatherEdit);
            if editing {
                self.surf.text(tr("Enter to save · Esc to cancel"), &self.f.tiny, rf(bx + 64.0, by + 114.0, 300.0, 16.0), self.fg(0.45 * a));
            }
            return;
        }
        let unit = if c.i("weather_unit") == 1 { "°" } else { "°" };
        let place = c.s("weather_place");
        let place = if place.is_empty() { city.clone() } else { place };
        // 2 : 8 — now on the left, the coming days or today's chart on the right.
        // Bigger text (Settings → Pages → Weather text size) takes a little more width.
        let wk = self.f.wx_k;
        let lw = (bw * 0.2 * wk.max(1.0)).min(bw * 0.3).round();
        let g = gfx::g();
        // Place (click to change)
        let pname = place.split(',').next().unwrap_or("").trim().to_string();
        let pw = g.measure(&pname, &self.f.wx_label).min(lw - 20.0);
        let px = bx + (lw - pw - 18.0) / 2.0;
        let pr = rf(px - 4.0, by - 1.0, pw + 26.0, 22.0);
        let hot = self.hot == Some(Hit::WeatherEdit);
        if hot {
            self.surf.fill_rr(pr, 8.0, self.fg(0.08 * a));
        }
        self.surf.text("\u{E707}", &self.f.icon_s, rf(px, by + 2.0, 14.0, 18.0), self.ic(pal::RED, a));
        self.surf.text(&pname, &self.f.wx_label, rf(px + 18.0, by + 2.0, pw + 4.0, 18.0), self.fg(if hot { 1.0 } else { 0.7 } * a));
        self.hits.push((Hit::WeatherEdit, pr));
        let gx = bx + lw + 16.0;
        let gw = bx + bw - gx;
        self.surf.fill_rect(rf(gx - 8.0, by + 4.0, 1.0, bh - 8.0), self.fg(0.1 * a));
        // Refresh + source, top right
        let rr = rf(bx + bw - 26.0, by - 3.0, 26.0, 26.0);
        if self.hot == Some(Hit::WeatherRefresh) {
            self.surf.fill_rr(rr, 8.0, self.fg(0.1 * a));
        }
        self.surf.text("\u{E72C}", &self.f.icon_s, rr, self.ic(pal::TEAL, 0.8 * a));
        self.hits.push((Hit::WeatherRefresh, rr));
        if let Some(w) = &d.weather {
            let src = crate::weather::PROVIDERS.get(w.provider as usize).copied().unwrap_or("");
            self.surf.text(&format!("{} {src}", tr("via")), &self.f.tiny_r, rf(bx + bw - 190.0, by, 158.0, 20.0), self.fg(0.32 * a));
        }
        // Next days | Today (hour by hour).
        // The highlight slides between the two.
        let vt = self.ui.wx_view.v.clamp(0.0, 1.0);
        {
            let mut sx = gx + 2.0;
            let mut rs = Vec::new();
            for (on, label) in [(false, tr("Next days")), (true, tr("Today"))] {
                let w_ = g.measure(label, &self.f.tiny) + 20.0;
                rs.push((on, label, rf(sx, by, w_, 22.0)));
                sx += w_ + 4.0;
            }
            let (r0, r1) = (rs[0].2, rs[1].2);
            let pill = rf(r0.left + (r1.left - r0.left) * vt, by, (r0.right - r0.left) + ((r1.right - r1.left) - (r0.right - r0.left)) * vt, 22.0);
            for (on, _, r) in &rs {
                if self.hot == Some(Hit::WxView(*on)) && self.wx_today != *on {
                    self.surf.fill_rr(*r, 11.0, self.fg(0.14 * a));
                } else {
                    self.surf.fill_rr(*r, 11.0, self.fg(0.06 * a));
                }
            }
            self.surf.fill_rr(pill, 11.0, self.el(pal::TEAL, 0.85 * a));
            for (on, label, r) in rs {
                let sel = if on { vt } else { 1.0 - vt };
                let on_c = Island::text_on(self.ic(pal::TEAL, 1.0), a);
                let off_c = self.fg(0.7 * a);
                let c = D2D1_COLOR_F { r: off_c.r + (on_c.r - off_c.r) * sel, g: off_c.g + (on_c.g - off_c.g) * sel, b: off_c.b + (on_c.b - off_c.b) * sel, a: off_c.a + (on_c.a - off_c.a) * sel };
                self.surf.text(label, &self.f.tiny_c, r, c);
                self.hits.push((Hit::WxView(on), r));
            }
        }

        let Some(w) = d.weather.clone() else {
            let m = match &d.weather_err {
                Some(e) if e == weather::NOT_FOUND => format!("“{city}” · {}", tr("Not found")),
                Some(e) => e.clone(),
                None => tr("Loading weather…").to_string(),
            };
            self.surf.text(&m, &self.f.body, rf(gx, by + 30.0, gw, bh - 30.0), self.fg(0.6 * a));
            return;
        };
        // ---- Now (left) ----
        let mono = self.icon_mode == 2;
        // The sky icon on top, the temperature under it.
        let t = format!("{}{unit}", w.temp.round() as i32);
        let tw = g.measure(&t, &self.f.wx_temp);
        // Everything stacks under the place name; bigger text squeezes the
        // gaps (and the icon) so the last line still fits the page.
        let sq = ((bh - 24.0) / (161.0 * wk)).min(1.0);
        let isz = 36.0 * wk * sq.max(0.8);
        let icon_cy = by + 22.0 + isz / 2.0;
        draw_wx(&self.surf, w.code, w.is_day, bx + lw / 2.0, icon_cy, isz, a, self.dark, self.blend(0.0), mono);
        let ty = by + 22.0 + isz - 9.0 * wk;
        self.surf.text(&t, &self.f.wx_temp, rf(bx + (lw - tw) / 2.0 - 3.0, ty, tw + 6.0, 62.0 * wk), self.fg(a));
        let dy = ty + 56.0 * wk * sq.max(0.9);
        self.surf.text(tr(weather::describe(w.code)), &self.f.wx_title, rf(bx - 6.0, dy, lw + 12.0, 22.0 * wk), self.fg(a));
        let (hi, lo) = w.days.first().map(|d| (d.max, d.min)).unwrap_or((w.temp, w.temp));
        let line = format!("{} {}{unit}  {} {}{unit}  ·  {} {}{unit}", tr("H"), hi.round() as i32, tr("L"), lo.round() as i32, tr("Feels"), w.feels.round() as i32);
        self.surf.text(&line, &self.f.wx_line, rf(bx - 10.0, dy + 22.0 * wk * sq.max(0.85), lw + 20.0, 16.0 * wk), self.fg(0.7 * a));
        let wind = if w.unit_f { "mph" } else { "km/h" };
        // Humidity and wind, each on its own line.
        let k = wk * sq.max(0.85);
        self.surf.text(&format!("{} {}%", tr("Humidity"), w.humidity), &self.f.wx_line, rf(bx - 10.0, dy + 38.0 * k, lw + 20.0, 16.0 * wk), self.fg(0.5 * a));
        self.surf.text(&format!("{} {} {wind}", tr("Wind"), w.wind.round() as i32), &self.f.wx_line, rf(bx - 10.0, dy + 53.0 * k, lw + 20.0, 16.0 * wk), self.fg(0.5 * a));

        // The two views cross-fade and slide past each other.
        let ease = vt * vt * (3.0 - 2.0 * vt);
        self.surf.clip(rf(gx - 4.0, by + 24.0, gw + 8.0, bh - 22.0));
        if ease < 0.999 {
            let dx = -ease * 36.0;
            self.wx_days(&w, gx + dx, by, gw, bh, a * (1.0 - ease));
        }
        if ease > 0.001 {
            let dx = (1.0 - ease) * 36.0;
            self.wx_today_chart(&w, gx + dx, by + 26.0, gw, bh - 26.0, a * ease, ease > 0.5);
        }
        self.surf.unclip();
    }

    /// The coming days, one column each.
    fn wx_days(&mut self, w: &weather::Weather, gx: f32, by: f32, gw: f32, bh: f32, a: f32) {
        let unit = "°";
        let mono = self.icon_mode == 2;
        let n = w.days.len().min(7);
        if n == 0 {
            return;
        }
        let colw = gw / n as f32;
        let today = w.days[0].day;
        let top = by + 24.0;
        let ch = bh - 26.0;
        for (i, day) in w.days.iter().take(n).enumerate() {
            let cx = gx + colw * i as f32;
            let label = if day.day == today { tr("Today").to_string() } else { util::short_day(&util::systime(day.day * 86400)) };
            let col_hot = i == 0;
            if col_hot {
                self.surf.fill_rr(rf(cx + 2.0, top, colw - 4.0, ch), 12.0, self.fg(0.06 * a));
            }
            self.surf.text(&label, &self.f.label_c, rf(cx, top + 8.0, colw, 16.0), self.fg(0.75 * a));
            let isz = (colw * 0.62).clamp(26.0, 40.0);
            draw_wx(&self.surf, day.code, true, cx + colw / 2.0, top + 50.0, isz, a, self.dark, self.blend(if col_hot { 0.06 } else { 0.0 }), mono);
            self.surf.text(&format!("{}{unit}", day.max.round() as i32), &self.f.body_c, rf(cx, top + 74.0, colw, 18.0), self.fg(a));
            self.surf.text(&format!("{}{unit}", day.min.round() as i32), &self.f.tiny_c, rf(cx, top + 93.0, colw, 16.0), self.fg(0.5 * a));
            let rain = if day.rain >= 20 {
                format!("{}%", day.rain)
            } else if day.rain < 0 && day.rain_mm >= 0.5 {
                format!("{:.0} mm", day.rain_mm)
            } else {
                String::new()
            };
            self.surf.text(&rain, &self.f.tiny_c, rf(cx, top + 111.0, colw, 14.0), self.ic(pal::CYAN, a));
        }
    }

    /// Today (and tomorrow) hour by hour: temperature and feels-like, humidity,
    /// chance of rain — a chart you can scroll sideways with the wheel, a
    /// dashed line at the current time, and the day's numbers on top.
    fn wx_today_chart(&mut self, w: &weather::Weather, x: f32, y: f32, wd: f32, h: f32, a: f32, live: bool) {
        let g = gfx::g();
        if w.hours.len() < 2 {
            self.surf.text(tr("No hourly forecast from this source"), &self.f.body, rf(x, y + 20.0, wd, 24.0), self.fg(0.5 * a));
            return;
        }
        // Fixed colours, so the three series always tell apart.
        let heat = rgb(pal::ORANGE, 1.0);
        let wet = rgb(pal::CYAN, 1.0);
        let rainc = rgb(pal::BLUE, 1.0);
        // A label only when it fits wholly inside the chart.
        let fits = |cx_: f32, half: f32| cx_ - half >= x && cx_ + half <= x + wd;
        let now_wall = util::unix_now() + w.offset;
        let today = now_wall.div_euclid(86400);
        // Colour legend: what each line and bar means.
        {
            let mut lx = x + 4.0;
            let ly = y + 1.0;
            let items: [(u8, D2D1_COLOR_F, &str); 5] = [
                (0, heat, tr("Temperature")),
                (1, with_alpha(heat, 0.45), tr("Feels like")),
                (2, wet, tr("Humidity")),
                (3, rainc, tr("Chance of rain")),
                (4, rgb(pal::RED, 1.0), tr("Now")),
            ];
            for (kind, col, label) in items {
                let sw = 14.0;
                let cyl = ly + 7.0;
                match kind {
                    0 => self.surf.line(lx, cyl, lx + sw, cyl, with_alpha(col, a), 2.2),
                    1 | 2 => self.surf.line(lx, cyl, lx + sw, cyl, with_alpha(col, if kind == 1 { 0.45 } else { 0.8 } * a), 1.4),
                    3 => self.surf.fill_rr(rf(lx + 3.0, cyl - 5.0, 8.0, 10.0), 2.0, with_alpha(col, 0.75 * a)),
                    _ => {
                        let mut yy = cyl - 6.0;
                        while yy < cyl + 6.0 {
                            self.surf.fill_rect(rf(lx + 6.5, yy, 1.4, 3.0), with_alpha(col, 0.9 * a));
                            yy += 5.0;
                        }
                    }
                }
                let tw = g.measure(label, &self.f.cal_sub);
                if lx + sw + 4.0 + tw > x + wd {
                    break;
                }
                self.surf.text(label, &self.f.cal_sub, rf(lx + sw + 4.0, ly, tw + 4.0, 14.0), self.fg(0.6 * a));
                lx += sw + 4.0 + tw + 12.0;
            }
        }
        // The chart.
        let cy0 = y + 20.0;
        let ch = h - 22.0;
        let area = rf(x, cy0, wd, ch);
        if live {
            self.zones.push((7, area));
        }
        let t0 = w.hours[0].t;
        let t1 = w.hours.last().map(|hh| hh.t).unwrap_or(t0);
        let pxh = 30.0;
        let total = ((t1 - t0) as f32 / 3600.0) * pxh + 40.0;
        let max_scroll = (total - wd).max(0.0);
        if self.wx_scroll < 0.0 {
            // First look: the current hour near the left.
            self.wx_scroll = (((now_wall - t0) as f32 / 3600.0) * pxh - wd * 0.2).clamp(0.0, max_scroll);
        }
        self.wx_scroll = self.wx_scroll.clamp(0.0, max_scroll);
        let xo = x + 20.0 - self.wx_scroll;
        let tx = |t: i64| xo + (t - t0) as f32 / 3600.0 * pxh;
        let (tmin, tmax) = w.hours.iter().fold((f32::MAX, f32::MIN), |(lo, hi), hh| (lo.min(hh.temp.min(hh.feels)), hi.max(hh.temp.max(hh.feels))));
        let span = (tmax - tmin).max(4.0);
        // Rows: temperature on top, humidity line over it, rain bars and hours at the bottom.
        let top = cy0 + 14.0;
        let bars_h = 26.0;
        let hours_y = cy0 + ch - 14.0;
        let bars_b = hours_y - 2.0;
        let curve_b = bars_b - bars_h - 14.0;
        let ty = |v: f32| curve_b - (v - tmin) / span * (curve_b - top);
        let hy = |v: f32| curve_b - v / 100.0 * (curve_b - top);
        self.surf.clip(area);
        // Midnight: tomorrow starts.
        for hh in w.hours.iter().filter(|hh| hh.t.rem_euclid(86400) == 0 && hh.t != t0) {
            let mx = tx(hh.t);
            self.surf.fill_rect(rf(mx, cy0, 1.0, ch - 14.0), self.fg(0.12 * a));
            self.surf.text(tr("Tomorrow"), &self.f.tiny, rf(mx + 4.0, cy0, 90.0, 14.0), self.fg(0.4 * a));
        }
        // Rain bars.
        for hh in &w.hours {
            let bx_ = tx(hh.t);
            if bx_ < x - pxh || bx_ > x + wd + pxh {
                continue;
            }
            if hh.rain > 0 {
                let bh_ = bars_h * hh.rain as f32 / 100.0;
                self.surf.fill_rr(rf(bx_ - 6.0, bars_b - bh_, 12.0, bh_.max(2.0)), 3.0, with_alpha(rainc, (0.35 + hh.rain as f32 / 200.0) * a));
                if hh.rain >= 10 && fits(bx_, 16.0) {
                    self.surf.text(&format!("{}%", hh.rain), &self.f.cal_sub, rf(bx_ - 16.0, bars_b - bh_ - 12.0, 32.0, 11.0), with_alpha(rainc, a));
                }
            } else if hh.mm > 0.05 && fits(bx_, 16.0) {
                self.surf.text(&format!("{:.1}", hh.mm), &self.f.cal_sub, rf(bx_ - 16.0, bars_b - 12.0, 32.0, 11.0), with_alpha(rainc, a));
            }
            // Hour labels.
            let hr = hh.t.rem_euclid(86400) / 3600;
            if !fits(bx_, 14.0) {
                continue;
            }
            self.surf.text(&format!("{hr}h"), &self.f.cal_sub, rf(bx_ - 14.0, hours_y, 28.0, 12.0), self.fg(0.45 * a));
        }
        // Humidity: a thin line with a number now and then.
        let hum: Vec<(f32, f32, i32)> = w.hours.iter().filter(|hh| hh.humidity >= 0).map(|hh| (tx(hh.t), hy(hh.humidity as f32), hh.humidity)).collect();
        if hum.len() > 1 {
            let pts: Vec<(f32, f32)> = hum.iter().map(|p| (p.0, p.1)).collect();
            self.surf.stroke_poly(&pts, false, with_alpha(wet, 0.55 * a), 1.2);
            for (i, p) in hum.iter().enumerate() {
                if i % 3 == 1 && fits(p.0, 14.0) {
                    self.surf.text(&format!("{}%", p.2), &self.f.cal_sub, rf(p.0 - 14.0, p.1 + 2.0, 28.0, 11.0), with_alpha(wet, 0.85 * a));
                }
            }
        }
        // Feels-like (faint) and temperature (bold) with numbers.
        let feels: Vec<(f32, f32)> = w.hours.iter().map(|hh| (tx(hh.t), ty(hh.feels))).collect();
        self.surf.stroke_poly(&feels, false, with_alpha(heat, 0.3 * a), 1.2);
        let temps: Vec<(f32, f32)> = w.hours.iter().map(|hh| (tx(hh.t), ty(hh.temp))).collect();
        self.surf.stroke_poly(&temps, false, with_alpha(heat, a), 2.2);
        let step = if pxh * 1.0 >= 26.0 { 1 } else { 2 };
        for (i, hh) in w.hours.iter().enumerate() {
            let (px, py) = temps[i];
            if px < x - 20.0 || px > x + wd + 20.0 {
                continue;
            }
            self.surf.fill_circle(px, py, 2.4, with_alpha(heat, a));
            if i % step == 0 && fits(px, 14.0) {
                self.surf.text(&format!("{}°", hh.temp.round() as i32), &self.f.cal_sub, rf(px - 14.0, py - 14.0, 28.0, 11.0), self.fg(0.85 * a));
            }
        }
        // Hover: the hour nearest the pointer, anywhere over the chart (the
        // columns are hits too, so moving between hours repaints at once).
        let mut hover: Option<usize> = None;
        if live {
            for (i, hh) in w.hours.iter().enumerate() {
                let px = tx(hh.t);
                let step = w.hours.get(i + 1).map(|n| tx(n.t) - px).unwrap_or(pxh);
                let col = rf(px - step / 2.0, cy0, step, ch);
                if col.right < x || col.left > x + wd {
                    continue;
                }
                let col = rf(col.left.max(x), col.top, (col.right.min(x + wd) - col.left.max(x)).max(1.0), ch);
                self.hits.push((Hit::WxHour(i), col));
            }
            let (mx, my) = self.last_pt;
            if self.hover && inside(&area, mx, my) && self.edit.is_none() {
                hover = temps.iter().enumerate().min_by(|p, q| (p.1 .0 - mx).abs().total_cmp(&(q.1 .0 - mx).abs())).map(|(i, _)| i).filter(|i| (temps[*i].0 - mx).abs() < pxh);
            }
        }
        // The card follows smoothly and fades in / out.
        if let Some(i) = hover {
            if self.ui.wx_last.is_none() || self.ui.wx_ha.v < 0.05 {
                self.ui.wx_gx.snap(temps[i].0);
            }
            self.ui.wx_last = Some(i);
            self.ui.wx_gx.set(temps[i].0);
            self.ui.wx_ha.set(1.0);
        } else {
            self.ui.wx_ha.set(0.0);
        }
        let ha = self.ui.wx_ha.v.clamp(0.0, 1.0);
        let shown = self.ui.wx_last.filter(|i| *i < w.hours.len() && ha > 0.01);
        if let Some(i) = shown {
            let ga = ha * a;
            let px = self.ui.wx_gx.v;
            self.surf.fill_rect(rf(px - 0.5, cy0 + 12.0, 1.0, ch - 26.0), self.fg(0.35 * ga));
            let r_ = 3.0 + 1.5 * ha;
            self.surf.fill_circle(temps[i].0, temps[i].1, r_ + 3.0, with_alpha(heat, 0.25 * ga));
            self.surf.fill_circle(temps[i].0, temps[i].1, r_, with_alpha(heat, ga));
            self.surf.fill_circle(feels[i].0, feels[i].1, 3.5 * ha, with_alpha(heat, 0.55 * ga));
            if let Some(hp) = hum.iter().find(|p| (p.0 - temps[i].0).abs() < 0.5) {
                self.surf.fill_circle(hp.0, hp.1, 3.5 * ha, with_alpha(wet, ga));
            }
        }
        // Now: a dashed line.
        let nx = tx(now_wall);
        if nx > x && nx < x + wd {
            let mut yy = cy0;
            while yy < hours_y - 2.0 {
                self.surf.fill_rect(rf(nx - 0.6, yy, 1.4, 4.0), rgb(pal::RED, 0.9 * a));
                yy += 7.0;
            }
            let nl = tr("Now");
            let nw = g.measure(nl, &self.f.cal_sub) + 10.0;
            let nr = rf(nx - nw / 2.0, cy0 - 2.0, nw, 15.0);
            self.surf.fill_rr(nr, 7.5, rgb(pal::RED, 0.9 * a));
            // Lifted a touch: the text's line box carries room for accents
            // above, so plain centring sits it low.
            self.surf.text(nl, &self.f.cal_sub, rf(nr.left, nr.top - 1.5, nw, 15.0), rgb(0xffffff, a));
        }
        self.surf.unclip();
        // The hovered hour in numbers: all four, in a small card by the point.
        if let Some(i) = shown {
            let a = a * ha;
            let hh = &w.hours[i];
            let px = temps[i].0;
            let hr = hh.t.rem_euclid(86400) / 3600;
            let day_word = if hh.t.div_euclid(86400) != today { format!(" · {}", tr("Tomorrow")) } else { String::new() };
            let unit = "°";
            let mut lines: Vec<(D2D1_COLOR_F, String)> = vec![
                (heat, format!("{} {}{unit}", tr("Temperature"), hh.temp.round() as i32)),
                (with_alpha(heat, 0.55), format!("{} {}{unit}", tr("Feels like"), hh.feels.round() as i32)),
            ];
            if hh.humidity >= 0 {
                lines.push((wet, format!("{} {}%", tr("Humidity"), hh.humidity)));
            }
            let rain = match (hh.rain >= 0, hh.mm > 0.0) {
                (true, true) => format!("{} {}% · {:.1} mm", tr("Chance of rain"), hh.rain, hh.mm),
                (true, false) => format!("{} {}%", tr("Chance of rain"), hh.rain),
                (false, true) => format!("{} {:.1} mm", tr("Rain"), hh.mm),
                (false, false) => format!("{} —", tr("Chance of rain")),
            };
            lines.push((rainc, rain));
            let head = format!("{hr}:00{day_word}");
            let cw_ = lines.iter().map(|l| g.measure(&l.1, &self.f.tiny)).fold(g.measure(&head, &self.f.tiny), f32::max) + 30.0;
            let chh = 22.0 + lines.len() as f32 * 16.0;
            let mut cx_ = px + 12.0;
            if cx_ + cw_ > x + wd {
                cx_ = px - 12.0 - cw_;
            }
            let cy_ = (temps[i].1 - chh - 10.0).max(cy0 + 12.0).min(cy0 + ch - chh);
            // Glide to the new spot (jump there when it first appears).
            if ha < 0.05 && hover.is_some() {
                self.ui.wx_hx.snap(cx_.max(x));
                self.ui.wx_hy.snap(cy_);
            }
            if hover.is_some() {
                self.ui.wx_hx.set(cx_.max(x));
                self.ui.wx_hy.set(cy_);
            }
            let lift = (1.0 - ha) * 6.0;
            let card = rf(self.ui.wx_hx.v, self.ui.wx_hy.v + lift, cw_, chh);
            let bgc = if self.dark { rgb(0x2c2c2e, 0.97 * a) } else { rgb(0xffffff, 0.97 * a) };
            self.surf.fill_rr(card, 9.0, bgc);
            self.surf.stroke_rr(card, 9.0, self.fg(0.14 * a), 1.0);
            self.surf.text(&head, &self.f.tiny, rf(card.left + 10.0, card.top + 3.0, cw_ - 14.0, 16.0), self.fg(0.95 * a));
            for (k, (c, t)) in lines.iter().enumerate() {
                let ly = card.top + 20.0 + k as f32 * 16.0;
                self.surf.fill_circle(card.left + 14.0, ly + 8.0, 3.5, with_alpha(*c, a));
                self.surf.text(t, &self.f.tiny, rf(card.left + 24.0, ly, cw_ - 28.0, 16.0), self.fg(0.85 * a));
            }
        }
        // Scroll hint: a thin bar under the chart.
        if max_scroll > 0.0 {
            let vw = wd * wd / total;
            let sx = x + (wd - vw) * (self.wx_scroll / max_scroll);
            self.surf.fill_rr(rf(sx, cy0 + ch + 1.0, vw, 2.5), 1.25, self.fg(0.22 * a));
        }
    }

    // ------------------------------------------------------------------
    // Focus timer
    // ------------------------------------------------------------------

    /// Focus: the ring on the left; phase chips, Start / Reset / Skip and the
    /// way to the long break fill the rest of the width. The ring sweeps
    /// smoothly, the chip highlight slides, the −/+ fade as it starts.
    pub(super) fn page_focus(&mut self, bx: f32, by: f32, bw: f32, bh: f32, a: f32) {
        let d = crate::data();
        let (phase, running, rem, prog, cycle, today) = (d.focus.phase, d.focus.running, d.focus.remaining().as_secs(), d.focus.progress(), d.focus.cycle, d.focus.today);
        let col = focus_color(phase);
        let phases = [Phase::Focus, Phase::Short, Phase::Long];
        let pi = phases.iter().position(|p| *p == phase).unwrap_or(0) as f32;
        // Targets for the transitions (first look: jump straight there).
        if self.ui.focus_prog.target == 0.0 && self.ui.focus_prog.v == 0.0 {
            self.ui.focus_prog.snap(prog);
            self.ui.focus_chip.snap(pi);
            self.ui.focus_run.snap(running as i32 as f32);
        }
        self.ui.focus_prog.set(prog);
        self.ui.focus_chip.set(pi);
        self.ui.focus_run.set(running as i32 as f32);
        let run = self.ui.focus_run.v.clamp(0.0, 1.0);
        // ---- The ring (left), as tall as the page allows ----
        let r = ((bh - 40.0) / 2.0).clamp(50.0, 72.0);
        let (cx, cy) = (bx + r + 10.0, by + bh / 2.0 - 2.0);
        self.surf.ring(cx, cy, r, 8.0, self.ui.focus_prog.v.clamp(0.0, 1.0), with_alpha(col, 0.18 * a), with_alpha(col, a));
        let lift = 6.0 * (1.0 - run);
        self.surf.text(&fmt_dur(rem), &self.f.big_c, rf(cx - 60.0, cy - 24.0 - lift, 120.0, 36.0), self.fg(a));
        self.surf.text(phase.name(), &self.f.tiny_c, rf(cx - 60.0, cy + 12.0 - lift, 120.0, 16.0), with_alpha(col, a));
        // −/+ change the shown phase's length (while it isn't running); they
        // fade away as it starts.
        let pm = 1.0 - run;
        if pm > 0.02 {
            for (k, (glyph, dir)) in [("\u{E738}", -1), ("\u{E710}", 1)].iter().enumerate() {
                let rr = rf(cx - 25.0 + k as f32 * 30.0, cy + 26.0 + 4.0 * run, 20.0, 20.0);
                let hot = self.hot == Some(Hit::FocusLen(*dir));
                self.surf.fill_circle(rr.left + 10.0, rr.top + 10.0, 9.5, self.fg(if hot { 0.16 } else { 0.08 } * a * pm));
                self.surf.text(glyph, &self.f.icon_s, rr, self.fg(0.8 * a * pm));
                if !running {
                    self.hits.push((Hit::FocusLen(*dir), rr));
                }
            }
        }
        // ---- Right: everything spans the rest of the width ----
        let x = cx + r + 26.0;
        let rw = bx + bw - x;
        let top = by + (bh - 150.0).max(0.0) / 2.0;
        // Phase chips: equal widths across; the highlight slides between them.
        let gap = 6.0;
        let cw = (rw - gap * 2.0) / 3.0;
        let chip = |i: usize| rf(x + i as f32 * (cw + gap), top + 4.0, cw, 26.0);
        let fc = self.ui.focus_chip.v.clamp(0.0, 2.0);
        let (i0, f) = (fc.floor() as usize, fc.fract());
        let (c0, c1) = (chip(i0), chip((i0 + 1).min(2)));
        let pill = rf(c0.left + (c1.left - c0.left) * f, c0.top, cw, 26.0);
        self.surf.fill_rr(pill, 13.0, with_alpha(col, 0.22 * a));
        for (i, p) in phases.into_iter().enumerate() {
            let label = format!("{} · {}m", tr(p.name()), crate::tools::Focus::minutes(p));
            let r_ = chip(i);
            let hot = self.hot == Some(Hit::FocusPhase(i as u8));
            if hot && p != phase {
                self.surf.fill_rr(r_, 13.0, self.fg(0.07 * a));
            }
            let near = (1.0 - (fc - i as f32).abs()).clamp(0.0, 1.0);
            let tc = if near > 0.5 { with_alpha(focus_color(p), a) } else { self.fg(0.55 * a) };
            self.surf.text(&label, &self.f.tiny_c, r_, tc);
            self.hits.push((Hit::FocusPhase(i as u8), r_));
        }
        // Start / Pause fills the row; Reset and Skip at the end.
        let label = tr(if running { "Pause" } else if d.focus.active() { "Resume" } else { "Start" });
        let glyph = if running { "\u{E769}" } else { "\u{E768}" };
        let by2 = top + 44.0;
        let sb = 44.0;
        self.button(rf(x, by2, rw - 2.0 * (sb + 10.0), 44.0), label, glyph, true, col, Hit::FocusToggle, a);
        self.button(rf(x + rw - 2.0 * sb - 10.0, by2, sb, 44.0), "", "\u{E72C}", false, col, Hit::FocusReset, a);
        self.button(rf(x + rw - sb, by2, sb, 44.0), "", "\u{E893}", false, col, Hit::FocusSkip, a);

        // The way to the long break: one dot per short break, then the long one.
        let every = config::cur().i("long_every").clamp(2, 12) as u32;
        let short_col = focus_color(Phase::Short);
        let long_col = focus_color(Phase::Long);
        let on_long = phase == Phase::Long;
        let dy = top + 112.0;
        for i in 0..every {
            let filled = on_long || i < cycle;
            self.surf.fill_circle(x + 6.0 + i as f32 * 16.0, dy, 4.5, if filled { with_alpha(short_col, a) } else { self.fg(0.18 * a) });
        }
        let lx = x + every as f32 * 16.0 + 4.0;
        let lr = rf(lx, dy - 7.0, 22.0, 14.0);
        if on_long {
            self.surf.fill_rr(lr, 7.0, with_alpha(long_col, a));
        } else {
            self.surf.stroke_rr(lr, 7.0, self.fg(0.25 * a), 1.2);
        }
        let left = every.saturating_sub(cycle);
        let hint = if on_long {
            tr("Long break now").to_string()
        } else if left == 0 {
            tr("Long break after this focus").to_string()
        } else {
            format!("{left} {}", tr(if left == 1 { "short break to the long one" } else { "short breaks to the long one" }))
        };
        self.surf.text(&hint, &self.f.tiny, rf(lx + 30.0, dy - 10.0, bx + bw - lx - 30.0, 20.0), self.fg(0.5 * a));
        let s = tr(if today == 1 { "focus session" } else { "focus sessions" });
        let lt = crate::data().focus.long_today;
        let lb = tr(if lt == 1 { "long break" } else { "long breaks" });
        self.surf.text(&format!("{}: {today} {s} · {lt} {lb}", tr("Today")), &self.f.artist, rf(x, dy + 16.0, rw, 18.0), self.fg(0.55 * a));
    }

    // ------------------------------------------------------------------
    // To-do
    // ------------------------------------------------------------------

    /// To-do | Notes switch (top left of the page); returns its width.
    /// To-do | Notes: the chosen side shows its name, the other just its icon;
    /// the highlight slides across (`t`: 0 to-do … 1 notes).
    fn todo_switch(&mut self, x: f32, y: f32, a: f32, t: f32) -> f32 {
        let g = gfx::g();
        let col = self.ic(pal::GREEN, 1.0);
        let segs = [(false, "\u{E73A}", tr("To-do")), (true, "\u{E70B}", tr("Notes"))];
        let sel = |on: bool| if on { t } else { 1.0 - t };
        let ws: Vec<f32> = segs.iter().map(|sg| 30.0 + sel(sg.0) * (g.measure(sg.2, &self.f.tiny) + 8.0)).collect();
        let total = ws.iter().sum::<f32>() + 4.0;
        self.surf.fill_rr(rf(x, y, total, 28.0), 14.0, self.fg(0.07 * a));
        let r0 = rf(x + 2.0, y + 2.0, ws[0], 24.0);
        let r1 = rf(x + 2.0 + ws[0], y + 2.0, ws[1], 24.0);
        for (k, r) in [r0, r1].iter().enumerate() {
            if self.hot == Some(Hit::TodoNotes(k == 1)) && self.notes_view != (k == 1) {
                self.surf.fill_rr(*r, 12.0, self.fg(0.08 * a));
            }
        }
        let pill = rf(r0.left + (r1.left - r0.left) * t, r0.top, ws[0] + (ws[1] - ws[0]) * t, 24.0);
        self.surf.fill_rr(pill, 12.0, with_alpha(col, 0.9 * a));
        let on_c = Island::text_on(col, 1.0);
        let off_c = self.fg(0.75);
        let mix = |k: f32| D2D1_COLOR_F { r: off_c.r + (on_c.r - off_c.r) * k, g: off_c.g + (on_c.g - off_c.g) * k, b: off_c.b + (on_c.b - off_c.b) * k, a: (off_c.a + (on_c.a - off_c.a) * k) * a };
        for (k, (on, icon, label)) in segs.iter().enumerate() {
            let r = if k == 0 { r0 } else { r1 };
            let sl = sel(*on);
            self.surf.text(icon, &self.f.icon_s, rf(r.left + 9.0, r.top, 14.0, 24.0), mix(sl));
            if sl > 0.05 {
                self.surf.clip(r);
                self.surf.text(label, &self.f.tiny, rf(r.left + 27.0, r.top, r.right - r.left - 28.0 + 40.0, 24.0), with_alpha(mix(sl), sl * a));
                self.surf.unclip();
            }
            self.hits.push((Hit::TodoNotes(*on), r));
        }
        total
    }

    /// Notes: the list on the left (+ New note), the note on the right with
    /// its colour and a toolbar (bold, italic, underline, strikethrough,
    /// bullets, colours, delete) — like Windows Sticky Notes.
    fn page_notes(&mut self, bx: f32, by: f32, bw: f32, bh: f32, a: f32) {
        use crate::tools::{Note, NOTE_COLORS};
        let g = gfx::g();
        let d = crate::data();
        // 2 : 8 — the list on the left, the note on the right.
        let lw = (bw * 0.2).round().max(96.0);
        // Keep a note open when there is one.
        if self.note_sel.and_then(|id| d.notes.get(id)).is_none() {
            self.note_sel = d.notes.notes.first().map(|n| n.id);
        }
        // ---- The list ----
        let nr = rf(bx, by + 36.0, lw, 26.0);
        let hot = self.hot == Some(Hit::NoteNew);
        self.surf.fill_rr(nr, 13.0, self.fg(if hot { 0.14 } else { 0.07 } * a));
        self.surf.text("\u{E710}", &self.f.icon_s, rf(nr.left + 8.0, nr.top, 16.0, 26.0), self.fg(0.85 * a));
        self.surf.text(tr("New note"), &self.f.tiny, rf(nr.left + 28.0, nr.top, lw - 32.0, 26.0), self.fg(0.85 * a));
        self.hits.push((Hit::NoteNew, nr));
        let top = by + 68.0;
        let bottom = by + bh;
        let rh = 38.0;
        let n = d.notes.notes.len();
        let max = (n as f32 * rh - (bottom - top)).max(0.0);
        self.notes_scroll = self.notes_scroll.clamp(0.0, max);
        let area = rf(bx - 2.0, top, lw + 4.0, bottom - top);
        self.zones.push((9, area));
        self.surf.clip(area);
        let fmt_day = |t: i64| util::short_day(&util::systime(util::local_secs(t)));
        for (i, note) in d.notes.notes.iter().enumerate() {
            let y = top + i as f32 * rh - self.notes_scroll;
            if y + rh < top || y > bottom {
                continue;
            }
            let r = rf(bx, y, lw, rh - 4.0);
            let sel = self.note_sel == Some(note.id);
            let col = rgb(NOTE_COLORS[note.color as usize % 6], 1.0);
            if sel {
                self.surf.fill_rr(r, 8.0, with_alpha(col, 0.22 * a));
            } else if self.hot == Some(Hit::NoteItem(i)) {
                self.surf.fill_rr(r, 8.0, self.fg(0.06 * a));
            }
            self.surf.fill_rr(rf(r.left + 3.0, r.top + 6.0, 3.5, rh - 16.0), 1.75, with_alpha(col, a));
            let title = note.title();
            let (t, ta) = if title.is_empty() { (tr("Empty note").to_string(), 0.45) } else { (title, 0.9) };
            // Title, and the day it was last changed under it.
            self.surf.text(&ellip(g, &t, &self.f.tiny, lw - 18.0), &self.f.tiny, rf(r.left + 12.0, r.top + 2.0, lw - 16.0, 17.0), self.fg(ta * a));
            self.surf.text(&fmt_day(note.updated), &self.f.small, rf(r.left + 12.0, r.top + 18.0, lw - 16.0, 13.0), self.fg(0.4 * a));
            self.hits.push((Hit::NoteItem(i), r));
        }
        self.surf.unclip();
        if n == 0 {
            self.surf.text_wrap(tr("No notes yet."), &self.f.wrap_src, rf(bx + 4.0, top + 4.0, lw - 8.0, 40.0), self.fg(0.4 * a));
        }
        // ---- The note ----
        let ex = bx + lw + 12.0;
        let ew_ = bx + bw - ex;
        let Some(note) = self.note_sel.and_then(|id| d.notes.get(id)).cloned() else {
            self.surf.text(tr("Write something down: New note"), &self.f.body_c, rf(ex, by, ew_, bh), self.fg(0.4 * a));
            return;
        };
        // A note you open settles in: fades up into place.
        let ni = self.ui.note_in.v.clamp(0.0, 1.0);
        let a = a * (0.25 + 0.75 * ni);
        let by = by + (1.0 - ni) * 10.0;
        let col = rgb(NOTE_COLORS[note.color as usize % 6], 1.0);
        let card = rf(ex, by, ew_, bh - (1.0 - ni) * 10.0);
        let bh = card.bottom - card.top;
        self.surf.fill_rr(card, 12.0, with_alpha(col, 0.16 * a));
        self.surf.fill_rr(rf(card.left, card.top, ew_, 5.0), 2.5, with_alpha(col, 0.9 * a));
        let editing = self.edit == Some(EditTarget::Note);
        let text = if editing { self.edit_buf.clone() } else { note.text.clone() };
        let (shown, styles) = Note::display(&text);
        let tb_h = 30.0;
        let tr_ = rf(card.left + 12.0, card.top + 12.0, ew_ - 24.0, bh - 18.0 - tb_h);
        self.hits.push((Hit::NoteText, tr_));
        let font = self.f.note.clone();
        let th = g.measure_h(if shown.is_empty() { " " } else { &shown }, &font, tr_.right - tr_.left);
        let vh = tr_.bottom - tr_.top;
        let max_s = (th - vh + 4.0).max(0.0);
        self.note_text_scroll = self.note_text_scroll.clamp(0.0, max_s);
        self.zones.push((10, tr_));
        self.surf.clip(rf(tr_.left - 2.0, tr_.top - 2.0, tr_.right - tr_.left + 4.0, vh + 4.0));
        let body = rf(tr_.left, tr_.top - self.note_text_scroll, tr_.right - tr_.left, th + 40.0);
        if shown.is_empty() && !editing {
            self.surf.text(tr("Take a note…"), &font, body, self.fg(0.4 * a));
        } else {
            if editing && self.edit_all && !shown.is_empty() {
                self.surf.fill_rr(rf(body.left - 3.0, body.top - 1.0, (tr_.right - tr_.left) + 6.0, th + 2.0), 4.0, with_alpha(col, 0.3 * a));
            }
            self.surf.text_rich(&shown, &font, body, self.fg(0.92 * a), &styles);
        }
        if editing && self.caret {
            let (cx, cy, lh) = self.surf.text_end(&shown, &font, tr_.right - tr_.left);
            self.surf.fill_rect(rf(body.left + cx, body.top + cy + 1.0, 1.5, (lh - 2.0).max(10.0)), self.fg(a));
        }
        self.surf.unclip();
        if max_s > 0.0 {
            let bar = vh * vh / (th + 4.0);
            let sy = tr_.top + (vh - bar) * (self.note_text_scroll / max_s);
            self.surf.fill_rr(rf(card.right - 6.0, sy, 2.5, bar), 1.25, self.fg(0.25 * a));
        }
        // ---- Toolbar ----
        let ty = card.bottom - tb_h;
        self.surf.fill_rect(rf(card.left + 8.0, ty - 1.0, ew_ - 16.0, 1.0), self.fg(0.1 * a));
        let on = if editing { Note::styles_at_end(&self.edit_buf) } else { [false; 4] };
        let kind = if editing {
            let ls = self.edit_buf.rfind('\n').map(|i| i + 1).unwrap_or(0);
            crate::tools::list_line(&self.edit_buf[ls..]).kind
        } else {
            0
        };
        let mut x = card.left + 8.0;
        let tools: [(u8, &str, bool); 6] = [(0, "B", on[0]), (1, "I", on[1]), (2, "U", on[2]), (3, "S", on[3]), (4, "\u{E8FD}", kind == 1), (5, "1.", kind == 2)];
        for (k, label, active) in tools {
            let r = rf(x, ty + 3.0, 26.0, 24.0);
            let hot = self.hot == Some(Hit::NoteStyle(k));
            if active {
                self.surf.fill_rr(r, 6.0, with_alpha(col, 0.55 * a));
            } else if hot {
                self.surf.fill_rr(r, 6.0, self.fg(0.1 * a));
            }
            if k == 4 {
                self.surf.text(label, &self.f.icon_s, r, self.fg(0.85 * a));
            } else if k == 5 {
                self.surf.text(label, &self.f.tiny_c, r, self.fg(0.85 * a));
            } else {
                let styles = [(0u32, 1u32, k == 0, k == 1, k == 2, k == 3)];
                let lw_ = g.measure(label, &self.f.note);
                self.surf.text_rich(label, &self.f.note, rf(r.left + (26.0 - lw_) / 2.0, r.top + 3.0, 20.0, 20.0), self.fg(0.85 * a), &styles);
            }
            self.hits.push((Hit::NoteStyle(k), r));
            x += 28.0;
        }
        x += 8.0;
        for (c, hex) in NOTE_COLORS.iter().enumerate() {
            let cxx = x + 8.0;
            let cyy = ty + 15.0;
            let hot = self.hot == Some(Hit::NoteColor(c as u8));
            self.surf.fill_circle(cxx, cyy, if hot { 7.5 } else { 6.5 }, rgb(*hex, a));
            if note.color as usize == c {
                self.surf.stroke_rr(rf(cxx - 9.5, cyy - 9.5, 19.0, 19.0), 9.5, self.fg(0.8 * a), 1.3);
            }
            self.hits.push((Hit::NoteColor(c as u8), rf(cxx - 9.0, cyy - 9.0, 18.0, 18.0)));
            x += 20.0;
        }
        let dr = rf(card.right - 32.0, ty + 3.0, 26.0, 24.0);
        let hot = self.hot == Some(Hit::NoteDelete);
        if hot {
            self.surf.fill_rr(dr, 6.0, rgb(pal::RED, 0.2 * a));
        }
        self.surf.text("\u{E74D}", &self.f.icon_s, dr, if hot { rgb(pal::RED, a) } else { self.fg(0.6 * a) });
        self.hits.push((Hit::NoteDelete, dr));
    }

    /// To-do and Notes, one switch apart: the two cross-fade and slide.
    pub(super) fn page_todo(&mut self, bx: f32, by: f32, bw: f32, bh: f32, a: f32) {
        let t = self.ui.todo_view.v.clamp(0.0, 1.0);
        let e = t * t * (3.0 - 2.0 * t);
        let sw = self.todo_switch(bx, by + 2.0, a, e);
        // The side going away draws, but takes no clicks.
        if e < 0.999 {
            let (h0, z0) = (self.hits.len(), self.zones.len());
            self.todo_list(bx, by, bw, bh, a * (1.0 - e), sw, -e * 24.0);
            if self.notes_view {
                self.hits.truncate(h0);
                self.zones.truncate(z0);
            }
        }
        if e > 0.001 {
            let (h0, z0) = (self.hits.len(), self.zones.len());
            self.page_notes(bx + (1.0 - e) * 24.0, by, bw, bh, a * e);
            if !self.notes_view {
                self.hits.truncate(h0);
                self.zones.truncate(z0);
            }
        }
    }

    /// The to-do list; `dx` slides the rows (the input box stays by the switch).
    fn todo_list(&mut self, bx: f32, by: f32, bw: f32, bh: f32, a: f32, sw: f32, dx: f32) {
        self.draw_field(rf(bx + sw + 8.0, by, bw - sw - 8.0, 32.0), EditTarget::Todo, tr("Add a task… end with @14:30 for a reminder"), a, Hit::TodoInput);
        let bx = bx + dx;
        let d = crate::data();
        let tasks = d.todo.tasks.clone();
        let fmt = config::cur().i("clock_fmt");
        if tasks.is_empty() {
            self.surf.text(tr("Nothing to do. Enjoy!"), &self.f.body_c, rf(bx, by + 40.0, bw, bh - 40.0), self.fg(0.45 * a));
            return;
        }
        let rows = 4usize;
        self.todo_scroll = self.todo_scroll.min(tasks.len().saturating_sub(rows));
        let now = util::unix_now();
        for (k, i) in (self.todo_scroll..tasks.len()).take(rows).enumerate() {
            let t = &tasks[i];
            let y = by + 42.0 + k as f32 * 29.0;
            let row = rf(bx, y, bw, 27.0);
            let hot_row = matches!(self.hot, Some(Hit::TodoCheck(j)) | Some(Hit::TodoDelete(j)) if j == i);
            if hot_row {
                self.surf.fill_rr(row, 8.0, self.fg(0.06 * a));
            }
            let ccx = bx + 14.0;
            let ccy = y + 13.5;
            if t.done {
                self.surf.fill_circle(ccx, ccy, 8.0, self.el(pal::GREEN, a));
                self.surf.text("\u{E73E}", &self.f.icon_s, rf(ccx - 8.0, ccy - 8.0, 16.0, 16.0), rgb(0x000000, 0.85 * a));
            } else {
                self.surf.stroke_rr(rf(ccx - 7.5, ccy - 7.5, 15.0, 15.0), 7.5, self.fg(0.55 * a), 1.5);
            }
            let ta = if t.done { 0.4 } else { 0.95 };
            let tx = bx + 32.0;
            let due_w = if t.due.is_some() { 70.0 } else { 0.0 };
            let tw = bw - 32.0 - due_w - 30.0;
            self.surf.text(&t.text, &self.f.body, rf(tx, y, tw, 27.0), self.fg(ta * a));
            if t.done {
                let sw = gfx::g().measure(&t.text, &self.f.body).min(tw);
                self.surf.fill_rect(rf(tx, ccy, sw, 1.2), self.fg(0.45 * a));
            }
            if let Some(due) = t.due {
                let st = util::systime(util::local_secs(due));
                let mut s = util::clock_string(&st, fmt);
                if util::local_day(due) != util::local_day(now) {
                    s = format!("{} {s}", util::short_day(&st));
                }
                let late = due <= now && !t.done;
                let r = rf(bx + bw - 30.0 - due_w, y + 4.0, due_w - 6.0, 19.0);
                self.surf.fill_rr(r, 9.5, if late { rgb(0xff453a, 0.22 * a) } else { self.fg(0.08 * a) });
                self.surf.text(&s, &self.f.tiny_c, r, if late { rgb(0xff6b60, a) } else { self.fg(0.7 * a) });
            }
            self.hits.push((Hit::TodoCheck(i), rf(bx, y, bw - 30.0, 27.0)));
            let xr = rf(bx + bw - 28.0, y + 1.0, 26.0, 25.0);
            if hot_row {
                self.surf.text("\u{E711}", &self.f.icon_s, xr, self.fg(if self.hot == Some(Hit::TodoDelete(i)) { 0.95 } else { 0.5 } * a));
            }
            self.hits.push((Hit::TodoDelete(i), xr));
        }
        if tasks.len() > rows {
            let more = tasks.len() - rows - self.todo_scroll;
            if more > 0 {
                self.surf.text(&format!("{more} {} ↓", tr("more")), &self.f.tiny_r, rf(bx + bw - 120.0, by + bh - 12.0, 120.0, 14.0), self.fg(0.45 * a));
            }
        }
    }

    // ------------------------------------------------------------------
    // Shelf
    // ------------------------------------------------------------------

    pub(super) fn page_shelf(&mut self, bx: f32, by: f32, bw: f32, bh: f32, a: f32) {
        let d = crate::data();
        let n = d.shelf.items.len();
        // Linked with another PC: its shelf is a tab away.
        let remote = crate::link::view().and_then(|v| v.session.map(|s| (s.0, v.remote_shelf)));
        if remote.is_none() {
            self.shelf_remote = false;
        }
        let (bx, by, bh) = if let Some((peer, _)) = &remote {
            if !self.drag_in {
                let g = gfx::g();
                let mut x = bx;
                for (label, on) in [(tr("This PC").to_string(), false), (peer.clone(), true)] {
                    let w = g.measure(&label, &self.f.tiny).min(160.0) + 22.0;
                    let r = rf(x, by - 1.0, w, 22.0);
                    let sel = self.shelf_remote == on;
                    let hot = self.hot == Some(Hit::ShelfTab(on));
                    self.surf.fill_rr(r, 11.0, if sel { self.el(pal::PURPLE, 0.85 * a) } else { self.fg(if hot { 0.16 } else { 0.07 } * a) });
                    self.surf.text(&label, &self.f.tiny_c, r, if sel { Island::text_on(self.ic(pal::PURPLE, 1.0), a) } else { self.fg(0.8 * a) });
                    self.hits.push((Hit::ShelfTab(on), r));
                    x += w + 6.0;
                }
            }
            (bx, by + 26.0, bh - 26.0)
        } else {
            (bx, by, bh)
        };
        if self.shelf_remote && !self.drag_in {
            if let Some((peer, items)) = remote {
                self.remote_shelf(&peer, &items, bx, by, bw, bh, a);
                return;
            }
        }
        if n == 0 || self.drag_in {
            let r = rf(bx + 2.0, by + 4.0, bw - 4.0, bh - 8.0);
            let col = if self.drag_in { self.ic(pal::PURPLE, a) } else { self.fg(0.3 * a) };
            if self.drag_in {
                self.surf.fill_rr(r, 16.0, self.el(pal::PURPLE, 0.10 * a));
            }
            self.surf.dashed_rr(r, 16.0, col, 1.5);
            let (t, s) = if self.drag_in { (tr("Release to add"), tr("Files stay where they are")) } else { (tr("Drop files here"), tr("Drag them out anytime · click to open")) };
            self.surf.text("\u{E8B7}", &self.f.icon_big, rf(bx, by + 18.0, bw, 50.0), col);
            self.surf.text(t, &self.f.title_c, rf(bx, by + 76.0, bw, 24.0), self.fg(a));
            self.surf.text(s, &self.f.artist_c, rf(bx, by + 102.0, bw, 20.0), self.fg(0.5 * a));
            if !self.drag_in {
                self.shelf_folder_button(bx + bw - 10.0, by + 12.0, a);
            }
            // Keep the centred texts centred.
            return;
        }
        self.surf.text(&format!("{n} {}", tr(if n == 1 { "item" } else { "items" })), &self.f.label, rf(bx, by, 120.0, 20.0), self.fg(0.5 * a));
        let cr = rf(bx + bw - 64.0, by - 1.0, 64.0, 22.0);
        let hot = self.hot == Some(Hit::ShelfClear);
        self.surf.fill_rr(cr, 11.0, self.fg(if hot { 0.16 } else { 0.08 } * a));
        self.surf.text(tr("Clear all"), &self.f.tiny_c, cr, self.fg(0.85 * a));
        self.hits.push((Hit::ShelfClear, cr));
        self.shelf_folder_button(cr.left - 6.0, by - 1.0, a);

        let tile_w = 86.0;
        let maxs = (n as f32 * tile_w - bw).max(0.0);
        self.shelf_scroll = self.shelf_scroll.min(maxs);
        let first = (self.shelf_scroll / tile_w) as usize;
        let count = (bw / tile_w) as usize + 2;
        let px = (52.0 * self.scale).ceil() as u32;
        d.shelf.ensure_icons(px, first..(first + count).min(n));
        let area = rf(bx, by + 24.0, bw, bh - 24.0);
        self.surf.clip(area);
        for i in first..(first + count).min(n) {
            let it = &d.shelf.items[i];
            let x = bx + i as f32 * tile_w - self.shelf_scroll;
            let tile = rf(x + 3.0, by + 26.0, tile_w - 6.0, bh - 28.0);
            let hot_t = matches!(self.hot, Some(Hit::ShelfItem(j)) | Some(Hit::ShelfRemove(j)) | Some(Hit::ShelfReveal(j)) if j == i);
            if hot_t {
                self.surf.fill_rr(tile, 12.0, self.fg(0.08 * a));
            }
            let ir = rf(x + (tile_w - 52.0) / 2.0, by + 36.0, 52.0, 52.0);
            match &it.icon {
                Some(img) => self.surf.image(img, ir, a),
                None => {
                    self.surf.fill_rr(ir, 10.0, self.fg(0.1 * a));
                    self.surf.text("\u{E8A5}", &self.f.icon_ctl, ir, self.fg(0.6 * a));
                }
            }
            self.surf.text_wrap(&it.name, &self.f.tiny_c, rf(x + 6.0, by + 94.0, tile_w - 12.0, 30.0), self.fg(0.85 * a));
            self.hits.push((Hit::ShelfItem(i), tile));
            // Under the name: open its folder with it selected.
            let fr = rf(x + (tile_w - 54.0) / 2.0, (by + 128.0).min(tile.bottom - 24.0), 54.0, 20.0);
            let hot_f = self.hot == Some(Hit::ShelfReveal(i));
            self.surf.fill_rr(fr, 10.0, self.fg(if hot_f { 0.2 } else { 0.08 } * a));
            self.surf.text("\u{E838}", &self.f.icon_s, rf(fr.left + 6.0, fr.top, 14.0, 20.0), if hot_f { self.ic(pal::PURPLE, a) } else { self.fg(0.75 * a) });
            self.surf.text(tr("Folder"), &self.f.tiny, rf(fr.left + 22.0, fr.top, 32.0, 20.0), self.fg(0.75 * a));
            self.hits.push((Hit::ShelfReveal(i), fr));
            if hot_t {
                let xr = rf(tile.right - 22.0, tile.top + 2.0, 20.0, 20.0);
                self.surf.fill_circle(xr.left + 10.0, xr.top + 10.0, 9.0, self.fg(if self.hot == Some(Hit::ShelfRemove(i)) { 0.3 } else { 0.16 } * a));
                self.surf.text("\u{E711}", &self.f.icon_s, xr, self.fg(0.9 * a));
                self.hits.push((Hit::ShelfRemove(i), xr));
            }
        }
        self.surf.unclip();
    }

    /// The other PC's shelf: click one to get it (it lands in Downloads\\Phemio
    /// and on the shelf here).
    fn remote_shelf(&mut self, peer: &str, items: &[(u64, String, u64, bool)], bx: f32, by: f32, bw: f32, bh: f32, a: f32) {
        let n = items.len();
        if n == 0 {
            self.surf.text("\u{E8B7}", &self.f.icon_big, rf(bx, by + 6.0, bw, 50.0), self.fg(0.3 * a));
            self.surf.text(&format!("{} {peer}", tr("Nothing on the shelf of")), &self.f.artist_c, rf(bx, by + 62.0, bw, 20.0), self.fg(0.55 * a));
            return;
        }
        let lv = crate::link::view();
        let have = lv.as_ref().map(|v| v.have.clone()).unwrap_or_default();
        let left = items.iter().filter(|x| !have.contains(&x.0)).count();
        let xfer = lv.as_ref().and_then(|v| v.xfer.clone().filter(|x| !x.out));
        if let Some(x) = xfer {
            // Download progress: percentage, speed, time left.
            self.xfer_bar(&x, bx, by - 2.0, bw - 90.0, a);
        } else {
            let head = lv.and_then(|v| v.xfer_msg).unwrap_or_else(|| format!("{n} {} · {}", tr(if n == 1 { "item" } else { "items" }), tr("click to get")));
            self.surf.text(&head, &self.f.label, rf(bx, by, bw - 220.0, 20.0), self.fg(0.5 * a));
        }
        // "Get all" fetches only what isn't here yet; with everything here it's a tick.
        let green = rgb(pal::GREEN, 1.0);
        let cr = if left == 0 {
            let label = format!("\u{2713} {}", tr("All here"));
            let w = gfx::g().measure(&label, &self.f.tiny) + 22.0;
            let cr = rf(bx + bw - w, by - 1.0, w, 22.0);
            self.surf.fill_rr(cr, 11.0, with_alpha(green, 0.16 * a));
            self.surf.text(&label, &self.f.tiny_c, cr, with_alpha(green, a));
            cr
        } else {
            let label = if left < n { format!("{} ({left})", tr("Get the rest")) } else { tr("Get all").to_string() };
            let w = gfx::g().measure(&label, &self.f.tiny) + 22.0;
            let cr = rf(bx + bw - w, by - 1.0, w, 22.0);
            let hot = self.hot == Some(Hit::ShelfGetAll);
            self.surf.fill_rr(cr, 11.0, self.fg(if hot { 0.16 } else { 0.08 } * a));
            self.surf.text(&label, &self.f.tiny_c, cr, self.fg(0.85 * a));
            self.hits.push((Hit::ShelfGetAll, cr));
            cr
        };
        // Where the files land: open that folder.
        self.shelf_folder_button(cr.left - 6.0, by - 1.0, a);
        let tile_w = 86.0;
        let maxs = (n as f32 * tile_w - bw).max(0.0);
        self.shelf_scroll = self.shelf_scroll.min(maxs);
        let first = (self.shelf_scroll / tile_w) as usize;
        let count = (bw / tile_w) as usize + 2;
        let area = rf(bx, by + 24.0, bw, bh - 24.0);
        self.surf.clip(area);
        for i in first..(first + count).min(n) {
            let (id, name, size, dir) = &items[i];
            let here = have.contains(id);
            let x = bx + i as f32 * tile_w - self.shelf_scroll;
            let tile = rf(x + 3.0, by + 26.0, tile_w - 6.0, bh - 28.0);
            // Not here yet: click to get it. Here: click to open it.
            let hot_t = self.hot == Some(Hit::ShelfGet(i)) || self.hot == Some(Hit::ShelfOpen(i));
            if hot_t {
                self.surf.fill_rr(tile, 12.0, self.fg(0.08 * a));
            }
            let ir = rf(x + (tile_w - 46.0) / 2.0, by + 34.0, 46.0, 46.0);
            self.surf.fill_rr(ir, 10.0, self.el(pal::PURPLE, 0.16 * a));
            self.surf.text(if *dir { "\u{E8B7}" } else { "\u{E8A5}" }, &self.f.icon_ctl, ir, self.ic(pal::PURPLE, a));
            let dr = rf(ir.right - 14.0, ir.bottom - 14.0, 18.0, 18.0);
            if here && hot_t {
                // Here: an "open" arrow on hover.
                self.surf.fill_circle(dr.left + 9.0, dr.top + 9.0, 9.0, with_alpha(green, a));
                self.surf.text("\u{E8A7}", &self.f.icon_s, dr, rgb(0xffffff, a));
            } else if here {
                // Already here (same contents in the shelf folder): a green tick.
                self.surf.fill_circle(dr.left + 9.0, dr.top + 9.0, 9.0, with_alpha(green, a));
                self.surf.text("\u{E73E}", &self.f.icon_s, dr, rgb(0xffffff, a));
            } else if hot_t {
                self.surf.fill_circle(dr.left + 9.0, dr.top + 9.0, 9.0, self.el(pal::PURPLE, a));
                self.surf.text("\u{E896}", &self.f.icon_s, dr, rgb(0xffffff, a));
            }
            self.surf.text_wrap(name, &self.f.tiny_c, rf(x + 6.0, by + 84.0, tile_w - 12.0, 28.0), self.fg(0.85 * a));
            let sz = if here && hot_t { tr("Click to open").to_string() } else if here { tr("Downloaded").to_string() } else if *dir { tr("Folder").to_string() } else { human_size(*size) };
            self.surf.text(&sz, &self.f.tiny_c, rf(x + 4.0, tile.bottom - 16.0, tile_w - 8.0, 14.0), if here { with_alpha(green, 0.9 * a) } else { self.fg(0.4 * a) });
            self.hits.push((if here { Hit::ShelfOpen(i) } else { Hit::ShelfGet(i) }, tile));
        }
        self.surf.unclip();
    }

    /// A folder button (right edge at `right`): opens where files from the
    /// other PC land (Settings → Link → shelf folder; Downloads\Phemio).
    fn shelf_folder_button(&mut self, right: f32, y: f32, a: f32) {
        let label = tr("Open folder");
        let w = gfx::g().measure(label, &self.f.tiny) + 38.0;
        let r = rf(right - w, y, w, 22.0);
        let hot = self.hot == Some(Hit::ShelfFolder);
        self.surf.fill_rr(r, 11.0, self.fg(if hot { 0.16 } else { 0.08 } * a));
        self.surf.text("\u{E838}", &self.f.icon_s, rf(r.left + 9.0, y, 14.0, 22.0), self.fg(0.85 * a));
        self.surf.text(label, &self.f.tiny, rf(r.left + 27.0, y, w - 30.0, 22.0), self.fg(0.85 * a));
        self.hits.push((Hit::ShelfFolder, r));
    }

    // ------------------------------------------------------------------
    // Mascot (idle eyes)
    // ------------------------------------------------------------------

    /// Idle mascot: a cat drawn with a few thin lines — two ears, two eyes,
    /// whiskers. It blinks, glances around, tilts its head when curious,
    /// dozes off, and perks up when you come back. Cats don't talk.
    pub(super) fn draw_mascot(&mut self, x: f32, y: f32, w: f32, h: f32, a: f32) {
        match config::cur().i("mascot_style") {
            0 => {
                self.draw_robot(x, y, w, h, a);
                return;
            }
            2 => {
                self.draw_duel(x, y, w, h, a);
                return;
            }
            _ => {}
        }
        let k = (h / 34.0).clamp(0.8, 1.4);
        // Wandering: the cat strolls along the island, bobbing as it walks.
        let room = (w / 2.0 - 40.0 * k).max(0.0);
        let speed = self.walk.vel.abs().min(1.5);
        let t = self.t0.elapsed().as_secs_f32();
        // Smooth bob while walking (sin² has no hard corners), plus hops.
        let bob = (t * 9.0).sin().powi(2) * speed * 1.6 - self.hop.v * 3.5;
        let cx = x + w / 2.0 + self.walk.v.clamp(-1.0, 1.0) * room;
        let cy = y + h / 2.0 + 1.5 - bob;
        let state = if self.mascot == Mascot::Off { self.masc_last } else { self.mascot };
        // Goodbye: a magic trick (see `cat_magic_bye`).
        if state == Mascot::Bye {
            let u = self.bye_at.elapsed().as_secs_f32();
            self.cat_magic_bye(rf(x, y, w, h), cx, cy, k, u, a);
            return;
        }
        // Drumming along: while you type its eyes go down to its paws.
        let typing = (self.tap_at.elapsed().as_secs_f32() < 1.4 || std::env::var_os("ISLET_DEMO_TAPS").is_some()) && state == Mascot::Awake;
        let music = self.media.playing && state == Mascot::Awake;
        let bye_u = self.bye_at.elapsed().as_secs_f32();
        let tilt = if state == Mascot::Bye {
            let left = (1.0 - bye_u / 1.1).max(0.0);
            if left > 0.0 {
                self.keep_anim = true;
            }
            (bye_u * 17.0).sin() * 0.26 * left
        } else {
            self.tilt.v
        };
        let (sn, cs) = tilt.sin_cos();
        // Local (unscaled) point → island coordinates, rotated by the head tilt.
        let p = |px: f32, py: f32| (cx + (px * cs - py * sn) * k, cy + (px * sn + py * cs) * k);
        let line_c = self.fg(0.95 * a);
        let eye_c = self.ic(pal::CYAN, a);
        let whisk_c = self.fg(0.6 * a);
        let sw = 2.9;
        let breath = self.breath.v;
        let ear = self.ear.v;
        let look = self.look.v.clamp(-5.0, 5.0);
        let look_y = self.look_y.v.clamp(-3.0, 3.0) + if typing { 1.4 } else { 0.0 };
        // Ears: open "^" strokes. Positive `ear` droops them; when curious the
        // ear on the high side folds a little; a flick wobbles one ear.
        for side in [-1.0f32, 1.0] {
            let flick = if side == self.flick_side { -self.flick.v * 1.6 } else { 0.0 };
            let e = ear + (side * tilt).max(0.0) * 5.0 + breath * 0.4 + flick;
            let pts = [p(side * 26.0, -5.0), p(side * 22.5 - flick * 0.6 * side, -14.0 + 3.5 * e), p(side * 14.0, -9.0 + e * 0.6)];
            self.surf.stroke_poly(&pts, false, line_c, sw);
        }
        // Eyes.
        let arc = |isl: &Island, ex: f32, ey: f32, up: bool, col: D2D1_COLOR_F| {
            let pts: Vec<_> = (0..=8)
                .map(|i| {
                    let t = i as f32 / 8.0;
                    let bend = (std::f32::consts::PI * t).sin() * 2.2;
                    p(ex - 3.5 + 7.0 * t, if up { ey - bend } else { ey + bend })
                })
                .collect();
            isl.surf.stroke_poly(&pts, false, col, sw);
        };
        match (state, self.eye_mode) {
            (Mascot::Bye, _) => {
                // Goodbye: happy squint.
                for side in [-1.0f32, 1.0] {
                    arc(self, side * 9.0, 2.0, true, eye_c);
                }
            }
            (Mascot::Doze, _) => {
                // Asleep: closed, curved down.
                for side in [-1.0f32, 1.0] {
                    arc(self, side * 9.0, 0.5 + breath * 0.5, false, with_alpha(eye_c, 0.8 * eye_c.a));
                }
            }
            (Mascot::Awake, 1) => {
                // Happy squint: "^ ^".
                for side in [-1.0f32, 1.0] {
                    arc(self, side * 9.0 + look * 0.5, 2.0 + look_y, true, eye_c);
                }
            }
            (Mascot::Awake, 2) => {
                // Sleepy: eyes narrowed to short level dashes.
                for side in [-1.0f32, 1.0] {
                    let ex = side * 9.0 + look;
                    let (x0, y0) = p(ex - 2.6, 1.6 + look_y);
                    let (x1, y1) = p(ex + 2.6, 1.6 + look_y);
                    self.surf.line(x0, y0, x1, y1, eye_c, sw + 0.5);
                }
            }
            _ => {
                // Two short upright lines that blink.
                let open = self.blink.v.clamp(0.08, 1.0);
                for side in [-1.0f32, 1.0] {
                    let base = match state {
                        Mascot::Hello => 9.0,
                        // Curious: the eye on the low side opens wider.
                        _ => 6.5 + (-side * tilt).max(0.0) * 9.0,
                    };
                    let hh = (base * open).max(1.4);
                    let ex = side * 9.0 + look;
                    let (x0, y0) = p(ex, 0.5 + look_y - hh / 2.0);
                    let (x1, y1) = p(ex, 0.5 + look_y + hh / 2.0);
                    self.surf.line(x0, y0, x1, y1, eye_c, sw + 0.5);
                }
            }
        }
        // Rosy cheeks (glowing when it's happy or saying goodbye).
        let rosy = if state == Mascot::Bye || self.eye_mode == 1 { 0.6 } else { 0.38 };
        for side in [-1.0f32, 1.0] {
            let (hx, hy) = p(side * 13.5, 5.0);
            self.surf.fill_circle(hx, hy, 2.6 * k, self.el(pal::RED, rosy * a));
        }
        // Whiskers: two each side, rising and falling a touch with breathing,
        // and waving when the cat is lively (walking, hopping, pouncing…).
        let wy = breath * 0.8;
        let wave = self.wag.v.max(speed * 0.8).clamp(0.0, 1.0);
        for side in [-1.0f32, 1.0] {
            for (i, (y0, y1)) in [(2.0f32, 0.0f32), (5.0, 6.5)].into_iter().enumerate() {
                let sway = (t * 17.0 + side * 1.3 + i as f32 * 0.9).sin() * 2.4 * wave;
                let (ax, ay) = p(side * 17.0, y0 + wy);
                let (mx, my) = p(side * 25.0, (y0 + y1) / 2.0 + wy + sway * 0.45);
                let (bx, by) = p(side * 32.0, y1 + wy + sway);
                self.surf.stroke_poly(&[(ax, ay), (mx, my), (bx, by)], false, whisk_c, 2.2);
            }
        }
        if wave > 0.01 {
            self.keep_anim = true;
        }
        // Nose and mouth.
        let since = |t: Option<Instant>| t.map(|t| t.elapsed().as_secs_f32()).unwrap_or(99.0);
        let tt = self.t0.elapsed().as_secs_f32();
        let (mouth, open) = if state == Mascot::Doze {
            (4, 0.0)
        } else if self.mouth_o {
            (2, 0.0)
        } else if state == Mascot::Hello {
            (3, 0.8)
        } else if since(self.meow_at) < 0.7 {
            self.keep_anim = true;
            (3, (since(self.meow_at) / 0.7 * PI_F).sin())
        } else if self.eye_mode == 2 {
            (4, 0.0)
        } else if since(self.blep_at) < 1.4 {
            (7, 0.0)
        } else if music {
            // Singing along, on the beat.
            (3, ((tt * 2.0).fract() * PI_F).sin() * 0.8)
        } else if self.eye_mode == 1 || state == Mascot::Hello {
            (1, 0.0)
        } else {
            (0, 0.0)
        };
        self.cat_mouth(&p, k, mouth, open, a);
        if state == Mascot::Awake {
            self.cat_drum(&p, k, music, a);
        }
        if music {
            self.cat_notes(cx, cy, k, a);
        }
    }
}

const PI_F: f32 = std::f32::consts::PI;

impl Island {
    /// The robot mascot: just a face — glowing eyes that blink, red cheeks and
    /// a mouth that smiles, grins, goes flat when bored or "o" when surprised.
    /// Lively: it wanders, hops, glances about and tilts its head (same moves
    /// as the cat), and dozes off with a drifting "z".
    pub(super) fn draw_robot(&mut self, x: f32, y: f32, w: f32, h: f32, a: f32) {
        let k = (h / 34.0).clamp(0.8, 1.4);
        let room = (w / 2.0 - 34.0 * k).max(0.0);
        let speed = self.walk.vel.abs().min(1.5);
        let t = self.t0.elapsed().as_secs_f32();
        let bob = (t * 9.0).sin().powi(2) * speed * 1.4 - self.hop.v * 3.5;
        let cx = x + w / 2.0 + self.walk.v.clamp(-1.0, 1.0) * room;
        let cy = y + h / 2.0 + 2.0 - bob;
        let tilt = self.tilt.v;
        let (sn, cs) = tilt.sin_cos();
        // Landing squash: a touch wider and shorter right after a hop.
        let squash = 1.0 + (self.hop.vel * 0.004).clamp(-0.06, 0.06);
        let p = |px: f32, py: f32| {
            let (px, py) = (px * (2.0 - squash), py * squash);
            (cx + (px * cs - py * sn) * k, cy + (px * sn + py * cs) * k)
        };
        let state = if self.mascot == Mascot::Off { self.masc_last } else { self.mascot };
        let line_c = self.fg(0.95 * a);
        let glow = self.ic(pal::CYAN, a);
        let look = self.look.v.clamp(-5.0, 5.0) * 0.7;
        let look_y = self.look_y.v.clamp(-3.0, 3.0) * 0.6;
        // A rounded rectangle as points (local units), mapped through `p`.
        let rr = |x0: f32, y0: f32, w: f32, h: f32, r: f32| -> Vec<(f32, f32)> {
            let r = r.min(w / 2.0).min(h / 2.0);
            let mut out = Vec::with_capacity(40);
            for (ccx, ccy, a0) in [(x0 + w - r, y0 + r, -90.0f32), (x0 + w - r, y0 + h - r, 0.0), (x0 + r, y0 + h - r, 90.0), (x0 + r, y0 + r, 180.0)] {
                for i in 0..=8 {
                    let ang = (a0 + i as f32 * 90.0 / 8.0).to_radians();
                    out.push(p(ccx + r * ang.cos(), ccy + r * ang.sin()));
                }
            }
            out
        };
        // Goodbye: something special — see `robot_bye`.
        if state == Mascot::Bye {
            let u = self.bye_at.elapsed().as_secs_f32();
            self.robot_bye(&p, k, u, a, glow);
            return;
        }
        // Eyes.
        let open = self.blink.v.clamp(0.06, 1.0);
        let ey = -2.5 + look_y;
        match (state, self.eye_mode) {
            (Mascot::Doze, _) => {
                for side in [-1.0f32, 1.0] {
                    let pts: Vec<_> = (0..=8).map(|i| { let u = i as f32 / 8.0; p(side * 7.0 - 3.0 + 6.0 * u, ey + 1.0 + (std::f32::consts::PI * u).sin() * 1.6) }).collect();
                    self.surf.stroke_poly(&pts, false, with_alpha(glow, 0.8 * glow.a), 2.4);
                }
                // A little "z" drifting up.
                let zt = (t * 0.6).fract();
                let (zx, zy) = p(24.0 + zt * 6.0, -10.0 - zt * 8.0);
                self.surf.text("z", &self.f.tiny_b, rf(zx - 5.0, zy - 7.0, 12.0, 14.0), self.fg(0.7 * a * (1.0 - zt)));
                self.keep_anim = true;
            }
            (_, 1) => {
                // Happy "^ ^".
                for side in [-1.0f32, 1.0] {
                    let ex = side * 7.0 + look;
                    let pts = [p(ex - 3.2, ey + 1.6), p(ex, ey - 1.8), p(ex + 3.2, ey + 1.6)];
                    self.surf.stroke_poly(&pts, false, glow, 2.6);
                }
            }
            (_, 2) => {
                // Bored: half-shut, a lid line on top.
                for side in [-1.0f32, 1.0] {
                    let ex = side * 7.0 + look;
                    let eye = rr(ex - 2.3, ey, 4.6, 2.6, 1.2);
                    self.surf.fill_poly(&eye, glow);
                    let (l0x, l0y) = p(ex - 3.4, ey - 0.6);
                    let (l1x, l1y) = p(ex + 3.4, ey - 0.6);
                    self.surf.line(l0x, l0y, l1x, l1y, line_c, 1.8);
                }
            }
            (_, 3) => {
                // Surprised: round eyes.
                for side in [-1.0f32, 1.0] {
                    let (ex, eyy) = p(side * 7.0 + look, ey);
                    self.surf.fill_circle(ex, eyy, 3.4 * k, glow);
                }
            }
            _ => {
                // Glowing pills that blink; the hello is wide-eyed; when curious
                // the eye on the low side opens wider.
                for side in [-1.0f32, 1.0] {
                    let base = match state {
                        Mascot::Hello => 9.0,
                        _ => 7.0 + (-side * tilt).max(0.0) * 7.0,
                    };
                    let eh = (base * open).max(1.2);
                    let ex = side * 7.0 + look;
                    let eye = rr(ex - 2.2, ey - eh / 2.0, 4.4, eh, 2.1);
                    self.surf.fill_poly(&eye, glow);
                }
            }
        }
        // Red cheeks, glowing brighter when it's happy.
        let happy = self.eye_mode == 1 || self.smile.v > 1.45 || state == Mascot::Hello;
        for side in [-1.0f32, 1.0] {
            let (hx, hy) = p(side * 14.0, 3.0);
            self.surf.fill_circle(hx, hy, 2.4 * k, self.el(pal::RED, if happy { 0.62 } else { 0.4 } * a));
        }
        // Mouth.
        let my = 5.2 + look_y * 0.4;
        let mx = look * 0.5;
        if self.mouth_o {
            let (ox, oy) = p(mx, my + 0.5);
            let ring: Vec<_> = (0..=16).map(|i| { let u = i as f32 / 16.0 * std::f32::consts::TAU; (ox + 2.1 * k * u.cos(), oy + 2.3 * k * u.sin()) }).collect();
            self.surf.stroke_poly(&ring, true, line_c, 2.0);
        } else {
            let sm = self.smile.v.clamp(-0.6, 2.2);
            let half = 4.5 + (sm - 1.0).max(0.0) * 1.2;
            if sm > 1.45 {
                // A big open grin.
                let depth = 2.2 + (sm - 1.45) * 2.5;
                let mut pts = vec![p(mx - half, my - 0.6)];
                for i in 0..=10 {
                    let u = i as f32 / 10.0;
                    pts.push(p(mx - half + 2.0 * half * u, my - 0.6 + (std::f32::consts::PI * u).sin() * depth));
                }
                self.surf.fill_poly(&pts, with_alpha(line_c, 0.9 * line_c.a));
            } else if sm.abs() < 0.2 {
                // Flat (bored): one straight stroke.
                let (l0x, l0y) = p(mx - half * 0.8, my + 0.8);
                let (l1x, l1y) = p(mx + half * 0.8, my + 0.8);
                self.surf.line(l0x, l0y, l1x, l1y, line_c, 2.2);
            } else {
                let pts: Vec<_> = (0..=10).map(|i| { let u = i as f32 / 10.0; p(mx - half + 2.0 * half * u, my + (std::f32::consts::PI * u).sin() * sm * 1.9) }).collect();
                self.surf.stroke_poly(&pts, false, line_c, 2.2);
            }
        }
    }

    /// The robot's goodbye: a wink and a big grin while a little heart floats
    /// up, then the face switches off like an old TV — squashed into a glowing
    /// line that shrinks to a dot and blinks out.
    fn robot_bye(&mut self, p: &dyn Fn(f32, f32) -> (f32, f32), k: f32, u: f32, a: f32, glow: D2D1_COLOR_F) {
        let line_c = self.fg(0.95 * a);
        let red = self.ic(pal::RED, 1.0);
        if u < 1.25 {
            self.keep_anim = true;
        }
        let ease = |t: f32| t * t * (3.0 - 2.0 * t);
        if u < 0.62 {
            // Wink: left eye a happy "^", right eye open — then it winks.
            let wink = ((u - 0.18) / 0.14).clamp(0.0, 1.0) * (1.0 - ((u - 0.40) / 0.12).clamp(0.0, 1.0));
            let ey = -2.5;
            let pts = [p(-10.2, ey + 1.6), p(-7.0, ey - 1.8), p(-3.8, ey + 1.6)];
            self.surf.stroke_poly(&pts, false, glow, 2.6);
            let eh = (8.0 * (1.0 - wink)).max(1.2);
            let (ex, eyy) = p(7.0, ey);
            self.surf.fill_rr(rf(ex - 2.2 * k, eyy - eh * k / 2.0, 4.4 * k, eh * k), 2.0 * k, glow);
            // Red cheeks aglow.
            for side in [-1.0f32, 1.0] {
                let (hx, hy) = p(side * 14.0, 3.0);
                self.surf.fill_circle(hx, hy, 2.6 * k, with_alpha(red, 0.7 * a));
            }
            // Big grin.
            let mut pts = vec![p(-6.5, 4.6)];
            for i in 0..=10 {
                let t = i as f32 / 10.0;
                pts.push(p(-6.5 + 13.0 * t, 4.6 + (std::f32::consts::PI * t).sin() * 4.2));
            }
            self.surf.fill_poly(&pts, line_c);
            // A little heart floats up and away.
            let ht = (u / 0.62).clamp(0.0, 1.0);
            let (hx, hy) = p(15.0 + ht * 7.0, -4.0 - ht * 12.0);
            let hs = (0.75 + ht * 0.5) * k;
            let heart: Vec<(f32, f32)> = (0..32)
                .map(|i| {
                    let t = i as f32 / 32.0 * std::f32::consts::TAU;
                    let xx = 16.0 * t.sin().powi(3);
                    let yy = 13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos();
                    (hx + xx * 0.22 * hs, hy - yy * 0.22 * hs)
                })
                .collect();
            self.surf.fill_poly(&heart, with_alpha(red, (1.0 - ht * 0.6) * a));
        } else {
            // Switching off: everything squeezes into a bright line…
            let v = ((u - 0.62) / 0.28).clamp(0.0, 1.0);
            let w = ((u - 0.90) / 0.25).clamp(0.0, 1.0);
            let (cx, cy) = p(0.0, 0.5);
            if w <= 0.0 {
                let hh = (12.0 * (1.0 - ease(v))).max(1.6) * k;
                let ww = (26.0 + 8.0 * ease(v)) * k;
                self.surf.fill_rr(rf(cx - ww / 2.0, cy - hh / 2.0, ww, hh), hh / 2.0, with_alpha(glow, (0.25 + 0.6 * v) * a));
                self.surf.fill_rr(rf(cx - ww * 0.4, cy - 0.8 * k, ww * 0.8, 1.6 * k), 0.8 * k, with_alpha(rgb(0xffffff, 1.0), v * a));
            } else {
                // …that shrinks to a dot and blinks out.
                let ww = (34.0 * (1.0 - ease(w))).max(2.4) * k;
                let fade = 1.0 - ((w - 0.7) / 0.3).clamp(0.0, 1.0);
                self.surf.fill_rr(rf(cx - ww / 2.0, cy - 1.2 * k, ww, 2.4 * k), 1.2 * k, with_alpha(rgb(0xffffff, 1.0), fade * a));
                self.surf.fill_circle(cx, cy, (2.0 + 2.5 * w) * k, with_alpha(glow, 0.35 * fade * a));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Vector weather icons (crisp at any size, no icon font needed)
// ---------------------------------------------------------------------------

// Outlines are built from circle arcs by hand (no geometry boolean ops), so
// they look the same on every Direct2D implementation.

const TAU: f32 = std::f32::consts::TAU;

/// Points along a circle from angle `a0` to `a1` (radians, y down), increasing.
fn arc_pts(out: &mut Vec<(f32, f32)>, cx: f32, cy: f32, r: f32, a0: f32, mut a1: f32) {
    while a1 <= a0 {
        a1 += TAU;
    }
    let n = (((a1 - a0) * r / 1.5).ceil() as usize).clamp(6, 64);
    for i in 0..=n {
        let t = a0 + (a1 - a0) * i as f32 / n as f32;
        out.push((cx + r * t.cos(), cy + r * t.sin()));
    }
}

/// Intersection of two circles: the point with the smaller y (upper one).
fn upper_meet(x0: f32, y0: f32, r0: f32, x1: f32, y1: f32, r1: f32) -> (f32, f32) {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let d = (dx * dx + dy * dy).sqrt().max(1e-3);
    let a = (r0 * r0 - r1 * r1 + d * d) / (2.0 * d);
    let h = (r0 * r0 - a * a).max(0.0).sqrt();
    let (mx, my) = (x0 + a * dx / d, y0 + a * dy / d);
    let p1 = (mx + h * dy / d, my - h * dx / d);
    let p2 = (mx - h * dy / d, my + h * dx / d);
    if p1.1 < p2.1 { p1 } else { p2 }
}

/// Outlined cloud, `u` wide, centred on (cx, cy). Filled with `under` first
/// so whatever is behind it (the sun) is hidden.
fn cloud(s: &Surface, cx: f32, cy: f32, u: f32, stroke: D2D1_COLOR_F, under: D2D1_COLOR_F, sw: f32) {
    let y = cy - 0.1 * u;
    let (lx, ly, lr) = (cx - 0.2 * u, y + 0.04 * u, 0.16 * u);
    let (tx, ty, tr_) = (cx + 0.02 * u, y - 0.06 * u, 0.24 * u);
    let (rx, ry, rr) = (cx + 0.22 * u, y + 0.06 * u, 0.14 * u);
    let p_lt = upper_meet(lx, ly, lr, tx, ty, tr_);
    let p_tr = upper_meet(tx, ty, tr_, rx, ry, rr);
    let ang = |cx: f32, cy: f32, p: (f32, f32)| (p.1 - cy).atan2(p.0 - cx);
    let mut pts = Vec::with_capacity(160);
    let half_pi = std::f32::consts::FRAC_PI_2;
    arc_pts(&mut pts, lx, ly, lr, half_pi, ang(lx, ly, p_lt));
    arc_pts(&mut pts, tx, ty, tr_, ang(tx, ty, p_lt), ang(tx, ty, p_tr));
    arc_pts(&mut pts, rx, ry, rr, ang(rx, ry, p_tr), half_pi);
    s.fill_poly(&pts, under);
    s.stroke_poly(&pts, true, stroke, sw);
}

fn sun(s: &Surface, cx: f32, cy: f32, k: f32, c: D2D1_COLOR_F, sw: f32) {
    s.fill_circle(cx, cy, 0.17 * k, with_alpha(c, c.a * 0.16));
    let mut pts = Vec::new();
    arc_pts(&mut pts, cx, cy, 0.17 * k, 0.0, TAU);
    s.stroke_poly(&pts, true, c, sw);
    for i in 0..8 {
        let ang = i as f32 * std::f32::consts::PI / 4.0;
        let (co, sn) = (ang.cos(), ang.sin());
        s.line(cx + co * 0.27 * k, cy + sn * 0.27 * k, cx + co * 0.36 * k, cy + sn * 0.36 * k, c, sw);
    }
}

/// Crescent: a circle with a smaller one bitten out of its upper right.
fn moon(s: &Surface, cx: f32, cy: f32, k: f32, c: D2D1_COLOR_F, sw: f32) {
    let r = 0.25 * k;
    let (ox, oy) = (cx + 0.5 * r, cy - 0.36 * r);
    let rc = r * 0.86;
    // The two crossing points of the circles.
    let (dx, dy) = (ox - cx, oy - cy);
    let d = (dx * dx + dy * dy).sqrt();
    let a = (r * r - rc * rc + d * d) / (2.0 * d);
    let h = (r * r - a * a).max(0.0).sqrt();
    let (mx, my) = (cx + a * dx / d, cy + a * dy / d);
    let p1 = (mx + h * dy / d, my - h * dx / d);
    let p2 = (mx - h * dy / d, my + h * dx / d);
    let toward = dy.atan2(dx);
    let on_o = |p: (f32, f32)| (p.1 - cy).atan2(p.0 - cx);
    let on_c = |p: (f32, f32)| (p.1 - oy).atan2(p.0 - ox);
    let norm = |mut x: f32| {
        while x < 0.0 {
            x += TAU;
        }
        x % TAU
    };
    let contains = |a0: f32, a1: f32, t: f32| {
        let span = norm(a1 - a0);
        norm(t - a0) <= span
    };
    // Outer arc: the long way round, away from the bite.
    let (mut s0, mut s1) = (on_o(p1), on_o(p2));
    if contains(s0, s1, toward) {
        std::mem::swap(&mut s0, &mut s1);
    }
    let mut pts = Vec::new();
    arc_pts(&mut pts, cx, cy, r, s0, s1);
    // Inner arc on the cut circle, from where the outer arc ended back to its start.
    let end_pt = *pts.last().unwrap();
    let start_pt = pts[0];
    let (b0, b1) = (on_c(end_pt), on_c(start_pt));
    let back = toward + std::f32::consts::PI;
    let mut inner = Vec::new();
    if contains(b0, b1, back) {
        arc_pts(&mut inner, ox, oy, rc, b0, b1);
    } else {
        arc_pts(&mut inner, ox, oy, rc, b1, b0);
        inner.reverse();
    }
    pts.extend(inner);
    s.fill_poly(&pts, with_alpha(c, c.a * 0.16));
    s.stroke_poly(&pts, true, c, sw);
}

/// Minimal line-art weather icon for a WMO code, about `k` across.
/// `under` is the colour behind the icon; `mono` draws everything in the text colour.
pub fn draw_wx(s: &Surface, code: i32, day: bool, cx: f32, cy: f32, k: f32, a: f32, dark: bool, under: D2D1_COLOR_F, mono: bool) {
    let sw = (k * 0.05).clamp(1.2, 3.0);
    let pick = |vivid: u32, light_mode: u32| {
        if mono {
            if dark { rgb(0xffffff, 0.9 * a) } else { rgb(0x1c1c1e, 0.9 * a) }
        } else {
            rgb(if dark { vivid } else { light_mode }, a)
        }
    };
    let sun_c = pick(0xffb340, 0xf08c00);
    let moon_c = pick(0xc8c2ff, 0x6e64e6);
    let cloud_c = pick(0xe5e7eb, 0x6b7280);
    let rain_c = pick(0x64d2ff, 0x0a84ff);
    let snow_c = pick(0xbfe9ff, 0x32ade6);
    let bolt_c = pick(0xffd60a, 0xf2a900);
    let under = with_alpha(under, under.a * a);
    let sky = |dx: f32, dy: f32, kk: f32| {
        if day {
            sun(s, cx + dx, cy + dy, kk, sun_c, sw)
        } else {
            moon(s, cx + dx, cy + dy, kk, moon_c, sw)
        }
    };
    // Cloud raised a little when something falls out of it.
    let wet_cloud = || cloud(s, cx, cy - 0.08 * k, 0.86 * k, cloud_c, under, sw);
    match code {
        0 => sky(0.0, 0.0, k * 1.05),
        1 | 2 => {
            sky(-0.15 * k, -0.13 * k, k * 0.78);
            cloud(s, cx + 0.08 * k, cy + 0.1 * k, 0.74 * k, cloud_c, under, sw);
        }
        3 => cloud(s, cx, cy + 0.04 * k, 0.95 * k, cloud_c, under, sw),
        45 | 48 => {
            cloud(s, cx, cy - 0.12 * k, 0.8 * k, cloud_c, under, sw);
            s.line(cx - 0.3 * k, cy + 0.22 * k, cx + 0.22 * k, cy + 0.22 * k, cloud_c, sw);
            s.line(cx - 0.16 * k, cy + 0.34 * k, cx + 0.32 * k, cy + 0.34 * k, cloud_c, sw);
        }
        51..=55 => {
            wet_cloud();
            for i in 0..3 {
                let xx = cx - 0.16 * k + i as f32 * 0.16 * k;
                let yy = cy + 0.27 * k + (i % 2) as f32 * 0.06 * k;
                s.line(xx, yy, xx - 0.01 * k, yy + 0.02 * k, rain_c, sw * 1.15);
            }
        }
        71..=77 | 85 | 86 => {
            wet_cloud();
            for i in 0..3 {
                let xx = cx - 0.17 * k + i as f32 * 0.17 * k;
                let yy = cy + 0.3 * k + (i % 2) as f32 * 0.07 * k;
                let r = 0.055 * k;
                for j in 0..3 {
                    let ang = j as f32 * std::f32::consts::PI / 3.0 + std::f32::consts::FRAC_PI_2;
                    s.line(xx - ang.cos() * r, yy - ang.sin() * r, xx + ang.cos() * r, yy + ang.sin() * r, snow_c, (sw * 0.75).max(1.0));
                }
            }
        }
        95..=99 => {
            wet_cloud();
            let y0 = cy + 0.13 * k;
            s.fill_poly(
                &[(cx + 0.03 * k, y0), (cx - 0.09 * k, y0 + 0.17 * k), (cx + 0.0 * k, y0 + 0.17 * k), (cx - 0.05 * k, y0 + 0.32 * k), (cx + 0.11 * k, y0 + 0.11 * k), (cx + 0.02 * k, y0 + 0.11 * k), (cx + 0.08 * k, y0)],
                bolt_c,
            );
        }
        56 | 57 | 61..=67 | 80..=82 => {
            wet_cloud();
            let freezing = matches!(code, 56 | 57 | 66 | 67);
            let n = if matches!(code, 65 | 82) { 4 } else { 3 };
            let step = 0.42 * k / n as f32;
            for i in 0..n {
                let xx = cx - 0.21 * k + step * (i as f32 + 0.5);
                if freezing && i == 1 {
                    s.fill_circle(xx, cy + 0.33 * k, sw * 0.9, snow_c);
                    continue;
                }
                s.line(xx + 0.035 * k, cy + 0.24 * k, xx - 0.035 * k, cy + 0.38 * k, rain_c, sw);
            }
        }
        _ => cloud(s, cx, cy, 0.9 * k, cloud_c, under, sw),
    }
}


/// A tiny moon, lit by the lunar day (new on the 1st, full on the 15th).
fn draw_moon_glyph(s: &Surface, cx: f32, cy: f32, r: f32, lday: i64, lit: D2D1_COLOR_F, dark: D2D1_COLOR_F) {
    s.fill_circle(cx, cy, r, dark);
    // Phase 0..1..0 over the month; draw the lit part as a squashed disc.
    let ph = ((lday - 1) as f32 / 29.5).clamp(0.0, 1.0);
    let lit_frac = 1.0 - (ph * 2.0 - 1.0).abs();
    if lit_frac < 0.05 {
        s.stroke_poly(&(0..=24).map(|i| { let t = i as f32 / 24.0 * TAU; (cx + r * t.cos(), cy + r * t.sin()) }).collect::<Vec<_>>(), true, lit, 1.0);
        return;
    }
    let waxing = ph < 0.5;
    let mut pts = Vec::with_capacity(50);
    for i in 0..=24 {
        let t = -std::f32::consts::FRAC_PI_2 + i as f32 / 24.0 * std::f32::consts::PI;
        let x = r * t.cos();
        pts.push((cx + if waxing { x } else { -x }, cy + r * t.sin()));
    }
    let k = 1.0 - 2.0 * lit_frac;
    for i in 0..=24 {
        let t = std::f32::consts::FRAC_PI_2 - i as f32 / 24.0 * std::f32::consts::PI;
        let x = r * t.cos() * k;
        pts.push((cx + if waxing { x } else { -x }, cy + r * t.sin()));
    }
    s.fill_poly(&pts, lit);
}

fn human_size(b: u64) -> String {
    let f = b as f64;
    if f < 1024.0 {
        format!("{b} B")
    } else if f < 1024.0 * 1024.0 {
        format!("{:.0} KB", f / 1024.0)
    } else if f < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", f / 1_048_576.0)
    } else {
        format!("{:.2} GB", f / 1_073_741_824.0)
    }
}

pub(super) fn ellip(g: &gfx::Gfx, s: &str, f: &IDWriteTextFormat, w: f32) -> String {
    if g.measure(s, f) <= w {
        return s.to_string();
    }
    let mut t: String = s.to_string();
    while !t.is_empty() && g.measure(&format!("{t}…"), f) > w {
        t.pop();
    }
    format!("{t}…")
}

/// 0 muted, 1–3 how loud (how many waves the speaker shows).
pub(super) fn speaker_level(vol: f32, muted: bool) -> u8 {
    if muted || vol <= 0.001 {
        0
    } else if vol < 0.34 {
        1
    } else if vol < 0.67 {
        2
    } else {
        3
    }
}
