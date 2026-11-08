//! Transparency (Settings → Transparency): see-through backgrounds for the
//! taskbar and File Explorer, with a level slider each, all reversible.
//!
//! The work happens inside Explorer, in a small helper DLL (`mglass/`, built
//! alongside and packed into Phemio.exe). Phemio writes the helper to
//! its data folder, loads it into Explorer with a one-off WH_GETMESSAGE hook
//! (the helper then keeps itself loaded and the hook is removed), and talks
//! to it through a small shared memory block plus a "settings changed"
//! message. The helper puts everything back when Phemio quits or crashes.
//!
//! Safety net: if Explorer restarts within a minute of switching this on
//! (a crash we may have caused), transparency is switched off by itself.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::System::Memory::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config;
use crate::util;

/// The helper, built from `mglass/` before Phemio (build.rs finds it).
static DLL: &[u8] = include_bytes!(env!("PHEMIO_MGLASS_DLL"));

// Keep in sync with mglass/src/lib.rs.
#[repr(C)]
struct Shared {
    magic: u32,
    proto: u32,
    host_pid: u32,
    gen: u32,
    tb_on: u32,
    tb_lvl: u32,
    ex_on: u32,
    ex_lvl: u32,
    look: u32,
    tb_state: u32,
    ex_state: u32,
    ex_mode: u32,
    dll: [u16; 300],
    log: [u16; 300],
}

const MAGIC: u32 = 0x4D47_4C53;

pub const T_GLASS: usize = 31;

struct State {
    view: isize,
    hmod: isize,
    /// Explorer processes the helper went into.
    injected: Vec<u32>,
    /// Hooks to remove a few seconds after they've loaded the helper.
    hooks: Vec<(isize, Instant)>,
    /// When transparency was last switched on (Explorer crash guard).
    enabled_at: Option<Instant>,
    note: String,
}

static ST: Mutex<State> = Mutex::new(State { view: 0, hmod: 0, injected: Vec::new(), hooks: Vec::new(), enabled_at: None, note: String::new() });

fn dll_path() -> std::path::PathBuf {
    config::dir().join("glass").join(format!("mglass-{}.dll", env!("CARGO_PKG_VERSION")))
}

pub fn log_path() -> std::path::PathBuf {
    config::dir().join("glass.log")
}

fn wide_into(dst: &mut [u16; 300], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(299).collect();
    dst.fill(0);
    dst[..w.len()].copy_from_slice(&w);
}

fn shared(st: &mut State) -> Option<&'static mut Shared> {
    if st.view == 0 {
        unsafe {
            let size = std::mem::size_of::<Shared>() as u32;
            let h = CreateFileMappingW(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, 0, size, w!("Local\\PhemioGlass2")).ok()?;
            let v = MapViewOfFile(h, FILE_MAP_ALL_ACCESS, 0, 0, size as usize);
            if v.Value.is_null() {
                return None;
            }
            st.view = v.Value as isize;
            let s = &mut *(v.Value as *mut Shared);
            s.magic = MAGIC;
            s.proto = 1;
        }
    }
    unsafe { (st.view as *mut Shared).as_mut() }
}

fn wm_apply() -> u32 {
    unsafe { RegisterWindowMessageW(w!("PhemioGlassApply")) }
}

fn enabled(c: &config::Config) -> (bool, bool) {
    (c.b("glass_tb"), c.b("glass_ex"))
}

/// Write the helper next to the settings (once per version).
fn ensure_dll() -> bool {
    let p = dll_path();
    if std::fs::metadata(&p).map(|m| m.len() == DLL.len() as u64).unwrap_or(false) {
        return true;
    }
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    // Older versions' copies are left alone (Explorer may still hold them).
    std::fs::write(&p, DLL).is_ok()
}

/// Settings changed (or Phemio started): pass them on to Explorer.
pub fn sync(host: HWND) {
    let c = config::cur();
    let (tb, ex) = enabled(&c);
    let mut st = match ST.lock() {
        Ok(s) => s,
        Err(_) => return,
    };
    if !tb && !ex && st.view == 0 {
        return;
    }
    if util::is_wine() && std::env::var("ISLET_GLASS").is_err() {
        return;
    }
    let was_on = st.view != 0 && shared(&mut st).map(|s| s.tb_on != 0 || s.ex_on != 0).unwrap_or(false);
    let Some(s) = shared(&mut st) else { return };
    let dll = dll_path().to_string_lossy().to_string();
    wide_into(&mut s.dll, &dll);
    wide_into(&mut s.log, &log_path().to_string_lossy());
    unsafe {
        std::ptr::write_volatile(&mut s.host_pid, std::process::id());
        std::ptr::write_volatile(&mut s.tb_on, tb as u32);
        std::ptr::write_volatile(&mut s.tb_lvl, c.i("glass_tb_lvl").clamp(0, 100) as u32);
        std::ptr::write_volatile(&mut s.ex_on, ex as u32);
        std::ptr::write_volatile(&mut s.ex_lvl, c.i("glass_ex_lvl").clamp(0, 100) as u32);
        std::ptr::write_volatile(&mut s.look, c.i("glass_look").clamp(0, 1) as u32);
        std::ptr::write_volatile(&mut s.ex_mode, c.i("glass_ex_mode").clamp(0, 1) as u32);
        if !tb {
            std::ptr::write_volatile(&mut s.tb_state, 0);
        }
        if !ex {
            std::ptr::write_volatile(&mut s.ex_state, 0);
        }
        s.gen = s.gen.wrapping_add(1);
    }
    if (tb || ex) && !was_on {
        st.enabled_at = Some(Instant::now());
        st.note.clear();
    }
    if tb || ex {
        if !ensure_dll() {
            st.note = "Couldn't write the helper to Phemio's data folder".into();
            return;
        }
        inject_all(&mut st, ex);
        unsafe {
            SetTimer(Some(host), T_GLASS, 1500, None);
        }
    }
    drop(st);
    notify();
}

/// While dragging a slider: just the numbers.
pub fn sync_live() {
    let c = config::cur();
    if let Ok(mut st) = ST.lock() {
        if st.view == 0 {
            return;
        }
        if let Some(s) = shared(&mut st) {
            unsafe {
                std::ptr::write_volatile(&mut s.tb_lvl, c.i("glass_tb_lvl").clamp(0, 100) as u32);
                std::ptr::write_volatile(&mut s.ex_lvl, c.i("glass_ex_lvl").clamp(0, 100) as u32);
            }
        }
    }
    notify();
}

/// Tell every copy of the helper to look at the settings again: its window
/// on the taskbar's XAML thread, and each Explorer process's own window.
fn notify() {
    let m = wm_apply();
    unsafe {
        for class in [w!("PhemioGlassX"), w!("PhemioGlassW")] {
            let mut h = HWND::default();
            loop {
                h = FindWindowExW(Some(HWND_MESSAGE), Some(h), class, PCWSTR::null()).unwrap_or_default();
                if h.is_invalid() {
                    break;
                }
                let _ = PostMessageW(Some(h), m, WPARAM(0), LPARAM(0));
            }
        }
    }
}

fn inject_all(st: &mut State, explorer_windows: bool) {
    unsafe {
        let mut threads: Vec<(u32, u32, HWND)> = Vec::new();
        if let Ok(tray) = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) {
            let mut pid = 0u32;
            let tid = GetWindowThreadProcessId(tray, Some(&mut pid));
            threads.push((pid, tid, tray));
        }
        if explorer_windows {
            // File Explorer windows in their own Explorer process ("Launch
            // folder windows in a separate process").
            let mut h = HWND::default();
            loop {
                h = FindWindowExW(None, Some(h), w!("CabinetWClass"), PCWSTR::null()).unwrap_or_default();
                if h.is_invalid() {
                    break;
                }
                let mut pid = 0u32;
                let tid = GetWindowThreadProcessId(h, Some(&mut pid));
                if !threads.iter().any(|t| t.0 == pid) {
                    threads.push((pid, tid, h));
                }
            }
        }
        // Forget processes that have gone (Explorer restarted, an app quit).
        st.injected.retain(|p| threads.iter().any(|t| t.0 == *p));
        for (pid, tid, hwnd) in threads {
            if pid == 0 || tid == 0 || st.injected.contains(&pid) {
                continue;
            }
            if st.hmod == 0 {
                let p = dll_path();
                let w: Vec<u16> = p.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
                match LoadLibraryW(PCWSTR(w.as_ptr())) {
                    Ok(m) => st.hmod = m.0 as isize,
                    Err(e) => {
                        st.note = format!("Couldn't load the helper: {e}");
                        util::log(&format!("glass: LoadLibrary failed: {e}"));
                        return;
                    }
                }
            }
            let hmod = HMODULE(st.hmod as *mut _);
            let Some(proc_) = GetProcAddress(hmod, s!("mglass_hook")) else { return };
            let proc_: HOOKPROC = Some(std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT>(proc_));
            match SetWindowsHookExW(WH_GETMESSAGE, proc_, Some(HINSTANCE(hmod.0)), tid) {
                Ok(hk) => {
                    st.hooks.push((hk.0 as isize, Instant::now()));
                    st.injected.push(pid);
                    // Wake the thread so the hook (and the helper) load now.
                    let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
                    util::log(&format!("glass: helper sent to pid {pid}"));
                }
                Err(e) => {
                    st.note = format!("Couldn't reach Explorer: {e}");
                    util::log(&format!("glass: SetWindowsHookEx failed: {e}"));
                }
            }
        }
    }
}

/// Every 1.5 s while transparency is on: remove hooks that have done their
/// job, reach File Explorer windows in new Explorer processes, re-send the
/// settings once the helper is up.
pub fn tick(host: HWND) {
    let c = config::cur();
    let (tb, ex) = enabled(&c);
    let mut resend = false;
    if let Ok(mut st) = ST.lock() {
        let now = Instant::now();
        st.hooks.retain(|(h, t)| {
            if now.duration_since(*t) > Duration::from_secs(4) {
                unsafe {
                    let _ = UnhookWindowsHookEx(HHOOK(*h as *mut _));
                }
                false
            } else {
                true
            }
        });
        if tb || ex {
            let before = st.injected.len();
            inject_all(&mut st, ex);
            resend = st.injected.len() != before;
            // Still starting: keep nudging until the helper answers.
            if let Some(s) = shared(&mut st) {
                if (tb && s.tb_state <= 1) || (ex && s.ex_state <= 1) {
                    resend = true;
                }
            }
        }
        if !tb && !ex && st.hooks.is_empty() {
            unsafe {
                let _ = KillTimer(Some(host), T_GLASS);
            }
        }
    }
    if resend {
        notify();
    }
}

/// Explorer restarted. Right after switching transparency on, assume we
/// caused it and switch off; otherwise load the helper into the new Explorer.
pub fn on_explorer_restart(host: HWND) {
    let c = config::cur();
    let (tb, ex) = enabled(&c);
    if !tb && !ex {
        return;
    }
    let recent = ST.lock().ok().and_then(|s| s.enabled_at).map(|t| t.elapsed() < Duration::from_secs(60)).unwrap_or(false);
    if recent {
        util::log("glass: Explorer restarted right after switching transparency on — switched it off");
        config::update(|c| {
            c.set("glass_tb", 0);
            c.set("glass_ex", 0);
        });
        if let Ok(mut st) = ST.lock() {
            st.note = "Explorer restarted right after this was switched on, so it was switched off to be safe".into();
            st.injected.clear();
        }
        crate::request_apply();
        return;
    }
    if let Ok(mut st) = ST.lock() {
        st.injected.clear();
    }
    // Give the new taskbar a moment, then go again.
    unsafe {
        SetTimer(Some(host), T_GLASS, 3000, None);
    }
    let _ = host;
}

/// Phemio is closing: everything back to Windows' own look.
pub fn shutdown() {
    if let Ok(mut st) = ST.lock() {
        if st.view == 0 {
            return;
        }
        if let Some(s) = shared(&mut st) {
            unsafe {
                std::ptr::write_volatile(&mut s.tb_on, 0);
                std::ptr::write_volatile(&mut s.ex_on, 0);
            }
        }
        for (h, _) in st.hooks.drain(..) {
            unsafe {
                let _ = UnhookWindowsHookEx(HHOOK(h as *mut _));
            }
        }
    }
    notify();
}

/// Restore Windows defaults (the button): both off.
pub fn restore() {
    config::update(|c| {
        c.set("glass_tb", 0);
        c.set("glass_ex", 0);
    });
    crate::request_apply();
}

/// For Settings: what the taskbar and File Explorer are doing.
pub fn status() -> (u32, u32, String) {
    match ST.lock() {
        Ok(mut st) => {
            let note = st.note.clone();
            if st.view == 0 {
                return (0, 0, note);
            }
            let s = shared(&mut st)
                .map(|s| unsafe { (std::ptr::read_volatile(&s.tb_state), std::ptr::read_volatile(&s.ex_state)) })
                .unwrap_or((0, 0));
            (s.0, s.1, note)
        }
        Err(_) => (0, 0, String::new()),
    }
}

pub fn state_text(on: bool, s: u32) -> &'static str {
    if !on {
        return "Off";
    }
    match s {
        2 => "Working: background only",
        3 => "Working: whole window faded (background only isn't possible here)",
        4 => "Not working on this Windows version",
        _ => "Starting…",
    }
}

