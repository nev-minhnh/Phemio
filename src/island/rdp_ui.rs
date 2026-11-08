//! The Remote Desktop page: computer, user name, password → Windows' own
//! Remote Desktop client.

use super::*;

impl Island {
    /// A field that shows its value when you're not typing in it.
    pub(super) fn rdp_field(&mut self, r: D2D_RECT_F, t: EditTarget, label: &str, value: &str, secret: bool, a: f32, hit: Hit) {
        if self.edit == Some(t) || value.is_empty() {
            self.draw_field(r, t, label, a, hit);
            return;
        }
        let hot = self.hot == Some(hit);
        self.surf.fill_rr(r, 10.0, self.fg(if hot { 0.1 } else { 0.07 } * a));
        let shown = if secret { "\u{2022}".repeat(value.chars().count().min(24)) } else { value.to_string() };
        let lw = gfx::g().measure(label, &self.f.tiny) + 8.0;
        self.surf.text(label, &self.f.tiny, rf(r.left + 12.0, r.top, lw, r.bottom - r.top), self.fg(0.4 * a));
        self.surf.text(&shown, &self.f.body, rf(r.left + 12.0 + lw, r.top, r.right - r.left - 24.0 - lw, r.bottom - r.top), self.fg(0.92 * a));
        self.hits.push((hit, r));
    }

    pub(super) fn page_rdp(&mut self, bx: f32, by: f32, bw: f32, bh: f32, a: f32) {
        let g = gfx::g();
        let accent = self.ic(pal::CYAN, 1.0);
        let lw = (bw * 0.6).round();
        let fh = 28.0;
        let gap = 6.0;
        // ---- Left: the three fields ----
        let (host, user, pass) = (self.rdp.host.clone(), self.rdp.user.clone(), self.rdp.pass.clone());
        let saved = !host.is_empty() && pass.is_empty() && self.rdp_saved_for.as_deref() == Some(host.as_str());
        self.rdp_field(rf(bx, by, lw, fh), EditTarget::RdpHost, tr("Computer (name or IP)"), &host, false, a, Hit::RdpField(0));
        self.rdp_field(rf(bx, by + fh + gap, lw, fh), EditTarget::RdpUser, tr("User name"), &user, false, a, Hit::RdpField(1));
        let pl = if saved { tr("Password (saved in Windows)") } else { tr("Password") };
        self.rdp_field(rf(bx, by + 2.0 * (fh + gap), lw, fh), EditTarget::RdpPass, pl, &pass, true, a, Hit::RdpField(2));
        // ---- Options and Connect ----
        let oy = by + 3.0 * (fh + gap) + 2.0;
        let mut x = bx;
        let opts = [(0u8, tr("Remember password"), self.rdp.remember), (1, tr("Full screen"), self.rdp.full), (2, tr("All screens"), self.rdp.all_screens)];
        for (k, label, on) in opts {
            let w = g.measure(label, &self.f.tiny) + 34.0;
            let r = rf(x, oy, w, 24.0);
            let hot = self.hot == Some(Hit::RdpOpt(k));
            if on {
                self.surf.fill_rr(r, 12.0, with_alpha(accent, if hot { 0.34 } else { 0.22 } * a));
            } else {
                self.surf.stroke_rr(rf(r.left + 0.5, r.top + 0.5, w - 1.0, 23.0), 11.5, self.fg(if hot { 0.4 } else { 0.22 } * a), 1.0);
            }
            let tc = if on { with_alpha(accent, a) } else { self.fg(0.6 * a) };
            self.surf.text(if on { "\u{E73E}" } else { "" }, &self.f.icon_s, rf(r.left + 8.0, r.top, 14.0, 24.0), tc);
            self.surf.text(label, &self.f.tiny, rf(r.left + 24.0, r.top, w - 26.0, 24.0), tc);
            self.hits.push((Hit::RdpOpt(k), r));
            x += w + 6.0;
        }
        let cl = tr("Connect");
        let cw = g.measure(cl, &self.f.body) + 44.0;
        let cr = rf(bx + lw - cw, oy + 30.0, cw, 30.0);
        let ready = !host.trim().is_empty();
        let hot = self.hot == Some(Hit::RdpGo);
        self.surf.fill_rr(cr, 15.0, with_alpha(accent, if !ready { 0.35 } else if hot { 1.0 } else { 0.88 } * a));
        self.surf.text("\u{E8AF}", &self.f.icon_s, rf(cr.left + 12.0, cr.top, 16.0, 30.0), Island::text_on(accent, a));
        self.surf.text(cl, &self.f.body, rf(cr.left + 32.0, cr.top, cw - 36.0, 30.0), Island::text_on(accent, a));
        if ready {
            self.hits.push((Hit::RdpGo, cr));
        }
        // What happened last.
        let note = self.rdp_msg.as_ref().filter(|m| m.2.elapsed().as_secs() < 10).map(|m| (m.0.clone(), m.1));
        let (nt, nc) = match note {
            Some((t, err)) => (t, if err { rgb(pal::RED, 0.9 * a) } else { self.fg(0.6 * a) }),
            None => (tr("Windows' Remote Desktop opens and signs in for you").to_string(), self.fg(0.38 * a)),
        };
        self.surf.text_wrap(&nt, &self.f.wrap_src, rf(bx, oy + 30.0, (lw - cw - 10.0).max(60.0), 32.0), nc);
        // ---- Right: the linked PC and recent computers ----
        let rx = bx + lw + 18.0;
        let rw = bx + bw - rx;
        let mut y = by;
        let linked = crate::link::view().and_then(|v| v.peer_ip.map(|ip| (v.session.map(|s| s.0).or_else(|| v.paired.first().map(|p| p.1.clone())).unwrap_or_default(), ip)));
        if let Some((name, ip)) = linked {
            let r = rf(rx, y, rw, 30.0);
            let hot = self.hot == Some(Hit::RdpLinked);
            self.surf.fill_rr(r, 10.0, self.el(pal::INDIGO, if hot { 0.3 } else { 0.18 } * a));
            self.surf.text("\u{E71B}", &self.f.icon_s, rf(r.left + 8.0, r.top, 18.0, 30.0), self.ic(pal::INDIGO, a));
            let t = if name.is_empty() { ip.clone() } else { format!("{name} · {ip}") };
            self.surf.text(&t, &self.f.tiny, rf(r.left + 30.0, r.top, rw - 36.0, 30.0), self.fg(0.9 * a));
            self.hits.push((Hit::RdpLinked, r));
            y += 36.0;
        }
        self.surf.text(tr("Recent"), &self.f.label, rf(rx, y, rw, 16.0), self.fg(0.45 * a));
        y += 20.0;
        let rec = crate::rdp::recents();
        if rec.is_empty() {
            self.surf.text_wrap(tr("Computers you connect to show up here."), &self.f.wrap_src, rf(rx, y, rw, 32.0), self.fg(0.38 * a));
        }
        for (i, (h, u)) in rec.iter().enumerate() {
            if y + 26.0 > by + bh {
                break;
            }
            let r = rf(rx - 4.0, y, rw + 4.0, 26.0);
            let hot = matches!(self.hot, Some(Hit::RdpRecent(j)) | Some(Hit::RdpForget(j)) if j == i);
            if hot {
                self.surf.fill_rr(r, 8.0, self.fg(0.08 * a));
            }
            self.surf.text("\u{E7F4}", &self.f.icon_s, rf(r.left + 4.0, r.top, 16.0, 26.0), self.fg(0.6 * a));
            let t = if u.is_empty() { h.clone() } else { format!("{h} · {u}") };
            self.surf.text(&t, &self.f.tiny, rf(r.left + 24.0, r.top, rw - 48.0, 26.0), self.fg(0.85 * a));
            self.hits.push((Hit::RdpRecent(i), r));
            if hot {
                let xr = rf(r.right - 22.0, r.top + 4.0, 18.0, 18.0);
                self.surf.text("\u{E711}", &self.f.icon_s, xr, self.fg(if self.hot == Some(Hit::RdpForget(i)) { 0.95 } else { 0.5 } * a));
                self.hits.push((Hit::RdpForget(i), xr));
            }
            y += 27.0;
        }
    }

    pub(super) fn rdp_connect(&mut self) {
        if let Some(t) = self.edit.filter(|t| matches!(t, EditTarget::RdpHost | EditTarget::RdpUser | EditTarget::RdpPass)) {
            let _ = t;
            self.end_edit(true);
            self.release_focus();
        }
        let r = crate::rdp::connect(&self.rdp);
        self.rdp_msg = Some(match r {
            Ok(()) => (format!("{} {}…", tr("Opening Remote Desktop to"), self.rdp.host.trim()), false, Instant::now()),
            Err(e) => (e, true, Instant::now()),
        });
        if self.rdp_msg.as_ref().map(|m| !m.1).unwrap_or(false) {
            // The password isn't kept here.
            self.rdp.pass.clear();
            self.rdp_saved_for = crate::rdp::has_saved(&self.rdp.host).then(|| self.rdp.host.clone());
        }
        self.render();
    }

    /// Pick a computer (recent / the linked PC): fill the fields.
    pub(super) fn rdp_pick(&mut self, host: String, user: Option<String>) {
        if self.edit.is_some() {
            self.end_edit(true);
        }
        self.rdp_saved_for = crate::rdp::has_saved(&host).then(|| host.clone());
        self.rdp.host = host;
        if let Some(u) = user {
            self.rdp.user = u;
        }
        self.rdp.pass.clear();
        self.render();
    }

    /// Tab: the next field.
    pub(super) fn rdp_next_field(&mut self) {
        let next = match self.edit {
            Some(EditTarget::RdpHost) => EditTarget::RdpUser,
            Some(EditTarget::RdpUser) => EditTarget::RdpPass,
            _ => EditTarget::RdpHost,
        };
        self.end_edit(true);
        self.begin_edit(next);
    }
}
