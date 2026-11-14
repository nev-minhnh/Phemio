//! The Dynamic Island / notch window.
//!
//! One layered window at the top of the primary monitor. The shape is drawn
//! with Direct2D and animated with springs between three states:
//! collapsed (clock / focus / media status, or the idle mascot), peek
//! (alerts) and expanded (pages you flip through with the arrows, the dots,
//! the mouse wheel or ←/→).

mod pages;
mod search_ui;
mod doc_ui;
mod translate_ui;
mod link_ui;
mod rdp_ui;
mod mascot;
pub(crate) use translate_ui::TransUi;
pub(crate) use search_ui::SearchUi;

use std::collections::VecDeque;
use std::time::Instant;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::anim::{clamp01, Spring};
use crate::config;
pub(crate) use crate::i18n::tr;
use crate::gfx::{self, inside, rf, rgb, with_alpha, Image, Surface, DISPLAY_FONT, ICON_FONT, UI_FONT};
use crate::media::{self, Cmd, MediaInfo};
use crate::util::{self, Monitor};

const CLASS: PCWSTR = w!("PhemioIsland");

const T_ANIM: usize = 1;
const T_CLOCK: usize = 2;
const T_PEEK: usize = 3;
const T_HOVER: usize = 4;
const T_COLLAPSE: usize = 5;
const T_EQ: usize = 6;
const T_SECOND: usize = 7;
const T_CARET: usize = 8;
const T_BLINK: usize = 10;
const T_BLINK2: usize = 11;
const T_ONTOP: usize = 12;
const T_BRIGHT_UI: usize = 13;
const T_PEEK_SWAP: usize = 15;
const T_CTL_RESCAN: usize = 16;
const T_SEARCH: usize = 17;
const T_TRANS: usize = 18;
const T_BYE: usize = 19;
/// Every 2 s: make sure the closed island isn't left blank (see `heal`).
const T_HEAL: usize = 21;
/// Taskbar watch: a check every 0.3 s, and "it just moved" from the event hook.
const T_TBPOS: usize = 40;
const T_TBNOW: usize = 41;
/// A long line scrolling sideways wants its next frame.
const T_MARQUEE: usize = 42;
/// A swipe's fingers have lifted (no more events for a moment).
const T_SWIPE: usize = 43;

/// The island's window and the taskbar being watched (for the event hook,
/// which has no other way in).
static TB_ISLAND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static TB_WATCHED: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// The taskbar moved or changed size (dragged to another edge, auto-hide
/// sliding in / out, resized): have the island look again right away.
/// The island's window (developer aids).
pub fn island_hwnd() -> isize {
    TB_ISLAND.load(std::sync::atomic::Ordering::Relaxed)
}

unsafe extern "system" fn on_taskbar_moved(_h: windows::Win32::UI::Accessibility::HWINEVENTHOOK, _ev: u32, hwnd: HWND, id_object: i32, _child: i32, _thread: u32, _time: u32) {
    use std::sync::atomic::Ordering::Relaxed;
    if id_object == 0 && hwnd.0 as isize == TB_WATCHED.load(Relaxed) {
        let island = TB_ISLAND.load(Relaxed);
        if island != 0 {
            let _ = SetTimer(Some(HWND(island as *mut _)), T_TBNOW, 15, None);
        }
    }
}
/// How long the goodbye lasts before the mascot fades out (ms).
const BYE_MS: u32 = 1500;
/// While more popups are waiting, each one shows this long.
const BURST_MS: u32 = 200;

/// The page area at the standard island size (600 × 260), which pages are laid out for.
const PAGE_W: f32 = 600.0 - 88.0;
const PAGE_H: f32 = 260.0 - 62.0;

const PAD: f32 = 22.0; // room around the shape for shadow and notch "ears"
pub(crate) use crate::palette as pal;

/// The page dots at the bottom of the opened island (Settings → Island)?
pub(crate) fn dots_bottom() -> bool {
    config::cur().i("dots_pos") == 1
}

/// The page dock above the dots (Settings → Island → Page dock position)?
pub(crate) fn dock_above() -> bool {
    config::cur().i("pdock_pos") == 0
}

/// Room the island grows by for the page dock when it opens beyond the dots
/// at the island's edge (dots at the top with the dock above them, or at the
/// bottom with the dock below): Some(true) at the top, Some(false) at the
/// bottom; None when the dock opens over the page.
pub(crate) fn dock_extends() -> Option<bool> {
    match (dots_bottom(), dock_above()) {
        (false, true) => Some(true),
        (true, false) => Some(false),
        _ => None,
    }
}
/// Most the island ever grows for the dock (its window is made this much
/// taller in those layouts).
pub(crate) const DOCK_EXT: f32 = 76.0;

/// The page dock's sizes for `n` pages: (icon, capsule height, how much a
/// pointed-at icon swells).
pub(crate) fn dock_dims(n: usize) -> (f32, f32, f32) {
    let room = ew() - 60.0;
    let base = ((room - 24.0) / n.max(1) as f32 - 8.0).clamp(20.0, 28.0);
    (base, 7.0 * 2.0 + base, base * 0.6)
}

/// How much the island grows for the dock: the capsule and a thin strip on
/// the outside for the pointed-at page's name (just enough).
fn dock_room() -> f32 {
    let (_, bh, _) = dock_dims(enabled_pages().len());
    match dock_extends() {
        Some(true) => bh + 18.0,
        Some(false) => bh + 31.0,
        None => 0.0,
    }
}

/// Where the page dots' line runs (its middle), on the island opened at `y0`.
pub(crate) fn dots_y(y0: f32) -> f32 {
    if dots_bottom() { y0 + eh() - 13.0 } else { y0 + 25.0 }
}

/// Expanded island size (Settings → Island → Expanded width / height).
pub fn ew() -> f32 {
    config::cur().i("exp_w").clamp(480, 900) as f32
}
pub fn eh() -> f32 {
    config::cur().i("exp_h").clamp(200, 400) as f32
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Home,
    Clock,
    Weather,
    Calendar,
    Focus,
    Todo,
    Shelf,
    Devices,
    Controls,
    Search,
    Link,
    Remote,
}

/// Pages with their own way in from the closed island (so the empty space
/// doesn't open them): Clock & Calendar (the time), Devices (the speaker),
/// Controls (the battery), Now Playing (the music, Alt+A).
fn own_spot(p: Page) -> bool {
    matches!(p, Page::Calendar | Page::Devices | Page::Controls | Page::Home)
}

pub const ALL_PAGES: [Page; 12] = [
    Page::Home,
    Page::Clock,
    Page::Weather,
    Page::Calendar,
    Page::Focus,
    Page::Todo,
    Page::Shelf,
    Page::Devices,
    Page::Controls,
    Page::Search,
    Page::Link,
    Page::Remote,
];

impl Page {
    pub fn key(self) -> &'static str {
        match self {
            Page::Home => "page_home",
            Page::Clock => "page_clock",
            Page::Weather => "page_weather",
            Page::Focus => "page_focus",
            Page::Todo => "page_todo",
            Page::Shelf => "page_shelf",
            Page::Devices => "page_devices",
            Page::Controls => "page_controls",
            Page::Calendar => "page_calendar",
            Page::Search => "page_search",
            Page::Link => "page_link",
            Page::Remote => "page_rdp",
        }
    }
    /// One line for Settings → Pages.
    pub fn about(self) -> &'static str {
        match self {
            Page::Home => "Any player: cover, controls, seek",
            Page::Clock => "Your time and a second time zone",
            Page::Weather => "Now and the next days for your location",
            Page::Calendar => "The time with seconds, date and your city beside this month",
            Page::Focus => "Pomodoro; countdown shows in the closed island",
            Page::Todo => "Type tasks straight into the island; @14:30 adds a reminder",
            Page::Shelf => "Drop files on the island, drag them out anywhere",
            Page::Devices => "Audio output & Bluetooth (with battery), volume, Wi-Fi, battery, energy saver",
            Page::Controls => "Wi-Fi, Bluetooth, airplane mode, keyboards, microphone, brightness, volume, Windows panels",
            Page::Search => "Apps, files (Everything), sums, currencies, base64, translation — also Alt+S anywhere",
            Page::Link => "One mouse and keyboard for your PCs: pair, arrange every screen, see the delay",
            Page::Remote => "Type a computer, user name and password: Windows' Remote Desktop connects",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Page::Home => "Now Playing",
            Page::Clock => "World Clock",
            Page::Weather => "Weather",
            Page::Focus => "Focus",
            Page::Todo => "To-do",
            Page::Shelf => "Shelf",
            Page::Devices => "Devices",
            Page::Controls => "Controls",
            Page::Calendar => "Clock & Calendar",
            Page::Search => "Search",
            Page::Link => "Links and Display",
            Page::Remote => "Remote Desktop",
        }
    }
    pub fn glyph(self) -> &'static str {
        match self {
            Page::Home => "\u{E8D6}",
            Page::Clock => "\u{E823}",
            Page::Weather => "",
            Page::Focus => "\u{E916}",
            Page::Todo => "\u{E762}",
            Page::Shelf => "\u{E8B7}",
            Page::Devices => "\u{E7F6}",
            Page::Controls => "\u{E9E9}",
            Page::Calendar => "\u{E787}",
            Page::Search => "\u{E721}",
            Page::Link => "\u{E71B}",
            Page::Remote => "\u{E8AF}",
        }
    }
    /// Palette colour of the page's icon.
    pub fn color(self) -> u32 {
        match self {
            Page::Home => pal::PINK,
            Page::Clock => pal::ORANGE,
            Page::Weather => pal::YELLOW,
            Page::Focus => pal::RED,
            Page::Todo => pal::GREEN,
            Page::Shelf => pal::PURPLE,
            Page::Devices => pal::BLUE,
            Page::Controls => pal::TEAL,
            Page::Calendar => pal::MINT,
            Page::Search => pal::BLUE,
            Page::Link => pal::INDIGO,
            Page::Remote => pal::CYAN,
        }
    }
    fn index(self) -> usize {
        ALL_PAGES.iter().position(|p| *p == self).unwrap_or(0)
    }
}

/// All pages in the user's order (Settings → Pages, ▲▼).
pub fn ordered_pages() -> Vec<Page> {
    let order = config::cur().s("page_order");
    let mut v: Vec<Page> = Vec::new();
    for k in order.split(',') {
        if let Some(p) = ALL_PAGES.iter().copied().find(|p| p.key() == k.trim() || p.key().strip_prefix("page_") == Some(k.trim())) {
            if !v.contains(&p) {
                v.push(p);
            }
        }
    }
    for p in ALL_PAGES {
        if !v.contains(&p) {
            v.push(p);
        }
    }
    v
}

/// Move a page one place earlier (-1) or later (+1).
pub fn move_page(p: Page, dir: i32) {
    let mut v = ordered_pages();
    if let Some(i) = v.iter().position(|x| *x == p) {
        let j = i as i32 + dir;
        if j >= 0 && (j as usize) < v.len() {
            v.swap(i, j as usize);
        }
    }
    let keys: Vec<&str> = v.iter().map(|p| p.key()).collect();
    config::update(|c| c.set_s("page_order", &keys.join(",")));
}

pub fn enabled_pages() -> Vec<Page> {
    let c = config::cur();
    let v: Vec<Page> = ordered_pages().into_iter().filter(|p| c.b(p.key())).collect();
    if v.is_empty() {
        vec![Page::Home]
    } else {
        v
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hit {
    Nav(i32),
    Dot(usize),
    Settings,
    Pin,
    Expand,
    Close,
    Prev,
    Play,
    Next,
    /// Now Playing: repeat off → the whole list → this song → off.
    Repeat,
    /// Now Playing: stop when this song ends (don't play the next).
    StopAfter,
    Seek,
    ClockEdit,
    WeatherEdit,
    WeatherRefresh,
    FocusToggle,
    FocusReset,
    FocusSkip,
    PowerSettings,
    FocusPhase(u8),
    FocusLen(i32),
    TodoInput,
    TodoCheck(usize),
    TodoDelete(usize),
    ShelfItem(usize),
    /// The small folder button under a shelf item: its folder, it selected.
    ShelfReveal(usize),
    ShelfRemove(usize),
    ShelfClear,
    Volume,
    Mute,
    OpenDevices,
    DevDeck,
    DevDot(usize),
    /// Controls page: tiles / buttons (see page_controls) and brightness.
    Ctl(u8),
    Bright,
    CalPrev,
    CalNext,
    /// Month name: back to this month.
    CalOpen,
    /// The month's name (opens the month / year picker), the picker's
    /// year arrows, a month in it, and the picker itself (eats clicks).
    CalTitle,
    CalPickYear(i32),
    CalPickMonth(u8),
    CalPickBg,
    /// Devices page: copy the IP address (0) or the MAC address (1).
    CopyNet(u8),
    /// To-do page: the To-do / Notes switch (true = Notes); a new note; a note
    /// in the list; the note's text; a style (0 bold, 1 italic, 2 underline,
    /// 3 strikethrough, 4 bullet); a colour; delete the note.
    TodoNotes(bool),
    NoteNew,
    NoteItem(usize),
    NoteText,
    NoteStyle(u8),
    NoteColor(u8),
    NoteDelete,
    /// Weather chart: an hour (hovered for its numbers).
    WxHour(usize),
    /// Weather: next days (false) or today hour by hour (true).
    WxView(bool),
    /// Closed island: the speaker icon (mute / unmute).
    ColMute,
    /// Closed island: the microphone (mute / unmute).
    ColMic,
    /// Devices page: open / close the sound-output list, pick an output.
    OutMenu,
    OutPick(usize),
    /// Play on several outputs: the switch, a device's tick, its delay and
    /// volume sliders, Auto-sync, Calibrate.
    MoToggle,
    MoTick(usize),
    MoDelay(usize),
    MoVol(usize),
    MoAuto,
    MoCalib,
    /// One speaker: make it the main output, line it up (estimate / mic),
    /// type its delay, nudge it ±5 ms.
    MoMain(usize),
    MoAutoOne(usize),
    MoCalibOne(usize),
    MoDelayEdit(usize),
    MoNudge(usize, i32),
    /// Controls page: one speaker of those playing together (index in the cluster).
    MoCVol(usize),
    /// The volume row's "speakers playing together" button, its card, and outside it.
    /// Stop a search inside files that's still going.
    SrchCancel,
    /// The search box's Names (false) / Contents (true) switch.
    SrchMode(bool),
    CluVol,
    CluCard,
    CluClose,
    /// Bring the playing app to the front (Now Playing's button and cover).
    OpenPlayer(u8),
    /// Sound input: open the list, pick a microphone.
    InMenu,
    InPick(usize),
    /// Chevron on the Wi-Fi (0) / Bluetooth (1) tile: open its list.
    CtlList(u8),
    CtlBack,
    CtlSettings,
    WifiNet(usize),
    WifiPass,
    WifiUser,
    WifiJoin,
    BtItem(usize),
    KbItem(usize),
    LocSettings,
    SrchBar,
    SrchWebAll,
    /// The page bar: a page's tile (click jumps there), and the bar itself.
    PageTile(usize),
    PageBar,
    /// A block of the read-picture document (click copies it); its Copy all.
    DocBlock(usize),
    DocCopy,
    /// The footer's page arrows (true: next page).
    SrchTurn(bool),
    SrchEnter,
    SrchGuide(usize),
    /// A page Gemini used for its answer.
    SrchSource(usize),
    /// A model chip on Gemini's answer; "Thought for …".
    AiModel(usize),
    AiThoughts,
    /// The Think chip (Gemini thinks first, or not).
    AiThink,
    /// An engine chip on the Alt+Q card (0 Google, 1-3 Gemini models).
    TrEngine(usize),
    /// Link page: a screen on the map (drag it), pair with PC, your answer to
    /// the code, unpair, go to the other PC, OK on a message, turn Link on.
    LinkScreen(usize),
    /// A screen's "Make main" on the Link page's map.
    LinkMain(usize),
    /// The picked screen's display settings: a stepper (0 scale, 1
    /// resolution, 2 orientation; true: next), make main, keep / revert, close.
    DispStep(u8, bool),
    DispMain,
    DispKeep,
    DispRevert,
    DispClose,
    LinkPair(u64),
    LinkDecide(bool),
    LinkUnpair(u64),
    LinkJump,
    LinkDismiss,
    LinkOn,
    /// Link page: hang up / call again, numbers on the screens, Remote
    /// Desktop, the address field and its Connect.
    LinkDisconnect,
    /// The Link page's on/off switch.
    LinkOff,
    LinkConnect,
    LinkIdentify,
    LinkAddr,
    LinkAddrGo,
    LinkFirewall,
    /// Shelf page: this PC's / the other PC's shelf; get one / all from there.
    ShelfTab(bool),
    ShelfGet(usize),
    /// Open one of the other PC's shelf items that's already here.
    ShelfOpen(usize),
    /// Open the folder files from the other PC land in.
    ShelfFolder,
    ShelfGetAll,
    /// Remote Desktop page: a field (0 computer, 1 user, 2 password), an
    /// option (0 remember, 1 full screen, 2 all screens), Connect, the linked
    /// PC, a recent computer and its ×.
    RdpField(u8),
    RdpOpt(u8),
    RdpGo,
    RdpLinked,
    RdpRecent(usize),
    RdpForget(usize),
    /// The Connect / Disconnect / Pair button on a Wi-Fi or Bluetooth row.
    WifiAct(usize),
    BtAct(usize),
    /// Power list: 0 lock, 1 sleep, 2 shut down, 3 restart.
    PowerDo(u8),
    /// Calendar: a day (to see it in both calendars); solar / lunar.
    CalDay(i64),
    CalMode(u8),
    TrCopy,
    TrClose,
    /// The field on the Search page.
    SrchPage,
    SrchItem(usize),
    SrchEngine,
    Peek,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum EditTarget {
    Todo,
    WeatherCity,
    TzCity,
    WifiPass,
    WifiUser,
    Search,
    LinkAddr,
    RdpHost,
    RdpUser,
    RdpPass,
    /// The note being written (Notes beside the to-do list).
    Note,
    /// A speaker's delay in ms (Play on several).
    MoDelay,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PeekKind {
    /// Single-line bar (volume, brightness).
    Bar,
    /// Icon circle + title/subtitle + right text (screenshots 4/5).
    Rich,
    /// Song change: art + title + equalizer.
    Track,
}

#[derive(Clone, Debug)]
pub struct Peek {
    pub kind: PeekKind,
    /// Same category replaces instead of queueing (e.g. "vol").
    pub cat: &'static str,
    pub glyph: &'static str,
    pub color: D2D1_COLOR_F,
    pub ring: Option<f32>,
    pub value: f32,
    pub title: String,
    pub sub: String,
    pub right: String,
    pub ms: u32,
    pub page: Option<Page>,
    /// Volume of several speakers at once: one row each (name, level).
    pub bars: Vec<(String, f32)>,
}

impl Peek {
    pub fn bar(cat: &'static str, glyph: &'static str, value: f32, label: String) -> Peek {
        Peek { kind: PeekKind::Bar, cat, glyph, color: rgb(0xffffff, 1.0), ring: None, value, title: String::new(), sub: String::new(), right: label, ms: 1600, page: None, bars: Vec::new() }
    }
    pub fn rich(cat: &'static str, glyph: &'static str, color: u32, ring: Option<f32>, title: &str, sub: &str, right: &str, ms: u32, page: Option<Page>) -> Peek {
        Peek { kind: PeekKind::Rich, cat, glyph, color: rgb(color, 1.0), ring, value: 0.0, title: title.into(), sub: sub.into(), right: right.into(), ms, page, bars: Vec::new() }
    }
    fn size(&self) -> (f32, f32) {
        match self.kind {
            PeekKind::Bar if self.bars.len() > 1 => (360.0, 18.0 + 24.0 * self.bars.len() as f32),
            PeekKind::Bar => (330.0, 40.0),
            PeekKind::Rich => (410.0, 66.0),
            PeekKind::Track => {
                let k = config::cur().i("peek_cover").clamp(60, 170) as f32 / 100.0;
                (410.0 + (k - 1.0).max(0.0) * 40.0, 58.0 * k)
            }
        }
    }
}

/// Small UI animations: view switches and hover cards.
pub(crate) struct UiAnim {
    /// Weather: 0 = Next days, 1 = Today.
    pub wx_view: Spring,
    /// Weather hover card: x, y of the card, its opacity, and the guide line's x.
    pub wx_hx: Spring,
    pub wx_hy: Spring,
    pub wx_ha: Spring,
    pub wx_gx: Spring,
    /// The hour last shown in the card (kept while it fades out).
    pub wx_last: Option<usize>,
    /// To-do (0) / Notes (1).
    pub todo_view: Spring,
    /// A new note shown: 0 → 1.
    pub note_in: Spring,
    /// Focus: which phase chip is highlighted (slides between them), the
    /// ring's progress as drawn, and running (1) / stopped (0).
    pub focus_chip: Spring,
    pub focus_prog: Spring,
    pub focus_run: Spring,
}

impl UiAnim {
    fn new(today: bool, notes: bool) -> Self {
        UiAnim {
            wx_view: Spring::unit(today as i32 as f32),
            wx_hx: Spring::size(0.0),
            wx_hy: Spring::size(0.0),
            wx_ha: Spring::new(0.0, 420.0, 38.0, 0.002),
            wx_gx: Spring::new(0.0, 520.0, 42.0, 0.05),
            wx_last: None,
            todo_view: Spring::unit(notes as i32 as f32),
            note_in: Spring::unit(1.0),
            focus_chip: Spring::new(0.0, 300.0, 32.0, 0.002),
            focus_prog: Spring::new(0.0, 120.0, 24.0, 0.0005),
            focus_run: Spring::unit(0.0),
        }
    }
    fn all(&mut self) -> [&mut Spring; 10] {
        [&mut self.wx_view, &mut self.wx_hx, &mut self.wx_hy, &mut self.wx_ha, &mut self.wx_gx, &mut self.todo_view, &mut self.note_in, &mut self.focus_chip, &mut self.focus_prog, &mut self.focus_run]
    }
    fn step(&mut self, dt: f32) -> bool {
        let mut m = false;
        for s in self.all() {
            m |= s.step(dt);
        }
        m
    }
    fn settled(&mut self) -> bool {
        self.all().iter().all(|s| s.settled())
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mascot {
    Off,
    Awake,
    Doze,
    Hello,
    /// Saying goodbye before it fades out (cat: shakes its head; robot: a
    /// wink, a heart, then it switches off like an old TV).
    Bye,
}

/// Font family / size / style chosen in Settings → Island → Text.
#[derive(Clone)]
pub(crate) struct TextStyle {
    pub ui: String,
    pub disp: String,
    pub scale: f32,
    pub bold: bool,
    pub italic: bool,
}

impl TextStyle {
    pub fn current() -> TextStyle {
        let c = config::cur();
        let (ui, disp) = match c.i("font_face") {
            1 => ("Segoe UI".to_string(), "Segoe UI".to_string()),
            2 => ("Bahnschrift".into(), "Bahnschrift".into()),
            3 => ("Cascadia Code".into(), "Cascadia Code".into()),
            4 => ("Georgia".into(), "Georgia".into()),
            5 if !c.s("font_custom").trim().is_empty() => (c.s("font_custom").trim().to_string(), c.s("font_custom").trim().to_string()),
            _ => (UI_FONT.to_string(), DISPLAY_FONT.to_string()),
        };
        let st = c.i("font_style");
        TextStyle { ui, disp, scale: c.i("font_scale").clamp(70, 150) as f32 / 100.0, bold: st == 1 || st == 3, italic: st >= 2 }
    }
    fn w(&self, weight: i32) -> i32 {
        if self.bold { weight.max(700) } else { weight }
    }
    pub fn ui(&self, size: f32, weight: i32, align: u8) -> IDWriteTextFormat {
        gfx::g().font_ex(&self.ui, size * self.scale, self.w(weight), align, self.italic)
    }
    pub fn disp(&self, size: f32, weight: i32, align: u8) -> IDWriteTextFormat {
        gfx::g().font_ex(&self.disp, size * self.scale, self.w(weight), align, self.italic)
    }
    pub fn wrap(&self, size: f32, weight: i32, align: u8) -> IDWriteTextFormat {
        gfx::g().font_wrap_ex(&self.ui, size * self.scale, self.w(weight), align, self.italic)
    }
}

/// Font family and weight for the big clocks (Settings → Pages → Clock style).
fn clock_face(ts: &TextStyle) -> (String, i32) {
    let c = config::cur();
    let fam = match c.i("clock_font") {
        1 => "Segoe UI Variable Display".to_string(),
        2 => "Bahnschrift".into(),
        3 => "Cascadia Code".into(),
        4 => "Georgia".into(),
        5 if !c.s("clock_font_custom").trim().is_empty() => c.s("clock_font_custom").trim().to_string(),
        _ => ts.disp.clone(),
    };
    let wt = [200, 300, 400, 600, 800][c.i("clock_weight").clamp(0, 4) as usize];
    (fam, wt)
}

/// The closed island's time ("col_t") or date ("col_d") font: Settings →
/// Island → Time / Date text: size (%), font and style; the island's own
/// text font and its designed weight by default.
fn col_font(ts: &TextStyle, key: &str, size: f32, weight: i32) -> IDWriteTextFormat {
    let c = config::cur();
    let fam = match c.i(&format!("{key}_font")) {
        1 => "Segoe UI Variable Display".to_string(),
        2 => "Segoe UI".into(),
        3 => "Bahnschrift".into(),
        4 => "Cascadia Code".into(),
        5 => "Georgia".into(),
        _ => ts.disp.clone(),
    };
    let k = c.i(&format!("{key}_size")).clamp(60, 160) as f32 / 100.0;
    // Default, Light, Regular, Bold, Italic, Bold italic.
    let st = c.i(&format!("{key}_style"));
    let (wt, it) = match st {
        1 => (300, false),
        2 => (400, false),
        3 => (700, false),
        4 => (weight, true),
        5 => (700, true),
        _ => (if ts.bold { weight.max(700) } else { weight }, ts.italic),
    };
    gfx::g().font_ex(&fam, size * ts.scale * k, wt, 1, it)
}

fn home_k() -> f32 {
    config::cur().i("home_clock_size").clamp(60, 160) as f32 / 100.0
}

pub(crate) struct Fonts {
    pub style: TextStyle,
    pub small_c: IDWriteTextFormat,
    pub pill: IDWriteTextFormat,
    pub title: IDWriteTextFormat,
    pub title_c: IDWriteTextFormat,
    pub artist: IDWriteTextFormat,
    pub artist_c: IDWriteTextFormat,
    pub tiny: IDWriteTextFormat,
    pub tiny_c: IDWriteTextFormat,
    pub tiny_r: IDWriteTextFormat,
    pub clock: Vec<(f32, IDWriteTextFormat)>,
    pub day: IDWriteTextFormat,
    pub date: IDWriteTextFormat,
    pub ampm: IDWriteTextFormat,
    pub icon: IDWriteTextFormat,
    pub icon_s: IDWriteTextFormat,
    pub icon_tile: IDWriteTextFormat,
    pub icon_m: IDWriteTextFormat,
    pub icon_big: IDWriteTextFormat,
    pub icon_ctl: IDWriteTextFormat,
    pub peek_t: IDWriteTextFormat,
    pub peek_s: IDWriteTextFormat,
    pub peek_r: IDWriteTextFormat,
    pub peek_big: IDWriteTextFormat,
    pub label: IDWriteTextFormat,
    pub label_c: IDWriteTextFormat,
    pub body: IDWriteTextFormat,
    pub body_c: IDWriteTextFormat,
    pub big_c: IDWriteTextFormat,
    pub wx_k: f32,
    pub wx_temp: IDWriteTextFormat,
    pub wx_title: IDWriteTextFormat,
    pub wx_line: IDWriteTextFormat,
    pub wx_label: IDWriteTextFormat,
    pub tiny_b: IDWriteTextFormat,
    pub srch: IDWriteTextFormat,
    /// Closed island: battery %, volume %, ON/OFF — one size for all.
    pub col_txt: IDWriteTextFormat,
    pub icon_col: IDWriteTextFormat,
    pub col_txt_c: IDWriteTextFormat,
    /// Closed island, time above the date: the two lines.
    pub col_t2: IDWriteTextFormat,
    pub col_d2: IDWriteTextFormat,
    /// Closed island, one line: the time, or the date (their own size and style).
    pub col_t1: IDWriteTextFormat,
    pub col_d1: IDWriteTextFormat,
    /// The speaker on the closed island (its glyphs run small).
    pub icon_spk: IDWriteTextFormat,
    pub wrap_src: IDWriteTextFormat,
    /// Notes: wrapped body text.
    pub note: IDWriteTextFormat,
    pub web_title: IDWriteTextFormat,
    pub ai_body: IDWriteTextFormat,
    pub cal_sub: IDWriteTextFormat,
    pub small: IDWriteTextFormat,
    pub wrap_tr: IDWriteTextFormat,
    pub srch_name: IDWriteTextFormat,
    /// The read-picture document: headings, table cells, code.
    pub doc_h1: IDWriteTextFormat,
    pub doc_h2: IDWriteTextFormat,
    pub doc_h3: IDWriteTextFormat,
    pub doc_cell: IDWriteTextFormat,
    pub doc_cell_b: IDWriteTextFormat,
    pub doc_code: IDWriteTextFormat,
}

impl Fonts {
    fn new() -> Fonts {
        let g = gfx::g();
        let ts = TextStyle::current();
        // Weather page, left side: its own text size (Settings → Pages).
        let wk = config::cur().i("wx_text").clamp(70, 160) as f32 / 100.0;
        Fonts {
            style: ts.clone(),
            small_c: ts.disp(14.0, 700, 1),
            pill: ts.ui(12.0, 600, 0),
            title: ts.ui(16.5, 650, 0),
            title_c: ts.ui(16.5, 650, 1),
            artist: ts.ui(13.0, 400, 0),
            artist_c: ts.ui(13.0, 400, 1),
            tiny: ts.ui(10.5, 500, 0),
            tiny_c: ts.ui(10.5, 500, 1),
            tiny_r: ts.ui(10.5, 500, 2),
            // Light, airy clock faces (largest first; the first that fits wins).
            clock: {
                let (fam, wt) = clock_face(&ts);
                [80.0f32, 72.0, 66.0, 60.0, 56.0, 52.0, 48.0, 44.0, 40.0, 38.0, 35.0, 32.0, 29.0, 26.0, 23.0, 20.0]
                    .iter()
                    .map(|&s| (s, gfx::g().font_ex(&fam, s, wt, 0, ts.italic)))
                    .collect()
            },
            day: ts.ui(12.5 * home_k(), 400, 1),
            date: ts.ui(11.0 * home_k(), 400, 1),
            ampm: ts.ui(11.5, 500, 0),
            icon: g.font(ICON_FONT, 14.0, 400, 1),
            icon_tile: g.font(ICON_FONT, 22.0, 400, 1),
            icon_m: g.font(ICON_FONT, 16.0, 400, 1),
            icon_s: g.font(ICON_FONT, 11.0, 400, 1),
            icon_big: g.font(ICON_FONT, 40.0, 400, 1),
            icon_ctl: g.font(ICON_FONT, 18.0, 400, 1),
            peek_t: ts.ui(15.0, 700, 0),
            peek_s: ts.ui(12.5, 400, 0),
            peek_r: ts.ui(13.0, 600, 2),
            peek_big: ts.disp(26.0, 500, 2),
            label: ts.ui(11.5, 700, 0),
            label_c: ts.ui(11.5, 700, 1),
            body: ts.ui(13.0, 500, 0),
            body_c: ts.ui(13.0, 600, 1),
            big_c: ts.disp(30.0, 800, 1),
            wx_k: wk,
            wx_temp: ts.disp(54.0 * wk, 700, 0),
            wx_title: ts.ui(16.5 * wk, 650, 1),
            wx_line: ts.ui(10.5 * wk, 500, 1),
            wx_label: ts.ui(11.5 * wk, 700, 0),
            tiny_b: gfx::g().font_ex(&ts.ui, 9.5, 700, 0, ts.italic),
            srch: ts.ui(19.0, 400, 0),
            col_txt: gfx::g().font_ex(&ts.ui, 10.5, 650, 0, false),
            col_txt_c: gfx::g().font_ex(&ts.ui, 10.5, 650, 1, false),
            col_t2: col_font(&ts, "col_t", 12.5, 700),
            col_d2: col_font(&ts, "col_d", 9.5, 550),
            col_t1: col_font(&ts, "col_t", 14.0, 700),
            col_d1: col_font(&ts, "col_d", 14.0, 700),
            icon_spk: g.font(ICON_FONT, 16.0, 400, 1),
            icon_col: g.font(ICON_FONT, 13.0, 400, 1),
            wrap_src: ts.wrap(11.5, 400, 0),
            note: ts.wrap(13.0, 400, 0),
            web_title: ts.ui(15.5, 500, 0),
            ai_body: ts.wrap(13.0, 400, 0),
            cal_sub: gfx::g().font_ex(&ts.ui, 8.5, 500, 1, false),
            small: gfx::g().font_ex(&ts.ui, 8.5, 500, 0, false),
            wrap_tr: ts.wrap(15.5, 500, 0),
            srch_name: ts.ui(14.0, 600, 0),
            doc_h1: ts.wrap(19.0, 700, 0),
            doc_h2: ts.wrap(16.5, 650, 0),
            doc_h3: ts.wrap(14.5, 650, 0),
            doc_cell: ts.wrap(12.0, 400, 0),
            doc_cell_b: ts.wrap(12.0, 650, 0),
            doc_code: gfx::g().font_wrap_ex("Consolas", 12.0 * ts.scale, 400, 0, false),
        }
    }
}

pub struct Island {
    pub hwnd: HWND,
    pub(crate) surf: Surface,
    pub(crate) f: Fonts,
    mon: Monitor,
    pub(crate) scale: f32,
    win: RECT,
    top: f32,

    w: Spring,
    h: Spring,
    a_col: Spring,
    a_peek: Spring,
    a_exp: Spring,
    slide: Spring,
    /// The page bar (hovering the dots): how open it is.
    pbar: Spring,

    pub expanded: bool,
    pub(crate) page: Page,
    peek: Option<Peek>,
    queue: VecDeque<Peek>,
    /// When the current popup appeared, and its content fade (0..1).
    peek_at: Option<Instant>,
    pub(crate) pk_a: Spring,
    last_peek: Option<Peek>,
    hover: bool,
    pub(crate) hot: Option<Hit>,
    pub(crate) hits: Vec<(Hit, D2D_RECT_F)>,
    /// Scrollable areas drawn this frame: 1 Controls list, 2 device deck, 3 Devices list.
    pub(crate) zones: Vec<(u8, D2D_RECT_F)>,
    /// How much the page body is scaled to fit the island (1 = standard size).
    pub(crate) page_fit: f32,
    /// Devices page, right column (connections / sound outputs) scroll offset.
    pub(crate) dev_scroll: f32,
    /// The page sliding out (or, while a swipe follows the fingers, the one
    /// sliding in) and where it sits relative to the current page: drawn at
    /// `slide + rel` page widths.
    slide_other: Option<(Page, f32)>,
    /// A touchpad swipe in progress: how far (in wheel units), and whether
    /// this gesture has already turned the page (the rest of it, momentum
    /// included, is ignored).
    swipe_acc: f32,
    swipe_on: bool,
    swipe_done: bool,
    press: Option<(Hit, i32, i32)>,
    wheel_acc: i32,

    pub(crate) media: MediaInfo,
    pub(crate) art: Option<Image>,
    /// The whole picture in its own shape (when it isn't square).
    pub(crate) art_fit: Option<Image>,
    /// A blurred square from the cover, behind a wide cover.
    pub(crate) art_blur: Option<Image>,
    art_id: u64,
    pub(crate) accent: D2D1_COLOR_F,
    pub(crate) icon_mode: i32,
    media_loaded: bool,

    /// Devices page: which Bluetooth card is in front (fractional while flipping).
    pub(crate) dev_card: Spring,
    dev_wheel: i32,

    pub(crate) edit: Option<EditTarget>,
    pub(crate) edit_buf: String,
    /// Where the caret is in `edit_buf` (byte index); None = at the end.
    pub(crate) edit_cur: Option<usize>,
    /// The field being edited as drawn: its text box, where the shown text
    /// starts in `edit_buf`, and its font (to put the caret where you click).
    pub(crate) edit_view: Option<(D2D_RECT_F, usize, IDWriteTextFormat, (f32, f32, f32))>,
    /// How what's being drawn maps to the window: (origin x, origin y, scale) — pages are scaled to fit.
    pub(crate) draw_map: (f32, f32, f32),
    pub(crate) caret: bool,
    todo_draft: String,
    hi_surrogate: Option<u16>,
    focused: bool,

    pub(crate) todo_scroll: usize,
    pub(crate) shelf_scroll: f32,
    /// The Shelf page shows the other PC's shelf.
    pub(crate) shelf_remote: bool,
    /// Calendar: the month / year picker is open (on this year).
    pub(crate) cal_picker: Option<i64>,
    /// To-do page shows Notes; the note open; how far the list / text is scrolled.
    pub(crate) notes_view: bool,
    pub(crate) note_sel: Option<u64>,
    pub(crate) notes_scroll: f32,
    pub(crate) note_text_scroll: f32,
    /// When the closed island was first seen looking wrong (`heal`).
    pub(crate) blank_since: Option<Instant>,
    /// Calendar: the picked day's details fade in / out (and which day is shown while fading).
    pub(crate) cal_fade: Spring,
    pub(crate) cal_shown: Option<i64>,
    /// Calendar day details: how far scrolled, and how far they can go.
    pub(crate) cal_detail_scroll: f32,
    pub(crate) cal_detail_max: f32,
    /// Weather page: today's hour-by-hour chart, and how far it's scrolled
    /// (negative: not yet placed at the current hour).
    pub(crate) wx_today: bool,
    /// Small UI animations (view switches, hover cards).
    pub(crate) ui: UiAnim,
    /// Devices page: what was just copied (0 IP, 1 MAC) and when.
    pub(crate) net_copied: Option<(u8, Instant)>,
    pub(crate) wx_scroll: f32,
    /// Remote Desktop page: what's typed, what happened, a saved password's computer.
    pub(crate) rdp: crate::rdp::Target,
    pub(crate) rdp_msg: Option<(String, bool, Instant)>,
    pub(crate) rdp_saved_for: Option<String>,
    /// Link page: the pointer dot as drawn (it glides) and where it's heading.
    pub(crate) link_dot: Option<(f32, f32)>,
    pub(crate) link_dot_target: Option<(f32, f32)>,
    pub(crate) drag_in: bool,
    vol_drag: bool,
    /// Dragging a delay / volume slider of a copy (Hit::MoDelay / MoVol).
    pub(crate) mo_drag: Option<Hit>,
    bright_drag: bool,
    /// Devices page: sound-output list open, and its entries.
    pub(crate) out_menu: bool,
    /// Controls page list: 0 none, 1 Wi-Fi networks, 2 Bluetooth devices.
    pub(crate) search: Option<SearchUi>,
    /// Month change on the calendar: −1…1 while sliding, 0 at rest.
    pub(crate) cal_slide: Spring,
    pub(crate) trans: Option<TransUi>,
    pub(crate) tr_a: Spring,
    pub(crate) srch_a: Spring,
    pub(crate) srch_icons: std::collections::HashMap<String, Option<Image>>,
    pub(crate) srch_more: bool,
    /// The picture pasted into the search box.
    pub(crate) srch_thumb: Option<Image>,
    /// The read-picture document, laid out; which block was just copied.
    pub(crate) doc_cache: Option<std::rc::Rc<doc_ui::DocLayout>>,
    pub(crate) doc_copied: Option<(usize, Instant)>,
    /// Where the document was drawn: (its id, origin x, y, visible area).
    pub(crate) doc_view: Option<(u64, f32, f32, D2D_RECT_F)>,
    /// Text selected in it (document id, from, to) and whether it's being dragged.
    pub(crate) doc_sel: Option<(u64, doc_ui::DPos, doc_ui::DPos)>,
    pub(crate) doc_drag: bool,
    /// Link page: the screen picked on the map (its display settings show),
    /// what Windows says about it (and when asked), and the last problem.
    pub(crate) link_sel: Option<String>,
    pub(crate) disp: Option<(String, crate::display::Info, Instant)>,
    pub(crate) disp_msg: Option<(String, Instant)>,
    eat_esc: bool,
    /// Alt+Q from a picture's text: its thumbnail, to show it again after.
    pub(crate) doc_back: Option<Option<Image>>,
    /// The window is sized for the search list.
    pub(crate) tall: bool,
    /// Something drawn this frame is still moving (keep the animation ticking).
    pub(crate) keep_anim: bool,
    /// For looping animations (thinking dots).
    pub(crate) born: Instant,
    pub(crate) ctl_list: u8,
    pub(crate) kb_list: Vec<crate::sysmon::Keyboard>,
    /// The list last opened (drawn while it fades out).
    pub(crate) ctl_last: u8,
    pub(crate) ctl_anim: Spring,
    pub(crate) ctl_scroll: f32,
    /// Enter went to the typing method (to finish a word): it still searches.
    pub(crate) ime_enter: bool,
    /// Ctrl+A: the whole field is selected.
    pub(crate) edit_all: bool,
    /// The Wi-Fi network / Bluetooth device row you clicked (highlighted).
    pub(crate) ctl_sel: Option<String>,
    /// Power list: the button waiting for its second click (shut down, restart).
    pub(crate) power_arm: Option<(u8, Instant)>,
    /// Network waiting for its password.
    pub(crate) wifi_pw_for: Option<String>,
    /// That network signs in with a user name too (work / school), and
    /// what's typed so far.
    pub(crate) wifi_ent: bool,
    pub(crate) wifi_user: String,
    pub(crate) wifi_pass: String,
    /// Result line shown in the list header for a few seconds.
    pub(crate) ctl_note: Option<(String, bool, Instant)>,
    /// 0 = connections shown, 1 = sound-output list shown (animated).
    pub(crate) out_anim: Spring,
    pub(crate) out_list: Vec<crate::audio::Output>,
    /// The sound list shows microphones (Sound input) instead of outputs.
    pub(crate) in_mode: bool,
    pub(crate) in_list: Vec<crate::audio::Output>,
    /// The speaker whose settings are open in the list (its id), and the one
    /// whose delay is being typed.
    pub(crate) mo_sel: Option<String>,
    pub(crate) mo_edit_id: String,
    /// The speakers playing together (main first), and the microphones —
    /// looked up when the page opens or something changes, not every frame.
    pub(crate) mo_cluster: Vec<(String, String)>,
    /// The speakers' own volume card: open?, its animation, and the button it hangs from.
    pub(crate) cv_open: bool,
    pub(crate) cv_anim: Spring,
    pub(crate) cv_anchor: Option<D2D_RECT_F>,
    /// Each grouped speaker's (volume, muted) as last read, the levels shown
    /// (gliding toward those), and the levels when the main slider was grabbed.
    pub(crate) mo_lv: Vec<(f32, bool)>,
    pub(crate) mo_show: Vec<f32>,
    vol_base: Option<Vec<f32>>,
    /// What the fonts were made from (see relayout).
    font_sig: String,
    /// The island sits on a taskbar at the top edge (kept above it).
    pub(crate) on_taskbar: bool,
    /// Settings → Island → "Attach to the taskbar": the taskbar it lives in
    /// (a horizontal one), the height there is for the closed island (DIPs),
    /// and whether it opens upwards (a taskbar at the bottom).
    dock_tb: HWND,
    dock_band: f32,
    pub(crate) up: bool,
    /// Right now shown inside the taskbar (closed), like the taskbar stats:
    /// drawn into `dock_child`, a child window of the taskbar, while the
    /// island's own window is hidden; when it opens, the island's window
    /// (kept above the taskbar) takes over again until it closes.
    pub(crate) docked: bool,
    pub(crate) dock_child: HWND,
    /// Couldn't make the window in the taskbar: not tried again until then.
    dock_wait: Option<Instant>,
    /// Out of the taskbar: show the island's own window once this frame is
    /// in it (not its old picture).
    show_main: bool,

    /// The taskbar as last seen (window, rectangle) and the hook on it.
    tb_seen: Option<(isize, RECT)>,
    tb_hook: Option<windows::Win32::UI::Accessibility::HWINEVENTHOOK>,
    /// Calendar page: month shown (months since year 0), chosen day, list scroll.
    pub(crate) cal_month: Option<i64>,
    /// Calendar: a picked day (shown in both calendars), and the lunar month
    /// shown (any day in it; None = this month).
    pub(crate) cal_sel: Option<i64>,
    pub(crate) cal_lmonth: Option<i64>,
    /// Brightness shown while dragging (the real value follows a moment later).
    pub(crate) bright_ui: Option<u8>,
    bright_sent: Option<Instant>,

    pub(crate) mascot: Mascot,
    pub(crate) blink: Spring,
    /// Mascot fade (0 = island content, 1 = mascot) and the state it's showing,
    /// kept while it fades out.
    pub(crate) masc_fade: Spring,
    pub(crate) masc_last: Mascot,
    /// What last covered the island (for the log), empty when on top.
    covered_by: String,
    covered_at: Option<Instant>,
    /// Remaining fast on-top re-checks after the taskbar jumped in front.
    burst: u32,
    /// Mascot: head tilt (radians), ear droop (-1 perked … +1 droopy), breathing.
    pub(crate) tilt: Spring,
    pub(crate) ear: Spring,
    pub(crate) breath: Spring,
    pub(crate) look: Spring,
    /// Mascot: walking offset, vertical glance, one-ear flick, eye style
    /// (0 lines, 1 happy "^", 2 half closed), and a clock for the walk bob.
    pub(crate) walk: Spring,
    pub(crate) look_y: Spring,
    pub(crate) flick: Spring,
    pub(crate) flick_side: f32,
    pub(crate) eye_mode: u8,
    /// Mascot hop (negative = up; under-damped so it lands with a bounce)
    /// and the second half of a two-part act (see T_BLINK2).
    pub(crate) hop: Spring,
    /// Mascot: the robot's smile (0 flat, 1 smile, ~2 grin), its "o" mouth,
    /// and how much the cat's whiskers wave (after something lively).
    pub(crate) smile: Spring,
    pub(crate) mouth_o: bool,
    pub(crate) wag: Spring,
    /// "Always" mascot: when you last clicked it away (or used the island).
    pub(crate) masc_dismissed: Option<Instant>,
    /// OLED care: the island's offset now and where it's drifting to
    /// (device pixels), and minutes since it last picked a new spot.
    drift: (i32, i32),
    drift_to: (i32, i32),
    drift_min: u32,
    /// When the goodbye started.
    pub(crate) bye_at: Instant,
    /// The cat's paws: when a key last went down, which paw (±1).
    pub(crate) tap_at: Instant,
    pub(crate) tap_side: f32,
    /// The cat sticking its tongue out / meowing (since when).
    pub(crate) blep_at: Option<Instant>,
    pub(crate) meow_at: Option<Instant>,
    /// Cat vs Robot: the final round's winner (0 cat, 1 robot) and a seed
    /// that varies the fights.
    pub(crate) duel_winner: u8,
    pub(crate) duel_seed: u64,
    /// Closed island: what each part opens when you hover / click it
    /// (battery → controls, time → calendar, music → player, speaker → devices),
    /// and where the pointer last was.
    pub(crate) col_zones: Vec<(Page, D2D_RECT_F)>,
    /// Link page: the map's placement (left, top, map units per DIP, map
    /// origin) and a screen being dragged (index, pointer start, its start,
    /// where it is now).
    pub(crate) link_fit: (f32, f32, f32, (f32, f32)),
    pub(crate) link_drag: Option<(usize, (f32, f32), (f32, f32), (f32, f32))>,
    pub(crate) last_pt: (f32, f32),
    follow: u8,
    act_armed: bool,
    pub(crate) t0: Instant,

    pub(crate) dark: bool,
    animating: bool,
    last_tick: Instant,
    pub(crate) eq_t: f32,
    suppressed: bool,
}

pub unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        // Hidden because the island went into the taskbar: not a real leave.
        WM_MOUSELEAVE if crate::app().island.as_ref().map(|i| i.hwnd == hwnd && i.docked).unwrap_or(false) => LRESULT(0),
        WM_TIMER | WM_MOUSEMOVE | WM_MOUSELEAVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONUP | WM_MOUSEWHEEL | WM_MOUSEHWHEEL | WM_CHAR | WM_KEYDOWN | WM_ACTIVATE => {
            if let Some(i) = crate::app().island.as_mut() {
                if i.hwnd == hwnd {
                    if let Some(r) = i.handle(msg, wp, lp) {
                        return r;
                    }
                }
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_MOUSEACTIVATE => {
            // Only take focus when a text field asked for it.
            if crate::app().island.as_ref().map(|i| i.edit.is_some() || i.focused).unwrap_or(false) {
                LRESULT(MA_ACTIVATE as isize)
            } else {
                LRESULT(MA_NOACTIVATE as isize)
            }
        }
        WM_DPICHANGED | WM_DISPLAYCHANGE => {
            crate::request_apply();
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

pub fn register() {
    util::register_class(CLASS, Some(wndproc), HICON::default());
    util::register_class(DOCK_CLASS, Some(dock_proc), HICON::default());
}

const DOCK_CLASS: PCWSTR = w!("PhemioIslandDock");

/// The island's window inside the taskbar: what it's told goes to the island.
unsafe extern "system" fn dock_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        // The pointer left the window in the taskbar because the island came
        // out of it: not a real leave.
        WM_MOUSELEAVE if crate::app().island.as_ref().map(|i| !i.docked).unwrap_or(true) => LRESULT(0),
        WM_MOUSEMOVE | WM_MOUSELEAVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONUP | WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            if let Some(i) = crate::app().island.as_mut() {
                if i.dock_child == hwnd {
                    if let Some(r) = i.handle(msg, wp, lp) {
                        return r;
                    }
                }
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

impl Island {
    pub fn new() -> Option<Box<Island>> {
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                CLASS,
                w!("Phemio"),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                None,
                Some(util::hinstance()),
                None,
            )
            .ok()?;
            let pages = enabled_pages();
            let last = ALL_PAGES.get(config::cur().i("last_page") as usize).copied().unwrap_or(Page::Home);
            let page = if pages.contains(&last) { last } else { pages[0] };
            let mut isl = Box::new(Island {
                hwnd,
                surf: Surface::new().ok()?,
                f: Fonts::new(),
                mon: Monitor::default(),
                scale: 1.0,
                win: RECT::default(),
                top: 0.0,
                w: Spring::size(160.0),
                h: Spring::size(32.0),
                a_col: Spring::unit(1.0),
                a_peek: Spring::unit(0.0),
                a_exp: Spring::unit(0.0),
                slide: Spring::new(0.0, 300.0, 30.0, 0.002),
                pbar: Spring::new(0.0, 420.0, 34.0, 0.002),
                expanded: false,
                page,
                peek: None,
                queue: VecDeque::new(),
                peek_at: None,
                pk_a: Spring::new(1.0, 1100.0, 66.0, 0.01),
                last_peek: None,
                hover: false,
                hot: None,
                hits: Vec::new(),
                press: None,
                wheel_acc: 0,
                media: MediaInfo::default(),
                art: None,
                art_fit: None,
                art_blur: None,
                art_id: 0,
                accent: rgb(0x34c759, 1.0),
                icon_mode: 0,
                media_loaded: false,
                dev_card: Spring::new(0.0, 210.0, 25.0, 0.002),
                dev_wheel: 0,
                zones: Vec::new(),
                page_fit: 1.0,
                dev_scroll: 0.0,
                slide_other: None,
                swipe_acc: 0.0,
                swipe_on: false,
                swipe_done: false,
                edit: None,
                edit_buf: String::new(),
                edit_cur: None,
                edit_view: None,
                draw_map: (0.0, 0.0, 1.0),
                caret: true,
                todo_draft: String::new(),
                hi_surrogate: None,
                focused: false,
                todo_scroll: 0,
                shelf_scroll: 0.0,
                shelf_remote: false,
                cal_picker: None,
                notes_view: config::cur().b("notes_view"),
                note_sel: None,
                notes_scroll: 0.0,
                note_text_scroll: 0.0,
                blank_since: None,
                cal_fade: Spring::unit(0.0),
                cal_shown: None,
                cal_detail_scroll: 0.0,
                cal_detail_max: 0.0,
                wx_today: config::cur().b("wx_today"),
                ui: UiAnim::new(config::cur().b("wx_today"), config::cur().b("notes_view")),
                net_copied: None,
                wx_scroll: -1.0,
                rdp: {
                    let c = config::cur();
                    let (host, user) = crate::rdp::recents().into_iter().next().unwrap_or_default();
                    crate::rdp::Target { host, user, pass: String::new(), remember: c.b("rdp_remember"), full: c.b("rdp_full"), all_screens: c.b("rdp_multi") }
                },
                rdp_msg: None,
                rdp_saved_for: None,
                link_dot: None,
                link_dot_target: None,
                drag_in: false,
                vol_drag: false,
                mo_drag: None,
                bright_drag: false,
                out_menu: false,
                search: None,
                cal_slide: Spring::new(0.0, 210.0, 25.0, 0.002),
                trans: None,
                tr_a: Spring::new(0.0, 260.0, 30.0, 0.003),
                srch_a: Spring::new(0.0, 260.0, 30.0, 0.003),
                srch_icons: std::collections::HashMap::new(),
                srch_more: false,
                srch_thumb: None,
                doc_cache: None,
                doc_copied: None,
                doc_view: None,
                doc_sel: None,
                doc_drag: false,
                link_sel: None,
                disp: None,
                disp_msg: None,
                eat_esc: false,
                doc_back: None,
                tall: false,
                keep_anim: false,
                born: Instant::now(),
                ctl_list: 0,
                kb_list: Vec::new(),
                ctl_last: 1,
                ctl_anim: Spring::new(0.0, 230.0, 26.0, 0.003),
                ctl_scroll: 0.0,
                wifi_pw_for: None,
                wifi_ent: false,
                wifi_user: String::new(),
                wifi_pass: String::new(),
                power_arm: None,
                ctl_sel: None,
                ime_enter: false,
                edit_all: false,
                ctl_note: None,
                out_anim: Spring::new(0.0, 230.0, 26.0, 0.003),
                out_list: Vec::new(),
                in_mode: false,
                in_list: Vec::new(),
                mo_sel: None,
                mo_edit_id: String::new(),
                mo_cluster: Vec::new(),
                cv_open: false,
                cv_anim: Spring::new(0.0, 260.0, 26.0, 0.003),
                cv_anchor: None,
                mo_lv: Vec::new(),
                mo_show: Vec::new(),
                vol_base: None,
                font_sig: String::new(),
                tb_seen: None,
                on_taskbar: false,
                dock_tb: HWND::default(),
                dock_band: 0.0,
                up: false,
                docked: false,
                dock_child: HWND::default(),
                dock_wait: None,
                show_main: false,

                tb_hook: None,
                cal_month: None,
                cal_sel: None,
                cal_lmonth: None,
                bright_ui: None,
                bright_sent: None,
                mascot: Mascot::Off,
                blink: Spring::new(1.0, 900.0, 40.0, 0.01),
                masc_fade: Spring::new(0.0, 40.0, 12.6, 0.003),
                // Critically damped: glides to a stop without wobble.
                walk: Spring::new(0.0, 16.0, 8.0, 0.02),
                look_y: Spring::new(0.0, 160.0, 18.0, 0.02),
                flick: Spring::new(0.0, 420.0, 9.0, 0.01),
                flick_side: 1.0,
                eye_mode: 0,
                hop: Spring::new(0.0, 320.0, 9.0, 0.01),
                smile: Spring::new(1.0, 90.0, 14.0, 0.002),
                mouth_o: false,
                wag: Spring::new(0.0, 5.0, 4.5, 0.002),
                masc_dismissed: None,
                drift: (0, 0),
                drift_to: (0, 0),
                drift_min: 0,
                bye_at: Instant::now(),
                tap_at: crate::util::ago(std::time::Duration::from_secs(60)),
                tap_side: 1.0,
                blep_at: None,
                meow_at: None,
                duel_winner: 0,
                duel_seed: util::rand_u32() as u64,
                col_zones: Vec::new(),
                link_fit: (0.0, 0.0, 1.0, (0.0, 0.0)),
                link_drag: None,
                last_pt: (0.0, 0.0),
                follow: 0,
                act_armed: false,
                t0: Instant::now(),
                masc_last: Mascot::Awake,
                covered_by: String::new(),
                covered_at: None,
                burst: 0,
                tilt: Spring::new(0.0, 120.0, 14.0, 0.002),
                ear: Spring::new(0.0, 260.0, 12.0, 0.01),
                breath: Spring::new(0.0, 6.0, 4.0, 0.01),
                look: Spring::new(0.0, 130.0, 22.0, 0.01),
                dark: true,
                animating: false,
                last_tick: Instant::now(),
                eq_t: 0.0,
                suppressed: false,
            });
            isl.relayout();
            isl.snap();
            isl.render();
            // (Already inside the taskbar: its own window stays hidden.)
            if !isl.docked {
                let _ = ShowWindow(hwnd, SW_SHOWNA);
            }
            SetTimer(Some(hwnd), T_CLOCK, util::ms_to_next_minute(), None);
            SetTimer(Some(hwnd), T_HEAL, 2000, None);
            crate::shelf::register(hwnd);
            Some(isl)
        }
    }

    pub fn destroy(&mut self) {
        self.drop_child();
        crate::shelf::revoke(self.hwnd);
        if let Some(h) = self.tb_hook.take() {
            unsafe {
                let _ = windows::Win32::UI::Accessibility::UnhookWinEvent(h);
            }
        }
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }

    // ------------------------------------------------------------------
    // Layout
    // ------------------------------------------------------------------

    pub(crate) fn notch(&self) -> bool {
        config::cur().i("island_style") == 1
    }

    /// The closed shape's height, and how much its contents are scaled
    /// (1 = as designed) — from Settings → Island → Closed height.
    pub(crate) fn col_dims(&self) -> (f32, f32) {
        let c = config::cur();
        // The thin "minimal" line has its own size.
        let pct = if c.b("minimal") && self.mascot == Mascot::Off { 1.0 } else { c.i("col_h").clamp(60, 140) as f32 / 100.0 };
        // Taller: only the shape grows (text and icons stay as they are).
        // Shorter than designed: the contents shrink with it so they fit.
        if self.notch() && !self.dock_tb.is_invalid() && self.dock_band > 8.0 {
            // In the taskbar: a share of its height (at most all of it).
            let h = self.dock_band * pct.min(1.0);
            return (h, (h / 32.0).min(1.0));
        }
        (if self.notch() { 32.0 } else { 34.0 } * pct, pct.min(1.0))
    }

    fn collapsed_size(&self) -> (f32, f32) {
        let c = config::cur();
        let notch = self.notch();
        // The mascot uses the island's normal closed size.
        if c.b("minimal") && self.mascot == Mascot::Off {
            return if notch { (170.0, 7.0) } else { (130.0, 8.0) };
        }
        // Its height (Settings → Island → Closed height; a notch attached to
        // the taskbar is the taskbar's height at 100%, never taller), and the
        // width and contents with it.
        let (h, k) = self.col_dims();
        let d = crate::data();
        let playing = self.media.has_session && self.media.playing;
        let mic_left = c.b("col_battery") && c.i("col_left") == 1;
        let saver = c.b("col_battery") && d.battery.is_some();
        let batt = saver && !mic_left;
        // (No battery level to read: a battery with an ✕ still takes the left.)
        let left = d.focus.active() || playing || c.b("col_battery");
        let right = playing || saver || c.i("col_right") == 1;
        let base = if !d.focus.active() && c.i("col_center") >= 2 {
            // Time (with seconds) above the date: the wider of the two.
            let wide = !util::is_24h(c.i("clock_fmt"));
            let t = gfx::g().measure(if wide { "18:88:88 PM" } else { "88:88:88" }, &self.f.col_t2);
            let dd = gfx::g().measure(&util::date_fmt(&util::now(), "ddd d MMM"), &self.f.col_d2) + luck_room(&c);
            (t.max(dd) + 84.0).max(128.0 * self.f.style.scale.max(1.0))
        } else if !d.focus.active() && c.i("col_center") == 1 {
            // Date: size to the text (month names vary by language).
            (gfx::g().measure(&center_text(&c), &self.f.col_d1) + luck_room(&c) + 84.0).max(128.0 * self.f.style.scale.max(1.0))
        } else if d.focus.active() {
            128.0 * self.f.style.scale.max(1.0)
        } else {
            // Room for the widest digits so the island doesn't twitch each second.
            let wide = !util::is_24h(c.i("clock_fmt"));
            let t = match (c.b("clock_secs"), wide) {
                (true, true) => "18:88:88 PM",
                (true, false) => "88:88:88",
                (false, true) => "18:88 PM",
                (false, false) => "88:88",
            };
            (gfx::g().measure(t, &self.f.col_t1) + 84.0).max(128.0 * self.f.style.scale.max(1.0))
        };
        // A battery drawn as a battery with its number needs a little more room.
        let batt_icon = batt && !d.focus.active() && c.i("batt_style") == 1;
        // The microphone with ON / OFF beside it, likewise.
        let mic_icon = mic_left && !d.focus.active() && crate::controls::state().mic_muted.is_some();
        let mut w = (base + if left { 44.0 } else { 0.0 } + if batt_icon || mic_icon { 34.0 } else { 0.0 } + if right { 48.0 } else { 0.0 } + if notch { 14.0 } else { 0.0 }) * k;
        // Cat vs Robot needs a ring.
        if self.mascot != Mascot::Off && c.i("mascot_style") == 2 {
            w = w.max(280.0);
        }
        (w, h)
    }

    /// Recompute monitor, scale and window rectangle from settings.
    pub fn relayout(&mut self) {
        let _p = util::Prof::new("island.relayout");
        // (First time: start watching the taskbar.)
        if self.tb_hook.is_none() && self.tb_seen.is_none() {
            self.watch_taskbar();
        }
        let c = config::cur();
        // Fonts only when something they're made from changed (making them
        // all again on every step of a settings slider is what made it lag).
        let sig = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            c.i("font_face"), c.s("font_custom"), c.i("font_style"), c.i("font_scale"), c.i("wx_text"),
            c.i("clock_font"), c.s("clock_font_custom"), c.i("clock_weight"), c.i("home_clock_size"),
            ["col_t_size", "col_t_font", "col_t_style", "col_d_size", "col_d_font", "col_d_style"].map(|k| c.i(k).to_string()).join(",")
        );
        if sig != self.font_sig {
            self.f = Fonts::new();
            self.font_sig = sig;
        }
        self.dark = util::is_dark(c.i("theme"));
        self.icon_mode = c.i("icon_colors");
        self.mon = util::primary_monitor();
        self.scale = self.mon.scale * c.i("island_size").clamp(70, 160) as f32 / 100.0;
        let s = self.scale;
        // Where is "the top"? With a taskbar on the top edge, hang below it so
        // the taskbar can never cover the island.
        let m = self.mon.full;
        // A taskbar on the top edge, judged from the taskbar window itself:
        // some top-taskbar setups don't reserve the work area.
        // A taskbar on the top edge (judged from the taskbar window itself:
        // some top-taskbar setups don't reserve the work area): the island
        // sits ON the taskbar, like the taskbar stats — at the very top, its
        // closed shape in the middle of the taskbar's height (a notch stays
        // on the edge) — and is kept above it. On another edge: as set.
        let tb_now = util::taskbar_rect();
        // Attached: a horizontal taskbar on this screen, at the top or bottom.
        let attach = tb_now.filter(|(_, r)| {
            c.b("island_in_tb") && r.left < m.right && r.right > m.left && (r.right - r.left) > (r.bottom - r.top) * 3 && (r.bottom - r.top) < (m.bottom - m.top) / 3
        });
        let was = self.dock_tb;
        self.dock_tb = attach.map(|(h, _)| h).unwrap_or_default();
        if self.dock_tb != was {
            // Another taskbar window (or none): out of the old one first.
            if self.docked {
                self.undock(true);
            }
            self.drop_child();
        }
        let top_tb = tb_now
            .map(|(_, r)| r)
            .filter(|r| attach.is_none() && r.left < m.right && r.right > m.left && r.top <= m.top + 2 && (r.right - r.left) > (r.bottom - r.top) && r.bottom < m.top + (m.bottom - m.top) / 3 && r.bottom > m.top + 4);
        self.on_taskbar = top_tb.is_some();
        let notch_tb = attach.is_some() && self.notch();
        if let (true, Some((_, r))) = (notch_tb, attach) {
            // The notch's height comes from the taskbar.
            self.dock_band = (r.bottom - r.top) as f32 / s;
        }
        let (_, ch) = self.collapsed_size();
        // In the taskbar the closed island must fit its height: a bigger
        // island (Size) is made just small enough. (A notch is the taskbar's
        // height whatever the size.)
        let s = match attach {
            Some((_, r)) if !notch_tb => {
                let fit = (r.bottom - r.top) as f32 * 0.86 / ch.max(1.0);
                if self.scale > fit {
                    self.scale = fit.max(0.5);
                }
                self.scale
            }
            _ => s,
        };
        let eh = if self.tall { eh().max(search_ui::SEARCH_MAX_H) } else { eh() + if dock_extends().is_some() { DOCK_EXT } else { 0.0 } };
        let ww = ((ew() + PAD * 2.0) * s).ceil() as i32;
        let mw = m.right - m.left;
        let off = (c.i("island_x") as f32 * self.mon.scale) as i32;
        let margin = ((12.0 - PAD) * s) as i32;
        let mut x = match c.i("island_align") {
            0 => m.left + margin + off,
            2 => m.right - ww - margin + off,
            _ => m.left + (mw - ww) / 2 + off,
        };
        x = x.clamp(m.left - (PAD * s) as i32, m.right - ww + (PAD * s) as i32);
        let (anchor_y, wh) = if let Some((_, r)) = attach {
            // In the taskbar: the closed island in the middle of its height.
            let band_px = r.bottom - r.top;
            let band = band_px as f32 / s;
            self.dock_band = band;
            // A notch from the screen's edge; the island in the middle.
            let gap = if notch_tb { 0.0 } else { ((band - ch) / 2.0).max(0.0).round() };
            // At the bottom (judged by which half of the screen it's in, so
            // an auto-hidden taskbar counts too) it opens upwards.
            self.up = (r.top + r.bottom) / 2 > (m.top + m.bottom) / 2;
            if self.up {
                let wh = ((gap + eh + PAD) * s).ceil() as i32;
                self.top = wh as f32 / s - gap;
                (m.bottom - wh, wh)
            } else {
                self.top = gap;
                (m.top, ((gap + eh + PAD) * s).ceil() as i32)
            }
        } else {
            self.up = false;
            let anchor_y = match top_tb {
                Some(_) => m.top,
                None => match c.i("anchor") {
                    0 if self.mon.work.top > m.top => self.mon.work.top,
                    2 => self.mon.work.top,
                    _ => m.top,
                },
            };
            self.top = match top_tb {
                _ if self.notch() => 0.0,
                Some(r) => {
                    let band = (r.bottom - m.top) as f32 / s;
                    ((band - 34.0) / 2.0).max(0.0).round()
                }
                None => c.i("island_y").clamp(0, 80) as f32,
            };
            (anchor_y, ((self.top + eh + PAD) * s).ceil() as i32)
        };
        let old_win = self.win;
        self.win = RECT { left: x, top: anchor_y, right: x + ww, bottom: anchor_y + wh };
        self.surf.resize(ww, wh);
        unsafe {
            let after = if c.b("island_top") || !self.dock_tb.is_invalid() { HWND_TOPMOST } else { HWND_NOTOPMOST };
            // Showing: only its place in the stack here. The new position and
            // size go with the next frame (UpdateLayeredWindow does both at
            // once): moving it now would show the old picture in the new
            // place for a moment — the island seen jumping or flashing.
            let flags = if IsWindowVisible(self.hwnd).as_bool() { SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE } else { SWP_NOACTIVATE };
            let _ = SetWindowPos(self.hwnd, Some(after), x, anchor_y, ww, wh, flags);
        }
        if self.docked && old_win != self.win {
            // The same for the window in the taskbar: the next frame goes to
            // the island's own window, and it goes back in once drawn in its
            // new place.
            self.undock(false);
        }
        {
            if was != self.dock_tb {
                // Attached: owned by the taskbar, so always above it while
                // open. Not attached: a window of its own again.
                unsafe {
                    SetWindowLongPtrW(self.hwnd, GWLP_HWNDPARENT, self.dock_tb.0 as isize);
                }
            }
        }
        if c.b("island_always") {
            self.expanded = true;
        } else if !self.hover && self.edit.is_none() {
            self.expanded = false;
        }
        let pages = enabled_pages();
        if !pages.contains(&self.page) {
            self.page = pages[0];
        }
        crate::data().focus.refresh_len();
        self.retarget();
        self.render();
    }

    /// Put the island back above other topmost windows (the taskbar likes to
    /// jump in front when the foreground window changes).
    fn blink_now(&mut self) {
        self.blink.v = 0.05;
        self.blink.vel = 0.0;
        self.blink.set(1.0);
    }

    /// The idle cat's next little act — an energetic one, picked at random
    /// every ½–1½ s.
    fn mascot_act(&mut self) {
        let r = util::rand_u32() % 21;
        if std::env::var("ISLET_LOG_MASCOT").is_ok() {
            util::log(&format!("mascot act {r}"));
        }
        let later = |isl: &mut Island, ms: u32, follow: u8| unsafe {
            isl.follow = follow;
            SetTimer(Some(isl.hwnd), T_BLINK2, ms, None);
        };
        let side = || if util::rand_u32() % 2 == 0 { 1.0f32 } else { -1.0 };
        if r != 12 {
            self.tilt.set(0.0);
        }
        // Blink often, between everything else (lively eyes).
        if r > 1 && util::rand_u32() % 3 == 0 {
            self.blink_now();
        }
        // Anything lively sets the cat's whiskers waving.
        if matches!(r, 4..=9 | 13 | 14 | 18) {
            self.wag.v = 1.0;
            self.wag.vel = 0.0;
            self.wag.set(0.0);
        }
        if r != 10 && r != 6 {
            self.look_y.set(0.0);
        }
        match r {
            0 => self.blink_now(),
            1 => {
                self.blink_now();
                later(self, 180, 0);
            }
            2 => self.look.set(-5.0),
            3 => self.look.set(5.0),
            // Rapid glance: one way, then the other.
            4 => {
                self.look.set(-5.0 * side());
                later(self, 260, 1);
            }
            // Hop! (ears up on the way)
            5 => {
                self.hop.v = -1.0;
                self.hop.vel = 0.0;
                self.hop.set(0.0);
                self.ear.v = -0.7;
                self.ear.set(0.0);
            }
            // Pounce: crouch with ears back, then spring to a new spot.
            6 => {
                self.ear.set(0.6);
                self.look_y.set(1.8);
                later(self, 320, 2);
            }
            // Wander / zoom along the island, eyes leading.
            7 | 8 | 9 => {
                let span = if r == 9 { 1.0 } else { 0.7 };
                let mut to = ((util::rand_u32() % 1000) as f32 / 1000.0 * 2.0 - 1.0) * span;
                if (to - self.walk.target).abs() < 0.35 {
                    to = -to.signum() * (0.4 + to.abs()).min(1.0);
                }
                let dir = if to > self.walk.target { 1.0 } else { -1.0 };
                self.walk.set(to);
                self.look.set(4.0 * dir);
            }
            // Look up, ears perked.
            10 => {
                self.look_y.set(-2.5);
                self.ear.v = -0.5;
                self.ear.set(-0.25);
            }
            // Happy squint (the robot beams).
            11 => {
                self.eye_mode = 1;
                self.smile.set(1.6);
                later(self, 700, 0);
            }
            // Bored: half-closed eyes, flat mouth.
            16 => {
                self.eye_mode = 2;
                self.smile.set(0.0);
                self.look.set(-3.0 * side());
                later(self, 1400, 0);
            }
            // Surprised: round eyes, "o" mouth.
            17 => {
                self.eye_mode = 3;
                self.mouth_o = true;
                self.ear.v = -0.8;
                self.ear.set(0.0);
                later(self, 650, 0);
            }
            // Big grin and a happy hop.
            18 => {
                self.eye_mode = 1;
                self.smile.set(2.0);
                self.hop.v = -1.1;
                self.hop.vel = 0.0;
                self.hop.set(0.0);
                later(self, 900, 0);
            }
            // Curious head tilt, then straighten up.
            12 => {
                let sd = side();
                self.tilt.set(0.24 * sd);
                self.look.set(1.8 * sd);
                later(self, 520, 3);
            }
            // The cat sticks its tongue out (blep), or meows.
            19 => {
                self.blep_at = Some(Instant::now());
                later(self, 1500, 0);
            }
            20 => {
                self.meow_at = Some(Instant::now());
                self.ear.v = -0.6;
                self.ear.set(0.0);
            }
            // Flick one ear.
            13 => {
                self.flick_side = side();
                self.flick.v = 1.0;
                self.flick.set(0.0);
            }
            // Wiggle both ears.
            14 => {
                self.ear.v = -1.0;
                self.ear.vel = 0.0;
                self.ear.set(0.0);
                self.blink_now();
            }
            // Sit in the middle and wait.
            _ => {
                self.walk.set(0.0);
                self.look.set(0.0);
                self.ear.set(0.0);
            }
        }
    }

    /// Keep the island visible and in front (Settings → Always on top).
    /// Checks what window is actually under the middle of the island; if it
    /// isn't the island, re-asserts "topmost" (toggling it, which makes
    /// Windows re-stack the window even when it thinks nothing changed).
    pub fn ensure_on_top(&mut self) {
        // Attached to the taskbar: owned by it, so above it when on its
        // own; inside it, kept in front of Explorer's own windows there.
        if !self.dock_tb.is_invalid() {
            self.covered_by = String::new();
            self.keep_child_on_top();
            return;
        }
        if self.suppressed || !config::cur().b("island_top") || !config::cur().b("island") {
            self.covered_by = String::new();
            return;
        }
        unsafe {
            if !IsWindowVisible(self.hwnd).as_bool() {
                util::log("island was hidden by something else; showing it again");
                let _ = ShowWindow(self.hwnd, SW_SHOWNA);
            }
            // Probe the middle of the shape and several points along its top
            // edge: when expanded, the middle sits below a top taskbar that
            // still covers the upper part.
            let s = self.scale;
            let cx = (self.win.left + self.win.right) / 2;
            let half_w = ((self.w.v.max(20.0) / 2.0 - 10.0).max(2.0) * s) as i32;
            let top_y = self.win.top + ((self.top + 5.0) * s) as i32;
            let mid_y = self.win.top + ((self.top + self.h.v.max(6.0) / 2.0) * s) as i32;
            let probes = [
                POINT { x: cx, y: mid_y },
                POINT { x: cx, y: top_y },
                POINT { x: cx - half_w * 2 / 3, y: top_y },
                POINT { x: cx + half_w * 2 / 3, y: top_y },
            ];
            let mut root = HWND::default();
            for pt in probes {
                let hit = WindowFromPoint(pt);
                let r = if hit.is_invalid() { HWND::default() } else { GetAncestor(hit, GA_ROOT) };
                if !r.is_invalid() && r != self.hwnd {
                    root = r;
                    break;
                }
            }
            if root.is_invalid() {
                return;
            }
            // Our own settings window may sit over it; that's fine.
            if crate::app().settings_hwnd() == root {
                return;
            }
            let proc = util::process_name(root);
            let class = util::class_name(root);
            // System overlays that are meant to cover everything (lock screen,
            // screen snipping, Click to Do, Start, search, Task View, Alt+Tab,
            // Quick Settings / notification centre): let them be.
            const OVERLAY_PROCS: &[&str] = &[
                "lockapp.exe", "logonui.exe", "snippingtool.exe", "screenclippinghost.exe", "clicktodo.exe",
                "startmenuexperiencehost.exe", "searchhost.exe", "searchapp.exe", "shellexperiencehost.exe",
                "shellhost.exe", "textinputhost.exe", "widgets.exe", "widgetboard.exe", "consent.exe",
            ];
            const OVERLAY_CLASSES: &[&str] = &[
                "LockScreenBackstopFrame", "MultitaskingViewFrame", "XamlExplorerHostIslandWindow", "ForegroundStaging",
                "TaskSwitcherWnd", "Windows.UI.Core.CoreWindow", "ControlCenterWindow", "Shell_Flyout",
            ];
            if OVERLAY_PROCS.contains(&proc.as_str()) || OVERLAY_CLASSES.contains(&class.as_str()) {
                return;
            }
            let who = format!("{proc} [{class}]");
            let taskbar = class == "Shell_TrayWnd" || class == "Shell_SecondaryTrayWnd";
            // Log each culprit at most once a minute.
            let quiet = who == self.covered_by && self.covered_at.map(|t| t.elapsed().as_secs() < 60).unwrap_or(false);
            if !quiet {
                util::log(&format!("island covered by {who}; bringing it back to the front"));
                self.covered_by = who;
                self.covered_at = Some(Instant::now());
            }
            // A plain raise first; the harder toggle only if that didn't do it
            // (dropping out of "topmost" for a moment can show as a flicker).
            self.raise_topmost();
            let still = probes.iter().any(|pt| {
                let hit = WindowFromPoint(*pt);
                let r = if hit.is_invalid() { HWND::default() } else { GetAncestor(hit, GA_ROOT) };
                r == root
            });
            if still {
                self.force_topmost();
            }
            // Explorer tends to re-stack the taskbar a few more times right
            // after it's clicked: keep re-checking quickly for half a second.
            if taskbar && self.burst == 0 {
                self.burst = 30;
                SetTimer(Some(self.hwnd), T_ONTOP, 16, None);
            }
        }
    }

    /// Attached to the taskbar: may the island be inside it now? Only while
    /// closed (aiming for closed, with `settled`: there already) and no
    /// taller than the taskbar.
    fn fits_taskbar(&self, settled: bool) -> bool {
        if self.dock_tb.is_invalid() || self.suppressed {
            return false;
        }
        let band = self.dock_band + 0.5;
        let aim = !self.expanded && self.trans.is_none() && !self.search_popup() && !self.cv_open && self.a_col.target >= 0.999 && self.h.target <= band;
        aim && (!settled || (self.a_col.v > 0.985 && self.h.v <= band && self.a_peek.v < 0.02 && self.a_exp.v < 0.02 && self.srch_a.v < 0.02 && self.tr_a.v < 0.02))
    }

    /// Before drawing: about to be bigger than the taskbar? Out of it first.
    fn dock_before(&mut self) {
        if self.docked && !self.fits_taskbar(false) {
            self.undock(false);
        }
    }

    /// After drawing: closed and settled? Back into the taskbar.
    fn dock_after(&mut self) {
        if !self.docked && self.fits_taskbar(true) {
            self.dock();
        }
    }

    /// Attached: the window in the taskbar went away (Explorer restarted)?
    /// Then the island shows in its own window until it can go back in.
    pub fn check_dock(&mut self) {
        self.keep_child_on_top();
        let ok = unsafe { !self.docked || (IsWindow(Some(self.dock_child)).as_bool() && IsWindow(Some(self.dock_tb)).as_bool()) };
        if !ok {
            util::log("island: its place in the taskbar went away (Explorer restarted?); showing it on its own until the taskbar is back");
            self.undock(true);
            self.dock_child = HWND::default();
            self.relayout();
        }
    }

    /// Where the window inside the taskbar goes (the taskbar's coordinates):
    /// exactly where the island's own window would be on screen.
    fn place_docked(&mut self) {
        unsafe {
            let mut r = RECT::default();
            if GetWindowRect(self.dock_tb, &mut r).is_err() {
                return;
            }
            let (ww, wh) = (self.win.right - self.win.left, self.win.bottom - self.win.top);
            let band = r.bottom - r.top;
            let y = if self.up { band - wh } else { 0 };
            let _ = SetWindowPos(self.dock_child, Some(HWND_TOP), self.win.left - r.left, y, ww, wh, SWP_NOACTIVATE);
        }
    }

    /// The window inside the taskbar (made when first needed, and again
    /// after Explorer restarted and took the old one with it).
    fn ensure_child(&mut self) -> bool {
        unsafe {
            if !self.dock_child.is_invalid() && IsWindow(Some(self.dock_child)).as_bool() && GetParent(self.dock_child).ok() == Some(self.dock_tb) {
                return true;
            }
            self.dock_child = HWND::default();
            if !IsWindow(Some(self.dock_tb)).as_bool() || self.dock_wait.map(|t| Instant::now() < t).unwrap_or(false) {
                return false;
            }
            // (No parent notifications: Explorer is never asked anything that
            // could make Phemio wait on it.)
            match CreateWindowExW(WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOPARENTNOTIFY, DOCK_CLASS, w!("Phemio"), WS_CHILD | WS_CLIPSIBLINGS, 0, 0, 1, 1, Some(self.dock_tb), None, Some(util::hinstance()), None) {
                Ok(h) => {
                    self.dock_child = h;
                    self.dock_wait = None;
                    // Files can be dropped on it there too (the shelf).
                    crate::shelf::register(h);
                    true
                }
                Err(e) => {
                    util::log(&format!("island: couldn't go into the taskbar: {e}"));
                    self.dock_wait = Some(Instant::now() + std::time::Duration::from_secs(10));
                    false
                }
            }
        }
    }

    fn drop_child(&mut self) {
        unsafe {
            if !self.dock_child.is_invalid() && IsWindow(Some(self.dock_child)).as_bool() {
                crate::shelf::revoke(self.dock_child);
                let _ = DestroyWindow(self.dock_child);
            }
        }
        self.dock_child = HWND::default();
    }

    /// Into the taskbar: shown by the window there, the island's own hidden.
    fn dock(&mut self) {
        if !self.ensure_child() {
            return;
        }
        self.docked = true;
        self.show_main = false;
        if std::env::var("ISLET_LOG_DOCK").is_ok() {
            util::log("island: into the taskbar");
        }
        self.place_docked();
        self.render();
        unsafe {
            if !self.suppressed {
                let _ = SetWindowPos(self.dock_child, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            }
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.sync_hover();
    }

    /// Out of the taskbar: the island's own window again (above the
    /// taskbar). `now`: at once; otherwise once this frame is drawn into it.
    fn undock(&mut self, now: bool) {
        self.docked = false;
        if std::env::var("ISLET_LOG_DOCK").is_ok() {
            util::log("island: out of the taskbar");
        }
        self.show_main = true;
        if now {
            self.finish_undock();
        }
    }

    fn finish_undock(&mut self) {
        if !self.show_main {
            return;
        }
        self.show_main = false;
        unsafe {
            let w = self.win;
            let show = if self.suppressed { SWP_NOACTIVATE } else { SWP_NOACTIVATE | SWP_SHOWWINDOW };
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), w.left, w.top, w.right - w.left, w.bottom - w.top, show);
            if !self.dock_child.is_invalid() {
                let _ = ShowWindow(self.dock_child, SW_HIDE);
            }
        }
        self.sync_hover();
    }

    /// The two windows swapped under the pointer: whether it's on the island
    /// now, watched by the window it's over (no hover-to-open from the swap).
    fn sync_hover(&mut self) {
        unsafe {
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            let w = self.win;
            let s = self.scale;
            // The island's shape, roughly (its window is larger than it).
            let cx = (w.left + w.right) / 2;
            let half = (self.w.v * s / 2.0) as i32;
            let (top, bottom) = if self.up { (w.bottom - ((self.h.v + 12.0) * s) as i32, w.bottom) } else { (w.top, w.top + ((self.top + self.h.v + 6.0) * s) as i32) };
            let on = p.x >= cx - half && p.x <= cx + half && p.y >= top && p.y <= bottom;
            self.hover = on;
            if on {
                let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.input_hwnd(), dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut tme);
                let _ = KillTimer(Some(self.hwnd), T_COLLAPSE);
            }
        }
    }

    /// Explorer may stack its own windows over ours inside the taskbar: back on top.
    fn keep_child_on_top(&self) {
        if !self.docked || self.suppressed || self.dock_child.is_invalid() {
            return;
        }
        unsafe {
            // Never both at once: the island's own window stays hidden.
            if IsWindowVisible(self.hwnd).as_bool() {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            // Only a window of Explorer's that's showing and actually over
            // the island counts: re-stacking for nothing (it used to be done
            // on every frame) makes the taskbar repaint, which flickers.
            let mut mine = RECT::default();
            let _ = GetWindowRect(self.dock_child, &mut mine);
            let mut above = false;
            let mut h = GetWindow(self.dock_child, GW_HWNDPREV).unwrap_or_default();
            let mut n = 0;
            while !h.is_invalid() && n < 64 {
                let mut r = RECT::default();
                if IsWindowVisible(h).as_bool() && GetWindowRect(h, &mut r).is_ok() && r.left < mine.right && r.right > mine.left && r.top < mine.bottom && r.bottom > mine.top {
                    above = true;
                    break;
                }
                h = GetWindow(h, GW_HWNDPREV).unwrap_or_default();
                n += 1;
            }
            if above || !IsWindowVisible(self.dock_child).as_bool() {
                let _ = SetWindowPos(self.dock_child, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            }
        }
    }

    /// The window that gets the pointer right now (the one in the taskbar
    /// while docked).
    fn input_hwnd(&self) -> HWND {
        if self.docked && !self.dock_child.is_invalid() {
            self.dock_child
        } else {
            self.hwnd
        }
    }

    fn raise_topmost(&self) {
        if self.docked {
            return;
        }
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }

    fn force_topmost(&self) {
        if self.docked {
            return;
        }
        unsafe {
            let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
            let _ = SetWindowPos(self.hwnd, Some(HWND_NOTOPMOST), 0, 0, 0, 0, flags);
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, flags | SWP_SHOWWINDOW);
        }
    }

    pub fn raise(&self) {
        if self.suppressed || self.docked || !config::cur().b("island_top") {
            return;
        }
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }

    fn snap(&mut self) {
        for s in [&mut self.w, &mut self.h, &mut self.a_col, &mut self.a_peek, &mut self.a_exp] {
            s.snap(s.target);
        }
    }

    pub fn retarget(&mut self) {
        let (w, h, col, pk, ex) = if self.trans.is_some() {
            (ew(), self.trans_height(), 0.0, 0.0, 0.0)
        } else if self.search_popup() {
            (ew(), self.search_height(), 0.0, 0.0, 0.0)
        } else if self.expanded {
            // (The page dock opening beyond the dots: the island grows for it.)
            let ext = if dock_extends().is_some() && self.pbar.target > 0.5 { dock_room() } else { 0.0 };
            (ew(), eh() + ext, 0.0, 0.0, 1.0)
        } else if let Some(p) = &self.peek {
            let (w, h) = p.size();
            (w, h, 0.0, 1.0, 0.0)
        } else {
            let (w, h) = self.collapsed_size();
            (w, h, 1.0, 0.0, 0.0)
        };
        self.w.set(w);
        self.h.set(h);
        self.a_col.set(col);
        self.a_peek.set(pk);
        self.a_exp.set(ex);
        self.kick();
        self.update_timers();
    }

    fn kick(&mut self) {
        if !self.animating {
            self.animating = true;
            self.last_tick = Instant::now();
            frame_clock::start(self.hwnd);
        }
    }

    /// OLED care (Settings → Island → Pixel drift): once a minute, move one
    /// pixel towards a spot up to 3 pixels away; a few minutes after getting
    /// there, pick another. Only while closed, so nothing moves under the
    /// pointer. A notch stays against its edge (sideways only); an island
    /// only moves away from the edge it hangs from.
    fn drift_step(&mut self) {
        let c = config::cur();
        if !c.b("pix_drift") {
            if self.drift != (0, 0) {
                self.drift = (0, 0);
                self.drift_to = (0, 0);
                self.render();
            }
            return;
        }
        if self.expanded || self.peek.is_some() || self.hover || self.suppressed {
            return;
        }
        if self.drift == self.drift_to {
            self.drift_min += 1;
            if self.drift_min < 3 + util::rand_u32() % 3 {
                return;
            }
            self.drift_min = 0;
            let r = util::rand_u32();
            let dx = (r % 7) as i32 - 3;
            let dy = if self.notch() {
                0
            } else {
                let v = ((r >> 8) % 3) as i32;
                if self.up { -v } else { v }
            };
            self.drift_to = (dx, dy);
            if self.drift_to == self.drift {
                return;
            }
        }
        let step = |a: i32, b: i32| a + (b - a).signum();
        // One axis at a time: a single-pixel nudge.
        if self.drift.0 != self.drift_to.0 {
            self.drift.0 = step(self.drift.0, self.drift_to.0);
        } else {
            self.drift.1 = step(self.drift.1, self.drift_to.1);
        }
        self.render();
    }

    fn eq_wanted(&self) -> bool {
        let c = config::cur();
        let visible = (!self.expanded && self.mascot == Mascot::Off && !c.b("minimal")) || matches!(&self.peek, Some(p) if p.kind == PeekKind::Track);
        c.b("island_eq") && self.media.playing && self.media.has_session && !self.suppressed && visible
    }

    /// Start/stop the periodic timers so nothing ticks unless it's on screen.
    pub fn update_timers(&mut self) {
        unsafe {
            let h = Some(self.hwnd);
            if self.eq_wanted() {
                SetTimer(h, T_EQ, 50, None);
            } else {
                let _ = KillTimer(h, T_EQ);
            }
            let d = crate::data();
            let c = config::cur();
            let closed_secs = !self.expanded
                && self.peek.is_none()
                && self.mascot == Mascot::Off
                && !c.b("minimal")
                && ((c.i("col_center") == 0 && c.b("clock_secs")) || c.i("col_center") >= 2);
            let need_second = !self.suppressed
                && ((self.expanded && ((self.page == Page::Home && self.media.playing) || self.page == Page::Clock || self.page == Page::Calendar || self.page == Page::Focus))
                    || (d.focus.running && (self.expanded || !c.b("minimal")))
                    || closed_secs);
            if need_second {
                SetTimer(h, T_SECOND, 1000, None);
            } else {
                let _ = KillTimer(h, T_SECOND);
            }
            if self.edit.is_some() {
                SetTimer(h, T_CARET, 530, None);
            } else {
                let _ = KillTimer(h, T_CARET);
            }
            // Arm the cat's next act only once per act (re-arming here on
            // every animation end used to stretch the gaps).
            match self.mascot {
                Mascot::Awake if !self.act_armed => {
                    self.act_armed = true;
                    SetTimer(h, T_BLINK, 450 + util::rand_u32() % 900, None);
                }
                Mascot::Doze if !self.act_armed => {
                    self.act_armed = true;
                    SetTimer(h, T_BLINK, 1700, None);
                }
                Mascot::Off => {
                    self.act_armed = false;
                    let _ = KillTimer(h, T_BLINK);
                }
                _ => {}
            }
        }
    }

    // ------------------------------------------------------------------
    // External events
    // ------------------------------------------------------------------

    pub fn on_media(&mut self) {
        let info = media::snapshot();
        let c = config::cur();
        let title_changed = info.title != self.media.title || info.artist != self.media.artist;
        if info.art_id != self.art_id {
            self.art_id = info.art_id;
            // Decoded once, big enough for the largest cover size.
            self.art = info.art.as_ref().and_then(|b| Image::from_bytes(b, (240.0 * self.scale) as u32, 0.10).ok());
            // Not square (a video's 16:9 frame): keep it whole too, to fit inside the square.
            self.art_fit = info.art.as_ref().and_then(|b| Image::from_bytes_fit(b, (320.0 * self.scale) as u32, 0.06).ok()).filter(|i| (i.w as f32 / i.h.max(1) as f32 - 1.0).abs() > 0.06);
            self.art_blur = match (&self.art_fit, &info.art) {
                (Some(_), Some(b)) => Image::blurred_square(b, (240.0 * self.scale) as u32, 0.10).ok(),
                _ => None,
            };
            self.accent = self.art.as_ref().map(accent_of).unwrap_or(rgb(0x34c759, 1.0));
        }
        let was_loaded = self.media_loaded;
        self.media = info;
        self.media_loaded = true;
        if was_loaded && title_changed && c.b("osd_track") && !self.media.title.is_empty() && self.media.playing && !self.expanded {
            let mut p = Peek::rich("track", "", 0, None, &self.media.title, &self.media.artist, "", 3200, Some(Page::Home));
            p.kind = PeekKind::Track;
            self.show_peek(p);
        } else {
            self.retarget();
        }
        self.render();
    }

    /// Show an alert. Same-category alerts update in place; others queue.
    pub fn show_peek(&mut self, p: Peek) {
        if self.suppressed {
            return;
        }
        let transient = matches!(p.kind, PeekKind::Bar | PeekKind::Track);
        if self.expanded || self.search_popup() || self.trans.is_some() {
            if !transient {
                self.queue.push_back(p);
            }
            return;
        }
        if let Some(cur) = &self.peek {
            if cur.cat != p.cat && !transient {
                if !self.queue.iter().any(|q| q.cat == p.cat && q.title == p.title) {
                    self.queue.push_back(p);
                }
                // Others are waiting: the one on screen only stays a moment.
                let shown = self.peek_at.map(|t| t.elapsed().as_millis() as u32).unwrap_or(0);
                unsafe { SetTimer(Some(self.hwnd), T_PEEK, BURST_MS.saturating_sub(shown).max(10), None) };
                return;
            }
        }
        // In a burst every popup but the last gets a short turn.
        let ms = if self.queue.is_empty() { p.ms } else { BURST_MS };
        let switching = self.peek.as_ref().map(|c| c.cat != p.cat).unwrap_or(false) || self.pk_a.target < 0.5;
        self.peek = Some(p.clone());
        self.last_peek = Some(p);
        self.peek_at = Some(Instant::now());
        if switching {
            // Fade the new content in.
            self.pk_a.snap(0.0);
            self.pk_a.set(1.0);
            self.kick();
        }
        unsafe { SetTimer(Some(self.hwnd), T_PEEK, ms, None) };
        self.retarget();
        self.render();
    }

    fn next_peek(&mut self) {
        if !self.queue.is_empty() && self.peek.is_some() {
            // Fade this one out, then bring in the next (see T_PEEK_SWAP).
            self.pk_a.set(0.0);
            self.kick();
            unsafe { SetTimer(Some(self.hwnd), T_PEEK_SWAP, 70, None) };
            return;
        }
        self.peek = None;
        self.peek_at = None;
        if let Some(n) = self.queue.pop_front() {
            self.show_peek(n);
        } else {
            self.retarget();
        }
    }

    /// The closed island must show its content. If it's ever caught half
    /// faded (or with the mascot gone but its fade left up) for a few
    /// seconds, put it right, redraw, and note what was wrong in the log.
    /// One line about the island's state (Settings → About → Copy diagnostics).
    pub fn diagnostics(&self) -> String {
        format!(
            "island: exp={} peek={} search={} trans={} supp={} anim={} col={:.2}->{:.2} pk={:.2} ex={:.2} w={:.0} h={:.0} mascot={:?} fade={:.2} page={:?} dark={}",
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
            self.masc_fade.v,
            self.page,
            self.dark
        )
    }

    fn heal(&mut self) {
        let closed = !self.expanded && self.peek.is_none() && self.search.is_none() && self.trans.is_none() && !self.suppressed;
        if !closed {
            self.blank_since = None;
            return;
        }
        let wrong = (self.a_col.target - 1.0).abs() > 0.01
            || (self.a_col.v < 0.95 && !self.animating)
            || self.a_exp.v > 0.05 && !self.animating
            || self.a_peek.v > 0.05 && !self.animating
            || (self.mascot == Mascot::Off && self.masc_fade.v > 0.05 && !self.animating)
            || (self.mascot == Mascot::Off && self.masc_fade.target > 0.5)
            || !self.a_col.v.is_finite()
            || !self.w.v.is_finite()
            || !self.h.v.is_finite();
        if !wrong {
            self.blank_since = None;
            return;
        }
        let since = *self.blank_since.get_or_insert_with(Instant::now);
        if since.elapsed().as_millis() < 3000 {
            return;
        }
        util::log(&format!(
            "island: blank closed island put right (col={:.2}->{:.2} pk={:.2} ex={:.2} w={:.0} h={:.0} mascot={:?} fade={:.2} anim={})",
            self.a_col.v, self.a_col.target, self.a_peek.v, self.a_exp.v, self.w.v, self.h.v, self.mascot, self.masc_fade.v, self.animating
        ));
        self.blank_since = None;
        if self.mascot == Mascot::Off {
            self.masc_fade.snap(0.0);
        }
        let (w, h) = self.collapsed_size();
        self.w.snap(w);
        self.h.snap(h);
        self.a_col.snap(1.0);
        self.a_peek.snap(0.0);
        self.a_exp.snap(0.0);
        self.animating = false;
        self.relayout();
        self.retarget();
        self.render();
    }

    pub fn set_suppressed(&mut self, s: bool) {
        if s == self.suppressed {
            return;
        }
        self.suppressed = s;
        unsafe {
            // In the taskbar it's the window there that hides / shows.
            let w = if self.docked && !self.dock_child.is_invalid() { self.dock_child } else { self.hwnd };
            let _ = ShowWindow(w, if s { SW_HIDE } else { SW_SHOWNA });
        }
        self.update_timers();
        if !s {
            self.render();
        }
    }

    /// The mascot is on show in "Always" mode (Settings → Island → Mascot).
    pub(crate) fn mascot_always(&self) -> bool {
        self.mascot != Mascot::Off && config::cur().i("mascot") == 2
    }

    /// Expand the island on the page matching the part of the closed island
    /// under the pointer (or the last page, elsewhere).
    fn open_at_pointer(&mut self) {
        if self.expanded || self.mascot_always() {
            self.set_expanded(true);
            return;
        }
        let (px, py) = self.last_pt;
        let want = self.col_zones.iter().find(|(_, r)| inside(r, px, py)).map(|(p, _)| *p).or_else(|| {
            // Empty space: the last page that isn't one of those with their own spot.
            let free = ALL_PAGES.get(config::cur().i("free_page").max(0) as usize).copied().filter(|p| !own_spot(*p) && enabled_pages().contains(p));
            // None used yet: the first page (in your order) without its own spot.
            free.or_else(|| ordered_pages().into_iter().find(|p| !own_spot(*p) && enabled_pages().contains(p))).or(Some(Page::Home))
        });
        if let Some(p) = want.filter(|p| enabled_pages().contains(p)) {
            self.go_page(p, 0);
            self.slide.snap(0.0);
            self.slide_other = None;
        }
        self.set_expanded(true);
    }

    /// The mascot leaves: it waves goodbye first, then fades out.
    pub fn say_bye(&mut self) {
        if matches!(self.mascot, Mascot::Awake | Mascot::Doze | Mascot::Hello) {
            self.set_mascot(Mascot::Bye);
        }
    }

    pub fn set_mascot(&mut self, m: Mascot) {
        if m == self.mascot {
            return;
        }
        self.mascot = m;
        self.act_armed = false;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), T_BLINK);
        }
        self.masc_fade.set(if m == Mascot::Off { 0.0 } else { 1.0 });
        if m != Mascot::Off {
            self.masc_last = m;
        }
        match m {
            Mascot::Awake => {
                self.look.snap(0.0);
                self.tilt.snap(0.0);
                self.ear.snap(0.0);
                self.breath.snap(0.0);
                self.walk.snap(0.0);
                self.look_y.snap(0.0);
                self.flick.snap(0.0);
                self.hop.snap(0.0);
                self.follow = 0;
                self.eye_mode = 0;
                self.smile.snap(1.0);
                self.mouth_o = false;
                self.wag.snap(0.0);
            }
            Mascot::Doze => {
                self.smile.set(0.35);
                self.mouth_o = false;
                self.look.set(0.0);
                self.look_y.set(0.0);
                self.tilt.set(0.0);
                self.walk.set(0.0);
                self.ear.set(0.7);
                self.eye_mode = 0;
            }
            Mascot::Hello => {
                // Noticed you: ears up, eyes wide, then the island returns.
                self.tilt.set(0.0);
                self.look.set(0.0);
                self.look_y.set(0.0);
                self.eye_mode = 0;
                self.ear.v = 0.8;
                self.ear.set(-0.8);
                self.smile.set(1.9);
                self.mouth_o = false;
                self.blink.snap(1.0);
                unsafe { SetTimer(Some(self.hwnd), T_BLINK, 650, None) };
            }
            Mascot::Bye => {
                self.bye_at = Instant::now();
                self.tilt.set(0.0);
                self.look.set(0.0);
                self.look_y.set(0.0);
                self.eye_mode = 0;
                self.mouth_o = false;
                self.smile.set(2.0);
                self.blink.snap(1.0);
                self.ear.v = 0.6;
                self.ear.set(-0.6);
                let style = config::cur().i("mascot_style");
                let ms = match style {
                    1 => (mascot::CAT_BYE_S * 1000.0) as u32 + 80,
                    2 => (mascot::DUEL_BYE_S * 1000.0) as u32 + 80,
                    _ => BYE_MS,
                };
                if style == 2 {
                    // The final round: who wins is decided right now.
                    self.duel_winner = (util::rand_u32() % 2) as u8;
                    let key = if self.duel_winner == 0 { "duel_cat" } else { "duel_bot" };
                    config::update(|c| {
                        let n = c.i(key);
                        c.set(key, n + 1)
                    });
                    util::log(&format!("mascot: final round, {} wins", if self.duel_winner == 0 { "cat" } else { "robot" }));
                }
                unsafe { SetTimer(Some(self.hwnd), T_BYE, ms, None) };
            }
            Mascot::Off => {}
        }
        self.kick();
        self.retarget();
        self.update_timers();
        self.render();
    }

    /// Something in the shared data changed (focus, to-do, weather…).
    pub fn refresh(&mut self) {
        self.retarget();
        self.render();
    }

    /// Alt+A: Now Playing open (closes again by itself a few seconds later
    /// unless the pointer is on it); pressed again while it's open: closed.
    pub fn toggle_now_playing(&mut self) {
        if self.expanded && self.page == Page::Home {
            self.set_expanded(false);
            return;
        }
        if !enabled_pages().contains(&Page::Home) {
            return;
        }
        self.go_page(Page::Home, 0);
        self.slide.snap(0.0);
        self.slide_other = None;
        self.set_expanded(true);
        if !self.hover {
            unsafe {
                SetTimer(Some(self.hwnd), T_COLLAPSE, 5000, None);
            }
        }
    }

    pub fn open_page(&mut self, p: Page) {
        if !enabled_pages().contains(&p) {
            return;
        }
        self.go_page(p, 0);
        self.set_expanded(true);
    }

    pub fn drag_hover(&mut self, on: bool) {
        self.drag_in = on;
        if on {
            let _ = unsafe { KillTimer(Some(self.hwnd), T_COLLAPSE) };
            if enabled_pages().contains(&Page::Shelf) {
                self.go_page(Page::Shelf, 0);
            }
            self.set_expanded(true);
        } else if !self.hover {
            unsafe { SetTimer(Some(self.hwnd), T_COLLAPSE, 600, None) };
        }
        self.render();
    }

    pub fn drop_files(&mut self, files: Vec<String>) {
        self.drag_in = false;
        if !files.is_empty() {
            crate::data().shelf.add(files);
            self.shelf_scroll = 0.0;
        }
        self.render();
    }

    // ------------------------------------------------------------------
    // Pages
    // ------------------------------------------------------------------

    /// Look up the speakers playing together and the microphones again.
    pub fn mo_refresh(&mut self) {
        self.mo_cluster = crate::multiout::cluster();
        self.mo_levels();
        // Back to the plain volume row when fewer than two play together.
        if self.mo_cluster.is_empty() && self.cv_open {
            self.set_cv(false);
        }
        self.in_list = crate::audio::list_inputs();
        if self.out_menu {
            self.out_list = crate::multiout::outputs();
        }
    }

    /// Read the grouped speakers' levels again; the sliders glide to them.
    pub fn mo_levels(&mut self) {
        let ids: Vec<String> = self.mo_cluster.iter().map(|c| c.0.clone()).collect();
        self.mo_lv = crate::multiout::levels(&ids);
        if self.mo_show.len() != self.mo_lv.len() {
            self.mo_show = self.mo_lv.iter().map(|l| l.0).collect();
        }
        if self.mo_show.iter().zip(&self.mo_lv).any(|(a, b)| (a - b.0).abs() > 0.001) {
            self.kick();
        } else {
            self.render();
        }
    }

    /// Glide the shown levels toward the real ones; true while moving.
    fn ease_levels(&mut self, dt: f32) -> bool {
        let k = 1.0 - (-dt * 16.0).exp();
        let mut moving = false;
        for (s, t) in self.mo_show.iter_mut().zip(&self.mo_lv) {
            let d = t.0 - *s;
            if d.abs() < 0.002 {
                *s = t.0;
            } else {
                *s += d * k;
                moving = true;
            }
        }
        moving
    }

    /// The volume the main slider shows: the group's (average) while several
    /// speakers play together, else the default output's.
    pub(crate) fn shown_volume(&self) -> (f32, bool) {
        if self.mo_cluster.len() >= 2 && self.mo_show.len() == self.mo_cluster.len() {
            let shown: Vec<(f32, bool)> = self.mo_show.iter().zip(&self.mo_lv).map(|(v, l)| (*v, l.1)).collect();
            return crate::multiout::group_level(&shown);
        }
        crate::data().volume
    }

    /// Mute button: the whole group while several play together.
    fn mute_clicked(&mut self) {
        if self.mo_cluster.len() >= 2 {
            let m = !self.mo_lv.iter().all(|l| l.1);
            for (id, _) in &self.mo_cluster {
                crate::multiout::set_dev_mute(id, m);
            }
            self.mo_levels();
        } else {
            crate::app().toggle_mute();
        }
    }

    /// Has the taskbar moved, changed size, or been made again (Explorer
    /// restarted)? Then lay the island out again. Also keeps the event hook
    /// on the current taskbar window and the 0.3 s check running.
    pub(crate) fn watch_taskbar(&mut self) {
        use std::sync::atomic::Ordering::Relaxed;
        TB_ISLAND.store(self.hwnd.0 as isize, Relaxed);
        unsafe {
            SetTimer(Some(self.hwnd), T_TBPOS, 300, None);
        }
        let now = util::taskbar_rect().map(|(h, r)| (h.0 as isize, r));
        let same = match (&now, &self.tb_seen) {
            (Some(a), Some(b)) => a.0 == b.0 && a.1 == b.1,
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        let prev = self.tb_seen;
        let new_window = now.map(|n| n.0) != self.tb_seen.map(|s| s.0);
        let first = self.tb_seen.is_none() && self.tb_hook.is_none();
        self.tb_seen = now;
        if new_window {
            // Watch the (new) taskbar window's moves as they happen.
            if let Some(h) = self.tb_hook.take() {
                unsafe {
                    let _ = windows::Win32::UI::Accessibility::UnhookWinEvent(h);
                }
            }
            if let Some((tb, _)) = now {
                TB_WATCHED.store(tb, Relaxed);
                unsafe {
                    let mut pid = 0u32;
                    let thread = GetWindowThreadProcessId(HWND(tb as *mut _), Some(&mut pid));
                    let h = windows::Win32::UI::Accessibility::SetWinEventHook(
                        EVENT_OBJECT_LOCATIONCHANGE,
                        EVENT_OBJECT_LOCATIONCHANGE,
                        None,
                        Some(on_taskbar_moved),
                        pid,
                        thread,
                        WINEVENT_OUTOFCONTEXT,
                    );
                    self.tb_hook = (!h.is_invalid()).then_some(h);
                }
            }
        }
        // Inside the taskbar, a move (auto-hide sliding) carries the island
        // along by itself; only a new size or a new taskbar needs a new layout.
        let resized = match (&now, &prev) {
            (Some(a), Some(b)) => a.1.right - a.1.left != b.1.right - b.1.left || a.1.bottom - a.1.top != b.1.bottom - b.1.top,
            _ => true,
        };
        if !first && (!self.docked || new_window || resized) {
            self.relayout();
        }
        // Moved or remade, the taskbar may have come up over the island.
        if self.on_taskbar && !self.suppressed && config::cur().b("island_top") {
            self.raise_topmost();
            unsafe {
                SetTimer(Some(self.hwnd), T_ONTOP, 16, None);
            }
            self.burst = self.burst.max(10);
        }
    }

    /// Open / close the speakers' volume card.
    pub(crate) fn set_cv(&mut self, open: bool) {
        self.cv_open = open;
        self.cv_anim.set(if open { 1.0 } else { 0.0 });
        self.kick();
    }

    fn go_page(&mut self, p: Page, dir: i32) {
        if p == self.page {
            return;
        }
        if self.cv_open {
            self.cv_open = false;
            self.cv_anim.snap(0.0);
        }
        let ord = ordered_pages();
        let pos = |x: Page| ord.iter().position(|y| *y == x).unwrap_or(0);
        let d = if dir != 0 { dir } else if pos(p) > pos(self.page) { 1 } else { -1 };
        self.end_edit(false);
        let old = self.page;
        self.page = p;
        // The new page comes in from the side while the old one goes out the
        // other way. (A swipe that let go part-way continues from there.)
        let from = match self.slide_other {
            Some((q, rel)) if self.swipe_on && q == p => (self.slide.v + rel).clamp(-1.0, 1.0),
            _ => d as f32,
        };
        self.swipe_on = false;
        self.slide.snap(from);
        self.slide.set(0.0);
        self.slide_other = Some((old, -(d as f32)));
        // Clock & Calendar, Devices and Controls each have their own spot on
        // the closed island; the empty space opens the last of the others.
        // (Now Playing too: it has the music wave, the empty left side when
        // nothing plays, and Alt+A.)
        let free = !own_spot(p);
        config::update(|c| {
            c.set("last_page", p.index() as i32);
            if free {
                c.set("free_page", p.index() as i32);
            }
        });
        if p == Page::Weather || p == Page::Clock {
            crate::app().refresh_weather(false);
        }
        if p == Page::Devices {
            crate::bt::spawn_scan(crate::app().host, crate::WM_BT);
        }
        if p == Page::Devices || p == Page::Controls {
            self.mo_refresh();
        }
        self.out_menu = false;
        self.out_anim.snap(0.0);
        if self.ctl_list != 0 {
            self.close_ctl_list(true);
        }
        if self.search.is_some() && p != Page::Search {
            self.close_search();
        }
        if p == Page::Calendar {
            self.cal_month = None;
            self.cal_lmonth = None;
            self.cal_sel = None;
            self.cal_fade.snap(0.0);
            // The second city's time comes with the weather lookup.
            crate::app().refresh_weather(false);
        }
        self.kick();
        self.update_timers();
        self.render();
    }

    /// The page `d` steps away (wrapping around).
    fn neighbour(&self, d: i32) -> Option<Page> {
        let pages = enabled_pages();
        if pages.len() < 2 {
            return None;
        }
        let i = pages.iter().position(|p| *p == self.page).unwrap_or(0) as i32;
        Some(pages[(i + d).rem_euclid(pages.len() as i32) as usize])
    }

    /// Touchpad swipe: the pages follow the fingers; letting go past a third
    /// of the way (or swiping all the way) turns the page, otherwise it
    /// springs back. One page per swipe — the momentum after lifting the
    /// fingers doesn't turn more.
    fn swipe(&mut self, d: i32) {
        unsafe {
            SetTimer(Some(self.hwnd), T_SWIPE, if self.swipe_done { 220 } else { 110 }, None);
        }
        if self.swipe_done {
            return;
        }
        // (Wheel units for a whole page: about a hand's swipe on a touchpad.)
        const FULL: f32 = 700.0;
        if !self.swipe_on {
            self.swipe_on = true;
            self.swipe_acc = 0.0;
        }
        self.swipe_acc += d as f32;
        let p = (self.swipe_acc / FULL).clamp(-1.0, 1.0);
        let dir = if p >= 0.0 { 1 } else { -1 };
        let Some(next) = self.neighbour(dir) else { return };
        if p.abs() >= 0.98 {
            // All the way: turn now.
            self.swipe_done = true;
            self.go_page(next, dir);
            return;
        }
        // Current page moves with the fingers; the next one shows beside it.
        self.slide.snap(-p);
        self.slide_other = Some((next, dir as f32));
        self.kick();
        self.render();
    }

    /// The fingers lifted (or the momentum ended).
    fn swipe_end(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), T_SWIPE);
        }
        if self.swipe_done {
            self.swipe_done = false;
            self.swipe_on = false;
            return;
        }
        if !self.swipe_on {
            return;
        }
        let p = (self.swipe_acc / 700.0).clamp(-1.0, 1.0);
        let dir = if p >= 0.0 { 1 } else { -1 };
        match self.neighbour(dir) {
            Some(next) if p.abs() > 0.33 => self.go_page(next, dir),
            _ => {
                // Back where it was.
                self.swipe_on = false;
                self.slide.set(0.0);
                self.kick();
            }
        }
        self.swipe_on = false;
        self.swipe_acc = 0.0;
    }

    fn step_page(&mut self, d: i32) {
        let pages = enabled_pages();
        if pages.len() < 2 {
            return;
        }
        let i = pages.iter().position(|p| *p == self.page).unwrap_or(0) as i32;
        let n = pages.len() as i32;
        let ni = (i + d).rem_euclid(n) as usize;
        self.go_page(pages[ni], d.signum());
    }

    // ------------------------------------------------------------------
    // Text editing (to-do, location, time zone city)
    // ------------------------------------------------------------------

    fn take_focus(&mut self) {
        unsafe {
            let ex = GetWindowLongW(self.hwnd, GWL_EXSTYLE) as u32;
            SetWindowLongW(self.hwnd, GWL_EXSTYLE, (ex & !WS_EX_NOACTIVATE.0) as i32);
            self.focused = true;
            let _ = SetForegroundWindow(self.hwnd);
            let _ = SetFocus(Some(self.hwnd));
        }
    }

    fn release_focus(&mut self) {
        unsafe {
            let ex = GetWindowLongW(self.hwnd, GWL_EXSTYLE) as u32;
            SetWindowLongW(self.hwnd, GWL_EXSTYLE, (ex | WS_EX_NOACTIVATE.0) as i32);
        }
        self.focused = false;
    }

    pub(crate) fn begin_edit(&mut self, t: EditTarget) {
        let c = config::cur();
        self.edit_all = false;
        self.edit_buf = match t {
            EditTarget::Todo => std::mem::take(&mut self.todo_draft),
            EditTarget::WeatherCity => c.s("weather_city"),
            EditTarget::TzCity => c.s("tz2_city"),
            EditTarget::WifiPass => std::mem::take(&mut self.wifi_pass),
            EditTarget::WifiUser => self.wifi_user.clone(),
            EditTarget::Search => String::new(),
            EditTarget::LinkAddr => crate::link::view().map(|v| v.manual).unwrap_or_default(),
            EditTarget::MoDelay => crate::multiout::target(&self.mo_edit_id).delay.to_string(),
            EditTarget::RdpHost => self.rdp.host.clone(),
            EditTarget::RdpUser => self.rdp.user.clone(),
            EditTarget::RdpPass => self.rdp.pass.clone(),
            EditTarget::Note => self.note_sel.and_then(|id| crate::data().notes.get(id).map(|n| n.text.clone())).unwrap_or_default(),
        };
        self.edit_cur = None;
        self.edit = Some(t);
        self.caret = true;
        self.take_focus();
        self.update_timers();
        self.render();
    }

    // ---- The caret in a one-line field ----

    /// The caret's place in `edit_buf` (kept valid whatever changed the text).
    pub(crate) fn cur_pos(&self) -> usize {
        let n = self.edit_buf.len();
        match self.edit_cur {
            Some(c) if c < n && self.edit_buf.is_char_boundary(c) => c,
            _ => n,
        }
    }

    fn set_cur(&mut self, p: usize) {
        self.edit_cur = (p < self.edit_buf.len()).then_some(p);
    }

    /// Type / paste at the caret.
    fn ins_str(&mut self, s: &str) {
        let p = self.cur_pos();
        self.edit_buf.insert_str(p, s);
        self.set_cur(p + s.len());
    }

    /// Backspace: the character before the caret.
    fn back_char(&mut self) {
        let p = self.cur_pos();
        if let Some((i, _)) = self.edit_buf[..p].char_indices().last() {
            self.edit_buf.replace_range(i..p, "");
            self.set_cur(i);
        }
    }

    /// Delete: the character after the caret.
    fn del_char(&mut self) {
        let p = self.cur_pos();
        if let Some(c) = self.edit_buf[p..].chars().next() {
            self.edit_buf.replace_range(p..p + c.len_utf8(), "");
            self.set_cur(p);
        }
    }

    /// Where a word jump from the caret lands (Ctrl+←/→, Ctrl+Backspace).
    fn word_from(&self, p: usize, back: bool) -> usize {
        let b = &self.edit_buf;
        if back {
            let before: Vec<(usize, char)> = b[..p].char_indices().collect();
            let mut k = before.len();
            while k > 0 && before[k - 1].1.is_whitespace() {
                k -= 1;
            }
            while k > 0 && !before[k - 1].1.is_whitespace() {
                k -= 1;
            }
            before.get(k).map(|x| x.0).unwrap_or(if k == 0 { 0 } else { p })
        } else {
            let mut it = b[p..].char_indices().peekable();
            while let Some((_, c)) = it.peek() {
                if c.is_whitespace() { break; }
                it.next();
            }
            while let Some((_, c)) = it.peek() {
                if !c.is_whitespace() { break; }
                it.next();
            }
            it.peek().map(|x| p + x.0).unwrap_or(b.len())
        }
    }

    /// ←/→ (Ctrl: a word), Home/End, Delete in a one-line field. True if handled.
    fn caret_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        if self.edit.is_none() || self.edit == Some(EditTarget::Note) || crate::search::is_image_mark(&self.edit_buf) {
            return false;
        }
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        let p = self.cur_pos();
        match vk {
            VK_LEFT => {
                let to = if ctrl { self.word_from(p, true) } else { self.edit_buf[..p].char_indices().last().map(|x| x.0).unwrap_or(0) };
                self.set_cur(to);
            }
            VK_RIGHT => {
                let to = if ctrl { self.word_from(p, false) } else { self.edit_buf[p..].chars().next().map(|c| p + c.len_utf8()).unwrap_or(p) };
                self.set_cur(to);
            }
            VK_HOME => self.set_cur(0),
            VK_END => self.edit_cur = None,
            VK_DELETE => {
                if std::mem::take(&mut self.edit_all) {
                    self.edit_buf.clear();
                } else if ctrl {
                    let to = self.word_from(p, false);
                    self.edit_buf.replace_range(p..to, "");
                    self.set_cur(p);
                } else {
                    self.del_char();
                }
                if self.edit == Some(EditTarget::Search) {
                    self.search_typed();
                    self.retarget();
                }
            }
            _ => return false,
        }
        if vk != VK_DELETE {
            self.edit_all = false;
        }
        self.caret = true;
        unsafe {
            // Keep the caret steady while moving it.
            SetTimer(Some(self.hwnd), T_CARET, 530, None);
        }
        self.render();
        true
    }

    /// One line of the text being edited, in `r`: scrolled so the caret stays
    /// in view, with the caret where it is and the selection (Ctrl+A) shown.
    /// Remembers where it drew, so a click can place the caret.
    pub(crate) fn draw_edit_line(&mut self, font: &IDWriteTextFormat, r: D2D_RECT_F, col: D2D1_COLOR_F, caret_col: D2D1_COLOR_F, caret_h: f32, show: Option<&str>, a: f32) {
        let g = gfx::g();
        let text = show.map(|s| s.to_string()).unwrap_or_else(|| self.edit_buf.clone());
        let maxw = (r.right - r.left - 6.0).max(10.0);
        // Masked text (a password) has one dot per character.
        let cur_chars = self.edit_buf[..self.cur_pos()].chars().count();
        let cur = text.char_indices().nth(cur_chars).map(|x| x.0).unwrap_or(text.len());
        let mut start = 0;
        while start < cur && g.measure(&text[start..cur], font) > maxw {
            start += text[start..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        }
        let shown = &text[start..];
        let cy = (r.top + r.bottom) / 2.0;
        if self.edit_all && !text.is_empty() {
            let sw = g.measure(shown, font).min(maxw + 4.0);
            self.surf.fill_rr(rf(r.left - 2.0, cy - caret_h / 2.0 - 2.0, sw + 4.0, caret_h + 4.0), 4.0, with_alpha(caret_col, 0.32 * a));
        }
        self.surf.clip(rf(r.left - 2.0, r.top, r.right - r.left + 4.0, r.bottom - r.top));
        self.surf.text(shown, font, rf(r.left, r.top, 4000.0, r.bottom - r.top), col);
        self.surf.unclip();
        if self.caret {
            let cx = r.left + g.measure(&text[start..cur], font) + 1.0;
            self.surf.fill_rect(rf(cx, cy - caret_h / 2.0, 1.8, caret_h), caret_col);
        }
        // (Clicks map back through the page's scaling; byte offsets are into the real text.)
        let real_start = if show.is_some() { self.edit_buf.char_indices().nth(text[..start].chars().count()).map(|x| x.0).unwrap_or(self.edit_buf.len()) } else { start };
        self.edit_view = Some((r, real_start, font.clone(), self.draw_map));
    }

    /// Developer aid: ISLET_DEMO_KEYS="text{Left}{Home}{Del}more" types into
    /// the Search page's field through the same paths as real keys.
    pub fn demo_keys(&mut self, spec: &str) {
        if self.search.is_none() {
            self.begin_page_search();
        }
        let mut rest = spec;
        while !rest.is_empty() {
            if let Some(r) = rest.strip_prefix('{') {
                let (name, after) = r.split_once('}').unwrap_or((r, ""));
                let vk = match name {
                    "Left" => VK_LEFT,
                    "Right" => VK_RIGHT,
                    "Home" => VK_HOME,
                    "End" => VK_END,
                    "Del" => VK_DELETE,
                    "Back" => {
                        self.on_char(8);
                        rest = after;
                        continue;
                    }
                    "Enter" => {
                        self.on_char(13);
                        rest = after;
                        continue;
                    }
                    _ => VK_ESCAPE,
                };
                self.caret_key(vk);
                rest = after;
            } else {
                let c = rest.chars().next().unwrap();
                self.on_char(c as u16);
                rest = &rest[c.len_utf8()..];
            }
        }
    }

    /// A click in the field being edited: put the caret there.
    fn caret_click(&mut self, x: f32, y: f32) -> bool {
        let Some((r, start, font, (ox, oy, k))) = self.edit_view.clone() else { return false };
        // Back into the field's own (unscaled) coordinates.
        let (x, y) = (ox + (x - ox) / k, oy + (y - oy) / k);
        if self.edit.is_none() || !inside(&rf(r.left - 6.0, r.top, r.right - r.left + 12.0, r.bottom - r.top), x, y) || crate::search::is_image_mark(&self.edit_buf) {
            return false;
        }
        let g = gfx::g();
        let start = start.min(self.edit_buf.len());
        let mut best = (start, f32::MAX);
        let mut idx: Vec<usize> = self.edit_buf[start..].char_indices().map(|(i, _)| start + i).collect();
        idx.push(self.edit_buf.len());
        for i in idx {
            let w = g.measure(&self.edit_buf[start..i], &font);
            let d = (r.left + w - x).abs();
            if d < best.1 {
                best = (i, d);
            }
        }
        self.edit_all = false;
        self.set_cur(best.0);
        self.caret = true;
        self.render();
        true
    }

    fn end_edit(&mut self, commit: bool) {        let Some(t) = self.edit.take() else { return };
        let text = std::mem::take(&mut self.edit_buf);
        match t {
            EditTarget::Todo => {
                if !commit {
                    self.todo_draft = text;
                }
            }
            EditTarget::WeatherCity if commit => {
                config::update(|c| {
                    c.set_s("weather_city", text.trim());
                    c.set_s("weather_place", "");
                });
                crate::app().refresh_weather(true);
            }
            EditTarget::Search => {
                // The Search page keeps what you typed.
                self.edit_buf = text;
            }
            EditTarget::LinkAddr if commit && !text.trim().is_empty() => crate::link::connect_address(&text),
            EditTarget::MoDelay => {
                let digits: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
                if let Ok(ms) = digits.parse::<i32>() {
                    crate::multiout::set_delay(&self.mo_edit_id, ms, true);
                }
            }
            // (Kept whether or not Enter was pressed: clicking the next field keeps this one.)
            EditTarget::RdpHost => {
                let h = text.trim().to_string();
                if h != self.rdp.host {
                    self.rdp_saved_for = crate::rdp::has_saved(&h).then(|| h.clone());
                }
                self.rdp.host = h;
            }
            EditTarget::RdpUser => self.rdp.user = text.trim().to_string(),
            EditTarget::RdpPass => self.rdp.pass = text,
            EditTarget::Note => {
                if let Some(id) = self.note_sel {
                    crate::data().notes.set_text(id, &text);
                }
            }
            EditTarget::WifiUser => self.wifi_user = text.trim().to_string(),
            EditTarget::WifiPass => {
                if commit && !text.is_empty() {
                    self.wifi_join(text);
                } else {
                    // Clicked the user name field (or away): keep what's typed.
                    self.wifi_pass = text;
                }
            }
            EditTarget::TzCity if commit => {
                config::update(|c| {
                    c.set_s("tz2_city", text.trim());
                    c.set_s("tz2_place", "");
                });
                crate::app().refresh_weather(true);
            }
            _ => {}
        }
        self.update_timers();
        self.render();
    }

    /// Join the network waiting for its password (and user name, at work / school).
    fn wifi_join(&mut self, password: String) {
        let Some(ssid) = self.wifi_pw_for.take() else { return };
        if password.is_empty() {
            self.wifi_pw_for = Some(ssid);
            return;
        }
        let user = if self.wifi_ent { Some(self.wifi_user.clone()).filter(|u| !u.is_empty()) } else { None };
        if self.wifi_ent && user.is_none() {
            self.wifi_pw_for = Some(ssid);
            self.wifi_pass = password;
            self.begin_edit(EditTarget::WifiUser);
            return;
        }
        self.wifi_ent = false;
        self.wifi_pass.clear();
        crate::connect::send(crate::connect::Job::WifiConnect { ssid, password: Some(password), user });
    }

    /// A toolbar button (or Ctrl+B / I / U): switch a style on or off from
    /// here on; the bullet button starts / ends a bulleted line.
    fn note_style(&mut self, k: u8) {
        if self.edit != Some(EditTarget::Note) {
            if self.note_sel.is_none() {
                return;
            }
            if self.edit.is_some() {
                self.end_edit(true);
            }
            self.begin_edit(EditTarget::Note);
        }
        if k < 4 {
            self.edit_buf.push(crate::tools::MARKS[k as usize]);
        } else {
            // 4 bullets, 5 numbering: on / off for the current line (or switch kind).
            self.note_list_kind(if k == 4 { 1 } else { 2 });
        }
        if let Some(id) = self.note_sel {
            crate::data().notes.set_text(id, &self.edit_buf);
        }
        self.caret = true;
        self.render();
    }

    /// The line being typed (the last one): where it starts, and its list shape.
    fn note_cur_line(&self) -> (usize, crate::tools::ListLine) {
        let ls = self.edit_buf.rfind('\n').map(|i| i + 1).unwrap_or(0);
        (ls, crate::tools::list_line(&self.edit_buf[ls..]))
    }

    /// Make the current line a bulleted (1) or numbered (2) item, or plain
    /// again when it already is that kind.
    fn note_list_kind(&mut self, kind: u8) {
        let (ls, l) = self.note_cur_line();
        let mk = if kind == 1 { crate::tools::BULLET } else { crate::tools::NUMBER };
        match l.marker {
            Some(m) if l.kind == kind => {
                self.edit_buf.replace_range(ls + m..ls + l.content, "");
            }
            Some(m) => {
                self.edit_buf.replace_range(ls + m..ls + l.content, &format!("{mk} "));
            }
            None => {
                self.edit_buf.insert_str(ls + l.content, &format!("{mk} "));
            }
        }
    }

    /// Typing in a note (everything but Ctrl+A / C / X, which work as everywhere).
    fn note_char(&mut self, ch: u16) -> bool {
        if ch == 1 {
            return false;
        }
        if ch == 3 || ch == 24 {
            // Copy / cut the note as text (without the invisible style marks).
            let (plain, _) = crate::tools::Note::display(&self.edit_buf);
            if !plain.is_empty() {
                util::set_clipboard_text(&plain.replace('\n', "\r\n"));
            }
            if ch == 24 {
                self.edit_buf.clear();
                if let Some(id) = self.note_sel {
                    crate::data().notes.set_text(id, "");
                }
            }
            self.edit_all = false;
            self.render();
            return true;
        }
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        let all = std::mem::take(&mut self.edit_all);
        let marks = crate::tools::MARKS;
        match ch {
            27 => {
                self.end_edit(true);
                self.release_focus();
                return true;
            }
            2 => self.edit_buf.push(marks[0]),
            9 if ctrl => self.edit_buf.push(marks[1]),
            21 => self.edit_buf.push(marks[2]),
            // Tab / Shift+Tab: a list item one level in / out (its bullet or
            // number changes with the level); elsewhere Tab is two spaces.
            9 => {
                let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
                let (ls, l) = self.note_cur_line();
                if shift {
                    if l.level > 0 {
                        self.edit_buf.replace_range(ls..ls + 2, "");
                    }
                } else if l.kind != 0 {
                    if l.level < crate::tools::MAX_LEVEL {
                        self.edit_buf.insert_str(ls, "  ");
                    }
                } else {
                    self.edit_buf.push_str("  ");
                }
            }
            13 | 10 => {
                if all {
                    self.edit_buf.clear();
                }
                // A list item continues the list at the same level; Enter on an
                // empty item steps it out a level, and at the top ends the list.
                let (ls, l) = self.note_cur_line();
                let empty = self.edit_buf[ls + l.content..].chars().all(|c| marks.contains(&c) || c == ' ');
                match (l.kind, l.marker) {
                    (k, Some(m)) if k != 0 && empty => {
                        if l.level > 0 {
                            self.edit_buf.replace_range(ls..ls + 2, "");
                        } else {
                            self.edit_buf.replace_range(ls + m..ls + l.content, "");
                        }
                    }
                    (k, Some(_)) if k != 0 => {
                        let mk = if k == 1 { crate::tools::BULLET } else { crate::tools::NUMBER };
                        let pre = format!("\n{}{mk} ", "  ".repeat(l.level));
                        self.edit_buf.push_str(&pre);
                    }
                    _ => self.edit_buf.push('\n'),
                }
            }
            8 | 127 => {
                if all {
                    self.edit_buf.clear();
                } else if ch == 127 {
                    let s = self.edit_buf.trim_end().to_string();
                    let cut = s.rfind([' ', '\n']).map(|i| i + 1).unwrap_or(0);
                    self.edit_buf.truncate(cut);
                } else {
                    // Marks are invisible: take them with the character before.
                    while self.edit_buf.ends_with(|c: char| marks.contains(&c)) {
                        self.edit_buf.pop();
                    }
                    // Backspace on an empty list item: no longer a list item.
                    let (ls, l) = self.note_cur_line();
                    match l.marker {
                        Some(m) if ls + l.content == self.edit_buf.len() => {
                            self.edit_buf.truncate(ls + m);
                        }
                        _ => {
                            self.edit_buf.pop();
                        }
                    }
                }
            }
            22 => {
                if let Some(t) = util::clipboard_text() {
                    if all {
                        self.edit_buf.clear();
                    }
                    let clean: String = t.replace("\r\n", "\n").chars().filter(|c| !marks.contains(c) && (*c == '\n' || !c.is_control())).collect();
                    self.edit_buf.push_str(&clean);
                }
            }
            0xD800..=0xDBFF => {
                self.hi_surrogate = Some(ch);
                return true;
            }
            0xDC00..=0xDFFF => {
                if let Some(hi) = self.hi_surrogate.take() {
                    if let Some(c) = char::decode_utf16([hi, ch]).next().and_then(|r| r.ok()) {
                        if all {
                            self.edit_buf.clear();
                        }
                        self.edit_buf.push(c);
                    }
                }
            }
            c if c >= 32 => {
                if let Some(c) = char::decode_utf16([c]).next().and_then(|r| r.ok()) {
                    if all {
                        self.edit_buf.clear();
                    }
                    self.edit_buf.push(c);
                }
            }
            _ => return true,
        }
        if let Some(id) = self.note_sel {
            crate::data().notes.set_text(id, &self.edit_buf);
        }
        self.note_text_scroll = f32::MAX; // follow the end while typing
        self.caret = true;
        self.render();
        true
    }

    fn on_char(&mut self, ch: u16) {
        // (The Esc that closed the translation, coming back as a character:
        // not for the picture's text it brought back.)
        if std::mem::take(&mut self.eat_esc) && ch == 27 {
            return;
        }
        let Some(t) = self.edit else { return };
        if t == EditTarget::Note && self.note_char(ch) {
            return;
        }
        // Ctrl+A selects everything; Ctrl+C / Ctrl+X copy / cut it (never a
        // password); with everything selected, typing or pasting replaces it.
        let secret = matches!(t, EditTarget::WifiPass | EditTarget::RdpPass);
        let all = std::mem::take(&mut self.edit_all);
        match ch {
            1 => {
                self.edit_all = !self.edit_buf.is_empty() && !crate::search::is_image_mark(&self.edit_buf);
                self.caret = true;
                self.render();
                return;
            }
            3 => {
                if !secret && !self.edit_buf.is_empty() && !crate::search::is_image_mark(&self.edit_buf) {
                    util::set_clipboard_text(&self.edit_buf);
                }
                self.edit_all = all;
                return;
            }
            24 => {
                if !secret && !self.edit_buf.is_empty() && !crate::search::is_image_mark(&self.edit_buf) {
                    util::set_clipboard_text(&self.edit_buf);
                }
                self.edit_buf.clear();
                if t == EditTarget::Search {
                    self.search_typed();
                    self.retarget();
                }
                self.render();
                return;
            }
            // Backspace / Ctrl+Backspace on a selection: clear it.
            8 | 127 if all => {
                self.edit_buf.clear();
                if t == EditTarget::Search {
                    self.search_typed();
                    self.retarget();
                }
                self.render();
                return;
            }
            // Typing or pasting over a selection replaces it.
            22 | 32.. if all && !(0xD800..=0xDFFF).contains(&ch) => self.edit_buf.clear(),
            0xD800..=0xDBFF if all => self.edit_buf.clear(),
            _ => {}
        }
        if t == EditTarget::WifiUser && matches!(ch, 9 | 13) {
            self.end_edit(true);
            self.begin_edit(EditTarget::WifiPass);
            return;
        }
        if t == EditTarget::Search {
            // Backspace in an empty dictionary box: back to the plain search.
            if ch == 8 && self.edit_buf.is_empty() && self.dict_mode() {
                self.set_dict(false);
                self.render();
                return;
            }
            // A "<" typed after pasting a picture: read its text instead.
            if self.edit_buf == crate::search::IMAGE_MARK && ch == 0x3C && crate::qr::clipboard_has_image() {
                self.edit_buf = "<".into();
                self.search_paste();
                self.retarget();
                return;
            }
            // The pasted picture goes away as a whole.
            if crate::search::is_image_mark(&self.edit_buf) && ch != 13 && ch != 10 && ch != 27 {
                self.edit_buf.clear();
                self.srch_thumb = None;
                if ch == 8 || ch == 127 {
                    self.search_typed();
                    self.retarget();
                    self.render();
                    return;
                }
            }
            match ch {
                13 => {
                    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
                    self.search_enter(ctrl);
                    return;
                }
                // Ctrl+Enter arrives as a line feed.
                10 => {
                    self.search_enter(true);
                    return;
                }
                27 => {
                    self.close_search();
                    return;
                }
                9 => return,
                _ => {}
            }
        }
        match ch {
            8 => self.back_char(),
            127 => {
                // Ctrl+Backspace: delete the word before the caret.
                let p = self.cur_pos();
                let from = self.word_from(p, true);
                self.edit_buf.replace_range(from..p, "");
                self.set_cur(from);
            }
            9 if matches!(t, EditTarget::RdpHost | EditTarget::RdpUser | EditTarget::RdpPass) => {
                self.rdp_next_field();
                return;
            }
            13 if matches!(t, EditTarget::RdpHost | EditTarget::RdpUser | EditTarget::RdpPass) => {
                self.end_edit(true);
                self.release_focus();
                if !self.rdp.host.trim().is_empty() {
                    self.rdp_connect();
                }
                return;
            }
            13 => {
                if t == EditTarget::Todo {
                    let text = std::mem::take(&mut self.edit_buf);
                    if !text.trim().is_empty() {
                        crate::data().todo.add(&text);
                        self.todo_scroll = 0;
                    }
                } else {
                    self.end_edit(true);
                    self.release_focus();
                    return;
                }
            }
            27 => {
                self.end_edit(false);
                self.release_focus();
                return;
            }
            22 if t == EditTarget::Search && self.search_paste() => {
                return;
            }
            22 => {
                if let Some(s) = util::clipboard_text() {
                    let line = s.lines().next().unwrap_or("").to_string();
                    self.ins_str(&line);
                }
            }
            0xD800..=0xDBFF => {
                self.hi_surrogate = Some(ch);
                return;
            }
            0xDC00..=0xDFFF => {
                if let Some(hi) = self.hi_surrogate.take() {
                    if let Some(c) = char::decode_utf16([hi, ch]).next().and_then(|r| r.ok()) {
                        self.ins_str(&c.to_string());
                    }
                }
            }
            c if c >= 32 => {
                if let Some(c) = char::from_u32(c as u32) {
                    if self.edit_buf.chars().count() < 120 {
                        self.ins_str(&c.to_string());
                    }
                }
            }
            _ => return,
        }
        self.caret = true;
        if t == EditTarget::Search {
            self.search_typed();
        }
        self.render();
    }

    // ------------------------------------------------------------------
    // Input
    // ------------------------------------------------------------------

    fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        unsafe {
            match msg {
                WM_TIMER => self.on_timer(wp.0),
                WM_MOUSEMOVE => {
                    if !self.hover {
                        self.hover = true;
                        let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.input_hwnd(), dwHoverTime: 0 };
                        let _ = TrackMouseEvent(&mut tme);
                        let _ = KillTimer(Some(self.hwnd), T_COLLAPSE);
                        if config::cur().b("island_hover") && !self.expanded {
                            SetTimer(Some(self.hwnd), T_HOVER, 150, None);
                        }
                    }
                    let (x, y) = self.to_dip(lp);
                    self.last_pt = (x, y);
                    if self.doc_drag {
                        self.doc_move(x, y);
                        return Some(LRESULT(0));
                    }
                    if self.link_drag.is_some() {
                        self.link_drag_move(x, y);
                        return Some(LRESULT(0));
                    }
                    if let Some(h) = self.mo_drag {
                        self.mo_drag_to(h, x, false);
                        return Some(LRESULT(0));
                    }
                    if self.vol_drag {
                        self.drag_volume(x);
                        return Some(LRESULT(0));
                    }
                    if self.bright_drag {
                        self.drag_brightness(x, false);
                        return Some(LRESULT(0));
                    }
                    // Drag a shelf item out.
                    if let Some((Hit::ShelfItem(i), px, py)) = self.press {
                        if (util::lo(lp) - px).abs() + (util::hi(lp) - py).abs() > (6.0 * self.scale) as i32 {
                            self.press = None;
                            if let Some(it) = crate::data().shelf.items.get(i) {
                                let p = vec![it.path.clone()];
                                crate::shelf::drag_out(&p);
                            }
                            return Some(LRESULT(0));
                        }
                    }
                    let hot = self.hit(x, y);
                    if hot != self.hot {
                        self.hot = hot;
                        self.render();
                    } else if self.pbar.v > 0.01 || self.pbar.target > 0.5 {
                        // The page dock swells with the pointer.
                        self.render();
                    }
                }
                WM_MOUSELEAVE => {
                    self.hover = false;
                    if self.edit.is_some() && GetForegroundWindow() != self.hwnd {
                        self.end_edit(false);
                        self.release_focus();
                    }
                    self.hot = None;
                    self.press = None;
                    let _ = KillTimer(Some(self.hwnd), T_HOVER);
                    if self.expanded && !config::cur().b("island_always") && self.edit.is_none() && !self.drag_in {
                        SetTimer(Some(self.hwnd), T_COLLAPSE, 420, None);
                    }
                    self.render();
                }
                WM_LBUTTONDOWN => {
                    let (x, y) = self.to_dip(lp);
                    // In the field being edited: move the caret there.
                    if self.caret_click(x, y) {
                        return Some(LRESULT(0));
                    }
                    // A picture's text: press and drag to select (not on its buttons).
                    if !matches!(self.hit(x, y), Some(Hit::DocBlock(_) | Hit::DocCopy | Hit::SrchMode(_) | Hit::SrchBar)) && self.doc_press(x, y) {
                        windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(self.hwnd);
                        return Some(LRESULT(0));
                    }
                    self.press = self.hit(x, y).map(|h| (h, util::lo(lp), util::hi(lp)));
                    if let Some(Hit::LinkScreen(i)) = self.hit(x, y) {
                        self.link_drag_start(i, x, y);
                        windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(self.hwnd);
                    }
                    if let Some(h @ (Hit::MoDelay(_) | Hit::MoVol(_) | Hit::MoCVol(_))) = self.hit(x, y) {
                        self.mo_drag = Some(h);
                        windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(self.hwnd);
                        self.mo_drag_to(h, x, false);
                        return Some(LRESULT(0));
                    }
                    if self.hit(x, y) == Some(Hit::Volume) {
                        self.vol_drag = true;
                        windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(self.hwnd);
                        self.drag_volume(x);
                    }
                    if self.hit(x, y) == Some(Hit::Bright) {
                        self.bright_drag = true;
                        windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(self.hwnd);
                        self.drag_brightness(x, false);
                    }
                }
                WM_LBUTTONUP => {
                    let (x, y) = self.to_dip(lp);
                    if self.doc_drag {
                        let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
                        self.doc_release();
                        self.take_focus();
                        return Some(LRESULT(0));
                    }
                    if self.link_drag.is_some() {
                        self.link_drag_end();
                        self.press = None;
                        let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
                        return Some(LRESULT(0));
                    }
                    if let Some(h) = self.mo_drag.take() {
                        self.mo_drag_to(h, x, true);
                        self.press = None;
                        let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
                        return Some(LRESULT(0));
                    }
                    if self.vol_drag {
                        self.vol_drag = false;
                        self.vol_base = None;
                        self.press = None;
                        let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
                        return Some(LRESULT(0));
                    }
                    if self.bright_drag {
                        self.drag_brightness(x, true);
                        self.bright_drag = false;
                        self.press = None;
                        let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
                        return Some(LRESULT(0));
                    }
                    let pressed = self.press.take().map(|p| p.0);
                    let h = self.hit(x, y);
                    if self.trans.is_some() {
                        match h {
                            Some(Hit::TrCopy) => self.trans_copy(),
                            Some(Hit::TrClose) => self.close_translate_back(),
                            Some(Hit::TrEngine(e)) => self.trans_engine(e),
                            _ => {}
                        }
                        return Some(LRESULT(0));
                    }
                    if self.search_popup() {
                        match h {
                            Some(Hit::SrchGuide(i)) => self.search_guide(i),
                            Some(Hit::SrchEnter) => self.search_enter(false),
                            Some(Hit::SrchTurn(next)) => self.search_page(if next { 1 } else { -1 }),
                            Some(Hit::DocBlock(k)) => self.doc_copy(Some(k)),
                            Some(Hit::DocCopy) => self.doc_copy(None),
                            Some(Hit::SrchWebAll) => {
                                let q = self.web_query();
                                util::shell_open(&crate::answers::google_url(&q));
                                self.close_search();
                            }
                            Some(Hit::SrchSource(i)) => self.open_source(i),
                            Some(Hit::AiModel(i)) => self.ai_model(i),
                            Some(Hit::AiThoughts) => self.ai_toggle_thoughts(),
                            Some(Hit::AiThink) => self.ai_toggle_think(),
                            Some(Hit::SrchItem(i)) => self.search_open(Some(i), false),
                            Some(Hit::SrchEngine) => self.search_engine_action(),
                            Some(h @ (Hit::SrchMode(_) | Hit::SrchCancel)) => {
                                self.click(h, x);
                                self.take_focus();
                            }
                            _ => self.take_focus(),
                        }
                        return Some(LRESULT(0));
                    }
                    if !self.expanded && self.a_exp.v < 0.5 {
                        if h == Some(Hit::Peek) {
                            self.click(Hit::Peek, x);
                        } else if h == Some(Hit::ColMute) && self.peek.is_none() {
                            let _ = KillTimer(Some(self.hwnd), T_HOVER);
                            crate::app().toggle_mute();
                            self.render();
                        } else if h == Some(Hit::ColMic) && self.peek.is_none() {
                            let _ = KillTimer(Some(self.hwnd), T_HOVER);
                            crate::controls::send(crate::controls::Cmd::ToggleMic);
                            self.render();
                        } else if self.mascot_always() {
                            // Click the mascot to send it away for a while.
                            let _ = KillTimer(Some(self.hwnd), T_HOVER);
                            self.masc_dismissed = Some(Instant::now());
                            self.say_bye();
                        } else {
                            let _ = KillTimer(Some(self.hwnd), T_HOVER);
                            self.last_pt = (x, y);
                            self.open_at_pointer();
                        }
                    } else if let Some(h) = h {
                        if pressed == Some(h) {
                            self.click(h, x);
                        }
                    } else {
                        // Click on empty island space: accept ←/→ keys.
                        if self.edit.is_some() {
                            self.end_edit(false);
                        }
                        self.take_focus();
                    }
                }
                WM_RBUTTONUP => {
                    let (x, y) = self.to_dip(lp);
                    match self.hit(x, y) {
                        Some(Hit::ShelfItem(i)) => {
                            crate::data().shelf.remove(i);
                            self.render();
                        }
                        Some(Hit::TodoCheck(i)) => {
                            crate::data().todo.remove(i);
                            self.render();
                        }
                        _ => crate::show_menu(),
                    }
                }
                WM_MOUSEHWHEEL => {
                    // Touchpad: two-finger swipe left / right flips pages
                    // (over the weather chart it scrolls the chart instead).
                    if self.expanded && self.page == Page::Weather && self.wx_today {
                        let mut pt = POINT { x: util::lo(lp), y: util::hi(lp) };
                        let _ = windows::Win32::Graphics::Gdi::ScreenToClient(self.hwnd, &mut pt);
                        let (x, y) = (pt.x as f32 / self.scale, pt.y as f32 / self.scale);
                        if self.zones.iter().any(|(z, r)| *z == 7 && inside(r, x, y)) {
                            let d = ((wp.0 >> 16) & 0xffff) as i16 as i32;
                            self.wx_scroll = (self.wx_scroll.max(0.0) + d as f32 / 120.0 * 60.0).max(0.0);
                            self.render();
                            return Some(LRESULT(0));
                        }
                    }
                    if self.expanded && self.edit.is_none() {
                        let d = ((wp.0 >> 16) & 0xffff) as i16 as i32;
                        self.swipe(d);
                    }
                }
                WM_MOUSEWHEEL => {
                    let d = ((wp.0 >> 16) & 0xffff) as i16 as i32;
                    if self.trans.is_some() {
                        self.trans_scroll(d);
                    } else if self.search_popup() {
                        // Scroll the results, never the volume.
                        self.search_scroll(d);
                    } else if !self.expanded {
                        crate::app().nudge_volume(d as f32 / 120.0 * 0.02);
                    } else {
                        let mut pt = POINT { x: util::lo(lp), y: util::hi(lp) };
                        let _ = windows::Win32::Graphics::Gdi::ScreenToClient(self.hwnd, &mut pt);
                        let (x, y) = (pt.x as f32 / self.scale, pt.y as f32 / self.scale);
                        if !self.scroll_page_content(x, y, d) {
                            self.wheel_acc += d;
                            if self.wheel_acc.abs() >= 120 {
                                let step = -self.wheel_acc.signum();
                                self.wheel_acc = 0;
                                self.step_page(step);
                            }
                        }
                    }
                }
                WM_KEYDOWN => {
                    let vk = VIRTUAL_KEY(wp.0 as u16);
                    // A typing method (Vietnamese Telex, Japanese, …) used Enter to
                    // finish the word: that Enter counts too (on key-up, once the
                    // finished word has arrived).
                    if vk == VK_PROCESSKEY && self.edit.is_some() && ((lp.0 >> 16) & 0xff) == 0x1C {
                        self.ime_enter = true;
                        return None;
                    }
                    if self.trans.is_some() {
                        let ctrl = windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(VK_CONTROL.0 as i32) < 0;
                        match vk {
                            VK_ESCAPE => {
                                self.eat_esc = self.doc_back.is_some();
                                self.close_translate_back();
                            }
                            VK_RETURN => self.trans_copy(),
                            k if ctrl && k.0 == b'C' as u16 => self.trans_copy(),
                            VK_UP | VK_PRIOR => self.trans_scroll(120),
                            VK_DOWN | VK_NEXT => self.trans_scroll(-120),
                            _ => {}
                        }
                        return Some(LRESULT(0));
                    }
                    // ←/→, Home/End, Delete edit the text where the caret is.
                    if self.caret_key(vk) {
                        return Some(LRESULT(0));
                    }
                    if self.edit == Some(EditTarget::Search) {
                        match vk {
                            VK_UP => self.search_move(-1),
                            VK_DOWN | VK_TAB => self.search_move(1),
                            VK_PRIOR => self.search_page(-1),
                            VK_NEXT => self.search_page(1),
                            _ => return None,
                        }
                        return Some(LRESULT(0));
                    }
                    if self.edit.is_none() {
                        match vk {
                            VK_LEFT => self.step_page(-1),
                            VK_RIGHT => self.step_page(1),
                            VK_ESCAPE => {
                                self.release_focus();
                                self.set_expanded(false);
                            }
                            VK_SPACE if self.page == Page::Home => crate::app().media_cmd(Cmd::Toggle),
                            _ => {}
                        }
                        return Some(LRESULT(0));
                    }
                    return None;
                }
                WM_CHAR => {
                    if wp.0 == 13 {
                        self.ime_enter = false;
                    }
                    self.on_char(wp.0 as u16);
                }
                WM_KEYUP if wp.0 as u16 == VK_RETURN.0 && self.ime_enter => {
                    self.ime_enter = false;
                    if self.edit.is_some() {
                        let _ = PostMessageW(Some(self.hwnd), WM_CHAR, WPARAM(13), LPARAM(0));
                    }
                    return None;
                }
                WM_ACTIVATE => {
                    if (wp.0 & 0xffff) as u32 == WA_INACTIVE && self.trans.is_some() && std::env::var("ISLET_DEMO_TRANSLATE").is_err() {
                        // Clicked somewhere else: the translation goes away.
                        self.close_translate();
                        return None;
                    }
                    if (wp.0 & 0xffff) as u32 == WA_INACTIVE && self.search_popup() && std::env::var("ISLET_DEMO_SEARCH").is_err() && std::env::var("ISLET_DEMO_DICT").is_err() {
                        // Clicked somewhere else: the search goes away, like Spotlight.
                        self.close_search();
                        return None;
                    }
                    if (wp.0 & 0xffff) as u32 == WA_INACTIVE && !self.search_popup() {
                        // Lost focus. While the pointer is still on the island keep
                        // the text field open; leaving the island closes it.
                        if !self.hover {
                            self.end_edit(false);
                            self.release_focus();
                            if self.expanded && !config::cur().b("island_always") {
                                SetTimer(Some(self.hwnd), T_COLLAPSE, 300, None);
                            }
                        }
                    }
                    return None;
                }
                _ => return None,
            }
        }
        Some(LRESULT(0))
    }

    /// Set a copy's delay / volume from where the pointer is on its slider.
    fn mo_drag_to(&mut self, h: Hit, x: f32, keep: bool) {
        let Some((_, r)) = self.hits.iter().find(|(k, _)| *k == h).copied() else { return };
        let f = ((x - r.left - 6.0) / (r.right - r.left - 12.0)).clamp(0.0, 1.0);
        match h {
            Hit::MoDelay(i) => {
                if let Some(o) = self.out_list.get(i) {
                    // 5 ms steps.
                    let ms = ((f * crate::multiout::MAX_DELAY as f32) / 5.0).round() as i32 * 5;
                    crate::multiout::set_delay(&o.id, ms, keep);
                }
            }
            Hit::MoVol(i) => {
                // The speaker's own volume (just this one).
                if let Some(o) = self.out_list.get(i) {
                    crate::multiout::set_dev_volume(&o.id, f);
                    let _ = keep;
                }
            }
            Hit::MoCVol(k) => {
                if let Some((id, _)) = self.mo_cluster.get(k) {
                    crate::multiout::set_dev_volume(id, f);
                }
                // Follows the finger at once.
                if let (Some(l), Some(sv)) = (self.mo_lv.get_mut(k), self.mo_show.get_mut(k)) {
                    l.0 = f;
                    *sv = f;
                }
            }
            _ => {}
        }
        self.render();
    }

    fn drag_volume(&mut self, x: f32) {
        if let Some((_, r)) = self.hits.iter().find(|(h, _)| *h == Hit::Volume) {
            let f = ((x - r.left - 4.0) / (r.right - r.left - 8.0)).clamp(0.0, 1.0);
            if self.mo_cluster.len() >= 2 && self.mo_lv.len() == self.mo_cluster.len() {
                // The whole group, from where it was when the slider was grabbed.
                let base = self.vol_base.get_or_insert_with(|| self.mo_lv.iter().map(|l| l.0).collect()).clone();
                let to = crate::multiout::group_target(&base, f);
                for (k, (id, _)) in self.mo_cluster.iter().enumerate() {
                    crate::multiout::set_dev_volume(id, to[k]);
                }
                for (k, v) in to.iter().enumerate() {
                    self.mo_lv[k].0 = *v;
                    self.mo_show[k] = *v;
                }
                if let Some(v) = to.first() {
                    crate::data().volume.0 = *v;
                }
            } else {
                crate::app().set_volume(f);
            }
            self.render();
        }
    }

    /// Show the Wi-Fi (1) or Bluetooth (2) list on the Controls page.
    pub(crate) fn open_ctl_list(&mut self, which: u8) {
        self.ctl_list = which;
        self.ctl_last = which;
        self.ctl_scroll = 0.0;
        self.ctl_note = None;
        self.ctl_anim.set(1.0);
        if which == 4 {
            self.kick();
            return;
        }
        if which == 3 {
            self.kb_list = crate::sysmon::keyboards();
            self.kick();
            return;
        }
        self.ctl_sel = None;
        crate::connect::clear(which == 1);
        crate::connect::send(if which == 1 { crate::connect::Job::ScanWifi } else { crate::connect::Job::ScanBt });
        // A radio that was just switched on needs a moment before it sees anything.
        unsafe { SetTimer(Some(self.hwnd), T_CTL_RESCAN, 3500, None) };
        self.kick();
    }

    pub(crate) fn close_ctl_list(&mut self, snap: bool) {
        if matches!(self.edit, Some(EditTarget::WifiPass) | Some(EditTarget::WifiUser)) {
            self.end_edit(false);
            self.release_focus();
        }
        self.wifi_pw_for = None;
        self.wifi_ent = false;
        self.wifi_pass.clear();
        self.ctl_list = 0;
        if snap {
            self.ctl_anim.snap(0.0);
        } else {
            self.ctl_anim.set(0.0);
        }
        self.kick();
    }

    fn control_action(&mut self, n: u8) {
        use crate::controls::{send, Cmd};
        match n {
            0 | 1 => {
                let st = crate::controls::state();
                let was_on = if n == 0 { st.wifi } else { st.bt } == Some(true);
                send(if n == 0 { Cmd::ToggleWifi } else { Cmd::ToggleBt });
                if was_on {
                    if self.ctl_list == n + 1 {
                        self.close_ctl_list(false);
                    }
                } else {
                    // Turned on: show what's around so one can connect at once.
                    self.open_ctl_list(n + 1);
                }
            }
            2 => send(Cmd::ToggleAirplane),
            // Keyboard: the list of installed keyboards.
            3 => {
                if self.ctl_list == 3 {
                    self.close_ctl_list(false);
                } else {
                    self.open_ctl_list(3);
                }
            }
            4 => send(Cmd::ToggleMic),
            5 => send(Cmd::ToggleDark),
            10 => {
                self.set_expanded(false);
                util::send_win_key(b'A');
            }
            // Devices page (Notifications when that page is switched off).
            11 => {
                if enabled_pages().contains(&Page::Devices) {
                    self.go_page(Page::Devices, 0);
                } else {
                    self.set_expanded(false);
                    util::send_win_key(b'N');
                }
            }
            12 => util::shell_open("ms-settings:"),
            13 => {
                if self.ctl_list == 4 {
                    self.close_ctl_list(false);
                } else {
                    self.power_arm = None;
                    self.open_ctl_list(4);
                }
            }
            _ => {}
        }
    }

    /// Brightness slider: the number follows the mouse at once; the screen is
    /// asked to change at most ~12 times a second (and once more on release).
    fn drag_brightness(&mut self, x: f32, last: bool) {
        if let Some((_, r)) = self.hits.iter().find(|(h, _)| *h == Hit::Bright) {
            let f = ((x - r.left - 4.0) / (r.right - r.left - 8.0)).clamp(0.0, 1.0);
            let v = (f * 100.0).round() as u8;
            self.bright_ui = Some(v);
            let due = self.bright_sent.map(|t| t.elapsed().as_millis() >= 80).unwrap_or(true);
            if due || last {
                crate::controls::send(crate::controls::Cmd::Brightness(v));
                self.bright_sent = Some(Instant::now());
            }
            if last {
                // Let the real value take over once the screen has caught up.
                unsafe { SetTimer(Some(self.hwnd), T_BRIGHT_UI, 1500, None) };
            }
            self.render();
        }
    }

    fn to_dip(&self, lp: LPARAM) -> (f32, f32) {
        (util::lo(lp) as f32 / self.scale, util::hi(lp) as f32 / self.scale)
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits.iter().rev().find(|(_, r)| inside(r, x, y)).map(|(b, _)| *b)
    }

    pub fn set_expanded(&mut self, e: bool) {
        if self.search_popup() || self.trans.is_some() {
            return;
        }
        if !e && self.search.is_some() {
            self.close_search();
        }
        let e = e || config::cur().b("island_always");
        if !e {
            // Nothing keeps looking inside files once the island closes.
            crate::search::cancel_content();
        }
        if !e && self.cv_open {
            self.cv_open = false;
            self.cv_anim.snap(0.0);
        }
        // "Always" mascot: the island stays closed until you click it away.
        if e && self.mascot_always() {
            return;
        }
        if e == self.expanded {
            return;
        }
        self.expanded = e;
        if e {
            self.peek = None;
            if self.mascot != Mascot::Off {
                // (Its fade goes too: left up, the closed island would come
                // back blank — the mascot gone but still covering the content.)
                self.mascot = Mascot::Off;
                self.masc_fade.snap(0.0);
            }
            if self.page == Page::Weather || self.page == Page::Clock {
                crate::app().refresh_weather(false);
            }
            if self.page == Page::Devices {
                crate::bt::spawn_scan(crate::app().host, crate::WM_BT);
            }
            if self.page == Page::Devices || self.page == Page::Controls {
                self.mo_refresh();
            }
        } else {
            self.end_edit(false);
            self.release_focus();
            self.drag_in = false;
        }
        self.retarget();
        if !e && !self.queue.is_empty() {
            self.next_peek();
        }
    }

    fn click(&mut self, b: Hit, x: f32) {
        let app = crate::app();
        let d = crate::data();
        match b {
            Hit::Nav(dir) => self.step_page(dir),
            Hit::Dot(i) | Hit::PageTile(i) => {
                if let Some(p) = enabled_pages().get(i) {
                    self.go_page(*p, 0);
                }
            }
            Hit::PageBar => {}
            Hit::Settings => crate::open_settings(0),
            Hit::Pin => {
                config::update(|c| {
                    let v = c.b("island_top");
                    c.set("island_top", !v as i32)
                });
                crate::request_apply();
            }
            Hit::Expand => {
                config::update(|c| {
                    let v = c.b("island_always");
                    c.set("island_always", !v as i32)
                });
                crate::request_apply();
            }
            // The red dot: quit Phemio.
            Hit::Close => {
                self.release_focus();
                // Closed for good, even if something is slow to stop.
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    std::process::exit(0);
                });
                unsafe {
                    let _ = PostMessageW(Some(crate::app().host), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
            Hit::Play => {
                app.media_cmd(Cmd::Toggle);
                self.media.playing = !self.media.playing;
            }
            Hit::Next => app.media_cmd(Cmd::Next),
            Hit::StopAfter => {
                crate::media::set_stop_after(!crate::media::stop_after());
                crate::app().media_cmd(crate::media::Cmd::Refresh);
                self.render();
            }
            Hit::Repeat => {
                // The player's own repeat: off → whole list → one song → off;
                // a player without one: Phemio repeats one song (off ↔ one).
                let next = match (self.media.repeat_native, self.media.repeat) {
                    (true, 0) => 2,
                    (true, 2) => 1,
                    (false, 0) => 1,
                    _ => 0,
                };
                self.media.repeat = next;
                app.media_cmd(Cmd::Repeat(next));
                self.render();
            }
            Hit::Prev => app.media_cmd(Cmd::Prev),
            Hit::Seek => {
                if let Some((_, r)) = self.hits.iter().find(|(h, _)| *h == Hit::Seek) {
                    let f = ((x - r.left) / (r.right - r.left)).clamp(0.0, 1.0);
                    let ms = (self.media.dur_ms as f32 * f) as i64;
                    app.media_cmd(Cmd::Seek(ms));
                    self.media.pos_ms = ms;
                    self.media.pos_stamp = media::filetime_now();
                }
            }
            Hit::ClockEdit => self.begin_edit(EditTarget::TzCity),
            Hit::WeatherEdit => self.begin_edit(EditTarget::WeatherCity),
            Hit::WeatherRefresh => app.refresh_weather(true),
            Hit::FocusToggle => {
                d.focus.toggle();
                app.focus_changed();
            }
            Hit::FocusReset => {
                d.focus.reset();
                app.focus_changed();
            }
            Hit::FocusPhase(i) => {
                let p = [crate::tools::Phase::Focus, crate::tools::Phase::Short, crate::tools::Phase::Long][i.min(2) as usize];
                d.focus.set_phase(p);
                app.focus_changed();
            }
            Hit::FocusLen(dir) => {
                let p = d.focus.phase;
                let key = crate::tools::Focus::key(p);
                let (step, lo, hi) = if p == crate::tools::Phase::Focus { (5, 5, 120) } else { (1, 1, 60) };
                let v = (crate::tools::Focus::minutes(p) + dir * step).clamp(lo, hi);
                config::update(|c| c.set(key, v));
                d.focus.refresh_len();
                app.focus_changed();
            }
            Hit::PowerSettings => util::shell_open("ms-settings:batterysaver"),
            Hit::FocusSkip => {
                d.focus.skip();
                app.focus_changed();
            }
            Hit::TodoInput => self.begin_edit(EditTarget::Todo),
            Hit::TodoCheck(i) => d.todo.toggle(i),
            Hit::TodoDelete(i) => d.todo.remove(i),
            Hit::ShelfItem(i) => {
                if let Some(it) = d.shelf.items.get(i) {
                    util::shell_open(&it.path);
                }
            }
            Hit::ShelfRemove(i) => d.shelf.remove(i),
            Hit::ShelfReveal(i) => {
                if let Some(it) = d.shelf.items.get(i) {
                    util::reveal(&it.path);
                }
            }
            Hit::Volume => {}
            Hit::Mute => self.mute_clicked(),
            Hit::OpenDevices => self.go_page(Page::Devices, 0),
            Hit::CluVol => {
                let open = !self.cv_open;
                if open {
                    self.mo_refresh();
                }
                self.set_cv(open && !self.mo_cluster.is_empty());
            }
            Hit::CluClose => self.set_cv(false),
            Hit::SrchCancel => {
                crate::search::cancel_content();
                self.render();
            }
            Hit::SrchMode(on) => {
                crate::search::cancel_content();
                config::update(|c| c.set("srch_content", on as i32));
                if !self.edit_buf.trim().is_empty() && !crate::search::is_image_mark(&self.edit_buf) {
                    self.search_now();
                }
                self.render();
            }
            Hit::OpenPlayer(_) => crate::player::open(&self.media.app_id, &self.media.title),
            Hit::CluCard => {}
            Hit::DevDeck => {
                // Click flips to the next card (and back to the first after the last).
                let n = Island::deck_cards().len() as i32;
                if n > 1 {
                    let cur = self.dev_card.target.round() as i32;
                    self.flip_device(if cur + 1 >= n { 0 } else { cur + 1 });
                }
            }
            Hit::DevDot(i) => self.flip_device(i as i32),
            Hit::Bright => {}
            Hit::Ctl(n) => self.control_action(n),
            Hit::CalPrev | Hit::CalNext => {
                let step = if b == Hit::CalPrev { -1 } else { 1 };
                if config::cur().i("cal_lunar") == 1 {
                    let base = self.cal_lmonth.unwrap_or(util::local_day(util::unix_now()));
                    let (start, n) = crate::lunar::month_shift(base, step);
                    self.cal_lmonth = Some(start + n / 2);
                } else {
                    let today = util::civil_from_days(util::local_day(util::unix_now()));
                    let cur = self.cal_month.unwrap_or(today.0 * 12 + today.1 as i64 - 1);
                    self.cal_month = Some(cur + step);
                }
                // The new month slides in from the side it comes from.
                self.cal_slide.v = if b == Hit::CalPrev { -1.0 } else { 1.0 };
                self.cal_slide.vel = 0.0;
                self.cal_slide.set(0.0);
                self.kick();
            }
            Hit::CalTitle => {
                self.cal_picker = match self.cal_picker {
                    Some(_) => None,
                    None => {
                        let today = util::local_day(util::unix_now());
                        Some(if config::cur().i("cal_lunar") == 1 {
                            crate::lunar::of(self.cal_lmonth.unwrap_or(today)).year
                        } else {
                            self.cal_month.map(|m| m.div_euclid(12)).unwrap_or(util::civil_from_days(today).0)
                        })
                    }
                };
                self.render();
            }
            Hit::CalPickYear(d) => {
                if let Some(y) = self.cal_picker.as_mut() {
                    *y = (*y + d as i64).clamp(1901, 2199);
                }
                self.render();
            }
            Hit::CalPickMonth(i) => {
                if let Some(year) = self.cal_picker.take() {
                    let mo = i as i64 + 1;
                    if config::cur().i("cal_lunar") == 1 {
                        self.cal_lmonth = Some(lunar_month_mid(year, mo));
                    } else {
                        self.cal_month = Some(year * 12 + mo - 1);
                    }
                    self.cal_slide.v = 1.0;
                    self.cal_slide.vel = 0.0;
                    self.cal_slide.set(0.0);
                    self.kick();
                }
                self.render();
            }
            Hit::CalPickBg => {}
            Hit::TodoNotes(on) => {
                if self.edit.is_some() {
                    self.end_edit(true);
                    self.release_focus();
                }
                self.notes_view = on;
                self.ui.todo_view.set(on as i32 as f32);
                self.kick();
                config::update(|c| c.set("notes_view", on as i32));
                self.render();
            }
            Hit::NoteNew => {
                if self.edit.is_some() {
                    self.end_edit(true);
                }
                let color = self.note_sel.and_then(|id| d.notes.get(id).map(|n| n.color)).unwrap_or(0);
                let id = d.notes.add(color);
                self.note_sel = Some(id);
                self.ui.note_in.snap(0.0);
                self.ui.note_in.set(1.0);
                self.kick();
                self.notes_scroll = 0.0;
                self.note_text_scroll = 0.0;
                self.begin_edit(EditTarget::Note);
            }
            Hit::NoteItem(i) => {
                if self.edit == Some(EditTarget::Note) {
                    self.end_edit(true);
                    self.release_focus();
                }
                let id = d.notes.notes.get(i).map(|n| n.id);
                if id != self.note_sel {
                    self.ui.note_in.snap(0.0);
                    self.ui.note_in.set(1.0);
                    self.kick();
                }
                self.note_sel = id;
                self.note_text_scroll = 0.0;
                self.render();
            }
            Hit::NoteText => {
                if self.edit != Some(EditTarget::Note) {
                    if self.edit.is_some() {
                        self.end_edit(true);
                    }
                    self.begin_edit(EditTarget::Note);
                }
            }
            Hit::NoteStyle(k) => self.note_style(k),
            Hit::NoteColor(c) => {
                if let Some(id) = self.note_sel {
                    d.notes.set_color(id, c);
                }
                self.render();
            }
            Hit::NoteDelete => {
                if let Some(id) = self.note_sel.take() {
                    if self.edit == Some(EditTarget::Note) {
                        self.edit = None;
                        self.edit_buf.clear();
                        self.release_focus();
                    }
                    let pos = d.notes.notes.iter().position(|n| n.id == id).unwrap_or(0);
                    d.notes.remove(id);
                    self.note_sel = d.notes.notes.get(pos.min(d.notes.notes.len().saturating_sub(1))).map(|n| n.id);
                }
                self.render();
            }
            Hit::WxHour(_) => {}
            Hit::CopyNet(k) => {
                let net = crate::data().net.clone();
                let v = if k == 0 { net.ip } else { net.mac };
                if let Some(v) = v {
                    util::set_clipboard_text(&v);
                    self.net_copied = Some((k, Instant::now()));
                    self.render();
                }
            }
            Hit::WxView(on) => {
                self.wx_today = on;
                self.ui.wx_view.set(on as i32 as f32);
                self.ui.wx_ha.snap(0.0);
                self.kick();
                self.wx_scroll = -1.0;
                config::update(|c| c.set("wx_today", on as i32));
                self.render();
            }
            Hit::CalOpen => {
                self.cal_picker = None;
                let today = util::civil_from_days(util::local_day(util::unix_now()));
                let back = today.0 * 12 + today.1 as i64 - 1;
                if let Some(cur) = self.cal_month {
                    self.cal_slide.v = if back < cur { -1.0 } else { 1.0 };
                    self.cal_slide.vel = 0.0;
                    self.cal_slide.set(0.0);
                    self.kick();
                }
                if self.cal_lmonth.is_some() && self.cal_month.is_none() {
                    self.cal_slide.v = -1.0;
                    self.cal_slide.vel = 0.0;
                    self.cal_slide.set(0.0);
                    self.kick();
                }
                self.cal_month = None;
                self.cal_lmonth = None;
                self.cal_sel = None;
                self.cal_fade.set(0.0);
                self.kick();
            }
            Hit::ColMute => crate::app().toggle_mute(),
            Hit::ColMic => crate::controls::send(crate::controls::Cmd::ToggleMic),
            Hit::SrchBar => {}
            Hit::SrchWebAll => {
                let q = self.web_query();
                util::shell_open(&crate::answers::google_url(&q));
                self.close_search();
            }
            Hit::SrchTurn(next) => self.search_page(if next { 1 } else { -1 }),
            Hit::DocBlock(k) => self.doc_copy(Some(k)),
            Hit::DocCopy => self.doc_copy(None),
            Hit::SrchSource(i) => self.open_source(i),
            Hit::AiModel(i) => self.ai_model(i),
            Hit::AiThoughts => self.ai_toggle_thoughts(),
            Hit::AiThink => self.ai_toggle_think(),
            Hit::SrchEnter => self.search_enter(false),
            Hit::SrchGuide(i) => self.search_guide(i),
            Hit::TrCopy => self.trans_copy(),
            Hit::TrEngine(e) => self.trans_engine(e),
            Hit::LinkPair(id) => crate::link::pair_with(id),
            Hit::LinkDecide(ok) => crate::link::decide(ok),
            Hit::LinkUnpair(id) => crate::link::unpair(id),
            Hit::LinkJump => crate::link::jump(),
            Hit::RdpField(k) => {
                let t = [EditTarget::RdpHost, EditTarget::RdpUser, EditTarget::RdpPass][k.min(2) as usize];
                if self.edit.is_some() {
                    self.end_edit(true);
                }
                self.begin_edit(t);
            }
            Hit::RdpOpt(k) => {
                let key = ["rdp_remember", "rdp_full", "rdp_multi"][k.min(2) as usize];
                let on = !config::cur().b(key);
                config::update(|c| c.set(key, on as i32));
                match k {
                    0 => self.rdp.remember = on,
                    1 => self.rdp.full = on,
                    _ => self.rdp.all_screens = on,
                }
                self.render();
            }
            Hit::RdpGo => self.rdp_connect(),
            Hit::RdpLinked => {
                if let Some(ip) = crate::link::view().and_then(|v| v.peer_ip) {
                    self.rdp_pick(ip, None);
                }
            }
            Hit::RdpRecent(i) => {
                if let Some((h, u)) = crate::rdp::recents().get(i).cloned() {
                    self.rdp_pick(h, Some(u));
                }
            }
            Hit::RdpForget(i) => {
                if let Some((h, _)) = crate::rdp::recents().get(i).cloned() {
                    crate::rdp::forget_recent(&h);
                    self.render();
                }
            }
            Hit::LinkDisconnect => crate::link::disconnect(),
            Hit::LinkOff => {
                config::update(|c| c.set("link_on", 0));
                crate::request_apply();
                self.render();
            }
            Hit::LinkConnect => crate::link::reconnect(),
            Hit::LinkFirewall => crate::link::firewall::allow(),
            Hit::LinkIdentify => crate::link::identify(),
            Hit::LinkAddr => self.begin_edit(EditTarget::LinkAddr),
            Hit::LinkAddrGo => {
                if self.edit == Some(EditTarget::LinkAddr) {
                    self.end_edit(true);
                    self.release_focus();
                } else if let Some(m) = crate::link::view().map(|v| v.manual).filter(|m| !m.is_empty()) {
                    crate::link::connect_address(&m);
                } else {
                    self.begin_edit(EditTarget::LinkAddr);
                }
            }
            Hit::ShelfTab(remote) => {
                self.shelf_remote = remote;
                self.shelf_scroll = 0.0;
                self.render();
            }
            Hit::ShelfGet(i) => {
                if let Some(v) = crate::link::view() {
                    if let Some(it) = v.remote_shelf.get(i).filter(|it| !v.have.contains(&it.0)) {
                        crate::link::shelf_get(vec![it.0]);
                    }
                }
            }
            Hit::ShelfOpen(i) => {
                if let Some(v) = crate::link::view() {
                    if let Some(p) = v.remote_shelf.get(i).and_then(|it| crate::link::clip::have_path(it.0)) {
                        util::shell_open(&p.to_string_lossy());
                    }
                }
            }
            Hit::ShelfFolder => {
                let dir = crate::link::clip::shelf_dir();
                let _ = std::fs::create_dir_all(&dir);
                util::shell_open(&dir.to_string_lossy());
            }
            Hit::ShelfGetAll => {
                // Only what isn't here yet.
                if let Some(v) = crate::link::view() {
                    let ids: Vec<u64> = v.remote_shelf.iter().map(|x| x.0).filter(|id| !v.have.contains(id)).collect();
                    if !ids.is_empty() {
                        crate::link::shelf_get(ids);
                    }
                }
            }
            Hit::LinkDismiss => crate::link::dismiss(),
            Hit::LinkOn => {
                config::update(|c| c.set("link_on", 1));
                crate::request_apply();
            }
            Hit::LinkScreen(_) => {}
            h @ (Hit::DispStep(..) | Hit::DispMain | Hit::DispKeep | Hit::DispRevert | Hit::DispClose) => self.display_click(h),
            Hit::LinkMain(i) => {
                // (Through the screen's settings, so a problem shows there.)
                if let Some(uid) = crate::link::view().and_then(|v| v.geo.screens.get(i).map(|s| s.uid())) {
                    self.link_sel = Some(uid);
                    self.display_click(Hit::DispMain);
                }
            }
            Hit::CalDay(d) => self.cal_pick(d),
            Hit::CalMode(m) => self.cal_mode(m),
            Hit::TrClose => self.close_translate_back(),
            Hit::SrchPage => self.begin_page_search(),
            Hit::SrchItem(i) => self.search_open(Some(i), false),
            Hit::SrchEngine => self.search_engine_action(),
            Hit::CtlList(n) => self.open_ctl_list(n + 1),
            Hit::CtlBack => self.close_ctl_list(false),
            Hit::CtlSettings => util::shell_open(match self.ctl_list {
                1 => "ms-settings:network-wifi",
                3 => "ms-settings:regionlanguage",
                _ => "ms-settings:bluetooth",
            }),
            Hit::LocSettings => util::shell_open("ms-settings:privacy-location"),
            Hit::KbItem(i) => {
                if let Some(k) = self.kb_list.get(i) {
                    crate::sysmon::switch_keyboard(k.hkl);
                }
                // The foreground app takes a moment to switch.
                unsafe { SetTimer(Some(self.hwnd), T_CTL_RESCAN, 200, None) };
            }
            Hit::WifiNet(i) | Hit::BtItem(i) => {
                let l = crate::connect::lists();
                let name = if matches!(b, Hit::WifiNet(_)) { l.wifi.get(i).map(|n| n.ssid.clone()) } else { l.bt.get(i).map(|d| d.name.clone()) };
                self.ctl_sel = if self.ctl_sel == name { None } else { name };
                self.render();
            }
            Hit::WifiAct(i) => {
                use crate::connect::{send, Job};
                if let Some(n) = crate::connect::lists().wifi.get(i).cloned() {
                    if self.wifi_pw_for.as_deref() == Some(n.ssid.as_str()) {
                        return;
                    }
                    if matches!(self.edit, Some(EditTarget::WifiPass) | Some(EditTarget::WifiUser)) {
                        self.end_edit(false);
                        self.release_focus();
                    }
                    self.wifi_pw_for = None;
                    self.wifi_ent = false;
                    if n.connected {
                        send(Job::WifiDisconnect);
                    } else if !n.profile.is_empty() || n.auth == 0 {
                        send(Job::WifiConnect { ssid: n.ssid, password: None, user: None });
                    } else if n.auth == 5 {
                        // Anything else (old WEP, certificates…) can't be joined from
                        // here: say so on the row, without opening Windows' panel.
                        crate::connect::failed(&n.ssid);
                        crate::connect::note(format!("{} {}", tr("Couldn't connect to"), n.ssid), false);
                    } else if matches!(n.auth, 3 | 4) {
                        // Work / school: user name, then password.
                        self.wifi_pw_for = Some(n.ssid);
                        self.wifi_ent = true;
                        self.begin_edit(EditTarget::WifiUser);
                    } else {
                        self.wifi_pw_for = Some(n.ssid);
                        self.begin_edit(EditTarget::WifiPass);
                    }
                }
                self.render();
            }
            Hit::WifiPass => {
                if self.edit.is_some() {
                    self.end_edit(false);
                }
                self.begin_edit(EditTarget::WifiPass);
            }
            Hit::WifiUser => {
                if self.edit.is_some() {
                    self.end_edit(false);
                }
                self.begin_edit(EditTarget::WifiUser);
            }
            Hit::WifiJoin => {
                if self.edit == Some(EditTarget::WifiUser) {
                    self.end_edit(true);
                    self.begin_edit(EditTarget::WifiPass);
                } else if self.edit == Some(EditTarget::WifiPass) {
                    self.end_edit(true);
                    self.release_focus();
                } else {
                    // Nothing being typed: join with what's there.
                    let pw = std::mem::take(&mut self.wifi_pass);
                    self.wifi_join(pw);
                }
            }
            Hit::PowerDo(k) => {
                // Shut down / restart: a second click within 4 s.
                // (Not a double-click: the second click must come a moment later.)
                let armed = self.power_arm.map(|(x, t)| x == k && t.elapsed() > std::time::Duration::from_millis(600) && t.elapsed() < std::time::Duration::from_secs(4)).unwrap_or(false);
                if k >= 2 && self.power_arm.map(|(x, t)| x == k && t.elapsed() <= std::time::Duration::from_millis(600)).unwrap_or(false) {
                    return;
                }
                if k >= 2 && !armed {
                    self.power_arm = Some((k, Instant::now()));
                    unsafe { SetTimer(Some(self.hwnd), T_CTL_RESCAN, 4100, None) };
                    self.render();
                    return;
                }
                self.power_arm = None;
                self.close_ctl_list(true);
                self.set_expanded(false);
                crate::sysmon::power(k);
            }
            Hit::BtAct(i) => {
                use crate::connect::{send, Job};
                if let Some(b) = crate::connect::lists().bt.get(i).cloned() {
                    if !b.paired {
                        send(Job::BtPair(b.id));
                    } else {
                        send(Job::BtConnect(b.id, !b.connected));
                    }
                }
            }
            Hit::OutMenu | Hit::InMenu => {
                let want_in = b == Hit::InMenu;
                // Same list switching between outputs and microphones: just swap.
                if self.out_menu && self.in_mode != want_in {
                    self.in_mode = want_in;
                } else {
                    self.out_menu = !self.out_menu;
                    self.in_mode = want_in;
                }
                if self.out_menu {
                    self.out_list = crate::multiout::outputs();
                    self.in_list = crate::audio::list_inputs();
                }
                self.out_anim.set(if self.out_menu { 1.0 } else { 0.0 });
                self.dev_scroll = 0.0;
                self.kick();
            }
            Hit::OutPick(i) => {
                // Opens (or closes) this speaker's settings; nothing changes yet.
                if let Some(o) = self.out_list.get(i) {
                    self.mo_sel = if self.mo_sel.as_deref() == Some(o.id.as_str()) { None } else { Some(o.id.clone()) };
                }
                self.render();
            }
            Hit::MoMain(i) => {
                if let Some(o) = self.out_list.get(i).cloned() {
                    if config::cur().b("mo_on") {
                        crate::multiout::make_main(&o.id);
                    } else if !crate::audio::set_default_output(&o.id) {
                        util::log("couldn't switch the sound output");
                    }
                    self.out_list = crate::multiout::outputs();
                }
                self.mo_refresh();
                self.render();
            }
            Hit::InPick(i) => {
                if let Some(m) = self.in_list.get(i).cloned() {
                    if !m.default && !crate::audio::set_default_output(&m.id) {
                        util::log("couldn't switch the microphone");
                    }
                    self.in_list = crate::audio::list_inputs();
                }
                self.mo_refresh();
                self.render();
            }
            Hit::MoAutoOne(i) => {
                if let Some(o) = self.out_list.get(i) {
                    crate::multiout::auto_sync(Some(o.id.clone()));
                }
            }
            Hit::MoCalibOne(i) => {
                if let Some(o) = self.out_list.get(i) {
                    crate::multiout::calibrate(Some(o.id.clone()));
                }
            }
            Hit::MoDelayEdit(i) => {
                if let Some(o) = self.out_list.get(i) {
                    self.mo_edit_id = o.id.clone();
                    self.begin_edit(EditTarget::MoDelay);
                    // The number is selected: typing replaces it.
                    self.edit_all = true;
                }
            }
            Hit::MoNudge(i, d) => {
                if let Some(o) = self.out_list.get(i) {
                    let t = crate::multiout::target(&o.id);
                    crate::multiout::set_delay(&o.id, t.delay + d, true);
                }
                self.render();
            }
            Hit::MoCVol(_) => {}
            Hit::MoToggle => {
                let on = !config::cur().b("mo_on");
                config::update(|c| c.set("mo_on", on as i32));
                crate::multiout::apply();
                // (The keyboard hook is needed for the volume keys.)
                crate::request_apply();
                self.mo_refresh();
                self.render();
            }
            Hit::MoTick(i) => {
                if let Some(o) = self.out_list.get(i) {
                    let t = crate::multiout::target(&o.id);
                    crate::multiout::set_on(&o.id, !t.on);
                }
                self.mo_refresh();
                self.render();
            }
            Hit::MoDelay(_) | Hit::MoVol(_) => {}
            Hit::MoAuto => crate::multiout::auto_sync(None),
            Hit::MoCalib => crate::multiout::calibrate(None),
            Hit::ShelfClear => {
                d.shelf.clear();
                self.shelf_scroll = 0.0;
            }
            Hit::Peek => {
                if let Some(p) = self.peek.clone() {
                    let _ = unsafe { KillTimer(Some(self.hwnd), T_PEEK) };
                    self.peek = None;
                    match p.page {
                        Some(pg) => self.open_page(pg),
                        None => self.retarget(),
                    }
                }
            }
        }
        self.update_timers();
        self.retarget();
        self.render();
    }

    fn on_timer(&mut self, id: usize) {
        unsafe {
            match id {
                link_ui::T_LINKUI => {
                    if self.expanded && self.page == Page::Link {
                        self.render();
                    } else {
                        let _ = KillTimer(Some(self.hwnd), link_ui::T_LINKUI);
                    }
                }
                T_ANIM => {
                    frame_clock::took();
                    if !self.animating {
                        return;
                    }
                    let now = Instant::now();
                    // Whole display frames: steps that match what's shown,
                    // so motion doesn't judder when a frame arrives a bit
                    // early or late.
                    let dt = frame_clock::frames(now - self.last_tick);
                    self.last_tick = now;
                    let mut moving = false;
                    for s in [&mut self.w, &mut self.h, &mut self.a_col, &mut self.a_peek, &mut self.a_exp, &mut self.slide, &mut self.dev_card, &mut self.blink, &mut self.look, &mut self.tilt, &mut self.ear, &mut self.breath, &mut self.masc_fade, &mut self.walk, &mut self.look_y, &mut self.flick, &mut self.pk_a, &mut self.out_anim, &mut self.hop, &mut self.smile, &mut self.wag, &mut self.ctl_anim, &mut self.cv_anim, &mut self.srch_a, &mut self.tr_a, &mut self.cal_slide, &mut self.cal_fade, &mut self.pbar] {
                        moving |= s.step(dt);
                    }
                    moving |= self.ui.step(dt);
                    moving |= self.ease_levels(dt);
                    if self.eq_wanted() {
                        self.eq_t += dt;
                    }
                    self.render();
                    moving |= std::mem::take(&mut self.keep_anim);
                    // Search closed and the island is small again: shrink the window back.
                    if !moving && self.tall && !self.search_popup() && self.trans.is_none() {
                        self.tall = false;
                        self.relayout();
                    }
                    if !moving {
                        self.animating = false;
                        frame_clock::stop();
                        self.update_timers();
                    }
                }
                T_SWIPE => self.swipe_end(),
                T_TBPOS | T_TBNOW => {
                    if id == T_TBNOW {
                        let _ = KillTimer(Some(self.hwnd), T_TBNOW);
                    }
                    self.watch_taskbar();
                }
                T_EQ => {
                    if !self.animating {
                        self.eq_t += 0.05;
                        self.render();
                    }
                }
                T_CLOCK => {
                    SetTimer(Some(self.hwnd), T_CLOCK, util::ms_to_next_minute(), None);
                    self.drift_step();
                    if config::cur().i("col_center") >= 1 {
                        self.retarget();
                    }
                    self.render();
                }
                T_SECOND => {
                    if id == T_SECOND {
                        // Tick just after each new second, so the seconds never lag.
                        let ms = util::now().wMilliseconds as u32;
                        SetTimer(Some(self.hwnd), T_SECOND, 1000 - ms.min(999) + 8, None);
                    }
                    if !self.animating {
                        self.render();
                    }
                }
                T_CARET => {
                    self.caret = !self.caret;
                    self.render();
                }
                T_PEEK => {
                    let _ = KillTimer(Some(self.hwnd), T_PEEK);
                    self.next_peek();
                }
                T_TRANS => {
                    self.translate_tick();
                }
                T_SEARCH => {
                    let _ = KillTimer(Some(self.hwnd), T_SEARCH);
                    if self.search.is_some() {
                        self.search_now();
                    }
                }
                T_CTL_RESCAN => {
                    let _ = KillTimer(Some(self.hwnd), T_CTL_RESCAN);
                    if self.ctl_list == 3 || self.ctl_list == 4 {
                        self.render();
                    }
                    let l = crate::connect::lists();
                    if self.ctl_list == 1 && !l.wifi_scanning {
                        crate::connect::send(crate::connect::Job::ScanWifi);
                        // Waiting for location to be allowed: keep checking while the list is open.
                        if l.wifi_denied {
                            SetTimer(Some(self.hwnd), T_CTL_RESCAN, 2500, None);
                        }
                    } else if self.ctl_list == 2 && !l.bt_scanning && l.bt.is_empty() {
                        crate::connect::send(crate::connect::Job::ScanBt);
                    }
                }
                T_PEEK_SWAP => {
                    let _ = KillTimer(Some(self.hwnd), T_PEEK_SWAP);
                    if let Some(n) = self.queue.pop_front() {
                        self.peek = None;
                        self.show_peek(n);
                    } else {
                        self.peek = None;
                        self.pk_a.snap(1.0);
                        self.retarget();
                    }
                }
                T_HOVER => {
                    let _ = KillTimer(Some(self.hwnd), T_HOVER);
                    if self.hover {
                        self.open_at_pointer();
                    }
                }
                T_COLLAPSE => {
                    let _ = KillTimer(Some(self.hwnd), T_COLLAPSE);
                    if !self.hover && self.edit.is_none() && !self.drag_in {
                        self.set_expanded(false);
                    }
                }
                T_ONTOP => {
                    self.burst = self.burst.saturating_sub(1);
                    if self.burst == 0 {
                        let _ = KillTimer(Some(self.hwnd), T_ONTOP);
                    }
                    let keep = self.burst;
                    self.ensure_on_top();
                    self.burst = keep;
                }
                T_MARQUEE => {
                    let _ = KillTimer(Some(self.hwnd), T_MARQUEE);
                    self.render();
                }
                T_BRIGHT_UI => {
                    let _ = KillTimer(Some(self.hwnd), T_BRIGHT_UI);
                    if !self.bright_drag {
                        self.bright_ui = None;
                        self.render();
                    }
                }
                T_HEAL => self.heal(),
                T_BYE => {
                    let _ = KillTimer(Some(self.hwnd), T_BYE);
                    if self.mascot == Mascot::Bye {
                        self.set_mascot(Mascot::Off);
                    }
                }
                mascot::T_DUEL => {
                    let _ = KillTimer(Some(self.hwnd), mascot::T_DUEL);
                    if (self.mascot != Mascot::Off || self.masc_fade.v > 0.01) && !self.animating {
                        self.render();
                    }
                }
                T_BLINK2 => {
                    let _ = KillTimer(Some(self.hwnd), T_BLINK2);
                    if std::env::var("ISLET_LOG_MASCOT").is_ok() {
                        util::log(&format!("blink2 mode={}", self.eye_mode));
                    }
                    if self.mascot == Mascot::Awake && self.follow != 0 {
                        match std::mem::take(&mut self.follow) {
                            1 => self.look.set(-self.look.target),
                            2 => {
                                // The pounce lands somewhere new.
                                self.look_y.set(0.0);
                                self.ear.set(-0.3);
                                self.hop.v = -1.3;
                                self.hop.vel = 0.0;
                                self.hop.set(0.0);
                                let to = if self.walk.target > 0.0 { -0.7 } else { 0.7 };
                                self.walk.set(to);
                                self.look.set(4.0 * to.signum());
                            }
                            _ => {
                                self.tilt.set(0.0);
                                self.look.set(0.0);
                            }
                        }
                        self.kick();
                    } else if self.mascot == Mascot::Awake {
                        if self.eye_mode != 0 || self.mouth_o || (self.smile.target - 1.0).abs() > 0.01 {
                            self.eye_mode = 0;
                            self.mouth_o = false;
                            self.smile.set(1.0);
                        } else {
                            self.blink_now();
                        }
                        self.kick();
                    }
                }
                T_BLINK => {
                    let _ = KillTimer(Some(self.hwnd), T_BLINK);
                    self.act_armed = false;
                    match self.mascot {
                        Mascot::Hello => {
                            self.set_mascot(Mascot::Off);
                        }
                        Mascot::Awake => {
                            self.mascot_act();
                            self.kick();
                            self.update_timers();
                        }
                        Mascot::Doze => {
                            // Slow breathing.
                            self.breath.set(if self.breath.target > 0.5 { 0.0 } else { 1.0 });
                            self.kick();
                            self.update_timers();
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    // ------------------------------------------------------------------
    // Drawing
    // ------------------------------------------------------------------

    /// Icon / indicator colour: the vibrant palette colour `c`, the album
    /// accent, or plain white/black — Settings → Island → Icon colours.
    pub(crate) fn ic(&self, c: u32, a: f32) -> D2D1_COLOR_F {
        match self.icon_mode {
            0 => {
                let col = rgb(pal::themed(c), a);
                if self.dark {
                    col
                } else {
                    // A touch deeper on a light island so it keeps contrast.
                    D2D1_COLOR_F { r: col.r * 0.82, g: col.g * 0.82, b: col.b * 0.82, a }
                }
            }
            1 => with_alpha(self.accent, a),
            _ => self.fg(0.9 * a),
        }
    }

    /// Colour of everything that isn't an icon (equaliser, slider fills,
    /// badges, dots…): Mono only makes the icons plain, so these keep the
    /// colour theme's colour there.
    pub(crate) fn el(&self, c: u32, a: f32) -> D2D1_COLOR_F {
        if self.icon_mode == 2 {
            let col = rgb(pal::themed(c), a);
            return if self.dark { col } else { D2D1_COLOR_F { r: col.r * 0.82, g: col.g * 0.82, b: col.b * 0.82, a } };
        }
        self.ic(c, a)
    }

    /// Background of an "on" toggle tile: the colour theme's shade of `c`.
    pub(crate) fn tile_col(&self, c: u32, a: f32) -> D2D1_COLOR_F {
        let col = rgb(pal::themed(c), a);
        if self.dark {
            col
        } else {
            D2D1_COLOR_F { r: col.r * 0.85, g: col.g * 0.85, b: col.b * 0.85, a }
        }
    }

    /// Black or white, whichever reads better on `bg`.
    pub(crate) fn text_on(bg: D2D1_COLOR_F, a: f32) -> D2D1_COLOR_F {
        let lum = 0.299 * bg.r + 0.587 * bg.g + 0.114 * bg.b;
        if lum > 0.62 { rgb(0x111114, a) } else { rgb(0xffffff, a) }
    }

    /// Opaque colour of `fg(k)` laid over the island background.
    pub(crate) fn blend(&self, k: f32) -> D2D1_COLOR_F {
        let v = if self.dark { k } else { 1.0 - k };
        D2D1_COLOR_F { r: v, g: v, b: v, a: 1.0 }
    }

    pub(crate) fn fg(&self, a: f32) -> D2D1_COLOR_F {
        if self.dark {
            rgb(0xffffff, a)
        } else {
            rgb(0x000000, a)
        }
    }

    pub fn render(&mut self) {
        if self.suppressed {
            return;
        }
        let _p = util::Prof::new("island.render");
        self.edit_view = None;
        let s = self.scale;
        if !self.surf.begin(s) {
            return;
        }
        self.hits.clear();
        self.zones.clear();
        let ww = self.surf.dip_w();
        let (w, h) = (self.w.v.max(20.0), self.h.v.max(4.0));
        let align = config::cur().i("island_align");
        let x = match align {
            0 => PAD,
            2 => ww - PAD - w,
            _ => (ww - w) / 2.0,
        };
        // Opening upwards (attached to a taskbar at the bottom): the bottom
        // edge stays put.
        let y = if self.up { self.top - h } else { self.top };
        // OLED care: the whole island sits a pixel or few off its spot.
        let (x, y) = (x + self.drift.0 as f32 / s, y + self.drift.1 as f32 / s);
        self.dock_before();
        let bg = if self.dark { rgb(0x000000, 1.0) } else { rgb(0xffffff, 1.0) };
        let shape = rf(x, y, w, h);

        if self.notch() {
            let rb = (h * 0.42).min(30.0);
            let ear = 9.0f32.min(h);
            // From a taskbar at the bottom it rises from the screen's edge.
            let geo = self.surf.notch_geo(x, y, w, h, 0.0, rb, ear).and_then(|g| if self.up { self.surf.flip_y(&g, y + h / 2.0) } else { Some(g) });
            if let Some(geo) = geo {
                if !self.dark {
                    self.surf.shadow(shape, rb, 0.6);
                }
                self.surf.fill_geo(&geo, bg);
            }
        } else {
            let r = (h / 2.0).min(36.0);
            self.surf.shadow(shape, r, if self.dark { 1.0 } else { 0.8 });
            self.surf.fill_rr(shape, r, bg);
            let edge = if self.dark { rgb(0xffffff, 0.10) } else { rgb(0x000000, 0.08) };
            self.surf.stroke_rr(rf(x + 0.5, y + 0.5, w - 1.0, h - 1.0), (r - 0.5).max(0.0), edge, 1.0);
        }

        self.surf.clip(shape);
        let (ac, ap, ae) = (clamp01(self.a_col.v), clamp01(self.a_peek.v), clamp01(self.a_exp.v));
        if ac > 0.01 {
            // Closed height other than 100%: the contents scaled to match,
            // drawn for the box they'd have at 100% (and what can be pointed
            // at scaled the same way).
            let (_, k) = self.col_dims();
            let (cx, cy) = (x + w / 2.0, y + h / 2.0);
            if (k - 1.0).abs() > 0.01 {
                let n_hits = self.hits.len();
                self.surf.scale_about(cx, cy, k);
                self.draw_collapsed(cx - w / (2.0 * k), cy - h / (2.0 * k), w / k, h / k, clamp01(ac * 2.0 - 1.0));
                self.surf.reset_scale();
                let sc = |r: &D2D_RECT_F| rf(cx + (r.left - cx) * k, cy + (r.top - cy) * k, (r.right - r.left) * k, (r.bottom - r.top) * k);
                for (_, r) in self.hits.iter_mut().skip(n_hits) {
                    *r = sc(r);
                }
                for (_, r) in self.col_zones.iter_mut() {
                    *r = sc(r);
                }
            } else {
                self.draw_collapsed(x, y, w, h, clamp01(ac * 2.0 - 1.0));
            }
        }
        if ap > 0.01 {
            self.draw_peek(x, y, w, h, clamp01(ap * 2.0 - 1.0));
        }
        if ae > 0.01 {
            self.draw_expanded(x, y, w, h, clamp01(ae * 1.6 - 0.6));
        }
        let sa = clamp01(self.srch_a.v);
        if sa > 0.01 && self.trans.is_none() {
            self.draw_search(x, y, w, h, clamp01(sa * 1.6 - 0.6));
        }
        let ta = clamp01(self.tr_a.v);
        if ta > 0.01 {
            self.draw_translate(x, y, w, h, clamp01(ta * 1.6 - 0.6));
        }
        self.surf.unclip();
        self.surf.end();
        // A long line scrolling: frames while it moves, a timer for its pauses.
        match self.surf.take_marquee() {
            // (25 frames a second is plenty for text gliding by, and light.)
            Some(t) => unsafe {
                SetTimer(Some(self.hwnd), T_MARQUEE, ((t * 1000.0) as u32).max(40), None);
            },
            None => {}
        }
        if self.docked {
            self.surf.present_child(self.dock_child);
        } else {
            self.surf.present(self.hwnd, self.win.left, self.win.top, None);
            self.finish_undock();
        }
        self.dock_after();
        // Something on screen still wants frames (a drawing asked for more, or
        // a fade was left half-way): keep the animation going. Without this, a
        // page redrawn by its own slow timer (the Link page's pointer dot) only
        // moved a few times a second, and a fade cut short could leave the
        // closed island blank.
        if !self.animating && (self.keep_anim || !self.ui.settled() || ![&self.a_col, &self.a_peek, &self.a_exp, &self.w, &self.h, &self.masc_fade].iter().all(|s| s.settled())) {
            self.kick();
        }
    }

    /// Beside the date (centred at `cx`, `dw` wide): today's mark on its
    /// left — a star for a lucky (hoàng đạo) day, a warning sign for an
    /// unlucky one — and this two-hour period's on its right: a thumbs up
    /// for a lucky hour, a thumbs down for an unlucky one.
    fn draw_luck(&self, cx: f32, dw: f32, cy: f32, sz: f32, a: f32) {
        if !config::cur().b("col_luck") {
            return;
        }
        let (day_good, hour_good) = luck_now();
        // The day: as on the Calendar page's day details — a filled star
        // for a lucky day, a red warning sign for an unlucky one.
        let f = if sz >= 12.0 { &self.f.icon_col } else { &self.f.icon_s };
        let c = config::cur();
        let (gl, gr) = (c.i("col_luck_gl").clamp(0, 30) as f32, c.i("col_luck_gr").clamp(0, 30) as f32);
        let lx = cx - dw / 2.0 - gl - sz;
        let day_c = if day_good { self.ic(pal::GREEN, a) } else { rgb(pal::RED, a) };
        self.surf.text(if day_good { "\u{E735}" } else { "\u{E7BA}" }, f, rf(lx - 2.0, cy - sz / 2.0 - 2.0, sz + 4.0, sz + 4.0), day_c);
        // The hour: a filled thumbs up (yellow), or a red broken heart.
        let rx = cx + dw / 2.0 + gr;
        if hour_good {
            draw_thumb(&self.surf, rx, cy - sz / 2.0, sz, true, rgb(0xffc83d, a));
        } else {
            // (A touch lower than centre: it lines up with the text and the star.)
            draw_broken_heart(&self.surf, rx + sz / 2.0, cy + sz * 0.2, sz, rgb(pal::RED, a));
        }
    }

    pub(crate) fn eq_bars(&self, right: f32, cy: f32, height: f32, a: f32) {
        let n = 4;
        let bw = 3.0;
        let gap = 2.6;
        let speeds = [7.3f32, 9.1, 6.2, 8.4];
        let phases = [0.0f32, 1.3, 2.1, 0.7];
        for i in 0..n {
            let k = if self.media.playing && config::cur().b("island_eq") {
                0.25 + 0.75 * ((self.eq_t * speeds[i] + phases[i]).sin() * 0.5 + 0.5)
            } else {
                0.3
            };
            let bh = (height * k).max(bw);
            let bx = right - (n - i) as f32 * (bw + gap);
            const BARS: [u32; 4] = [pal::PINK, pal::ORANGE, pal::GREEN, pal::CYAN];
            self.surf.fill_rr(rf(bx, cy - bh / 2.0, bw, bh), bw / 2.0, self.el(BARS[i], a));
        }
    }

    fn draw_collapsed(&mut self, x: f32, y: f32, w: f32, h: f32, a: f32) {
        self.col_zones.clear();
        if a <= 0.01 {
            return;
        }
        // The island's content fades out, then the mascot fades in (and the
        // reverse when it leaves), so the two never overlap.
        let mf = self.masc_fade.v.clamp(0.0, 1.0);
        let ease = |t: f32| t * t * (3.0 - 2.0 * t);
        let ma = ease(((mf - 0.45) / 0.55).clamp(0.0, 1.0));
        let ca = ease((1.0 - mf / 0.55).clamp(0.0, 1.0));
        if ma > 0.005 {
            self.draw_mascot(x, y, w, h, a * ma);
        }
        if ca <= 0.005 {
            return;
        }
        let a = a * ca;
        let c = config::cur();
        if c.b("minimal") {
            return;
        }
        let cy = y + h / 2.0;
        let inset = if self.notch() { 16.0 } else { 9.0 };
        let d = crate::data();
        let playing = self.media.has_session && self.media.playing;
        // Left: the battery, or the microphone (Settings → Island → Left icon shows).
        let mic_left = c.b("col_battery") && c.i("col_left") == 1;
        let saver = if c.b("col_battery") { d.battery } else { None };
        let batt = if mic_left { None } else { saver };
        let mic = if mic_left { crate::controls::state().mic_muted } else { None };
        // How far the left content reaches (its hover zone ends there).
        let mut left_end = x;
        // Left: focus ring, else the battery ring or the microphone (they
        // stay put while music plays), else album art on PCs without a battery.
        if d.focus.active() {
            let col = focus_color(d.focus.phase);
            self.surf.ring(x + inset + 10.0, cy, 8.5, 3.0, d.focus.progress(), with_alpha(col, 0.25 * a), with_alpha(col, a));
            left_end = x + inset + 20.0;
        } else if let Some(muted) = mic {
            // The microphone: red and struck through when muted. Click: mute / unmute.
            let bx = x + inset - 1.0;
            let col = if muted { rgb(pal::RED, a) } else { self.ic(pal::GREEN, a) };
            if self.hot == Some(Hit::ColMic) {
                self.surf.fill_circle(bx + 8.0, cy, 11.0, self.fg(0.12 * a));
            }
            self.surf.text(if muted { "\u{EC54}" } else { "\u{E720}" }, &self.f.icon_col, rf(bx - 1.0, cy - 9.0, 18.0, 18.0), col);
            let t = tr(if muted { "OFF" } else { "ON" });
            let tx = bx + 20.0;
            self.surf.text(t, &self.f.col_txt, rf(tx, y, 40.0, h), if muted { col } else { self.fg(0.92 * a) });
            left_end = tx + gfx::g().measure(t, &self.f.col_txt);
            if a > 0.5 {
                self.hits.push((Hit::ColMic, rf(bx - 6.0, y, left_end - bx + 10.0, h)));
            }
        } else if let (Some((pct, charging)), 1) = (batt, c.i("batt_style")) {
            // A battery with its level and the number beside it.
            let col = battery_color(pct, charging, self.dark);
            let (bw, bh) = (22.0, 11.5);
            let bx = x + inset;
            let body = rf(bx, cy - bh / 2.0, bw, bh);
            self.surf.stroke_rr(rf(body.left + 0.6, body.top + 0.6, bw - 1.2, bh - 1.2), 3.6, self.fg(0.55 * a), 1.2);
            self.surf.fill_rr(rf(body.right + 1.2, cy - 2.6, 2.2, 5.2), 1.1, self.fg(0.55 * a));
            let fw = ((bw - 4.4) * pct as f32 / 100.0).max(1.5);
            self.surf.fill_rr(rf(bx + 2.2, cy - bh / 2.0 + 2.2, fw, bh - 4.4), 2.0, with_alpha(col, a));
            if charging {
                self.surf.text("\u{E945}", &self.f.icon_s, rf(bx, cy - 8.0, bw, 16.0), self.fg(a));
            }
            let t = format!("{pct}%");
            self.surf.text(&t, &self.f.col_txt, rf(body.right + 6.0, y, 40.0, h), self.fg(0.92 * a));
            left_end = body.right + 6.0 + gfx::g().measure(&t, &self.f.col_txt);
        } else if let Some((pct, charging)) = batt {
            let col = battery_color(pct, charging, self.dark);
            let r = (h / 2.0 - 4.0).min(12.5);
            let cx = x + inset + r - 1.0;
            self.surf.ring(cx, cy, r, 2.6, pct as f32 / 100.0, self.fg(0.16 * a), with_alpha(col, a));
            let f = &self.f.col_txt_c;
            self.surf.text(&pct.to_string(), f, rf(cx - r - 2.0, cy - r, r * 2.0 + 4.0, r * 2.0), self.fg(a));
            left_end = cx + r;
        } else if playing {
            let sz = h - 12.0;
            match &self.art {
                Some(img) => self.surf.image(img, rf(x + inset - 2.0, cy - sz / 2.0, sz, sz), a),
                None => self.surf.fill_rr(rf(x + inset - 2.0, cy - sz / 2.0, sz, sz), 6.0, self.fg(0.2 * a)),
            }
            left_end = x + inset - 2.0 + sz;
        } else if c.b("col_battery") && !mic_left {
            // The battery level can't be read: a battery with an ✕.
            left_end = x + inset + self.battery_x(x + inset, cy, a);
        }
        // Centre: countdown while focusing, else the clock.
        let text = if d.focus.active() {
            fmt_dur(d.focus.remaining().as_secs())
        } else {
            center_text(&c)
        };
        let tc = if d.focus.active() { with_alpha(focus_color(d.focus.phase), a) } else { self.fg(0.95 * a) };
        let tw_c = if !d.focus.active() && c.i("col_center") >= 2 {
            // Two lines: time (with seconds) above the date, or (3) the
            // date above the time.
            let st = util::now();
            let t = util::clock_string_secs(&st, c.i("clock_fmt"));
            let dt = util::date_fmt(&st, "ddd d MMM");
            let lh = h / 2.0;
            let dr = if c.i("col_center") == 3 {
                self.surf.text(&t, &self.f.col_t2, rf(x, cy - 1.5, w, lh), tc);
                rf(x, cy - lh + 2.0, w, lh)
            } else {
                self.surf.text(&t, &self.f.col_t2, rf(x, cy - lh + 1.5, w, lh), tc);
                rf(x, cy - 1.0, w, lh)
            };
            self.surf.text(&dt, &self.f.col_d2, dr, self.fg(0.7 * a));
            let dw = gfx::g().measure(&dt, &self.f.col_d2);
            self.draw_luck(x + w / 2.0, dw, (dr.top + dr.bottom) / 2.0, 11.0, a);
            gfx::g().measure(&t, &self.f.col_t2).max(dw + luck_room(&c)) + 12.0
        } else if !d.focus.active() && c.i("col_center") == 1 {
            self.surf.text(&text, &self.f.col_d1, rf(x, y, w, h), tc);
            let dw = gfx::g().measure(&text, &self.f.col_d1);
            self.draw_luck(x + w / 2.0, dw, cy, 13.0, a);
            dw + luck_room(&c) + 12.0
        } else if d.focus.active() {
            self.surf.text(&text, &self.f.small_c, rf(x, y, w, h), tc);
            gfx::g().measure(&text, &self.f.small_c) + 12.0
        } else {
            self.surf.text(&text, &self.f.col_t1, rf(x, y, w, h), tc);
            gfx::g().measure(&text, &self.f.col_t1) + 12.0
        };
        // Hover targets: only what's drawn — the left item, the time, the
        // right item. Anywhere else opens the page you were last on.
        let left_page = if d.focus.active() {
            Some(Page::Focus)
        } else if batt.is_some() || mic.is_some() {
            Some(Page::Controls)
        } else if playing {
            Some(Page::Home)
        } else if c.b("col_battery") && !mic_left {
            Some(Page::Controls)
        } else {
            None
        };
        // The left item (battery → Controls…) first, so it wins.
        if let (Some(p), true) = (left_page, left_end > x) {
            self.col_zones.push((p, rf(x, y, left_end - x + 6.0, h)));
        }
        if enabled_pages().contains(&Page::Home) {
            // The empty space on the left (after the left item, up to the
            // time) always opens Now Playing.
            let time_left = x + (w - tw_c) / 2.0;
            let from = if left_page.is_some() && left_end > x { left_end + 6.0 } else { x };
            if time_left > from {
                self.col_zones.push((Page::Home, rf(from, y, time_left - from, h)));
            }
        }
        self.col_zones.push((if d.focus.active() { Page::Focus } else { Page::Calendar }, rf(x + (w - tw_c) / 2.0, y, tw_c, h)));
        // Equalizer animation off: the speaker instead of the bars.
        let eq = playing && c.b("island_eq");
        let speaker = (playing && !eq) || (!playing && c.i("col_right") == 1);
        let right_page = if eq {
            Some(Page::Home)
        } else if speaker {
            Some(Page::Devices)
        } else if saver.is_some() {
            Some(Page::Controls)
        } else {
            None
        };
        // How far left the right content starts.
        let mut right_start = x + w;
        // Right: equalizer, else energy saver status or the speaker (your choice).
        if eq {
            self.eq_bars(x + w - inset + 2.0, cy, h * 0.42, a);
            right_start = x + w - inset + 2.0 - 4.0 * (3.0 + 2.6);
        } else if speaker {
            let (vol, muted) = d.volume;
            let rx = x + w - inset + 2.0;
            let glyph = if muted || vol <= 0.001 { "\u{E74F}" } else if vol < 0.34 { "\u{E993}" } else if vol < 0.67 { "\u{E994}" } else { "\u{E995}" };
            let col = if muted { rgb(pal::RED, a) } else { self.ic(pal::BLUE, a) };
            let label = if muted { tr("OFF").to_string() } else { format!("{}%", (vol * 100.0).round() as i32) };
            let tw = gfx::g().measure(&label, &self.f.col_txt);
            self.surf.text(&label, &self.f.col_txt, rf(rx - tw - 1.0, y, tw + 2.0, h), if muted { col } else { self.fg(0.92 * a) });
            let gx = rx - tw - 25.0;
            if self.hot == Some(Hit::ColMute) {
                self.surf.fill_circle(gx + 10.0, cy, 12.0, self.fg(0.12 * a));
            }
            // As big as the microphone on the left.
            self.surf.text(glyph, &self.f.icon_spk, rf(gx - 2.0, cy - 12.0, 24.0, 24.0), col);
            if a > 0.5 {
                self.hits.push((Hit::ColMute, rf(gx - 6.0, y, rx - gx + 8.0, h)));
            }
            right_start = gx - 6.0;
        } else if saver.is_some() {
            let on = d.energy_saver;
            let col = if on { rgb(0x34c759, a) } else { self.fg(0.38 * a) };
            let rx = x + w - inset + 2.0;
            let label = tr(if on { "ON" } else { "OFF" });
            let tw = gfx::g().measure(label, &self.f.col_txt);
            self.surf.text(label, &self.f.col_txt, rf(rx - tw - 1.0, y, tw + 2.0, h), col);
            draw_leaf(&self.surf, rx - tw - 10.0, cy, 6.5, col);
            right_start = rx - tw - 18.0;
        }
        if let (Some(p), true) = (right_page, right_start < x + w) {
            self.col_zones.push((p, rf(right_start - 6.0, y, x + w - right_start + 6.0, h)));
        }
    }

    /// A battery outline with an ✕ in it: the battery level can't be read.
    /// Returns its width.
    pub(crate) fn battery_x(&self, bx: f32, cy: f32, a: f32) -> f32 {
        let (bw, bh) = (22.0, 11.5);
        let body = rf(bx, cy - bh / 2.0, bw, bh);
        self.surf.stroke_rr(rf(body.left + 0.6, body.top + 0.6, bw - 1.2, bh - 1.2), 3.6, self.fg(0.55 * a), 1.2);
        self.surf.fill_rr(rf(body.right + 1.2, cy - 2.6, 2.2, 5.2), 1.1, self.fg(0.55 * a));
        let (mx, d) = (bx + bw / 2.0, 2.6);
        let col = self.fg(0.85 * a);
        self.surf.line(mx - d, cy - d, mx + d, cy + d, col, 1.5);
        self.surf.line(mx - d, cy + d, mx + d, cy - d, col, 1.5);
        bw + 3.4
    }

    fn draw_peek(&mut self, x: f32, y: f32, w: f32, h: f32, a: f32) {
        // Content cross-fades between popups (bursts).
        let a = a * self.pk_a.v.clamp(0.0, 1.0);
        if a <= 0.01 {
            return;
        }
        let Some(p) = self.peek.clone().or_else(|| self.last_peek.clone()) else { return };
        let cy = y + h / 2.0;
        let inset = if self.notch() { 22.0 } else { 16.0 };
        if a > 0.5 && self.peek.is_some() {
            self.hits.push((Hit::Peek, rf(x, y, w, h)));
        }
        match p.kind {
            PeekKind::Bar if p.bars.len() > 1 => {
                // Several speakers: one row each — name, level, number.
                let rows = p.bars.len() as f32;
                let top = cy - rows * 12.0;
                for (k, (name, v)) in p.bars.iter().enumerate() {
                    let ry = top + k as f32 * 24.0 + 12.0;
                    self.speaker(x + inset + 10.0, ry, 14.0, pages::speaker_level(*v, false), self.fg(if k == 0 { 0.95 } else { 0.7 } * a));
                    let nw = 96.0;
                    self.surf.text(name, &self.f.tiny, rf(x + inset + 26.0, ry - 9.0, nw, 18.0), self.fg(0.8 * a));
                    let bx = x + inset + 26.0 + nw + 6.0;
                    let bw = w - (bx - x) - inset - 46.0;
                    self.surf.fill_rr(rf(bx, ry - 2.5, bw, 5.0), 2.5, self.fg(0.18 * a));
                    if *v > 0.0 {
                        self.surf.fill_rr(rf(bx, ry - 2.5, (bw * v).max(5.0), 5.0), 2.5, self.fg(0.95 * a));
                    }
                    self.surf.text(&format!("{}%", (v * 100.0).round() as i32), &self.f.tiny_r, rf(x + w - inset - 42.0, ry - 9.0, 42.0, 18.0), self.fg(0.9 * a));
                }
            }
            PeekKind::Bar => {
                self.peek_glyph(p.glyph, x + inset + 9.0, cy, 13.0, self.fg(a));
                let bx = x + inset + 30.0;
                let bw = w - inset * 2.0 - 30.0 - 54.0;
                self.surf.fill_rr(rf(bx, cy - 3.0, bw, 6.0), 3.0, self.fg(0.18 * a));
                if p.value > 0.0 {
                    self.surf.fill_rr(rf(bx, cy - 3.0, (bw * p.value).max(6.0), 6.0), 3.0, self.fg(0.95 * a));
                }
                self.surf.text(tr(&p.right), &self.f.peek_r, rf(x + w - inset - 60.0, y, 60.0, h), self.fg(0.9 * a));
            }
            PeekKind::Rich => {
                let r = 19.0;
                let ccx = x + inset + r;
                let col = with_alpha(p.color, a);
                self.surf.fill_circle(ccx, cy, r, with_alpha(p.color, 0.16 * a));
                if let Some(f) = p.ring {
                    self.surf.ring(ccx, cy, r - 2.5, 3.5, f, with_alpha(p.color, 0.22 * a), col);
                }
                self.peek_glyph(p.glyph, ccx, cy, r, col);
                let tx = ccx + r + 14.0;
                let rw = if p.right.is_empty() { 0.0 } else { gfx::g().measure(tr(&p.right), &self.f.peek_big) + 6.0 };
                let tw = x + w - inset - rw - tx;
                if p.sub.is_empty() {
                    self.surf.text(tr(&p.title), &self.f.peek_t, rf(tx, y, tw, h), self.fg(a));
                } else {
                    self.surf.text(tr(&p.title), &self.f.peek_t, rf(tx, cy - 20.0, tw, 20.0), self.fg(a));
                    self.surf.text(tr(&p.sub), &self.f.peek_s, rf(tx, cy + 1.0, tw, 18.0), self.fg(0.55 * a));
                }
                if !p.right.is_empty() {
                    self.surf.text(tr(&p.right), &self.f.peek_big, rf(x + w - inset - rw, y, rw, h), self.fg(a));
                }
            }
            PeekKind::Track => {
                let sz = h - 16.0;
                let ax = x + inset - 4.0;
                match &self.art {
                    Some(img) => self.surf.image(img, rf(ax, cy - sz / 2.0, sz, sz), a),
                    None => self.surf.fill_rr(rf(ax, cy - sz / 2.0, sz, sz), 8.0, self.fg(0.15 * a)),
                }
                let tx = ax + sz + 12.0;
                let tw = w - (tx - x) - 50.0;
                self.surf.text(&p.title, &self.f.peek_t, rf(tx, cy - 19.0, tw, 19.0), self.fg(a));
                self.surf.text(&p.sub, &self.f.peek_s, rf(tx, cy + 1.0, tw, 17.0), self.fg(0.6 * a));
                self.eq_bars(x + w - inset, cy, 16.0, a);
            }
        }
    }

    fn draw_expanded(&mut self, x: f32, y: f32, w: f32, h: f32, a: f32) {
        let c = config::cur();
        // Laid out at the final size and centred in the growing shape — or,
        // grown for the page dock, at the far side from where it opens.
        let x0 = x + (w - ew()) / 2.0;
        let y0 = match dock_extends() {
            Some(top) if self.expanded && h > eh() => {
                if top { y + (h - eh()) } else { y }
            }
            _ => y + (h - eh()) / 2.0,
        };
        let interactive = a > 0.5;
        let pages = enabled_pages();

        // Header: page pill · dots · buttons
        let mut label = tr(self.page.name()).to_string();
        if self.page == Page::Todo {
            let d = crate::data();
            label = format!("{}  {}/{}", tr("To-do"), d.todo.tasks.len() - d.todo.open_count(), d.todo.tasks.len());
        }
        let pw = gfx::g().measure(&label, &self.f.pill) + 44.0;
        let pill = rf(x0 + 16.0, y0 + 12.0, pw, 26.0);
        self.surf.fill_rr(pill, 13.0, self.fg(0.08 * a));
        self.page_icon(self.page, pill.left + 17.0, pill.top + 13.0, self.ic(self.page.color(), a), a);
        self.surf.text(&label, &self.f.pill, rf(pill.left + 30.0, pill.top, pw - 34.0, 26.0), self.fg(0.92 * a));

        if pages.len() > 1 {
            let n = pages.len() as f32;
            let total = (n - 1.0) * 13.0 + 18.0;
            let mut dx = x0 + ew() / 2.0 - total / 2.0;
            for (i, p) in pages.iter().enumerate() {
                let cur = *p == self.page;
                let dw = if cur { 18.0 } else { 6.0 };
                let hot = self.hot == Some(Hit::Dot(i));
                let col = if cur { self.fg(0.9 * a) } else { self.fg(if hot { 0.6 } else { 0.28 } * a) };
                // (Settings → Island → Page dots: at the top, or the bottom.)
                let dy = dots_y(y0);
                self.surf.fill_rr(rf(dx, dy - 3.0, dw, 6.0), 3.0, col);
                if interactive {
                    self.hits.push((Hit::Dot(i), rf(dx - 3.0, dy - 13.0, dw + 6.0, 26.0)));
                }
                dx += dw + 7.0;
            }
        }

        let blue = rgb(0x3b82f6, a);
        let cy = y0 + 25.0;
        let mut bx = x0 + ew() - 32.0;
        let close = rf(bx - 4.0, cy - 13.0, 26.0, 26.0);
        let close_hot = self.hot == Some(Hit::Close);
        self.surf.fill_circle(bx + 9.0, cy, if close_hot { 7.5 } else { 6.5 }, rgb(0xff453a, a));
        if close_hot {
            // Pointed at: an × (it quits Phemio).
            self.surf.line(bx + 6.2, cy - 2.8, bx + 11.8, cy + 2.8, rgb(0x5c0a06, a), 1.4);
            self.surf.line(bx + 11.8, cy - 2.8, bx + 6.2, cy + 2.8, rgb(0x5c0a06, a), 1.4);
        }
        if interactive {
            self.hits.push((Hit::Close, close));
        }
        bx -= 30.0;
        let header_btns = [
            (Hit::Expand, if c.b("island_always") { "\u{E73F}" } else { "\u{E740}" }, c.b("island_always"), pal::TEAL),
            (Hit::Pin, if c.b("island_top") { "\u{E840}" } else { "\u{E718}" }, c.b("island_top"), pal::BLUE),
            (Hit::Settings, "\u{E713}", false, pal::INDIGO),
        ];
        for (b, glyph, on, pc) in header_btns {
            let r = rf(bx - 4.0, cy - 13.0, 26.0, 26.0);
            if self.hot == Some(b) {
                self.surf.fill_rr(r, 8.0, self.fg(0.10 * a));
            }
            let col = if self.icon_mode == 0 { self.ic(pc, if on || b == Hit::Settings { a } else { 0.75 * a }) } else if on { blue } else { self.fg(0.85 * a) };
            self.surf.text(glyph, &self.f.icon, r, col);
            if interactive {
                self.hits.push((b, r));
            }
            bx -= 30.0;
        }

        // Side arrows
        // (Dots at the bottom: the page gives them a little room there.)
        let (bx0, by0, bw, bh) = (x0 + 44.0, y0 + 50.0, ew() - 88.0, eh() - 62.0 - if dots_bottom() { 16.0 } else { 0.0 });
        if pages.len() > 1 {
            for (dir, glyph, ax) in [(-1, "\u{E76B}", x0 + 8.0), (1, "\u{E76C}", x0 + ew() - 36.0)] {
                let r = rf(ax, by0 + bh / 2.0 - 22.0, 28.0, 44.0);
                let hot = self.hot == Some(Hit::Nav(dir));
                if hot {
                    self.surf.fill_rr(r, 10.0, self.fg(0.10 * a));
                }
                self.surf.text(glyph, &self.f.icon, r, self.fg(if hot { 0.95 } else { 0.45 } * a));
                if interactive {
                    self.hits.push((Hit::Nav(dir), r));
                }
            }
        }

        // Page body: on a page change the new page slides in from the side
        // as the old one slides out the other way (and during a swipe they
        // follow the fingers).
        let sv = self.slide.v;
        if sv.abs() < 0.0005 && !self.swipe_on {
            self.slide_other = None;
        }
        self.surf.clip(rf(bx0 - 6.0, by0 - 4.0, bw + 12.0, bh + 10.0));
        let interactive = interactive && sv.abs() < 0.3 && !self.swipe_on;
        let n_hits = self.hits.len();
        let n_zones = self.zones.len();
        // Fit to the island: pages are laid out for the standard size; on a
        // bigger (or smaller) island everything scales evenly to fill it, and
        // the page gets the extra room along the longer side.
        let fit = if config::cur().b("page_fit") { (bw / PAGE_W).min(bh / PAGE_H).clamp(0.6, 2.5) } else { 1.0 };
        let (bw, bh) = (bw / fit, bh / fit);
        self.page_fit = fit;
        let real_pt = self.last_pt;
        self.draw_map = (bx0, by0, fit);
        if (fit - 1.0).abs() > 0.001 {
            self.surf.scale_about(bx0, by0, fit);
            self.last_pt = (bx0 + (real_pt.0 - bx0) / fit, by0 + (real_pt.1 - by0) / fit);
        }
        // (A full page's width apart, like real pages side by side.)
        let travel = bw + 24.0;
        if let Some((other, rel)) = self.slide_other {
            let off = sv + rel;
            let oa = a * (1.0 - off.abs() * 0.75).clamp(0.0, 1.0);
            // Search and Remote desktop keep live state of their own: they
            // just fade rather than being drawn a second time.
            if oa > 0.01 && other != self.page && !matches!(other, Page::Search | Page::Remote) {
                let (nh, nz) = (self.hits.len(), self.zones.len());
                let cur = std::mem::replace(&mut self.page, other);
                self.draw_page_body(other, bx0 + off * travel, by0, bw, bh, oa);
                self.page = cur;
                self.hits.truncate(nh);
                self.zones.truncate(nz);
            }
        }
        let pa = a * (1.0 - sv.abs() * 0.75).clamp(0.0, 1.0);
        let p = self.page;
        self.draw_page_body(p, bx0 + sv * travel, by0, bw, bh, pa);
        self.draw_map = (0.0, 0.0, 1.0);
        if (fit - 1.0).abs() > 0.001 {
            self.surf.reset_scale();
            self.last_pt = real_pt;
            let map = |r: D2D_RECT_F| rf(bx0 + (r.left - bx0) * fit, by0 + (r.top - by0) * fit, (r.right - r.left) * fit, (r.bottom - r.top) * fit);
            for h in &mut self.hits[n_hits..] {
                h.1 = map(h.1);
            }
            for z in &mut self.zones[n_zones..] {
                z.1 = map(z.1);
            }
        }
        if !interactive {
            self.hits.truncate(n_hits);
        }
        self.surf.unclip();
        self.draw_page_bar(x0, y0, a);
    }

    fn draw_page_body(&mut self, p: Page, x: f32, y: f32, bw: f32, bh: f32, pa: f32) {
        match p {
            Page::Home => self.page_home(x, y, bw, bh, pa),
            Page::Clock => self.page_clock(x, y, bw, bh, pa),
            Page::Weather => self.page_weather(x, y, bw, bh, pa),
            Page::Focus => self.page_focus(x, y, bw, bh, pa),
            Page::Todo => self.page_todo(x, y, bw, bh, pa),
            Page::Shelf => self.page_shelf(x, y, bw, bh, pa),
            Page::Devices => self.page_devices(x, y, bw, bh, pa),
            Page::Controls => self.page_controls(x, y, bw, bh, pa),
            Page::Calendar => self.page_calendar(x, y, bw, bh, pa),
            Page::Search => self.page_search(x, y, bw, bh, pa),
            Page::Link => self.page_link(x, y, bw, bh, pa),
            Page::Remote => self.page_rdp(x, y, bw, bh, pa),
        }
    }

    /// Hovering the page dots: every page as its icon (the same ones as at
    /// the top left of the pages) on a small capsule dock next to the dots;
    /// the icons swell under the pointer like the macOS Dock, its name shows
    /// beside the dock, and a click goes there.
    fn draw_page_bar(&mut self, x0: f32, y0: f32, a: f32) {
        let pages = enabled_pages();
        if pages.len() < 2 {
            return;
        }
        let n = pages.len();
        // (The dock above the dots, its icons swelling upwards; or below.)
        let bottom = dock_above();
        // At the island's edge the icons swell inwards (towards the dots) and
        // the name sits in a thin strip on the outside; over the page they
        // swell away from the dots, like the macOS Dock.
        let edge_dock = dock_extends().is_some();
        let grow_up = if edge_dock { !bottom } else { bottom };
        // Resting size, the most it swells, how far the swell reaches.
        let (base, _, swell_px) = dock_dims(n);
        let (gap, pad) = (8.0f32, 7.0f32);
        let max_k = 1.0 + swell_px / base;
        let reach = base * 2.6;
        let cx = x0 + ew() / 2.0;
        let rest_w = n as f32 * (base + gap) - gap + pad * 2.0 + 8.0;
        let bh = pad * 2.0 + base;
        let dy = dots_y(y0);
        let total = (n as f32 - 1.0) * 13.0 + 18.0;
        let dots = rf(cx - total / 2.0 - 10.0, dy - 17.0, total + 20.0, 34.0);
        // Over the dots, or under them.
        let bar_top = if bottom { dots.top - 2.0 - bh } else { dots.bottom + 2.0 };
        let rest = rf(cx - rest_w / 2.0, bar_top, rest_w, bh);
        let (px, py) = self.last_pt;
        let inside = |r: &D2D_RECT_F| px >= r.left && px <= r.right && py >= r.top && py <= r.bottom;
        // Kept open while the pointer is on the dots, on the dock (and the
        // icons swelled out of it), or on the way between them.
        let swell = base * (max_k - 1.0);
        let reach_box = if bottom {
            rf(rest.left - base, rest.top - swell, rest.right - rest.left + base * 2.0, dots.bottom - rest.top + swell)
        } else {
            rf(rest.left - base, dots.top, rest.right - rest.left + base * 2.0, rest.bottom + swell - dots.top)
        };
        let want = self.hover && a > 0.5 && self.slide.v.abs() < 0.3 && (inside(&dots) || (self.pbar.target > 0.5 && inside(&reach_box)));
        let target = if want { 1.0 } else { 0.0 };
        if (self.pbar.target - target).abs() > 0.01 {
            self.pbar.set(target);
            if dock_extends().is_some() {
                // The island makes room for it (and gives it back).
                self.retarget();
            }
            self.kick();
        }
        let t = self.pbar.v.clamp(0.0, 1.0);
        if t < 0.01 {
            return;
        }
        let ba = a * t;
        // How much each icon swells: most right under the pointer, fading
        // smoothly to nothing a couple of icons away (measured on the resting
        // layout, so the icons don't chase the pointer).
        let on_bar = t > 0.6 && inside(&reach_box) && !inside(&dots);

        let ks: Vec<f32> = (0..n)
            .map(|i| {
                if !on_bar {
                    return 1.0;
                }
                let c = rest.left + pad + 4.0 + i as f32 * (base + gap) + base / 2.0;
                let d = ((px - c).abs() / reach).min(1.0);
                1.0 + (max_k - 1.0) * (0.5 + 0.5 * (d * std::f32::consts::PI).cos())
            })
            .collect();
        // The capsule grows with the swollen icons and stays centred.
        let w: f32 = ks.iter().map(|k| base * k + gap).sum::<f32>() - gap + pad * 2.0 + 8.0;
        let lift = (1.0 - t) * if bottom { 8.0 } else { -8.0 };
        let bar = rf(cx - w / 2.0, bar_top + lift, w, bh);
        let round = bh / 2.0;
        // Frosted glass (the page under it blurred, a soft tint, no outline),
        // or (Settings → Island → Page dock) no panel at all: the icons stand
        // on their own, each with a soft halo.
        let glass = config::cur().i("pdock_bg") == 0;
        if glass {
            self.surf.shadow(bar, round, 0.45 * t);
            self.surf.backdrop_blur(bar, round, 18.0, ba);
            let tint = if self.dark { rgb(0x1c1c1e, 0.62 * ba) } else { rgb(0xffffff, 0.6 * ba) };
            self.surf.fill_rr(bar, round, tint);
        }
        let halo = if self.dark { rgb(0x000000, 0.75 * ba) } else { rgb(0xffffff, 0.85 * ba) };
        let live = t > 0.6;
        if live {
            self.hits.push((Hit::PageBar, bar));
        }
        let hot_i = match self.hot {
            Some(Hit::PageTile(i)) => Some(i),
            _ => None,
        };
        // Icons sit on the capsule's inner edge and swell away from the dots.
        let mut x = bar.left + pad + 4.0;
        let mut label: Option<(usize, f32, f32)> = None;
        for (i, p) in pages.iter().enumerate() {
            let sz = base * ks[i];
            let icx = x + sz / 2.0;
            let icy = if grow_up { bar.bottom - pad - sz / 2.0 } else { bar.top + pad + sz / 2.0 };
            let cur = *p == self.page;
            let hot = hot_i == Some(i);
            // Line icons: the page you're on (and the one pointed at) in its
            // colour, the rest quiet.
            let col = if cur || hot { self.ic(p.color(), ba) } else { self.fg(0.82 * ba) };
            if !glass {
                self.surf.fill_circle(icx, icy, sz * 0.62, halo);
            }
            let k = sz / 22.0;
            self.surf.scale_about(icx, icy, k);
            match p {
                Page::Weather => pages::draw_wx(&self.surf, 2, true, icx, icy + 1.0, 17.0, ba, self.dark, self.blend(0.08), self.icon_mode == 2),
                _ => self.surf.text(p.glyph(), &self.f.icon_m, rf(icx - 14.0, icy - 14.0, 28.0, 28.0), col),
            }
            self.surf.reset_scale();
            // The page you're on: a small dot beside it (towards the page).
            if cur {
                let my = if grow_up { icy - sz / 2.0 - 3.0 } else { icy + sz / 2.0 + 3.0 };
                self.surf.fill_circle(icx, my, 1.5, with_alpha(self.ic(p.color(), 1.0), ba));
            }
            if hot {
                label = Some((i, icx, if grow_up { icy - sz / 2.0 } else { icy + sz / 2.0 }));
            }
            if live {
                let ext = sz - base;
                let r = if grow_up { rf(x - gap / 2.0, bar.top - ext, sz + gap, bh + ext) } else { rf(x - gap / 2.0, bar.top, sz + gap, bh + ext) };
                self.hits.push((Hit::PageTile(i), r));
            }
            x += sz + gap;
        }
        // The name of the one pointed at, in a pill beyond the swollen icon.
        if let Some((i, icx, edge)) = label {
            let name = tr(pages[i].name());
            let g = gfx::g();
            let lw = g.measure(name, &self.f.tiny) + 16.0;
            // (At the island's edge there's no room beyond the icons: the
            // name goes on the dots' side of the dock instead.)
            // Above the icons when the dock is above the dots, below when below.
            let ly = match (edge_dock, bottom) {
                (true, true) => bar.top - 20.0,
                (true, false) => bar.bottom + 3.0,
                (false, true) => edge - 24.0,
                (false, false) => edge + 6.0,
            };
            let lr = rf(icx - lw / 2.0, ly, lw, 18.0);
            let pill = if self.dark { rgb(0x2c2c2e, 0.95 * ba) } else { rgb(0xffffff, 0.97 * ba) };
            self.surf.shadow(lr, 9.0, 0.4 * t);
            self.surf.fill_rr(lr, 9.0, pill);
            self.surf.text(name, &self.f.tiny_c, lr, self.fg(0.95 * ba));
        }
    }

    /// Icon-font glyph, a text symbol, or the vector water drop.
    fn peek_glyph(&self, glyph: &str, cx: f32, cy: f32, r: f32, col: D2D1_COLOR_F) {
        let first = glyph.chars().next().unwrap_or(' ');
        if glyph == "leaf" {
            draw_leaf(&self.surf, cx, cy, r * 0.5, col);
        } else if ('\u{E000}'..='\u{F8FF}').contains(&first) {
            self.surf.text(glyph, &self.f.icon, rf(cx - r, cy - r, r * 2.0, r * 2.0), col);
        } else {
            self.surf.text(glyph, &self.f.small_c, rf(cx - r, cy - r, r * 2.0, r * 2.0), col);
        }
    }

    fn page_icon(&self, p: Page, cx: f32, cy: f32, col: D2D1_COLOR_F, a: f32) {
        match p {
            Page::Weather => pages::draw_wx(&self.surf, 2, true, cx, cy + 1.0, 17.0, a, self.dark, self.blend(0.08), self.icon_mode == 2),
            _ => self.surf.text(p.glyph(), &self.f.icon_s, rf(cx - 11.0, cy - 13.0, 22.0, 26.0), col),
        }
    }

    /// Wheel over a scrollable list scrolls it; returns false otherwise.
    fn scroll_page_content(&mut self, x: f32, y: f32, d: i32) -> bool {
        // Lists scroll by the pixel (smooth with a touchpad's small steps).
        if let Some(z) = self.zones.iter().rev().find(|(_, r)| inside(r, x, y)).map(|z| z.0) {
            match z {
                1 => {
                    self.ctl_scroll = (self.ctl_scroll - d as f32 / 120.0 * 56.0).max(0.0);
                    self.render();
                    return true;
                }
                3 => {
                    self.dev_scroll = (self.dev_scroll - d as f32 / 120.0 * 48.0).max(0.0);
                    self.render();
                    return true;
                }
                // Notes: the list, and a long note.
                9 => {
                    self.notes_scroll = (self.notes_scroll - d as f32 / 120.0 * 40.0).max(0.0);
                    self.render();
                    return true;
                }
                10 => {
                    self.note_text_scroll = (self.note_text_scroll.min(100_000.0) - d as f32 / 120.0 * 40.0).max(0.0);
                    self.render();
                    return true;
                }
                // The calendar's day details scroll when they're long.
                8 => {
                    self.cal_detail_scroll = (self.cal_detail_scroll - d as f32 / 120.0 * 40.0).clamp(0.0, self.cal_detail_max);
                    self.render();
                    return true;
                }
                // The weather chart scrolls sideways (the page doesn't flip).
                7 => {
                    self.wx_scroll = (self.wx_scroll.max(0.0) - d as f32 / 120.0 * 60.0).max(0.0);
                    self.render();
                    return true;
                }
                _ => {}
            }
        }
        if self.page == Page::Search && self.zones.iter().any(|(z, r)| *z == 4 && inside(r, x, y)) {
            self.search_scroll(d);
            return true;
        }
        let over = |h: &Hit| matches!(h, Hit::KbItem(_) | Hit::ShelfItem(_) | Hit::ShelfRemove(_) | Hit::ShelfReveal(_) | Hit::TodoCheck(_) | Hit::TodoDelete(_) | Hit::DevDeck | Hit::DevDot(_));
        if !self.hits.iter().any(|(h, r)| over(h) && inside(r, x, y)) && !self.zones.iter().any(|(z, r)| *z == 2 && inside(r, x, y)) {
            return false;
        }
        match self.page {
            Page::Shelf => {
                self.shelf_scroll = (self.shelf_scroll - d as f32 / 120.0 * 90.0).max(0.0);
            }
            Page::Todo => {
                let n = crate::data().todo.tasks.len();
                if d < 0 && self.todo_scroll + 4 < n {
                    self.todo_scroll += 1;
                } else if d > 0 && self.todo_scroll > 0 {
                    self.todo_scroll -= 1;
                }
            }
            Page::Devices => {
                let n = Island::deck_cards().len() as i32;
                if n < 2 {
                    return false;
                }
                self.dev_wheel += d;
                if self.dev_wheel.abs() >= 120 {
                    let step = -self.dev_wheel.signum();
                    self.dev_wheel = 0;
                    let cur = self.dev_card.target.round() as i32;
                    let to = (cur + step).clamp(0, n - 1);
                    if to == cur {
                        // A little bump at either end of the deck.
                        self.dev_card.vel += step as f32 * 1.5;
                        self.kick_anim();
                    } else {
                        self.flip_device(to);
                    }
                }
                return true;
            }
            _ => return false,
        }
        self.render();
        true
    }

    fn flip_device(&mut self, i: i32) {
        let n = Island::deck_cards().len() as i32;
        if n == 0 {
            return;
        }
        self.dev_card.set(i.clamp(0, n - 1) as f32);
        self.kick_anim();
    }

    /// The device list changed: keep the front card in range.
    pub fn devices_changed(&mut self) {
        let n = Island::deck_cards().len() as f32;
        if self.dev_card.target > (n - 1.0).max(0.0) {
            self.dev_card.snap((n - 1.0).max(0.0));
        }
    }

    pub(crate) fn kick_anim(&mut self) {
        self.kick();
    }
}

/// Room the lucky / unlucky marks take beside the date (both sides).
fn luck_room(c: &config::Config) -> f32 {
    if c.b("col_luck") {
        // Two marks (about 13 wide each) and the chosen gaps.
        26.0 + (c.i("col_luck_gl").clamp(0, 30) + c.i("col_luck_gr").clamp(0, 30)) as f32
    } else {
        0.0
    }
}

/// Is today a lucky (hoàng đạo) day, and is this two-hour period (giờ) a lucky one?
fn luck_now() -> (bool, bool) {
    // Developer aid: ISLET_FAKE_LUCK=1,0 (lucky day, unlucky hour).
    if let Ok(v) = std::env::var("ISLET_FAKE_LUCK") {
        let b: Vec<bool> = v.split(',').map(|x| x.trim() == "1").collect();
        if b.len() == 2 {
            return (b[0], b[1]);
        }
    }
    let st = util::now();
    let day = util::days_from_civil(st.wYear as i64, st.wMonth as i64, st.wDay as i64);
    let info = crate::lunar::info(day);
    // Tý is 23–1h, Sửu 1–3h, … (from 23:00 the hour belongs to the next day).
    let chi = ((st.wHour as usize + 1) / 2) % 12;
    let hours = if st.wHour >= 23 { crate::lunar::info(day + 1).hours } else { info.hours };
    (crate::lunar::GODS[info.god].1, hours[chi])
}

/// What the middle of the closed island shows: the time, or the date ("Mon 26 Oct").
fn center_text(c: &config::Config) -> String {
    let st = util::now();
    if c.i("col_center") == 1 {
        let d = util::date_fmt(&st, "ddd d MMM");
        if !d.is_empty() {
            return d;
        }
    }
    if c.b("clock_secs") {
        util::clock_string_secs(&st, c.i("clock_fmt"))
    } else {
        util::clock_string(&st, c.i("clock_fmt"))
    }
}

pub fn battery_color(pct: u8, charging: bool, dark: bool) -> D2D1_COLOR_F {
    // Never plain white: charging is green, low is orange / red, otherwise
    // a theme colour that also tells roughly how full it is.
    let c = if charging {
        pal::themed(pal::GREEN)
    } else if pct <= 10 {
        pal::RED
    } else if pct <= 20 {
        pal::ORANGE
    } else if pct < 50 {
        pal::themed(pal::YELLOW)
    } else {
        pal::themed(pal::TEAL)
    };
    let col = rgb(c, 1.0);
    if dark {
        col
    } else {
        D2D1_COLOR_F { r: col.r * 0.82, g: col.g * 0.82, b: col.b * 0.82, a: 1.0 }
    }
}

/// A filled thumbs up (or, `up` false, thumbs down) in the `sz` square at
/// (x, y): the hand and its cuff, drawn as shapes so it's solid.
pub fn draw_thumb(s: &Surface, x: f32, y: f32, sz: f32, up: bool, c: D2D1_COLOR_F) {
    // Thumbs up, in a 0–1 box (y down); thumbs down is it upside down.
    const HAND: [(f32, f32); 17] = [
        (0.30, 0.42), (0.46, 0.16), (0.50, 0.04), (0.57, 0.0), (0.65, 0.04), (0.67, 0.14), (0.62, 0.36),
        (0.90, 0.36), (0.99, 0.43), (0.95, 0.53), (0.98, 0.61), (0.93, 0.70), (0.95, 0.78), (0.89, 0.87), (0.89, 0.93), (0.82, 1.0), (0.30, 1.0),
    ];
    let p = |(px, py): (f32, f32)| (x + px * sz, y + if up { py } else { 1.0 - py } * sz);
    let pts: Vec<(f32, f32)> = HAND.iter().map(|&q| p(q)).collect();
    s.fill_poly(&pts, c);
    // The cuff, with a hairline gap from the hand.
    let (t, b) = if up { (0.44, 1.0) } else { (0.0, 0.56) };
    s.fill_rr(rf(x, y + t * sz, 0.22 * sz, (b - t) * sz), 0.06 * sz, c);
}

/// A filled broken heart, `sz` wide, centred at (cx, cy): the two halves
/// split by a zigzag crack and pulled a little apart.
pub fn draw_broken_heart(s: &Surface, cx: f32, cy: f32, sz: f32, c: D2D1_COLOR_F) {
    use std::f32::consts::PI;
    // The classic heart curve: x ±16, y from about -12 (lobes) to 17 (tip), y down.
    let heart = |t: f32| (16.0 * t.sin().powi(3), -(13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos()));
    let k = sz / 32.0;
    let map = |(x, y): (f32, f32), dx: f32| (cx + (x + dx) * k, cy + (y - 2.5) * k);
    // The crack, from the dip at the top down to the tip.
    const CRACK: [(f32, f32); 5] = [(0.0, -5.0), (-3.0, 0.5), (2.5, 5.5), (-2.0, 10.5), (0.0, 17.0)];
    let n = 24;
    // Right half: around the right side from the dip to the tip, then up the crack.
    let mut right: Vec<(f32, f32)> = (0..=n).map(|i| map(heart(PI * i as f32 / n as f32), 1.6)).collect();
    right.extend(CRACK.iter().rev().map(|&p| map(p, 1.6)));
    // Left half: up the left side from the tip to the dip, then down the crack.
    let mut left: Vec<(f32, f32)> = (0..=n).map(|i| map(heart(PI + PI * i as f32 / n as f32), -1.6)).collect();
    left.extend(CRACK.iter().map(|&p| map(p, -1.6)));
    s.fill_poly(&right, c);
    s.fill_poly(&left, c);
}

/// Energy-saver leaf.
pub fn draw_leaf(s: &Surface, cx: f32, cy: f32, r: f32, c: D2D1_COLOR_F) {
    let n = 20;
    let (sn, cs) = (std::f32::consts::FRAC_PI_4.sin(), std::f32::consts::FRAC_PI_4.cos());
    let mut pts = Vec::with_capacity(n * 2);
    for i in 0..=n {
        // Almond outline, rotated 45°.
        let t = std::f32::consts::PI * i as f32 / n as f32;
        for side in [1.0f32] {
            let lx = side * r * 0.55 * t.sin();
            let ly = -r * t.cos();
            pts.push((cx + lx * cs - ly * sn, cy + lx * sn + ly * cs));
        }
    }
    for i in (0..=n).rev() {
        let t = std::f32::consts::PI * i as f32 / n as f32;
        let lx = -r * 0.55 * t.sin();
        let ly = -r * t.cos();
        pts.push((cx + lx * cs - ly * sn, cy + lx * sn + ly * cs));
    }
    s.fill_poly(&pts, c);
    // Stem / midrib
    s.line(cx - r * 0.95 * sn, cy + r * 0.95 * cs, cx + r * 0.5 * sn, cy - r * 0.5 * cs, rgb(0x000000, 0.55 * c.a), 1.0);
}

pub fn focus_color(p: crate::tools::Phase) -> D2D1_COLOR_F {
    match p {
        crate::tools::Phase::Focus => rgb(0x34c759, 1.0),
        crate::tools::Phase::Short => rgb(0x5ac8fa, 1.0),
        crate::tools::Phase::Long => rgb(0xaf52de, 1.0),
    }
}

pub fn fmt_dur(secs: u64) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

pub fn fmt_ms(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// A vivid colour taken from the album art for the equalizer bars.
fn accent_of(img: &Image) -> D2D1_COLOR_F {
    let px = img.pixels();
    let mut best = (0.0f32, rgb(0x34c759, 1.0));
    let step = ((px.len() / 4) / 400).max(1) * 4;
    let mut i = 0;
    while i + 3 < px.len() {
        let a = px[i + 3] as f32;
        if a > 200.0 {
            let (b, g, r) = (px[i] as f32 / a, px[i + 1] as f32 / a, px[i + 2] as f32 / a);
            let mx = r.max(g).max(b);
            let mn = r.min(g).min(b);
            let sat = if mx > 0.0 { (mx - mn) / mx } else { 0.0 };
            let score = sat * (0.35 + mx);
            if mx > 0.35 && score > best.0 {
                best = (score, D2D1_COLOR_F { r, g, b, a: 1.0 });
            }
        }
        i += step;
    }
    if best.0 < 0.18 {
        rgb(0xffffff, 0.9)
    } else {
        let c = best.1;
        let mx = c.r.max(c.g).max(c.b);
        let k = if mx < 0.8 { 0.85 / mx } else { 1.0 };
        D2D1_COLOR_F { r: (c.r * k).min(1.0), g: (c.g * k).min(1.0), b: (c.b * k).min(1.0), a: 1.0 }
    }
}

/// A day in the middle of lunar month `m` of lunar year `y` (not the leap one).
fn lunar_month_mid(y: i64, m: i64) -> i64 {
    // Tết falls between 21 Jan and 20 Feb: start near there and walk.
    let guess = util::days_from_civil(y, 2, 5) + ((m - 1) as f64 * 29.53) as i64;
    let mut d = guess;
    for _ in 0..6 {
        let (start, n) = crate::lunar::month_of(d);
        let l = crate::lunar::of(start);
        let diff = (y - l.year) * 12 + (m - l.month);
        if diff == 0 {
            if l.leap {
                // The leap month repeats the number: the first one is the plain month.
                d = crate::lunar::month_shift(start, -1).0 + 15;
                continue;
            }
            return start + n / 2;
        }
        d = crate::lunar::month_shift(start, diff.signum()).0 + 15;
    }
    guess
}


/// The island's animation clock: frames come at the display's own pace
/// (DWM's vertical blank, via DwmFlush on a small thread) instead of a 15 ms
/// window timer — Windows timers tick at 15.6 ms steps, arrive late when the
/// message queue is busy, and never line up with a 60 / 120 / 144 Hz screen,
/// which is what made animations stutter. A frame is posted as the island's
/// T_ANIM timer message, never more than one waiting at a time.
mod frame_clock {
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::*;
    use windows::Win32::System::Threading::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    static ON: AtomicBool = AtomicBool::new(false);
    static PENDING: AtomicBool = AtomicBool::new(false);
    static HWND_: AtomicIsize = AtomicIsize::new(0);
    static WAKE: AtomicIsize = AtomicIsize::new(0);
    /// The display's frame time, in microseconds.
    static PERIOD_US: AtomicU64 = AtomicU64::new(16_667);
    static POSTED: AtomicU64 = AtomicU64::new(0);

    fn since_start_ms() -> u64 {
        static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        T0.get_or_init(Instant::now).elapsed().as_millis() as u64
    }

    fn wake_event() -> HANDLE {
        let h = WAKE.load(Ordering::Acquire);
        if h != 0 {
            return HANDLE(h as *mut _);
        }
        let e = unsafe { CreateEventW(None, false, false, None) }.unwrap_or_default();
        match WAKE.compare_exchange(0, e.0 as isize, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {
                std::thread::Builder::new().name("frames".into()).spawn(run).ok();
                e
            }
            Err(other) => {
                unsafe {
                    let _ = CloseHandle(e);
                }
                HANDLE(other as *mut _)
            }
        }
    }

    pub fn start(hwnd: HWND) {
        HWND_.store(hwnd.0 as isize, Ordering::Release);
        PENDING.store(false, Ordering::Release);
        ON.store(true, Ordering::Release);
        let e = wake_event();
        unsafe {
            let _ = SetEvent(e);
        }
    }

    pub fn stop() {
        ON.store(false, Ordering::Release);
    }

    /// The posted frame has been taken: the next one may come.
    pub fn took() {
        PENDING.store(false, Ordering::Release);
    }

    /// The time since the last frame, rounded to whole display frames
    /// (1–4 of them), in seconds.
    pub fn frames(d: Duration) -> f32 {
        let p = PERIOD_US.load(Ordering::Relaxed).clamp(4_000, 50_000) as f32 / 1e6;
        let n = (d.as_secs_f32() / p).round().clamp(1.0, 4.0);
        n * p
    }

    fn refresh_period() -> u64 {
        use windows::Win32::Graphics::Dwm::*;
        unsafe {
            let mut t = DWM_TIMING_INFO { cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32, ..Default::default() };
            if DwmGetCompositionTimingInfo(HWND::default(), &mut t).is_ok() && t.rateRefresh.uiNumerator > 0 && t.rateRefresh.uiDenominator > 0 {
                let hz = t.rateRefresh.uiNumerator as f64 / t.rateRefresh.uiDenominator as f64;
                if (20.0..=500.0).contains(&hz) {
                    return (1e6 / hz).round() as u64;
                }
            }
        }
        16_667
    }

    fn run() {
        let mut last_period = Instant::now() - Duration::from_secs(10);
        loop {
            if !ON.load(Ordering::Acquire) {
                let e = HANDLE(WAKE.load(Ordering::Acquire) as *mut _);
                unsafe {
                    WaitForSingleObject(e, INFINITE);
                }
                continue;
            }
            // (The refresh rate can change: a different screen, power mode…)
            if last_period.elapsed() > Duration::from_secs(2) {
                PERIOD_US.store(refresh_period(), Ordering::Relaxed);
                last_period = Instant::now();
            }
            let t0 = Instant::now();
            let synced = unsafe { windows::Win32::Graphics::Dwm::DwmFlush() }.is_ok();
            // No vertical blank to wait for (composition off, remote desktop,
            // Wine): pace by the clock instead.
            let period = Duration::from_micros(PERIOD_US.load(Ordering::Relaxed));
            if !synced || t0.elapsed() < Duration::from_millis(2) {
                let left = period.saturating_sub(t0.elapsed());
                std::thread::sleep(left);
            }
            // (A frame not taken for a quarter second was lost: send another.)
            if PENDING.load(Ordering::Acquire) && POSTED.load(Ordering::Relaxed) > 0 && since_start_ms().saturating_sub(POSTED.load(Ordering::Relaxed)) > 250 {
                PENDING.store(false, Ordering::Release);
            }
            if ON.load(Ordering::Acquire) && !PENDING.swap(true, Ordering::AcqRel) {
                POSTED.store(since_start_ms().max(1), Ordering::Relaxed);
                let h = HWND(HWND_.load(Ordering::Acquire) as *mut _);
                unsafe {
                    if PostMessageW(Some(h), WM_TIMER, WPARAM(super::T_ANIM), LPARAM(0)).is_err() {
                        PENDING.store(false, Ordering::Release);
                    }
                }
            }
        }
    }
}
