//! Alt+S: a search bar grows out of the island; results fall in below it.

use windows::Win32::Graphics::Direct2D::Common::*;

use super::*;
use crate::search::{self, Engine, Kind};

/// Bar height, row height, rows shown, footer.
pub(super) const BAR_H: f32 = 56.0;
pub(super) const ROW_H: f32 = 46.0;
pub(super) const MAX_ROWS: usize = 8;
pub(super) const FOOT_H: f32 = 34.0;
/// A QR(text) result: the code drawn big.
pub(super) const QR_ROW_H: f32 = 186.0;
/// The guide shown in an empty search box.
pub(super) const GUIDE_H: f32 = 34.0;

/// (badge colour, badge, title, example, what a click types)
pub(super) fn guide() -> Vec<(u32, &'static str, &'static str, &'static str, &'static str)> {
    vec![
        (pal::BLUE, "\u{E71D}", "Apps and files", "Type any name — files and folders come from Everything", ""),
        (pal::ORANGE, "=", "Calculator", "12*(3+4)  ·  2^10  ·  15%", "12*(3+4)"),
        (pal::GREEN, "$", "Currency", "100 usd to vnd  ·  $25 in eur  ·  50 eur", "100 usd to vnd"),
        (pal::PURPLE, "\u{E8C1}", "Translate", "tr good morning  ·  tr ja: hello  ·  Alt+Q on selected text", "tr "),
        (pal::PURPLE, "\u{E82D}", "Dictionary", "Alt+W anywhere, then type a word: its translation (Google Translate)", "dict "),
        (pal::TEAL, "64", "Base64", "B64(text) encodes  ·  b64d SGVsbG8= or paste base64 to decode", "B64("),
        (pal::PINK, "QR", "QR code", "QR(text) makes one  ·  paste a picture (Ctrl+V) to read one", "QR("),
        (pal::PINK, "Aa", "Text in a picture", "Type < then paste a picture (Ctrl+V): its text, tables and layout kept", "<"),
        (pal::BLUE, "AI", "Ask Gemini", "? your question — written as it thinks (3 models to pick)", "? "),
        (pal::INDIGO, "\u{E774}", "Google", "> your question — Google's top 10 results, right here", "> "),
    ]
}
/// Tallest the search can get (window height while searching).
/// A web result, laid out like a search page: site, link, title, snippet.
pub(super) const WEB_ROW_H: f32 = 100.0;
pub(super) const WEB_ROWS: usize = 4;
/// Results per page (PgUp / PgDn, or the footer's arrows, to turn).
pub(super) const PAGE: usize = 50;

impl SearchUi {
    pub(super) fn pages(&self) -> usize {
        self.all.as_ref().map(|r| r.items.len().div_ceil(PAGE).max(1)).unwrap_or(1)
    }

    /// Show page `p` of `all` (the chosen row: its first, or its last).
    pub(super) fn show_page(&mut self, p: usize, last: bool) {
        let Some(all) = &self.all else { return };
        let p = p.min(self.pages() - 1);
        let from = (p * PAGE).min(all.items.len());
        let to = ((p + 1) * PAGE).min(all.items.len());
        self.page = p;
        self.results = Some(search::Results { query: all.query.clone(), items: all.items[from..to].to_vec(), engine: all.engine.clone(), total: all.total, pending: all.pending });
        let n = to - from;
        self.sel = if last { n.saturating_sub(1) } else { 0 };
        let vis = self.vis_rows.max(1);
        self.first = if last { n.saturating_sub(vis) } else { 0 };
    }
}
pub(super) const SEARCH_MAX_H: f32 = BAR_H + 6.0 + WEB_ROW_H * WEB_ROWS as f32 + FOOT_H + 6.0;

pub(crate) struct SearchUi {
    /// On the Search page (inside the expanded island) rather than the Alt+S popup.
    pub in_page: bool,
    /// Rows that fit on screen right now.
    pub vis_rows: usize,
    pub wheel: i32,
    /// The text the shown results are for (Enter mode: what was last searched).
    pub searched: String,
    pub sel: usize,
    pub first: usize,
    /// The page of results on show (at most PAGE rows of `all`).
    pub results: Option<search::Results>,
    /// Every result of the search.
    pub all: Option<search::Results>,
    pub page: usize,
    /// Alt+W: the box is a dictionary (what's typed is looked up in Google
    /// Translate; no "dict" to type).
    pub dict: bool,
    /// When the current list arrived (rows slide in from here).
    pub at: Instant,
    /// Gemini's answer: how far it's scrolled, and how far it can go.
    pub ai_scroll: f32,
    pub ai_max: f32,
    /// The live answer being shown, and how much of it (and its thinking) is out.
    pub ai_id: u64,
    pub ai_shown: f32,
    pub ai_tshown: f32,
    pub ai_tick: Instant,
    /// Keep the newest words in view (until you scroll up).
    pub ai_follow: bool,
    /// The thinking is unfolded.
    pub ai_thoughts: bool,
}

impl Island {
    /// "Filename | Content": what the search box looks at — file names
    /// (Everything) or what's inside files (Windows' search index). Drawn
    /// with its right edge at `right`; returns its width.
    pub(super) fn search_mode_switch(&mut self, right: f32, cy: f32, a: f32) -> f32 {
        let g = gfx::g();
        let content = config::cur().b("srch_content");
        let labels = [(tr("Filename"), false), (tr("Content"), true)];
        let ws: Vec<f32> = labels.iter().map(|(l, _)| g.measure(l, &self.f.tiny) + 20.0).collect();
        let total = ws.iter().sum::<f32>() + 4.0;
        let outer = rf(right - total, cy - 13.0, total, 26.0);
        self.surf.fill_rr(outer, 13.0, self.fg(0.07 * a));
        let mut x = outer.left + 2.0;
        let blue = self.el(pal::BLUE, 1.0);
        for (k, (label, on)) in labels.iter().enumerate() {
            let r = rf(x, cy - 11.0, ws[k], 22.0);
            let sel = content == *on;
            let hot = self.hot == Some(Hit::SrchMode(*on));
            if sel {
                self.surf.fill_rr(r, 11.0, with_alpha(blue, 0.85 * a));
            } else if hot {
                self.surf.fill_rr(r, 11.0, self.fg(0.1 * a));
            }
            self.surf.text(label, &self.f.tiny_c, r, if sel { Island::text_on(blue, a) } else { self.fg(0.7 * a) });
            self.hits.push((Hit::SrchMode(*on), r));
            x += ws[k];
        }
        total
    }

    /// Looking inside files: a moving bar, what's happening, and Cancel (in `r`;
    /// a narrow `r` gets just the Cancel button).
    pub(super) fn content_status(&mut self, r: D2D_RECT_F, a: f32) {
        let g = gfx::g();
        let label = tr("Cancel");
        let bw = g.measure(label, &self.f.tiny) + 26.0;
        let h = (r.bottom - r.top).min(26.0);
        let br = rf(r.right - bw, (r.top + r.bottom) / 2.0 - h / 2.0, bw, h);
        let hot = self.hot == Some(Hit::SrchCancel);
        self.surf.fill_rr(br, h / 2.0, self.el(pal::RED, if hot { 0.34 } else { 0.18 } * a));
        self.surf.text(label, &self.f.tiny_c, br, self.fg(0.9 * a));
        self.hits.push((Hit::SrchCancel, br));
        let room = br.left - r.left - 12.0;
        if room > 160.0 {
            let t = tr("Looking inside files…");
            let tw = g.measure(t, &self.f.tiny) + 8.0;
            self.surf.text(t, &self.f.tiny, rf(r.left, r.top, tw, r.bottom - r.top), self.fg(0.6 * a));
            let lx = r.left + tw + 8.0;
            let lw = (br.left - 14.0 - lx).max(20.0);
            self.busy_line(rf(lx, (r.top + r.bottom) / 2.0 - 1.5, lw, 3.0), a);
        }
        self.keep_anim = true;
    }

    /// An "it's working" line: a soft segment gliding back and forth along `r`.
    pub(super) fn busy_line(&mut self, r: D2D_RECT_F, a: f32) {
        let w = r.right - r.left;
        let h = r.bottom - r.top;
        self.surf.fill_rr(r, h / 2.0, self.fg(0.1 * a));
        let t = self.t0.elapsed().as_secs_f32();
        let p = ((t * 1.6).sin() * 0.5 + 0.5).powf(1.2);
        let seg = (w * 0.28).max(18.0);
        let x = r.left + (w - seg) * p;
        self.surf.fill_rr(rf(x, r.top, seg, h), h / 2.0, self.el(pal::BLUE, 0.9 * a));
        self.keep_anim = true;
    }

    /// The Alt+S popup is showing (not the Search page).
    pub fn search_popup(&self) -> bool {
        self.search.as_ref().map(|s| !s.in_page).unwrap_or(false)
    }

    /// Click on the Search page's field.
    pub(super) fn begin_page_search(&mut self) {
        search::start(crate::app().host, crate::WM_SEARCH);
        if self.search.is_none() {
            self.search = Some(SearchUi { in_page: true, vis_rows: 4, wheel: 0, searched: String::new(), sel: 0, first: 0, results: None, all: None, page: 0, dict: false, at: Instant::now(), ai_scroll: 0.0, ai_max: 0.0, ai_id: 0, ai_shown: 0.0, ai_tshown: 0.0, ai_tick: Instant::now(), ai_follow: true, ai_thoughts: false });
        }
        self.edit = Some(EditTarget::Search);
        self.caret = true;
        self.take_focus();
        self.update_timers();
        let stale = self.search.as_ref().map(|s| s.searched.trim() != self.edit_buf.trim()).unwrap_or(true);
        if !self.edit_buf.trim().is_empty() && stale && Self::search_live() {
            self.search_now();
        }
        self.render();
    }

    /// Wheel / two-finger swipe over the results.
    pub(super) fn search_scroll(&mut self, d: i32) {
        let Some(s) = self.search.as_mut() else { return };
        // Gemini's answer scrolls smoothly, line by line.
        if s.results.as_ref().and_then(|r| r.items.first()).map(|i| matches!(i.kind, Kind::Gemini | Kind::OcrDoc)).unwrap_or(false) {
            let to = (s.ai_scroll - d as f32 * 0.45).clamp(0.0, s.ai_max);
            s.ai_follow = to >= s.ai_max - 1.0;
            if to != s.ai_scroll {
                s.ai_scroll = to;
                self.render();
            }
            return;
        }
        let n = s.results.as_ref().map(|r| r.items.len()).unwrap_or(0);
        s.wheel += d;
        let mut moved = false;
        while s.wheel.abs() >= 40 {
            let step = -s.wheel.signum();
            s.wheel += step * 40;
            let max_first = n.saturating_sub(s.vis_rows);
            let to = (s.first as i32 + step).clamp(0, max_first as i32) as usize;
            if to != s.first {
                s.first = to;
                moved = true;
            }
        }
        if moved {
            // Keep the chosen row on screen.
            s.sel = s.sel.clamp(s.first, (s.first + s.vis_rows).saturating_sub(1).max(s.first));
            self.render();
        }
    }

    /// Alt+S (again closes it).
    pub fn toggle_search(&mut self) {
        if self.search_popup() {
            self.close_search();
        } else {
            self.open_search();
        }
    }

    /// Alt+W: the search box as a dictionary ("dict " typed for you). Pressed
    /// again while it's showing: closed.
    pub fn open_dictionary(&mut self) {
        if self.search_popup() && self.dict_mode() {
            self.close_search();
            return;
        }
        if !self.search_popup() {
            self.open_search();
        }
        if !self.search_popup() {
            return;
        }
        self.set_dict(true);
        self.search_now();
        self.retarget();
        self.render();
    }

    pub(super) fn dict_mode(&self) -> bool {
        self.search.as_ref().map(|s| s.dict).unwrap_or(false)
    }

    /// Into (or out of) dictionary mode, with an empty box.
    pub(super) fn set_dict(&mut self, on: bool) {
        self.srch_thumb = None;
        self.edit_buf.clear();
        self.edit_cur = None;
        if let Some(s) = self.search.as_mut() {
            s.dict = on;
            s.results = None;
            s.all = None;
            s.page = 0;
            s.searched.clear();
            s.at = Instant::now();
        }
        if on {
            self.search_now();
        }
        self.retarget();
    }

    /// What's searched for: the box's text (in dictionary mode, the word
    /// to look up, as "dict word").
    pub(super) fn search_text(&self) -> String {
        if self.dict_mode() {
            format!("dict {}", self.edit_buf.trim())
        } else {
            self.edit_buf.clone()
        }
    }

    pub fn open_search(&mut self) {
        if self.suppressed {
            return;
        }
        // From the Search page: the popup takes over.
        if self.search.is_some() {
            self.search = None;
            self.edit = None;
        }
        if self.expanded {
            self.expanded = false;
        }
        self.peek = None;
        if self.mascot != Mascot::Off {
            self.set_mascot(Mascot::Off);
        }
        search::start(crate::app().host, crate::WM_SEARCH);
        search::clear();
        self.search = Some(SearchUi { in_page: false, vis_rows: MAX_ROWS, wheel: 0, searched: String::new(), sel: 0, first: 0, results: None, all: None, page: 0, dict: false, at: Instant::now(), ai_scroll: 0.0, ai_max: 0.0, ai_id: 0, ai_shown: 0.0, ai_tshown: 0.0, ai_tick: Instant::now(), ai_follow: true, ai_thoughts: false });
        self.tall = true;
        self.relayout();
        self.edit_buf.clear();
        self.edit = Some(EditTarget::Search);
        self.caret = true;
        self.take_focus();
        self.srch_a.set(1.0);
        self.retarget();
        self.update_timers();
        self.render();
    }

    pub fn close_search(&mut self) {
        // A look inside files stops with it.
        crate::search::cancel_content();
        let Some(was) = self.search.take() else { return };
        self.srch_thumb = None;
        if was.in_page {
            if self.edit == Some(EditTarget::Search) {
                self.edit = None;
            }
            self.release_focus();
            self.update_timers();
            self.render();
            return;
        }
        if self.edit == Some(EditTarget::Search) {
            self.edit = None;
            self.edit_buf.clear();
        }
        self.release_focus();
        self.srch_a.set(0.0);
        self.retarget();
        self.update_timers();
        self.render();
        // The window shrinks back once the island has (see the animation tick).
        if !self.queue.is_empty() {
            self.next_peek();
        }
    }

    /// Developer aid (screenshots): paste the clipboard picture.
    pub fn demo_paste(&mut self) {
        if let Some(m) = crate::qr::make("https://example.com") {
            crate::qr::copy_image(&m);
        }
        if std::env::var("ISLET_DEMO_PASTE").map(|v| v == "ocr").unwrap_or(false) {
            self.edit_buf = "<".into();
        }
        self.search_paste();
    }

    /// Developer aid (screenshots): type `q` straight away.
    pub fn demo_query(&mut self, q: &str) {
        self.edit_buf = q.to_string();
        self.search_now();
        self.render();
    }

    /// New results from the worker.
    pub fn on_search(&mut self) {
        let Some(r) = search::take() else { return };
        if let Some(s) = self.search.as_mut() {
            if r.query != s.searched.trim() {
                return;
            }
            // More of the same search (inside files, rows come in as they're
            // read): the page and the chosen row stay where they are.
            // (Only real results count: the "Search Google / Ask Gemini" rows
            // shown while it looks are no reason to keep a place.)
            let kept = s.all.as_ref().filter(|o| o.query == r.query && o.items.iter().any(|i| i.kind != Kind::WebAction)).map(|o| {
                let mut it = r.items.iter();
                o.items.iter().filter(|a| a.kind != Kind::WebAction).all(|a| it.any(|b| b.target == a.target && b.kind == a.kind))
            });
            let chosen = s.results.as_ref().and_then(|x| x.items.get(s.sel)).map(|i| i.target.clone());
            let (first_off, was_page) = (s.sel.saturating_sub(s.first), s.page);
            // Still at the very top: stay at the top as more arrive.
            let at_top = s.page == 0 && s.sel == 0 && s.first == 0;
            s.all = Some(r);
            match (kept, chosen) {
                _ if at_top && kept == Some(true) => {
                    s.show_page(0, false);
                }
                (Some(true), Some(t)) => {
                    let idx = s.all.as_ref().and_then(|a| a.items.iter().position(|i| i.target == t)).unwrap_or(was_page * PAGE);
                    s.show_page(idx / PAGE, false);
                    s.sel = idx % PAGE;
                    s.first = s.sel.saturating_sub(first_off);
                }
                _ => {
                    s.at = Instant::now();
                    s.ai_scroll = 0.0;
                    s.show_page(0, false);
                }
            }
        }
        // Developer aid: ISLET_DEMO_NAV="{PgDn}{Up}{Down}" once results are in.
        if let Ok(nav) = std::env::var("ISLET_DEMO_NAV") {
            static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if self.search.as_ref().and_then(|s| s.all.as_ref()).map(|r| r.items.len() > 1).unwrap_or(false) && !DONE.swap(true, std::sync::atomic::Ordering::SeqCst) {
                for k in nav.split('}').map(|k| k.trim_start_matches('{')) {
                    match k {
                        "PgDn" => self.search_page(1),
                        "PgUp" => self.search_page(-1),
                        "Down" => self.search_move(1),
                        "Up" => self.search_move(-1),
                        "Back" => self.on_char(8),
                        _ => {}
                    }
                }
            }
        }
        self.retarget();
        self.kick();
        self.render();
    }

    /// Search as you type (Settings → General), or only when Enter is pressed.
    pub(super) fn search_live() -> bool {
        config::cur().b("search_live")
    }

    /// Nothing happens until Enter: Enter mode, or a question for Gemini.
    pub(super) fn waits_for_enter(&self) -> bool {
        if self.dict_mode() {
            return false;
        }
        !Self::search_live() || search::after_prefix(&self.edit_buf, &search::AI_PREFIXES).is_some()
    }

    pub(super) fn search_typed(&mut self) {
        // Gemini always waits for Enter (each question uses your quota).
        let ai = search::after_prefix(&self.edit_buf, &search::AI_PREFIXES).is_some();
        if (Self::search_live() || self.dict_mode()) && !ai {
            unsafe { SetTimer(Some(self.hwnd), T_SEARCH, 45, None) };
            // (Emptied: the whole guide at once.)
            self.retarget();
            return;
        }
        // Enter mode: the old results no longer match what's typed.
        // (Only once, when the shown results stop matching — so the hint
        // doesn't blink on every key.)
        let typed = self.search_text();
        let changed = self.search.as_ref().map(|s| (s.results.is_some() || !s.searched.is_empty()) && s.searched.trim() != typed.trim()).unwrap_or(false);
        if changed {
            if let Some(s) = self.search.as_mut() {
                s.results = None;
                s.searched.clear();
                s.at = Instant::now();
            }
        }
        // Its height follows what's typed (emptied: the whole guide again).
        self.retarget();
    }

    /// Enter: search when the text is new, otherwise open the chosen result.
    pub(super) fn search_enter(&mut self, folder: bool) {
        let typed = self.search_text();
        let fresh = self.search.as_ref().map(|s| s.results.is_some() && s.searched.trim() == typed.trim()).unwrap_or(false);
        if fresh || self.edit_buf.trim().is_empty() {
            self.search_open(None, folder);
        } else {
            self.search_now();
            self.retarget();
            self.render();
        }
    }

    pub(super) fn search_now(&mut self) {
        let q = self.search_text();
        if let Some(s) = self.search.as_mut() {
            s.searched = q.trim().to_string();
        }
        if q.trim().is_empty() {
            if let Some(s) = self.search.as_mut() {
                s.results = None;
                s.all = None;
                s.page = 0;
            }
            self.retarget();
        }
        search::query(&q);
    }

    /// How tall the search island wants to be right now.
    pub(super) fn search_height(&self) -> f32 {
        let Some(s) = &self.search else { return BAR_H };
        if self.edit_buf.trim().is_empty() && !s.dict {
            return BAR_H + 6.0 + guide().len() as f32 * GUIDE_H + FOOT_H + 6.0;
        }
        let Some(r) = &s.results else {
            // Enter mode, text typed: one line saying so.
            return if !self.waits_for_enter() { BAR_H } else { BAR_H + 6.0 + ROW_H + FOOT_H + 6.0 };
        };
        if r.query.is_empty() {
            return BAR_H;
        }
        if r.items.first().map(|i| i.kind == Kind::QrCode).unwrap_or(false) {
            return BAR_H + 6.0 + QR_ROW_H + FOOT_H + 6.0;
        }
        // A picture's text: as tall as the search gets (it scrolls).
        if r.items.first().map(|i| i.kind == Kind::OcrDoc).unwrap_or(false) {
            return SEARCH_MAX_H;
        }
        if let Some(it) = r.items.first().filter(|i| i.kind == Kind::Gemini) {
            return BAR_H + 6.0 + self.ai_height(it, ew() - 16.0) + FOOT_H + 6.0;
        }
        if r.items.first().map(|i| i.kind == Kind::Web).unwrap_or(false) {
            return BAR_H + 6.0 + r.items.len().min(WEB_ROWS) as f32 * WEB_ROW_H + FOOT_H + 6.0;
        }
        let rows = r.items.len().min(MAX_ROWS).max(1);
        BAR_H + 6.0 + rows as f32 * ROW_H + FOOT_H + 6.0
    }

    /// Click on a guide line: type its example.
    pub(super) fn search_guide(&mut self, i: usize) {
        let Some((_, _, _, _, fill)) = guide().get(i).copied() else { return };
        if fill.is_empty() {
            return;
        }
        if fill == "dict " && self.search_popup() {
            self.set_dict(true);
            self.render();
            return;
        }
        self.edit_buf = fill.to_string();
        // Examples run at once; prefixes ("tr ", "? ", "QR()") wait for your text.
        let run = !(fill.ends_with(' ') || fill.ends_with('('));
        if self.edit != Some(EditTarget::Search) {
            if self.search.as_ref().map(|s| s.in_page).unwrap_or(false) {
                self.begin_page_search();
            }
        }
        if run {
            self.search_now();
        } else {
            self.search_typed();
        }
        self.retarget();
        self.render();
    }

    /// The typed web question (without "? ").
    pub(super) fn web_query(&self) -> String {
        let q = self.search.as_ref().map(|s| s.searched.clone()).unwrap_or_default();
        let lower = q.to_lowercase();
        let _ = lower;
        search::after_prefix(&q, &search::WEB_PREFIXES).or_else(|| search::after_prefix(&q, &search::AI_PREFIXES)).unwrap_or(q)
    }

    /// One web result, laid out like the search page itself.
    fn draw_web_row(&mut self, it: &search::Item, rr: D2D_RECT_F, a: f32, sel: bool, k: usize) {
        let g = gfx::g();
        let accent = self.ic(pal::BLUE, 1.0);
        if sel {
            self.surf.fill_rr(rr, 14.0, with_alpha(accent, 0.14 * a));
        } else if self.hot == Some(Hit::SrchItem(k)) {
            self.surf.fill_rr(rr, 14.0, self.fg(0.06 * a));
        }
        let Some(m) = it.web.clone() else { return };
        // Its place in the results (1, 2, 3…) in a disc tinted with the
        // accent, so it reads in both light and dark mode.
        let ir = rf(rr.left + 12.0, rr.top + 9.0, 26.0, 26.0);
        self.surf.fill_circle(ir.left + 13.0, ir.top + 13.0, 13.0, with_alpha(accent, (if self.dark { 0.30 } else { 0.16 }) * a));
        let n = (20_000 - it.score + 1).clamp(1, 99);
        let num = if self.dark { rgb(0xe8f0fe, a) } else { with_alpha(accent, a) };
        self.surf.text(&n.to_string(), &self.f.body_c, ir, num);
        let tx = ir.right + 10.0;
        let tw = rr.right - tx - 30.0;
        self.surf.text(&m.site, &self.f.body, rf(tx, rr.top + 4.0, tw, 18.0), self.fg(0.9 * a));
        let crumb = ellipsize_middle(g, &m.crumb, &self.f.tiny, tw);
        self.surf.text(&crumb, &self.f.tiny, rf(tx, rr.top + 21.0, tw, 16.0), self.fg(0.5 * a));
        // Title in the link colour.
        let link = if self.dark { rgb(0x99c3ff, a) } else { rgb(0x1a0dab, a) };
        let lx = rr.left + 12.0;
        let lw = rr.right - lx - 30.0;
        self.surf.text(&it.name, &self.f.web_title, rf(lx, rr.top + 40.0, lw, 22.0), link);
        if !m.snippet.is_empty() {
            self.surf.clip(rf(lx, rr.top + 62.0, lw, 32.0));
            self.surf.text_wrap(&m.snippet, &self.f.wrap_src, rf(lx, rr.top + 62.0, lw, 60.0), self.fg(0.62 * a));
            self.surf.unclip();
        }
        if sel {
            self.surf.text("\u{E751}", &self.f.icon_s, rf(rr.right - 28.0, rr.top + 40.0, 22.0, 22.0), self.fg(0.6 * a));
        }
    }

    /// "Press Enter to search" (search-on-Enter mode, something typed).
    fn enter_hint(&mut self, x: f32, y: f32, w: f32, h: f32, a: f32) {
        let accent = self.ic(pal::BLUE, 1.0);
        let r = rf(x, y, w, h);
        let k = (self.search.as_ref().map(|s| s.at.elapsed().as_secs_f32()).unwrap_or(1.0) * 6.0).min(1.0);
        if k < 1.0 {
            self.keep_anim = true;
        }
        let a = a * k;
        self.surf.fill_rr(r, 12.0, with_alpha(accent, 0.1 * a));
        let br = rf(r.left + 10.0, r.top + (h - 26.0) / 2.0, 26.0, 26.0);
        self.surf.fill_rr(br, 8.0, with_alpha(accent, 0.9 * a));
        self.surf.text("\u{E751}", &self.f.icon_s, br, Island::text_on(accent, a));
        let q = self.edit_buf.trim().to_string();
        let lower = q.to_lowercase();
        let what = if search::after_prefix(&q, &search::WEB_PREFIXES).is_some() {
            tr("Press Enter to search Google").to_string()
        } else if search::after_prefix(&q, &search::AI_PREFIXES).is_some() {
            tr("Press Enter to ask Gemini").to_string()
        } else if ["tr ", "translate ", "dịch "].iter().any(|p| lower.starts_with(p)) {
            tr("Press Enter to translate").to_string()
        } else {
            tr("Press Enter to search").to_string()
        };
        self.surf.text(&what, &self.f.srch_name, rf(br.right + 12.0, r.top, r.right - br.right - 20.0, h), self.fg(0.85 * a));
        self.hits.push((Hit::SrchEnter, r));
    }

    /// Ctrl+V: a picture on the clipboard is read as a QR code.
    pub(super) fn search_paste(&mut self) -> bool {
        if !crate::qr::clipboard_has_image() {
            return false;
        }
        // A sharp thumbnail of the picture stands in the box.
        // Decoded at exactly the size it's shown, so it stays crisp.
        self.srch_thumb = crate::qr::clipboard_image_bytes().and_then(|b| {
            let probe = Image::from_bytes_fit(&b, 512, 0.0).ok()?;
            let (w, h) = (probe.w as f32, probe.h as f32);
            let dh = 38.0f32.min(72.0 * h / w.max(1.0));
            let dw = (dh * w / h.max(1.0)).min(72.0);
            let px = (dw.max(dh) * self.scale).round().max(8.0) as u32;
            Image::from_bytes_fit(&b, px, 0.0).ok()
        });
        // A "<" in the box: read the text in it (OCR); otherwise its QR code.
        let ocr = self.edit_buf.contains('<');
        let mark = if ocr { search::OCR_MARK } else { search::IMAGE_MARK };
        self.edit_buf = mark.to_string();
        self.edit_cur = None;
        if let Some(s) = self.search.as_mut() {
            s.searched = mark.to_string();
        }
        search::read_pasted(ocr);
        self.render();
        true
    }

    /// The guide lines (empty search box).
    pub(super) fn draw_guide(&mut self, x: f32, top: f32, w: f32, rh: f32, a: f32, compact: bool) {
        let t = self.search.as_ref().map(|s| s.at.elapsed().as_secs_f32()).unwrap_or(1.0);
        for (i, (col, badge, title, ex, _)) in guide().into_iter().enumerate() {
            let ti = ((t * 6.0) - i as f32 * 0.35).clamp(0.0, 1.0);
            if ti < 1.0 {
                self.keep_anim = true;
            }
            let ra = a * ti;
            let r = rf(x, top + i as f32 * rh + 4.0 * (1.0 - ti), w, rh - 2.0);
            if self.hot == Some(Hit::SrchGuide(i)) {
                self.surf.fill_rr(r, 9.0, self.fg(0.07 * ra));
            }
            // The badge sits on the row's middle line, as the text does (on the
            // page the rows are short: the badge keeps a readable size and is
            // only as wide as its letters need).
            let bs = (rh - 6.0).clamp(14.0, 24.0);
            let cy = r.top + (rh - 2.0) / 2.0;
            let icon = badge.chars().next().map(|ch| ('\u{E000}'..='\u{F8FF}').contains(&ch)).unwrap_or(false);
            let slot = bs.max(22.0);
            let bw = if icon || badge == "AI" { bs } else { bs.max(gfx::g().measure(badge, &self.f.tiny_c) + 6.0).min(slot) };
            let br = rf(r.left + 8.0 + (slot - bw) / 2.0, cy - bs / 2.0, bw, bs);
            let c = self.ic(col, 1.0);
            if badge == "AI" {
                self.gemini_tile(br, ra);
            } else {
                self.surf.fill_rr(br, bs * 0.28, with_alpha(c, 0.9 * ra));
                self.surf.text(badge, if icon { &self.f.icon_s } else { &self.f.tiny_c }, br, Island::text_on(c, ra));
            }
            // Titles line up whatever the badge's width.
            let tx = r.left + 8.0 + slot + 9.0;
            let g = gfx::g();
            let title = tr(title);
            let tw = g.measure(title, &self.f.body) + 12.0;
            self.surf.text(title, &self.f.body, rf(tx, r.top, tw, rh - 2.0), self.fg(0.9 * ra));
            let ex = tr(ex);
            let ew = r.right - tx - tw - 6.0;
            if ew > 40.0 {
                let shown = if compact { ellipsize_middle(g, ex, &self.f.tiny, ew) } else { ex.to_string() };
                self.surf.text(&shown, &self.f.tiny, rf(tx + tw, r.top, ew, rh - 2.0), self.fg(0.5 * ra));
            }
            self.hits.push((Hit::SrchGuide(i), r));
        }
    }

    pub(super) fn search_move(&mut self, d: i32) {
        let Some(s) = self.search.as_mut() else { return };
        let n = s.results.as_ref().map(|r| r.items.len()).unwrap_or(0);
        if n == 0 {
            return;
        }
        let vis = s.vis_rows.max(1);
        // Past the end of a page: on to the next (or back to the one before).
        let pages = s.pages();
        if pages > 1 && d.abs() == 1 && ((d > 0 && s.sel + 1 >= n) || (d < 0 && s.sel == 0)) {
            let p = (s.page as i32 + d).rem_euclid(pages as i32) as usize;
            s.show_page(p, d < 0);
            self.render();
            return;
        }
        s.sel = if d.abs() > 1 { (s.sel as i32 + d).clamp(0, n as i32 - 1) as usize } else { (s.sel as i32 + d).rem_euclid(n as i32) as usize };
        if s.sel < s.first {
            s.first = s.sel;
        } else if s.sel >= s.first + vis {
            s.first = s.sel + 1 - vis;
        }
        self.render();
    }

    /// PgDn / PgUp (or the footer's arrows): the next / previous page of
    /// results; with only one page, a screenful down / up.
    pub(super) fn search_page(&mut self, d: i32) {
        let Some(s) = self.search.as_mut() else { return };
        let pages = s.pages();
        if pages <= 1 {
            self.search_move(d * MAX_ROWS as i32);
            return;
        }
        let p = (s.page as i32 + d).clamp(0, pages as i32 - 1) as usize;
        if p != s.page {
            s.show_page(p, false);
            s.at = Instant::now();
        }
        self.render();
    }

    /// Enter (or a click): open; `folder` (Ctrl+Enter) shows it in Explorer.
    pub(super) fn search_open(&mut self, i: Option<usize>, folder: bool) {
        let Some(s) = &self.search else { return };
        let i = i.unwrap_or(s.sel);
        let Some(it) = s.results.as_ref().and_then(|r| r.items.get(i)).cloned() else {
            // Nothing to open: offer Everything when that's what is missing.
            self.search_engine_action();
            return;
        };
        if matches!(it.kind, Kind::Dict | Kind::OcrText) && it.target.is_empty() {
            // A hint ("type a word", "paste a picture"): nothing to do yet.
            return;
        }
        if it.kind == Kind::WebAction {
            // Run the web search right here.
            self.edit_buf = it.target.clone();
            if let Some(s) = self.search.as_mut() {
                s.sel = 0;
                s.first = 0;
            }
            self.search_now();
            self.render();
            return;
        }
        search::open(&it, folder);
        let in_page = s.in_page;
        self.close_search();
        if in_page {
            self.edit_buf.clear();
            self.set_expanded(false);
        }
    }

    pub(super) fn search_engine_action(&mut self) {
        let engine = self.search.as_ref().and_then(|s| s.results.as_ref()).map(|r| r.engine.clone());
        match engine {
            Some(Engine::NotRunning(exe)) => {
                search::start_everything(&exe);
                // Ask again once it's up.
                unsafe { SetTimer(Some(self.hwnd), T_SEARCH, 1500, None) };
            }
            Some(Engine::Missing) => {
                util::shell_open(search::DOWNLOAD_URL);
                self.close_search();
            }
            _ => {}
        }
    }

    /// Load a result's icon into the cache (a few per frame, so typing stays smooth).
    fn search_icon(&mut self, target: &str, budget: &mut u32) {
        if self.srch_icons.contains_key(target) {
            return;
        }
        if *budget == 0 {
            self.srch_more = true;
            return;
        }
        *budget -= 1;
        let px = (28.0 * self.scale).round() as u32;
        let img = crate::shelf::shell_image(target, px);
        if self.srch_icons.len() > 300 {
            self.srch_icons.clear();
        }
        self.srch_icons.insert(target.to_string(), img);
    }

    pub(super) fn draw_search(&mut self, x: f32, y: f32, w: f32, h: f32, a: f32) {
        if a <= 0.01 {
            return;
        }
        let g = gfx::g();
        let accent = self.ic(pal::BLUE, 1.0);
        // ---- The bar ----
        let bar = rf(x, y, w, BAR_H);
        let dict = self.dict_mode();
        // (Dictionary mode: a book instead of the magnifier.)
        let glyph = if dict { "\u{E82D}" } else { "\u{E721}" };
        self.surf.text(glyph, &self.f.icon_ctl, rf(x + 16.0, y + (BAR_H - 30.0) / 2.0, 30.0, 30.0), if dict { with_alpha(accent, a) } else { self.fg(0.65 * a) });
        let tx = x + 54.0;
        let n_sw = self.hits.len();
        // (The Filename | Content switch means nothing to the dictionary.)
        let sw_w = if dict { 0.0 } else { self.search_mode_switch(x + w - 132.0, y + BAR_H / 2.0, a) };
        // (Its hits go on top of the bar's, below.)
        let sw_hits: Vec<_> = self.hits.drain(n_sw..).collect();
        let tw = w - 54.0 - 120.0 - sw_w - 12.0;
        let q = self.edit_buf.clone();
        let fr = rf(tx, y, tw, BAR_H);
        if search::is_image_mark(&q) && self.srch_thumb.is_some() {
            // The pasted picture, small and sharp, where the text would be.
            let (iw, ih) = self.srch_thumb.as_ref().map(|i| (i.w as f32, i.h as f32)).unwrap_or((1.0, 1.0));
            let th = 38.0f32;
            let tw_ = (th * iw / ih.max(1.0)).clamp(20.0, 72.0);
            let th = (tw_ * ih / iw.max(1.0)).min(38.0);
            let r = rf(tx, y + (BAR_H - th) / 2.0, tw_, th);
            self.surf.fill_rr(rf(r.left - 2.0, r.top - 2.0, tw_ + 4.0, th + 4.0), 8.0, self.fg(0.18 * a));
            self.surf.clip(r);
            if let Some(img) = &self.srch_thumb {
                self.surf.image(img, r, a);
            }
            self.surf.unclip();
            let what = if q == search::OCR_MARK { tr("Pasted picture · its text (OCR)") } else { tr("Pasted picture · QR code") };
            self.surf.text(what, &self.f.tiny, rf(r.right + 12.0, y, (tw - tw_ - 12.0).max(40.0), BAR_H), self.fg(0.5 * a));
        } else if q.is_empty() {
            let ph = if dict {
                tr("Look up a word (Google Translate)")
            } else if matches!(self.search.as_ref().and_then(|s| s.results.as_ref()).map(|r| &r.engine), Some(Engine::Missing) | Some(Engine::NotRunning(_))) {
                tr("Search apps")
            } else {
                tr("Search apps and files")
            };
            self.surf.text(ph, &self.f.srch, fr, self.fg(0.38 * a));
        } else {
            let font = self.f.srch.clone();
            self.draw_edit_line(&font, rf(tx, fr.top, tw - 6.0, fr.bottom - fr.top), self.fg(a), with_alpha(accent, a), 22.0, None, a);
        }
        if q.is_empty() && self.caret {
            self.surf.fill_rect(rf(tx + 1.0, y + BAR_H / 2.0 - 11.0, 2.0, 22.0), with_alpha(accent, a));
        }
        // Right of the bar: what's answering.
        let (res, pending) = self.search.as_ref().map(|s| (s.results.clone(), s.results.as_ref().map(|r| r.pending).unwrap_or(false))).unwrap_or((None, false));
        let hint = if pending {
            tr("Searching…").to_string()
        } else if let Some(r) = &res {
            match r.engine {
                Engine::Ready if r.total > 0 => format!("{} {}", fmt_count(r.total), tr(if r.total == 1 { "file" } else { "files" })),
                _ => String::new(),
            }
        } else {
            "Alt+S".to_string()
        };
        self.surf.text(&hint, &self.f.tiny_r, rf(x + w - 130.0, y, 112.0, BAR_H), self.fg(0.4 * a));
        self.hits.push((Hit::SrchBar, bar));
        self.hits.extend(sw_hits);

        if self.edit_buf.trim().is_empty() && !self.dict_mode() {
            if h > BAR_H + 20.0 {
                let top = y + BAR_H;
                self.surf.fill_rect(rf(x + 16.0, top, w - 32.0, 1.0), self.fg(0.1 * a));
                self.surf.clip(rf(x, top + 2.0, w, (h - BAR_H - FOOT_H - 6.0).max(0.0)));
                self.draw_guide(x + 8.0, top + 6.0, w - 16.0, GUIDE_H, a, false);
                self.surf.unclip();
                let fy = y + h - FOOT_H - 2.0;
                self.surf.fill_rect(rf(x + 16.0, fy, w - 32.0, 1.0), self.fg(0.08 * a));
                let keys = tr("↵ open   ·   Ctrl+↵ show in folder   ·   ↑↓ choose   ·   ←→ edit   ·   Esc close");
                self.surf.text(keys, &self.f.tiny, rf(x + 18.0, fy + 1.0, w - 36.0, FOOT_H), self.fg(0.4 * a));
            }
            return;
        }
        let Some(r) = res else {
            if self.waits_for_enter() && h > BAR_H + 20.0 {
                self.enter_hint(x + 8.0, y + BAR_H + 6.0, w - 16.0, ROW_H - 4.0, a);
            }
            return;
        };
        if r.query.is_empty() || h < BAR_H + 4.0 {
            return;
        }
        let top = y + BAR_H;
        self.surf.fill_rect(rf(x + 16.0, top, w - 32.0, 1.0), self.fg(0.1 * a));
        let list_top = top + 6.0;
        let bottom = y + h - FOOT_H - 4.0;
        self.surf.clip(rf(x, list_top, w, (bottom - list_top).max(0.0)));
        let (sel, first, at) = self.search.as_ref().map(|s| (s.sel, s.first, s.at)).unwrap();
        let t = at.elapsed().as_secs_f32();
        let mut budget = 3u32;
        self.srch_more = false;
        if r.items.is_empty() && !r.pending {
            self.surf.text(tr("No results"), &self.f.body, rf(x + 54.0, list_top, w - 70.0, ROW_H), self.fg(0.5 * a));
        }
        let web = r.items.first().map(|i| i.kind == Kind::Web).unwrap_or(false);
        let ai = r.items.first().filter(|i| i.kind == Kind::Gemini).cloned();
        let doc = r.items.first().map(|i| i.kind == Kind::OcrDoc).unwrap_or(false);
        if let Some(s) = self.search.as_mut() {
            s.vis_rows = if web { WEB_ROWS } else { MAX_ROWS };
        }
        if doc {
            self.draw_doc(rf(x + 8.0, list_top - 4.0, w - 16.0, bottom - list_top + 4.0), a);
        }
        if let Some(it) = &ai {
            let ti = (t * 6.0).clamp(0.0, 1.0);
            if ti < 1.0 {
                self.keep_anim = true;
            }
            self.draw_ai(it, rf(x + 8.0, list_top - 6.0 * (1.0 - ti), w - 16.0, bottom - list_top), a * ti, false);
        }
        for (k, it) in r.items.iter().enumerate().skip(first).take(if ai.is_some() || doc { 0 } else if web { WEB_ROWS + 1 } else { MAX_ROWS }) {
            if it.kind == Kind::Web {
                let row = k - first;
                let ti = ((t * 7.0) - row as f32 * 0.55).clamp(0.0, 1.0);
                let ti = 1.0 - (1.0 - ti) * (1.0 - ti);
                if ti < 1.0 {
                    self.keep_anim = true;
                }
                let rr = rf(x + 8.0, list_top + row as f32 * WEB_ROW_H - 8.0 * (1.0 - ti), w - 16.0, WEB_ROW_H - 4.0);
                self.draw_web_row(it, rr, a * ti, k == sel, k);
                if rr.bottom <= bottom + 2.0 {
                    self.hits.push((Hit::SrchItem(k), rr));
                }
                continue;
            }
            if it.kind == Kind::QrCode {
                if let Some(m) = it.qr.clone() {
                    self.draw_qr_row(&m, it, rf(x + 8.0, list_top, w - 16.0, QR_ROW_H - 4.0), a, k == sel, k);
                }
                continue;
            }
            let row = k - first;
            // Rows fall in one after another.
            let ti = ((t * 7.0) - row as f32 * 0.55).clamp(0.0, 1.0);
            let ti = 1.0 - (1.0 - ti) * (1.0 - ti);
            if ti < 1.0 {
                self.keep_anim = true;
            }
            let ra = a * ti;
            let ry = list_top + row as f32 * ROW_H - 8.0 * (1.0 - ti);
            let rr = rf(x + 8.0, ry, w - 16.0, ROW_H - 4.0);
            let is_sel = k == sel;
            if is_sel {
                self.surf.fill_rr(rr, 12.0, with_alpha(accent, 0.22 * ra));
            } else if self.hot == Some(Hit::SrchItem(k)) {
                self.surf.fill_rr(rr, 12.0, self.fg(0.07 * ra));
            }
            // Dictionary: one of the word's other translations, laid out like
            // Google Translate — the word, its part of speech, and what it
            // means back, as chips.
            if it.kind == Kind::Dict {
                if let Some(back) = &it.snip {
                    let inner = rr.right - rr.left - 24.0;
                    let ww = (inner * 0.36).max(120.0);
                    let wx = rr.left + 14.0;
                    self.surf.text(&it.name, &self.f.srch_name, rf(wx, rr.top, ww - 10.0, ROW_H - 4.0), self.fg(0.95 * ra));
                    let px = wx + ww;
                    let pw = 96.0;
                    self.surf.text(&it.dir, &self.f.tiny, rf(px, rr.top, pw - 8.0, ROW_H - 4.0), self.fg(0.6 * ra));
                    let mut cx = px + pw;
                    let limit = rr.right - if is_sel { 36.0 } else { 12.0 };
                    for b in back.split('\u{1}').filter(|b| !b.is_empty()) {
                        let cw = g.measure(b, &self.f.tiny) + 16.0;
                        if cx + cw > limit {
                            if cx + 20.0 <= limit {
                                self.surf.text("…", &self.f.tiny, rf(cx, rr.top, 20.0, ROW_H - 4.0), self.fg(0.5 * ra));
                            }
                            break;
                        }
                        let cr = rf(cx, rr.top + (ROW_H - 4.0 - 22.0) / 2.0, cw, 22.0);
                        self.surf.fill_rr(cr, 5.0, self.fg(0.1 * ra));
                        self.surf.text(b, &self.f.tiny_c, cr, self.fg(0.8 * ra));
                        cx += cw + 6.0;
                    }
                    if is_sel {
                        self.surf.text("\u{E751}", &self.f.icon_s, rf(rr.right - 30.0, rr.top, 24.0, ROW_H - 4.0), self.fg(0.6 * ra));
                    }
                    if rr.bottom <= bottom + 2.0 {
                        self.hits.push((Hit::SrchItem(k), rr));
                    }
                    continue;
                }
            }
            // Icon
            let ir = rf(rr.left + 10.0, rr.top + (ROW_H - 4.0 - 28.0) / 2.0, 28.0, 28.0);
            self.result_icon(it, ir, ra, &mut budget);
            // Name (the matching part in the accent colour) and where it lives.
            let nx = ir.right + 12.0;
            let tag = match it.kind {
                Kind::App => tr("App"),
                Kind::Folder => tr("Folder"),
                k if k.is_answer() => tr("Enter copies"),
                _ => "",
            };
            let tagw = if tag.is_empty() { 0.0 } else { g.measure(tag, &self.f.tiny) + 16.0 };
            let mut nw = rr.right - nx - tagw - 10.0;
            // Inside files: name and folder on the left, the passage on the right.
            if let Some(snip) = &it.snip {
                let full = nw;
                nw = (full * 0.4).max(120.0);
                let sx = nx + nw + 14.0;
                let sr = rf(sx, rr.top + 3.0, (nx + full - sx - 26.0).max(20.0), ROW_H - 10.0);
                self.surf.clip(sr);
                let marks = Island::word_marks(snip, &r.query);
                self.surf.text_marked(snip, &self.f.wrap_src, sr, self.fg(0.72 * ra), with_alpha(accent, ra), &marks);
                self.surf.unclip();
            }
            let sub = match it.kind {
                Kind::App => String::new(),
                Kind::Calc => it.dir.clone(),
                _ => it.dir.clone(),
            };
            let name_y = if sub.is_empty() { rr.top } else { rr.top + 3.0 };
            let name_h = if sub.is_empty() { ROW_H - 4.0 } else { 22.0 };
            if it.kind == Kind::Thinking {
                self.thinking_text(&it.name, rf(nx, name_y, nw, name_h), ra);
            } else if it.kind.is_answer() || it.kind == Kind::Error {
                self.surf.text(&it.name, &self.f.srch_name, rf(nx, name_y, nw, name_h), self.fg(0.95 * ra));
            } else {
                self.draw_match(&it.name, &r.query, rf(nx, name_y, nw, name_h), ra, accent);
            }
            if !sub.is_empty() {
                let sub = ellipsize_middle(g, &sub, &self.f.tiny, nw);
                self.surf.text(&sub, &self.f.tiny, rf(nx, rr.top + 23.0, nw, 16.0), self.fg(0.48 * ra));
            }
            if !tag.is_empty() {
                let tr_ = rf(rr.right - tagw - 6.0, rr.top + (ROW_H - 4.0 - 20.0) / 2.0, tagw, 20.0);
                if is_sel {
                    self.surf.text("\u{E751}", &self.f.icon_s, tr_, self.fg(0.6 * ra));
                } else {
                    self.surf.fill_rr(tr_, 10.0, self.fg(0.07 * ra));
                    self.surf.text(tag, &self.f.tiny_c, tr_, self.fg(0.55 * ra));
                }
            } else if is_sel {
                self.surf.text("\u{E751}", &self.f.icon_s, rf(rr.right - 30.0, rr.top, 24.0, ROW_H - 4.0), self.fg(0.6 * ra));
            }
            if rr.bottom <= bottom + 2.0 {
                self.hits.push((Hit::SrchItem(k), rr));
            }
        }
        self.surf.unclip();
        if self.srch_more {
            self.keep_anim = true;
        }
        // ---- Footer ----
        let fy = y + h - FOOT_H - 2.0;
        self.surf.fill_rect(rf(x + 16.0, fy, w - 32.0, 1.0), self.fg(0.08 * a));
        let fr = rf(x + 18.0, fy + 1.0, w - 36.0, FOOT_H);
        if search::content_busy() {
            self.content_status(rf(x + 18.0, fy + 1.0, w - 36.0, FOOT_H), a);
            return;
        }
        if doc {
            let keys = tr("↵ copy all   ·   drag to select and copy   ·   Alt+Q translates   ·   Esc close");
            self.surf.text(keys, &self.f.tiny, fr, self.fg(0.4 * a));
            return;
        }
        if web || ai.is_some() {
            // Web: the full page on Google. Gemini: the same question on Google.
            let keys = if web { tr("↵ open   ·   Ctrl+↵ copy link   ·   ↑↓ choose   ·   ←→ edit   ·   Esc close") } else { tr("↵ copy the answer   ·   scroll to read   ·   ←→ edit   ·   Esc close") };
            self.surf.text(keys, &self.f.tiny, fr, self.fg(0.4 * a));
            let label = if web { tr("All results on Google").to_string() } else { tr("Search Google").to_string() };
            let lw = g.measure(&label, &self.f.tiny) + 34.0;
            let br = rf(x + w - 18.0 - lw, fy + 6.0, lw, FOOT_H - 10.0);
            let hot = self.hot == Some(Hit::SrchWebAll);
            self.surf.fill_rr(br, (FOOT_H - 10.0) / 2.0, with_alpha(accent, if hot { 0.32 } else { 0.18 } * a));
            self.surf.text(&label, &self.f.tiny, rf(br.left + 12.0, br.top, lw - 30.0, br.bottom - br.top), self.fg(0.9 * a));
            self.surf.text("\u{E8A7}", &self.f.icon_s, rf(br.right - 22.0, br.top, 14.0, br.bottom - br.top), with_alpha(accent, a));
            self.hits.push((Hit::SrchWebAll, br));
            return;
        }
        match &r.engine {
            Engine::Ready => {
                let many = self.search.as_ref().map(|s| s.pages() > 1).unwrap_or(false);
                if self.dict_mode() {
                    self.surf.text(tr("↵ copy   ·   ↑↓ choose   ·   ←→ edit   ·   Esc close"), &self.f.tiny, fr, self.fg(0.4 * a));
                    self.surf.text("Google Translate", &self.f.tiny_r, fr, self.fg(0.3 * a));
                    return;
                }
                let keys = if many { tr("↵ open   ·   ↑↓ choose   ·   PgUp PgDn pages   ·   Esc close") } else { tr("↵ open   ·   Ctrl+↵ show in folder   ·   ↑↓ choose   ·   ←→ edit   ·   Esc close") };
                self.surf.text(keys, &self.f.tiny, fr, self.fg(0.4 * a));
                // Who answered: Phemio's own index (Content, drives chosen),
                // Windows' index (Content), or Everything (names).
                let src = if !config::cur().b("srch_content") {
                    "Everything"
                } else if crate::contentindex::enabled() {
                    "Phemio"
                } else {
                    "Windows Search"
                };
                self.surf.text(src, &self.f.tiny_r, fr, self.fg(0.3 * a));
                // More than a page: which rows these are, and arrows to turn.
                let (page, pages, n) = self.search.as_ref().map(|s| (s.page, s.pages(), s.all.as_ref().map(|r| r.items.len()).unwrap_or(0))).unwrap_or((0, 1, 0));
                if pages > 1 {
                    let label = format!("{}–{} / {n}", page * PAGE + 1, ((page + 1) * PAGE).min(n));
                    let lw = g.measure(&label, &self.f.tiny) + 8.0;
                    let right = fr.right - g.measure(src, &self.f.tiny_r) - 14.0;
                    let bh = FOOT_H - 12.0;
                    let nb = rf(right - bh, fy + 7.0, bh, bh);
                    let lr = rf(nb.left - 4.0 - lw, fy + 1.0, lw, FOOT_H);
                    let pb = rf(lr.left - 4.0 - bh, fy + 7.0, bh, bh);
                    for (r, next, glyph) in [(pb, false, "\u{E76B}"), (nb, true, "\u{E76C}")] {
                        let on = if next { page + 1 < pages } else { page > 0 };
                        let hot = self.hot == Some(Hit::SrchTurn(next));
                        self.surf.fill_rr(r, bh / 2.0, with_alpha(accent, if !on { 0.06 } else if hot { 0.32 } else { 0.16 } * a));
                        self.surf.text(glyph, &self.f.icon_s, r, if on { with_alpha(accent, a) } else { self.fg(0.25 * a) });
                        if on {
                            self.hits.push((Hit::SrchTurn(next), r));
                        }
                    }
                    self.surf.text(&label, &self.f.tiny_c, lr, self.fg(0.6 * a));
                }
            }
            other => {
                let (glyph, label) = match other {
                    Engine::NotRunning(_) => ("\u{E768}", tr("Everything isn't running — start it to search files")),
                    _ => ("\u{E896}", tr("Install Everything (free, voidtools.com) to search all your files")),
                };
                let lw = g.measure(label, &self.f.tiny) + 40.0;
                let br = rf(x + (w - lw) / 2.0, fy + 6.0, lw, FOOT_H - 10.0);
                let hot = self.hot == Some(Hit::SrchEngine);
                self.surf.fill_rr(br, (FOOT_H - 10.0) / 2.0, with_alpha(accent, if hot { 0.32 } else { 0.18 } * a));
                self.surf.text(glyph, &self.f.icon_s, rf(br.left + 10.0, br.top, 16.0, br.bottom - br.top), with_alpha(accent, a));
                self.surf.text(label, &self.f.tiny, rf(br.left + 30.0, br.top, lw - 34.0, br.bottom - br.top), self.fg(0.85 * a));
                self.hits.push((Hit::SrchEngine, br));
            }
        }
    }

    /// A QR(text) result: the code on white, the text beside it.
    fn draw_qr_row(&mut self, m: &crate::qr::Matrix, it: &search::Item, r: D2D_RECT_F, a: f32, sel: bool, k: usize) {
        let accent = self.ic(pal::BLUE, 1.0);
        if sel {
            self.surf.fill_rr(r, 14.0, with_alpha(accent, 0.12 * a));
        }
        let side = (r.bottom - r.top - 12.0).max(40.0);
        let q = rf(r.left + 10.0, r.top + 6.0, side, side);
        self.surf.fill_rr(q, 10.0, rgb(0xffffff, a));
        let inner = side - 16.0;
        let cell = inner / m.n as f32;
        for yy in 0..m.n {
            for xx in 0..m.n {
                if m.dark[yy * m.n + xx] {
                    self.surf.fill_rect(rf(q.left + 8.0 + xx as f32 * cell, q.top + 8.0 + yy as f32 * cell, cell + 0.35, cell + 0.35), rgb(0x000000, a));
                }
            }
        }
        let tx = q.right + 16.0;
        let tw = r.right - tx - 10.0;
        self.surf.text(tr("QR code"), &self.f.label, rf(tx, r.top + 12.0, tw, 16.0), self.fg(0.45 * a));
        self.surf.text_wrap(&it.name, &self.f.wrap_src, rf(tx, r.top + 32.0, tw, side - 70.0), self.fg(0.9 * a));
        let hint = tr("Enter copies the picture — paste it anywhere");
        self.surf.text(hint, &self.f.tiny, rf(tx, q.bottom - 22.0, tw, 18.0), with_alpha(accent, a));
        self.hits.push((Hit::SrchItem(k), r));
    }

    /// The name with the typed text picked out.
    /// Where the search's words appear in `text` (UTF-16 start, length), any case.
    pub(super) fn word_marks(text: &str, q: &str) -> Vec<(u32, u32)> {
        // The search's terms exactly as found (whole words, accents count).
        let terms = crate::contentsearch::words_of(q);
        let chars: Vec<char> = text.chars().collect();
        // UTF-16 offset of each char.
        let mut at = Vec::with_capacity(chars.len() + 1);
        let mut u = 0u32;
        for c in &chars {
            at.push(u);
            u += c.len_utf16() as u32;
        }
        at.push(u);
        let mut marks: Vec<(u32, u32)> = crate::contentsearch::find_terms(text, &terms).into_iter().map(|(s, e, _)| (at[s], at[e] - at[s])).collect();
        marks.sort();
        marks.dedup();
        marks
    }

    fn draw_match(&mut self, name: &str, q: &str, r: D2D_RECT_F, a: f32, accent: D2D1_COLOR_F) {
        let g = gfx::g();
        let f = &self.f.srch_name;
        let lower = name.to_lowercase();
        let ql = q.trim().to_lowercase();
        let found = if ql.is_empty() { None } else { lower.find(&ql) };
        // Only when byte offsets line up with the original (no case-folding surprises).
        match found.filter(|&i| name.is_char_boundary(i) && name.is_char_boundary(i + ql.len()) && lower.len() == name.len()) {
            Some(i) if g.measure(name, f) <= r.right - r.left => {
                let (a0, rest) = name.split_at(i);
                let (m, b0) = rest.split_at(ql.len());
                let w0 = g.measure(a0, f);
                let wm = g.measure(m, f);
                self.surf.text(a0, f, rf(r.left, r.top, w0 + 4.0, r.bottom - r.top), self.fg(0.92 * a));
                self.surf.text(m, f, rf(r.left + w0, r.top, wm + 4.0, r.bottom - r.top), with_alpha(accent, a));
                self.surf.text(b0, f, rf(r.left + w0 + wm, r.top, r.right - r.left - w0 - wm, r.bottom - r.top), self.fg(0.92 * a));
            }
            _ => self.surf.text(name, f, r, self.fg(0.92 * a)),
        }
    }
}

impl Island {
    /// The Search page: a field, then the results in a scrolling list.
    pub(super) fn page_search(&mut self, bx: f32, by: f32, bw: f32, bh: f32, a: f32) {
        let g = gfx::g();
        let accent = self.ic(pal::BLUE, 1.0);
        let editing = self.edit == Some(EditTarget::Search) && self.search.as_ref().map(|s| s.in_page).unwrap_or(false);
        let field = rf(bx, by - 4.0, bw, 36.0);
        self.surf.fill_rr(field, 12.0, self.fg(if editing { 0.12 } else { 0.07 } * a));
        if editing {
            self.surf.stroke_rr(field, 12.0, with_alpha(accent, 0.7 * a), 1.2);
        }
        self.surf.text("\u{E721}", &self.f.icon, rf(field.left + 8.0, field.top, 24.0, 36.0), self.fg(0.6 * a));
        let tx = field.left + 38.0;
        let n_sw = self.hits.len();
        let sw_w = self.search_mode_switch(field.right - 108.0, field.top + 18.0, a);
        let sw_hits: Vec<_> = self.hits.drain(n_sw..).collect();
        let tw = bw - 38.0 - 110.0 - sw_w - 10.0;
        let q = self.edit_buf.clone();
        let font = self.f.title.clone();
        if search::is_image_mark(&q) && self.srch_thumb.is_some() {
            let (iw, ih) = self.srch_thumb.as_ref().map(|i| (i.w as f32, i.h as f32)).unwrap_or((1.0, 1.0));
            let th = 26.0f32.min(60.0 * ih / iw.max(1.0));
            let twd = th * iw / ih.max(1.0);
            let r = rf(tx, field.top + (36.0 - th) / 2.0, twd, th);
            if let Some(img) = &self.srch_thumb {
                self.surf.image(img, r, a);
            }
            let what = if q == search::OCR_MARK { tr("Pasted picture · its text (OCR)") } else { tr("Pasted picture · QR code") };
            self.surf.text(what, &self.f.tiny, rf(r.right + 10.0, field.top, (tw - twd - 10.0).max(40.0), 36.0), self.fg(0.5 * a));
        } else if editing {
            if q.is_empty() {
                self.surf.text(tr("Search apps and files"), &font, rf(tx + 4.0, field.top, tw, 36.0), self.fg(0.38 * a));
            }
            self.draw_edit_line(&font, rf(tx, field.top, tw - 6.0, 36.0), self.fg(a), with_alpha(accent, a), 18.0, None, a);
        } else if q.is_empty() {
            self.surf.text(tr("Search apps and files"), &font, rf(tx, field.top, tw, 36.0), self.fg(0.38 * a));
        } else {
            let mut shown = q.clone();
            while g.measure(&shown, &font) > tw - 6.0 && shown.chars().count() > 1 {
                shown = shown.chars().skip(1).collect();
            }
            self.surf.text(&shown, &font, rf(tx, field.top, tw, 36.0), self.fg(a));
        }
        let res = self.search.as_ref().filter(|s| s.in_page).and_then(|s| s.results.clone());
        let hint = match &res {
            Some(r) if r.pending => tr("Searching…").to_string(),
            Some(r) if r.engine == Engine::Ready && r.total > 0 => format!("{} {}", fmt_count(r.total), tr(if r.total == 1 { "file" } else { "files" })),
            _ => "Alt+S".to_string(),
        };
        if search::content_busy() {
            // Looking inside files: a moving line under the field, and Cancel.
            self.content_status(rf(field.right - 104.0, field.top + 4.0, 98.0, 28.0), a);
            self.busy_line(rf(field.left + 12.0, field.bottom - 3.0, field.right - field.left - 24.0, 2.0), a);
        } else {
            self.surf.text(&hint, &self.f.tiny_r, rf(field.right - 104.0, field.top, 94.0, 36.0), self.fg(0.4 * a));
        }
        if !editing {
            self.hits.push((Hit::SrchPage, field));
        }
        self.hits.extend(sw_hits);
        let top = field.bottom + 6.0;
        let bottom = by + bh;
        let rh = 30.0;
        let vis = (((bottom - top) / rh).floor() as usize).max(1);
        if let Some(s) = self.search.as_mut() {
            s.vis_rows = vis;
        }
        if !self.edit_buf.trim().is_empty() && res.is_none() && self.waits_for_enter() {
            self.enter_hint(bx - 4.0, top, bw + 8.0, 30.0, a);
            return;
        }
        let Some(r) = res.filter(|r| !r.query.is_empty() && !self.edit_buf.trim().is_empty()) else {
            // Nothing typed yet: what the box can do.
            let gh = ((bottom - top) / guide().len() as f32).min(26.0);
            self.surf.clip(rf(bx - 6.0, top, bw + 12.0, bottom - top));
            self.draw_guide(bx - 4.0, top, bw + 8.0, gh, a, true);
            self.surf.unclip();
            if matches!(self.search.as_ref().and_then(|s| s.results.as_ref()).map(|r| &r.engine), Some(Engine::Missing) | Some(Engine::NotRunning(_))) {
                self.engine_button(bx, bottom - 26.0, bw, a);
            }
            return;
        };
        self.zones.push((4, rf(bx - 10.0, top, bw + 20.0, bottom - top)));
        let (sel, first, at) = self.search.as_ref().map(|s| (s.sel, s.first, s.at)).unwrap();
        let t = at.elapsed().as_secs_f32();
        let mut budget = 3u32;
        self.srch_more = false;
        self.surf.clip(rf(bx - 6.0, top, bw + 12.0, bottom - top));
        if r.items.is_empty() && !r.pending {
            self.surf.text(tr("No results"), &self.f.body, rf(bx + 8.0, top, bw, rh), self.fg(0.5 * a));
        }
        if let Some(it) = r.items.first().filter(|i| i.kind == Kind::Gemini).cloned() {
            self.draw_ai(&it, rf(bx - 4.0, top, bw + 8.0, bottom - top), a, true);
            self.surf.unclip();
            return;
        }
        if let Some(it) = r.items.first().filter(|i| i.kind == Kind::QrCode) {
            if let Some(m) = it.qr.clone() {
                let it = it.clone();
                self.draw_qr_row(&m, &it, rf(bx - 4.0, top, bw + 8.0, bottom - top), a, true, 0);
            }
            self.surf.unclip();
            return;
        }
        for (k, it) in r.items.iter().enumerate().skip(first).take(vis + 1) {
            let row = k - first;
            let ti = ((t * 7.0) - row as f32 * 0.55).clamp(0.0, 1.0);
            let ti = 1.0 - (1.0 - ti) * (1.0 - ti);
            if ti < 1.0 {
                self.keep_anim = true;
            }
            let ra = a * ti;
            let rr = rf(bx - 4.0, top + row as f32 * rh - 6.0 * (1.0 - ti), bw + 8.0, rh - 2.0);
            if k == sel {
                self.surf.fill_rr(rr, 9.0, with_alpha(accent, 0.22 * ra));
            } else if self.hot == Some(Hit::SrchItem(k)) {
                self.surf.fill_rr(rr, 9.0, self.fg(0.07 * ra));
            }
            let ir = rf(rr.left + 8.0, rr.top + (rh - 2.0 - 20.0) / 2.0, 20.0, 20.0);
            self.result_icon(it, ir, ra, &mut budget);
            let nx = ir.right + 10.0;
            let name_w = (g.measure(&it.name, &self.f.srch_name) + 4.0).min(bw * 0.55);
            if it.kind == Kind::Thinking {
                self.thinking_text(&it.name, rf(nx, rr.top, name_w + 60.0, rh - 2.0), ra);
            } else {
                self.draw_match(&it.name, &r.query, rf(nx, rr.top, name_w, rh - 2.0), ra, accent);
            }
            let sub = match it.kind {
                Kind::App => tr("App").to_string(),
                _ => it.dir.clone(),
            };
            let sw = rr.right - nx - name_w - 16.0;
            if let (Some(snip), true) = (&it.snip, sw > 30.0) {
                // Inside files: the passage around the words.
                let t = super::pages::ellip(g, snip, &self.f.tiny, sw);
                let marks = Island::word_marks(&t, &r.query);
                self.surf.text_marked(&t, &self.f.tiny, rf(nx + name_w + 8.0, rr.top, sw, rh - 2.0), self.fg(0.7 * ra), with_alpha(accent, ra), &marks);
            } else if sw > 30.0 && !sub.is_empty() {
                let sub = ellipsize_middle(g, &sub, &self.f.tiny, sw);
                self.surf.text(&sub, &self.f.tiny, rf(nx + name_w + 8.0, rr.top, sw, rh - 2.0), self.fg(0.45 * ra));
            }
            if rr.bottom <= bottom + 1.0 {
                self.hits.push((Hit::SrchItem(k), rr));
            }
        }
        self.surf.unclip();
        if self.srch_more {
            self.keep_anim = true;
        }
        // Scroll position.
        let n = r.items.len();
        if n > vis {
            let vh = bottom - top;
            let th = (vh * vis as f32 / n as f32).max(16.0);
            let ty = top + (vh - th) * (first as f32 / (n - vis) as f32);
            self.surf.fill_rr(rf(bx + bw + 6.0, ty, 3.0, th), 1.5, self.fg(0.25 * a));
        }
        if r.engine != Engine::Ready && r.items.len() < vis {
            self.engine_button(bx, bottom - 26.0, bw, a);
        }
    }

    fn engine_button(&mut self, bx: f32, y: f32, bw: f32, a: f32) {
        let g = gfx::g();
        let accent = self.ic(pal::BLUE, 1.0);
        let engine = self.search.as_ref().and_then(|s| s.results.as_ref()).map(|r| r.engine.clone()).unwrap_or(Engine::Missing);
        let (glyph, label) = match engine {
            Engine::NotRunning(_) => ("\u{E768}", tr("Everything isn't running — start it to search files")),
            _ => ("\u{E896}", tr("Install Everything (free, voidtools.com) to search all your files")),
        };
        let lw = g.measure(label, &self.f.tiny) + 40.0;
        let br = rf(bx + (bw - lw) / 2.0, y, lw, 24.0);
        let hot = self.hot == Some(Hit::SrchEngine);
        self.surf.fill_rr(br, 12.0, with_alpha(accent, if hot { 0.32 } else { 0.18 } * a));
        self.surf.text(glyph, &self.f.icon_s, rf(br.left + 10.0, br.top, 16.0, 24.0), with_alpha(accent, a));
        self.surf.text(label, &self.f.tiny, rf(br.left + 30.0, br.top, lw - 34.0, 24.0), self.fg(0.85 * a));
        self.hits.push((Hit::SrchEngine, br));
    }
}

impl Island {
    /// A result's icon: the file / app icon, or a badge for answers.
    pub(super) fn result_icon(&mut self, it: &search::Item, ir: D2D_RECT_F, ra: f32, budget: &mut u32) {
        let badge = |isl: &mut Island, col: u32, text: &str, icon: bool| {
            let c = isl.ic(col, 1.0);
            isl.surf.fill_rr(ir, (ir.bottom - ir.top) * 0.28, with_alpha(c, 0.9 * ra));
            let f = if icon { &isl.f.icon } else { &isl.f.tiny_c };
            isl.surf.text(text, f, ir, Island::text_on(c, ra));
        };
        match it.kind {
            Kind::Calc => badge(self, pal::ORANGE, "=", false),
            Kind::QrText => {
                // The site's icon on a white tile, or a clean link badge.
                let key = format!("qrfav:{}", it.target);
                if !self.srch_icons.contains_key(&key) {
                    let px = ((ir.right - ir.left - 8.0) * self.scale).round() as u32;
                    let img = it.web.as_ref().and_then(|m| m.icon.as_ref()).and_then(|b| Image::from_bytes(b, px, 0.0).ok());
                    self.srch_icons.insert(key.clone(), img);
                }
                if self.srch_icons.get(&key).map(|i| i.is_some()).unwrap_or(false) {
                    let side = ir.right - ir.left;
                    self.surf.fill_rr(ir, side * 0.28, rgb(0xffffff, 0.96 * ra));
                    if let Some(Some(img)) = self.srch_icons.get(&key) {
                        self.surf.image(img, rf(ir.left + 4.0, ir.top + 4.0, side - 8.0, side - 8.0), ra);
                    }
                } else if it.target.starts_with("http") {
                    badge(self, pal::PINK, "\u{E71B}", true);
                } else {
                    badge(self, pal::PINK, "QR", false);
                }
            }
            Kind::Currency => badge(self, pal::GREEN, "$", false),
            Kind::Base64 => badge(self, pal::TEAL, "64", false),
            Kind::Translate => badge(self, pal::PURPLE, "\u{E8C1}", true),
            Kind::Dict => badge(self, pal::PURPLE, "\u{E82D}", true),
            Kind::OcrText => badge(self, pal::PINK, "Aa", false),
            Kind::Web => badge(self, pal::BLUE, "\u{E774}", true),
            Kind::WebAction if it.target.starts_with("ai ") => self.gemini_tile(ir, ra),
            Kind::WebAction => badge(self, pal::BLUE, "\u{E721}", true),
            Kind::Gemini => self.gemini_tile(ir, ra),
            Kind::Error => badge(self, pal::RED, "!", false),
            Kind::Thinking => {
                let cx = (ir.left + ir.right) / 2.0;
                let cy = (ir.top + ir.bottom) / 2.0;
                self.thinking_dots(cx, cy, (ir.right - ir.left) / 2.0, ra);
            }
            _ => {
                self.search_icon(&it.target, budget);
                match self.srch_icons.get(&it.target).and_then(|i| i.as_ref()) {
                    Some(img) => self.surf.image(img, ir, ra),
                    None => {
                        let glyph = match it.kind {
                            Kind::App => "\u{E71D}",
                            Kind::Folder => "\u{E8B7}",
                            _ => "\u{E8A5}",
                        };
                        let f = if ir.right - ir.left > 24.0 { &self.f.icon_ctl } else { &self.f.icon };
                        self.surf.text(glyph, f, ir, self.fg(0.6 * ra));
                    }
                }
            }
        }
    }

    /// Three dots breathing in turn, like someone thinking.
    pub(crate) fn thinking_dots(&mut self, cx: f32, cy: f32, r: f32, a: f32) {
        let t = self.born.elapsed().as_secs_f32();
        let cols = [pal::BLUE, pal::PURPLE, pal::PINK];
        for i in 0..3 {
            let ph = (t * 5.0 - i as f32 * 0.7).sin() * 0.5 + 0.5;
            let x = cx + (i as f32 - 1.0) * r * 0.62;
            let y = cy - ph * r * 0.35;
            self.surf.fill_circle(x, y, r * (0.16 + 0.08 * ph), self.el(cols[i], (0.45 + 0.55 * ph) * a));
        }
        self.keep_anim = true;
    }

    /// Text with a light sweeping across it while waiting.
    pub(crate) fn thinking_text(&mut self, text: &str, r: D2D_RECT_F, a: f32) {
        let g = gfx::g();
        let f = &self.f.srch_name;
        let t = self.born.elapsed().as_secs_f32();
        let w = g.measure(text, f).max(1.0);
        let sweep = ((t * 0.8).fract() * 1.6 - 0.3) * w;
        let mut x = r.left;
        for ch in text.chars() {
            let s = ch.to_string();
            let cw = g.measure(&s, f);
            let d = ((x + cw / 2.0 - r.left) - sweep).abs() / (w * 0.25);
            let k = (1.0 - d).clamp(0.0, 1.0);
            self.surf.text(&s, f, rf(x, r.top, cw + 4.0, r.bottom - r.top), self.fg((0.4 + 0.55 * k) * a));
            x += cw;
            if x > r.right {
                break;
            }
        }
        self.keep_anim = true;
    }
}

// ---------------------------------------------------------------------------
// Gemini's answer
// ---------------------------------------------------------------------------

/// Header (sparkle, "Gemini", model), a note, the sources row.
const AI_HEAD: f32 = 34.0;
const AI_NOTE: f32 = 18.0;
const AI_SRC: f32 = 34.0;

impl Island {
    /// Gemini's sparkle: a four-pointed star, blue into violet.
    pub(crate) fn gemini_star(&mut self, cx: f32, cy: f32, r: f32, a: f32) {
        let star = |rad: f32| -> Vec<(f32, f32)> {
            (0..48)
                .map(|i| {
                    let t = i as f32 / 48.0 * std::f32::consts::TAU;
                    let (c, s) = (t.cos(), t.sin());
                    // A superellipse with p < 1: pinched sides, sharp points.
                    let p = 2.3f32;
                    (cx + rad * c.signum() * c.abs().powf(p), cy + rad * s.signum() * s.abs().powf(p))
                })
                .collect()
        };
        self.surf.fill_poly(&star(r), rgb(0x4f8df9, a));
        let inner: Vec<(f32, f32)> = star(r * 0.62).into_iter().map(|(x, y)| (x + r * 0.12, y + r * 0.12)).collect();
        self.surf.fill_poly(&inner, rgb(0x9b72cb, 0.85 * a));
    }

    /// A rounded tile with the sparkle (search rows, the guide).
    pub(crate) fn gemini_tile(&mut self, r: D2D_RECT_F, a: f32) {
        let side = r.right - r.left;
        self.surf.fill_rr(r, side * 0.28, if self.dark { rgb(0x1f2a44, 0.95 * a) } else { rgb(0xe8efff, 0.95 * a) });
        self.gemini_star((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0, side * 0.36, a);
    }

    /// Bring the typed-out answer forward (a little each frame, faster when
    /// far behind), like a chat app. Returns (thought chars, answer chars) to show.
    fn ai_reveal(&mut self, l: &crate::gemini::Live) -> (usize, usize) {
        let thoughts = crate::gemini::clean(&l.thoughts).chars().count();
        let text = l.shown().chars().count();
        let Some(s) = self.search.as_mut() else { return (thoughts, text) };
        if s.ai_id != l.id {
            s.ai_id = l.id;
            s.ai_shown = 0.0;
            s.ai_tshown = 0.0;
            s.ai_scroll = 0.0;
            s.ai_follow = true;
            s.ai_thoughts = false;
            s.ai_tick = Instant::now();
        }
        let dt = s.ai_tick.elapsed().as_secs_f32().min(0.1);
        s.ai_tick = Instant::now();
        if text > 0 {
            // Writing the answer: thoughts are complete.
            s.ai_tshown = thoughts as f32;
            let behind = text as f32 - s.ai_shown;
            s.ai_shown = (s.ai_shown + dt * (110.0 + behind.max(0.0) * 5.0)).min(text as f32);
        } else {
            let behind = thoughts as f32 - s.ai_tshown;
            s.ai_tshown = (s.ai_tshown + dt * (160.0 + behind.max(0.0) * 6.0)).min(thoughts as f32);
        }
        let (ts, sh) = (s.ai_tshown as usize, s.ai_shown as usize);
        if sh < text || ts < thoughts || !l.done {
            self.keep_anim = true;
        }
        (ts, sh)
    }

    /// What the answer's body holds: (thinking line, thoughts shown, answer shown).
    fn ai_parts(&self, l: &crate::gemini::Live, ts: usize, sh: usize) -> (bool, String, String) {
        let open = self.search.as_ref().map(|s| s.ai_thoughts).unwrap_or(false);
        let thoughts: String = crate::gemini::clean(&l.thoughts).chars().take(ts).collect();
        let text: String = l.shown().chars().take(sh).collect();
        let thinking_line = !l.thoughts.is_empty() || (l.text.is_empty() && !l.done);
        let show_thoughts = if l.text.is_empty() { thoughts } else if open { thoughts } else { String::new() };
        (thinking_line, show_thoughts, text)
    }

    /// Height of the body (thinking line, thoughts, answer, error) at text width `tw`.
    fn ai_body_h(&self, l: &crate::gemini::Live, ts: usize, sh: usize, tw: f32) -> f32 {
        let g = gfx::g();
        let (line, thoughts, text) = self.ai_parts(l, ts, sh);
        let mut h = if line { 24.0 } else { 0.0 };
        if !thoughts.is_empty() {
            h += g.measure_h(&thoughts, &self.f.wrap_src, tw - 14.0) + 8.0;
        }
        if !text.is_empty() {
            h += g.measure_h(&text, &self.f.ai_body, tw);
        }
        if !l.error.is_empty() {
            h += g.measure_h(&l.error, &self.f.wrap_src, tw) + 8.0;
        }
        h.max(20.0)
    }

    fn ai_text_w(w: f32) -> f32 {
        w - 28.0
    }

    /// How tall the answer wants to be at width `w` (popup).
    pub(super) fn ai_height(&self, _it: &search::Item, w: f32) -> f32 {
        let Some(l) = crate::gemini::live(crate::gemini::ASK) else { return 120.0 };
        let (ts, sh) = self.search.as_ref().map(|s| (s.ai_tshown as usize, s.ai_shown as usize)).unwrap_or((0, 0));
        let body = self.ai_body_h(&l, ts, sh, Self::ai_text_w(w));
        let note = if l.note.is_empty() { 0.0 } else { AI_NOTE + 4.0 };
        let src = if l.sources.is_empty() { 0.0 } else { AI_SRC };
        (AI_HEAD + note + body + 12.0 + src + 4.0).clamp(110.0, WEB_ROW_H * WEB_ROWS as f32)
    }

    /// Gemini's answer: model chips, its thinking, the text as it's written
    /// (scrolls when long), and the pages it used.
    pub(super) fn draw_ai(&mut self, _it: &search::Item, r: D2D_RECT_F, a: f32, compact: bool) {
        let Some(l) = crate::gemini::live(crate::gemini::ASK) else { return };
        let (ts, sh) = self.ai_reveal(&l);
        // The popup grows with the words as they're typed out.
        if !compact && self.search_popup() && (self.search_height() - self.h.target).abs() > 0.5 {
            self.retarget();
            self.keep_anim = true;
        }
        let g = gfx::g();
        let accent = self.ic(pal::BLUE, 1.0);
        let pad = if compact { 6.0 } else { 12.0 };
        let x = r.left + pad;
        let w = r.right - r.left - pad * 2.0;
        // ---- Header: sparkle, name, the three models, Copy ----
        let head = if compact { 28.0 } else { AI_HEAD };
        self.gemini_star(x + 9.0, r.top + head / 2.0, 9.0, a);
        let name_w = g.measure("Gemini", &self.f.body) + 10.0;
        self.surf.text("Gemini", &self.f.body, rf(x + 24.0, r.top, name_w, head), self.fg(0.95 * a));
        let mut cx = x + 24.0 + name_w + 4.0;
        for (i, m) in crate::gemini::MODELS.iter().enumerate() {
            let cw = g.measure(m.1, &self.f.tiny) + 18.0;
            let chr = rf(cx, r.top + (head - 22.0) / 2.0, cw, 22.0);
            let on = l.choice == i;
            let hot = self.hot == Some(Hit::AiModel(i));
            let dim = if crate::gemini::available(i) { 1.0 } else { 0.45 };
            if on {
                self.surf.fill_rr(chr, 11.0, with_alpha(accent, 0.85 * a));
            } else {
                self.surf.fill_rr(chr, 11.0, self.fg(if hot { 0.16 } else { 0.07 } * a));
            }
            let tc = if on { Island::text_on(accent, a) } else { self.fg(0.75 * dim * a) };
            self.surf.text(m.1, &self.f.tiny_c, chr, tc);
            self.hits.push((Hit::AiModel(i), chr));
            cx += cw + 4.0;
        }
        // Thinking on / off (asks again with the new setting): a little switch.
        {
            let on = config::cur().b("gemini_think");
            let label = format!("{} {}", tr("Think"), tr(if on { "On" } else { "Off" }));
            let cw = g.measure(&label, &self.f.tiny) + 46.0;
            let chr = rf(cx + 4.0, r.top + (head - 22.0) / 2.0, cw, 22.0);
            let hot = self.hot == Some(Hit::AiThink);
            let pc = self.ic(pal::PURPLE, 1.0);
            self.surf.fill_rr(chr, 11.0, if on { with_alpha(pc, if hot { 0.3 } else { 0.2 } * a) } else { self.fg(if hot { 0.14 } else { 0.07 } * a) });
            // The switch.
            let sw = rf(chr.left + 7.0, chr.top + 5.0, 22.0, 12.0);
            self.surf.fill_rr(sw, 6.0, if on { with_alpha(pc, a) } else { self.fg(0.25 * a) });
            let kx = if on { sw.right - 6.0 } else { sw.left + 6.0 };
            self.surf.fill_circle(kx, sw.top + 6.0, 4.5, rgb(0xffffff, a));
            let tc = if on { with_alpha(pc, a) } else { self.fg(0.55 * a) };
            self.surf.text(&label, &self.f.tiny, rf(chr.left + 34.0, chr.top, cw - 36.0, 22.0), tc);
            self.hits.push((Hit::AiThink, chr));
            cx = chr.right + 4.0;
        }
        let ready = !l.text.is_empty();
        let cl = tr("Copy");
        let cw = g.measure(cl, &self.f.tiny) + 34.0;
        let cr = rf(r.right - pad - cw, r.top + (head - 22.0) / 2.0, cw, 22.0);
        if ready && cr.left > cx + 4.0 {
            let hot = self.hot == Some(Hit::SrchItem(0));
            self.surf.fill_rr(cr, 11.0, with_alpha(accent, if hot { 0.3 } else { 0.16 } * a));
            self.surf.text("\u{E8C8}", &self.f.icon_s, rf(cr.left + 8.0, cr.top, 14.0, 22.0), with_alpha(accent, a));
            self.surf.text(cl, &self.f.tiny, rf(cr.left + 24.0, cr.top, cw - 26.0, 22.0), self.fg(0.85 * a));
            self.hits.push((Hit::SrchItem(0), cr));
        }
        let mut y = r.top + head;
        if !l.note.is_empty() {
            self.surf.text(&l.note, &self.f.tiny, rf(x, y - 2.0, w, AI_NOTE), self.ic(pal::ORANGE, 0.95 * a));
            y += AI_NOTE + 4.0;
        }
        // ---- Body (scrolls; follows the writing unless you scrolled up) ----
        let src_h = if l.sources.is_empty() { 0.0 } else if compact { 28.0 } else { AI_SRC };
        let body_bottom = r.bottom - src_h - 4.0;
        let tw = Self::ai_text_w(r.right - r.left).min(w - 6.0);
        let content = self.ai_body_h(&l, ts, sh, tw);
        let avail = (body_bottom - y).max(10.0);
        let max = (content - avail + 6.0).max(0.0);
        let off = self.search.as_mut().map(|s| {
            s.ai_max = max;
            if s.ai_follow {
                s.ai_scroll = max;
            }
            s.ai_scroll = s.ai_scroll.min(max);
            s.ai_scroll
        }).unwrap_or(0.0);
        let (line, thoughts, text) = self.ai_parts(&l, ts, sh);
        self.surf.clip(rf(r.left, y, r.right - r.left, avail));
        let mut yy = y - off;
        if line {
            let lr = rf(x, yy, tw, 22.0);
            if l.text.is_empty() && !l.done {
                // Still thinking: a light sweeps the words.
                let secs = l.started.elapsed().as_secs_f32();
                self.thinking_dots(x + 8.0, yy + 11.0, 8.0, a);
                self.thinking_text(&format!("{}  {:.0}s", tr("Thinking"), secs), rf(x + 22.0, yy, tw - 22.0, 22.0), a);
            } else {
                let open = self.search.as_ref().map(|s| s.ai_thoughts).unwrap_or(false);
                let secs = l.thought_secs.unwrap_or(0.0);
                let label = if l.thoughts.is_empty() { tr("Answered").to_string() } else { format!("{} {:.1}s", tr("Thought for"), secs) };
                let hot = self.hot == Some(Hit::AiThoughts);
                let lw_ = g.measure(&label, &self.f.tiny) + 30.0;
                let pr = rf(x - 4.0, yy, lw_, 22.0);
                if hot {
                    self.surf.fill_rr(pr, 11.0, self.fg(0.08 * a));
                }
                if !l.thoughts.is_empty() {
                    self.surf.text(if open { "\u{E70D}" } else { "\u{E76C}" }, &self.f.icon_s, rf(x, yy, 14.0, 22.0), self.fg(0.5 * a));
                    if yy + 22.0 > y && yy < y + avail {
                        self.hits.push((Hit::AiThoughts, pr));
                    }
                }
                self.surf.text(&label, &self.f.tiny, rf(x + 18.0, yy, lw_, 22.0), self.fg(0.5 * a));
            }
            let _ = lr;
            yy += 24.0;
        }
        if !thoughts.is_empty() {
            // The thinking, quiet and indented, with a line down its side.
            let th = g.measure_h(&thoughts, &self.f.wrap_src, tw - 14.0);
            self.surf.fill_rect(rf(x + 3.0, yy + 2.0, 1.5, th), self.fg(0.18 * a));
            self.surf.text_wrap(&thoughts, &self.f.wrap_src, rf(x + 14.0, yy, tw - 14.0, th + 30.0), self.fg(0.48 * a));
            yy += th + 8.0;
        }
        if !text.is_empty() {
            let th = g.measure_h(&text, &self.f.ai_body, tw);
            self.surf.text_wrap(&text, &self.f.ai_body, rf(x, yy, tw, th + 40.0), self.fg(0.92 * a));
            // A caret blinking at the end while it writes.
            if !l.done || sh < l.shown().chars().count() {
                let blink = (self.born.elapsed().as_secs_f32() * 3.0).sin() > -0.3;
                if blink {
                    self.surf.fill_rr(rf(x + caret_x(g, &text, &self.f.ai_body, tw), yy + th - 15.0, 7.0, 13.0), 2.0, with_alpha(accent, 0.8 * a));
                }
            }
            yy += th;
        }
        if !l.error.is_empty() {
            let eh = g.measure_h(&l.error, &self.f.wrap_src, tw);
            self.surf.text_wrap(&l.error, &self.f.wrap_src, rf(x, yy + 6.0, tw, eh + 20.0), rgb(pal::RED, 0.9 * a));
        }
        self.surf.unclip();
        if max > 0.0 {
            // Where you are in a long answer.
            let bar_h = (avail * avail / (content + 6.0)).max(18.0);
            let by = y + (avail - bar_h) * (off / max);
            self.surf.fill_rr(rf(r.right - 5.0, by, 3.0, bar_h), 1.5, self.fg(0.25 * a));
        }
        if l.sources.is_empty() {
            return;
        }
        // Chips: the pages Gemini read (click opens one).
        let sy = r.bottom - src_h + (src_h - 24.0) / 2.0;
        let mut cx = x;
        let label = if l.searched { tr("Google Search") } else { tr("Sources") };
        let lw = g.measure(label, &self.f.tiny) + 8.0;
        self.surf.text(label, &self.f.tiny, rf(cx, sy, lw, 24.0), self.fg(0.45 * a));
        cx += lw;
        for (i, src) in l.sources.iter().enumerate() {
            let label = format!("{}  {}", i + 1, src.title);
            let chw = (g.measure(&label, &self.f.tiny) + 20.0).min(200.0);
            if cx + chw > r.right - pad {
                let more = format!("+{}", l.sources.len() - i);
                self.surf.text(&more, &self.f.tiny, rf(cx, sy, 40.0, 24.0), self.fg(0.45 * a));
                break;
            }
            let chr = rf(cx, sy, chw, 24.0);
            let hot = self.hot == Some(Hit::SrchSource(i));
            self.surf.fill_rr(chr, 12.0, self.fg(if hot { 0.16 } else { 0.08 } * a));
            let shown = ellipsize_middle(g, &label, &self.f.tiny, chw - 18.0);
            self.surf.text(&shown, &self.f.tiny, rf(chr.left + 10.0, sy, chw - 14.0, 24.0), self.fg(0.8 * a));
            self.hits.push((Hit::SrchSource(i), chr));
            cx += chw + 6.0;
        }
    }

    /// A model chip on the answer: ask the same question with that model.
    pub(super) fn ai_model(&mut self, i: usize) {
        config::update(|c| c.set("gemini_model", i as i32));
        if let Some(l) = crate::gemini::live(crate::gemini::ASK) {
            if l.choice != i || l.done {
                crate::gemini::start(crate::gemini::ASK, true, &l.query, None, i, crate::app().host.0 as isize, crate::WM_SEARCH);
            }
        }
        self.retarget();
        self.render();
    }

    /// The Think chip: Gemini thinks before answering (or answers straight away).
    pub(super) fn ai_toggle_think(&mut self) {
        let on = !config::cur().b("gemini_think");
        config::update(|c| c.set("gemini_think", on as i32));
        if let Some(l) = crate::gemini::live(crate::gemini::ASK) {
            crate::gemini::start(crate::gemini::ASK, true, &l.query, None, l.choice, crate::app().host.0 as isize, crate::WM_SEARCH);
        }
        self.retarget();
        self.render();
    }

    /// "Thought for …": show or hide the thinking.
    pub(super) fn ai_toggle_thoughts(&mut self) {
        if let Some(s) = self.search.as_mut() {
            s.ai_thoughts = !s.ai_thoughts;
            s.ai_follow = false;
        }
        self.retarget();
        self.render();
    }

    /// A source chip: open the page Gemini used.
    pub(super) fn open_source(&mut self, i: usize) {
        let url = crate::gemini::live(crate::gemini::ASK).and_then(|l| l.sources.get(i).map(|s| s.url.clone()));
        if let Some(u) = url {
            util::shell_open(&u);
            let in_page = self.search.as_ref().map(|s| s.in_page).unwrap_or(false);
            self.close_search();
            if in_page {
                self.set_expanded(false);
            }
        }
    }
}

/// Where the last line of wrapped `text` ends (for the writing caret).
pub(super) fn caret_x(g: &gfx::Gfx, text: &str, f: &IDWriteTextFormat, width: f32) -> f32 {
    let last = text.rsplit('\n').next().unwrap_or(text);
    // Width of the last wrapped line: take words from the end until it wraps.
    let total_h = g.measure_h(last, f, width);
    let one = g.measure_h("Ag", f, width);
    if total_h <= one * 1.2 {
        return g.measure(last, f).min(width - 8.0) + 3.0;
    }
    let words: Vec<&str> = last.split(' ').collect();
    let mut tail = String::new();
    for wd in words.iter().rev() {
        let t = if tail.is_empty() { wd.to_string() } else { format!("{wd} {tail}") };
        if g.measure(&t, f) > width {
            break;
        }
        tail = t;
    }
    g.measure(&tail, f).min(width - 8.0) + 3.0
}

fn fmt_count(n: u32) -> String {
    match n {
        0..=999 => n.to_string(),
        1000..=999_999 => format!("{:.1}k", n as f32 / 1000.0).replace(".0k", "k"),
        _ => format!("{:.1}M", n as f32 / 1_000_000.0),
    }
}

/// "C:\Users\minh\…\Thesis" — keep the start and the end of long paths.
fn ellipsize_middle(g: &gfx::Gfx, s: &str, f: &IDWriteTextFormat, maxw: f32) -> String {
    // Lines too long now scroll sideways to show the rest (gfx `text`).
    if !s.is_empty() {
        return s.to_string();
    }
    if g.measure(s, f) <= maxw {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut keep = chars.len();
    while keep > 8 {
        keep -= 2;
        let head: String = chars[..keep / 2].iter().collect();
        let tail: String = chars[chars.len() - keep / 2..].iter().collect();
        let t = format!("{head}…{tail}");
        if g.measure(&t, f) <= maxw {
            return t;
        }
    }
    s.chars().take(8).collect()
}
