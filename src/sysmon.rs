//! System signals for the island's alerts: Caps Lock / keyboard language,
//! Wi-Fi / internet, screen brightness, idle time.
//!
//! The keyboard hook only looks at the key code of Caps Lock and the usual
//! language-switch keys and immediately passes everything on. Nothing typed is
//! read or stored.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Globalization::*;
use windows::Win32::NetworkManagement::IpHelper::*;
use windows::Win32::Networking::WinSock::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Variant::*;
use windows::Win32::System::Wmi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::util;

static HOST: AtomicIsize = AtomicIsize::new(0);
static KEY_MSG: AtomicU32 = AtomicU32::new(0);

pub const KEY_CAPS: usize = 1;
pub const KEY_LANG: usize = 2;
/// Any key went down while the mascot was showing (nothing about which key).
pub const KEY_ANY: usize = 3;
/// A key went down while the cat is out (it taps a paw; nothing about which key).
pub const KEY_TAP: usize = 4;
/// Volume up / down while several speakers play together (Phemio handles them).
pub const KEY_VOL_UP: usize = 5;
pub const KEY_VOL_DOWN: usize = 6;

/// Report Caps Lock / language keys (Alerts → Caps Lock & keyboard language).
pub static WATCH_KEYS: AtomicBool = AtomicBool::new(false);
/// Report the next key press once (the mascot is showing).
pub static WAKE_ON_KEY: AtomicBool = AtomicBool::new(false);
/// The cat drums along with your typing (only *that* a key went down is passed on).
pub static TAP_KEYS: AtomicBool = AtomicBool::new(false);
/// A tap is on its way to the window (so a fast typist doesn't flood it).
pub static TAP_SENT: AtomicBool = AtomicBool::new(false);
/// Several speakers play together: the volume keys move them all (by the step
/// in Settings), so Windows doesn't get them.
pub static VOL_KEYS: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Keyboard hook
// ---------------------------------------------------------------------------

pub struct KeyHook(HHOOK);

unsafe extern "system" fn kb_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && VOL_KEYS.load(Ordering::Relaxed) {
        let k = &*(lp.0 as *const KBDLLHOOKSTRUCT);
        let vk = VIRTUAL_KEY(k.vkCode as u16);
        if vk == VK_VOLUME_UP || vk == VK_VOLUME_DOWN {
            let down = wp.0 as u32 == WM_KEYDOWN || wp.0 as u32 == WM_SYSKEYDOWN;
            if down {
                let h = HOST.load(Ordering::Relaxed);
                let what = if vk == VK_VOLUME_UP { KEY_VOL_UP } else { KEY_VOL_DOWN };
                let _ = PostMessageW(Some(HWND(h as *mut _)), KEY_MSG.load(Ordering::Relaxed), WPARAM(what), LPARAM(0));
            }
            // Ours: Windows would only move the main speaker.
            return LRESULT(1);
        }
    }
    if code == HC_ACTION as i32 && (wp.0 as u32 == WM_KEYDOWN || wp.0 as u32 == WM_SYSKEYDOWN) && WAKE_ON_KEY.swap(false, Ordering::Relaxed) {
        let h = HOST.load(Ordering::Relaxed);
        let _ = PostMessageW(Some(HWND(h as *mut _)), KEY_MSG.load(Ordering::Relaxed), WPARAM(KEY_ANY), LPARAM(0));
    } else if code == HC_ACTION as i32 && (wp.0 as u32 == WM_KEYDOWN || wp.0 as u32 == WM_SYSKEYDOWN) && TAP_KEYS.load(Ordering::Relaxed) && !TAP_SENT.swap(true, Ordering::Relaxed) {
        let h = HOST.load(Ordering::Relaxed);
        let _ = PostMessageW(Some(HWND(h as *mut _)), KEY_MSG.load(Ordering::Relaxed), WPARAM(KEY_TAP), LPARAM(0));
    }
    if code == HC_ACTION as i32 && WATCH_KEYS.load(Ordering::Relaxed) && (wp.0 as u32 == WM_KEYUP || wp.0 as u32 == WM_SYSKEYUP) {
        let k = &*(lp.0 as *const KBDLLHOOKSTRUCT);
        let vk = VIRTUAL_KEY(k.vkCode as u16);
        let what = if vk == VK_CAPITAL {
            KEY_CAPS
        } else if matches!(vk, VK_SPACE | VK_LSHIFT | VK_RSHIFT | VK_SHIFT | VK_LMENU | VK_RMENU | VK_MENU | VK_LCONTROL | VK_RCONTROL | VK_CONTROL | VK_LWIN | VK_RWIN) {
            KEY_LANG
        } else {
            0
        };
        if what != 0 {
            let h = HOST.load(Ordering::Relaxed);
            let _ = PostMessageW(Some(HWND(h as *mut _)), KEY_MSG.load(Ordering::Relaxed), WPARAM(what), LPARAM(0));
        }
    }
    CallNextHookEx(None, code, wp, lp)
}

impl KeyHook {
    pub fn start(host: HWND, msg: u32) -> Option<KeyHook> {
        HOST.store(host.0 as isize, Ordering::Relaxed);
        KEY_MSG.store(msg, Ordering::Relaxed);
        unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_proc), Some(util::hinstance()), 0).ok().map(KeyHook) }
    }
}

impl Drop for KeyHook {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWindowsHookEx(self.0);
        }
    }
}

pub fn caps_on() -> bool {
    unsafe { GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0 }
}

/// Language id of the keyboard layout used by the foreground app.
pub fn current_lang() -> u16 {
    unsafe {
        let fg = GetForegroundWindow();
        let tid = GetWindowThreadProcessId(fg, None);
        (GetKeyboardLayout(tid).0 as usize & 0xffff) as u16
    }
}

/// ("Vietnamese", "VI") for a language id.
pub fn lang_name(lang: u16) -> (String, String) {
    unsafe {
        let mut name = [0u16; 85];
        let n = LCIDToLocaleName(lang as u32, Some(&mut name), 0);
        if n == 0 {
            return (format!("Layout {lang:04x}"), String::new());
        }
        let loc = PCWSTR(name.as_ptr());
        let mut full = [0u16; 128];
        let mut short = [0u16; 16];
        GetLocaleInfoEx(loc, LOCALE_SLOCALIZEDDISPLAYNAME, Some(&mut full));
        GetLocaleInfoEx(loc, LOCALE_SISO639LANGNAME, Some(&mut short));
        (util::from_wide(&full), util::from_wide(&short).to_uppercase())
    }
}

/// An installed keyboard (input language + layout).
#[derive(Clone, Debug)]
pub struct Keyboard {
    pub hkl: isize,
    pub lang: u16,
    /// "Vietnamese (Vietnam)"
    pub name: String,
    /// "VI"
    pub short: String,
    /// "Vietnamese", "US", "United Kingdom"…
    pub layout: String,
}

/// Every keyboard in Settings → Time & language → Language & region.
pub fn keyboards() -> Vec<Keyboard> {
    if let Ok(v) = std::env::var("ISLET_FAKE_KEYBOARDS") {
        return v
            .split(';')
            .enumerate()
            .map(|(i, e)| {
                let p: Vec<&str> = e.split(':').collect();
                Keyboard { hkl: i as isize + 1, lang: i as u16 + 1, name: p[0].into(), short: p.get(1).unwrap_or(&"").to_string(), layout: p.get(2).unwrap_or(&"").to_string() }
            })
            .collect();
    }
    unsafe {
        let n = GetKeyboardLayoutList(None);
        let mut list = vec![HKL::default(); n.max(0) as usize];
        let n = GetKeyboardLayoutList(Some(&mut list));
        list.truncate(n.max(0) as usize);
        list.into_iter()
            .map(|h| {
                let v = h.0 as usize;
                let lang = (v & 0xffff) as u16;
                let (name, short) = lang_name(lang);
                Keyboard { hkl: h.0 as isize, lang, name, short, layout: layout_text(v) }
            })
            .collect()
    }
}

/// The layout's own name from the registry ("US", "Vietnamese", "Dvorak").
fn layout_text(hkl: usize) -> String {
    use windows::Win32::System::Registry::*;
    let hi = ((hkl >> 16) & 0xffff) as u32;
    let base = "SYSTEM\\CurrentControlSet\\Control\\Keyboard Layouts";
    let read = |klid: &str, value: &str| -> Option<String> {
        let key = util::wide(&format!("{base}\\{klid}"));
        let val = util::wide(value);
        let mut buf = [0u16; 128];
        let mut sz = (buf.len() * 2) as u32;
        let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, PCWSTR(key.as_ptr()), PCWSTR(val.as_ptr()), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut sz)) };
        (r.is_ok()).then(|| util::from_wide(&buf))
    };
    if hi & 0xf000 == 0xf000 {
        // A variant layout: find the KLID whose "Layout Id" matches.
        let want = format!("{:04x}", hi & 0x0fff);
        unsafe {
            let key = util::wide(base);
            let mut hk = HKEY::default();
            if RegOpenKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(key.as_ptr()), Some(0), KEY_READ, &mut hk).is_ok() {
                let mut i = 0;
                loop {
                    let mut name = [0u16; 32];
                    let mut len = name.len() as u32;
                    if RegEnumKeyExW(hk, i, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None).is_err() {
                        break;
                    }
                    let klid = util::from_wide(&name);
                    if read(&klid, "Layout Id").map(|id| id.eq_ignore_ascii_case(&want)).unwrap_or(false) {
                        let _ = RegCloseKey(hk);
                        return read(&klid, "Layout Text").unwrap_or_default();
                    }
                    i += 1;
                }
                let _ = RegCloseKey(hk);
            }
        }
        return String::new();
    }
    read(&format!("0000{hi:04X}"), "Layout Text").unwrap_or_default()
}

/// The keyboard the foreground app is typing with.
pub fn current_hkl() -> isize {
    unsafe {
        let fg = GetForegroundWindow();
        let tid = GetWindowThreadProcessId(fg, None);
        GetKeyboardLayout(tid).0 as isize
    }
}

/// Switch the foreground app (and, with Windows' default settings, every app) to `hkl`.
pub fn switch_keyboard(hkl: isize) {
    unsafe {
        let fg = GetForegroundWindow();
        if !fg.is_invalid() {
            let _ = PostMessageW(Some(fg), WM_INPUTLANGCHANGEREQUEST, WPARAM(0), LPARAM(hkl));
        }
    }
}

// ---------------------------------------------------------------------------
// Network connectivity
// ---------------------------------------------------------------------------

static NET_MSG: AtomicU32 = AtomicU32::new(0);
static NET_FIRST: AtomicBool = AtomicBool::new(true);

pub struct NetWatch(HANDLE);

unsafe extern "system" fn net_cb(_ctx: *const core::ffi::c_void, hint: NL_NETWORK_CONNECTIVITY_HINT) {
    let level = hint.ConnectivityLevel.0 as usize;
    let first = NET_FIRST.swap(false, Ordering::Relaxed) as isize;
    let h = HOST.load(Ordering::Relaxed);
    let _ = PostMessageW(Some(HWND(h as *mut _)), NET_MSG.load(Ordering::Relaxed), WPARAM(level), LPARAM(first));
}

impl NetWatch {
    pub fn start(host: HWND, msg: u32) -> Option<NetWatch> {
        if util::is_wine() {
            return None;
        }
        HOST.store(host.0 as isize, Ordering::Relaxed);
        NET_MSG.store(msg, Ordering::Relaxed);
        NET_FIRST.store(true, Ordering::Relaxed);
        unsafe {
            let mut h = HANDLE::default();
            (NotifyNetworkConnectivityHintChange(Some(net_cb), None, true, &mut h) == NO_ERROR).then_some(NetWatch(h))
        }
    }
}

impl Drop for NetWatch {
    fn drop(&mut self) {
        unsafe {
            let _ = CancelMibChangeNotify2(self.0);
        }
    }
}

/// NL_NETWORK_CONNECTIVITY_LEVEL_HINT: 3 = internet, 4 = constrained internet.
pub fn level_online(level: usize) -> bool {
    level == 3 || level == 4
}

// ---------------------------------------------------------------------------
// Brightness (laptop panels) via a WMI event query on a sleeping thread
// ---------------------------------------------------------------------------

static BRIGHT_STOP: AtomicBool = AtomicBool::new(false);

pub fn start_brightness(host: HWND, msg: u32) {
    BRIGHT_STOP.store(false, Ordering::Relaxed);
    let h = host.0 as isize;
    let _ = std::thread::Builder::new().name("brightness".into()).stack_size(256 * 1024).spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = brightness_loop(h, msg);
        CoUninitialize();
    });
}

pub fn stop_brightness() {
    BRIGHT_STOP.store(true, Ordering::Relaxed);
}

unsafe fn brightness_loop(h: isize, msg: u32) -> Result<()> {
    let loc: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
    let empty = BSTR::new();
    let svc = loc.ConnectServer(&BSTR::from("ROOT\\WMI"), &empty, &empty, &empty, 0, &empty, None)?;
    let _ = CoSetProxyBlanket(&svc, 10, 0, PCWSTR::null(), RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, None, EOAC_NONE);
    let en = svc.ExecNotificationQuery(
        &BSTR::from("WQL"),
        &BSTR::from("SELECT * FROM WmiMonitorBrightnessEvent"),
        WBEM_FLAG_RETURN_IMMEDIATELY | WBEM_FLAG_FORWARD_ONLY,
        None,
    )?;
    loop {
        let mut objs: [Option<IWbemClassObject>; 1] = [None];
        let mut n = 0u32;
        let hr = en.Next(WBEM_INFINITE, &mut objs, &mut n);
        if BRIGHT_STOP.load(Ordering::Relaxed) {
            break;
        }
        if hr.is_err() {
            break;
        }
        if n == 1 {
            if let Some(o) = &objs[0] {
                let mut v = VARIANT::default();
                if o.Get(w!("Brightness"), 0, &mut v, None, None).is_ok() {
                    let b = v.Anonymous.Anonymous.Anonymous.bVal as usize;
                    let _ = PostMessageW(Some(HWND(h as *mut _)), msg, WPARAM(b.min(100)), LPARAM(0));
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Idle
// ---------------------------------------------------------------------------

pub fn idle_ms() -> u32 {
    unsafe {
        let mut li = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        if GetLastInputInfo(&mut li).as_bool() {
            windows::Win32::System::SystemInformation::GetTickCount().wrapping_sub(li.dwTime)
        } else {
            0
        }
    }
}

/// The power list: 0 lock, 1 sleep, 2 shut down, 3 restart.
pub fn power(what: u8) {
    unsafe {
        match what {
            0 => {
                let _ = windows::Win32::System::Shutdown::LockWorkStation();
            }
            1 => {
                // Sleep (not hibernate), wake events allowed.
                let _ = windows::Win32::System::Power::SetSuspendState(false, false, false);
            }
            2 | 3 => {
                let args = util::wide(if what == 2 { "/s /t 0" } else { "/r /t 0" });
                windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), w!("shutdown.exe"), PCWSTR(args.as_ptr()), PCWSTR::null(), SW_HIDE);
            }
            _ => {}
        }
    }
}
