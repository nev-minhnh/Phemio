//! A pasted picture's text, read with its layout kept: Markdown shown as a
//! small document (headings, paragraphs, lists, tables, code) that scrolls.
//! Click a block to copy it; Enter or Copy copies everything.

use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::DirectWrite::IDWriteTextFormat;

use super::*;

/// One block of the document.
#[derive(Clone, Debug)]
pub(crate) enum Blk {
    Head(u8, String),
    Para(String),
    /// Nesting, marker ("•", "1."), text.
    Item(usize, String, String),
    Table(Vec<Vec<String>>),
    Code(String),
    Rule,
}

/// A block placed: where it starts, how tall, and (tables) column widths
/// and row heights.
#[derive(Clone, Debug)]
pub(crate) struct Placed {
    pub blk: Blk,
    pub y: f32,
    pub h: f32,
    pub cols: Vec<f32>,
    pub rows: Vec<f32>,
}

/// A run of text in the document (a paragraph, a heading, a list item's
/// text, a code block, a table cell), where it sits: what's drawn, what the
/// pointer selects.
#[derive(Clone)]
pub(crate) struct DSeg {
    pub text: String,
    pub styles: Vec<(u32, u32, bool, bool, bool, bool)>,
    pub font: IDWriteTextFormat,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// How strong its ink is.
    pub ink: f32,
    /// What comes between it and the next one when copied ("\n\n", "\t", "\n").
    pub sep: &'static str,
}

/// A place in the text: (run, UTF-16 position in it).
pub(crate) type DPos = (usize, u32);

/// The document laid out for (text, width).
pub(crate) struct DocLayout {
    pub key: (u64, i32),
    pub blocks: Vec<Placed>,
    pub segs: Vec<DSeg>,
    pub height: f32,
}

const GAP: f32 = 8.0;
const CELL_PAD: f32 = 8.0;

/// Markdown → blocks.
pub(crate) fn parse(md: &str) -> Vec<Blk> {
    let mut out = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let lines: Vec<&str> = md.lines().collect();
    let flush = |para: &mut Vec<String>, out: &mut Vec<Blk>| {
        if !para.is_empty() {
            out.push(Blk::Para(para.join("\n")));
            para.clear();
        }
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_end();
        let t = line.trim_start();
        if t.starts_with("```") {
            flush(&mut para, &mut out);
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                code.push(lines[i]);
                i += 1;
            }
            out.push(Blk::Code(code.join("\n")));
            i += 1;
            continue;
        }
        if t.is_empty() {
            flush(&mut para, &mut out);
            i += 1;
            continue;
        }
        if t.starts_with('|') {
            flush(&mut para, &mut out);
            let mut rows = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('|') {
                let cells = split_row(lines[i].trim());
                let sep = cells.iter().all(|c| {
                    let c = c.trim();
                    !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' '))
                });
                if !sep {
                    rows.push(cells);
                }
                i += 1;
            }
            if !rows.is_empty() {
                out.push(Blk::Table(rows));
            }
            continue;
        }
        if t == "---" || t == "***" || t == "___" {
            flush(&mut para, &mut out);
            out.push(Blk::Rule);
            i += 1;
            continue;
        }
        let hashes = t.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') {
            flush(&mut para, &mut out);
            out.push(Blk::Head(hashes as u8, t[hashes..].trim().to_string()));
            i += 1;
            continue;
        }
        let indent = (line.len() - t.len()) / 2;
        let bullet = ["- ", "* ", "+ "].iter().find_map(|p| t.strip_prefix(p));
        let numbered = t.split_once(['.', ')']).filter(|(n, rest)| !n.is_empty() && n.len() <= 3 && n.chars().all(|c| c.is_ascii_digit()) && rest.starts_with(' '));
        if let Some(rest) = bullet {
            flush(&mut para, &mut out);
            out.push(Blk::Item(indent, "•".into(), rest.trim().to_string()));
        } else if let Some((n, rest)) = numbered {
            flush(&mut para, &mut out);
            let sep = &t[n.len()..n.len() + 1];
            out.push(Blk::Item(indent, format!("{n}{sep}"), rest.trim().to_string()));
        } else {
            para.push(t.to_string());
        }
        i += 1;
    }
    flush(&mut para, &mut out);
    out
}

/// "| a | b \| c |" → ["a", "b | c"].
fn split_row(line: &str) -> Vec<String> {
    let inner = line.trim().trim_start_matches('|');
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut esc = false;
    for ch in inner.chars() {
        if esc {
            cur.push(ch);
            esc = false;
        } else if ch == '\\' {
            esc = true;
        } else if ch == '|' {
            cells.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(ch);
        }
    }
    cells.push(cur.trim().to_string());
    cells
}

/// Inline Markdown (**bold**, *italic*, `code`, \escapes) → plain text and
/// its styled stretches (UTF-16).
pub(crate) fn inline(s: &str) -> (String, Vec<(u32, u32, bool, bool, bool, bool)>) {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut u16pos = 0u32;
    let (mut bold, mut ital) = (false, false);
    let mut b_start = 0u32;
    let mut i_start = 0u32;
    let mut styles = Vec::new();
    let mut k = 0;
    while k < chars.len() {
        let c = chars[k];
        let next = chars.get(k + 1).copied();
        if c == '\\' && next.map(|n| "*_`|#\\[]".contains(n)).unwrap_or(false) {
            out.push(next.unwrap());
            u16pos += next.unwrap().len_utf16() as u32;
            k += 2;
            continue;
        }
        if (c == '*' && next == Some('*')) || (c == '_' && next == Some('_')) {
            if bold {
                styles.push((b_start, u16pos - b_start, true, false, false, false));
            } else {
                b_start = u16pos;
            }
            bold = !bold;
            k += 2;
            continue;
        }
        let word_edge = |j: usize| j == 0 || !chars[j - 1].is_alphanumeric();
        if c == '*' && (ital || next.map(|n| !n.is_whitespace()).unwrap_or(false)) || (c == '_' && (ital || word_edge(k))) {
            if ital {
                styles.push((i_start, u16pos - i_start, false, true, false, false));
            } else {
                i_start = u16pos;
            }
            ital = !ital;
            k += 1;
            continue;
        }
        if c == '`' {
            k += 1;
            continue;
        }
        out.push(c);
        u16pos += c.len_utf16() as u32;
        k += 1;
    }
    (out, styles)
}

impl Island {
    fn doc_font(&self, b: &Blk) -> IDWriteTextFormat {
        match b {
            Blk::Head(1, _) => self.f.doc_h1.clone(),
            Blk::Head(2, _) => self.f.doc_h2.clone(),
            Blk::Head(_, _) => self.f.doc_h3.clone(),
            Blk::Code(_) => self.f.doc_code.clone(),
            _ => self.f.ai_body.clone(),
        }
    }

    /// The document laid out at width `w` (kept until the text or width changes).
    pub(super) fn doc_layout(&mut self, md: &str, w: f32) -> std::rc::Rc<DocLayout> {
        use std::hash::{Hash, Hasher};
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        md.hash(&mut hs);
        let key = (hs.finish(), w as i32);
        if let Some(d) = &self.doc_cache {
            if d.key == key {
                return d.clone();
            }
        }
        let g = gfx::g();
        let mut y = 0.0;
        let mut blocks = Vec::new();
        let mut segs: Vec<DSeg> = Vec::new();
        for blk in parse(md) {
            let f = self.doc_font(&blk);
            let (h, cols, rows) = match &blk {
                Blk::Head(_, t) | Blk::Para(t) => {
                    let (plain, st) = inline(t);
                    let top = if matches!(blk, Blk::Head(..)) && y > 0.0 { 6.0 } else { 0.0 };
                    (top + self.surf.text_rich(&plain, &f, rf(0.0, 0.0, w, 4000.0), rgb(0, 0.0), &st), vec![], vec![])
                }
                Blk::Item(lv, _, t) => {
                    let (plain, st) = inline(t);
                    let ind = 18.0 + *lv as f32 * 16.0;
                    (self.surf.text_rich(&plain, &f, rf(0.0, 0.0, (w - ind).max(40.0), 4000.0), rgb(0, 0.0), &st) - 4.0, vec![], vec![])
                }
                Blk::Code(t) => (g.measure_h(t, &f, w - 20.0) + 16.0, vec![], vec![]),
                Blk::Rule => (10.0, vec![], vec![]),
                Blk::Table(rows) => {
                    let n = rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
                    let cf = self.f.doc_cell.clone();
                    let mut nat = vec![30.0f32; n];
                    for r in rows {
                        for (k, c) in r.iter().enumerate() {
                            let (plain, _) = inline(c);
                            let longest = plain.lines().map(|l| g.measure(l, &cf)).fold(0.0, f32::max);
                            nat[k] = nat[k].max(longest + CELL_PAD * 2.0 + 2.0);
                        }
                    }
                    let total: f32 = nat.iter().sum();
                    let cols: Vec<f32> = if total <= w {
                        nat
                    } else {
                        // Too wide: each column gets a share, none too narrow.
                        let min = (w / n as f32).min(70.0);
                        let extra = (w - min * n as f32).max(0.0);
                        let over: f32 = nat.iter().map(|c| (c - min).max(0.0)).sum::<f32>().max(1.0);
                        nat.iter().map(|c| min + extra * (c - min).max(0.0) / over).collect()
                    };
                    let mut rh = Vec::new();
                    for r in rows {
                        let mut h = 0.0f32;
                        for (k, c) in r.iter().enumerate() {
                            let (plain, st) = inline(c);
                            let cw = (cols[k] - CELL_PAD * 2.0).max(10.0);
                            h = h.max(self.surf.text_rich(&plain, &cf, rf(0.0, 0.0, cw, 4000.0), rgb(0, 0.0), &st));
                        }
                        rh.push(h.max(14.0) + 10.0);
                    }
                    (rh.iter().sum::<f32>() + 1.0, cols, rh)
                }
            };
            // Its text runs, placed exactly where they're drawn.
            match &blk {
                Blk::Head(_, t) | Blk::Para(t) => {
                    let (plain, st) = inline(t);
                    let top = if matches!(blk, Blk::Head(..)) && y > 0.0 { 6.0 } else { 0.0 };
                    let ink = if matches!(blk, Blk::Head(..)) { 0.95 } else { 0.9 };
                    segs.push(DSeg { text: plain, styles: st, font: f.clone(), x: 0.0, y: y + top, w, h: h - top, ink, sep: "\n\n" });
                }
                Blk::Item(lv, m, t) => {
                    let (plain, st) = inline(t);
                    let ind = 18.0 + *lv as f32 * 16.0;
                    let _ = m;
                    segs.push(DSeg { text: plain, styles: st, font: f.clone(), x: ind, y: y - 2.0, w: (w - ind).max(40.0), h: h + 4.0, ink: 0.9, sep: "\n" });
                }
                Blk::Code(t) => segs.push(DSeg { text: t.clone(), styles: vec![], font: f.clone(), x: 10.0, y: y + 8.0, w: w - 20.0, h: h - 8.0, ink: 0.88, sep: "\n\n" }),
                Blk::Rule => {}
                Blk::Table(trows) => {
                    let mut ry = y;
                    for (ri, r) in trows.iter().enumerate() {
                        let rh = rows.get(ri).copied().unwrap_or(20.0);
                        let mut cx = 0.0;
                        for (k, cw) in cols.iter().enumerate() {
                            let (plain, st) = r.get(k).map(|c| inline(c)).unwrap_or_default();
                            let font = if ri == 0 { self.f.doc_cell_b.clone() } else { self.f.doc_cell.clone() };
                            let last = k + 1 == cols.len();
                            segs.push(DSeg { text: plain, styles: st, font, x: cx + CELL_PAD, y: ry + 5.0, w: (cw - CELL_PAD * 2.0).max(10.0), h: rh - 5.0, ink: 0.9, sep: if !last { "\t" } else if ri + 1 == trows.len() { "\n\n" } else { "\n" } });
                            cx += cw;
                        }
                        ry += rh;
                    }
                }
            }
            blocks.push(Placed { blk, y, h, cols, rows });
            y += h + GAP;
        }
        let d = std::rc::Rc::new(DocLayout { key, blocks, segs, height: y });
        self.doc_cache = Some(d.clone());
        d
    }

    /// The document view (the search popup's results area).
    pub(super) fn draw_doc(&mut self, r: D2D_RECT_F, a: f32) {
        let Some(doc) = crate::search::ocr_doc() else { return };
        let g = gfx::g();
        let accent = self.ic(pal::BLUE, 1.0);
        let pad = 12.0;
        let x = r.left + pad;
        let w = r.right - r.left - pad * 2.0;
        // ---- Header: what read it, Copy ----
        let head = 30.0;
        self.surf.text("\u{E8D2}", &self.f.icon_s, rf(x, r.top, 16.0, head), with_alpha(accent, a));
        let title = format!("{}  ·  {}", tr("Text in the picture"), doc.engine);
        let cl = tr("Copy all");
        let cw = g.measure(cl, &self.f.tiny) + 34.0;
        self.surf.text(&title, &self.f.tiny, rf(x + 20.0, r.top, w - cw - 30.0, head), self.fg(0.6 * a));
        if !doc.text.is_empty() {
            let cr = rf(r.right - pad - cw, r.top + (head - 22.0) / 2.0, cw, 22.0);
            let hot = self.hot == Some(Hit::DocCopy);
            let done = self.doc_copied.map(|(k, t)| k >= usize::MAX - 1 && t.elapsed().as_secs_f32() < 1.5).unwrap_or(false);
            self.surf.fill_rr(cr, 11.0, with_alpha(accent, if hot { 0.3 } else { 0.16 } * a));
            self.surf.text(if done { "\u{E73E}" } else { "\u{E8C8}" }, &self.f.icon_s, rf(cr.left + 8.0, cr.top, 14.0, 22.0), with_alpha(accent, a));
            self.surf.text(if done { tr("Copied") } else { cl }, &self.f.tiny, rf(cr.left + 24.0, cr.top, cw - 26.0, 22.0), self.fg(0.85 * a));
            self.hits.push((Hit::DocCopy, cr));
        }
        let mut y = r.top + head;
        if !doc.note.is_empty() {
            self.surf.text(&doc.note, &self.f.tiny, rf(x, y - 2.0, w, 18.0), self.ic(pal::ORANGE, 0.95 * a));
            y += 20.0;
        }
        let avail = (r.bottom - y - 4.0).max(10.0);
        if doc.text.is_empty() {
            if !doc.error.is_empty() {
                self.surf.text_wrap(&doc.error, &self.f.wrap_src, rf(x, y + 6.0, w, 60.0), rgb(pal::RED, 0.9 * a));
            } else {
                let secs = doc.started.elapsed().as_secs_f32();
                self.thinking_dots(x + 8.0, y + 14.0, 8.0, a);
                self.thinking_text(&format!("{}  {:.0}s", tr("Reading the text in the picture…"), secs), rf(x + 22.0, y + 3.0, w - 22.0, 22.0), a);
                self.keep_anim = true;
            }
            return;
        }
        let tw = w - 8.0;
        let lay = self.doc_layout(&doc.text, tw);
        let max = (lay.height - avail + 6.0).max(0.0);
        let off = self.search.as_mut().map(|s| {
            // (It stays at the top as it comes in: read from the start.)
            s.ai_max = max;
            s.ai_scroll = s.ai_scroll.min(max);
            s.ai_scroll
        }).unwrap_or(0.0);
        let copied = self.doc_copied.filter(|(_, t)| t.elapsed().as_secs_f32() < 1.5).map(|(k, _)| k);
        if copied.is_some() {
            self.keep_anim = true;
        }
        self.surf.clip(rf(r.left, y, r.right - r.left, avail));
        // Where the document sits now (for the pointer: selecting text).
        self.doc_view = Some((doc.id, x, y - off, rf(r.left, y, r.right - r.left, avail)));
        if self.doc_sel.map(|s| s.0 != doc.id).unwrap_or(false) {
            self.doc_sel = None;
        }
        let (px, py) = self.last_pt;
        let mut pills: Vec<(usize, D2D_RECT_F)> = Vec::new();
        let pointer_in = px >= r.left && px <= r.right && py >= y && py <= y + avail && !self.doc_drag;
        for (k, p) in lay.blocks.iter().enumerate() {
            let by = y + p.y - off;
            if by > y + avail || by + p.h < y {
                continue;
            }
            let br = rf(x - 6.0, by - 3.0, tw + 12.0, p.h + 6.0);
            let over = pointer_in && px >= br.left && px <= br.right && py >= br.top && py <= br.bottom;
            if copied == Some(k) {
                self.surf.fill_rr(br, 8.0, with_alpha(accent, 0.18 * a));
            } else if over {
                self.surf.fill_rr(br, 8.0, self.fg(0.05 * a));
            }
            self.draw_block(p, x, by, tw, a, accent);
            if (over || copied == Some(k)) && !matches!(p.blk, Blk::Rule) {
                pills.push((k, br));
            }
        }
        // The text, with what's selected highlighted under it.
        let sel = self.doc_sel.map(|(_, a, b)| if a <= b { (a, b) } else { (b, a) }).filter(|(a, b)| a != b);
        for (i, sg) in lay.segs.iter().enumerate() {
            let sy = y + sg.y - off;
            if sy > y + avail || sy + sg.h < y || sg.text.is_empty() {
                continue;
            }
            let sx = x + sg.x;
            if let Some((lo, hi)) = sel {
                if i >= lo.0 && i <= hi.0 {
                    let n = sg.text.encode_utf16().count() as u32;
                    let from = if i == lo.0 { lo.1 } else { 0 };
                    let to = if i == hi.0 { hi.1 } else { n };
                    if to > from {
                        if let Some(l) = g.rich_layout(&sg.text, &sg.font, sg.w, &sg.styles) {
                            for (lx, ly, lw, lh) in gfx::Gfx::range_rects(&l, from, to - from) {
                                self.surf.fill_rect(rf(sx + lx, sy + ly, lw.max(3.0), lh), with_alpha(accent, 0.35 * a));
                            }
                        }
                    }
                }
            }
            self.surf.text_rich(&sg.text, &sg.font, rf(sx, sy, sg.w, sg.h + 12.0), self.fg(sg.ink * a), &sg.styles);
        }
        // On the block under the pointer: a small Copy for all of it.
        for (k, br) in pills {
            {
                let lab = if copied == Some(k) { tr("Copied") } else { tr("Copy") };
                let lw = g.measure(lab, &self.f.tiny) + 30.0;
                let lr = rf(br.right - lw - 4.0, br.top + 2.0, lw, 20.0);
                let hot = self.hot == Some(Hit::DocBlock(k));
                self.surf.fill_rr(lr, 10.0, with_alpha(accent, if hot { 1.0 } else { 0.8 } * a));
                self.surf.text(if copied == Some(k) { "\u{E73E}" } else { "\u{E8C8}" }, &self.f.icon_s, rf(lr.left + 8.0, lr.top, 14.0, 20.0), Island::text_on(accent, a));
                self.surf.text(lab, &self.f.tiny, rf(lr.left + 24.0, lr.top, lw - 26.0, 20.0), Island::text_on(accent, a));
                if lr.top >= y - 2.0 {
                    self.hits.push((Hit::DocBlock(k), lr));
                }
            }
        }
        if !doc.done {
            let blink = (self.born.elapsed().as_secs_f32() * 3.0).sin() > -0.3;
            let ly = y + lay.height - off;
            if blink && ly < y + avail {
                self.surf.fill_rr(rf(x, ly - 4.0, 7.0, 13.0), 2.0, with_alpha(accent, 0.8 * a));
            }
            self.keep_anim = true;
        }
        self.surf.unclip();
        if max > 0.0 {
            let bar_h = (avail * avail / (lay.height + 6.0)).max(18.0);
            let sy = y + (avail - bar_h) * (off / max);
            self.surf.fill_rr(rf(r.right - 5.0, sy, 3.0, bar_h), 1.5, self.fg(0.25 * a));
        }
    }

    /// A block's decorations (the text itself is drawn from its runs).
    fn draw_block(&mut self, p: &Placed, x: f32, y: f32, w: f32, a: f32, accent: D2D1_COLOR_F) {
        let f = self.doc_font(&p.blk);
        match &p.blk {
            Blk::Item(lv, marker, _) => {
                let ind = 18.0 + *lv as f32 * 16.0;
                let mx = x + ind - 16.0;
                self.surf.text_wrap(marker, &f, rf(mx - 4.0, y - 2.0, 24.0, 30.0), with_alpha(accent, a));
            }
            Blk::Code(_) => {
                self.surf.fill_rr(rf(x, y, w, p.h), 6.0, self.fg(0.07 * a));
            }
            Blk::Rule => {
                self.surf.fill_rect(rf(x, y + 5.0, w, 1.0), self.fg(0.18 * a));
            }
            Blk::Table(_) => {
                let tw: f32 = p.cols.iter().sum();
                if let Some(h0) = p.rows.first() {
                    self.surf.fill_rect(rf(x, y, tw, *h0), with_alpha(accent, 0.12 * a));
                }
                let line = self.fg(0.14 * a);
                let mut ry = y;
                for rh in &p.rows {
                    ry += rh;
                    self.surf.fill_rect(rf(x, ry - 0.5, tw, 1.0), line);
                }
                self.surf.fill_rect(rf(x, y, tw, 1.0), line);
                let mut cx = x;
                self.surf.fill_rect(rf(cx, y, 1.0, ry - y), line);
                for cw in &p.cols {
                    cx += cw;
                    self.surf.fill_rect(rf(cx - 1.0, y, 1.0, ry - y), line);
                }
            }
            _ => {}
        }
    }

    /// The place in the text under (px, py) (island DIPs).
    pub(super) fn doc_pos_at(&mut self, px: f32, py: f32) -> Option<DPos> {
        let (_, ox, oy, _) = self.doc_view?;
        let lay = self.doc_cache.clone()?;
        let (dx, dy) = (px - ox, py - oy);
        // The run under the pointer, or the nearest one (above / beside).
        let mut best: Option<(usize, f32)> = None;
        for (i, sg) in lay.segs.iter().enumerate() {
            let vy = if dy < sg.y { sg.y - dy } else if dy > sg.y + sg.h { dy - sg.y - sg.h } else { 0.0 };
            let vx = if dx < sg.x { sg.x - dx } else if dx > sg.x + sg.w { dx - sg.x - sg.w } else { 0.0 };
            let d = vy * 1000.0 + vx;
            if best.map(|b| d < b.1).unwrap_or(true) {
                best = Some((i, d));
            }
        }
        let (i, _) = best?;
        let sg = &lay.segs[i];
        let l = gfx::g().rich_layout(&sg.text, &sg.font, sg.w, &sg.styles)?;
        Some((i, gfx::Gfx::hit_pos(&l, dx - sg.x, (dy - sg.y).clamp(0.0, sg.h.max(1.0)))))
    }

    /// The selected text, as it reads (cells by tabs, paragraphs by lines).
    pub(super) fn doc_selected(&self) -> Option<String> {
        let (_, a, b) = self.doc_sel?;
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        if lo == hi {
            return None;
        }
        let lay = self.doc_cache.clone()?;
        let mut out = String::new();
        for i in lo.0..=hi.0.min(lay.segs.len().saturating_sub(1)) {
            let sg = &lay.segs[i];
            let u: Vec<u16> = sg.text.encode_utf16().collect();
            let from = if i == lo.0 { lo.1 as usize } else { 0 }.min(u.len());
            let to = if i == hi.0 { hi.1 as usize } else { u.len() }.min(u.len());
            out.push_str(&String::from_utf16_lossy(&u[from..to.max(from)]));
            if i < hi.0 {
                out.push_str(sg.sep);
            }
        }
        Some(out).filter(|s| !s.trim().is_empty())
    }

    /// All of the document as text (for Alt+Q).
    pub(super) fn doc_plain(&self) -> Option<String> {
        let lay = self.doc_cache.clone()?;
        let mut out = String::new();
        for sg in &lay.segs {
            out.push_str(&sg.text);
            out.push_str(sg.sep);
        }
        Some(out.trim().to_string()).filter(|s| !s.is_empty())
    }

    /// Showing a picture's text right now?
    pub(super) fn doc_showing(&self) -> bool {
        self.search_popup() && self.search.as_ref().and_then(|s| s.results.as_ref()).and_then(|r| r.items.first()).map(|i| i.kind == crate::search::Kind::OcrDoc).unwrap_or(false)
    }

    /// Selecting with the pointer: pressed (start), moved, let go (copy).
    pub(super) fn doc_press(&mut self, px: f32, py: f32) -> bool {
        if std::env::var("ISLET_LOG_DOC").is_ok() {
            crate::util::log(&format!("doc press {px},{py} view={:?} showing={}", self.doc_view.map(|v| (v.1, v.2, v.3.top, v.3.bottom)), self.doc_showing()));
        }
        let Some((id, _, _, clip)) = self.doc_view else { return false };
        if !self.doc_showing() || px < clip.left || px > clip.right || py < clip.top || py > clip.bottom {
            return false;
        }
        let Some(p) = self.doc_pos_at(px, py) else { return false };
        self.doc_sel = Some((id, p, p));
        self.doc_drag = true;
        self.render();
        true
    }

    pub(super) fn doc_move(&mut self, px: f32, py: f32) {
        // Past the top or bottom edge: scroll along.
        if let Some((_, _, _, clip)) = self.doc_view {
            let d = if py < clip.top { -(clip.top - py).min(40.0) } else if py > clip.bottom { (py - clip.bottom).min(40.0) } else { 0.0 };
            if d != 0.0 {
                if let Some(s) = self.search.as_mut() {
                    s.ai_scroll = (s.ai_scroll + d * 0.5).clamp(0.0, s.ai_max);
                }
            }
        }
        if std::env::var("ISLET_LOG_DOC").is_ok() {
            crate::util::log(&format!("doc move {px},{py} -> {:?} sel={:?}", self.doc_pos_at(px, py), self.doc_sel.map(|s| (s.1, s.2))));
        }
        if let Some(p) = self.doc_pos_at(px, py) {
            if let Some(s) = self.doc_sel.as_mut() {
                s.2 = p;
            }
        }
        self.render();
    }

    pub(super) fn doc_release(&mut self) {
        self.doc_drag = false;
        match self.doc_selected() {
            // Selected: on the clipboard at once.
            Some(t) => {
                crate::util::set_clipboard_text(&t);
                self.doc_copied = Some((usize::MAX - 1, Instant::now()));
            }
            None => self.doc_sel = None,
        }
        self.render();
    }

    /// A click on a block: copy it (a table as tab-separated rows, which
    /// paste as a table into Word or Excel; the rest as plain text).
    pub(super) fn doc_copy(&mut self, k: Option<usize>) {
        let Some(doc) = crate::search::ocr_doc() else { return };
        let text = match k {
            None => doc.text.clone(),
            Some(k) => {
                let Some(lay) = self.doc_cache.clone() else { return };
                let Some(p) = lay.blocks.get(k) else { return };
                match &p.blk {
                    Blk::Head(_, t) | Blk::Para(t) => inline(t).0,
                    Blk::Item(_, m, t) => format!("{m} {}", inline(t).0),
                    Blk::Code(t) => t.clone(),
                    Blk::Rule => return,
                    Blk::Table(rows) => rows.iter().map(|r| r.iter().map(|c| inline(c).0.replace('\n', " ")).collect::<Vec<_>>().join("\t")).collect::<Vec<_>>().join("\r\n"),
                }
            }
        };
        crate::util::set_clipboard_text(&text);
        self.doc_copied = Some((k.unwrap_or(usize::MAX), Instant::now()));
        self.render();
    }
}
