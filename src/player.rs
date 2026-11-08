//! Bring the app that's playing to the front (Now Playing's "open" button and
//! its cover). The media session names the app by its AppUserModelID:
//! "Spotify.exe", "SpotifyAB.SpotifyMusic_…!Spotify", "Chrome._crx_…" (a web
//! app such as YouTube Music), "MSEdge"… The window to show is found by that
//! ID, else by the program it belongs to; failing both, Windows starts the
//! app by its ID.

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::WindowsAndMessaging::*;

/// PKEY_AppUserModel_ID.
const PKEY_AUMID: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3), pid: 5 };

fn window_aumid(h: HWND) -> String {
    unsafe {
        let Ok(store) = SHGetPropertyStoreForWindow::<IPropertyStore>(h) else { return String::new() };
        let Ok(v) = store.GetValue(&PKEY_AUMID) else { return String::new() };
        match PropVariantToStringAlloc(&v) {
            Ok(p) => {
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as *const _));
                s
            }
            Err(_) => String::new(),
        }
    }
}

fn title(h: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// How well a window fits the app (0 = not at all).
fn score(h: HWND, id: &str, song: &str) -> f32 {
    let idl = id.to_lowercase();
    let head = idl.split('!').next().unwrap_or("");
    let after_bang = idl.split('!').nth(1).unwrap_or("");
    let first = idl.split('.').next().unwrap_or("");
    let mut s = 0.0;
    let aumid = window_aumid(h).to_lowercase();
    if !aumid.is_empty() && aumid == idl {
        s = 3.0;
    } else {
        let exe = crate::util::process_name(h);
        let stem = exe.strip_suffix(".exe").unwrap_or(&exe).to_string();
        if !stem.is_empty() {
            if stem == first || stem == after_bang || exe == idl {
                s = 2.0;
            } else if stem.len() >= 4 && head.contains(&stem) {
                s = 1.5;
            }
        }
    }
    // Several windows (a browser): the one showing the song.
    if s > 0.0 && !song.is_empty() && title(h).to_lowercase().contains(&song.to_lowercase()) {
        s += 0.5;
    }
    s
}

struct Search {
    id: String,
    song: String,
    best: Option<(f32, HWND)>,
}

unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> BOOL {
    let st = &mut *(lp.0 as *mut Search);
    if !IsWindowVisible(h).as_bool() && !IsIconic(h).as_bool() {
        return TRUE;
    }
    if GetWindow(h, GW_OWNER).map(|o| !o.is_invalid()).unwrap_or(false) || GetWindowTextLengthW(h) == 0 || crate::util::is_cloaked(h) && !IsIconic(h).as_bool() {
        return TRUE;
    }
    let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
    if ex & WS_EX_TOOLWINDOW.0 != 0 {
        return TRUE;
    }
    let sc = score(h, &st.id, &st.song);
    if sc > 0.0 && st.best.map(|b| sc > b.0).unwrap_or(true) {
        st.best = Some((sc, h));
    }
    TRUE
}

/// Show the window (restoring it if minimised) even though the island
/// itself never takes the focus.
fn bring_front(h: HWND) {
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        // Windows only lets the app with the latest input pick the foreground
        // window; a lone Alt tap counts as that.
        let k = |up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VK_MENU, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, ..Default::default() } },
        };
        SendInput(&[k(false), k(true)], std::mem::size_of::<INPUT>() as i32);
        let _ = SetForegroundWindow(h);
        let _ = BringWindowToTop(h);
    }
}

/// Bring the playing app forward; `song` helps pick among a browser's windows.
pub fn open(app_id: &str, song: &str) {
    if app_id.is_empty() {
        return;
    }
    let mut st = Search { id: app_id.to_string(), song: song.to_string(), best: None };
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut st as *mut Search as isize));
    }
    crate::util::log(&format!("player: open {app_id} → {}", st.best.map(|b| format!("window ({:.1})", b.0)).unwrap_or_else(|| "no window, start it".into())));
    match st.best {
        Some((_, h)) => bring_front(h),
        // No window (closed to the tray): ask Windows to start it by its ID.
        None => crate::util::shell_open(&format!("shell:AppsFolder\\{app_id}")),
    }
}

/// A short name for the app ("Spotify", "Chrome", "Apple Music"…), or
/// None when the ID says nothing readable (a web app's random ID).
pub fn name(app_id: &str) -> Option<String> {
    let l = app_id.to_lowercase();
    const KNOWN: [(&str, &str); 19] = [
        ("spotify", "Spotify"),
        ("applemusic", "Apple Music"),
        ("itunes", "iTunes"),
        ("zunemusic", "Media Player"),
        ("tidal", "TIDAL"),
        ("deezer", "Deezer"),
        ("amazonmusic", "Amazon Music"),
        ("vlc", "VLC"),
        ("foobar", "foobar2000"),
        ("aimp", "AIMP"),
        ("musicbee", "MusicBee"),
        ("youtube", "YouTube Music"),
        ("msedge", "Edge"),
        ("chrome", "Chrome"),
        ("brave", "Brave"),
        ("firefox", "Firefox"),
        ("opera", "Opera"),
        ("vivaldi", "Vivaldi"),
        ("discord", "Discord"),
    ];
    // A web app ("Chrome._crx_…") is named by its window rather than its browser.
    if l.contains("_crx_") {
        return None;
    }
    if let Some((_, n)) = KNOWN.iter().find(|(k, _)| l.contains(k)) {
        return Some(n.to_string());
    }
    let tail = l.split('!').nth(1).filter(|t| *t != "app").unwrap_or_else(|| l.split('!').next().unwrap_or(""));
    let base = tail.strip_suffix(".exe").unwrap_or(tail);
    let base = base.rsplit('.').next().unwrap_or(base).split('_').next().unwrap_or(base);
    if base.is_empty() || base.len() > 24 {
        return None;
    }
    let mut c = base.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str())
}
