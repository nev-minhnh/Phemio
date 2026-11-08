//! Alt+Q: the selected text, translated, on the island — by Google Translate
//! or by a Gemini model, which writes the translation out as it goes (its
//! thinking first). Chips on the card switch between them. Esc or a click
//! anywhere else closes it.

use std::sync::Mutex;

use super::*;
use crate::answers::{self, Translation};

pub(crate) enum TransState {
    Reading,
    Translating,
    Done(Translation),
    /// Gemini is writing it (see `gemini::live(TRANSLATE)`).
    Gemini,
    Failed(String),
}

pub(crate) struct TransUi {
    pub seq: u32,
    pub src: String,
    pub state: TransState,
    /// Last time the card was looked at (hovered) or changed (keeps it up).
    pub seen: Instant,
    /// When the Google result arrived (it fades in from here — once).
    pub landed: Instant,
    pub copied: Option<Instant>,
    pub scroll: f32,
    /// Keep the newest words in view while Gemini writes (until you scroll up).
    pub follow: bool,
    /// How much of Gemini's thinking / translation is out (typed-out effect).
    pub shown: f32,
    pub tshown: f32,
    pub tick: Instant,
    pub live_id: u64,
    /// The window that had the keyboard before Alt+Q (Esc gives it back).
    pub prev: isize,
}

enum Msg {
    Read(String),
    Done(std::result::Result<Translation, String>),
}

static OUT: Mutex<Vec<(u32, Msg)>> = Mutex::new(Vec::new());
static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

const HEAD_H: f32 = 44.0;
const PAD_X: f32 = 22.0;

fn post(seq: u32, m: Msg) {
    if let Ok(mut o) = OUT.lock() {
        o.push((seq, m));
    }
    unsafe {
        let _ = PostMessageW(Some(crate::app().host), crate::WM_TRANSLATE, WPARAM(0), LPARAM(0));
    }
}

/// 0 = Google Translate, 1-3 = the Gemini models.
fn engine() -> usize {
    crate::gemini::translate_choice().map(|i| i + 1).unwrap_or(0)
}

impl Island {
    pub fn trans_active(&self) -> bool {
        self.trans.is_some()
    }

    /// Alt+Q: read the selection, translate it, show it.
    pub fn start_translate(&mut self) {
        if self.suppressed {
            return;
        }
        // From a picture's text: what's selected in it, or all of it — and
        // back to it when the translation closes.
        let from_doc = if self.doc_showing() { self.doc_selected().or_else(|| self.doc_plain()) } else { None };
        if from_doc.is_some() {
            self.doc_back = Some(self.srch_thumb.take());
        }
        if self.search_popup() {
            self.close_search();
        }
        if self.expanded {
            self.expanded = false;
        }
        self.peek = None;
        if self.mascot != Mascot::Off {
            self.set_mascot(Mascot::Off);
        }
        crate::gemini::stop(crate::gemini::TRANSLATE);
        let prev = unsafe { GetForegroundWindow().0 as isize };
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let now = Instant::now();
        self.trans = Some(TransUi { seq, src: String::new(), state: TransState::Reading, seen: now, landed: now, copied: None, scroll: 0.0, follow: true, shown: 0.0, tshown: 0.0, tick: now, live_id: 0, prev });
        self.tall = true;
        self.relayout();
        self.tr_a.set(1.0);
        self.retarget();
        if let Some(text) = from_doc {
            let text: String = text.trim().chars().take(8000).collect();
            post(seq, Msg::Read(text));
            unsafe { SetTimer(Some(self.hwnd), T_TRANS, 500, None) };
            self.render();
            return;
        }
        std::thread::spawn(move || {
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
            }
            let Some(text) = crate::selection::selected_text() else {
                post(seq, Msg::Done(Err(crate::i18n::tr("Select some text first, then press Alt+Q").to_string())));
                return;
            };
            let text: String = text.trim().chars().take(4000).collect();
            post(seq, Msg::Read(text));
        });
        unsafe { SetTimer(Some(self.hwnd), T_TRANS, 500, None) };
        self.render();
    }

    /// Translate the card's text with engine `e` (0 Google, 1-3 Gemini).
    fn trans_run(&mut self, e: usize) {
        let host = crate::app().host.0 as isize;
        let Some(t) = self.trans.as_mut() else { return };
        if t.src.is_empty() {
            return;
        }
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        t.seq = seq;
        t.scroll = 0.0;
        t.follow = true;
        t.copied = None;
        t.seen = Instant::now();
        crate::gemini::stop(crate::gemini::TRANSLATE);
        if e == 0 {
            t.state = TransState::Translating;
            let text = t.src.clone();
            std::thread::spawn(move || post(seq, Msg::Done(answers::translate(&text, None))));
        } else {
            t.state = TransState::Gemini;
            t.shown = 0.0;
            t.tshown = 0.0;
            crate::gemini::start(crate::gemini::TRANSLATE, false, &t.src, None, e - 1, host, crate::WM_TRANSLATE);
        }
    }

    /// An engine chip on the card.
    pub(super) fn trans_engine(&mut self, e: usize) {
        config::update(|c| c.set("translate_engine", e as i32));
        self.trans_run(e);
        self.retarget();
        self.kick();
        self.render();
    }

    pub fn close_translate(&mut self) {
        if self.trans.take().is_none() {
            self.doc_back = None;
            return;
        }
        crate::gemini::stop(crate::gemini::TRANSLATE);
        unsafe {
            let _ = KillTimer(Some(self.hwnd), T_TRANS);
        }
        self.release_focus();
        self.tr_a.set(0.0);
        self.retarget();
        self.render();
        if std::env::var("ISLET_LOG_DOC").is_ok() {
            crate::util::log(&format!("translate closed; back to the picture's text: {}", self.doc_back.is_some()));
        }
        // Came from a picture's text: back to it.
        if let Some(thumb) = self.doc_back.take() {
            self.open_search();
            if self.search_popup() {
                self.srch_thumb = thumb;
                self.edit_buf = crate::search::OCR_MARK.to_string();
                if let Some(s) = self.search.as_mut() {
                    s.searched = crate::search::OCR_MARK.to_string();
                }
                crate::search::show_doc_again();
                self.retarget();
                self.render();
            }
            return;
        }
        if !self.queue.is_empty() {
            self.next_peek();
        }
    }

    /// Esc: close, and hand the keyboard back to where you were.
    pub(super) fn close_translate_back(&mut self) {
        let prev = self.trans.as_ref().map(|t| t.prev).unwrap_or(0);
        self.close_translate();
        if prev != 0 {
            unsafe {
                let _ = SetForegroundWindow(HWND(prev as *mut _));
            }
        }
    }

    /// Worker news (and Gemini writing more).
    pub fn on_translate(&mut self) {
        let msgs: Vec<(u32, Msg)> = OUT.lock().map(|mut o| std::mem::take(&mut *o)).unwrap_or_default();
        let mut take_focus = false;
        let mut run: Option<usize> = None;
        let Some(t) = self.trans.as_mut() else { return };
        for (seq, m) in msgs {
            if seq != t.seq {
                continue;
            }
            match m {
                Msg::Read(s) => {
                    t.src = s;
                    take_focus = true;
                    run = Some(engine());
                }
                Msg::Done(Ok(tr)) => {
                    t.state = TransState::Done(tr);
                    t.landed = Instant::now();
                }
                Msg::Done(Err(e)) => t.state = TransState::Failed(e),
            }
            t.seen = Instant::now();
        }
        if matches!(t.state, TransState::Gemini) {
            t.seen = Instant::now();
        }
        if let Some(e) = run {
            self.trans_run(e);
        }
        // The card has the keyboard now: Esc closes it, so does a click elsewhere.
        if take_focus {
            self.take_focus();
        }
        self.retarget();
        self.kick();
        self.render();
    }

    /// Every half second: the card goes away a while after you stop looking.
    pub(super) fn translate_tick(&mut self) {
        let hover = self.hover;
        let Some(t) = self.trans.as_mut() else { return };
        if hover {
            t.seen = Instant::now();
        }
        let stay = match &t.state {
            TransState::Done(tr) => 7.0 + tr.text.chars().count() as f32 * 0.05,
            TransState::Gemini => match crate::gemini::live(crate::gemini::TRANSLATE) {
                Some(l) if l.done => 7.0 + l.text.chars().count() as f32 * 0.05,
                _ => 30.0,
            },
            TransState::Failed(_) => 4.0,
            _ => 30.0,
        };
        if t.seen.elapsed().as_secs_f32() > stay.min(30.0) {
            self.close_translate();
        }
    }

    fn trans_width(&self) -> f32 {
        ew() - PAD_X * 2.0
    }

    /// The typed-out amounts for Gemini's text (advanced while drawing).
    fn trans_reveal(&mut self) -> Option<(crate::gemini::Live, usize, usize)> {
        let l = crate::gemini::live(crate::gemini::TRANSLATE)?;
        let thoughts = crate::gemini::clean(&l.thoughts).chars().count();
        let text = l.shown().chars().count();
        let t = self.trans.as_mut()?;
        if t.live_id != l.id {
            t.live_id = l.id;
            t.shown = 0.0;
            t.tshown = 0.0;
            t.tick = Instant::now();
        }
        let dt = t.tick.elapsed().as_secs_f32().min(0.1);
        t.tick = Instant::now();
        if text > 0 {
            t.tshown = thoughts as f32;
            let behind = text as f32 - t.shown;
            t.shown = (t.shown + dt * (90.0 + behind.max(0.0) * 5.0)).min(text as f32);
        } else {
            let behind = thoughts as f32 - t.tshown;
            t.tshown = (t.tshown + dt * (160.0 + behind.max(0.0) * 6.0)).min(thoughts as f32);
        }
        let (ts, sh) = (t.tshown as usize, t.shown as usize);
        if sh < text || ts < thoughts || !l.done {
            self.keep_anim = true;
        }
        Some((l, ts, sh))
    }

    /// Gemini's part of the body: (thinking line, thoughts, translation so far).
    fn trans_gemini_parts(l: &crate::gemini::Live, ts: usize, sh: usize) -> (bool, String, String) {
        let text: String = l.shown().chars().take(sh).collect();
        // Its thinking isn't shown for translations (it talks about how it was
        // asked to translate): just the "thinking" line, then the translation.
        let _ = ts;
        let thoughts = String::new();
        (!l.thoughts.is_empty() || (l.text.is_empty() && !l.done), thoughts, text)
    }

    /// Everything on the card, scrolled or not.
    fn trans_full_height(&self) -> f32 {
        let Some(t) = &self.trans else { return 0.0 };
        let g = gfx::g();
        let w = self.trans_width();
        let src_h = if t.src.is_empty() { 0.0 } else { g.measure_h(&t.src, &self.f.wrap_src, w).min(34.0) + 10.0 };
        let body = match &t.state {
            TransState::Done(tr) => g.measure_h(&tr.text, &self.f.wrap_tr, w) + 8.0,
            TransState::Failed(e) => g.measure_h(e, &self.f.wrap_src, w) + 8.0,
            TransState::Gemini => match crate::gemini::live(crate::gemini::TRANSLATE) {
                Some(l) => {
                    let (line, thoughts, text) = Self::trans_gemini_parts(&l, t.tshown as usize, t.shown as usize);
                    let mut h = if line { 24.0 } else { 0.0 };
                    if !thoughts.is_empty() {
                        h += g.measure_h(&thoughts, &self.f.wrap_src, w - 14.0) + 6.0;
                    }
                    if !text.is_empty() {
                        h += g.measure_h(&text, &self.f.wrap_tr, w) + 8.0;
                    }
                    if !l.error.is_empty() {
                        h += g.measure_h(&l.error, &self.f.wrap_src, w) + 8.0;
                    }
                    h.max(44.0)
                }
                None => 44.0,
            },
            _ => 44.0,
        };
        HEAD_H + src_h + body + 16.0
    }

    /// The card's height for its current content.
    pub(super) fn trans_height(&self) -> f32 {
        self.trans_full_height().min(search_ui::SEARCH_MAX_H)
    }

    pub(super) fn trans_scroll(&mut self, d: i32) {
        let max = (self.trans_full_height() - self.trans_height()).max(0.0);
        if let Some(t) = self.trans.as_mut() {
            // Scrolling keeps the card up but doesn't replay its fade.
            t.scroll = (t.scroll - d as f32 / 120.0 * 40.0).clamp(0.0, max);
            t.follow = t.scroll >= max - 1.0;
            t.seen = Instant::now();
        }
        self.render();
    }

    /// The finished text (whichever engine wrote it).
    fn trans_text(&self) -> Option<String> {
        match &self.trans.as_ref()?.state {
            TransState::Done(tr) => Some(tr.text.clone()),
            TransState::Gemini => crate::gemini::live(crate::gemini::TRANSLATE).map(|l| l.shown()).filter(|s| !s.is_empty()),
            _ => None,
        }
    }

    pub(super) fn trans_copy(&mut self) {
        if let Some(text) = self.trans_text() {
            util::set_clipboard_text(&text);
            if let Some(t) = self.trans.as_mut() {
                t.copied = Some(Instant::now());
                t.seen = Instant::now();
            }
        }
        self.render();
    }

    pub(super) fn draw_translate(&mut self, x: f32, y: f32, w: f32, h: f32, a: f32) {
        if a <= 0.01 {
            return;
        }
        let live = if matches!(self.trans.as_ref().map(|t| &t.state), Some(TransState::Gemini)) { self.trans_reveal() } else { None };
        // The card grows with the words as they're typed out.
        if live.is_some() && (self.trans_height() - self.h.target).abs() > 0.5 {
            self.retarget();
            self.keep_anim = true;
        }
        let max = (self.trans_full_height() - h).max(0.0);
        let Some(t) = self.trans.as_mut() else { return };
        if t.follow {
            t.scroll = max;
        }
        t.scroll = t.scroll.min(max);
        let landed = t.landed;
        let (src, scroll, copied) = (t.src.clone(), t.scroll, t.copied.map(|c| c.elapsed().as_secs_f32() < 1.6).unwrap_or(false));
        let state = match &t.state {
            TransState::Reading => 0,
            TransState::Translating => 1,
            TransState::Done(_) => 2,
            TransState::Failed(_) => 3,
            TransState::Gemini => 4,
        };
        let done = if let TransState::Done(tr) = &t.state { Some(tr.clone()) } else { None };
        let err = if let TransState::Failed(e) = &t.state { Some(e.clone()) } else { None };
        let g = gfx::g();
        let purple = self.ic(pal::PURPLE, 1.0);
        let accent = self.ic(pal::BLUE, 1.0);
        let tx = x + PAD_X;
        let tw = w - PAD_X * 2.0;
        // ---- Header: badge, engine chips, where it's going, copy & close ----
        let badge = rf(tx, y + 12.0, 24.0, 24.0);
        self.surf.fill_rr(badge, 7.0, with_alpha(purple, 0.9 * a));
        self.surf.text("\u{E8C1}", &self.f.icon_s, badge, Island::text_on(purple, a));
        let cur = engine();
        let mut cx = badge.right + 10.0;
        for (i, name) in crate::gemini::TRANSLATORS.iter().enumerate() {
            let cw = g.measure(name, &self.f.tiny) + 18.0;
            let r = rf(cx, y + 13.0, cw, 22.0);
            let on = i == cur;
            let hot = self.hot == Some(Hit::TrEngine(i));
            let col = if i == 0 { purple } else { accent };
            if on {
                self.surf.fill_rr(r, 11.0, with_alpha(col, 0.85 * a));
            } else {
                self.surf.fill_rr(r, 11.0, self.fg(if hot { 0.16 } else { 0.07 } * a));
            }
            if i > 0 && !on {
                self.gemini_star(r.left + 9.0, r.top + 11.0, 4.0, 0.8 * a);
            }
            let tr_ = if i > 0 && !on { rf(r.left + 8.0, r.top, cw - 6.0, 22.0) } else { r };
            self.surf.text(name, &self.f.tiny_c, tr_, if on { Island::text_on(col, a) } else { self.fg(0.75 * a) });
            self.hits.push((Hit::TrEngine(i), r));
            cx += cw + 4.0;
        }
        let ready = self.trans_text().is_some();
        let mut bx = x + w - PAD_X - 26.0;
        for (hit, glyph) in [(Hit::TrClose, "\u{E711}"), (Hit::TrCopy, if copied { "\u{E73E}" } else { "\u{E8C8}" })] {
            if hit == Hit::TrCopy && !ready {
                continue;
            }
            let r = rf(bx, y + 11.0, 26.0, 26.0);
            if self.hot == Some(hit) {
                self.surf.fill_rr(r, 8.0, self.fg(0.12 * a));
            }
            let col = if hit == Hit::TrCopy && copied { self.ic(pal::GREEN, a) } else { self.fg(0.75 * a) };
            self.surf.text(glyph, &self.f.icon_s, r, col);
            self.hits.push((hit, r));
            bx -= 32.0;
        }
        // Languages (Google tells us both; Gemini picks the direction itself).
        let room = bx + 26.0 - cx - 8.0;
        let langs = match (&done, state) {
            (Some(tr), _) => format!("{} → {}", answers::lang_label(&tr.from), answers::lang_label(&tr.to)),
            (_, 3) => tr("Couldn't translate").to_string(),
            _ => format!("→ {}", answers::lang_label(&answers::home_lang())),
        };
        if room > 50.0 {
            let shown = if g.measure(&langs, &self.f.tiny) > room { format!("→ {}", answers::lang_label(&answers::home_lang())) } else { langs };
            let col = if state == 3 { rgb(pal::RED, a) } else { self.fg(0.5 * a) };
            self.surf.text(&shown, &self.f.tiny, rf(cx + 4.0, y + 12.0, room, 24.0), col);
        }
        // ---- Body ----
        let body_top = y + HEAD_H;
        self.surf.clip(rf(x, body_top, w, (y + h - body_top - 6.0).max(0.0)));
        let mut yy = body_top - scroll;
        if !src.is_empty() {
            let sh = g.measure_h(&src, &self.f.wrap_src, tw).min(34.0);
            self.surf.clip(rf(tx, yy, tw, sh));
            self.surf.text_wrap(&src, &self.f.wrap_src, rf(tx, yy, tw, sh + 20.0), self.fg(0.45 * a));
            self.surf.unclip();
            yy += sh + 4.0;
            self.surf.fill_rect(rf(tx, yy + 2.0, 28.0, 1.5), with_alpha(purple, 0.6 * a));
            yy += 8.0;
        }
        match (state, done, err) {
            (2, Some(tr), _) => {
                let th = g.measure_h(&tr.text, &self.f.wrap_tr, tw);
                // Fades in as it lands (once — hovering or scrolling doesn't replay it).
                let k = landed.elapsed().as_secs_f32().min(0.3) / 0.3;
                if k < 1.0 {
                    self.keep_anim = true;
                }
                self.surf.text_wrap(&tr.text, &self.f.wrap_tr, rf(tx, yy + 6.0 * (1.0 - k), tw, th + 10.0), self.fg(0.96 * a * k));
            }
            (3, _, Some(e)) => {
                self.surf.text_wrap(&e, &self.f.wrap_src, rf(tx, yy, tw, 40.0), self.fg(0.65 * a));
            }
            (4, ..) if live.is_some() => {
                let (l, ts, sh) = live.unwrap();
                let (line, thoughts, text) = Self::trans_gemini_parts(&l, ts, sh);
                if line {
                    if l.text.is_empty() && !l.done {
                        self.thinking_dots(tx + 8.0, yy + 11.0, 8.0, a);
                        let label = format!("{} · {}  {:.0}s", crate::gemini::short_name(&l.model), tr("Thinking"), l.started.elapsed().as_secs_f32());
                        self.thinking_text(&label, rf(tx + 22.0, yy, tw - 22.0, 22.0), a);
                    } else {
                        let label = match l.thought_secs {
                            Some(s) if !l.thoughts.is_empty() => format!("{} · {} {:.1}s", crate::gemini::short_name(&l.model), tr("Thought for"), s),
                            _ => crate::gemini::short_name(&l.model),
                        };
                        self.gemini_star(tx + 6.0, yy + 11.0, 5.0, a);
                        self.surf.text(&label, &self.f.tiny, rf(tx + 16.0, yy, tw - 16.0, 22.0), self.fg(0.5 * a));
                    }
                    yy += 24.0;
                }
                if !thoughts.is_empty() {
                    let th = g.measure_h(&thoughts, &self.f.wrap_src, tw - 14.0);
                    self.surf.fill_rect(rf(tx + 3.0, yy + 2.0, 1.5, th), self.fg(0.18 * a));
                    self.surf.text_wrap(&thoughts, &self.f.wrap_src, rf(tx + 14.0, yy, tw - 14.0, th + 30.0), self.fg(0.48 * a));
                    yy += th + 6.0;
                }
                if !text.is_empty() {
                    let th = g.measure_h(&text, &self.f.wrap_tr, tw);
                    self.surf.text_wrap(&text, &self.f.wrap_tr, rf(tx, yy, tw, th + 30.0), self.fg(0.96 * a));
                    if !l.done || sh < l.shown().chars().count() {
                        if (self.born.elapsed().as_secs_f32() * 3.0).sin() > -0.3 {
                            self.surf.fill_rr(rf(tx + search_ui::caret_x(g, &text, &self.f.wrap_tr, tw), yy + th - 17.0, 7.0, 15.0), 2.0, with_alpha(accent, 0.8 * a));
                        }
                    }
                    yy += th + 8.0;
                }
                if !l.error.is_empty() {
                    self.surf.text_wrap(&l.error, &self.f.wrap_src, rf(tx, yy, tw, 60.0), rgb(pal::RED, 0.9 * a));
                }
            }
            _ => {
                // Thinking…
                self.thinking_dots(tx + 22.0, yy + 18.0, 18.0, a);
                // Placeholder lines shimmering where the text will be.
                let ph = self.born.elapsed().as_secs_f32();
                for i in 0..2 {
                    let lw = tw * if i == 0 { 0.62 } else { 0.38 } - 60.0;
                    let pulse = ((ph * 3.0 - i as f32 * 0.6).sin() * 0.5 + 0.5) * 0.08 + 0.06;
                    self.surf.fill_rr(rf(tx + 56.0, yy + 8.0 + i as f32 * 16.0, lw, 9.0), 4.5, self.fg(pulse * a));
                }
            }
        }
        self.surf.unclip();
        if max > 0.0 {
            let vh = h - HEAD_H - 6.0;
            let bar_h = (vh * vh / (vh + max)).max(18.0);
            let by = body_top + (vh - bar_h) * (scroll / max);
            self.surf.fill_rr(rf(x + w - 8.0, by, 3.0, bar_h), 1.5, self.fg(0.25 * a));
        }
    }
}
