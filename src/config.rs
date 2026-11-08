//! Settings, stored as a plain `key=value` file in %APPDATA%\Phemio\config.ini.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

/// Every numeric/boolean setting with its default.
pub const DEFAULTS: &[(&str, i32)] = &[
    // general
    ("theme", 0),           // 0 auto, 1 dark, 2 light
    ("hide_fullscreen", 0), // hide while a full-screen app is in front
    ("autostart", 0),
    // island look & position
    ("island", 1),
    ("island_style", 1), // 0 island (floating pill), 1 notch (attached to the top)
    ("minimal", 0),      // closed notch is a thin line
    ("island_align", 1), // 0 left, 1 center, 2 right
    ("island_x", 0),     // horizontal offset in DIPs
    ("anchor", 1),       // 0 auto (below a top taskbar), 1 screen top, 2 below taskbar
    ("island_y", 0),     // gap from the top (island style)
    ("island_size", 90),
    ("dots_pos", 0),      // page dots: 0 top of the opened island, 1 bottom
    ("pdock_pos", 1),     // page dock: 0 above the dots, 1 below
    ("pdock_bg", 0),      // page dock: 0 frosted glass, 1 no background
    ("col_h", 100),       // closed island / notch height (%)
    ("exp_w", 600), // expanded island width (DIPs, before Size)
    ("exp_h", 260), // expanded island height
    ("mo_on", 0),
    ("mo_step", 3), // volume key step (%) for all the speakers playing together // play on several outputs at once (Devices → Sound output)
    ("page_fit", 1), // scale page contents to fill a bigger / smaller island
    ("tz_refresh", 30), // world clock weather: minutes between updates
    ("wx_text", 100), // weather page, left side: text size (%)
    ("island_hover", 1),  // expand on hover (otherwise click)
    ("island_always", 0), // always expanded
    ("music_hotkeys", 1), // Alt+A Now Playing, Alt+Z / Alt+X previous / next
    ("island_in_tb", 0),  // live inside the taskbar (like the taskbar stats)
    ("island_top", 1),    // always on top ("pin")
    ("clock_fmt", 1),     // 0 system, 1 12-hour, 2 24-hour
    ("mascot", 1), // 0 off, 1 when idle, 2 always (click it to send it away)
    ("mascot_sec", 10), // idle seconds before the mascot appears
    ("island_eq", 1),
    ("icon_colors", 0), // 0 colour theme, 1 album accent, 2 mono
    ("theme_colors", 3), // palette::THEME_NAMES
    ("clock_secs", 1), // seconds on the closed island
    ("cover_size", 75), // Now Playing cover, %
    ("peek_cover", 90), // song-change popup cover, %
    ("col_right", 1), // closed island right icon: 0 energy saver, 1 speaker
    ("col_center", 0), // closed island centre: 0 time, 1 date, 2 time (with seconds) above the date, 3 date above the time (with seconds)
    ("col_battery", 1),
    ("col_left", 0), // closed island left: 0 battery, 1 microphone (mute status)
    ("col_luck", 1), // lucky / unlucky day and hour marks beside the closed island's date
    ("col_luck_gl", 4), // space between the day mark and the date (DIPs)
    ("col_luck_gr", 4), // space between the date and the hour mark
    // closed island time / date text: size %, font (0 as the island's text), style (0 as designed)
    ("col_t_size", 100),
    ("col_t_font", 0),
    ("col_t_style", 0),
    ("col_d_size", 100),
    ("col_d_font", 0),
    ("col_d_style", 0),
    ("pix_drift", 1), // OLED care: the closed island drifts 1–3 px every few minutes
    ("batt_style", 1), // closed island battery: 0 ring, 1 battery icon + number // battery ring + energy saver in the closed island when no music plays
    ("font_face", 1),   // 0 Segoe UI Variable, 1 Segoe UI, 2 Bahnschrift, 3 Cascadia Code, 4 Georgia, 5 custom
    ("font_scale", 90),
    ("font_style", 0), // 0 normal, 1 bold, 2 italic, 3 bold italic
    // pages
    ("page_home", 1),
    ("home_date", 1),
    ("home_clock_size", 100),
    ("wc_size", 130),
    ("clock_font", 1), // 0 same as text, 1 Segoe UI, 2 Bahnschrift, 3 Cascadia, 4 Georgia, 5 other
    ("clock_weight", 2), // 0 thin, 1 light, 2 regular, 3 semibold, 4 bold
    ("lang", 0), // 0 English, 1 Tiếng Việt
    ("page_clock", 1),
    ("page_weather", 1),
    ("page_focus", 1),
    ("page_todo", 0),
    ("page_shelf", 0),
    ("page_devices", 1),
    ("page_controls", 1),
    ("page_calendar", 1),
    ("page_search", 1),
    ("page_link", 1),
    ("page_rdp", 1),
    ("rdp_remember", 1),
    ("rdp_full", 1),
    ("rdp_multi", 0),
    ("wx_today", 0),
    ("notes_view", 0),
    ("link_on", 1), // Link: one mouse and keyboard across PCs
    ("osd_btdev", 1), // any Bluetooth device connects / disconnects
    ("osd_radios", 1), // Wi-Fi / Bluetooth / airplane / microphone
    ("osd_saver", 1), // energy saver on / off
    ("search_hotkey", 1), // Alt+S opens search
    ("srch_content", 0), // search box: 0 file names (Everything), 1 inside files
    ("search_live", 0), // 1: results while typing, 0: press Enter to search
    ("translate_hotkey", 1), // Alt+Q translates the selected text
    ("dict_hotkey", 1), // Alt+W opens the dictionary (search box)
    ("translate_lang", 0), // into: Vietnamese, English, Japanese, Korean, Chinese, French
    ("currency_home", 0), // VND, USD, AUD, EUR, GBP, JPY, CNY
    ("gemini_model", 2), // 0 Gemini 3.8 Flash, 1 3.6 Flash, 2 3.5 Flash Lite
    ("translate_engine", 0), // 0 Google Translate, 1-3 the Gemini models above (3 = 3.5 Flash Lite)
    ("mascot_style", 0), // 0 robot, 1 cat, 2 cat vs robot
    ("duel_cat", 0), // cat vs robot: rounds won by each
    ("duel_bot", 0),
    ("gemini_search", 1),
    ("gemini_think", 1),
    ("cal_lunar", 0), // calendar page: 0 solar (lunar dates small), 1 lunar (solar dates small) // Gemini may look things up on Google Search first
    ("last_page", 3),
    ("free_page", 0),
    // Notifications moved to a corner: on, corner (0 top left, 1 top right,
    // 2 bottom left, 3 bottom right), padding, how many at once, seconds,
    // read from (0 Windows' notification store, 1 the notification listener),
    // and switch off Windows' own banners for moved apps.
    ("weather_unit", 0),
    ("weather_src", 0), // forecast source, see weather::PROVIDERS
    // focus
    ("focus_min", 25),
    ("break_min", 5),
    ("long_min", 15),
    ("long_every", 4),
    ("focus_sound", 1),
    // alerts
    ("osd_volume", 1),
    ("osd_brightness", 1),
    ("osd_keys", 1), // Caps Lock + keyboard language
    ("osd_charge", 1),
    ("osd_network", 1),
    ("osd_audio", 1), // headphones / output device changes
    ("osd_track", 1),
    ("remind_todo", 1),
    // taskbar monitor
    ("tbm", 1),
    ("tbm_icons", 1),    // icons instead of text labels
    ("tbm_layout", 0),   // 0 auto, 1 one line, 2 two lines
    // Transparency page: taskbar / File Explorer see-through, level (%),
    // look (0 clear, 1 frosted)
    ("glass_tb", 0),
    ("glass_tb_lvl", 60),
    ("glass_ex", 0),
    ("glass_ex_lvl", 50),
    ("glass_look", 1),
    ("glass_ex_mode", 0), // File Explorer: 0 background only, 1 whole window
    ("tbm_anchor", 0),   // 0 next to the tray, 1 far left (top), 2 far right (bottom)
    ("tbm_shift", 460),    // dragged offset (DIPs)
    ("tbm_size", 80),
    ("tbm_edge", 6), // far left / far right: space from the taskbar's end (DIPs)
    ("tbm_gap", 10), // space between items
    ("tbm_bg", 0), // 0 taskbar colour (solid), 1 transparent
    ("tbm_bg_h", 90), // background height, % of the taskbar
    ("tbm_bg_r", 6), // background corner radius, px
    ("tbm_interval", 1), // seconds
    ("tbm_net", 1),
    ("tbm_cpu", 1),
    ("tbm_ram", 1),
    ("tbm_spd", 1),
    ("tbm_gpu", 1),
    ("tbm_dio", 0),
    ("tbm_dsk", 0),
    ("tbm_npu", 1),
];

/// Text settings.
pub const STR_KEYS: &[&str] = &[
    "page_order",
    "gemini_key",
    "clock_font_custom",
    "weather_city",
    "weather_place",
    "tz2_city",
    "mo_targets",
    "tz2_place",
    "tz2_abbr",
    "font_custom",
    "tbm_order",
    "tbm_drives",
    // Content search: drives Phemio indexes (letters, e.g. "CD").
    "cs_drives",
];

/// Defaults for the text settings (empty when not listed).
pub const STR_DEFAULTS: &[(&str, &str)] = &[
    // No key in the source: set your own in Settings → General → Gemini API key
    // (or build with PHEMIO_GEMINI_KEY=<key> to give a private build a default one).
    ("gemini_key", match option_env!("PHEMIO_GEMINI_KEY") {
        Some(k) => k,
        None => "",
    }),
    ("page_order", "page_controls,page_weather,page_home,page_calendar,page_clock,page_focus,page_todo,page_shelf,page_devices,page_search,page_link,page_rdp"),
    ("rdp_recent", ""),
    ("shelf_dir", ""),
    ("weather_city", "Hanoi"),
    ("weather_place", "Hanoi, Vietnam"),
    ("tz2_city", "Melbourne"),
    ("tz2_place", "Melbourne, Australia"),
    ("font_custom", ""),
    ("tbm_order", "net,spd,cpu,ram,gpu,npu,dio,dsk"),
];

#[derive(Clone, Debug)]
pub struct Config {
    vals: BTreeMap<String, i32>,
    strs: BTreeMap<String, String>,
}

impl Config {
    pub fn i(&self, k: &str) -> i32 {
        if let Some(v) = self.vals.get(k) {
            return *v;
        }
        DEFAULTS.iter().find(|(n, _)| *n == k).map(|(_, v)| *v).unwrap_or(0)
    }
    pub fn b(&self, k: &str) -> bool {
        self.i(k) != 0
    }
    pub fn set(&mut self, k: &str, v: i32) {
        self.vals.insert(k.to_string(), v);
    }
    pub fn s(&self, k: &str) -> String {
        match self.strs.get(k) {
            Some(v) => v.clone(),
            None => STR_DEFAULTS.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string()).unwrap_or_default(),
        }
    }
    pub fn set_s(&mut self, k: &str, v: &str) {
        let clean: String = v.chars().filter(|c| *c != '\n' && *c != '\r').collect();
        self.strs.insert(k.to_string(), clean);
    }

    fn load() -> Config {
        let mut c = Config { vals: BTreeMap::new(), strs: BTreeMap::new() };
        let Ok(text) = std::fs::read_to_string(path()) else { return c };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let k = k.trim();
            if STR_KEYS.contains(&k) {
                c.strs.insert(k.to_string(), v.trim().to_string());
            } else if let Ok(n) = v.trim().parse::<i32>() {
                c.vals.insert(k.to_string(), n);
            }
        }
        c
    }

    fn save(&self) {
        let p = path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut s = String::from("# Phemio settings\n");
        for (k, d) in DEFAULTS {
            s.push_str(&format!("{k}={}\n", self.vals.get(*k).copied().unwrap_or(*d)));
        }
        for k in STR_KEYS {
            s.push_str(&format!("{k}={}\n", self.s(k)));
        }
        // Write beside it, then swap in: other threads never read half a file.
        let tmp = p.with_extension("ini.tmp");
        if std::fs::write(&tmp, &s).is_ok() && std::fs::rename(&tmp, &p).is_ok() {
            return;
        }
        let _ = std::fs::write(p, s);
    }
}

pub fn dir() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("Phemio")
}

pub fn data_file(name: &str) -> PathBuf {
    let d = dir();
    let _ = std::fs::create_dir_all(&d);
    d.join(name)
}

fn path() -> PathBuf {
    dir().join("config.ini")
}

/// Bumped on every change, so worker threads (each with its own copy)
/// pick up new settings instead of keeping what they read at start.
static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

thread_local! {
    static CUR: RefCell<Rc<Config>> = RefCell::new(Rc::new(Config::load()));
    static SEEN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Snapshot of the current configuration (cheap Rc clone).
pub fn cur() -> Rc<Config> {
    let g = GEN.load(std::sync::atomic::Ordering::Acquire);
    if SEEN.with(|s| s.get()) != g {
        SEEN.with(|s| s.set(g));
        CUR.with(|c| *c.borrow_mut() = Rc::new(Config::load()));
    }
    CUR.with(|c| c.borrow().clone())
}

/// Changed here but not yet written (a slider being dragged).
static DIRTY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Change the settings for this thread only, without writing the file —
/// for a slider being dragged, where every step would otherwise wait on the
/// disk. `flush` writes and publishes it when the drag ends.
pub fn update_live(f: impl FnOnce(&mut Config)) {
    let _ = cur();
    CUR.with(|c| {
        let mut n = (**c.borrow()).clone();
        f(&mut n);
        *c.borrow_mut() = Rc::new(n);
    });
    DIRTY.store(true, std::sync::atomic::Ordering::Release);
}

/// Write and publish what `update_live` changed.
pub fn flush() {
    if DIRTY.swap(false, std::sync::atomic::Ordering::AcqRel) {
        CUR.with(|c| c.borrow().save());
        let g = GEN.fetch_add(1, std::sync::atomic::Ordering::AcqRel) + 1;
        SEEN.with(|s| s.set(g));
    }
}

/// Mutate, persist and publish a new configuration.
pub fn update(f: impl FnOnce(&mut Config)) {
    DIRTY.store(false, std::sync::atomic::Ordering::Release);
    // Start from the newest settings (another thread may have changed them),
    // never write an old copy back over them.
    let _ = cur();
    CUR.with(|c| {
        let mut n = (**c.borrow()).clone();
        f(&mut n);
        n.save();
        *c.borrow_mut() = Rc::new(n);
    });
    let g = GEN.fetch_add(1, std::sync::atomic::Ordering::AcqRel) + 1;
    SEEN.with(|s| s.set(g));
}
