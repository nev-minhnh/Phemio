//! The settings window: choose which functions are on and how the island
//! looks. Custom-drawn with the same Direct2D layer; destroyed when closed so
//! it costs nothing while you are not looking at it.

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{self, Config};
use crate::gfx::{self, inside, rf, rgb, with_alpha, Surface, DISPLAY_FONT, ICON_FONT, UI_FONT};
use crate::util;
use crate::i18n::tr;

const CLASS: PCWSTR = w!("PhemioSettings");
const W: f32 = 800.0;
const H: f32 = 620.0;
const SIDE: f32 = 210.0;
const T_TICK: usize = 1;
const T_CARET: usize = 2;
/// While a slider is dragged: show its effect at most this often (ms).
const T_LIVE: usize = 3;
/// A long line scrolling sideways wants its next frame.
const T_MARQUEE: usize = 4;
/// After the wheel moved a slider: save and apply in full.
const T_WHEEL_APPLY: usize = 5;
/// Animation frames (the tab pill, the page fading in, scrolling, switches).
const T_ANIM: usize = 6;
/// Sidebar: where the first tab sits and the step between tabs.
const TAB_Y0: f32 = 134.0;
const TAB_STEP: f32 = 40.0;
const TAB_H: f32 = 34.0;
const LIVE_MS: u32 = 50;

#[derive(Clone)]
enum Ctl {
    Toggle(&'static str),
    Choice(&'static str, &'static [&'static str]),
    Slider(&'static str, i32, i32, i32, &'static str),
    Edit(&'static str, &'static str),
    Metric(usize),
    /// A page in Settings → Pages (index into the user's order).
    Page(usize),
    Button(&'static str, Act),
    Text(String),
    /// An on/off switch whose state isn't a setting (Do Not Disturb, an app
    /// in the notification list): what it shows, worked out when listed.
    Switch(Sw, bool),
}

#[derive(Clone, Copy, PartialEq)]
enum Sw {
    /// A drive for the content index (Settings → General → Content search).
    Drive(char),
}

#[derive(Clone, Copy, PartialEq)]
enum Act {
    OpenFolder,
    Url(&'static str),
    TestGemini,
    GlassRestore,
    GlassLog,
    PickShelfDir,
    CopyDiag,
    Exit,
    RefreshWeather,
    IndexNow,
    IndexStop,
    IndexClear,
    FirewallAllow,
}

struct Row {
    label: String,
    sub: String,
    ctl: Ctl,
    enabled: bool,
    header: bool,
}

/// Closed island time / date: fonts and styles.
const FONTS: &[&str] = &["Default", "Segoe UI Variable", "Segoe UI", "Bahnschrift", "Cascadia", "Georgia"];
const STYLES: &[&str] = &["Default", "Thin", "Regular", "Bold", "Italic", "Bold italic"];

fn row(label: &str, sub: &str, ctl: Ctl, enabled: bool) -> Row {
    Row { label: label.into(), sub: sub.into(), ctl, enabled, header: false }
}
fn header(label: &str) -> Row {
    Row { label: label.into(), sub: String::new(), ctl: Ctl::Text(String::new()), enabled: true, header: true }
}

const TABS: [(&str, &str); 7] = [("General", "\u{E713}"), ("Island", "\u{E7F4}"), ("Pages", "\u{E8A1}"), ("Alerts", "\u{EA8F}"), ("Taskbar", "\u{E9D9}"), ("Transparency", "\u{E790}"), ("About", "\u{E946}")];

fn rows(tab: usize, c: &Config) -> Vec<Row> {
    let mut v = Vec::new();
    let on = c.b("island");
    match tab {
        0 => {
            v.push(row("Dynamic Island", "Show the island at the top of the screen", Ctl::Toggle("island"), true));
            v.push(row("Start with Windows", "Launch Phemio when you sign in", Ctl::Toggle("autostart"), true));
            v.push(row("Language", "", Ctl::Choice("lang", &crate::i18n::LANGS), true));
            v.push(row("Theme", "", Ctl::Choice("theme", &["Auto", "Dark", "Light"]), true));
            v.push(row("Hide in full screen", "Step aside for games, videos and slides", Ctl::Toggle("hide_fullscreen"), true));
            v.push(row("Search with Alt+S", "Apps right away; files and folders through Everything (voidtools)", Ctl::Toggle("search_hotkey"), true));
            v.push(row("Search as you type", "Off: type, then press Enter to search", Ctl::Toggle("search_live"), true));
            v.push(row("Link with your other PCs", "One mouse and keyboard across PCs running Phemio (Island → Link page to pair and arrange screens)", Ctl::Toggle("link_on"), true));
            if c.b("link_on") {
                use crate::link::firewall::{busy, state, Fw};
                let (sub, need) = match state() {
                    Fw::Allowed => ("Windows Firewall lets other PCs reach Phemio", false),
                    Fw::Off => ("Windows Firewall is off on this network", false),
                    Fw::Missing | Fw::Blocked => ("Windows Firewall keeps other PCs out, so finding them is slow — Allow adds a rule (Windows asks first)", true),
                    Fw::Unknown => ("Lets other PCs find this one quickly (adds a Windows Firewall rule; Windows asks first)", true),
                };
                v.push(row("Let other PCs find this one", sub, Ctl::Button("Allow", Act::FirewallAllow), need && !busy()));
            }
            v.push(row("Music with Alt+A, Alt+Z, Alt+X", "Alt+A: Now Playing · Alt+Z: previous song · Alt+X: next song", Ctl::Toggle("music_hotkeys"), true));
            v.push(row("Translate with Alt+Q", "Select text anywhere, press Alt+Q: the translation shows on the island (Google Translate or Gemini). Esc or a click elsewhere closes it", Ctl::Toggle("translate_hotkey"), true));
            v.push(row("Dictionary with Alt+W", "Type a word: its translations, meanings and examples, from Google Translate. ←/→ edit the word, ↑/↓ pick a line, Enter copies it", Ctl::Toggle("dict_hotkey"), true));
            v.push(row("Translate into", "Text already in this language goes to English", Ctl::Choice("translate_lang", &crate::answers::LANG_NAMES), true));
            v.push(row("Translate with", "Google Translate, or a Gemini model (writes as it thinks; needs the Gemini key below)", Ctl::Choice("translate_engine", &crate::gemini::TRANSLATORS), true));
            v.push(row("Your currency", "\"100 usd\" in the search converts to this", Ctl::Choice("currency_home", &crate::answers::CURRENCIES), true));
            v.push(header("Gemini"));
            v.push(row("Gemini API key", "From Google AI Studio. \"? question\" in the search asks Gemini", Ctl::Edit("gemini_key", "Paste your key"), true));
            v.push(row("Check the key", "Asks Gemini a tiny question and shows what works", Ctl::Button("Test", Act::TestGemini), !c.s("gemini_key").trim().is_empty()));
            v.push(row("Gemini model", "For \"? question\" — you can also switch on the answer itself", Ctl::Choice("gemini_model", &crate::gemini::MODEL_SHORT), true));
            v.push(row("Gemini searches Google", "Answers from fresh Google results, with the pages it used (when your key allows it)", Ctl::Toggle("gemini_search"), true));
            v.push(row("Gemini thinks first", "Better answers to hard questions, a little slower (also the Think chip on an answer). Translations never think", Ctl::Toggle("gemini_think"), true));
            v.push(row("Get a free key", "aistudio.google.com — sign in with Google, then Create API key", Ctl::Button("Open", Act::Url(crate::gemini::KEY_URL)), true));
            v.push(row("Get Everything", "Free file search engine by voidtools — needed for files, not for apps", Ctl::Button("Download", Act::Url(crate::search::DOWNLOAD_URL)), true));
            v.push(header("Content search"));
            v.push(row("Index these drives", "Content search looks inside Word, Excel, PowerPoint, PDF and text files on them", Ctl::Text(String::new()), true));
            let chosen = crate::contentindex::drives();
            for (d, name, total, free) in crate::contentindex::fixed_drives() {
                let label = if name.is_empty() { format!("{d}:") } else { format!("{d}:  {name}") };
                let sub = format!("{free:.0} GB {} {total:.0} GB", tr("free of"));
                v.push(row(&label, &sub, Ctl::Switch(Sw::Drive(d), chosen.contains(&d)), true));
            }
            let st = crate::contentindex::status();
            let line = crate::contentindex::status_line();
            if st.running {
                v.push(row("Index", &line, Ctl::Button("Stop", Act::IndexStop), true));
            } else {
                v.push(row("Index", &line, Ctl::Button("Update now", Act::IndexNow), !chosen.is_empty()));
            }
            v.push(row("Clear the index", "Forgets what Phemio read (read again on the next update)", Ctl::Button("Delete", Act::IndexClear), st.files > 0 && !st.running));
            v.push(header("Clock"));
            v.push(row("Clock format", "Closed island and all clocks", Ctl::Choice("clock_fmt", &["System", "12-hour", "24-hour"]), true));
            v.push(header("Places"));
            v.push(row("Your location", "For weather (city name, or lat,lon)", Ctl::Edit("weather_city", "e.g. Hanoi"), true));
            v.push(row("Second time zone", "A city whose time you want to see", Ctl::Edit("tz2_city", "e.g. Melbourne"), true));
            v.push(row("Temperature", "", Ctl::Choice("weather_unit", &["°C", "°F"]), true));
            v.push(row("Forecast source", "Free services, no account needed", Ctl::Choice("weather_src", &crate::weather::PROVIDERS), true));
        }
        1 => {
            v.push(row("Style", "Floating island, or a notch attached to the top", Ctl::Choice("island_style", &["Island", "Notch"]), on));
            v.push(row("Minimal notch", "The closed island becomes a thin line", Ctl::Toggle("minimal"), on));
            v.push(row("Position", "Where it sits along the top", Ctl::Choice("island_align", &["Left", "Center", "Right"]), on));
            v.push(row("Horizontal offset", "Fine-tune the position", Ctl::Slider("island_x", -800, 800, 1, "px"), on));
            v.push(row("Attach to the taskbar", "Inside a taskbar at the top or bottom, like the taskbar stats: never covered, moves and auto-hides with it. Off: placed as below", Ctl::Toggle("island_in_tb"), on));
            let att = c.b("island_in_tb");
            v.push(row("Vertical anchor", "Auto hangs below a taskbar on the top edge", Ctl::Choice("anchor", &["Auto", "Screen top", "Below taskbar"]), on && !att));
            v.push(row("Top margin", "Gap from the top (island style)", Ctl::Slider("island_y", 0, 60, 1, "px"), on && c.i("island_style") == 0 && !att));
            v.push(row("Size", "", Ctl::Slider("island_size", 70, 150, 1, "%"), on));
            v.push(row("Page dots", "The line of dots that switches pages (point at it for the page dock): at the top of the opened island, or at the bottom", Ctl::Choice("dots_pos", &["Top", "Bottom"]), on));
            v.push(row("Page dock position", "Above the page dots or below them. At the island's edge (dots at the top with the dock above, or at the bottom with the dock below) the island grows to make room while it shows", Ctl::Choice("pdock_pos", &["Top", "Bottom"]), on));
            v.push(row("Page dock", "Point at the page dots: every page as an icon. On frosted glass, or the icons on their own", Ctl::Choice("pdock_bg", &["Frosted glass", "No background"]), on));
            v.push(row("Closed height", "The closed island or notch, also in the taskbar (a notch there: share of the taskbar's height)", Ctl::Slider("col_h", 60, 140, 1, "%"), on && !c.b("minimal")));
            v.push(row("Expanded width", "How wide the island opens", Ctl::Slider("exp_w", 480, 900, 10, ""), on));
            v.push(row("Expanded height", "How tall the island opens", Ctl::Slider("exp_h", 200, 400, 5, ""), on));
            v.push(row("Fit pages to the island", "On a bigger or smaller island, everything on the pages scales to fill it", Ctl::Toggle("page_fit"), on));
            v.push(header("Behaviour"));
            v.push(row("Expand on hover", "Otherwise click to expand", Ctl::Toggle("island_hover"), on));
            v.push(row("Always expanded", "Keep the full view open", Ctl::Toggle("island_always"), on));
            v.push(row("Always on top", "Stay above other windows (and a top taskbar)", Ctl::Toggle("island_top"), on));
            v.push(row("Equalizer animation", "While music plays: moving bars, or (off) the speaker and its volume", Ctl::Toggle("island_eq"), on));
            v.push(row("Colour theme", "Icons, indicators, battery and charts", Ctl::Choice("theme_colors", &crate::palette::THEME_NAMES), on));
            v.push(row("Icon colours", "The colour theme, the album's colour, or plain", Ctl::Choice("icon_colors", &["Palette", "Album", "Mono"]), on));
            v.push(row("Show seconds", "On the closed island's clock", Ctl::Toggle("clock_secs"), on && c.i("col_center") == 0));
            v.push(row("Closed island shows", "The middle of the closed island: the time, the date, or both (with seconds), either way up", Ctl::Choice("col_center", &["Time", "Date", "Time / date", "Date / time"]), on));
            v.push(row("Lucky / unlucky marks", "Beside the date (lunar calendar): left, a star for a lucky day or a warning sign for an unlucky one · right, a thumbs up / down for a lucky / unlucky hour", Ctl::Toggle("col_luck"), on && c.i("col_center") >= 1));
            let lk = on && c.i("col_center") >= 1 && c.b("col_luck");
            v.push(row("Day mark spacing", "Between the lucky / unlucky day mark and the date", Ctl::Slider("col_luck_gl", 0, 30, 1, "px"), lk));
            v.push(row("Hour mark spacing", "Between the date and the lucky / unlucky hour mark", Ctl::Slider("col_luck_gr", 0, 30, 1, "px"), lk));
            let tm = c.i("col_center") != 1;
            let dt = c.i("col_center") >= 1;
            v.push(row("Time text size", "The closed island's clock", Ctl::Slider("col_t_size", 60, 160, 5, "%"), on && tm));
            v.push(row("Time font", "", Ctl::Choice("col_t_font", FONTS), on && tm));
            v.push(row("Time style", "", Ctl::Choice("col_t_style", STYLES), on && tm));
            v.push(row("Date text size", "The closed island's date", Ctl::Slider("col_d_size", 60, 160, 5, "%"), on && dt));
            v.push(row("Date font", "", Ctl::Choice("col_d_font", FONTS), on && dt));
            v.push(row("Date style", "", Ctl::Choice("col_d_style", STYLES), on && dt));
            v.push(row("Right-side icon", "When no music plays: energy saver, or the speaker (click it to mute / unmute)", Ctl::Choice("col_right", &["Energy saver", "Speaker"]), on));
            v.push(row("Left-side icon", "Closed island: the battery (with energy saver on the right), or the microphone (click it to mute / unmute)", Ctl::Toggle("col_battery"), on));
            v.push(row("Left icon shows", "", Ctl::Choice("col_left", &["Battery", "Microphone"]), on && c.b("col_battery")));
            v.push(row("Battery look", "Closed island, left: a ring, or a battery with the percentage", Ctl::Choice("batt_style", &["Ring", "Battery + %"]), on && c.b("col_battery") && c.i("col_left") == 0));
            v.push(row("Pixel drift", "OLED care: the closed island moves 1–3 pixels every few minutes, so no pixel stays lit in one spot all day", Ctl::Toggle("pix_drift"), on));
            v.push(header("Text"));
            v.push(row("Font", "", Ctl::Choice("font_face", &["Default", "Segoe UI", "Bahnschrift", "Cascadia", "Georgia", "Other"]), on));
            v.push(row("Other font", "Any installed font, e.g. Inter or Roboto", Ctl::Edit("font_custom", "Font name"), on && c.i("font_face") == 5));
            v.push(row("Text size", "", Ctl::Slider("font_scale", 80, 130, 5, "%"), on));
            v.push(row("Style", "", Ctl::Choice("font_style", &["Normal", "Bold", "Italic", "Bold italic"]), on));
            v.push(header("Mascot"));
            v.push(row("Mascot", "When idle: shows while the PC rests, any key or mouse move brings the island back. Always: stays on the island — click it to send it away", Ctl::Choice("mascot", &["Off", "When idle", "Always"]), on));
            v.push(row("Mascot look", "A line-drawn robot, a cat (it drums along when you type or play music), or the two of them fighting it out", Ctl::Choice("mascot_style", &["Robot", "Cat", "Cat vs Robot"]), on && c.b("mascot")));
            v.push(row("Appears after", "Idle time (When idle) · time until it comes back after a click (Always)", Ctl::Slider("mascot_sec", 10, 1800, 10, ""), on && c.b("mascot")));
        }
        2 => {
            v.push(header("Pages  (arrows change the order)"));
            for (i, p) in crate::island::ordered_pages().into_iter().enumerate() {
                v.push(row(p.name(), p.about(), Ctl::Page(i), on));
            }
            v.push(header("Now Playing"));
            v.push(row("Cover size", "Album art on Now Playing", Ctl::Slider("cover_size", 50, 125, 5, "%"), on && c.b("page_home")));
            v.push(row("Song-change cover size", "Album art in the popup when the song changes", Ctl::Slider("peek_cover", 60, 170, 5, "%"), on && c.b("osd_track")));
            v.push(header("Shelf"));
            v.push(row("Files from the other PC go to", "Shelf items you get from a linked PC (empty = Downloads\\Phemio)", Ctl::Edit("shelf_dir", "Downloads\\Phemio"), true));
            v.push(row("Pick the folder", "", Ctl::Button("Choose…", Act::PickShelfDir), true));
            v.push(header("World Clock"));
            v.push(row("Weather text size", "The current weather on the left of the Weather page", Ctl::Slider("wx_text", 70, 160, 5, "%"), on && c.b("page_weather")));
            v.push(row("World Clock size", "", Ctl::Slider("wc_size", 60, 150, 5, "%"), on && c.b("page_clock")));
            v.push(row("Volume step for several speakers", "Volume up / down moves every speaker playing together by this much (Devices → Sound output → Play on several)", Ctl::Slider("mo_step", 1, 10, 1, "%"), on));
            v.push(row("World Clock weather update", "How often each city's current weather is fetched again (minutes)", Ctl::Slider("tz_refresh", 5, 180, 5, " min"), on && c.b("page_clock")));
            v.push(header("Clock & Calendar"));
            let cc = on && c.b("page_calendar");
            v.push(row("Date and city", "Under the time", Ctl::Toggle("home_date"), cc));
            v.push(row("Clock size", "Clock & Calendar page", Ctl::Slider("home_clock_size", 60, 160, 5, "%"), cc));
            v.push(header("Clock style"));
            v.push(row("Clock font", "Now Playing and World Clock", Ctl::Choice("clock_font", &["Text font", "Segoe UI", "Bahnschrift", "Cascadia", "Georgia", "Other"]), on));
            v.push(row("Other clock font", "Any installed font, e.g. Inter or Roboto", Ctl::Edit("clock_font_custom", "Font name"), on && c.i("clock_font") == 5));
            v.push(row("Clock weight", "", Ctl::Choice("clock_weight", &["Thin", "Light\u{200B}", "Regular", "Semibold", "Bold"]), on));
            v.push(header("Focus timer"));
            let f = on && c.b("page_focus");
            v.push(row("Focus length", "", Ctl::Slider("focus_min", 5, 120, 5, " min"), f));
            v.push(row("Short break", "", Ctl::Slider("break_min", 1, 60, 1, " min"), f));
            v.push(row("Long break", "", Ctl::Slider("long_min", 5, 60, 1, " min"), f));
            v.push(row("Long break after", "This many short breaks, then a long one", Ctl::Slider("long_every", 2, 8, 1, " short breaks"), f));
            v.push(row("Chime", "Play a sound when a phase ends", Ctl::Toggle("focus_sound"), f));
        }
        3 => {
            v.push(row("Volume", "Bar when the volume changes (scroll the closed island to change it)", Ctl::Toggle("osd_volume"), on));
            v.push(row("Brightness", "Laptop screens", Ctl::Toggle("osd_brightness"), on));
            v.push(row("Caps Lock & keyboard language", "Only those keys are watched; nothing you type is read", Ctl::Toggle("osd_keys"), on));
            v.push(row("Charging & battery", "Charger plugged / unplugged, low battery", Ctl::Toggle("osd_charge"), on));
            v.push(row("Wi-Fi & internet", "Connection lost / back", Ctl::Toggle("osd_network"), on));
            v.push(row("Headphones & audio output", "Earbuds connected, output switched", Ctl::Toggle("osd_audio"), on));
            v.push(row("Song changes", "Peek the new track", Ctl::Toggle("osd_track"), on));
            v.push(row("Bluetooth devices", "Any Bluetooth device connects or disconnects", Ctl::Toggle("osd_btdev"), on));
            v.push(row("Wi-Fi, Bluetooth, airplane mode & microphone", "Turned on / off, muted / unmuted", Ctl::Toggle("osd_radios"), on));
            v.push(row("Energy saver", "On / off — also when Windows turns it on by itself at low battery", Ctl::Toggle("osd_saver"), on));
            v.push(row("To-do reminders", "Tap the island when a task is due", Ctl::Toggle("remind_todo"), on));
        }
        4 => {
            let t = c.b("tbm");
            let status = crate::app().tbmon.as_ref().map(|m| m.status.clone()).unwrap_or_else(|| "Off".into());
            v.push(row("Taskbar monitor", &format!("{} · {}", tr("Live stats inside the taskbar"), tr(&status)), Ctl::Toggle("tbm"), true));
            v.push(row("Labels", "", Ctl::Choice("tbm_icons", &["Text", "Icons"]), t));
            v.push(row("Layout", "Auto: two lines on a normal taskbar, one line on a small one", Ctl::Choice("tbm_layout", &["Auto", "One line", "Two lines"]), t));
            v.push(row("Position", "Next to the tray (drag the stats to fine-tune), or pinned to the far left / far right end (top / bottom on a side taskbar), always on top · click them for these settings", Ctl::Choice("tbm_anchor", &["Next to tray", "Far left", "Far right"]), t));
            v.push(row("Space from the edge", "Far left: the gap on its left · far right: the gap on its right", Ctl::Slider("tbm_edge", 0, 300, 1, "px"), t && c.i("tbm_anchor") != 0));
            v.push(row("Text size", "", Ctl::Slider("tbm_size", 70, 140, 5, "%"), t));
            v.push(row("Update every", "", Ctl::Slider("tbm_interval", 1, 5, 1, " s"), t));
            v.push(row("Disk space drives", "Letters to add up, e.g. C, D (empty = all fixed drives)", Ctl::Edit("tbm_drives", "All fixed drives"), t && c.b("tbm_dsk")));
            v.push(row("Background", "Taskbar colour (picked automatically) or see-through", Ctl::Choice("tbm_bg", &["Taskbar colour", "Transparent"]), t));
            v.push(row("Background height", "Share of the taskbar's height", Ctl::Slider("tbm_bg_h", 30, 100, 5, "%"), t && c.i("tbm_bg") == 0));
            v.push(row("Background corners", "", Ctl::Slider("tbm_bg_r", 0, 20, 1, " px"), t && c.i("tbm_bg") == 0));
            v.push(row("Spacing", "Space between items", Ctl::Slider("tbm_gap", 0, 30, 1, " px"), t));
            v.push(header("Items  (▲▼ reorder · ⋯ padding)"));
            let open = TBM_OPEN.load(Ordering::Relaxed);
            for (i, m) in crate::tbmon::order().into_iter().enumerate() {
                let (_, _, name, sub) = crate::tbmon::METRICS.iter().find(|x| x.0 == m).copied().unwrap();
                v.push(row(name, sub, Ctl::Metric(i), t));
                if open == i as i32 {
                    let k = crate::tbmon::pad_keys(m);
                    for (j, side) in ["Left padding", "Top padding", "Right padding", "Bottom padding"].iter().enumerate() {
                        v.push(row(side, "", Ctl::Slider(k[j], -10, 30, 1, " px"), t));
                    }
                }
            }
        }
        5 => {
            let (tbs, exs, note) = crate::glass::status();
            let (tb, ex) = (c.b("glass_tb"), c.b("glass_ex"));
            if !note.is_empty() {
                v.push(row("Note", &note, Ctl::Text(String::new()), true));
            }
            v.push(header("Taskbar"));
            v.push(row("See-through taskbar", crate::glass::state_text(tb, tbs), Ctl::Toggle("glass_tb"), true));
            v.push(row("Transparency", "0 %: as Windows draws it · 100 %: no background at all (icons stay solid)", Ctl::Slider("glass_tb_lvl", 0, 100, 1, "%"), tb));
            v.push(header("File Explorer"));
            v.push(row("See-through File Explorer", crate::glass::state_text(ex, exs), Ctl::Toggle("glass_ex"), true));
            v.push(row("Transparency", "How see-through File Explorer is", Ctl::Slider("glass_ex_lvl", 0, 100, 1, "%"), ex));
            v.push(row("What fades", "Background only: files, folders and text stay solid (dark mode) · Whole window: everything fades, never below 30 %", Ctl::Choice("glass_ex_mode", &["Background only", "Whole window"]), ex));
            v.push(header("Look"));
            v.push(row("Behind the background", "Clear: the desktop shows through as it is · Frosted: blurred, like glass (Windows turns blur off while energy saver is on)", Ctl::Choice("glass_look", &["Clear glass", "Frosted glass"]), tb || ex));
            v.push(row("Restore Windows defaults", "Switches both off and puts Windows' own look back at once (also happens when Phemio quits)", Ctl::Button("Restore", Act::GlassRestore), tb || ex));
            v.push(row("Helper log", "What the transparency helper did inside Explorer — useful when something doesn't work", Ctl::Button("Open", Act::GlassLog), true));
        }
        _ => {
            let (pws, ws, private) = util::memory_mb();
            v.push(row("Phemio 0.37.0", "Phemio – Your desktop cameo · a Dynamic Island for Windows 11, native Rust + Direct2D", Ctl::Text(String::new()), true));
            v.push(row("Source code", "github.com/nev-minhnh/Phemio · MIT or Apache-2.0", Ctl::Button("Open", Act::Url("https://github.com/nev-minhnh/Phemio")), true));
            let detail = format!("{} · {} {ws:.0} MB · {} {private:.0} MB", tr("Same as Task Manager"), tr("working set"), tr("committed"));
            v.push(row("Memory in use", &detail, Ctl::Text(format!("{pws:.1} MB")), true));
            v.push(row("Data folder", &config::dir().to_string_lossy(), Ctl::Button("Open", Act::OpenFolder), true));
            v.push(row("Copy diagnostics", "Version, the island's state and the recent log — paste it into a message when something looks wrong", Ctl::Button("Copy", Act::CopyDiag), true));
            v.push(row("Weather & clocks", "Open-Meteo, MET Norway or wttr.in", Ctl::Button("Refresh", Act::RefreshWeather), true));
            v.push(row("Quit Phemio", "", Ctl::Button("Quit", Act::Exit), true));
            v.push(header("Tips"));
            v.push(row("Pages", "Arrows, dots, mouse wheel, or ←/→ after clicking the island", Ctl::Text(String::new()), true));
            v.push(row("Closed island", "Scroll to change volume · drop files to put them on the shelf", Ctl::Text(String::new()), true));
            v.push(row("Right-click", "Tray icon or island for the menu · shelf item / task to remove it", Ctl::Text(String::new()), true));
        }
    }
    v
}

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    /// The search box (top of the side bar), and its ✕.
    Search,
    SearchClear,
    Tab(usize),
    Toggle(usize),
    Seg(usize, i32),
    Slider(usize),
    Edit(usize),
    Button(usize),
    MetricToggle(usize),
    MetricMove(usize, i32),
    MetricPad(usize),
    PageToggle(usize),
    PageMove(usize, i32),
    Switch(usize),
}

/// Which taskbar item has its padding sliders open (-1 none).
static TBM_OPEN: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
use std::sync::atomic::Ordering;

struct Fonts {
    brand: IDWriteTextFormat,
    brand_sub: IDWriteTextFormat,
    tab: IDWriteTextFormat,
    /// A tab name too long for the sidebar at the usual size.
    tab_small: IDWriteTextFormat,
    icon: IDWriteTextFormat,
    title: IDWriteTextFormat,
    label: IDWriteTextFormat,
    sub: IDWriteTextFormat,
    seg: IDWriteTextFormat,
    value: IDWriteTextFormat,
    section: IDWriteTextFormat,
    edit: IDWriteTextFormat,
}

pub struct Settings {
    pub hwnd: HWND,
    surf: Surface,
    scale: f32,
    tab: usize,
    scroll: f32,
    content_h: f32,
    hot: Option<Hit>,
    drag: Option<(usize, &'static str, i32, i32, i32, D2D_RECT_F)>,
    hits: Vec<(Hit, D2D_RECT_F)>,
    fonts: Fonts,
    dark: bool,
    edit: Option<(&'static str, String)>,
    /// Ctrl+A: the whole box is selected (typing replaces it).
    sel_all: bool,
    caret: bool,
    /// Redraw only this part next paint (the slider being dragged).
    partial: Option<D2D_RECT_F>,
    /// Where each row was drawn last (to redraw one alone).
    row_rects: Vec<D2D_RECT_F>,
    /// A dragged slider's setting waiting to be shown on the island.
    live_key: Option<&'static str>,
    /// What's typed in the search box: settings from every tab that match.
    query: String,
    /// The caret in the box being edited (byte index; None = at the end), and
    /// that box as last drawn (its text area, where the shown text starts).
    ecur: Option<usize>,
    eview: Option<(D2D_RECT_F, usize)>,
    anim: Anim,
}

/// Phemio's colours (the logo's: black glass and a neon-cyan tube).
#[derive(Clone, Copy)]
struct Pal {
    dark: bool,
    bg: D2D1_COLOR_F,
    side_top: D2D1_COLOR_F,
    side_bot: D2D1_COLOR_F,
    card: D2D1_COLOR_F,
    /// The neon: switches on, the chosen option, the slider's fill.
    accent: D2D1_COLOR_F,
    /// The neon's deeper end (gradients).
    accent2: D2D1_COLOR_F,
    /// Text on the neon.
    on_accent: u32,
}

impl Pal {
    fn new(dark: bool) -> Pal {
        if dark {
            Pal {
                dark,
                bg: rgb(0x0e1014, 1.0),
                side_top: rgb(0x0b0d11, 1.0),
                side_bot: rgb(0x07080a, 1.0),
                card: rgb(0x161a20, 1.0),
                accent: rgb(0x22c8ff, 1.0),
                accent2: rgb(0x1a8cff, 1.0),
                on_accent: 0x02121c,
            }
        } else {
            Pal {
                dark,
                bg: rgb(0xf2f6f9, 1.0),
                side_top: rgb(0xe9f1f6, 1.0),
                side_bot: rgb(0xe1eaf1, 1.0),
                card: rgb(0xffffff, 1.0),
                accent: rgb(0x0a9fdc, 1.0),
                accent2: rgb(0x0a72d6, 1.0),
                on_accent: 0xffffff,
            }
        }
    }
    fn fg(&self, a: f32) -> D2D1_COLOR_F {
        if self.dark {
            rgb(0xeef6fb, a)
        } else {
            rgb(0x0d141b, a)
        }
    }
    fn on(&self, a: f32) -> D2D1_COLOR_F {
        rgb(self.on_accent, a)
    }
}

/// Ease out (cubic).
fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Moves `v` toward `to` — a fraction of the way each frame, by time — and
/// says whether it's still moving.
fn approach(v: &mut f32, to: f32, dt: f32, speed: f32, eps: f32) -> bool {
    let d = to - *v;
    if d.abs() <= eps {
        *v = to;
        return false;
    }
    *v += d * (1.0 - (-dt * speed).exp());
    true
}

/// What's moving in the window: each value glides to where it should be.
struct Anim {
    last: std::time::Instant,
    /// The chosen tab's pill (y) in the sidebar.
    tab_y: f32,
    /// The page fading in after a tab change or a new search (0 → 1).
    page_t: f32,
    /// Where the page is scrolling to.
    scroll_to: f32,
    /// Switches: how far each one's knob is across (0 off … 1 on).
    knobs: std::collections::HashMap<String, f32>,
    /// What each switch should show (from the last paint).
    knob_to: std::collections::HashMap<String, f32>,
    /// The pointer over a sidebar tab: its highlight fading in or out.
    tab_hot: [f32; 8],
    running: bool,
}

/// Where Ctrl+←/→ (or Ctrl+Backspace / Delete) lands from `p` in `buf`.
fn word_jump(buf: &str, p: usize, back: bool) -> usize {
    if back {
        let v: Vec<(usize, char)> = buf[..p].char_indices().collect();
        let mut k = v.len();
        while k > 0 && v[k - 1].1.is_whitespace() {
            k -= 1;
        }
        while k > 0 && !v[k - 1].1.is_whitespace() {
            k -= 1;
        }
        v.get(k).map(|x| x.0).unwrap_or(0)
    } else {
        let mut it = buf[p..].char_indices().peekable();
        while it.peek().map(|x| !x.1.is_whitespace()).unwrap_or(false) {
            it.next();
        }
        while it.peek().map(|x| x.1.is_whitespace()).unwrap_or(false) {
            it.next();
        }
        it.peek().map(|x| p + x.0).unwrap_or(buf.len())
    }
}

/// Every setting (all tabs) whose name, description or options match `q`,
/// under its tab's name; a tab's name matches all of it.
fn search_rows(q: &str, c: &Config) -> Vec<Row> {
    let q = q.trim().to_lowercase();
    let mut out = Vec::new();
    for (t, (tab, _)) in TABS.iter().enumerate() {
        let tab_hit = tab.to_lowercase().contains(&q) || tr(tab).to_lowercase().contains(&q);
        let mut found: Vec<Row> = Vec::new();
        let mut section = String::new();
        for r in rows(t, c) {
            if r.header {
                section = r.label.clone();
                continue;
            }
            let mut hay = format!("{} {} {} {} {}", r.label, r.sub, tr(&r.label), tr(&r.sub), section).to_lowercase();
            if let Ctl::Choice(_, opts) = &r.ctl {
                for o in opts.iter() {
                    hay.push(' ');
                    hay.push_str(&o.to_lowercase());
                    hay.push(' ');
                    hay.push_str(&tr(o).to_lowercase());
                }
            }
            if tab_hit || hay.contains(&q) {
                found.push(r);
            }
        }
        if !found.is_empty() {
            out.push(header(tab));
            out.extend(found);
        }
    }
    out
}

pub unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let app = crate::app();
    let Some(s) = app.settings.as_mut().filter(|s| s.hwnd == hwnd) else {
        return DefWindowProcW(hwnd, msg, wp, lp);
    };
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            s.render();
            s.surf.blit(hdc);
            s.surf.debug_dump(hwnd);
            let _ = EndPaint(hwnd, &ps);
            // A long line scrolling: its next frame.
            if let Some(t) = s.surf.take_marquee() {
                SetTimer(Some(hwnd), T_MARQUEE, ((t * 1000.0) as u32).max(40), None);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            s.on_move(lp);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            s.on_down(lp);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if s.drag.take().is_some() {
                let _ = ReleaseCapture();
                let _ = KillTimer(Some(hwnd), T_LIVE);
                s.live_key = None;
                // Written and applied in full once, when you let go.
                config::flush();
                crate::request_apply();
                s.invalidate();
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL if s.wheel_slider(((wp.0 >> 16) & 0xffff) as i16) => LRESULT(0),
        WM_MOUSEWHEEL => {
            let d = ((wp.0 >> 16) & 0xffff) as i16 as f32 / 120.0;
            s.anim.scroll_to = (s.anim.scroll_to - d * 72.0).clamp(0.0, (s.content_h - (H - 80.0)).max(0.0));
            s.kick();
            LRESULT(0)
        }
        WM_CHAR => {
            s.on_char(wp.0 as u16);
            LRESULT(0)
        }
        // ←/→ (Ctrl: a word), Home/End, Delete in the box being edited.
        WM_KEYDOWN if s.edit.is_some() && s.caret_key(VIRTUAL_KEY(wp.0 as u16)) => LRESULT(0),
        WM_TIMER => {
            if wp.0 == T_LIVE {
                let _ = KillTimer(Some(hwnd), T_LIVE);
                if let Some(k) = s.live_key.take() {
                    crate::app().apply_live(k);
                }
            } else if wp.0 == T_WHEEL_APPLY {
                let _ = KillTimer(Some(hwnd), T_WHEEL_APPLY);
                config::flush();
                crate::request_apply();
            } else if wp.0 == T_ANIM {
                s.tick();
            } else if wp.0 == T_MARQUEE {
                let _ = KillTimer(Some(hwnd), T_MARQUEE);
                s.invalidate();
            } else if wp.0 == T_CARET {
                s.caret = !s.caret;
                s.invalidate();
            } else if s.tab >= 4 || (s.tab == 0 && crate::contentindex::status().running) {
                s.invalidate();
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let r = &*(lp.0 as *const RECT);
            s.scale = (wp.0 & 0xffff) as f32 / 96.0;
            let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            s.resize();
            LRESULT(0)
        }
        WM_CLOSE => {
            s.commit_edit();
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            app.settings_closed();
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

pub fn register(icon: HICON) {
    util::register_class(CLASS, Some(wndproc), icon);
}

/// Repaint the Settings window (if it's open) from any thread.
pub fn refresh() {
    unsafe {
        if let Ok(h) = FindWindowW(CLASS, PCWSTR::null()) {
            if !h.is_invalid() {
                let _ = InvalidateRect(Some(h), None, false);
            }
        }
    }
}

impl Settings {
    pub fn open(tab: usize) -> Option<Box<Settings>> {
        unsafe {
            let mon = util::primary_monitor();
            let scale = mon.scale;
            let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
            let mut r = RECT { left: 0, top: 0, right: (W * scale) as i32, bottom: (H * scale) as i32 };
            let _ = windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi(&mut r, style, false, WINDOW_EX_STYLE(0), (scale * 96.0) as u32);
            let (ww, wh) = (r.right - r.left, r.bottom - r.top);
            let wa = mon.work;
            let x = wa.left + ((wa.right - wa.left) - ww) / 2;
            let y = wa.top + ((wa.bottom - wa.top) - wh) / 2;
            let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), CLASS, w!("Phemio Settings"), style, x, y, ww, wh, None, None, Some(util::hinstance()), None).ok()?;
            let dark = util::is_dark(config::cur().i("theme"));
            let v: u32 = dark as u32;
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const u32 as *const _, 4);
            let g = gfx::g();
            let mut s = Box::new(Settings {
                hwnd,
                surf: Surface::new().ok()?,
                scale,
                tab,
                scroll: std::env::var("ISLET_SETTINGS_SCROLL").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0),
                content_h: 0.0,
                hot: None,
                drag: None,
                hits: Vec::new(),
                fonts: Fonts {
                    brand: g.font(DISPLAY_FONT, 22.0, 800, 0),
                    brand_sub: g.font(UI_FONT, 11.5, 500, 0),
                    tab: g.font(UI_FONT, 13.5, 600, 0),
                    tab_small: g.font(UI_FONT, 12.0, 600, 0),
                    icon: g.font(ICON_FONT, 16.0, 400, 1),
                    title: g.font(DISPLAY_FONT, 24.0, 700, 0),
                    label: g.font(UI_FONT, 14.0, 600, 0),
                    sub: g.font(UI_FONT, 12.0, 400, 0),
                    seg: g.font(UI_FONT, 12.5, 600, 1),
                    value: g.font(UI_FONT, 12.5, 600, 2),
                    section: g.font(UI_FONT, 12.0, 700, 0),
                    edit: g.font(UI_FONT, 13.0, 500, 0),
                },
                dark,
                edit: None,
                sel_all: false,
                caret: true,
                partial: None,
                row_rects: Vec::new(),
                live_key: None,
                query: String::new(),
                ecur: None,
                eview: None,
                anim: Anim {
                    last: std::time::Instant::now(),
                    tab_y: TAB_Y0 + tab as f32 * TAB_STEP,
                    page_t: 1.0,
                    scroll_to: 0.0,
                    knobs: Default::default(),
                    knob_to: Default::default(),
                    tab_hot: [0.0; 8],
                    running: false,
                },
            });
            s.anim.scroll_to = s.scroll;
            s.resize();
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            SetTimer(Some(hwnd), T_TICK, 1000, None);
            Some(s)
        }
    }

    pub fn show(&mut self, tab: usize) {
        if tab != self.tab {
            self.go_tab(tab);
        }
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(self.hwnd);
        }
        self.invalidate();
    }

    fn resize(&mut self) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut r);
        }
        self.surf.resize(r.right.max(1), r.bottom.max(1));
        self.invalidate();
    }

    pub fn refresh_theme(&mut self) {
        self.dark = util::is_dark(config::cur().i("theme"));
        let v: u32 = self.dark as u32;
        unsafe {
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const u32 as *const _, 4);
        }
        self.invalidate();
    }

    /// Another tab: the pill glides there, the page fades in from the top.
    fn go_tab(&mut self, t: usize) {
        self.tab = t;
        self.scroll = 0.0;
        self.anim.scroll_to = 0.0;
        self.anim.page_t = 0.0;
        self.kick();
    }

    /// Start the animation frames (if they aren't running).
    fn kick(&mut self) {
        if !self.anim.running {
            self.anim.running = true;
            self.anim.last = std::time::Instant::now();
            unsafe {
                SetTimer(Some(self.hwnd), T_ANIM, 8, None);
            }
        }
    }

    /// One animation frame: move everything on by the time since the last.
    fn tick(&mut self) {
        let now = std::time::Instant::now();
        let dt = (now - self.anim.last).as_secs_f32().clamp(0.0, 0.05);
        self.anim.last = now;
        let a = &mut self.anim;
        let mut busy = false;
        let ty = TAB_Y0 + self.tab as f32 * TAB_STEP;
        busy |= approach(&mut a.tab_y, ty, dt, 16.0, 0.2);
        if a.page_t < 1.0 {
            a.page_t = (a.page_t + dt / 0.26).min(1.0);
            busy = true;
        }
        let max = (self.content_h - (H - 80.0)).max(0.0);
        a.scroll_to = a.scroll_to.clamp(0.0, max);
        busy |= approach(&mut self.scroll, a.scroll_to, dt, 14.0, 0.3);
        for (k, v) in a.knobs.iter_mut() {
            let to = a.knob_to.get(k).copied().unwrap_or(*v);
            busy |= approach(v, to, dt, 20.0, 0.01);
        }
        for (t, h) in a.tab_hot.iter_mut().enumerate() {
            let to = if self.hot == Some(Hit::Tab(t)) { 1.0 } else { 0.0 };
            busy |= approach(h, to, dt, 18.0, 0.02);
        }
        if !busy {
            a.running = false;
            unsafe {
                let _ = KillTimer(Some(self.hwnd), T_ANIM);
            }
        }
        self.invalidate();
    }

    /// A switch's knob position (0 off … 1 on), gliding when it changed.
    fn knob(&mut self, key: String, on: bool) -> f32 {
        let to = if on { 1.0 } else { 0.0 };
        let cur = *self.anim.knobs.entry(key.clone()).or_insert(to);
        if (self.anim.knob_to.insert(key, to).unwrap_or(to) - to).abs() > 0.0 || (cur - to).abs() > 0.01 {
            self.kick();
        }
        cur
    }

    /// The rows on show: the tab's, or — while searching — every match.
    fn view_rows(&self, c: &Config) -> Vec<Row> {
        if self.query.trim().is_empty() {
            rows(self.tab, c)
        } else {
            search_rows(&self.query, c)
        }
    }

    /// Paint right away — only `part` (DIPs) when given.
    fn paint_now(&mut self, part: Option<D2D_RECT_F>) {
        self.partial = part.map(|r| rf(r.left - 4.0, r.top.max(64.0), r.right - r.left + 8.0, (r.bottom - r.top.max(64.0)).max(0.0)));
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
            let _ = UpdateWindow(self.hwnd);
        }
    }

    pub fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    /// The wheel over a slider (that's being pointed at) moves it a step.
    fn wheel_slider(&mut self, delta: i16) -> bool {
        let Some(Hit::Slider(i)) = self.hot else { return false };
        let c = config::cur();
        let rows = self.view_rows(&c);
        let Some(Ctl::Slider(k, min, max, step, _)) = rows.get(i).map(|r| r.ctl.clone()) else { return false };
        if !rows[i].enabled {
            return false;
        }
        let v = (c.i(k) + if delta > 0 { step } else { -step }).clamp(min, max);
        if v != c.i(k) {
            config::update(|c| c.set(k, v));
            crate::app().apply_live(k);
            let row = self.row_rects.get(i).copied();
            self.paint_now(row);
            unsafe { SetTimer(Some(self.hwnd), T_WHEEL_APPLY, 400, None) };
        }
        true
    }

    fn pos(&self, lp: LPARAM) -> (f32, f32) {
        (util::lo(lp) as f32 / self.scale, util::hi(lp) as f32 / self.scale)
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits.iter().rev().find(|(_, r)| inside(r, x, y)).map(|(h, _)| *h)
    }

    fn on_move(&mut self, lp: LPARAM) {
        let (x, y) = self.pos(lp);
        if let Some((_, key, min, max, step, track)) = self.drag {
            let f = ((x - track.left) / (track.right - track.left)).clamp(0.0, 1.0);
            let raw = min as f32 + (max - min) as f32 * f;
            let v = ((raw / step as f32).round() as i32 * step).clamp(min, max);
            if config::cur().i(key) != v {
                // In memory while dragging (written once on release); the
                // slider itself is redrawn at once — just its row — and the
                // island shows the change a little later, at most every
                // LIVE_MS, so the knob never waits on it.
                config::update_live(|c| c.set(key, v));
                if self.live_key.replace(key).is_none() {
                    unsafe {
                        SetTimer(Some(self.hwnd), T_LIVE, LIVE_MS, None);
                    }
                }
                let row = self.drag.map(|d| d.0).and_then(|i| self.row_rects.get(i).copied());
                self.paint_now(row);
            }
            return;
        }
        let h = self.hit(x, y);
        if h != self.hot {
            if matches!(h, Some(Hit::Tab(_))) || matches!(self.hot, Some(Hit::Tab(_))) {
                self.hot = h;
                self.kick();
            }
            self.hot = h;
            self.invalidate();
        }
        unsafe {
            let cur = match h {
                Some(Hit::Edit(_)) => IDC_IBEAM,
                Some(_) => IDC_HAND,
                None => IDC_ARROW,
            };
            SetCursor(LoadCursorW(None, cur).ok());
        }
    }

    fn commit_edit(&mut self) {
        if let Some((key, text)) = self.edit.take() {
            let c = config::cur();
            if key == "__search" {
                self.query = text;
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), T_CARET);
                }
                return;
            }
            if key == "gemini_key" {
                if c.s(key) != text.trim() {
                    config::update(|c| c.set_s(key, text.trim()));
                    crate::gemini::forget_status();
                }
            } else if key == "shelf_dir" {
                config::update(|c| c.set_s(key, text.trim().trim_matches('"')));
                crate::link::shelf_dir_changed();
            } else if key == "font_custom" || key == "clock_font_custom" || key == "tbm_drives" {
                config::update(|c| c.set_s(key, text.trim()));
                crate::request_apply();
            } else if c.s(key) != text.trim() {
                let prefix = if key == "weather_city" { "weather" } else { "tz2" };
                config::update(|c| {
                    c.set_s(key, text.trim());
                    c.set_s(&format!("{prefix}_place"), "");
                });
                crate::app().refresh_weather(true);
            }
            unsafe {
                let _ = KillTimer(Some(self.hwnd), T_CARET);
            }
        }
    }

    fn on_char(&mut self, ch: u16) {
        // Typing or pasting (Ctrl+V), or Ctrl+A, with no box open: it goes to
        // the search box.
        if self.edit.is_none() && ((ch >= 32 && ch != 127) || ch == 22 || (ch == 1 && !self.query.is_empty())) {
            self.ecur = None;
            self.edit = Some(("__search", self.query.clone()));
            unsafe {
                SetTimer(Some(self.hwnd), T_CARET, 530, None);
            }
        }
        if ch == 27 && self.edit.as_ref().map(|e| e.0) == Some("__search") {
            self.query.clear();
            self.edit = None;
            self.scroll = 0.0;
            self.anim.scroll_to = 0.0;
            self.anim.page_t = 0.0;
            self.kick();
            return;
        }
        if ch == 6 {
            // Ctrl+F: the search box.
            self.commit_edit();
            self.ecur = None;
            self.edit = Some(("__search", self.query.clone()));
            self.caret = true;
            self.invalidate();
            return;
        }
        let searching = self.edit.as_ref().map(|e| e.0) == Some("__search");
        self.on_char_edit(ch);
        if searching {
            self.sync_search();
            self.invalidate();
        }
    }

    fn on_char_edit(&mut self, ch: u16) {
        let Some((_, buf)) = self.edit.as_ref() else { return };
        let empty = buf.is_empty();
        let all = std::mem::take(&mut self.sel_all);
        match ch {
            // Ctrl+A: select everything; Ctrl+C / Ctrl+X: copy / cut it (the
            // whole box — there's no partial selection).
            1 => self.sel_all = !empty,
            3 => {
                if !empty {
                    util::set_clipboard_text(&buf.clone());
                }
                self.sel_all = all;
            }
            24 => {
                if !empty {
                    util::set_clipboard_text(&buf.clone());
                    self.change(|b, _| {
                        b.clear();
                        0
                    });
                }
            }
            8 | 127 if all => self.change(|b, _| {
                b.clear();
                0
            }),
            // Backspace: the character before the caret.
            8 => self.change(|b, p| match b[..p].char_indices().last() {
                Some((i, _)) => {
                    b.replace_range(i..p, "");
                    i
                }
                None => p,
            }),
            // Ctrl+Backspace: the word before the caret.
            127 => self.change(|b, p| {
                let from = word_jump(b, p, true);
                b.replace_range(from..p, "");
                from
            }),
            13 => self.commit_edit(),
            27 => self.edit = None,
            22 => {
                if let Some(t) = util::clipboard_text() {
                    let line = t.lines().next().unwrap_or("").trim_end_matches('\r').to_string();
                    self.change(|b, p| {
                        let p = if all {
                            b.clear();
                            0
                        } else {
                            p
                        };
                        b.insert_str(p, &line);
                        p + line.len()
                    });
                }
            }
            c if c >= 32 => {
                if let Some(c) = char::decode_utf16([c]).next().and_then(|r| r.ok()) {
                    self.change(|b, p| {
                        let p = if all {
                            b.clear();
                            0
                        } else {
                            p
                        };
                        b.insert(p, c);
                        p + c.len_utf8()
                    });
                }
            }
            _ => {}
        }
        self.caret = true;
        self.invalidate();
    }

    /// The caret's place in the box being edited (always valid).
    fn cur_pos(&self) -> usize {
        let n = self.edit.as_ref().map(|e| e.1.len()).unwrap_or(0);
        match (self.ecur, self.edit.as_ref()) {
            (Some(c), Some((_, b))) if c < n && b.is_char_boundary(c) => c,
            _ => n,
        }
    }

    fn set_cur(&mut self, p: usize) {
        let n = self.edit.as_ref().map(|e| e.1.len()).unwrap_or(0);
        self.ecur = (p < n).then_some(p);
    }

    /// Change the box's text: `f` gets it and the caret, returns the new caret.
    fn change(&mut self, f: impl FnOnce(&mut String, usize) -> usize) {
        let p = self.cur_pos();
        let Some((_, b)) = self.edit.as_mut() else { return };
        let to = f(b, p);
        self.set_cur(to);
    }

    /// ←/→ (Ctrl: a word), Home/End, Delete (Ctrl: a word). True if handled.
    fn caret_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        let ctrl = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(VK_CONTROL.0 as i32) } < 0;
        let p = self.cur_pos();
        let Some(buf) = self.edit.as_ref().map(|e| e.1.clone()) else { return false };
        match vk {
            VK_LEFT => self.set_cur(if ctrl { word_jump(&buf, p, true) } else { buf[..p].char_indices().last().map(|x| x.0).unwrap_or(0) }),
            VK_RIGHT => self.set_cur(if ctrl { word_jump(&buf, p, false) } else { buf[p..].chars().next().map(|c| p + c.len_utf8()).unwrap_or(p) }),
            VK_HOME => self.set_cur(0),
            VK_END => self.ecur = None,
            VK_DELETE => {
                if std::mem::take(&mut self.sel_all) {
                    self.change(|b, _| {
                        b.clear();
                        0
                    });
                } else {
                    let to = if ctrl { word_jump(&buf, p, false) } else { buf[p..].chars().next().map(|c| p + c.len_utf8()).unwrap_or(p) };
                    self.change(|b, p| {
                        b.replace_range(p..to, "");
                        p
                    });
                }
                self.sync_search();
            }
            _ => return false,
        }
        if vk != VK_DELETE {
            self.sel_all = false;
        }
        self.caret = true;
        unsafe {
            // Keep the caret steady while it moves.
            SetTimer(Some(self.hwnd), T_CARET, 530, None);
        }
        self.invalidate();
        true
    }

    /// The search box's text changed: show its matches.
    fn sync_search(&mut self) {
        if let Some(("__search", b)) = self.edit.as_ref().map(|(k, b)| (*k, b.clone())) {
            if b != self.query {
                // A fresh search (or back to the tab): the results fade in.
                if b.trim().is_empty() != self.query.trim().is_empty() {
                    self.anim.page_t = 0.0;
                    self.kick();
                }
                self.query = b;
                self.scroll = 0.0;
                self.anim.scroll_to = 0.0;
            }
        }
    }

    /// A click inside the box being edited: the caret goes there. True if handled.
    fn caret_click(&mut self, x: f32, y: f32) -> bool {
        let Some((r, start)) = self.eview else { return false };
        if self.edit.is_none() || !inside(&rf(r.left - 6.0, r.top, r.right - r.left + 12.0, r.bottom - r.top), x, y) {
            return false;
        }
        let buf = self.edit.as_ref().map(|e| e.1.clone()).unwrap_or_default();
        let start = start.min(buf.len());
        let g = gfx::g();
        let mut stops: Vec<usize> = buf[start..].char_indices().map(|(i, _)| start + i).collect();
        stops.push(buf.len());
        let best = stops.into_iter().min_by(|&i, &j| {
            let di = (r.left + g.measure(&buf[start..i], &self.fonts.edit) - x).abs();
            let dj = (r.left + g.measure(&buf[start..j], &self.fonts.edit) - x).abs();
            di.total_cmp(&dj)
        });
        self.sel_all = false;
        self.set_cur(best.unwrap_or(buf.len()));
        self.caret = true;
        self.invalidate();
        true
    }

    /// One line of an edit box's text in `tr` (`mid`: its middle), scrolled
    /// so the caret stays in view, with the caret and the Ctrl+A selection.
    /// `editing`: this is the box being edited (remembered for clicks).
    fn draw_edit_text(&mut self, text: &str, tr: D2D_RECT_F, mid: f32, editing: bool, col: D2D1_COLOR_F, caret: D2D1_COLOR_F, sel: D2D1_COLOR_F) {
        let g = gfx::g();
        let maxw = (tr.right - tr.left - 4.0).max(10.0);
        let cur = if editing { self.cur_pos().min(text.len()) } else { text.len() };
        let mut start = 0;
        while start < cur && g.measure(&text[start..cur], &self.fonts.edit) > maxw {
            start += text[start..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        }
        let shown = &text[start..];
        if editing && self.sel_all && !shown.is_empty() {
            let w = g.measure(shown, &self.fonts.edit).min(maxw + 4.0);
            self.surf.fill_rr(rf(tr.left - 2.0, mid - 11.0, w + 4.0, 22.0), 4.0, sel);
        }
        self.surf.clip(rf(tr.left - 2.0, tr.top, tr.right - tr.left + 4.0, tr.bottom - tr.top));
        self.surf.text(shown, &self.fonts.edit, rf(tr.left, tr.top, 4000.0, tr.bottom - tr.top), col);
        self.surf.unclip();
        if editing {
            if self.caret {
                let cx = tr.left + g.measure(&text[start..cur], &self.fonts.edit);
                self.surf.fill_rect(rf(cx + 1.0, mid - 8.0, 1.5, 16.0), caret);
            }
            self.eview = Some((tr, start));
        }
    }

    fn on_down(&mut self, lp: LPARAM) {
        let (x, y) = self.pos(lp);
        if self.caret_click(x, y) {
            return;
        }
        let h = self.hit(x, y);
        if !matches!(h, Some(Hit::Edit(_))) {
            self.commit_edit();
        }
        let Some(h) = h else {
            self.invalidate();
            return;
        };
        let c = config::cur();
        let rows = self.view_rows(&c);
        match h {
            Hit::Search => {
                if self.edit.as_ref().map(|e| e.0) != Some("__search") {
                    self.commit_edit();
                    self.ecur = None;
                    self.edit = Some(("__search", self.query.clone()));
                }
                self.caret = true;
                unsafe {
                    SetTimer(Some(self.hwnd), T_CARET, 530, None);
                    let _ = SetFocus(Some(self.hwnd));
                }
            }
            Hit::SearchClear => {
                self.query.clear();
                if self.edit.as_ref().map(|e| e.0) == Some("__search") {
                    self.ecur = None;
                    self.edit = Some(("__search", String::new()));
                }
                self.scroll = 0.0;
                self.anim.scroll_to = 0.0;
                self.anim.page_t = 0.0;
                self.kick();
            }
            Hit::Tab(t) => {
                if t != self.tab || !self.query.trim().is_empty() {
                    self.go_tab(t);
                }
                // A tab: leave the search.
                self.query.clear();
                if self.edit.as_ref().map(|e| e.0) == Some("__search") {
                    self.edit = None;
                }
            }
            Hit::Toggle(i) => {
                if let Some(Ctl::Toggle(k)) = rows.get(i).map(|r| r.ctl.clone()) {
                    let v = !c.b(k);
                    config::update(|c| c.set(k, v as i32));
                    crate::request_apply();
                }
            }
            Hit::Switch(i) => {
                if let Some(Ctl::Switch(sw, on)) = rows.get(i).map(|r| r.ctl.clone()) {
                    match sw {
                        Sw::Drive(d) => crate::contentindex::set_drive(d, !on),
                    }
                }
            }
            Hit::Seg(i, v) => {
                if let Some(Ctl::Choice(k, _)) = rows.get(i).map(|r| r.ctl.clone()) {
                    config::update(|c| {
                        c.set(k, v);
                        // Picking Left / Center / Right starts again from that spot.
                        if k == "island_align" {
                            c.set("island_x", 0);
                        }
                        if k == "tbm_anchor" {
                            c.set("tbm_shift", 0);
                        }
                    });
                    if k == "weather_unit" || k == "weather_src" {
                        crate::app().refresh_weather(true);
                    }
                    crate::request_apply();
                }
            }
            Hit::Slider(i) => {
                if let Some(Ctl::Slider(k, min, max, step, _)) = rows.get(i).map(|r| r.ctl.clone()) {
                    if let Some((_, r)) = self.hits.iter().find(|(hh, _)| *hh == h) {
                        let track = D2D_RECT_F { left: r.left + 8.0, right: r.right - 8.0, ..*r };
                        self.drag = Some((i, k, min, max, step, track));
                        unsafe { SetCapture(self.hwnd) };
                        self.on_move(lp);
                    }
                }
            }
            Hit::Edit(i) => {
                if let Some(Ctl::Edit(k, _)) = rows.get(i).map(|r| r.ctl.clone()) {
                    if self.edit.as_ref().map(|e| e.0) != Some(k) {
                        self.ecur = None;
                        self.edit = Some((k, c.s(k)));
                    }
                    self.caret = true;
                    unsafe {
                        SetTimer(Some(self.hwnd), T_CARET, 530, None);
                        let _ = SetFocus(Some(self.hwnd));
                    }
                }
            }
            Hit::MetricToggle(i) => {
                let Some(&m) = crate::tbmon::order().get(i) else { return };
                let k = format!("tbm_{}", crate::tbmon::key_of(m));
                let v = !c.b(&k);
                config::update(|c| c.set(&k, v as i32));
                crate::request_apply();
            }
            Hit::PageToggle(i) => {
                if let Some(p) = crate::island::ordered_pages().get(i).copied() {
                    let k = p.key();
                    let v = !c.b(k);
                    config::update(|c| c.set(k, v as i32));
                    crate::request_apply();
                }
            }
            Hit::PageMove(i, d) => {
                if let Some(p) = crate::island::ordered_pages().get(i).copied() {
                    crate::island::move_page(p, d);
                    crate::request_apply();
                }
            }
            Hit::MetricPad(i) => {
                let cur = TBM_OPEN.load(Ordering::Relaxed);
                TBM_OPEN.store(if cur == i as i32 { -1 } else { i as i32 }, Ordering::Relaxed);
            }
            Hit::MetricMove(i, d) => {
                if let Some(&m) = crate::tbmon::order().get(i) {
                    crate::tbmon::move_metric(m, d);
                }
                crate::request_apply();
            }
            Hit::Button(i) => {
                if let Some(Ctl::Button(_, act)) = rows.get(i).map(|r| r.ctl.clone()) {
                    match act {
                        Act::Url(u) => util::shell_open(u),
                        Act::PickShelfDir => {
                            if let Some(d) = util::pick_folder(self.hwnd) {
                                config::update(|c| c.set_s("shelf_dir", &d));
                                crate::link::shelf_dir_changed();
                            }
                        }
                        Act::OpenFolder => {
                            let _ = std::fs::create_dir_all(config::dir());
                            util::shell_open(&config::dir().to_string_lossy());
                        }
                        Act::CopyDiag => {
                            let mut t = format!("Phemio {} · language {} · link_on {}\n", env!("CARGO_PKG_VERSION"), crate::i18n::lang(), c.b("link_on"));
                            if let Some(i) = crate::app().island.as_ref() {
                                t.push_str(&i.diagnostics());
                                t.push('\n');
                            }
                            if let Some(v) = crate::link::view() {
                                t.push_str(&format!("link: session={:?} hold={} quiet={} paired={}\n", v.session.as_ref().map(|s| &s.0), v.hold, v.quiet, v.paired.len()));
                            }
                            let log = std::fs::read_to_string(config::dir().join("phemio.log")).unwrap_or_default();
                            let lines: Vec<&str> = log.lines().collect();
                            t.push_str("--- log ---\n");
                            t.push_str(&lines[lines.len().saturating_sub(60)..].join("\n"));
                            let crash = std::fs::read_to_string(config::dir().join("crash.log")).unwrap_or_default();
                            if !crash.is_empty() {
                                t.push_str("\n--- crash.log ---\n");
                                t.push_str(&crash);
                            }
                            util::set_clipboard_text(&t);
                        }
                        Act::Exit => unsafe {
                            let _ = PostMessageW(Some(crate::app().host), WM_CLOSE, WPARAM(0), LPARAM(0));
                        },
                        Act::RefreshWeather => crate::app().refresh_weather(true),
                        Act::GlassRestore => crate::glass::restore(),
                        Act::GlassLog => {
                            let p = crate::glass::log_path();
                            if !p.exists() {
                                let _ = std::fs::write(&p, "");
                            }
                            util::shell_open(&p.to_string_lossy());
                        }
                        Act::IndexNow => crate::contentindex::update_now(),
                        Act::IndexStop => crate::contentindex::stop_pass(),
                        Act::IndexClear => crate::contentindex::clear(),
                        Act::FirewallAllow => crate::link::firewall::allow(),
                        Act::TestGemini => {
                            self.commit_edit();
                            crate::gemini::test();
                        }
                    }
                }
            }
        }
        self.invalidate();
    }

    /// A switch in `tr` with its knob `t` of the way across (0 off … 1 on):
    /// the neon fills in as the knob slides.
    fn draw_switch(&self, tr: D2D_RECT_F, t: f32, hot: bool, a: f32, pal: &Pal) {
        let e = ease(t);
        let (w, h) = (tr.right - tr.left, tr.bottom - tr.top);
        let rad = h / 2.0;
        if e < 0.999 {
            self.surf.stroke_rr(rf(tr.left + 0.75, tr.top + 0.75, w - 1.5, h - 1.5), rad - 0.75, pal.fg(0.42 * a * (1.0 - e)), 1.5);
        }
        if e > 0.001 {
            if pal.dark {
                self.surf.fill_rr(rf(tr.left - 2.0, tr.top - 2.0, w + 4.0, h + 4.0), rad + 2.0, with_alpha(pal.accent, 0.14 * e * a));
            }
            self.surf.fill_rr_grad(tr, rad, with_alpha(pal.accent, e * a), with_alpha(pal.accent2, e * a));
        }
        let kx = tr.left + rad + (w - 2.0 * rad) * e;
        let mid = tr.top + rad;
        let kr = if hot { 7.5 } else { 6.5 } + e * 0.5;
        if e < 0.999 {
            self.surf.fill_circle(kx, mid, kr, pal.fg(0.72 * a * (1.0 - e)));
        }
        if e > 0.001 {
            self.surf.fill_circle(kx, mid + 0.8, kr + 0.6, rgb(0x000000, 0.18 * e * a));
            self.surf.fill_circle(kx, mid, kr, rgb(0xffffff, e * a));
        }
    }

    // ------------------------------------------------------------------

    fn render(&mut self) {
        let _p = util::Prof::new("settings.render");
        let s = self.scale;
        // Just one row (a slider being dragged): keep the rest as it is.
        let part = self.partial.take().filter(|p| p.bottom > p.top);
        let ok = match part {
            Some(p) => self.surf.begin_part(s, p),
            None => self.surf.begin(s),
        };
        if !ok {
            return;
        }
        let n_hits = self.hits.len();
        if part.is_none() {
            self.hits.clear();
            self.row_rects.clear();
            self.eview = None;
        }
        let c = config::cur();
        let dark = self.dark;
        let pal = Pal::new(dark);
        let fg = |a: f32| pal.fg(a);
        let bg = pal.bg;
        let card = pal.card;
        let accent = pal.accent;
        // The page fading in (and settling a little upward) after a tab change.
        let page_e = ease(self.anim.page_t);
        let (w, h) = (self.surf.dip_w(), self.surf.dip_h());
        self.surf.clear(bg);

        let rows = self.view_rows(&c);
        let cx = SIDE + 26.0;
        let cw = w - SIDE - 52.0;
        let mut y = 78.0 - self.scroll + (1.0 - page_e) * 18.0;
        self.surf.clip(rf(SIDE, 64.0, w - SIDE, h - 64.0));
        if let Some(p) = part {
            self.surf.clip(p);
            self.surf.clear(bg);
        }
        for (i, r) in rows.iter().enumerate() {
            if part.is_none() {
                while self.row_rects.len() < i {
                    self.row_rects.push(rf(0.0, 0.0, 0.0, 0.0));
                }
            }
            if r.header {
                if part.is_none() {
                    self.row_rects.push(rf(0.0, 0.0, 0.0, 0.0));
                }
                y += 10.0;
                self.surf.text(&tr(&r.label).to_uppercase(), &self.fonts.section, rf(cx + 4.0, y, cw, 22.0), with_alpha(accent, if dark { 0.85 } else { 0.95 }));
                y += 28.0;
                continue;
            }
            // Room the control takes on the right.
            let ctl_w = match &r.ctl {
                Ctl::Choice(_, opts) => opts.iter().map(|o| gfx::g().measure(tr(o), &self.fonts.seg) + if opts.len() >= 5 { 16.0 } else { 26.0 }).sum::<f32>() + 6.0,
                Ctl::Slider(..) => 300.0,
                Ctl::Edit(..) => 260.0,
                Ctl::Toggle(_) => 46.0,
                Ctl::Metric(_) => 164.0,
                Ctl::Page(_) => 130.0,
                Ctl::Button(l, _) => gfx::g().measure(tr(l), &self.fonts.seg) + 32.0,
                Ctl::Text(t) if !t.is_empty() => 220.0,
                Ctl::Text(_) => 0.0,
                Ctl::Switch(..) => 46.0,
            };
            // A long row of choices that would run into the name: the
            // choices go on their own line under it.
            let stack = matches!(r.ctl, Ctl::Choice(..)) && gfx::g().measure(tr(&r.label), &self.fonts.label) + ctl_w + 60.0 > cw;
            let rh = match (stack, r.sub.is_empty()) {
                (true, true) => 84.0,
                (true, false) => 100.0,
                (false, true) => 52.0,
                (false, false) => 62.0,
            };
            if part.is_none() {
                self.row_rects.push(rf(cx, y, cw, rh));
            } else if part.map(|p| y > p.bottom || y + rh < p.top).unwrap_or(false) {
                y += rh + 8.0;
                continue;
            }
            self.surf.fill_rr(rf(cx, y, cw, rh), 12.0, card);
            self.surf.stroke_rr(rf(cx + 0.5, y + 0.5, cw - 1.0, rh - 1.0), 11.5, if dark { rgb(0x22c8ff, 0.07) } else { rgb(0x0d141b, 0.06) }, 1.0);
            let a = if r.enabled { 1.0 } else { 0.38 };
            let text_w = cw - 320.0;
            let wide_text = stack || matches!(r.ctl, Ctl::Text(ref t) if t.is_empty());
            if r.sub.is_empty() && !stack {
                self.surf.text(tr(&r.label), &self.fonts.label, rf(cx + 18.0, y, text_w + 60.0, rh), fg(0.95 * a));
            } else {
                self.surf.text(tr(&r.label), &self.fonts.label, rf(cx + 18.0, y + 10.0, if stack { cw - 36.0 } else { text_w + 100.0 }, 22.0), fg(0.95 * a));
                if !r.sub.is_empty() {
                    let sub_w = if wide_text { cw - 36.0 } else { (cw - 36.0 - ctl_w - 16.0).max(80.0) };
                    self.surf.text(tr(&r.sub), &self.fonts.sub, rf(cx + 18.0, y + 32.0, sub_w, 18.0), fg(0.55 * a));
                }
            }
            let right = cx + cw - 18.0;
            let mid = if stack { y + rh - 26.0 } else { y + rh / 2.0 };
            match &r.ctl {
                Ctl::Toggle(k) => {
                    let on = c.b(k);
                    let tr = rf(right - 46.0, mid - 12.0, 46.0, 24.0);
                    let t = self.knob(format!("t:{k}"), on);
                    self.draw_switch(tr, t, self.hot == Some(Hit::Toggle(i)), a, &pal);
                    if r.enabled {
                        self.hits.push((Hit::Toggle(i), rf(tr.left - 6.0, tr.top - 6.0, 58.0, 36.0)));
                    }
                }
                Ctl::Choice(k, opts) => {
                    let cur = c.i(k);
                    let spad = if opts.len() >= 5 { 16.0 } else { 26.0 };
                    let ws: Vec<f32> = opts.iter().map(|o| gfx::g().measure(tr(o), &self.fonts.seg) + spad).collect();
                    let total: f32 = ws.iter().sum::<f32>() + 6.0;
                    let mut sx = right - total;
                    self.surf.fill_rr(rf(sx, mid - 16.0, total, 32.0), 9.0, fg(0.07));
                    sx += 3.0;
                    for (j, o) in opts.iter().enumerate() {
                        let r2 = rf(sx, mid - 13.0, ws[j], 26.0);
                        let sel = cur == j as i32;
                        if sel {
                            self.surf.fill_rr_grad(r2, 7.0, with_alpha(accent, a), with_alpha(pal.accent2, a));
                        } else if self.hot == Some(Hit::Seg(i, j as i32)) {
                            self.surf.fill_rr(r2, 7.0, fg(0.08));
                        }
                        self.surf.text(tr(o), &self.fonts.seg, r2, if sel { pal.on(a) } else { fg(0.85 * a) });
                        if r.enabled {
                            self.hits.push((Hit::Seg(i, j as i32), r2));
                        }
                        sx += ws[j];
                    }
                }
                Ctl::Slider(k, min, max, _, unit) => {
                    let v = c.i(k).clamp(*min, *max);
                    let tw = 190.0;
                    let tx = right - tw;
                    let f = (v - min) as f32 / (max - min) as f32;
                    let dragging = self.drag.map(|d| d.0) == Some(i);
                    let hot = self.hot == Some(Hit::Slider(i)) || dragging;
                    // A soft 6 px track; transparency sliders sit on a
                    // checkerboard that shows through more as the value grows.
                    let track = rf(tx, mid - 3.0, tw, 6.0);
                    self.surf.fill_rr(track, 3.0, fg(0.16 * a));
                    if dark {
                        self.surf.fill_rr(rf(tx - 1.5, mid - 4.5, (tw * f).max(6.0) + 3.0, 9.0), 4.5, with_alpha(accent, 0.16 * a));
                    }
                    self.surf.fill_rr(rf(tx, mid - 3.0, (tw * f).max(6.0), 6.0), 3.0, with_alpha(accent, a));
                    // The knob: a soft shadow, white, a little bigger under the pointer.
                    let kx = tx + tw * f;
                    let kr = if dragging { 10.0 } else if hot { 9.5 } else { 8.5 };
                    self.surf.fill_circle(kx, mid + 1.2, kr + 1.2, rgb(0x000000, 0.22 * a));
                    self.surf.fill_circle(kx, mid, kr, rgb(0xffffff, a));
                    self.surf.fill_circle(kx, mid, if dragging { 5.0 } else { 4.0 }, with_alpha(accent, a));
                    let label = if false && v == 0 {
                        tr("Off").to_string()
                    } else if *k == "mascot_sec" {
                        match (v / 60, v % 60) {
                            (0, sec) => format!("{sec} s"),
                            (m, 0) => format!("{m} min"),
                            (m, sec) => format!("{m} min {sec} s"),
                        }
                    } else {
                        format!("{v}{unit}")
                    };
                    self.surf.text(&label, &self.fonts.value, rf(tx - 104.0, mid - 10.0, 92.0, 20.0), fg(if dragging { 0.95 } else { 0.75 } * a));
                    // While dragging, the value also rides on a bubble above the knob.
                    if dragging {
                        let bw = gfx::g().measure(&label, &self.fonts.seg) + 14.0;
                        let br = rf((kx - bw / 2.0).clamp(tx - 6.0, tx + tw + 6.0 - bw), mid - 28.0, bw, 17.0);
                        self.surf.fill_rr(br, 8.5, with_alpha(accent, 0.95));
                        self.surf.text(&label, &self.fonts.seg, br, pal.on(1.0));
                    }
                    if r.enabled {
                        self.hits.push((Hit::Slider(i), rf(tx - 8.0, mid - 14.0, tw + 16.0, 28.0)));
                    }
                }
                Ctl::Edit(k, placeholder) => {
                    let er = rf(right - 260.0, mid - 16.0, 260.0, 32.0);
                    let editing = self.edit.as_ref().map(|e| e.0) == Some(*k);
                    let a2 = if r.enabled { 1.0 } else { 0.4 };
                    let _ = a2;
                    self.surf.fill_rr(er, 8.0, fg(if editing { 0.10 } else { 0.06 }));
                    self.surf.stroke_rr(er, 8.0, if editing { accent } else { fg(0.15) }, if editing { 1.5 } else { 1.0 });
                    let tr = rf(er.left + 10.0, er.top, er.right - er.left - 20.0, 32.0);
                    let mut text = if editing { self.edit.as_ref().unwrap().1.clone() } else { c.s(k) };
                    // The key is a secret: only its ends show unless you're editing it.
                    if *k == "gemini_key" && !editing {
                        text = crate::gemini::masked(text.trim());
                    }
                    if text.is_empty() {
                        self.surf.text(crate::i18n::tr(placeholder), &self.fonts.edit, tr, fg(0.35));
                    }
                    if !text.is_empty() || editing {
                        self.draw_edit_text(&text, tr, mid, editing, fg(0.95), fg(0.95), with_alpha(accent, 0.35));
                    }
                    // Resolved place shown under the label.
                    let key_place = match *k {
                        "weather_city" => "weather_place",
                        "tz2_city" => "tz2_place",
                        _ => "",
                    };
                    let place = if key_place.is_empty() { String::new() } else { c.s(key_place) };
                    if !place.is_empty() && !editing {
                        self.surf.text(&format!("✓ {place}"), &self.fonts.sub, rf(er.left, er.bottom + 1.0, 260.0, 14.0), rgb(0x34c759, 0.9));
                    }
                    if *k == "gemini_key" && !editing {
                        let st = crate::gemini::status();
                        if !st.is_empty() {
                            let col = if st.starts_with('✓') { rgb(0x34c759, 0.95) } else if st.starts_with('✗') { rgb(0xff6b5e, 0.95) } else { fg(0.6) };
                            let sw = gfx::g().measure(&st, &self.fonts.sub).min(cw - 40.0);
                            self.surf.text(&st, &self.fonts.sub, rf(er.right - sw, er.bottom + 1.0, sw, 14.0), col);
                        }
                    }
                    if r.enabled {
                        self.hits.push((Hit::Edit(i), er));
                    }
                }
                Ctl::Page(idx) => {
                    let page = crate::island::ordered_pages().get(*idx).copied();
                    let on = page.map(|p| c.b(p.key())).unwrap_or(false);
                    let tr_ = rf(right - 46.0, mid - 12.0, 46.0, 24.0);
                    let t = self.knob(format!("p:{idx}"), on);
                    self.draw_switch(tr_, t, self.hot == Some(Hit::PageToggle(*idx)), a, &pal);
                    for (j, (d, glyph)) in [(-1, "\u{E70E}"), (1, "\u{E70D}")].iter().enumerate() {
                        let br = rf(right - 120.0 + j as f32 * 34.0, mid - 14.0, 30.0, 28.0);
                        let hot = self.hot == Some(Hit::PageMove(*idx, *d));
                        let last = crate::island::ordered_pages().len().saturating_sub(1);
                        let can = !((*d < 0 && *idx == 0) || (*d > 0 && *idx >= last));
                        let ga = if can { 1.0 } else { 0.3 };
                        self.surf.fill_rr(br, 7.0, fg(if hot && can { 0.16 } else { 0.07 } * a));
                        self.surf.text(glyph, &self.fonts.icon, br, fg(0.85 * ga * a));
                        if r.enabled && can {
                            self.hits.push((Hit::PageMove(*idx, *d), br));
                        }
                    }
                    if r.enabled {
                        self.hits.push((Hit::PageToggle(*idx), rf(tr_.left - 6.0, tr_.top - 6.0, 58.0, 36.0)));
                    }
                }
                Ctl::Metric(idx) => {
                    let Some(&m) = crate::tbmon::order().get(*idx) else { y += rh + 8.0; continue };
                    let on = c.b(&format!("tbm_{}", crate::tbmon::key_of(m)));
                    let tr = rf(right - 46.0, mid - 12.0, 46.0, 24.0);
                    let t = self.knob(format!("m:{idx}"), on);
                    self.draw_switch(tr, t, self.hot == Some(Hit::MetricToggle(*idx)), a, &pal);
                    {
                        let br = rf(right - 154.0, mid - 14.0, 30.0, 28.0);
                        let open = TBM_OPEN.load(Ordering::Relaxed) == *idx as i32;
                        let hot = self.hot == Some(Hit::MetricPad(*idx));
                        self.surf.fill_rr(br, 7.0, if open { with_alpha(accent, a) } else { fg(if hot { 0.16 } else { 0.07 } * a) });
                        self.surf.text("\u{E712}", &self.fonts.icon, br, if open { pal.on(a) } else { fg(0.85 * a) });
                        if r.enabled {
                            self.hits.push((Hit::MetricPad(*idx), br));
                        }
                    }
                    for (j, (d, glyph)) in [(-1, "\u{E70E}"), (1, "\u{E70D}")].iter().enumerate() {
                        let br = rf(right - 120.0 + j as f32 * 34.0, mid - 14.0, 30.0, 28.0);
                        let hot = self.hot == Some(Hit::MetricMove(*idx, *d));
                        let last = crate::tbmon::order().len().saturating_sub(1);
                        let can = !((*d < 0 && *idx == 0) || (*d > 0 && *idx >= last));
                        let ga = if can { 1.0 } else { 0.3 };
                        self.surf.fill_rr(br, 7.0, fg(if hot && can { 0.16 } else { 0.07 } * a));
                        // (Segoe Fluent Icons, like the page arrows; the text font has no arrows.)
                        self.surf.text(glyph, &self.fonts.icon, br, fg(0.85 * ga * a));
                        if r.enabled && can {
                            self.hits.push((Hit::MetricMove(*idx, *d), br));
                        }
                    }
                    if r.enabled {
                        self.hits.push((Hit::MetricToggle(*idx), rf(tr.left - 6.0, tr.top - 6.0, 58.0, 36.0)));
                    }
                }
                Ctl::Button(label, _) => {
                    let bw = gfx::g().measure(tr(label), &self.fonts.seg) + 32.0;
                    let br = rf(right - bw, mid - 15.0, bw, 30.0);
                    let hot = self.hot == Some(Hit::Button(i));
                    if hot {
                        self.surf.fill_rr_grad(br, 8.0, with_alpha(accent, a), with_alpha(pal.accent2, a));
                    } else {
                        self.surf.fill_rr(br, 8.0, fg(0.07 * a));
                        self.surf.stroke_rr(rf(br.left + 0.5, br.top + 0.5, br.right - br.left - 1.0, br.bottom - br.top - 1.0), 7.5, with_alpha(accent, 0.35 * a), 1.0);
                    }
                    self.surf.text(tr(label), &self.fonts.seg, br, if hot { pal.on(a) } else { fg(0.9 * a) });
                    if r.enabled {
                        self.hits.push((Hit::Button(i), br));
                    }
                }
                Ctl::Text(t) => {
                    if !t.is_empty() {
                        self.surf.text(t, &self.fonts.value, rf(right - 220.0, mid - 11.0, 220.0, 22.0), fg(0.9));
                    }
                }
                Ctl::Switch(_, on) => {
                    let on = *on;
                    let tr = rf(right - 46.0, mid - 12.0, 46.0, 24.0);
                    let t = self.knob(format!("s:{}", r.label), on);
                    self.draw_switch(tr, t, self.hot == Some(Hit::Switch(i)), a, &pal);
                    if r.enabled {
                        self.hits.push((Hit::Switch(i), rf(tr.left - 6.0, tr.top - 6.0, 58.0, 36.0)));
                    }
                }
            }
            y += rh + 8.0;
        }
        self.content_h = y + self.scroll - 78.0 + 20.0 - (1.0 - page_e) * 18.0;
        if page_e < 1.0 && part.is_none() {
            self.surf.fill_rect(rf(SIDE, 64.0, w - SIDE, h - 64.0), with_alpha(bg, 1.0 - page_e));
        }
        if part.is_some() {
            // The row's hits were already there.
            self.hits.truncate(n_hits);
            self.surf.unclip();
            self.surf.unclip();
            self.surf.end();
            return;
        }
        self.surf.unclip();

        self.surf.fill_rect(rf(SIDE, 0.0, w - SIDE, 64.0), bg);
        let searching = !self.query.trim().is_empty();
        let title = if searching { tr("Search results").to_string() } else { tr(TABS[self.tab].0).to_string() };
        self.surf.text(&title, &self.fonts.title, rf(cx + 2.0, 18.0, cw, 36.0), fg(1.0));
        if searching && rows.is_empty() {
            self.surf.text(tr("No settings match"), &self.fonts.label, rf(cx + 4.0, 90.0, cw, 24.0), fg(0.5));
        }

        // The sidebar: a little darker, the logo's black glass.
        self.surf.fill_rr_grad(rf(0.0, 0.0, SIDE, h), 0.0, pal.side_top, pal.side_bot);
        self.surf.fill_rect(rf(SIDE - 1.0, 0.0, 1.0, h), if dark { rgb(0x22c8ff, 0.08) } else { rgb(0x0d141b, 0.07) });
        gfx::draw_logo(&self.surf, 20.0, 21.0, 36.0, 1.0, 1.0);
        self.surf.text("Phemio", &self.fonts.brand, rf(64.0, 18.0, 140.0, 30.0), fg(1.0));
        self.surf.text(tr("Your desktop cameo"), &self.fonts.brand_sub, rf(65.0, 45.0, SIDE - 70.0, 16.0), with_alpha(accent, if dark { 0.9 } else { 1.0 }));
        // Search box: settings from every tab.
        {
            let sr = rf(14.0, 80.0, SIDE - 28.0, 32.0);
            let editing = self.edit.as_ref().map(|e| e.0) == Some("__search");
            self.surf.fill_rr(sr, 8.0, fg(if editing { 0.10 } else { 0.06 }));
            self.surf.stroke_rr(sr, 8.0, if editing { accent } else { fg(0.12) }, if editing { 1.5 } else { 1.0 });
            self.surf.text("\u{E721}", &self.fonts.icon, rf(sr.left + 6.0, sr.top, 22.0, 32.0), fg(0.55));
            let text = if editing { self.edit.as_ref().map(|e| e.1.clone()).unwrap_or_default() } else { self.query.clone() };
            let tr_ = rf(sr.left + 32.0, sr.top, sr.right - sr.left - 58.0, 32.0);
            if text.is_empty() {
                self.surf.text(tr("Search settings"), &self.fonts.edit, tr_, fg(0.38));
            }
            if !text.is_empty() || editing {
                self.draw_edit_text(&text, tr_, sr.top + 16.0, editing, fg(0.95), fg(0.95), with_alpha(accent, 0.35));
            }
            self.hits.push((Hit::Search, sr));
            if !text.is_empty() {
                let xr = rf(sr.right - 26.0, sr.top + 5.0, 22.0, 22.0);
                if self.hot == Some(Hit::SearchClear) {
                    self.surf.fill_rr(xr, 6.0, fg(0.12));
                }
                self.surf.text("\u{E711}", &self.fonts.icon, xr, fg(0.7));
                self.hits.push((Hit::SearchClear, xr));
            }
        }
        // The chosen tab's pill glides from tab to tab.
        if !searching {
            let pr = rf(12.0, self.anim.tab_y, SIDE - 24.0, TAB_H);
            self.surf.fill_rr(pr, 9.0, with_alpha(accent, if dark { 0.13 } else { 0.12 }));
            self.surf.stroke_rr(rf(pr.left + 0.5, pr.top + 0.5, pr.right - pr.left - 1.0, TAB_H - 1.0), 8.5, with_alpha(accent, if dark { 0.22 } else { 0.25 }), 1.0);
            if dark {
                self.surf.fill_rr(rf(pr.left - 1.0, pr.top + 7.0, 5.0, TAB_H - 14.0), 2.5, with_alpha(accent, 0.3));
            }
            self.surf.fill_rr_grad(rf(pr.left + 0.5, pr.top + 9.0, 3.0, TAB_H - 18.0), 1.5, accent, pal.accent2);
        }
        // Icon hues: the logo's neon, from cyan to blue.
        const TAB_COLORS: [u32; 7] = [0x22c8ff, 0x3fb8ff, 0x2fd8e8, 0x5aa0ff, 0x36e0c8, 0x7cc4ff, 0x4f8cff];
        for (t, (name, glyph)) in TABS.iter().enumerate() {
            let r = rf(12.0, TAB_Y0 + t as f32 * TAB_STEP, SIDE - 24.0, TAB_H);
            let sel = t == self.tab && !searching;
            let hot = self.anim.tab_hot.get(t).copied().unwrap_or(0.0);
            if !sel && hot > 0.01 {
                self.surf.fill_rr(r, 9.0, fg(0.055 * hot));
            }
            let gc = if sel {
                accent
            } else if c.i("icon_colors") == 0 {
                rgb(if dark { TAB_COLORS[t % 7] } else { crate::palette::themed(TAB_COLORS[t % 7]) }, 0.9)
            } else {
                fg(0.7)
            };
            self.surf.text(glyph, &self.fonts.icon, rf(r.left + 10.0, r.top, 26.0, TAB_H), gc);
            // A long name (some languages): a size smaller so it fits.
            let name = tr(name);
            let room = r.right - r.left - 50.0;
            let f = if gfx::g().measure(name, &self.fonts.tab) > room { &self.fonts.tab_small } else { &self.fonts.tab };
            self.surf.text(name, f, rf(r.left + 42.0, r.top, room + 4.0, TAB_H), fg(if sel { 1.0 } else { 0.78 }));
            self.hits.push((Hit::Tab(t), r));
        }
        self.surf.text(&format!("v{}", env!("CARGO_PKG_VERSION")), &self.fonts.brand_sub, rf(22.0, h - 30.0, SIDE - 40.0, 16.0), fg(0.32));
        self.surf.end();
    }
}
