//! Phemio – Your desktop cameo: a Dynamic Island / notch for Windows 11.
//!
//! One process, one UI thread, plus small sleeping threads for media events
//! and brightness, and short-lived threads for web lookups. A hidden "host"
//! window owns the tray icon, receives system broadcasts and schedules
//! reminders, and forwards everything to the island.

#![windows_subsystem = "windows"]

mod anim;
mod answers;
mod audio;
mod bt;
mod config;
mod connect;
mod contentsearch;
mod contentindex;
mod display;
mod qr;
mod ocr;
mod rdp;
mod search;
mod selection;
mod controls;
mod gemini;
mod lunar;
mod link;
mod gfx;
mod i18n;
mod island;
mod multiout;
mod player;
mod tzinfo;
mod media;
mod net;
mod palette;
mod netinfo;
mod settings;
mod shelf;
mod tbmon;
mod glass;
mod sysmon;
mod tools;
mod util;
mod weather;
mod webview;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::Power::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use audio::{DeviceWatcher, Volume};
use island::{Island, Mascot, Page, Peek};
use media::Media;
use settings::Settings;

pub const WM_MEDIA: u32 = WM_APP + 1;
pub const WM_VOLUME: u32 = WM_APP + 2;
pub const WM_TRAY: u32 = WM_APP + 3;
pub const WM_APPLY: u32 = WM_APP + 4;
pub const WM_SHOW_SETTINGS: u32 = WM_APP + 6;
pub const WM_WEATHER: u32 = WM_APP + 7;
pub const WM_KEYEVT: u32 = WM_APP + 9;
pub const WM_NET: u32 = WM_APP + 10;
pub const WM_BRIGHT: u32 = WM_APP + 11;
pub const WM_AUDIODEV: u32 = WM_APP + 12;
pub const WM_BT: u32 = WM_APP + 13;
pub const WM_TBMON: u32 = WM_APP + 14;
pub const WM_CTRL: u32 = WM_APP + 15;
pub const WM_BTCHANGE: u32 = WM_APP + 16;
pub const WM_LISTS: u32 = WM_APP + 19;
pub const WM_SEARCH: u32 = WM_APP + 20;
pub const WM_TRANSLATE: u32 = WM_APP + 21;
pub const WM_LINK: u32 = WM_APP + 22;
pub const WM_MULTIOUT: u32 = WM_APP + 23;
const HOTKEY_TRANSLATE: i32 = 0x5302;
const HOTKEY_SEARCH: i32 = 0x5301;
/// Alt+A Now Playing, Alt+Z previous song, Alt+X next song.
const HOTKEY_PLAYER: i32 = 0x5303;
const HOTKEY_PREV: i32 = 0x5304;
const HOTKEY_NEXT: i32 = 0x5305;
const HOTKEY_DICT: i32 = 0x5306;

const HOST_CLASS: PCWSTR = w!("PhemioHost");
const T_MINUTE: usize = 1;
const T_FOCUS: usize = 2;
const T_IDLE: usize = 3;
const T_LANG: usize = 4;
const T_CAPS: usize = 5;
const T_TRIM: usize = 6;
const T_FULLSCREEN: usize = 7;
const T_RAISE: usize = 21;
const T_BTSCAN: usize = 24;
const T_RAISE2: usize = 22;
const T_ZGUARD: usize = 23;
const T_AUDIO: usize = 8;
/// Attached to the taskbar: is the island still there? (Explorer restarting
/// takes the windows inside the taskbar with it.)
const T_DOCKCHECK: usize = 25;

const M_SETTINGS: usize = 1;
const M_ISLAND: usize = 2;
const M_MINIMAL: usize = 3;
const M_MASCOT: usize = 4;
const M_EXIT: usize = 5;

/// Shared data shown on the island pages.
pub struct Data {
    pub focus: tools::Focus,
    pub todo: tools::Todo,
    pub notes: tools::Notes,
    pub shelf: shelf::Shelf,
    pub weather: Option<weather::Weather>,
    pub weather_err: Option<String>,
    weather_busy: bool,
    pub tz2: Option<weather::Weather>,
    pub tz2_err: Option<String>,
    tz2_busy: bool,
    /// Laptop battery: (percent, charging). None on desktops.
    pub battery: Option<(u8, bool)>,
    /// Windows energy saver (battery saver) on?
    pub energy_saver: bool,
    /// Win11 24H2+ energy saver status, once Windows has told us.
    es_new: Option<bool>,
    /// Classic battery saver status.
    es_old: bool,
    /// Default audio output ("Headphones (Bose Flex 2 SoundLink)").
    pub output: String,
    /// Connected Bluetooth devices.
    pub bt: Vec<bt::BtDevice>,
    pub volume: (f32, bool),
    pub net: netinfo::NetInfo,
    /// Calendar occurrences already reminded about.
    pub online: Option<bool>,
}

impl Data {
    /// The Bluetooth device currently used for audio, if any.
    pub fn output_bt(&self) -> Option<&bt::BtDevice> {
        bt::match_endpoint(&self.output, &self.bt)
    }
}

/// Win11 24H2+ "Energy saver" status (0 off, 1 standard, 2 high savings).
const GUID_ENERGY_SAVER_STATUS: GUID = GUID::from_u128(0x550e8400_e29b_41d4_a716_446655440000);

static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Energy saver turned on / off (by Windows at the battery threshold, or from
/// Quick Settings). Pops up once per change.
pub fn energy_saver_changed(on: bool) {
    let d = data();
    if d.energy_saver == on {
        return;
    }
    d.energy_saver = on;
    let settled = STARTED.get().map(|t| t.elapsed().as_secs() >= 4).unwrap_or(false);
    if settled && config::cur().b("osd_saver") {
        let tr = i18n::tr;
        let mut sub = tr(if on { "Turned on" } else { "Turned off" }).to_string();
        if let Some((p, false)) = d.battery {
            sub.push_str(&format!(" · {p}%"));
        }
        peek(Peek::rich("saver", "leaf", if on { 0x30d158 } else { 0x8e8e93 }, None, "Energy saver", &sub, if on { "ON" } else { "OFF" }, 2000, Some(Page::Controls)));
    }
    app().island_refresh();
}

fn read_power() {
    let mut ps = SYSTEM_POWER_STATUS::default();
    unsafe {
        if GetSystemPowerStatus(&mut ps).is_ok() {
            let d = data();
            d.battery = (ps.BatteryFlag != 128 && ps.BatteryLifePercent <= 100).then_some((ps.BatteryLifePercent, ps.ACLineStatus == 1));
            // Only a fallback: on 24H2 this flag stays 0 while energy saver
            // is on above 20 % battery; the power notifications are the truth.
            if d.es_new.is_none() && ps.SystemStatusFlag == 1 {
                d.energy_saver = true;
            }
        }
    }
    // Developer aid: ISLET_FAKE_BATT=percent,charging,saver
    if let Ok(v) = std::env::var("ISLET_FAKE_BATT") {
        let p: Vec<u8> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        if p.len() == 3 {
            data().battery = Some((p[0], p[1] != 0));
            data().energy_saver = p[2] != 0;
        }
    }
}

pub struct App {
    pub host: HWND,
    pub island: Option<Box<Island>>,
    pub settings: Option<Box<Settings>>,
    pub tbmon: Option<Box<tbmon::Monitor>>,
    media: Option<Media>,
    volume: Option<Volume>,
    devwatch: Option<DeviceWatcher>,
    keyhook: Option<sysmon::KeyHook>,
    hotkey_on: bool,
    hotkey_tr: bool,
    hotkey_music: bool,
    hotkey_dict: bool,
    netwatch: Option<sysmon::NetWatch>,
    brightness: bool,
    shell_msg: u32,
    taskbar_created: u32,
    ac_online: u8,
    fullscreen: bool,
    apply_pending: bool,
    last_lang: u16,
    online: Option<bool>,
    autostart: Option<bool>,
    /// Announce the output device after the next Bluetooth scan.
    announce: bool,
    bt_scans: u32,
    fs_votes: u32,
    /// Bluetooth devices seen in the last scan (None before the first).
    bt_prev: Option<Vec<String>>,
    /// Last radio / microphone state, to announce changes.
    ctrl_prev: Option<controls::State>,
}

static mut APP: *mut App = std::ptr::null_mut();
static mut DATA: *mut Data = std::ptr::null_mut();

/// The single UI-thread application state.
#[allow(static_mut_refs)]
pub fn app() -> &'static mut App {
    unsafe { &mut *APP }
}

#[allow(static_mut_refs)]
pub fn data() -> &'static mut Data {
    unsafe { &mut *DATA }
}

pub fn request_apply() {
    let a = app();
    if !a.apply_pending {
        a.apply_pending = true;
        unsafe {
            let _ = PostMessageW(Some(a.host), WM_APPLY, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn open_settings(tab: usize) {
    let a = app();
    if let Some(s) = a.settings.as_mut() {
        s.show(tab);
    } else {
        a.settings = Settings::open(tab);
    }
}

fn peek(p: Peek) {
    if let Some(i) = app().island.as_mut() {
        i.show_peek(p);
    }
}

pub fn show_menu() {
    let a = app();
    let c = config::cur();
    unsafe {
        let Ok(m) = CreatePopupMenu() else { return };
        let chk = |b: bool| if b { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(m, MF_STRING, M_SETTINGS, &HSTRING::from(i18n::tr("Settings…")));
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(m, MF_STRING | chk(c.b("island")), M_ISLAND, &HSTRING::from(i18n::tr("Show island")));
        let _ = AppendMenuW(m, MF_STRING | chk(c.b("minimal")), M_MINIMAL, &HSTRING::from(i18n::tr("Minimal notch")));
        let _ = AppendMenuW(m, MF_STRING | chk(c.b("mascot")), M_MASCOT, &HSTRING::from(i18n::tr("Mascot")));
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(m, MF_STRING, M_EXIT, &HSTRING::from(i18n::tr("Quit Phemio")));
        let _ = SetMenuDefaultItem(m, M_SETTINGS as u32, 0);
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let _ = SetForegroundWindow(a.host);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, p.x, p.y, None, a.host, None).0 as usize;
        let _ = DestroyMenu(m);
        let flip = |k: &str| {
            config::update(|c| {
                let v = c.b(k);
                c.set(k, !v as i32)
            });
            request_apply();
        };
        match cmd {
            M_SETTINGS => open_settings(0),
            M_ISLAND => flip("island"),
            M_MINIMAL => flip("minimal"),
            M_MASCOT => flip("mascot"),
            M_EXIT => {
                let _ = PostMessageW(Some(a.host), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            _ => {}
        }
    }
}

impl App {
    pub fn media_cmd(&self, c: media::Cmd) {
        if let Some(m) = &self.media {
            m.send(c);
        }
    }

    pub fn nudge_volume(&mut self, delta: f32) {
        if let Some(v) = &self.volume {
            v.nudge(delta);
        } else if let Some(v) = Volume::open(None) {
            v.nudge(delta);
        }
    }

    pub fn set_volume(&mut self, level: f32) {
        match &self.volume {
            Some(v) => v.set(level),
            None => {
                if let Some(v) = Volume::open(None) {
                    v.set(level);
                }
            }
        }
        data().volume = (level, false);
    }

    pub fn toggle_mute(&mut self) {
        let v = self.volume.take().or_else(|| Volume::open(None));
        if let Some(v) = &v {
            v.toggle_mute();
            data().volume = v.get();
        }
        if config::cur().b("osd_volume") {
            self.volume = v;
        }
    }

    /// Re-read the default output device + volume, and rescan Bluetooth.
    fn refresh_audio(&mut self, announce: bool) {
        if self.volume.is_some() {
            self.volume = None;
            self.volume = Volume::open(Some((self.host, WM_VOLUME)));
        }
        let name = audio::default_output_name().unwrap_or_default();
        let changed = name != data().output;
        data().output = name;
        if let Some(v) = self.volume.as_ref().map(|v| v.get()).or_else(|| Volume::open(None).map(|v| v.get())) {
            data().volume = v;
        }
        if changed && announce {
            self.announce = true;
        }
        bt::spawn_scan(self.host, WM_BT);
    }

    fn on_bt(&mut self) {
        let Some((mut list, net)) = bt::take() else { return };
        data().net = net;
        // Developer aid: ISLET_FAKE_BT="Name:battery;Name:battery…" (the first is the audio output).
        if let Ok(v) = std::env::var("ISLET_FAKE_BT") {
            list = v.split(';').filter_map(|e| e.split_once(':')).map(|(n, b)| bt::BtDevice::simple(n.trim(), b.trim().parse().ok())).collect();
            if let Some(first) = list.first() {
                data().output = format!("Headphones ({})", first.name);
            }
        }
        self.bt_scans += 1;
        // Any Bluetooth device connecting or disconnecting gets a popup (the
        // audio-output one is announced below with its sound details).
        let names: Vec<String> = list.iter().map(|b| b.name.clone()).collect();
        if let Some(prev) = self.bt_prev.as_ref() {
            if config::cur().b("osd_btdev") {
                let out = bt::match_endpoint(&data().output, &list).map(|b| b.name.clone());
                const CATS: [&str; 4] = ["bt0", "bt1", "bt2", "bt3"];
                let mut k = 0;
                for b in list.iter().filter(|b| !prev.contains(&b.name)) {
                    if self.announce && out.as_deref() == Some(b.name.as_str()) {
                        continue;
                    }
                    let sub = format!("{} · {}", i18n::tr("Connected"), i18n::tr(b.kind.label()));
                    let right = b.battery.map(|p| format!("{p}%")).unwrap_or_default();
                    peek(Peek::rich(CATS[k % 4], b.kind.glyph(), b.kind.color(), b.battery.map(|p| p as f32 / 100.0), &b.name, &sub, &right, 3200, Some(Page::Devices)));
                    k += 1;
                }
                for n in prev.iter().filter(|n| !names.contains(n)) {
                    let kind = bt::Kind::from_name(n);
                    let sub = format!("{} · {}", i18n::tr("Disconnected"), i18n::tr(kind.label()));
                    peek(Peek::rich(CATS[k % 4], kind.glyph(), 0x8e8e93, None, n, &sub, "", 2600, Some(Page::Devices)));
                    k += 1;
                }
            }
        }
        self.bt_prev = Some(names);
        data().bt = list;
        if let Some(i) = self.island.as_mut() {
            i.devices_changed();
        }
        if self.announce && config::cur().b("osd_audio") && !data().output.is_empty() {
            self.announce = false;
            let d = data();
            let first = self.bt_scans == 1;
            let btd = d.output_bt().cloned();
            // At start-up only announce a Bluetooth device (that's the interesting one).
            if !first || btd.is_some() {
                let name = d.output.clone();
                let lower = name.to_lowercase();
                let head = btd.is_some() || ["headphone", "headset", "buds", "airpods", "earphone", "hands-free"].iter().any(|k| lower.contains(k));
                let glyph = if head { "\u{E7F6}" } else { "\u{E7F5}" };
                let title = bt::inner(&name).to_string();
                let kind = name.split(" (").next().unwrap_or("Audio output").to_string();
                let kind_t = i18n::tr(&kind).to_lowercase();
                let sub = if btd.is_some() { format!("{} · Bluetooth {}", i18n::tr("Connected"), kind_t) } else { format!("{} {}", i18n::tr("Now playing through"), kind_t) };
                let batt = btd.as_ref().and_then(|b| b.battery);
                let right = batt.map(|b| format!("{b}%")).unwrap_or_default();
                peek(Peek::rich("audio", glyph, 0x5ac8fa, batt.map(|b| b as f32 / 100.0), &title, &sub, &right, 3500, Some(Page::Devices)));
            }
        }
        self.announce = false;
        self.island_refresh();
    }

    /// Radios, airplane mode or microphone changed.
    #[allow(clippy::needless_return)]
    fn on_ctrl(&mut self, first: bool) {
        let st = controls::state();
        let prev = self.ctrl_prev.replace(st.clone());
        let c = config::cur();
        if let (Some(p), false, true) = (prev, first, c.b("osd_radios")) {
            let onoff = |b: bool| if b { "ON" } else { "OFF" };
            if st.airplane != p.airplane {
                if let Some(on) = st.airplane {
                    peek(Peek::rich("radio_air", "\u{E709}", if on { 0xff9f0a } else { 0x8e8e93 }, None, "Airplane mode", if on { "Wireless connections are off" } else { "Wireless connections are back" }, onoff(on), 2200, Some(Page::Controls)));
                }
            } else {
                if st.wifi != p.wifi {
                    if let Some(on) = st.wifi {
                        peek(Peek::rich("radio_wifi", "\u{E701}", if on { 0x30d158 } else { 0x8e8e93 }, None, "Wi-Fi", if on { "Turned on" } else { "Turned off" }, onoff(on), 2000, Some(Page::Controls)));
                    }
                }
                if st.bt != p.bt {
                    if let Some(on) = st.bt {
                        peek(Peek::rich("radio_bt", "\u{E702}", if on { 0x0a84ff } else { 0x8e8e93 }, None, "Bluetooth", if on { "Turned on" } else { "Turned off" }, onoff(on), 2000, Some(Page::Controls)));
                    }
                }
            }
            if st.mic_muted != p.mic_muted {
                if let Some(m) = st.mic_muted {
                    let name = if st.mic_name.is_empty() { "Microphone".to_string() } else { st.mic_name.clone() };
                    // Same wording as the Controls tile: "Mute mic" ON = muted.
                    let sub = format!("{} · {}", i18n::tr(if m { "Microphone muted" } else { "Microphone on" }), name);
                    peek(Peek::rich("mic", if m { "\u{EC54}" } else { "\u{E720}" }, if m { 0xff453a } else { 0x30d158 }, None, "Mute mic", &sub, if m { "ON" } else { "OFF" }, 2000, Some(Page::Controls)));
                }
            }
        }
        // Bluetooth switched on/off: the device list changes too.
        if prev_bt_changed(&st, &self.bt_prev) {
            unsafe { SetTimer(Some(self.host), T_BTSCAN, 1500, None) };
        }
        self.island_refresh();
    }

    pub fn settings_hwnd(&self) -> HWND {
        self.settings.as_ref().map(|s| s.hwnd).unwrap_or_default()
    }

    pub fn settings_closed(&mut self) {
        self.settings = None;
        unsafe { SetTimer(Some(self.host), T_TRIM, 1500, None) };
    }

    fn island_refresh(&mut self) {
        if let Some(i) = self.island.as_mut() {
            i.refresh();
        }
        if let Some(s) = &self.settings {
            s.invalidate();
        }
    }

    /// Re-check the weather and the second clock. `force` ignores the cache.
    pub fn refresh_weather(&mut self, force: bool) {
        let c = config::cur();
        let d = data();
        let now = util::unix_now();
        let unit_f = c.i("weather_unit") == 1;
        let city = c.s("weather_city");
        if force {
            d.weather_err = None;
        }
        if !city.is_empty() && !d.weather_busy {
            let provider = c.i("weather_src");
            // The world clock shows this place's weather too: its update time applies.
            let every = if c.b("page_clock") { (c.i("tz_refresh").clamp(5, 180) as i64 * 60).min(30 * 60) } else { 30 * 60 };
            let stale = d.weather.as_ref().map(|w| now - w.fetched > every || w.unit_f != unit_f || w.provider != provider).unwrap_or(true);
            if force || (stale && (d.weather_err.is_none() || force)) {
                d.weather_busy = true;
                if force {
                    d.weather = None;
                }
                // Always by the city's name (looked up each time), never by location.
                weather::spawn(weather::Kind::Weather, city, None, unit_f, c.i("weather_src"), self.host, WM_WEATHER);
            }
        } else if city.is_empty() {
            d.weather = None;
        }
        let city2 = c.s("tz2_city");
        if !city2.is_empty() && !d.tz2_busy {
            let every = config::cur().i("tz_refresh").clamp(5, 180) as i64 * 60;
            let stale = d.tz2.as_ref().map(|w| now - w.fetched > every).unwrap_or(true);
            if force || (stale && d.tz2_err.is_none()) {
                if force {
                    d.tz2_err = None;
                    d.tz2 = None;
                }
                d.tz2_busy = true;
                weather::spawn(weather::Kind::Tz, city2, None, unit_f, 0, self.host, WM_WEATHER);
            }
        } else if city2.is_empty() {
            d.tz2 = None;
            d.tz2_err = None;
        }
        self.island_refresh();
    }

    fn on_weather(&mut self) {
        let d = data();
        let c = config::cur();
        for o in weather::take_results() {
            let (key, busy_city) = match o.kind {
                weather::Kind::Weather => ("weather", c.s("weather_city")),
                weather::Kind::Tz => ("tz2", c.s("tz2_city")),
            };
            match o.kind {
                weather::Kind::Weather => d.weather_busy = false,
                weather::Kind::Tz => d.tz2_busy = false,
            }
            if o.city != busy_city {
                continue; // answer for an old query
            }
            if let Some(g) = &o.geo {
                config::update(|c| c.set_s(&format!("{key}_place"), &g.name));
            }
            match o.kind {
                weather::Kind::Weather => {
                    d.weather_err = o.error.clone();
                    // No such city: show that, not the old place's weather.
                    if o.error.as_deref() == Some(weather::NOT_FOUND) {
                        d.weather = None;
                    }
                    if o.weather.is_some() {
                        d.weather = o.weather;
                    }
                }
                weather::Kind::Tz => {
                    d.tz2_err = o.error.clone();
                    if o.error.as_deref() == Some(weather::NOT_FOUND) {
                        d.tz2 = None;
                    }
                    if o.weather.is_some() {
                        d.tz2 = o.weather;
                    }
                }
            }
        }
        self.island_refresh();
    }

    /// Focus timer started/paused/changed: arm the end-of-phase timer.
    pub fn focus_changed(&mut self) {
        let f = &data().focus;
        unsafe {
            if f.running {
                SetTimer(Some(self.host), T_FOCUS, (f.remaining().as_millis() as u32).max(50) + 30, None);
            } else {
                let _ = KillTimer(Some(self.host), T_FOCUS);
            }
        }
        self.island_refresh();
    }

    fn on_focus_timer(&mut self) {
        if let Some(ended) = data().focus.on_timer() {
            let c = config::cur();
            if c.b("focus_sound") {
                audio::chime();
            }
            let next = data().focus.phase;
            let (title, sub) = match ended {
                tools::Phase::Focus => ("Time for a break", format!("{} {}", i18n::tr(next.name()), i18n::tr("started"))),
                _ => ("Break's over", "Ready to focus again?".to_string()),
            };
            let col = if ended == tools::Phase::Focus { 0x5ac8fa } else { 0x34c759 };
            peek(Peek::rich("focus", "\u{E916}", col, Some(1.0), title, &sub, "", 6000, Some(Page::Focus)));
        }
        self.focus_changed();
    }

    /// Once a minute: reminders, stale weather.
    /// Re-read the battery; redraw if the level or charging state changed.
    fn battery_changed(&mut self) {
        let before = data().battery;
        read_power();
        if data().battery != before {
            self.island_refresh();
        }
    }

    fn on_minute(&mut self) {
        unsafe { SetTimer(Some(self.host), T_MINUTE, util::ms_to_next_minute(), None) };
        // Safety net in case a battery notification is missed.
        self.battery_changed();
        let c = config::cur();
        if !c.b("island") {
            return;
        }
        let d = data();
        if c.b("remind_todo") && c.b("page_todo") {
            for t in d.todo.take_due() {
                peek(Peek::rich("todo", "\u{E823}", 0xff9f0a, None, "Time's up!", &t, "", 9000, Some(Page::Todo)));
            }
        }
        if c.b("page_weather") || c.b("page_clock") {
            self.refresh_weather(false);
        }
        // Bluetooth battery levels: every minute while the Devices page is open,
        // every 3 minutes otherwise.
        let on_devices = self.island.as_ref().map(|i| i.expanded && i.page == Page::Devices).unwrap_or(false);
        if on_devices || util::now().wMinute % 3 == 0 {
            bt::spawn_scan(self.host, WM_BT);
        }
        if !data().focus.active() {
            self.island_refresh();
        }
    }

    /// Idle → mascot eyes; back → "hi!".
    /// Any input while the mascot shows: it reacts for a moment, then the
    /// island's usual content comes back.
    fn wake_mascot(&mut self) {
        if std::env::var("ISLET_LOG_MASCOT").is_ok() {
            util::log("wake (key)");
        }
        // Any key: the mascot slips away at once (fade-out) — or, when it's
        // there always, it just wakes up.
        let always = config::cur().i("mascot") == 2;
        if let Some(isl) = self.island.as_mut() {
            if always {
                if isl.mascot == Mascot::Doze {
                    isl.set_mascot(Mascot::Awake);
                }
            } else {
                // Waves goodbye, then fades out.
                isl.say_bye();
            }
        }
        unsafe { SetTimer(Some(self.host), T_IDLE, 1500, None) };
    }

    fn on_idle_timer(&mut self) {
        let c = config::cur();
        let poll = |ms: u32| unsafe {
            SetTimer(Some(app().host), T_IDLE, ms, None);
        };
        let Some(isl) = self.island.as_mut() else {
            poll(30_000);
            return;
        };
        // Developer aid (screenshots): ISLET_DEMO_BYE=<ms> says goodbye once, that long after start.
        if let Some(ms) = std::env::var("ISLET_DEMO_BYE").ok().and_then(|v| v.parse::<u64>().ok()) {
            static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if isl.t0.elapsed().as_millis() as u64 >= ms && !DONE.swap(true, std::sync::atomic::Ordering::Relaxed) {
                isl.masc_dismissed = Some(std::time::Instant::now());
                isl.say_bye();
            }
        }
        // "Always": on show whenever the island is closed, except for a while
        // after you click it away (or used the island).
        if c.i("mascot") == 2 {
            let thr = c.i("mascot_sec").clamp(10, 3600) as u64;
            let in_use = isl.expanded || isl.search.is_some() || isl.trans.is_some();
            if in_use {
                isl.masc_dismissed = Some(std::time::Instant::now());
            }
            let away = isl.masc_dismissed.map(|t| t.elapsed().as_secs() < thr).unwrap_or(false);
            if in_use || away {
                // (A goodbye in progress finishes by itself.)
                if isl.mascot != Mascot::Off && isl.mascot != Mascot::Bye {
                    if in_use {
                        isl.set_mascot(Mascot::Off);
                    } else {
                        isl.say_bye();
                    }
                }
                sysmon::WAKE_ON_KEY.store(false, std::sync::atomic::Ordering::Relaxed);
                sysmon::TAP_KEYS.store(false, std::sync::atomic::Ordering::Relaxed);
                poll(1000);
                return;
            }
            let idle = sysmon::idle_ms();
            let want = if idle >= thr as u32 * 1000 + 3 * 60_000 { Mascot::Doze } else { Mascot::Awake };
            if isl.mascot == Mascot::Off || (isl.mascot == Mascot::Awake && want == Mascot::Doze) {
                isl.set_mascot(want);
            } else if isl.mascot == Mascot::Doze && want == Mascot::Awake {
                isl.set_mascot(Mascot::Awake);
            }
            sysmon::WAKE_ON_KEY.store(isl.mascot == Mascot::Doze, std::sync::atomic::Ordering::Relaxed);
            // Awake cat: its paws drum along as you type.
            sysmon::TAP_KEYS.store(isl.mascot == Mascot::Awake && c.i("mascot_style") == 1, std::sync::atomic::Ordering::Relaxed);
            poll(if isl.mascot == Mascot::Doze { 200 } else { 1000 });
            return;
        }
        sysmon::TAP_KEYS.store(false, std::sync::atomic::Ordering::Relaxed);
        let busy = data().focus.running || isl.expanded || isl.media.playing;
        if !c.b("mascot") || busy {
            if isl.mascot != Mascot::Off && isl.mascot != Mascot::Bye {
                if isl.expanded || !c.b("mascot") {
                    isl.set_mascot(Mascot::Off);
                } else {
                    isl.say_bye();
                }
            }
            poll(if isl.media.playing { 5_000 } else { 20_000 });
            return;
        }
        let idle = sysmon::idle_ms();
        let thr = c.i("mascot_sec").clamp(10, 3600) as u32 * 1000;
        if std::env::var("ISLET_LOG_MASCOT").is_ok() {
            util::log(&format!("idle {idle} thr {thr} mascot {:?}", isl.mascot));
        }
        if idle >= thr {
            // Dozes off after a few more minutes of quiet.
            let m = if idle >= thr + 3 * 60_000 { Mascot::Doze } else { Mascot::Awake };
            isl.set_mascot(m);
            sysmon::WAKE_ON_KEY.store(true, std::sync::atomic::Ordering::Relaxed);
            // Clicks, touchpad and mouse moves show up here within 0.1 s; keys
            // also come through the hook at once.
            poll(100);
        } else {
            sysmon::WAKE_ON_KEY.store(false, std::sync::atomic::Ordering::Relaxed);
            // You're back (mouse / touchpad): a goodbye, then the island.
            isl.say_bye();
            poll((thr - idle).clamp(500, 20_000));
        }
    }

    /// Bring every component in line with the current configuration.
    fn apply(&mut self) {
        let _p = util::Prof::new("app.apply");
        self.apply_pending = false;
        let c = config::cur();
        let on = c.b("island");
        // Link: one mouse and keyboard across PCs.
        if c.b("link_on") {
            link::start(self.host.0 as isize, WM_LINK);
        } else {
            link::stop();
        }
        // Alt+S → search.
        let want_hk = on && c.b("search_hotkey");
        if want_hk != self.hotkey_on {
            unsafe {
                use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_NOREPEAT};
                if want_hk {
                    self.hotkey_on = RegisterHotKey(Some(self.host), HOTKEY_SEARCH, MOD_ALT | MOD_NOREPEAT, b'S' as u32).is_ok();
                    if !self.hotkey_on {
                        util::log("Alt+S is already taken by another app");
                    }
                } else {
                    let _ = UnregisterHotKey(Some(self.host), HOTKEY_SEARCH);
                    self.hotkey_on = false;
                }
            }
        }
        // Alt+Q → translate the selection.
        let want_tq = on && c.b("translate_hotkey");
        if want_tq != self.hotkey_tr {
            unsafe {
                use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_NOREPEAT};
                if want_tq {
                    self.hotkey_tr = RegisterHotKey(Some(self.host), HOTKEY_TRANSLATE, MOD_ALT | MOD_NOREPEAT, b'Q' as u32).is_ok();
                    if !self.hotkey_tr {
                        util::log("Alt+Q is already taken by another app");
                    }
                } else {
                    let _ = UnregisterHotKey(Some(self.host), HOTKEY_TRANSLATE);
                    self.hotkey_tr = false;
                }
            }
        }
        // Alt+W → dictionary.
        let want_dict = on && c.b("dict_hotkey");
        if want_dict != self.hotkey_dict {
            unsafe {
                use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_NOREPEAT};
                if want_dict {
                    self.hotkey_dict = RegisterHotKey(Some(self.host), HOTKEY_DICT, MOD_ALT | MOD_NOREPEAT, b'W' as u32).is_ok();
                    if !self.hotkey_dict {
                        util::log("Alt+W is already taken by another app");
                    }
                } else {
                    let _ = UnregisterHotKey(Some(self.host), HOTKEY_DICT);
                    self.hotkey_dict = false;
                }
            }
        }
        // Alt+A / Alt+Z / Alt+X → Now Playing, previous, next.
        let want_mu = on && c.b("music_hotkeys");
        if want_mu != self.hotkey_music {
            unsafe {
                use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_NOREPEAT};
                let keys = [(HOTKEY_PLAYER, b'A'), (HOTKEY_PREV, b'Z'), (HOTKEY_NEXT, b'X')];
                if want_mu {
                    for (id, k) in keys {
                        if RegisterHotKey(Some(self.host), id, MOD_ALT | MOD_NOREPEAT, k as u32).is_err() {
                            util::log(&format!("Alt+{} is already taken by another app", k as char));
                        }
                    }
                } else {
                    for (id, _) in keys {
                        let _ = UnregisterHotKey(Some(self.host), id);
                    }
                }
                self.hotkey_music = want_mu;
            }
        }
        if on {
            match self.island.as_mut() {
                Some(i) => i.relayout(),
                None => self.island = Island::new(),
            }
            unsafe {
                if c.b("island_in_tb") {
                    SetTimer(Some(self.host), T_DOCKCHECK, 1000, None);
                } else {
                    let _ = KillTimer(Some(self.host), T_DOCKCHECK);
                }
            }
        } else if let Some(mut i) = self.island.take() {
            i.destroy();
        }
        let want = |k: &str| on && c.b(k);
        let want_media = want("page_home") || want("osd_track");
        if want_media && self.media.is_none() {
            self.media = Some(Media::start(self.host, WM_MEDIA));
        } else if !want_media {
            self.media = None;
        }
        if want("osd_volume") && self.volume.is_none() {
            self.volume = Volume::open(Some((self.host, WM_VOLUME)));
        } else if !want("osd_volume") {
            self.volume = None;
        }
        if on {
            if self.devwatch.is_none() {
                self.devwatch = DeviceWatcher::start(self.host, WM_AUDIODEV);
                self.refresh_audio(false);
            }
        } else {
            self.devwatch = None;
        }
        sysmon::WATCH_KEYS.store(want("osd_keys"), std::sync::atomic::Ordering::Relaxed);
        if want("osd_keys") || c.b("mascot") || c.b("mo_on") {
            if self.keyhook.is_none() {
                self.keyhook = sysmon::KeyHook::start(self.host, WM_KEYEVT);
                self.last_lang = sysmon::current_lang();
            }
        } else {
            self.keyhook = None;
        }
        if want("osd_network") {
            if self.netwatch.is_none() {
                self.online = None;
                self.netwatch = sysmon::NetWatch::start(self.host, WM_NET);
            }
        } else {
            self.netwatch = None;
        }
        let laptop = {
            let mut ps = SYSTEM_POWER_STATUS::default();
            unsafe { GetSystemPowerStatus(&mut ps).is_ok() && ps.BatteryFlag != 128 }
        };
        if want("osd_brightness") && laptop && !util::is_wine() {
            if !self.brightness {
                sysmon::start_brightness(self.host, WM_BRIGHT);
                self.brightness = true;
            }
        } else if self.brightness {
            sysmon::stop_brightness();
            self.brightness = false;
        }
        if self.autostart != Some(c.b("autostart")) {
            self.autostart = Some(c.b("autostart"));
            util::set_autostart(c.b("autostart"));
        }
        // Taskbar monitor (independent of the island)
        if c.b("tbm") {
            match self.tbmon.as_mut() {
                Some(m) => m.refresh(),
                None => self.tbmon = Some(tbmon::Monitor::new(self.host, WM_TBMON)),
            }
        } else if let Some(mut m) = self.tbmon.take() {
            m.destroy();
        }
        // Transparency (taskbar / File Explorer)
        glass::sync(self.host);
        if let Some(s) = self.settings.as_mut() {
            s.refresh_theme();
        }
        self.focus_changed();
        self.check_fullscreen();
        unsafe { SetTimer(Some(self.host), T_IDLE, 5000, None) };
    }

    /// A settings slider is being dragged: show just what that setting changes
    /// (everything else is applied once, when it's let go).
    pub fn apply_live(&mut self, key: &str) {
        let _p = util::Prof::new("app.apply_live");
        if key == "glass_tb_lvl" || key == "glass_ex_lvl" {
            glass::sync_live();
            return;
        }
        if key.starts_with("tbm_") {
            if let Some(m) = self.tbmon.as_mut() {
                m.refresh();
            }
            return;
        }
        const ISLAND: [&str; 16] = ["col_luck_gl", "col_luck_gr", "island_x", "island_y", "island_size", "col_h", "exp_w", "exp_h", "font_scale", "cover_size", "peek_cover", "wx_text", "wc_size", "home_clock_size", "col_t_size", "col_d_size"];
        if ISLAND.contains(&key) {
            if let Some(i) = self.island.as_mut() {
                i.relayout();
            }
        }
    }

    fn relayout_all(&mut self) {
        if let Some(i) = self.island.as_mut() {
            i.relayout();
        }
    }

    /// Hide the island while a full-screen app is in front.
    /// Hide the island for a full-screen app. A shell flyout (Quick Settings,
    /// notification centre, calendar, Start…) never counts, the verdict has
    /// to hold on two checks in a row before hiding, and while hidden it is
    /// re-checked every second so the island always comes back.
    fn check_fullscreen(&mut self) {
        let fs = config::cur().b("hide_fullscreen") && foreground_is_fullscreen(self);
        unsafe {
            if fs && !self.fullscreen {
                self.fs_votes += 1;
                if self.fs_votes < 2 {
                    SetTimer(Some(self.host), T_FULLSCREEN, 350, None);
                    return;
                }
            }
            self.fs_votes = 0;
            if fs != self.fullscreen {
                self.fullscreen = fs;
                if let Some(i) = self.island.as_mut() {
                    i.set_suppressed(fs);
                    if !fs {
                        i.raise();
                    }
                }
            }
            if self.fullscreen {
                SetTimer(Some(self.host), T_FULLSCREEN, 1000, None);
            }
        }
    }

    fn add_tray(&self) {
        unsafe {
            let mut nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.host,
                uID: 1,
                uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
                uCallbackMessage: WM_TRAY,
                hIcon: util::app_icon(true),
                ..Default::default()
            };
            let tip = util::wide("Phemio – Dynamic Island");
            nid.szTip[..tip.len()].copy_from_slice(&tip);
            let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        }
    }

    fn remove_tray(&self) {
        unsafe {
            let nid = NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: self.host, uID: 1, ..Default::default() };
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        }
    }

    fn shutdown(&mut self) {
        config::flush();
        glass::shutdown();
        contentindex::shutdown();
        link::stop();
        self.media = None;
        self.volume = None;
        self.devwatch = None;
        self.keyhook = None;
        self.netwatch = None;
        sysmon::stop_brightness();
        if let Some(mut i) = self.island.take() {
            i.destroy();
        }
        if let Some(mut m) = self.tbmon.take() {
            m.destroy();
        }
        self.remove_tray();
    }
}

fn prev_bt_changed(st: &controls::State, prev: &Option<Vec<String>>) -> bool {
    // Radio off but devices still listed (or on with none listed yet).
    match st.bt {
        Some(false) => prev.as_ref().map(|p| !p.is_empty()).unwrap_or(false),
        Some(true) => prev.as_ref().map(|p| p.is_empty()).unwrap_or(false),
        None => false,
    }
}

/// Shell overlays that cover the screen but aren't "full-screen apps".
const SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "XamlExplorerHostIslandWindow",
    "MultitaskingViewFrame",
    "ForegroundStaging",
    "TaskSwitcherWnd",
    "Windows.UI.Core.CoreWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "NotifyIconOverflowWindow",
    "LockScreenBackstopFrame",
    "ControlCenterWindow",
    "Xaml_WindowedPopupClass",
    "Windows.UI.Input.InputSite.WindowClass",
    "Shell_InputSwitchTopLevelWindow",
    "Shell_Flyout",
    "TrayNotifyWnd",
    "TrayClockWClass",
    "Microsoft.UI.Content.DesktopChildSiteBridge",
    "ControlCenterWindow",
    "Windows.UI.Input.InputSite.WindowClass",
];

const SHELL_PROCESSES: &[&str] = &[
    "explorer.exe",
    "shellexperiencehost.exe",
    "shellhost.exe",
    "startmenuexperiencehost.exe",
    "searchhost.exe",
    "searchapp.exe",
    "textinputhost.exe",
    "lockapp.exe",
    "widgets.exe",
    "widgetboard.exe",
    "screenclippinghost.exe",
    "snippingtool.exe",
    "notificationui.exe",
    "notificationcenter.exe",
    "controlcenter.exe",
    "peopleexperiencehost.exe",
    "windowsinternal.composableshell.experiences.textinput.inputapp.exe",
    "islet.exe",
    "marisland.exe",
    "dewdrop.exe",
    "phemio.exe",
];

fn foreground_is_fullscreen(app: &App) -> bool {
    let (fs, why) = fullscreen_verdict(app);
    if fs != app.fullscreen {
        util::log(&format!("{} island: {why}", if fs { "hide" } else { "show" }));
    }
    fs
}

/// Is a real full-screen app in front? The foreground window must cover the
/// whole screen the island is on. Windows' own "busy / full screen" signal is
/// only used to confirm that — on its own it can stay stuck after a shell
/// flyout, which used to hide the island behind ordinary windows.
fn fullscreen_verdict(app: &App) -> (bool, String) {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || util::is_cloaked(fg) || !IsWindowVisible(fg).as_bool() {
            return (false, "no visible foreground window".into());
        }
        let ours = [app.host, app.island.as_ref().map(|i| i.hwnd).unwrap_or_default(), app.settings_hwnd()];
        if ours.contains(&fg) {
            return (false, "Phemio is in front".into());
        }
        let class = util::class_name(fg);
        let proc = util::process_name(fg);
        let who = format!("{proc} [{class}]");
        if SHELL_CLASSES.contains(&class.as_str()) {
            return (false, format!("{who} is part of the shell"));
        }
        if proc.is_empty() || SHELL_PROCESSES.contains(&proc.as_str()) {
            return (false, format!("{who} is part of the shell"));
        }
        let ex = GetWindowLongW(fg, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TOOLWINDOW.0 != 0 || ex & WS_EX_NOACTIVATE.0 != 0 {
            return (false, format!("{who} is a popup"));
        }
        let mut r = RECT::default();
        if GetWindowRect(fg, &mut r).is_err() {
            return (false, format!("{who}: no rectangle"));
        }
        let m = util::primary_monitor().full;
        let covers = r.left <= m.left + 1 && r.top <= m.top + 1 && r.right >= m.right - 1 && r.bottom >= m.bottom - 1;
        if !covers {
            return (false, format!("{who} doesn't cover the screen ({},{} {}x{})", r.left, r.top, r.right - r.left, r.bottom - r.top));
        }
        let style = GetWindowLongW(fg, GWL_STYLE) as u32;
        if style & WS_CAPTION.0 != WS_CAPTION.0 {
            return (true, format!("{who} covers the screen without a title bar"));
        }
        // A captioned window covering everything is usually just maximized
        // with an auto-hiding taskbar — unless Windows says it's full screen.
        let quns = SHQueryUserNotificationState().ok();
        if quns == Some(QUNS_RUNNING_D3D_FULL_SCREEN) || quns == Some(QUNS_PRESENTATION_MODE) {
            return (true, format!("{who} covers the screen and Windows reports full screen"));
        }
        (false, format!("{who} is maximized, not full screen"))
    }
}

/// A window was shown or hidden somewhere — e.g. Quick Settings, the
/// notification centre or the calendar flyout opening or closing. The shell
/// may re-stack the taskbar on top then, so the island re-raises itself
/// shortly after (coalesced through a timer, so this stays cheap).
unsafe extern "system" fn on_show_hide(_h: windows::Win32::UI::Accessibility::HWINEVENTHOOK, ev: u32, w: HWND, obj: i32, _c: i32, _t: u32, _ms: u32) {
    // Show / hide of a window, or windows re-stacked (EVENT_OBJECT_REORDER).
    if APP.is_null() || w.is_invalid() || (ev != EVENT_OBJECT_REORDER && obj != 0) {
        return;
    }
    let a = app();
    if a.island.is_none() {
        return;
    }
    SetTimer(Some(a.host), T_RAISE, 30, None);
}

unsafe extern "system" fn on_foreground(_h: windows::Win32::UI::Accessibility::HWINEVENTHOOK, _ev: u32, _w: HWND, _o: i32, _c: i32, _t: u32, _ms: u32) {
    if APP.is_null() {
        return;
    }
    let a = app();
    if let Some(i) = a.island.as_mut() {
        // Clicking the tray / clock activates the taskbar, and Explorer then
        // stacks it on top — answer right away and once more shortly after.
        i.ensure_on_top();
    }
    SetTimer(Some(a.host), T_RAISE, 60, None);
    SetTimer(Some(a.host), T_FULLSCREEN, 300, None);
}

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if APP.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let a = app();
    let c = config::cur();
    if msg == a.shell_msg && a.shell_msg != 0 {
        const HSHELL_WINDOWACTIVATED: usize = 4;
        const HSHELL_RUDEAPPACTIVATED: usize = 0x8004;
        if wp.0 == HSHELL_WINDOWACTIVATED || wp.0 == HSHELL_RUDEAPPACTIVATED || wp.0 == 1 {
            if let Some(i) = a.island.as_ref() {
                i.raise();
            }
        }
        SetTimer(Some(hwnd), T_FULLSCREEN, 300, None);
        return LRESULT(0);
    }
    if msg == a.taskbar_created && a.taskbar_created != 0 {
        glass::on_explorer_restart(hwnd);
        a.add_tray();
        if let Some(m) = a.tbmon.as_mut() {
            m.attach();
            m.refresh();
        }
        a.relayout_all();
        return LRESULT(0);
    }
    match msg {
        WM_MEDIA => {
            if let Some(i) = a.island.as_mut() {
                i.on_media();
                // Music starts: the cat leaves (unless it's there always).
                if i.media.playing && i.mascot != Mascot::Off && config::cur().i("mascot") != 2 {
                    i.say_bye();
                }
            }
        }
        WM_WEATHER => a.on_weather(),
        WM_VOLUME => {
            let (prev, prev_m) = data().volume;
            data().volume = (wp.0 as f32 / 1000.0, lp.0 != 0);
            // Several speakers together: muting the main one (mute key, Windows'
            // button) mutes them all, and the island's sliders follow.
            if c.b("mo_on") {
                let m = lp.0 != 0;
                if m != prev_m {
                    for (id, _) in multiout::cluster().iter().skip(1) {
                        multiout::set_dev_mute(id, m);
                    }
                }
                if let Some(i) = a.island.as_mut() {
                    if !i.mo_cluster.is_empty() {
                        i.mo_levels();
                    }
                }
            }
            // Several speakers together: a change to the main one (Windows' slider,
            // the island's) moves the others by the same amount.
            let v = wp.0 as f32 / 1000.0;
            if c.b("mo_on") && !multiout::own_main_change(v) && (v - prev).abs() > 0.002 && lp.0 == 0 {
                let bars = multiout::cluster_shift(v - prev, false);
                if let Some(i) = a.island.as_mut() {
                    i.mo_levels();
                }
                if bars.len() > 1 {
                    if c.b("osd_volume") && !a.island.as_ref().map(|i| i.expanded).unwrap_or(false) {
                        let mut p = Peek::bar("vol", "\u{E995}", v, String::new());
                        p.bars = bars;
                        peek(p);
                    }
                    return LRESULT(0);
                }
            } else if c.b("mo_on") && multiout::own_main_change(v) {
                // Our own step: its alert is already up.
                return LRESULT(0);
            }
            if c.b("osd_volume") && !a.island.as_ref().map(|i| i.expanded).unwrap_or(false) {
                let v = wp.0 as f32 / 1000.0;
                let muted = lp.0 != 0;
                let glyph = if muted || v <= 0.001 {
                    "\u{E74F}"
                } else if v < 0.34 {
                    "\u{E993}"
                } else if v < 0.67 {
                    "\u{E994}"
                } else {
                    "\u{E995}"
                };
                let label = if muted { i18n::tr("Muted").to_string() } else { format!("{}%", (v * 100.0).round() as i32) };
                peek(Peek::bar("vol", glyph, if muted { 0.0 } else { v }, label));
            }
        }
        WM_BRIGHT => {
            if c.b("osd_brightness") {
                let v = wp.0 as f32 / 100.0;
                peek(Peek::bar("bright", "\u{E706}", v, format!("{}%", wp.0)));
            }
        }
        WM_KEYEVT => match wp.0 {
            sysmon::KEY_CAPS => {
                SetTimer(Some(hwnd), T_CAPS, 60, None);
            }
            sysmon::KEY_ANY => a.wake_mascot(),
            sysmon::KEY_VOL_UP | sysmon::KEY_VOL_DOWN => {
                // Several speakers together: all move by the step in Settings.
                let bars = multiout::cluster_step(if wp.0 == sysmon::KEY_VOL_UP { 1 } else { -1 });
                if let Some((_, v)) = bars.first() {
                    data().volume.0 = *v;
                }
                if let Some(i) = a.island.as_mut() {
                    i.mo_levels();
                }
                if bars.len() > 1 {
                    let mut p = Peek::bar("vol", "\u{E995}", bars[0].1, String::new());
                    p.bars = bars;
                    peek(p);
                }
            }
            sysmon::KEY_TAP => {
                sysmon::TAP_SENT.store(false, std::sync::atomic::Ordering::Relaxed);
                if let Some(i) = a.island.as_mut() {
                    i.cat_tap();
                }
            }
            _ => {
                SetTimer(Some(hwnd), T_LANG, 250, None);
            }
        },
        WM_NET => {
            let online = sysmon::level_online(wp.0);
            let first = lp.0 != 0;
            data().online = Some(online);
            bt::spawn_scan(a.host, WM_BT);
            a.island_refresh();
            if a.online != Some(online) {
                let was = a.online;
                a.online = Some(online);
                if !first && was.is_some() && c.b("osd_network") {
                    if online {
                        peek(Peek::rich("net", "\u{E701}", 0x34c759, None, "Connected", "Back online", "", 2600, None));
                    } else {
                        peek(Peek::rich("net", "\u{E701}", 0xff453a, None, "No internet", "Check your Wi-Fi or cable", "", 3200, None));
                    }
                }
                if online && !first {
                    a.refresh_weather(false);
                }
            }
        }
        WM_AUDIODEV => {
            // Bluetooth endpoints take a moment to settle after connecting.
            SetTimer(Some(hwnd), T_AUDIO, 900, None);
        }
        WM_BT => a.on_bt(),
        WM_CTRL => a.on_ctrl(wp.0 != 0),
        WM_SEARCH => {
            if let Some(i) = a.island.as_mut() {
                i.on_search();
            }
        }
        WM_TRANSLATE => {
            if let Some(i) = a.island.as_mut() {
                i.on_translate();
            }
        }
        WM_LINK => {
            for j in link::take_jobs() {
                match j {
                    link::UiJob::Clip(c) => link::clip::set(hwnd, &c),
                    link::UiJob::Identify => link::show_identify(),
                    link::UiJob::DispSet(key, op, a, b, c) => {
                        let r = match op {
                            1 => display::set_scale(&key, a as u32),
                            2 => display::set_mode(&key, (a as u32, b as u32), c as u32),
                            3 => {
                                display::keep();
                                Ok(())
                            }
                            4 => display::revert(),
                            _ => Ok(()),
                        };
                        if let Err(e) = r {
                            util::log(&format!("link: the other PC's display change didn't work: {e}"));
                        }
                    }
                    link::UiJob::MainScreen(key) => {
                        if let Err(e) = link::set_main_local(&key) {
                            util::log(&format!("link: couldn't change the main screen: {e}"));
                        }
                    }
                    link::UiJob::Demo => {
                        if let Some(c) = link::clip::demo() {
                            link::clip::set_local(hwnd, &c);
                        }
                    }
                    link::UiJob::Shelf(paths, label) => {
                        data().shelf.add(paths);
                        peek(Peek::rich("link", "\u{E7B8}", 0x5e5ce6, None, &label, i18n::tr("Saved on the shelf · Downloads\\Phemio"), "", 3500, Some(Page::Shelf)));
                    }
                    link::UiJob::Peek(title, sub, ring) => peek(Peek::rich("link", "\u{E8C8}", 0x5e5ce6, ring, &title, &sub, "", 2500, Some(Page::Link))),
                }
            }
            if let Some(i) = a.island.as_mut() {
                i.on_link();
            }
        }
        0x031D /* WM_CLIPBOARDUPDATE */ => {
            if config::cur().b("link_on") {
                link::clip::offer(true);
            }
        }
        WM_HOTKEY if wp.0 as i32 == HOTKEY_TRANSLATE => {
            if let Some(i) = a.island.as_mut() {
                i.start_translate();
            }
        }
        WM_HOTKEY if wp.0 as i32 == HOTKEY_PLAYER => {
            if let Some(i) = a.island.as_mut() {
                i.toggle_now_playing();
            }
        }
        WM_HOTKEY if wp.0 as i32 == HOTKEY_DICT => {
            if let Some(i) = a.island.as_mut() {
                i.open_dictionary();
                if let Ok(q) = std::env::var("ISLET_DEMO_DICT") {
                    if !q.is_empty() {
                        i.demo_query(&q);
                    }
                }
            }
        }
        WM_HOTKEY if wp.0 as i32 == HOTKEY_PREV => a.media_cmd(media::Cmd::Prev),
        WM_HOTKEY if wp.0 as i32 == HOTKEY_NEXT => a.media_cmd(media::Cmd::Next),
        WM_HOTKEY if wp.0 as i32 == HOTKEY_SEARCH => {
            if let Some(i) = a.island.as_mut() {
                i.toggle_search();
                if std::env::var("ISLET_DEMO_PASTE").is_ok() {
                    i.demo_paste();
                } else if let Ok(q) = std::env::var("ISLET_DEMO_SEARCH") {
                    i.demo_query(&q);
                    if let Ok(k) = std::env::var("ISLET_DEMO_KEYS") {
                        i.demo_keys(&k);
                    }
                }
            }
        }
        WM_LISTS => {
            let note = connect::take_note();
            if let Some(i) = a.island.as_mut() {
                if let Some((m, ok)) = note {
                    if i.expanded {
                        i.ctl_note = Some((m, ok, std::time::Instant::now()));
                    } else {
                        let tr = i18n::tr;
                        peek(Peek::rich("conn", if ok { "\u{E73E}" } else { "\u{E783}" }, if ok { palette::GREEN } else { palette::RED }, None, tr(if ok { "Connected" } else { "Couldn't connect" }), &m, "", 2500, Some(island::Page::Controls)));
                    }
                }
                if i.expanded {
                    i.render();
                }
            }
        }
        WM_BTCHANGE => {
            // A Bluetooth device connected / disconnected: rescan shortly
            // (events arrive in bursts).
            SetTimer(Some(hwnd), T_BTSCAN, 700, None);
        }
        WM_TBMON => {
            if let Some(m) = a.tbmon.as_mut() {
                m.on_snapshot();
            }
        }
        WM_APPLY => a.apply(),
        WM_SHOW_SETTINGS => open_settings(0),
        WM_TRAY => match (lp.0 as u32) & 0xffff {
            WM_LBUTTONUP => open_settings(0),
            WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(),
            _ => {}
        },
        WM_TIMER => match wp.0 {
            T_MINUTE => a.on_minute(),
            T_FOCUS => {
                let _ = KillTimer(Some(hwnd), T_FOCUS);
                a.on_focus_timer();
            }
            T_IDLE => a.on_idle_timer(),
            glass::T_GLASS => glass::tick(hwnd),
            T_DOCKCHECK => {
                if let Some(i) = a.island.as_mut() {
                    i.check_dock();
                }
            }
            T_BTSCAN => {
                let _ = KillTimer(Some(hwnd), T_BTSCAN);
                bt::spawn_scan(hwnd, WM_BT);
            }
            T_RAISE => {
                let _ = KillTimer(Some(hwnd), T_RAISE);
                if let Some(i) = a.island.as_mut() {
                    i.ensure_on_top();
                }
                // Flyouts animate; check again once they've settled.
                SetTimer(Some(hwnd), T_RAISE2, 250, None);
            }
            T_RAISE2 => {
                let _ = KillTimer(Some(hwnd), T_RAISE2);
                if let Some(i) = a.island.as_mut() {
                    i.ensure_on_top();
                }
            }
            T_ZGUARD => {
                // Safety net: if the island should be showing but is hidden
                // or something is stacked over it, put it back on top.
                if let Some(i) = a.island.as_mut() {
                    i.ensure_on_top();
                }
            }
            T_FULLSCREEN => {
                let _ = KillTimer(Some(hwnd), T_FULLSCREEN);
                a.check_fullscreen();
            }
            T_CAPS => {
                let _ = KillTimer(Some(hwnd), T_CAPS);
                let on = sysmon::caps_on();
                let s = if on { "TYPING IN CAPITALS" } else { "typing in lowercase" };
                peek(Peek::rich("keys", "⇪", if on { 0xff9f0a } else { 0x8e8e93 }, None, "Caps Lock", s, if on { "ON" } else { "OFF" }, 1600, None));
            }
            T_LANG => {
                let _ = KillTimer(Some(hwnd), T_LANG);
                let lang = sysmon::current_lang();
                if lang != a.last_lang {
                    a.last_lang = lang;
                    let (name, short) = sysmon::lang_name(lang);
                    let p = Peek::rich("keys", "\u{E765}", 0x3b82f6, None, &name, "Keyboard language", &short, 1800, None);
                    peek(p);
                }
            }
            T_AUDIO => {
                let _ = KillTimer(Some(hwnd), T_AUDIO);
                a.refresh_audio(true);
                // Outputs changed (main one switched, a speaker came or went): copy afresh.
                if config::cur().b("mo_on") {
                    multiout::apply();
                }
                if let Some(i) = a.island.as_mut() {
                    i.mo_refresh();
                }
            }
            T_TRIM => {
                let _ = KillTimer(Some(hwnd), T_TRIM);
                util::trim_memory();
            }
            _ => {}
        },
        WM_POWERBROADCAST => {
            if wp.0 as u32 == PBT_POWERSETTINGCHANGE && lp.0 != 0 {
                let ps = &*(lp.0 as *const POWERBROADCAST_SETTING);
                if (ps.PowerSetting == GUID_ENERGY_SAVER_STATUS || ps.PowerSetting == windows::Win32::System::SystemServices::GUID_POWER_SAVING_STATUS) && ps.DataLength >= 4 {
                    let v = std::ptr::read_unaligned(ps.Data.as_ptr() as *const u32);
                    let d = data();
                    if ps.PowerSetting == GUID_ENERGY_SAVER_STATUS {
                        d.es_new = Some(v != 0);
                    } else {
                        d.es_old = v != 0;
                    }
                    // The newer status wins: the classic one reads "off" while
                    // energy saver runs on AC power or above 20 %.
                    let on = d.es_new.unwrap_or(d.es_old);
                    energy_saver_changed(on);
                }
                if ps.PowerSetting == windows::Win32::System::SystemServices::GUID_BATTERY_PERCENTAGE_REMAINING
                    || ps.PowerSetting == windows::Win32::System::SystemServices::GUID_ACDC_POWER_SOURCE
                {
                    a.battery_changed();
                }
                return LRESULT(1);
            }
            if wp.0 as u32 == PBT_APMPOWERSTATUSCHANGE {
                a.battery_changed();
                let mut ps = SYSTEM_POWER_STATUS::default();
                if GetSystemPowerStatus(&mut ps).is_ok() && ps.BatteryFlag != 128 && ps.BatteryLifePercent <= 100 && ps.ACLineStatus != a.ac_online {
                    let first = a.ac_online == 255;
                    a.ac_online = ps.ACLineStatus;
                    if !first && c.b("osd_charge") {
                        let pct = ps.BatteryLifePercent.min(100);
                        let frac = Some(pct as f32 / 100.0);
                        let p = if ps.ACLineStatus == 1 {
                            Peek::rich("power", "\u{E945}", 0x34c759, frac, "Charging", "Charger connected", &format!("{pct}%"), 2800, None)
                        } else {
                            let col = if pct <= 20 { 0xff453a } else { 0xffffff };
                            Peek::rich("power", "\u{E850}", col, frac, "On battery", "Charger disconnected", &format!("{pct}%"), 2800, None)
                        };
                        peek(p);
                    }
                }
                // Low battery warning
                if GetSystemPowerStatus(&mut ps).is_ok() && ps.ACLineStatus == 0 && ps.BatteryLifePercent == 10 && c.b("osd_charge") {
                    peek(Peek::rich("power", "\u{E850}", 0xff453a, Some(0.1), "Low battery", "Plug in your charger", "10%", 5000, None));
                }
            }
            return LRESULT(1);
        }
        WM_SETTINGCHANGE => {
            let what = if lp.0 != 0 { PCWSTR(lp.0 as *const u16).to_string().unwrap_or_default() } else { String::new() };
            if what == "ImmersiveColorSet" {
                a.relayout_all();
                if let Some(s) = a.settings.as_mut() {
                    s.refresh_theme();
                }
            } else if wp.0 as u32 == SPI_SETWORKAREA.0 {
                a.relayout_all();
            }
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED => a.relayout_all(),
        WM_QUERYENDSESSION => return LRESULT(1),
        WM_ENDSESSION => {
            if wp.0 != 0 {
                a.shutdown();
            }
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
        }
        WM_MULTIOUT => {
            if let Some(i) = a.island.as_mut() {
                if i.expanded && i.page == Page::Devices {
                    i.render();
                }
            }
        }
        WM_DESTROY => {
            multiout::stop();
            a.shutdown();
            PostQuitMessage(0);
        }
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    LRESULT(0)
}

fn main() {
    unsafe {
        let _ = OleInitialize(None);
        let _ = CoInitializeSecurity(None, -1, None, None, RPC_C_AUTHN_LEVEL_DEFAULT, RPC_C_IMP_LEVEL_IMPERSONATE, None, EOAC_NONE, None);
        // Settings → Link → Allow, run as administrator: add the firewall rule, then go.
        if std::env::args().any(|a| a == "--firewall-allow") {
            link::firewall::allow_elevated();
            return;
        }

        util::migrate_from_islet();
        util::cleanup_notifications_feature();
        // Single instance: a second launch just opens settings in the first.
        let started_by_windows = std::env::args().any(|a| a == "--autostart");
        if started_by_windows {
            // Signing in: start ahead of the rest (back to normal once it's up).
            use windows::Win32::System::Threading::{GetCurrentProcess, SetPriorityClass, HIGH_PRIORITY_CLASS};
            let _ = SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);
        }
        let _mutex = CreateMutexW(None, true, w!("Local\\PhemioSingleInstance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // Already running (started twice at sign-in): nothing to open.
            if started_by_windows {
                return;
            }
            if let Ok(h) = FindWindowW(HOST_CLASS, PCWSTR::null()) {
                let _ = PostMessageW(Some(h), WM_SHOW_SETTINGS, WPARAM(0), LPARAM(0));
            }
            return;
        }

        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // A file reader giving up on a broken file (the content index
            // goes on with the next): a note in the log, not a crash.
            if contentindex::in_reader() {
                util::log(&format!("content index: a file couldn't be read ({info})"));
                return;
            }
            // Keep the last few problems (Link threads carry on after one).
            let p = config::dir().join("crash.log");
            let old = std::fs::read_to_string(&p).unwrap_or_default();
            let keep: String = old.chars().rev().take(12_000).collect::<Vec<_>>().into_iter().rev().collect();
            let thread = std::thread::current().name().unwrap_or("?").to_string();
            let _ = std::fs::write(&p, format!("{keep}[{} {}] thread {thread}: {info}\n", env!("CARGO_PKG_VERSION"), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)));
            // Never leave the pointer hidden.
            link::input::show_pointer_now();
            prev(info);
        }));

        let icon = util::app_icon(false);
        util::register_class(HOST_CLASS, Some(host_proc), icon);
        island::register();
        tbmon::register();
        settings::register(icon);
        contentindex::start();

        let host = CreateWindowExW(WS_EX_TOOLWINDOW, HOST_CLASS, w!("Phemio"), WS_POPUP, 0, 0, 0, 0, None, None, Some(util::hinstance()), None).expect("host window");
        // Link sends what you copy to the other PC.
        let _ = windows::Win32::System::DataExchange::AddClipboardFormatListener(host);

        DATA = Box::into_raw(Box::new(Data {
            focus: tools::Focus::new(),
            todo: tools::Todo::load(),
            notes: tools::Notes::load(),
            shelf: shelf::Shelf::load(),
            weather: None,
            weather_err: None,
            weather_busy: false,
            tz2: None,
            tz2_err: None,
            tz2_busy: false,
            battery: None,
            energy_saver: false,
            es_new: None,
            es_old: false,
            output: String::new(),
            bt: Vec::new(),
            volume: (0.0, false),
            net: netinfo::NetInfo::default(),
            online: None,
        }));
        APP = Box::into_raw(Box::new(App {
            host,
            island: None,
            settings: None,
            tbmon: None,
            media: None,
            volume: None,
            devwatch: None,
            keyhook: None,
            hotkey_on: false,
            hotkey_tr: false,
            hotkey_music: false,
            hotkey_dict: false,
            netwatch: None,
            brightness: false,

            shell_msg: RegisterWindowMessageW(w!("SHELLHOOK")),
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            ac_online: 255,
            fullscreen: false,
            apply_pending: false,
            last_lang: 0,
            online: None,
            autostart: None,
            announce: true,
            bt_scans: 0,
            fs_votes: 0,
            bt_prev: None,
            ctrl_prev: None,
        }));

        let _ = RegisterShellHookWindow(host);
        let _ = windows::Win32::UI::Accessibility::SetWinEventHook(
            EVENT_OBJECT_SHOW,
            EVENT_OBJECT_REORDER,
            None,
            Some(on_show_hide),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        SetTimer(Some(host), T_ZGUARD, 150, None);
        // Foreground changes the shell hook misses (flyouts closing, focus
        // going back to the taskbar) still re-check full screen and z-order.
        let _ = windows::Win32::UI::Accessibility::SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(on_foreground),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        read_power();
        let _ = STARTED.set(std::time::Instant::now());
        // Energy saver, plus battery percentage (sent on every 1% change) and
        // power source — the plain power-status message only comes on plug/unplug.
        for g in [
            GUID_ENERGY_SAVER_STATUS,
            windows::Win32::System::SystemServices::GUID_POWER_SAVING_STATUS,
            windows::Win32::System::SystemServices::GUID_BATTERY_PERCENTAGE_REMAINING,
            windows::Win32::System::SystemServices::GUID_ACDC_POWER_SOURCE,
        ] {
            let _ = RegisterPowerSettingNotification(HANDLE(host.0), &g, DEVICE_NOTIFY_WINDOW_HANDLE);
        }
        let mut ps = SYSTEM_POWER_STATUS::default();
        if GetSystemPowerStatus(&mut ps).is_ok() {
            app().ac_online = ps.ACLineStatus;
        }
        app().add_tray();
        bt::spawn_scan(host, WM_BT);
        bt::start_watcher(host, WM_BTCHANGE);
        connect::start(host, WM_LISTS);
        controls::start(host, WM_CTRL);
        // Developer aid: ISLET_DEMO_CAPS shows the Caps Lock peek after start-up.
        // Developer aid: open the search with a query at start (screenshots).
        if let Ok(v) = std::env::var("ISLET_DEMO_TRANSLATE") {
            let h = host.0 as isize;
            // (A number: when, in ms.)
            let ms = v.parse::<u64>().unwrap_or(2500);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(ms));
                let _ = PostMessageW(Some(HWND(h as *mut _)), WM_HOTKEY, WPARAM(HOTKEY_TRANSLATE as usize), LPARAM(0));
            });
        }
        // Developer aid: ISLET_DEMO_ESC=<ms> presses Esc on the island then.
        if let Some(ms) = std::env::var("ISLET_DEMO_ESC").ok().and_then(|v| v.parse::<u64>().ok()) {
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(ms));
                let h = crate::island::island_hwnd();
                if h != 0 {
                    let _ = PostMessageW(Some(HWND(h as *mut _)), WM_KEYDOWN, WPARAM(0x1B), LPARAM(0));
                }
            });
        }
        // Developer aid: ISLET_DEMO_PLAYER presses Alt+A after start-up.
        if std::env::var("ISLET_DEMO_PLAYER").is_ok() {
            let h = host.0 as isize;
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(2500));
                let _ = PostMessageW(Some(HWND(h as *mut _)), WM_HOTKEY, WPARAM(HOTKEY_PLAYER as usize), LPARAM(0));
            });
        }
        if std::env::var("ISLET_DEMO_DICT").is_ok() {
            let h = host.0 as isize;
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(2500));
                let _ = PostMessageW(Some(HWND(h as *mut _)), WM_HOTKEY, WPARAM(HOTKEY_DICT as usize), LPARAM(0));
            });
        }
        if std::env::var("ISLET_DEMO_SEARCH").is_ok() {
            let h = host.0 as isize;
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(2500));
                let _ = PostMessageW(Some(HWND(h as *mut _)), WM_HOTKEY, WPARAM(HOTKEY_SEARCH as usize), LPARAM(0));
            });
        }
        // Developer aid: ISLET_TEST_DISPLAY logs what the display code sees
        // and re-applies the current arrangement (checks the calls work).
        if std::env::var("ISLET_TEST_DISPLAY").is_ok() {
            for s in link::local_screens() {
                util::log(&format!("display test: {} {:?}", s.key, display::info(&s.key)));
            }
            let list: Vec<(String, i32, i32)> = link::local_screens().iter().map(|s| (s.key.clone(), s.x, s.y)).collect();
            util::log(&format!("display test: arrange {:?}", display::arrange(&list)));
        }
        // Developer aid: open Settings on a tab at start (screenshots).
        if let Some(t) = std::env::var("ISLET_OPEN_SETTINGS").ok().and_then(|v| v.parse::<usize>().ok()) {
            open_settings(t);
        }
        if std::env::var("ISLET_DEMO_CAPS").is_ok() {
            SetTimer(Some(host), T_CAPS, 4000, None);
        }
        app().apply();
        multiout::init(host, WM_MULTIOUT);
        multiout::apply();
        if let Some(i) = app().island.as_mut() {
            i.mo_refresh();
        }
        // Developer aid: ISLET_OPEN_PAGE=<page index> opens the island on that page.
        if let Some(p) = std::env::var("ISLET_OPEN_PAGE").ok().and_then(|v| v.parse::<usize>().ok()).and_then(|i| island::ALL_PAGES.get(i).copied()) {
            if let Some(i) = app().island.as_mut() {
                i.open_page(p);
                if let Ok(k) = std::env::var("ISLET_DEMO_KEYS") {
                    i.demo_keys(&k);
                }
            }
        }
        if std::env::args().any(|a| a == "--autostart") {
            // Up and showing: an ordinary app again after the sign-in rush.
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(20));
                use windows::Win32::System::Threading::{GetCurrentProcess, SetPriorityClass, NORMAL_PRIORITY_CLASS};
                let _ = SetPriorityClass(GetCurrentProcess(), NORMAL_PRIORITY_CLASS);
            });
        }
        app().refresh_weather(false);

        if !config::dir().join("config.ini").exists() {
            config::update(|_| {});
            open_settings(0);
        }

        SetTimer(Some(host), T_MINUTE, util::ms_to_next_minute(), None);
        SetTimer(Some(host), T_TRIM, 4000, None);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = DeregisterShellHookWindow(host);
    }
    // Everything's put away: end here (no helper thread keeps it running).
    std::process::exit(0);
}
