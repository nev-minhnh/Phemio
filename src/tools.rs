//! Focus timer and to-do list. Plain data + small text files in
//! %APPDATA%\Phemio. Nothing here ticks on its own; the host schedules timers.

use std::time::{Duration, Instant};

use crate::config::{self, Config};
use crate::util;

// ---------------------------------------------------------------------------
// Focus (Pomodoro)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Phase {
    Focus,
    Short,
    Long,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::Focus => "Focus",
            Phase::Short => "Short break",
            Phase::Long => "Long break",
        }
    }
}

pub struct Focus {
    pub phase: Phase,
    pub running: bool,
    end: Option<Instant>,
    left: Duration,
    /// Short breaks taken in the current cycle; after `long_every` of them
    /// the next break is a long one.
    pub cycle: u32,
    started: bool,
    pub today: u32,
    /// Long breaks reached today.
    pub long_today: u32,
    today_day: i64,
}

impl Focus {
    pub fn new() -> Focus {
        let c = config::cur();
        Focus { phase: Phase::Focus, running: false, end: None, left: Self::len(&c, Phase::Focus), cycle: 0, started: false, today: 0, long_today: 0, today_day: 0 }
    }

    /// Minutes setting of a phase.
    pub fn key(p: Phase) -> &'static str {
        match p {
            Phase::Focus => "focus_min",
            Phase::Short => "break_min",
            Phase::Long => "long_min",
        }
    }

    pub fn minutes(p: Phase) -> i32 {
        config::cur().i(Self::key(p)).clamp(1, 180)
    }

    /// Pick a phase by hand (paused, full length).
    pub fn set_phase(&mut self, p: Phase) {
        self.phase = p;
        self.running = false;
        self.started = false;
        self.end = None;
        self.left = Self::len(&config::cur(), p);
    }

    fn new_day(&mut self) {
        let day = util::local_day(util::unix_now());
        if day != self.today_day {
            self.today_day = day;
            self.today = 0;
            self.long_today = 0;
        }
    }

    fn len(c: &Config, p: Phase) -> Duration {
        let m = match p {
            Phase::Focus => c.i("focus_min"),
            Phase::Short => c.i("break_min"),
            Phase::Long => c.i("long_min"),
        };
        Duration::from_secs(m.clamp(1, 180) as u64 * 60)
    }

    pub fn total(&self) -> Duration {
        Self::len(&config::cur(), self.phase)
    }

    pub fn remaining(&self) -> Duration {
        match (self.running, self.end) {
            (true, Some(e)) => e.saturating_duration_since(Instant::now()),
            _ => self.left,
        }
    }

    /// 0 → 1 as the phase elapses.
    pub fn progress(&self) -> f32 {
        let t = self.total().as_secs_f32().max(1.0);
        (1.0 - self.remaining().as_secs_f32() / t).clamp(0.0, 1.0)
    }

    /// Something to show in the closed notch?
    pub fn active(&self) -> bool {
        self.running || self.started
    }

    pub fn toggle(&mut self) {
        if self.running {
            self.left = self.remaining();
            self.running = false;
            self.end = None;
        } else {
            self.end = Some(Instant::now() + self.left);
            self.running = true;
            self.started = true;
        }
    }

    pub fn reset(&mut self) {
        self.started = false;
        self.running = false;
        self.end = None;
        self.left = self.total();
    }

    /// Jump to the next phase (paused).
    pub fn skip(&mut self) {
        self.advance(false);
    }

    fn advance(&mut self, completed: bool) {
        let c = config::cur();
        self.new_day();
        let every = c.i("long_every").clamp(2, 12) as u32;
        match self.phase {
            Phase::Focus => {
                if completed {
                    self.today += 1;
                }
                // `every` short breaks, then one long break.
                if self.cycle >= every {
                    self.phase = Phase::Long;
                    self.long_today += 1;
                } else {
                    self.phase = Phase::Short;
                    self.cycle += 1;
                }
            }
            Phase::Long => {
                self.cycle = 0;
                self.phase = Phase::Focus;
            }
            Phase::Short => self.phase = Phase::Focus,
        }
        self.running = false;
        self.started = false;
        self.end = None;
        self.left = Self::len(&c, self.phase);
    }

    /// Called when the phase timer fires. Returns the phase that just ended.
    pub fn on_timer(&mut self) -> Option<Phase> {
        if self.running && self.remaining().as_millis() < 400 {
            let ended = self.phase;
            self.advance(true);
            // Breaks start on their own; the next focus waits for you.
            if self.phase != Phase::Focus {
                self.toggle();
            }
            Some(ended)
        } else {
            None
        }
    }

    /// Settings changed while idle → pick up the new length.
    pub fn refresh_len(&mut self) {
        if !self.started {
            self.left = self.total();
        }
    }
}

// ---------------------------------------------------------------------------
// To-do
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Task {
    pub text: String,
    pub done: bool,
    pub due: Option<i64>,
    pub notified: bool,
}

pub struct Todo {
    pub tasks: Vec<Task>,
}

impl Todo {
    pub fn load() -> Todo {
        let mut tasks = Vec::new();
        if let Ok(t) = std::fs::read_to_string(config::data_file("todo.txt")) {
            for l in t.lines() {
                let p: Vec<&str> = l.splitn(4, '\t').collect();
                if p.len() == 4 {
                    tasks.push(Task { done: p[0] == "1", due: p[1].parse().ok(), notified: p[2] == "1", text: p[3].to_string() });
                }
            }
        }
        Todo { tasks }
    }

    pub fn save(&self) {
        let s: String = self
            .tasks
            .iter()
            .map(|t| format!("{}\t{}\t{}\t{}\n", t.done as u8, t.due.map(|d| d.to_string()).unwrap_or_default(), t.notified as u8, t.text))
            .collect();
        let _ = std::fs::write(config::data_file("todo.txt"), s);
    }

    /// "Call mum @18:30" → task with a reminder today (or tomorrow if past).
    pub fn add(&mut self, input: &str) {
        let input = input.trim();
        if input.is_empty() {
            return;
        }
        let (text, due) = match input.rfind('@') {
            Some(i) => match util::parse_hm(&input[i + 1..]) {
                Some((h, m)) => {
                    let mut t = util::today_at(h, m);
                    if t <= util::unix_now() {
                        t += 86400;
                    }
                    (input[..i].trim().to_string(), Some(t))
                }
                None => (input.to_string(), None),
            },
            None => (input.to_string(), None),
        };
        let text = if text.is_empty() { input.to_string() } else { text };
        self.tasks.insert(0, Task { text, done: false, due, notified: false });
        self.save();
    }

    pub fn toggle(&mut self, i: usize) {
        if let Some(t) = self.tasks.get_mut(i) {
            t.done = !t.done;
        }
        // Done tasks sink to the bottom.
        self.tasks.sort_by_key(|t| t.done);
        self.save();
    }

    pub fn remove(&mut self, i: usize) {
        if i < self.tasks.len() {
            self.tasks.remove(i);
            self.save();
        }
    }

    pub fn snooze(&mut self, text: &str, mins: i64) {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.text == text) {
            t.due = Some(util::unix_now() + mins * 60);
            t.notified = false;
        }
        self.save();
    }

    /// Tasks whose reminder time has come (marked as notified).
    pub fn take_due(&mut self) -> Vec<String> {
        let now = util::unix_now();
        let mut out = Vec::new();
        for t in &mut self.tasks {
            if !t.done && !t.notified && t.due.map(|d| d <= now + 20).unwrap_or(false) {
                t.notified = true;
                out.push(t.text.clone());
            }
        }
        if !out.is_empty() {
            self.save();
        }
        out
    }

    pub fn open_count(&self) -> usize {
        self.tasks.iter().filter(|t| !t.done).count()
    }
}

// ---------------------------------------------------------------------------
// Notes (beside the to-do list): sticky notes with a colour and simple
// formatting. Formatting lives in the text as invisible marks that switch a
// style on / off from that point (what the toolbar buttons insert). A list
// line is "<2 spaces per level><BULLET or NUMBER> text"; how it shows
// (•, ◦, ▪ / 1., a., i.) follows from its level and the lines around it.
// ---------------------------------------------------------------------------

/// Invisible marks: bold, italic, underline, strikethrough.
pub const MARKS: [char; 4] = ['\u{E001}', '\u{E002}', '\u{E003}', '\u{E004}'];
/// List items: bulleted and numbered.
pub const BULLET: char = '\u{E005}';
pub const NUMBER: char = '\u{E006}';
/// Deepest list level.
pub const MAX_LEVEL: usize = 6;

/// A line's list shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListLine {
    /// Indent level (2 spaces each).
    pub level: usize,
    /// 0 plain, 1 bullet, 2 numbered.
    pub kind: u8,
    /// Byte offset (in the line) of the marker, if any.
    pub marker: Option<usize>,
    /// Byte offset where the item's own text starts.
    pub content: usize,
}

/// Read a line's indent and list marker (style marks in front are skipped).
pub fn list_line(line: &str) -> ListLine {
    let mut spaces = 0;
    for (i, ch) in line.char_indices() {
        if ch == ' ' {
            spaces += 1;
        } else if MARKS.contains(&ch) {
            continue;
        } else if ch == BULLET || ch == NUMBER {
            let after = i + ch.len_utf8();
            let content = if line[after..].starts_with(' ') { after + 1 } else { after };
            return ListLine { level: (spaces / 2).min(MAX_LEVEL), kind: if ch == BULLET { 1 } else { 2 }, marker: Some(i), content };
        } else {
            return ListLine { level: spaces / 2, kind: 0, marker: None, content: i };
        }
    }
    ListLine { level: spaces / 2, kind: 0, marker: None, content: line.len() }
}

fn alpha_label(mut n: usize) -> String {
    let mut s = Vec::new();
    while n > 0 {
        n -= 1;
        s.push((b'a' + (n % 26) as u8) as char);
        n /= 26;
    }
    s.iter().rev().collect()
}

fn roman_label(mut n: usize) -> String {
    let table = [(1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"), (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")];
    let mut out = String::new();
    for (v, r) in table {
        while n >= v {
            out.push_str(r);
            n -= v;
        }
    }
    out
}

/// The text with list markers turned into what they show: bullets by level
/// (•, ◦, ▪) and numbers by level (1., a., i.), counted per list.
pub fn render_lists(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut counters: Vec<usize> = Vec::new();
    for (n, line) in text.split('\n').enumerate() {
        if n > 0 {
            out.push('\n');
        }
        let l = list_line(line);
        match (l.kind, l.marker) {
            (k, Some(m)) if k != 0 => {
                counters.truncate(l.level + 1);
                while counters.len() <= l.level {
                    counters.push(0);
                }
                let label = if k == 1 {
                    counters[l.level] = 0;
                    ["\u{2022}", "\u{25E6}", "\u{25AA}"][l.level % 3].to_string()
                } else {
                    counters[l.level] += 1;
                    let c = counters[l.level];
                    match l.level % 3 {
                        0 => format!("{c}."),
                        1 => format!("{}.", alpha_label(c)),
                        _ => format!("{}.", roman_label(c)),
                    }
                };
                out.push_str(&line[..m]);
                out.push_str(&label);
                out.push(' ');
                out.push_str(&line[l.content..]);
            }
            _ => {
                // Any other line ends the lists above it.
                counters.clear();
                out.push_str(line);
            }
        }
    }
    out
}

/// Note colours (like Windows Sticky Notes): yellow, green, pink, purple, blue, grey.
pub const NOTE_COLORS: [u32; 6] = [0xffd84d, 0x8fd694, 0xff8fb8, 0xc2a3ff, 0x7ec4ff, 0xb0b0b8];

#[derive(Clone, Debug)]
pub struct Note {
    pub id: u64,
    pub text: String,
    pub color: u8,
    pub updated: i64,
}

impl Note {
    /// The text as shown (no marks), and its styled stretches in UTF-16
    /// units: (start, length, bold, italic, underline, strikethrough).
    pub fn display(text: &str) -> (String, Vec<(u32, u32, bool, bool, bool, bool)>) {
        let text = &render_lists(text);
        let mut out = String::new();
        let mut on = [false; 4];
        let mut runs = Vec::new();
        let mut pos = 0u32;
        let mut run_start = 0u32;
        let mut cur = on;
        for ch in text.chars() {
            if let Some(k) = MARKS.iter().position(|m| *m == ch) {
                if pos > run_start && cur.iter().any(|x| *x) {
                    runs.push((run_start, pos - run_start, cur[0], cur[1], cur[2], cur[3]));
                }
                on[k] = !on[k];
                cur = on;
                run_start = pos;
                continue;
            }
            out.push(ch);
            pos += ch.len_utf16() as u32;
        }
        if pos > run_start && cur.iter().any(|x| *x) {
            runs.push((run_start, pos - run_start, cur[0], cur[1], cur[2], cur[3]));
        }
        (out, runs)
    }

    /// Which styles are on at the end of the text (where you type).
    pub fn styles_at_end(text: &str) -> [bool; 4] {
        let mut on = [false; 4];
        for ch in text.chars() {
            if let Some(k) = MARKS.iter().position(|m| *m == ch) {
                on[k] = !on[k];
            }
        }
        on
    }

    /// First line without marks or list markers (for the list).
    pub fn title(&self) -> String {
        self.text
            .split('\n')
            .map(|l| l[list_line(l).content..].chars().filter(|c| !MARKS.contains(c)).collect::<String>().trim().to_string())
            .find(|l| !l.is_empty())
            .unwrap_or_default()
    }
}

pub struct Notes {
    pub notes: Vec<Note>,
}

impl Notes {
    pub fn load() -> Notes {
        let mut notes = Vec::new();
        if let Ok(b) = std::fs::read(config::data_file("notes.json")) {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&b) {
                for n in v.as_array().cloned().unwrap_or_default() {
                    notes.push(Note {
                        id: n["id"].as_u64().unwrap_or(0),
                        // Older notes: "• " bullets become list items.
                        text: n["text"].as_str().unwrap_or("").split('\n').map(|l| match l.strip_prefix("• ") { Some(r) => format!("{BULLET} {r}"), None => l.to_string() }).collect::<Vec<_>>().join("\n"),
                        color: n["color"].as_u64().unwrap_or(0).min(5) as u8,
                        updated: n["updated"].as_i64().unwrap_or(0),
                    });
                }
            }
        }
        Notes { notes }
    }

    pub fn save(&self) {
        let v: Vec<serde_json::Value> = self.notes.iter().map(|n| serde_json::json!({"id": n.id, "text": n.text, "color": n.color, "updated": n.updated})).collect();
        let p = config::data_file("notes.json");
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, serde_json::to_vec_pretty(&v).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }

    /// A new empty note (first in the list); its id.
    pub fn add(&mut self, color: u8) -> u64 {
        let id = (crate::util::unix_now() as u64) << 8 | (self.notes.len() as u64 & 0xff);
        self.notes.insert(0, Note { id, text: String::new(), color, updated: crate::util::unix_now() });
        self.save();
        id
    }

    pub fn get(&self, id: u64) -> Option<&Note> {
        self.notes.iter().find(|n| n.id == id)
    }

    pub fn set_text(&mut self, id: u64, text: &str) {
        if let Some(n) = self.notes.iter_mut().find(|n| n.id == id) {
            if n.text != text {
                n.text = text.to_string();
                n.updated = crate::util::unix_now();
                self.save();
            }
        }
    }

    pub fn set_color(&mut self, id: u64, c: u8) {
        if let Some(n) = self.notes.iter_mut().find(|n| n.id == id) {
            n.color = c.min(5);
            self.save();
        }
    }

    pub fn remove(&mut self, id: u64) {
        self.notes.retain(|n| n.id != id);
        self.save();
    }
}

#[cfg(test)]
mod list_tests {
    use super::*;
    #[test]
    fn numbering() {
        let t = format!("{N} one\n  {N} sub\n  {N} sub2\n    {B} deep\n{N} two\nplain\n{N} again", N = NUMBER, B = BULLET);
        assert_eq!(render_lists(&t), "1. one\n  a. sub\n  b. sub2\n    \u{25AA} deep\n2. two\nplain\n1. again");
        assert_eq!(roman_label(4), "iv");
        assert_eq!(alpha_label(27), "aa");
    }
}
