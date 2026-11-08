//! Assorted Win32 helpers: strings, monitors, theme, time, clipboard, memory.

use std::ffi::c_void;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Globalization::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::ProcessStatus::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::SystemInformation::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

pub fn hinstance() -> HINSTANCE {
    unsafe { GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default() }
}

/// The icon embedded in the exe (resource 1) at the system's large or small
/// icon size — the same picture Explorer and Task Manager show.
pub fn app_icon(small: bool) -> HICON {
    unsafe {
        let (cx, cy) = if small { (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON)) } else { (GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CYICON)) };
        match LoadImageW(Some(hinstance()), PCWSTR(1 as *const u16), IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR) {
            Ok(h) if !h.is_invalid() => HICON(h.0),
            _ => crate::gfx::make_app_icon(if small { 16 } else { 32 }),
        }
    }
}

/// One-time move from the old names (Islet, Marisland, then DewDrop): an
/// old copy still running is asked to quit, its data folders become
/// Phemio's, and the autostart entry is renamed.
pub fn migrate_from_islet() {
    unsafe {
        // Two islands at once would be a mess: the old app quits first (it
        // saves its settings and puts the taskbar etc. back as it goes).
        for class in [w!("DewDropHost"), w!("MarislandHost")] {
            if let Ok(old) = FindWindowW(class, PCWSTR::null()) {
                let _ = PostMessageW(Some(old), WM_CLOSE, WPARAM(0), LPARAM(0));
                for _ in 0..40 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    if FindWindowW(class, PCWSTR::null()).is_err() {
                        break;
                    }
                }
                log("asked the old copy (DewDrop/Marisland) to quit — Phemio takes over");
            }
        }
    }
    let new = crate::config::dir();
    for (var, sub) in [("APPDATA", "DewDrop"), ("LOCALAPPDATA", "DewDrop"), ("APPDATA", "Marisland"), ("LOCALAPPDATA", "Marisland"), ("APPDATA", "Islet")] {
        let Ok(base) = std::env::var(var) else { continue };
        let old = std::path::PathBuf::from(&base).join(sub);
        let new = std::path::PathBuf::from(&base).join("Phemio");
        if !old.is_dir() || new.join("config.ini").exists() || new.join("contentindex.db").exists() {
            continue;
        }
        // The whole folder at once; if something in it is still in use
        // (Explorer may hold the old transparency helper), file by file.
        if !new.exists() && std::fs::rename(&old, &new).is_ok() {
            for l in ["dewdrop.log", "marisland.log"] {
                let _ = std::fs::rename(new.join(l), new.join("phemio.log.old"));
            }
            log(&format!("moved {} over from {sub}", new.display()));
            continue;
        }
        let _ = std::fs::create_dir_all(&new);
        fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
            let Ok(rd) = std::fs::read_dir(from) else { return };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name == "crash.log" || name.ends_with(".log") || name.ends_with(".log.old") || name == "glass" || name.ends_with(".tmp") {
                    continue;
                }
                let (src, dst) = (e.path(), to.join(&name));
                if src.is_dir() {
                    let _ = std::fs::create_dir_all(&dst);
                    copy_dir(&src, &dst);
                } else if !dst.exists() {
                    let _ = std::fs::copy(&src, &dst);
                }
            }
        }
        copy_dir(&old, &new);
        log(&format!("copied settings and data over from {sub}"));
    }
    let _ = std::fs::create_dir_all(&new);
    unsafe {
        let mut had_old = false;
        for name in [w!("Islet"), w!("Marisland"), w!("DewDrop")] {
            let mut sz = 0u32;
            if RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, name, RRF_RT_REG_SZ, None, None, Some(&mut sz)) == ERROR_SUCCESS {
                let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, name);
                had_old = true;
            }
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run"), name);
        }
        if had_old {
            set_autostart(true);
        }
    }
}

pub fn register_class(name: PCWSTR, proc_: WNDPROC, icon: HICON) {
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: proc_,
            hInstance: hinstance(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: icon,
            hIconSm: app_icon(true),
            lpszClassName: name,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
}

pub fn lo(l: LPARAM) -> i32 {
    (l.0 & 0xffff) as i16 as i32
}
pub fn hi(l: LPARAM) -> i32 {
    ((l.0 >> 16) & 0xffff) as i16 as i32
}

// ---------------------------------------------------------------------------
// Monitors / DPI
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
pub struct Monitor {
    pub full: RECT,
    pub work: RECT,
    pub scale: f32,
}

pub fn primary_monitor() -> Monitor {
    unsafe {
        let hm = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(hm, &mut mi);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(hm, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        Monitor { full: mi.rcMonitor, work: mi.rcWork, scale: dx as f32 / 96.0 }
    }
}

/// The main taskbar's rectangle (screen coordinates).
pub fn taskbar_rect() -> Option<(HWND, RECT)> {
    unsafe {
        let tb = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()).ok()?;
        let mut r = RECT::default();
        GetWindowRect(tb, &mut r).ok()?;
        (r.right > r.left && r.bottom > r.top).then_some((tb, r))
    }
}

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

const PERSONALIZE: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

fn reg_dword(sub: PCWSTR, name: PCWSTR) -> Option<u32> {
    unsafe {
        let mut v = 0u32;
        let mut sz = 4u32;
        let r = RegGetValueW(HKEY_CURRENT_USER, sub, name, RRF_RT_REG_DWORD, None, Some(&mut v as *mut u32 as *mut c_void), Some(&mut sz));
        (r == ERROR_SUCCESS).then_some(v)
    }
}

/// true = dark. `pref`: 0 follow Windows, 1 dark, 2 light.
pub fn is_dark(pref: i32) -> bool {
    match pref {
        1 => true,
        2 => false,
        _ => reg_dword(PERSONALIZE, w!("AppsUseLightTheme")).map(|v| v == 0).unwrap_or(true),
    }
}

/// Is the taskbar dark? (SystemUsesLightTheme, separate from apps' theme)
pub fn is_dark_taskbar() -> bool {
    reg_dword(PERSONALIZE, w!("SystemUsesLightTheme")).map(|v| v == 0).unwrap_or(true)
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// `d` ago — or the start of time as Rust knows it, if the PC hasn't been on
/// that long (just after a restart, `Instant::now() - d` would crash: Windows
/// counts instants from when it started).
pub fn ago(d: std::time::Duration) -> std::time::Instant {
    let now = std::time::Instant::now();
    now.checked_sub(d).unwrap_or(now)
}

pub fn now() -> SYSTEMTIME {
    unsafe { GetLocalTime() }
}

fn st_to_secs(st: &SYSTEMTIME) -> i64 {
    days_from_civil(st.wYear as i64, st.wMonth as i64, st.wDay as i64) * 86400
        + st.wHour as i64 * 3600
        + st.wMinute as i64 * 60
        + st.wSecond as i64
}

/// Seconds since 1970 (UTC).
pub fn unix_now() -> i64 {
    unsafe { st_to_secs(&GetSystemTime()) }
}

/// Current local UTC offset in seconds (includes daylight saving).
pub fn local_offset() -> i64 {
    unsafe {
        let l = GetLocalTime();
        let u = GetSystemTime();
        let d = st_to_secs(&l) - st_to_secs(&u);
        // round to the nearest quarter hour to cancel the tick between the two calls
        ((d as f64 / 900.0).round() * 900.0) as i64
    }
}

/// Local wall-clock seconds-since-epoch for a unix time.
pub fn local_secs(unix: i64) -> i64 {
    unix + local_offset()
}

/// Local day number (days since 1970-01-01 in local time).
pub fn local_day(unix: i64) -> i64 {
    local_secs(unix).div_euclid(86400)
}

/// Unix time of today's (local) hh:mm.
pub fn today_at(h: u32, m: u32) -> i64 {
    let day = local_day(unix_now());
    day * 86400 + h as i64 * 3600 + m as i64 * 60 - local_offset()
}

/// Howard Hinnant's days_from_civil.
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// SYSTEMTIME for "wall clock seconds" (unix + some offset).
pub fn systime(wall: i64) -> SYSTEMTIME {
    let days = wall.div_euclid(86400);
    let rem = wall.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    SYSTEMTIME {
        wYear: y as u16,
        wMonth: m as u16,
        wDayOfWeek: ((days + 4).rem_euclid(7)) as u16,
        wDay: d as u16,
        wHour: (rem / 3600) as u16,
        wMinute: ((rem % 3600) / 60) as u16,
        wSecond: (rem % 60) as u16,
        wMilliseconds: 0,
    }
}

pub fn date_fmt(st: &SYSTEMTIME, fmt: &str) -> String {
    unsafe {
        let f = wide(fmt);
        let mut buf = [0u16; 64];
        // Day and month names in the island's language.
        let l = wide(crate::i18n::locale());
        let loc = PCWSTR(l.as_ptr());
        let n = GetDateFormatEx(loc, ENUM_DATE_FORMATS_FLAGS(0), Some(st), PCWSTR(f.as_ptr()), Some(&mut buf), PCWSTR::null());
        if n > 0 {
            from_wide(&buf)
        } else {
            String::new()
        }
    }
}

fn system_24h() -> bool {
    unsafe {
        let mut buf = [0u16; 8];
        let n = GetLocaleInfoEx(PCWSTR::null(), LOCALE_ITIME, Some(&mut buf));
        n > 0 && buf[0] == b'1' as u16
    }
}

pub fn is_24h(fmt: i32) -> bool {
    match fmt {
        1 => false,
        2 => true,
        _ => system_24h(),
    }
}

/// ("3:26", "PM") – suffix empty in 24-hour mode.
pub fn clock_parts(st: &SYSTEMTIME, fmt: i32) -> (String, String) {
    if is_24h(fmt) {
        (format!("{:02}:{:02}", st.wHour, st.wMinute), String::new())
    } else {
        let h = match st.wHour % 12 {
            0 => 12,
            h => h,
        };
        (format!("{}:{:02}", h, st.wMinute), if st.wHour < 12 { "AM".into() } else { "PM".into() })
    }
}

/// "14:05:09" / "2:05:09 PM".
pub fn clock_string_secs(st: &SYSTEMTIME, fmt: i32) -> String {
    let (t, s) = clock_parts(st, fmt);
    let t = format!("{t}:{:02}", st.wSecond);
    if s.is_empty() {
        t
    } else {
        format!("{t} {s}")
    }
}

pub fn clock_string(st: &SYSTEMTIME, fmt: i32) -> String {
    let (t, s) = clock_parts(st, fmt);
    if s.is_empty() {
        t
    } else {
        format!("{t} {s}")
    }
}

pub fn short_day(st: &SYSTEMTIME) -> String {
    date_fmt(st, "ddd")
}

/// "UTC+7", "UTC+5:30", "UTC−3"
pub fn utc_label(offset_secs: i64) -> String {
    let sign = if offset_secs < 0 { "−" } else { "+" };
    let a = offset_secs.abs();
    let (h, m) = (a / 3600, (a % 3600) / 60);
    if m == 0 {
        format!("UTC{sign}{h}")
    } else {
        format!("UTC{sign}{h}:{m:02}")
    }
}

/// Milliseconds until the next minute boundary (+ a little slack).
pub fn ms_to_next_minute() -> u32 {
    let st = now();
    60_000 - (st.wSecond as u32 * 1000 + st.wMilliseconds as u32) + 50
}

/// Parses "14:30", "2pm", "2:30 pm", "9" → (hour, minute).
pub fn parse_hm(s: &str) -> Option<(u32, u32)> {
    let s = s.trim().to_lowercase().replace(' ', "").replace('.', ":").replace('h', ":");
    let (body, pm, am) = if let Some(b) = s.strip_suffix("pm") {
        (b.to_string(), true, false)
    } else if let Some(b) = s.strip_suffix("am") {
        (b.to_string(), false, true)
    } else {
        (s.clone(), false, false)
    };
    let body = body.trim_end_matches(':');
    let (h, m) = match body.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, if m.is_empty() { 0 } else { m.parse::<u32>().ok()? }),
        None => (body.parse::<u32>().ok()?, 0),
    };
    let mut h = h;
    if pm && h < 12 {
        h += 12;
    }
    if am && h == 12 {
        h = 0;
    }
    (h < 24 && m < 60).then_some((h, m))
}

// ---------------------------------------------------------------------------
// Misc
// ---------------------------------------------------------------------------

pub fn url_encode(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

pub fn exe_path() -> String {
    std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn set_autostart(on: bool) {
    unsafe {
        // The app used to be called Islet, then Marisland; never leave their
        // old entries behind.
        let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Islet"));
        let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Marisland"));
        if on {
            // Written again at every start, so it follows the exe if it moved.
            let v = wide(&format!("\"{}\" --autostart", exe_path()));
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Phemio"), REG_SZ.0, Some(v.as_ptr() as *const c_void), (v.len() * 2) as u32);
            // Switched off in Task Manager's Startup apps (or by a cleaner)?
            // The setting in Phemio is what counts: enabled again.
            let approved: [u8; 12] = [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run"),
                w!("Phemio"),
                REG_BINARY.0,
                Some(approved.as_ptr() as *const c_void),
                approved.len() as u32,
            );
            // Windows holds sign-in apps back for ~10 s (and until it's
            // idle) after the desktop shows: no wait, for this user.
            let zero = 0u32;
            for name in [w!("StartupDelayInMSec"), w!("WaitForIdleState")] {
                let _ = RegSetKeyValueW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Serialize"), name, REG_DWORD.0, Some(&zero as *const u32 as *const c_void), 4);
            }
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Phemio"));
        }
    }
}

pub fn is_cloaked(hwnd: HWND) -> bool {
    let mut v = 0u32;
    unsafe { DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut v as *mut u32 as *mut c_void, 4).is_ok() && v != 0 }
}

/// Lower-case exe file name of the process that owns a window ("explorer.exe").
pub fn process_name(hwnd: HWND) -> String {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return String::new();
        }
        let p = String::from_utf16_lossy(&buf[..len as usize]);
        p.rsplit('\\').next().unwrap_or("").to_lowercase()
    }
}

pub fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn clipboard_text() -> Option<String> {
    unsafe {
        OpenClipboard(None).ok()?;
        let mut out = None;
        if let Ok(h) = GetClipboardData(13u32) {
            let p = GlobalLock(HGLOBAL(h.0)) as *const u16;
            if !p.is_null() {
                let mut n = 0;
                while *p.add(n) != 0 && n < 100_000 {
                    n += 1;
                }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
                let _ = GlobalUnlock(HGLOBAL(h.0));
            }
        }
        let _ = CloseClipboard();
        out
    }
}

pub fn set_clipboard_text(t: &str) {
    use windows::Win32::System::Memory::{GlobalAlloc, GMEM_MOVEABLE};
    unsafe {
        if OpenClipboard(None).is_err() {
            return;
        }
        let _ = windows::Win32::System::DataExchange::EmptyClipboard();
        let w: Vec<u16> = t.encode_utf16().chain(std::iter::once(0)).collect();
        if let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2) {
            let p = GlobalLock(h) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                let _ = GlobalUnlock(h);
                let _ = windows::Win32::System::DataExchange::SetClipboardData(13u32, Some(HANDLE(h.0)));
            }
        }
        let _ = CloseClipboard();
    }
}

/// Open the folder `path` is in, with it selected (File Explorer).
pub fn reveal(path: &str) {
    unsafe {
        let arg = wide(&format!("/select,\"{path}\""));
        windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), w!("explorer.exe"), PCWSTR(arg.as_ptr()), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

pub fn shell_open(path: &str) {
    unsafe {
        let w = wide(path);
        windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), PCWSTR(w.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// 0.32 could move other apps' notifications (removed in 0.33): put back
/// any Windows banner it had switched off, and delete what it left behind.
pub fn cleanup_notifications_feature() {
    let ini = crate::config::dir().join("config.ini");
    let saved = std::fs::read_to_string(&ini)
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("notif_banners_saved=").map(|v| v.trim().to_string())))
        .unwrap_or_default();
    for e in saved.split(';') {
        let Some((app, v)) = e.rsplit_once('=') else { continue };
        let Ok(v) = v.parse::<i64>() else { continue };
        unsafe {
            let k = wide(&format!("Software\\Microsoft\\Windows\\CurrentVersion\\Notifications\\Settings\\{app}"));
            let name = wide("ShowBanner");
            let mut hk = HKEY::default();
            if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(k.as_ptr()), Some(0), KEY_SET_VALUE, &mut hk).is_ok() {
                if v < 0 {
                    let _ = RegDeleteValueW(hk, PCWSTR(name.as_ptr()));
                } else {
                    let d = (v as u32).to_le_bytes();
                    let _ = RegSetValueExW(hk, PCWSTR(name.as_ptr()), Some(0), REG_DWORD, Some(&d));
                }
                let _ = RegCloseKey(hk);
            }
        }
    }
    if !saved.is_empty() {
        log("notifications feature removed: Windows' banners put back");
        // Drop the line so this runs once (the next save rewrites the file anyway).
        if let Ok(t) = std::fs::read_to_string(&ini) {
            let kept: Vec<&str> = t.lines().filter(|l| !l.starts_with("notif_")).collect();
            let _ = std::fs::write(&ini, kept.join("\n") + "\n");
        }
    }
    let _ = std::fs::remove_dir_all(crate::config::dir().join("pkg"));
    if let Some(d) = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())) {
        let _ = std::fs::remove_file(d.join("Phemio.logo.png"));
    }
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("marisland-wpn"));
}

/// (private working set — what Task Manager shows —, working set, committed
/// private bytes) in MB.
pub fn memory_mb() -> (f32, f32, f32) {
    unsafe {
        let mut c = PROCESS_MEMORY_COUNTERS_EX2::default();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32;
        let mb = |b: usize| b as f32 / 1048576.0;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, c.cb).is_ok() {
            // Counted page by page when Windows doesn't say (what Task Manager shows).
            let pws = private_ws_bytes().or((c.PrivateWorkingSetSize > 0).then_some(c.PrivateWorkingSetSize)).unwrap_or(c.WorkingSetSize);
            return (mb(pws), mb(c.WorkingSetSize), mb(c.PrivateUsage));
        }
        let mut c = PROCESS_MEMORY_COUNTERS_EX::default();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, c.cb).is_ok() {
            (mb(private_ws_bytes().unwrap_or(c.WorkingSetSize)), mb(c.WorkingSetSize), mb(c.PrivateUsage))
        } else {
            (0.0, 0.0, 0.0)
        }
    }
}

/// The private working set — pages in RAM that belong to this process only
/// (Task Manager's "Memory" column) — counted from the working set list.
fn private_ws_bytes() -> Option<usize> {
    unsafe {
        let mut entries = 32 * 1024usize;
        for _ in 0..4 {
            // [count, entry, entry, …], one usize each.
            let mut buf = vec![0usize; entries + 1];
            let r = QueryWorkingSet(GetCurrentProcess(), buf.as_mut_ptr() as *mut _, (buf.len() * std::mem::size_of::<usize>()) as u32);
            let n = buf[0];
            if r.is_ok() && n <= entries {
                // Bit 8: the page is shared (DLL code, mapped files…).
                let private = buf[1..=n].iter().filter(|e| *e & (1 << 8) == 0).count();
                return Some(private * 4096);
            }
            entries = n.max(entries) + 4096;
        }
        None
    }
}

/// Give back pages that were only needed during start-up / settings.
pub fn trim_memory() {
    unsafe {
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

/// Developer aid: ISLET_WINE=1 when testing under Wine, where a few system
/// watchers are stubs. (Set by hand — the app never probes what it runs on.)
pub fn is_wine() -> bool {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *W.get_or_init(|| std::env::var_os("ISLET_WINE").is_some())
}

/// Tiny xorshift RNG (mascot glances, blink timing).
pub fn rand_u32() -> u32 {
    use std::cell::Cell;
    thread_local!(static S: Cell<u32> = Cell::new(0));
    S.with(|s| {
        let mut x = s.get();
        if x == 0 {
            x = (unix_now() as u32) ^ 0x9E37_79B9 | 1;
        }
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        s.set(x);
        // Mix the output so the low bits (used with %) are well spread.
        let mut z = x.wrapping_mul(0x9E37_79B1);
        z ^= z >> 16;
        z = z.wrapping_mul(0x85EB_CA6B);
        z ^ (z >> 13)
    })
}

/// Append a line to phemio.log in the data folder (kept small). Used for the
/// few decisions worth knowing about afterwards, like hiding the island.
pub fn log(msg: &str) {
    use std::io::Write;
    let path = crate::config::dir().join("phemio.log");
    if std::fs::metadata(&path).map(|m| m.len() > 128 * 1024).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }
    let st = now();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{:04}-{:02}-{:02} {:02}:{:02}:{:02}  {msg}", st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond);
    }
}

/// Press Win+<key> (opens Quick Settings, Notifications, Cast …).
pub fn send_win_key(key: u8) {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    let k = |vk: u16, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, time: 0, dwExtraInfo: 0 },
        },
    };
    let inputs = [k(VK_LWIN.0, false), k(key as u16, false), k(key as u16, true), k(VK_LWIN.0, true)];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Windows' folder picker; the folder chosen, if any.
pub fn pick_folder(owner: HWND) -> Option<String> {
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Shell::*;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let o = d.GetOptions().ok()?;
        d.SetOptions(o | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
        d.Show(Some(owner)).ok()?;
        let item = d.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

/// Developer aid: ISLET_PROFILE=1 logs how long the named step took (ms).
pub struct Prof(&'static str, std::time::Instant);
impl Prof {
    pub fn new(what: &'static str) -> Option<Prof> {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        ON.get_or_init(|| std::env::var("ISLET_PROFILE").is_ok()).then(|| Prof(what, std::time::Instant::now()))
    }
}
impl Drop for Prof {
    fn drop(&mut self) {
        log(&format!("prof {} {:.2} ms", self.0, self.1.elapsed().as_secs_f64() * 1000.0));
    }
}
