//! mglass — Phemio's transparency helper.
//!
//! Phemio loads this DLL into Explorer (a short-lived WH_GETMESSAGE hook
//! on the taskbar's thread; the DLL then pins itself). Inside Explorer:
//!
//! * Taskbar — done the way TranslucentTB does it on Windows 11: a XAML
//!   diagnostics session (InitializeXamlDiagnosticsEx, the API behind Visual
//!   Studio's Live Visual Tree) on a fresh thread, our TAP object gets the
//!   site, a visual tree watcher (advised from another thread; its callbacks
//!   arrive on the XAML thread) finds each taskbar's `BackgroundFill` and
//!   `BackgroundStroke` rectangles under `TaskbarFrame`, and we swap their
//!   Fill for a see-through brush (a solid colour with the chosen alpha, or
//!   an acrylic "Backdrop" brush), keeping the original to put back.
//! * File Explorer — a see-through backdrop and the file list / folder tree
//!   background repainted with transparency (or the whole window faded, by
//!   choice or when that doesn't take).
//!
//! Settings come from Phemio through a small shared memory block and a
//! "settings changed" message; status goes back the same way. When
//! Phemio quits or crashes, everything is put back.
#![allow(non_snake_case, clippy::missing_safety_doc)]

mod xaml;

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::Mutex;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::WindowsAndMessaging::*;

use xaml::Windows::UI::Xaml as wux;
use xaml::Windows::Win32::UI::Xaml::Diagnostics as diag;

// ---------------------------------------------------------------------------
// Shared with Phemio (keep in sync with src/glass.rs)
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct Shared {
    pub magic: u32,
    pub proto: u32,
    pub host_pid: u32,
    pub gen: u32,
    pub tb_on: u32,
    pub tb_lvl: u32,
    pub ex_on: u32,
    pub ex_lvl: u32,
    /// 0 clear, 1 frosted
    pub look: u32,
    /// 0 off, 1 starting, 2 background only, 3 whole window, 4 failed
    pub tb_state: u32,
    pub ex_state: u32,
    /// File Explorer: 0 background only, 1 whole window
    pub ex_mode: u32,
    /// The DLL that should act (others stand down), and the log file.
    pub dll: [u16; 300],
    pub log: [u16; 300],
}

const MAGIC: u32 = 0x4D47_4C53;
const MAP_NAME: PCWSTR = w!("Local\\PhemioGlass2");
/// One per Explorer process (File Explorer windows, host watch).
const WORKER_CLASS: PCWSTR = w!("PhemioGlassW");
/// On the taskbar's XAML thread (the taskbar's look is changed there).
const XAML_CLASS: PCWSTR = w!("PhemioGlassX");

const ST_OFF: u32 = 0;
const ST_START: u32 = 1;
const ST_BG: u32 = 2;
const ST_WHOLE: u32 = 3;
const ST_FAIL: u32 = 4;

static SELF: AtomicIsize = AtomicIsize::new(0);
static SHARED: AtomicIsize = AtomicIsize::new(0);
static IN_EXPLORER: AtomicBool = AtomicBool::new(false);
static HOST_GONE: AtomicBool = AtomicBool::new(false);
static WM_APPLY: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, Default, PartialEq)]
struct Cfg {
    tb_on: bool,
    tb_lvl: u32,
    ex_on: bool,
    ex_lvl: u32,
    frosted: bool,
    ex_whole: bool,
}

fn shared() -> Option<&'static mut Shared> {
    let p = SHARED.load(Ordering::Acquire) as *mut Shared;
    unsafe { p.as_mut() }
}

fn self_path() -> Vec<u16> {
    let mut buf = [0u16; 300];
    let n = unsafe { GetModuleFileNameW(Some(HMODULE(SELF.load(Ordering::Relaxed) as *mut c_void)), &mut buf) } as usize;
    buf[..n].to_vec()
}

fn wstr(a: &[u16]) -> &[u16] {
    &a[..a.iter().position(|&c| c == 0).unwrap_or(a.len())]
}

/// This DLL is the one Phemio wants acting (an older copy left pinned in
/// Explorer after an update stands down).
fn is_current() -> bool {
    match shared() {
        Some(s) => {
            let want = String::from_utf16_lossy(wstr(&s.dll)).to_lowercase();
            let me = String::from_utf16_lossy(&self_path()).to_lowercase();
            !want.is_empty() && want == me
        }
        None => false,
    }
}

fn cfg() -> Cfg {
    if HOST_GONE.load(Ordering::Relaxed) || !is_current() {
        return Cfg::default();
    }
    let Some(s) = shared() else { return Cfg::default() };
    unsafe {
        let r = |p: *const u32| std::ptr::read_volatile(p);
        Cfg {
            tb_on: r(&s.tb_on) != 0,
            tb_lvl: r(&s.tb_lvl).min(100),
            ex_on: r(&s.ex_on) != 0,
            ex_lvl: r(&s.ex_lvl).min(100),
            frosted: r(&s.look) == 1,
            ex_whole: r(&s.ex_mode) == 1,
        }
    }
}

fn set_tb_state(v: u32) {
    if let Some(s) = shared() {
        if is_current() {
            unsafe { std::ptr::write_volatile(&mut s.tb_state, v) };
        }
    }
}

fn set_ex_state(v: u32) {
    if let Some(s) = shared() {
        if is_current() {
            unsafe { std::ptr::write_volatile(&mut s.ex_state, v) };
        }
    }
}

static LOG_LOCK: Mutex<()> = Mutex::new(());

pub fn log(msg: &str) {
    open_shared();
    let Some(s) = shared() else { return };
    let path = String::from_utf16_lossy(wstr(&s.log));
    if path.is_empty() {
        return;
    }
    let _g = LOG_LOCK.lock();
    if std::fs::metadata(&path).map(|m| m.len() > 256 * 1024).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let t = unsafe { windows::Win32::System::SystemInformation::GetTickCount64() };
        let _ = writeln!(f, "[{:>10}] pid {} · {msg}", t, std::process::id());
    }
}

fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "system" fn DllMain(hinst: HINSTANCE, reason: u32, _r: *mut c_void) -> BOOL {
    if reason == 1 {
        SELF.store(hinst.0 as isize, Ordering::Relaxed);
        // Only inside Explorer: Phemio itself loads us just to set the hook.
        let mut buf = [0u16; 300];
        let n = unsafe { GetModuleFileNameW(None, &mut buf) } as usize;
        let exe = String::from_utf16_lossy(&buf[..n]).to_lowercase();
        IN_EXPLORER.store(exe.ends_with("\\explorer.exe"), Ordering::Relaxed);
        // Phemio loads us too (only to set its hook); the worker sees
        // that and stops. Anywhere else, Phemio put us there on purpose.
        {
            unsafe {
                // Stay loaded for good: XAML holds our objects, and our
                // paint hooks point into this module.
                let mut h = HMODULE::default();
                let _ = GetModuleHandleExW(
                    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
                    PCWSTR(DllMain as *const u16),
                    &mut h,
                );
                let _ = CreateThread(None, 0, Some(worker_main), None, THREAD_CREATION_FLAGS(0), None);
            }
        }
    }
    TRUE
}

/// The hook Phemio sets on the taskbar's thread (and on a File Explorer
/// window's thread in a separate Explorer process). Its only job is getting
/// this DLL loaded there.
#[no_mangle]
pub extern "system" fn mglass_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(clsid: *const GUID, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    *out = std::ptr::null_mut();
    if clsid.is_null() || *clsid != CLSID_TAP {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let f: IClassFactory = Factory.into();
    f.query(iid, out)
}

// ---------------------------------------------------------------------------
// Worker thread (one per Explorer process)
// ---------------------------------------------------------------------------

unsafe extern "system" fn worker_main(_: *mut c_void) -> u32 {
    let _ = std::panic::catch_unwind(|| worker());
    0
}

fn open_shared() -> bool {
    if SHARED.load(Ordering::Relaxed) != 0 {
        return true;
    }
    unsafe {
        let Ok(h) = OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, MAP_NAME) else { return false };
        let v = MapViewOfFile(h, FILE_MAP_ALL_ACCESS, 0, 0, std::mem::size_of::<Shared>());
        if v.Value.is_null() {
            return false;
        }
        // (The handle stays open: the block outlives a Phemio restart.)
        let s = &*(v.Value as *const Shared);
        if s.magic != MAGIC {
            return false;
        }
        SHARED.store(v.Value as isize, Ordering::Release);
    }
    true
}

fn wm_apply() -> u32 {
    let m = WM_APPLY.load(Ordering::Relaxed);
    if m != 0 {
        return m;
    }
    let m = unsafe { RegisterWindowMessageW(w!("PhemioGlassApply")) };
    WM_APPLY.store(m, Ordering::Relaxed);
    m
}

fn tray_pid() -> u32 {
    unsafe {
        let tray = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()).unwrap_or_default();
        if tray.is_invalid() {
            return 0;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(tray, Some(&mut pid));
        pid
    }
}

fn worker() {
    unsafe {
        wm_apply();
        for _ in 0..50 {
            if open_shared() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if !open_shared() {
            return;
        }
        if shared().map(|s| std::ptr::read_volatile(&s.host_pid)) == Some(GetCurrentProcessId()) {
            return;
        }
        let mut exe = [0u16; 300];
        let n = GetModuleFileNameW(None, &mut exe) as usize;
        log(&format!("helper loaded in {} ({})", String::from_utf16_lossy(&exe[..n]), String::from_utf16_lossy(&self_path())));
        // A message-only window: Phemio posts "settings changed" to it.
        let hinst = HINSTANCE(SELF.load(Ordering::Relaxed) as *mut c_void);
        let wc = WNDCLASSW { lpfnWndProc: Some(worker_proc), hInstance: hinst, lpszClassName: WORKER_CLASS, ..Default::default() };
        RegisterClassW(&wc);
        let _w = CreateWindowExW(WINDOW_EX_STYLE(0), WORKER_CLASS, w!(""), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(hinst), None);
        // New File Explorer windows in this process.
        let _hook = SetWinEventHook(EVENT_OBJECT_SHOW, EVENT_OBJECT_SHOW, None, Some(win_event), GetCurrentProcessId(), 0, WINEVENT_OUTOFCONTEXT);
        // The taskbar lives here: start the XAML session.
        if tray_pid() == GetCurrentProcessId() {
            std::thread::spawn(|| {
                let _ = std::panic::catch_unwind(xaml_init);
            });
        }
        let mut host_pid = 0u32;
        let mut host = HANDLE::default();
        apply_windows_all();
        loop {
            // Follow Phemio: a new one (restarted) or none (quit / crashed).
            let hp = shared().map(|s| std::ptr::read_volatile(&s.host_pid)).unwrap_or(0);
            if hp != host_pid {
                if !host.is_invalid() {
                    let _ = CloseHandle(host);
                }
                host_pid = hp;
                host = OpenProcess(PROCESS_SYNCHRONIZE, false, hp).unwrap_or_default();
                if !host.is_invalid() && HOST_GONE.swap(false, Ordering::Relaxed) {
                    log("Phemio is back");
                    apply_windows_all();
                    post_xaml();
                }
            }
            let c = cfg();
            let timeout = if HOST_GONE.load(Ordering::Relaxed) {
                3000
            } else if PENDING_CHECK.load(Ordering::Relaxed) || (c.tb_on && XAML_STATE.load(Ordering::Relaxed) != XS_FOUND) {
                1500
            } else {
                INFINITE
            };
            let handles = if host.is_invalid() || HOST_GONE.load(Ordering::Relaxed) { vec![] } else { vec![host] };
            let r = MsgWaitForMultipleObjects(if handles.is_empty() { None } else { Some(&handles) }, false, timeout, QS_ALLINPUT);
            if r.0 == WAIT_OBJECT_0.0 && !handles.is_empty() {
                // Phemio has gone: put everything back.
                HOST_GONE.store(true, Ordering::Relaxed);
                let _ = CloseHandle(host);
                host = HANDLE::default();
                log("Phemio quit: restoring the taskbar and File Explorer");
                apply_windows_all();
                post_xaml();
                continue;
            }
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if r == WAIT_TIMEOUT {
                check_fallback();
                taskbar_status();
            }
        }
    }
}

unsafe extern "system" fn worker_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == wm_apply() {
        let _ = std::panic::catch_unwind(|| {
            apply_windows_all();
            taskbar_status();
        });
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

unsafe extern "system" fn win_event(_h: HWINEVENTHOOK, _ev: u32, hwnd: HWND, idobj: i32, _idchild: i32, _tid: u32, _t: u32) {
    if idobj != 0 || hwnd.is_invalid() {
        return;
    }
    let _ = std::panic::catch_unwind(|| {
        if is_target_window(hwnd) {
            let w = want(cfg());
            if w.on {
                apply_window(hwnd, w);
            }
        }
    });
}

/// Layered-window alpha for a whole window (None: back to normal). Returns
/// whether WS_EX_LAYERED is ours to remove later.
unsafe fn layer(hwnd: HWND, alpha: Option<u8>, added: bool) -> bool {
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    match alpha {
        Some(a) => {
            let mut now_added = added;
            if ex & WS_EX_LAYERED.0 as isize == 0 {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as isize);
                now_added = true;
            }
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), a, LWA_ALPHA);
            now_added
        }
        None => {
            if added && ex & WS_EX_LAYERED.0 as isize != 0 {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex & !(WS_EX_LAYERED.0 as isize));
                let _ = RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_FRAME | RDW_ERASE);
            }
            false
        }
    }
}

/// Whole-window alpha for a given transparency: never below 30 % visible.
fn whole_alpha(lvl: u32) -> u8 {
    (255.0 * (1.0 - 0.7 * lvl as f32 / 100.0)).round() as u8
}

// ---------------------------------------------------------------------------
// Taskbar (XAML), the TranslucentTB way
// ---------------------------------------------------------------------------

/// Our TAP ("tool attach point") class, created by XAML through DllGetClassObject.
const CLSID_TAP: GUID = GUID::from_u128(0x6b9e2a57_3c4d_4e8a_9f21_7a5d0c3b8e14);

const XS_NONE: u32 = 0;
const XS_WAITING: u32 = 1;
const XS_READY: u32 = 2;
const XS_FOUND: u32 = 3;
const XS_FAILED: u32 = 4;
static XAML_STATE: AtomicU32 = AtomicU32::new(XS_NONE);
static XAML_SINCE: AtomicU32 = AtomicU32::new(0);
/// The message window on the XAML thread.
static XAML_WIN: AtomicIsize = AtomicIsize::new(0);

struct Diag(diag::IXamlDiagnostics);
unsafe impl Send for Diag {}
static DIAG: Mutex<Option<Diag>> = Mutex::new(None);

fn tick() -> u32 {
    (unsafe { windows::Win32::System::SystemInformation::GetTickCount64() } / 1000) as u32
}

type InitXamlDiag = unsafe extern "system" fn(PCWSTR, u32, PCWSTR, PCWSTR, GUID, PCWSTR) -> HRESULT;

fn xaml_init() {
    if XAML_STATE.compare_exchange(XS_NONE, XS_WAITING, Ordering::Relaxed, Ordering::Relaxed).is_err() {
        return;
    }
    XAML_SINCE.store(tick(), Ordering::Relaxed);
    unsafe {
        let wux_dll = match LoadLibraryExW(w!("Windows.UI.Xaml.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) {
            Ok(m) => m,
            Err(e) => {
                log(&format!("taskbar: couldn't load Windows.UI.Xaml.dll: {e}"));
                XAML_STATE.store(XS_FAILED, Ordering::Relaxed);
                return;
            }
        };
        let Some(f) = GetProcAddress(wux_dll, s!("InitializeXamlDiagnosticsEx")) else {
            log("taskbar: InitializeXamlDiagnosticsEx not found");
            XAML_STATE.store(XS_FAILED, Ordering::Relaxed);
            return;
        };
        let f: InitXamlDiag = std::mem::transmute(f);
        let mut path = self_path();
        path.push(0);
        let pid = GetCurrentProcessId();
        // XAML diagnostics can only be started once per thread (later calls
        // just say S_OK), so each try runs on a thread of its own.
        let mut hr = HRESULT(-1);
        for attempt in 1..=60 {
            let p = path.clone();
            hr = std::thread::spawn(move || {
                let conn: Vec<u16> = format!("VisualDiagConnection{attempt}").encode_utf16().chain(Some(0)).collect();
                f(PCWSTR(conn.as_ptr()), pid, PCWSTR::null(), PCWSTR(p.as_ptr()), CLSID_TAP, PCWSTR::null())
            })
            .join()
            .unwrap_or(HRESULT(-1));
            if hr.is_ok() {
                break;
            }
            if attempt == 1 {
                log(&format!("taskbar: InitializeXamlDiagnosticsEx {:#010x}, retrying", hr.0));
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        if hr.is_err() {
            log(&format!("taskbar: XAML diagnostics unavailable ({:#010x})", hr.0));
            XAML_STATE.store(XS_FAILED, Ordering::Relaxed);
            return;
        }
        log("taskbar: XAML diagnostics started");
        if XAML_STATE.load(Ordering::Relaxed) == XS_WAITING {
            XAML_STATE.store(XS_READY, Ordering::Relaxed);
        }
        XAML_SINCE.store(tick(), Ordering::Relaxed);
    }
}

/// The taskbar's status for Settings (from the worker thread).
fn taskbar_status() {
    let c = cfg();
    if !c.tb_on {
        if is_current() {
            set_tb_state(ST_OFF);
        }
        return;
    }
    let st = XAML_STATE.load(Ordering::Relaxed);
    let waited = tick().saturating_sub(XAML_SINCE.load(Ordering::Relaxed));
    match st {
        XS_FOUND => set_tb_state(ST_BG),
        XS_FAILED => set_tb_state(ST_FAIL),
        XS_READY if waited > 20 => {
            log("taskbar: the background wasn't found in 20 s (Windows built the taskbar differently?)");
            XAML_STATE.store(XS_FAILED, Ordering::Relaxed);
            set_tb_state(ST_FAIL);
        }
        _ => set_tb_state(ST_START),
    }
}

/// Ask the XAML thread to apply the settings.
fn post_xaml() {
    let h = XAML_WIN.load(Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut c_void)), wm_apply(), WPARAM(0), LPARAM(0));
        }
    }
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, iid: *const GUID, out: *mut *mut c_void) -> Result<()> {
        unsafe {
            if out.is_null() {
                return Err(E_POINTER.into());
            }
            *out = std::ptr::null_mut();
            if outer.is_some() {
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            let tap: IObjectWithSite = Tap { site: Mutex::new(None) }.into();
            tap.query(iid, out).ok()
        }
    }
    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

#[implement(IObjectWithSite)]
struct Tap {
    site: Mutex<Option<IUnknown>>,
}

static WATCHING: AtomicBool = AtomicBool::new(false);

impl IObjectWithSite_Impl for Tap_Impl {
    fn SetSite(&self, site: Ref<IUnknown>) -> Result<()> {
        let site: Option<IUnknown> = site.cloned();
        if let Ok(mut s) = self.site.lock() {
            *s = site.clone();
        }
        let Some(site) = site else { return Ok(()) };
        // Only ever one watcher.
        if WATCHING.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        let diag_: diag::IXamlDiagnostics = match site.cast() {
            Ok(d) => d,
            Err(e) => {
                log(&format!("taskbar: no IXamlDiagnostics: {e}"));
                return Ok(());
            }
        };
        let svc: diag::IVisualTreeService = match site.cast() {
            Ok(s) => s,
            Err(e) => {
                log(&format!("taskbar: no IVisualTreeService: {e}"));
                return Ok(());
            }
        };
        if let Ok(mut d) = DIAG.lock() {
            *d = Some(Diag(diag_));
        }
        // Advising from another thread avoids hangs; XAML brings the
        // callbacks to its own thread.
        let raw = svc.into_raw() as isize;
        std::thread::spawn(move || {
            let _ = std::panic::catch_unwind(|| unsafe {
                let svc = diag::IVisualTreeService::from_raw(raw as *mut c_void);
                let cb: diag::IVisualTreeServiceCallback2 = Watcher.into();
                let r = svc.AdviseVisualTreeChange(&cb);
                log(&format!("taskbar: watching the taskbar's visual tree ({r:?})"));
                std::mem::forget(cb);
                std::mem::forget(svc);
            });
        });
        Ok(())
    }
    fn GetSite(&self, iid: *const GUID, out: *mut *mut c_void) -> Result<()> {
        unsafe {
            match self.site.lock().ok().and_then(|s| s.clone()) {
                Some(s) => s.query(iid, out).ok(),
                None => Err(E_FAIL.into()),
            }
        }
    }
}

/// A taskbar rectangle we restyle (lives on the XAML thread only).
struct Part {
    handle: u64,
    shape: wux::Shapes::Shape,
    border: bool,
    original: Option<wux::Media::Brush>,
    ours: Option<wux::Media::Brush>,
    /// Frosted glass: a blur of what's behind, as a composition visual on
    /// top of the background rectangle (TranslucentTB's XamlBlurBrush, done
    /// as a child visual).
    blur: Option<windows::UI::Composition::SpriteVisual>,
}

thread_local! {
    static PARTS: RefCell<Vec<Part>> = const { RefCell::new(Vec::new()) };
    static APPLYING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[implement(diag::IVisualTreeServiceCallback2)]
struct Watcher;

fn from_handle<T: Interface>(h: u64) -> Option<T> {
    let d = DIAG.lock().ok()?.as_ref().map(|d| d.0.clone())?;
    unsafe { d.GetIInspectableFromHandle(h).ok()?.cast::<T>().ok() }
}

/// Is `el` inside a taskbar (an ancestor named TaskbarFrame)?
fn in_taskbar_frame(el: &wux::DependencyObject) -> bool {
    let mut cur = el.clone();
    for _ in 0..16 {
        let Ok(p) = wux::Media::VisualTreeHelper::GetParent(&cur) else { return false };
        if let Ok(fe) = p.cast::<wux::FrameworkElement>() {
            if fe.Name().map(|n| n == "TaskbarFrame").unwrap_or(false) {
                return true;
            }
        }
        cur = p;
    }
    false
}

impl diag::IVisualTreeServiceCallback_Impl for Watcher_Impl {
    fn OnVisualTreeChange(&self, relation: &diag::ParentChildRelation, element: &diag::VisualElement, mutation: diag::VisualMutationType) -> Result<()> {
        // The strings are ours to free.
        let (ty, name) = unsafe { (std::ptr::read(&*element.Type), std::ptr::read(&*element.Name)) };
        let (_f, _h) = unsafe { (std::ptr::read(&*element.SrcInfo.FileName), std::ptr::read(&*element.SrcInfo.Hash)) };
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ensure_xaml_window();
            if mutation.0 == 1 {
                // Removed (only the handle is valid).
                PARTS.with(|p| p.borrow_mut().retain(|x| x.handle != element.Handle));
                return;
            }
            if ty != "Windows.UI.Xaml.Shapes.Rectangle" {
                return;
            }
            let n = name.to_string();
            let border = n == "BackgroundStroke";
            if n != "BackgroundFill" && !border {
                return;
            }
            let Some(parent) = from_handle::<wux::DependencyObject>(relation.Parent) else { return };
            let parent_is_frame = parent.cast::<wux::FrameworkElement>().and_then(|f| f.Name()).map(|n| n == "TaskbarFrame").unwrap_or(false);
            if !parent_is_frame && !in_taskbar_frame(&parent) {
                return;
            }
            let Some(shape) = from_handle::<wux::Shapes::Shape>(element.Handle) else { return };
            let original = shape.Fill().ok();
            // Explorer may (re)set the brush later — the first real one is
            // the original; later ones not ours get re-covered.
            if let Ok(dp) = wux::Shapes::Shape::FillProperty() {
                let cb = wux::DependencyPropertyChangedCallback::new(|sender, _dp| {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fill_changed(sender)));
                    Ok(())
                });
                let _ = shape.RegisterPropertyChangedCallback(&dp, &cb);
            }
            PARTS.with(|p| {
                p.borrow_mut().push(Part { handle: element.Handle, shape: shape.clone(), border, original, ours: None, blur: None });
            });
            XAML_STATE.store(XS_FOUND, Ordering::Relaxed);
            log(&format!("taskbar: found {n}"));
            if cfg().tb_on {
                apply_taskbar();
            }
        }));
        Ok(())
    }
}

impl diag::IVisualTreeServiceCallback2_Impl for Watcher_Impl {
    fn OnElementStateChanged(&self, _element: u64, _state: diag::VisualElementState, _ctx: &PCWSTR) -> Result<()> {
        Ok(())
    }
}

fn fill_changed(sender: windows_core::Ref<wux::DependencyObject>) {
    if APPLYING.with(|a| a.get()) {
        return;
    }
    let Some(sender) = sender.as_ref() else { return };
    let mut reapply = false;
    PARTS.with(|p| {
        for part in p.borrow_mut().iter_mut() {
            let same = part.shape.cast::<wux::DependencyObject>().map(|d| &d == sender).unwrap_or(false);
            if !same {
                continue;
            }
            let now = part.shape.Fill().ok();
            if now.is_some() && now != part.ours {
                // Explorer's own new brush (theme change and so on).
                part.original = now;
                reapply = part.ours.is_some();
            }
        }
    });
    if reapply {
        post_xaml();
    }
}

/// The message window on the XAML thread, so settings reach this thread.
fn ensure_xaml_window() {
    if XAML_WIN.load(Ordering::Relaxed) != 0 {
        return;
    }
    unsafe {
        let hinst = HINSTANCE(SELF.load(Ordering::Relaxed) as *mut c_void);
        let wc = WNDCLASSW { lpfnWndProc: Some(xaml_proc), hInstance: hinst, lpszClassName: XAML_CLASS, ..Default::default() };
        RegisterClassW(&wc);
        if let Ok(h) = CreateWindowExW(WINDOW_EX_STYLE(0), XAML_CLASS, w!(""), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(hinst), None) {
            XAML_WIN.store(h.0 as isize, Ordering::Relaxed);
        }
    }
}

unsafe extern "system" fn xaml_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == wm_apply() {
        let _ = std::panic::catch_unwind(apply_taskbar);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

fn light_taskbar() -> bool {
    unsafe {
        let mut v = 0u32;
        let mut sz = 4u32;
        let r = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut c_void),
            Some(&mut sz),
        );
        r == ERROR_SUCCESS && v == 1
    }
}

/// The taskbar's see-through brush: its own colour at the chosen strength.
/// (For frosted glass a blur goes on top of it — see `blur_visual`.)
fn glass_brush(c: Cfg) -> Option<wux::Media::Brush> {
    let base = if light_taskbar() { (0xf3u8, 0xf3u8, 0xf3u8) } else { (0x20u8, 0x20u8, 0x20u8) };
    let keep = 1.0 - c.tb_lvl as f64 / 100.0;
    let b = wux::Media::SolidColorBrush::new().ok()?;
    let _ = b.SetColor(windows::UI::Color { A: (keep.clamp(0.0, 1.0) * 255.0).round() as u8, R: base.0, G: base.1, B: base.2 });
    b.cast().ok()
}

fn clear_brush() -> Option<wux::Media::Brush> {
    let b = wux::Media::SolidColorBrush::new().ok()?;
    let _ = b.SetColor(windows::UI::Color { A: 0, R: 0, G: 0, B: 0 });
    b.cast().ok()
}

/// D2D's Gaussian blur as a Windows.UI.Composition effect (what
/// TranslucentTB's GaussianBlurEffect does).
#[implement(windows::Graphics::Effects::IGraphicsEffect, windows::Graphics::Effects::IGraphicsEffectSource, windows::Win32::System::WinRT::Graphics::Direct2D::IGraphicsEffectD2D1Interop)]
struct BlurEffect {
    source: windows::Graphics::Effects::IGraphicsEffectSource,
    amount: f32,
}

impl windows::Graphics::Effects::IGraphicsEffectSource_Impl for BlurEffect_Impl {}

impl windows::Graphics::Effects::IGraphicsEffect_Impl for BlurEffect_Impl {
    fn Name(&self) -> Result<HSTRING> {
        Ok(HSTRING::from("PhemioBlur"))
    }
    fn SetName(&self, _name: &HSTRING) -> Result<()> {
        Ok(())
    }
}

impl windows::Win32::System::WinRT::Graphics::Direct2D::IGraphicsEffectD2D1Interop_Impl for BlurEffect_Impl {
    fn GetEffectId(&self) -> Result<GUID> {
        Ok(windows::Win32::Graphics::Direct2D::CLSID_D2D1GaussianBlur)
    }
    fn GetNamedPropertyMapping(&self, name: &PCWSTR, index: *mut u32, mapping: *mut windows::Win32::System::WinRT::Graphics::Direct2D::GRAPHICS_EFFECT_PROPERTY_MAPPING) -> Result<()> {
        unsafe {
            let n = name.to_string().unwrap_or_default();
            let i = match n.as_str() {
                "BlurAmount" => 0,
                "Optimization" => 1,
                "BorderMode" => 2,
                _ => return Err(E_INVALIDARG.into()),
            };
            if index.is_null() || mapping.is_null() {
                return Err(E_INVALIDARG.into());
            }
            *index = i;
            *mapping = windows::Win32::System::WinRT::Graphics::Direct2D::GRAPHICS_EFFECT_PROPERTY_MAPPING_DIRECT;
        }
        Ok(())
    }
    fn GetPropertyCount(&self) -> Result<u32> {
        Ok(3)
    }
    fn GetProperty(&self, index: u32) -> Result<windows::Foundation::IPropertyValue> {
        use windows::Foundation::PropertyValue;
        match index {
            // Standard deviation, optimization (balanced), border mode (hard:
            // no fading at the taskbar's edges).
            0 => PropertyValue::CreateSingle(self.amount)?.cast(),
            1 => PropertyValue::CreateUInt32(1)?.cast(),
            2 => PropertyValue::CreateUInt32(1)?.cast(),
            _ => Err(E_BOUNDS.into()),
        }
    }
    fn GetSource(&self, index: u32) -> Result<windows::Graphics::Effects::IGraphicsEffectSource> {
        if index == 0 {
            Ok(self.source.clone())
        } else {
            Err(E_BOUNDS.into())
        }
    }
    fn GetSourceCount(&self) -> Result<u32> {
        Ok(1)
    }
}

/// A visual that shows a blur of whatever is behind it, sized to `shape`.
fn blur_visual(shape: &wux::Shapes::Shape) -> Result<windows::UI::Composition::SpriteVisual> {
    use windows::UI::Composition::*;
    let el: wux::UIElement = shape.cast()?;
    let comp = wux::Hosting::ElementCompositionPreview::GetElementVisual(&el)?.Compositor()?;
    let backdrop = comp.CreateBackdropBrush()?;
    let src = CompositionEffectSourceParameter::Create(&HSTRING::from("backdrop"))?;
    let effect: windows::Graphics::Effects::IGraphicsEffect = BlurEffect { source: src.cast()?, amount: 12.0 }.into();
    let factory = comp.CreateEffectFactory(&effect)?;
    let brush = factory.CreateBrush()?;
    brush.SetSourceParameter(&HSTRING::from("backdrop"), &backdrop)?;
    let sprite = comp.CreateSpriteVisual()?;
    sprite.SetBrush(&brush)?;
    sprite.SetRelativeSizeAdjustment(windows_numerics::Vector2 { X: 1.0, Y: 1.0 })?;
    Ok(sprite)
}

fn set_blur(part: &mut Part, on: bool) {
    let Ok(el) = part.shape.cast::<wux::UIElement>() else { return };
    if on && part.blur.is_none() {
        match blur_visual(&part.shape) {
            Ok(v) => {
                if wux::Hosting::ElementCompositionPreview::SetElementChildVisual(&el, &v).is_ok() {
                    part.blur = Some(v);
                } else {
                    log("taskbar: couldn't add the blur");
                }
            }
            Err(e) => log(&format!("taskbar: blur unavailable: {e}")),
        }
    } else if !on && part.blur.take().is_some() {
        let _ = wux::Hosting::ElementCompositionPreview::SetElementChildVisual(&el, None::<&windows::UI::Composition::Visual>);
    }
}

/// On the XAML thread.
fn apply_taskbar() {
    let c = cfg();
    APPLYING.with(|a| a.set(true));
    PARTS.with(|p| {
        let mut parts = p.borrow_mut();
        for part in parts.iter_mut() {
            if c.tb_on {
                // The thin line along the top goes as soon as it's see-through.
                let brush = if part.border { if c.tb_lvl > 0 { clear_brush() } else { part.original.clone() } } else { glass_brush(c) };
                if let Some(b) = brush {
                    if part.shape.SetFill(&b).is_ok() {
                        part.ours = Some(b);
                    }
                }
                if !part.border {
                    set_blur(part, c.frosted);
                }
            } else {
                set_blur(part, false);
                if part.ours.take().is_some() {
                    match &part.original {
                        Some(o) => {
                            let _ = part.shape.SetFill(o);
                        }
                        // (Never had one of its own: back to what its style says.)
                        None => {
                            if let Ok(dp) = wux::Shapes::Shape::FillProperty() {
                                let _ = part.shape.ClearValue(&dp);
                            }
                        }
                    }
                }
            }
        }
    });
    APPLYING.with(|a| a.set(false));
    if c.tb_on {
        set_tb_state(ST_BG);
    } else if is_current() {
        set_tb_state(ST_OFF);
    }
}

// ---------------------------------------------------------------------------
// Windows: File Explorer (in Explorer) and other apps (anywhere else)
//
// The way the Windows 11 File Explorer Styler and Translucent Windows mods do
// it: the window gets a see-through backdrop and DWM's frame is extended over
// the whole client area, so the alpha channel decides what shows through.
// GDI leaves alpha at 0, so painting the background colour as black makes it
// fully see-through; for a level in between, the background is filled with
// its own colour at that alpha. Text and icons are untouched. Only works
// for apps that paint with GDI and a dark theme (light text) — others get
// the whole window faded instead.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Bg,
    Whole,
}

struct WinState {
    backdrop: Option<i32>,
    layered_added: bool,
    mode: Mode,
}

static WINS: Mutex<Option<HashMap<isize, WinState>>> = Mutex::new(None);
/// Background painting is on (read by the paint hooks).
static EX_ACTIVE: AtomicBool = AtomicBool::new(false);
/// How much of the background colour stays (0–255).
static EX_ALPHA: AtomicU32 = AtomicU32::new(255);
static GLASS_FILLS: AtomicU32 = AtomicU32::new(0);
static PENDING_CHECK: AtomicBool = AtomicBool::new(false);
static CHECK_FROM: AtomicU32 = AtomicU32::new(0);
static CHECK_AT: AtomicU32 = AtomicU32::new(0);
/// Background-only didn't take in this process: whole window from now on.
static EX_NO_BG: AtomicBool = AtomicBool::new(false);

/// What this process's windows should look like.
#[derive(Clone, Copy)]
struct Want {
    on: bool,
    lvl: u32,
    whole: bool,
    frosted: bool,
}

fn want(c: Cfg) -> Want {
    if IN_EXPLORER.load(Ordering::Relaxed) {
        Want { on: c.ex_on, lvl: c.ex_lvl, whole: c.ex_whole, frosted: c.frosted }
    } else {
        // (Only File Explorer windows are changed.)
        Want { on: false, lvl: 0, whole: false, frosted: c.frosted }
    }
}

fn dark_apps() -> bool {
    unsafe {
        let mut v = 1u32;
        let mut sz = 4u32;
        let r = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut c_void),
            Some(&mut sz),
        );
        r == ERROR_SUCCESS && v == 0
    }
}

/// File Explorer windows (in Explorer), or an app's main windows and dialogs.
fn is_target_window(h: HWND) -> bool {
    unsafe {
        if !IsWindowVisible(h).as_bool() {
            return false;
        }
        let parent = GetAncestor(h, GA_PARENT);
        if !parent.is_invalid() && parent != GetDesktopWindow() {
            return false;
        }
        if IN_EXPLORER.load(Ordering::Relaxed) {
            return class_of(h) == "CabinetWClass";
        }
        let style = GetWindowLongPtrW(h, GWL_STYLE) as u32;
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        if ex & (WS_EX_NOACTIVATE.0 | WS_EX_TRANSPARENT.0 | WS_EX_TOOLWINDOW.0) != 0 {
            return false;
        }
        let caption = style & WS_CAPTION.0 == WS_CAPTION.0;
        let sysmenu = style & WS_SYSMENU.0 != 0;
        caption && sysmenu
    }
}

fn own_windows() -> Vec<HWND> {
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
        let v = &mut *(lp.0 as *mut Vec<HWND>);
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid == GetCurrentProcessId() && is_target_window(h) {
            v.push(h);
        }
        TRUE
    }
    let mut v: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    v
}

fn managed(root: HWND) -> bool {
    // (Called from paint hooks: never wait for the lock.)
    WINS.try_lock().ok().and_then(|w| w.as_ref().map(|m| m.get(&(root.0 as isize)).map(|s| s.mode == Mode::Bg).unwrap_or(false))).unwrap_or(false)
}

fn apply_windows_all() {
    let c = cfg();
    let w = want(c);
    EX_ALPHA.store(((1.0 - w.lvl as f32 / 100.0) * 255.0).round() as u32, Ordering::Relaxed);
    load_bg_colors();
    let wins = own_windows();
    unsafe {
        if w.on {
            for h in &wins {
                apply_window(*h, w);
            }
        }
        // Windows to put back: everything when off, else ones that went away.
        let keep: Vec<isize> = if w.on { wins.iter().map(|h| h.0 as isize).collect() } else { Vec::new() };
        let drop_: Vec<(isize, WinState)> = WINS
            .lock()
            .ok()
            .and_then(|mut m| m.as_mut().map(|m| {
                let gone: Vec<isize> = m.keys().copied().filter(|k| !keep.contains(k)).collect();
                gone.into_iter().filter_map(|k| m.remove(&k).map(|s| (k, s))).collect()
            }))
            .unwrap_or_default();
        for (h, st) in drop_ {
            revert_window(HWND(h as *mut c_void), st);
        }
        if !w.on {
            EX_ACTIVE.store(false, Ordering::Relaxed);
            if is_current() {
                set_ex_state(ST_OFF);
            }
        } else if wins.is_empty() {
            set_ex_state(if w.whole || EX_NO_BG.load(Ordering::Relaxed) || !dark_apps() { ST_WHOLE } else { ST_BG });
        }
    }
}

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    color: u32,
    anim: u32,
}

#[repr(C)]
struct WinCompAttrData {
    attrib: u32,
    data: *mut c_void,
    size: usize,
}

/// Blur behind the window (frosted), or off.
unsafe fn set_accent_blur(hwnd: HWND, on: bool) {
    type SetWca = unsafe extern "system" fn(HWND, *mut WinCompAttrData) -> BOOL;
    let Ok(u) = GetModuleHandleW(w!("user32.dll")) else { return };
    let Some(f) = GetProcAddress(u, s!("SetWindowCompositionAttribute")) else { return };
    let f: SetWca = std::mem::transmute(f);
    let mut ap = AccentPolicy { state: if on { 3 } else { 0 }, flags: 0, color: 0, anim: 0 };
    let mut d = WinCompAttrData { attrib: 19, data: &mut ap as *mut _ as *mut c_void, size: std::mem::size_of::<AccentPolicy>() };
    let _ = f(hwnd, &mut d);
}

/// Make the window send its frame and backdrop to DWM again.
unsafe fn composition_update(hwnd: HWND) {
    let wp = WINDOWPOS { hwnd, flags: SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE, ..Default::default() };
    let _ = SendMessageTimeoutW(hwnd, WM_WINDOWPOSCHANGED, WPARAM(0), LPARAM(&wp as *const _ as isize), SMTO_ABORTIFHUNG, 500, None);
    let _ = SendMessageTimeoutW(hwnd, WM_DWMCOMPOSITIONCHANGED, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 500, None);
}

unsafe fn apply_window(hwnd: HWND, w: Want) {
    let key = hwnd.0 as isize;
    let want_bg = !w.whole && dark_apps() && !EX_NO_BG.load(Ordering::Relaxed);
    // Window calls are never made while holding WINS: they can send messages
    // to the window's thread, which may be painting (and our paint hooks
    // look at WINS) — that would deadlock.
    let known = WINS.lock().ok().and_then(|m| m.as_ref().and_then(|m| m.get(&key).map(|s| (s.mode, s.backdrop, s.layered_added))));
    let (mut mode, backdrop, layered) = match known {
        Some(k) => k,
        None => {
            let mut b = 0i32;
            let got = DwmGetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &mut b as *mut i32 as *mut c_void, 4).is_ok();
            (if want_bg { Mode::Bg } else { Mode::Whole }, got.then_some(b), false)
        }
    };
    if want_bg && mode == Mode::Whole && known.is_some() {
        mode = Mode::Bg;
    }
    if !want_bg && mode == Mode::Bg {
        mode = Mode::Whole;
        if known.is_some() {
            undo_backdrop(hwnd, backdrop);
        }
    }
    let store = |layered_added: bool| {
        if let Ok(mut m) = WINS.lock() {
            m.get_or_insert_with(HashMap::new).insert(key, WinState { backdrop, layered_added, mode });
        }
    };
    match mode {
        Mode::Bg => {
            let layered = if layered { layer(hwnd, None, true) } else { false };
            store(layered);
            ensure_paint_hooks();
            EX_ACTIVE.store(true, Ordering::Relaxed);
            // Frosted: blur behind; clear: no backdrop at all.
            let b: i32 = if w.frosted { 0 } else { 1 };
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &b as *const i32 as *const c_void, 4);
            set_accent_blur(hwnd, w.frosted);
            let m = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
            let _ = DwmExtendFrameIntoClientArea(hwnd, &m);
            let _ = RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_ERASE | RDW_FRAME);
            // Did our painting take? Checked shortly (see check_fallback).
            if !PENDING_CHECK.swap(true, Ordering::Relaxed) {
                CHECK_FROM.store(GLASS_FILLS.load(Ordering::Relaxed), Ordering::Relaxed);
                CHECK_AT.store(tick(), Ordering::Relaxed);
            }
            set_ex_state(ST_BG);
        }
        Mode::Whole => {
            let layered = layer(hwnd, Some(whole_alpha(w.lvl)), layered);
            store(layered);
            set_ex_state(ST_WHOLE);
        }
    }
}

/// The window's own backdrop and frame back.
unsafe fn undo_backdrop(hwnd: HWND, backdrop: Option<i32>) {
    set_accent_blur(hwnd, false);
    if let Some(b) = backdrop {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &b as *const i32 as *const c_void, 4);
    }
    // The app sets its own frame margins again on these.
    let m = MARGINS::default();
    let _ = DwmExtendFrameIntoClientArea(hwnd, &m);
    composition_update(hwnd);
    let _ = RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_ERASE | RDW_FRAME);
}

unsafe fn revert_window(hwnd: HWND, st: WinState) {
    if !IsWindow(Some(hwnd)).as_bool() {
        return;
    }
    if st.layered_added {
        layer(hwnd, None, true);
    }
    if st.mode == Mode::Bg {
        undo_backdrop(hwnd, st.backdrop);
    }
}

/// A few seconds after switching background-only on: if no background was
/// painted see-through, this app draws differently — fade whole windows.
fn check_fallback() {
    if !PENDING_CHECK.load(Ordering::Relaxed) || tick().saturating_sub(CHECK_AT.load(Ordering::Relaxed)) < 3 {
        return;
    }
    PENDING_CHECK.store(false, Ordering::Relaxed);
    let n = GLASS_FILLS.load(Ordering::Relaxed);
    if n != CHECK_FROM.load(Ordering::Relaxed) {
        log(&format!("windows: background-only painting works ({n} fills so far)"));
        return;
    }
    let visible = own_windows().into_iter().any(|h| unsafe { !IsIconic(h).as_bool() });
    if !visible {
        PENDING_CHECK.store(true, Ordering::Relaxed);
        CHECK_AT.store(tick(), Ordering::Relaxed);
        return;
    }
    log(&format!("windows: no background was painted see-through (fills {}, patched {}): fading whole windows instead", n, PATCHED.load(Ordering::Relaxed)));
    EX_NO_BG.store(true, Ordering::Relaxed);
    EX_ACTIVE.store(false, Ordering::Relaxed);
    apply_windows_all();
}

// ----- Background colours -----

/// The colours windows paint their content background with: the window
/// colour, and in dark mode the fill colours of the theme's DarkMode parts.
static BG_COLORS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

fn load_bg_colors() {
    use windows::Win32::UI::Controls::*;
    let mut v: Vec<u32> = Vec::new();
    unsafe {
        v.push(GetSysColor(COLOR_WINDOW));
        if !IN_EXPLORER.load(Ordering::Relaxed) {
            v.push(GetSysColor(COLOR_BTNFACE));
        }
        let parts: [(PCWSTR, i32); 7] = [
            (w!("DarkMode::ItemsView"), 0),
            (w!("DarkMode::ExplorerNavPane"), 0),
            (w!("DarkMode::ExplorerStatusBar"), 0),
            (w!("DarkMode::ReadingPane"), 1),
            (w!("DarkMode::ProperTree"), 2),
            (w!("DarkMode_Explorer::TreeView"), 0),
            (w!("DarkMode_ItemsView::ListView"), 0),
        ];
        for (class, part) in parts {
            let t = OpenThemeData(None, class);
            if t.is_invalid() {
                continue;
            }
            // TMT_FILLCOLOR
            if let Ok(c) = GetThemeColor(t, part, 0, THEME_PROPERTY_SYMBOL_ID(3802)) {
                if !v.contains(&c.0) {
                    v.push(c.0);
                }
            }
            let _ = CloseThemeData(t);
        }
    }
    // Only dark backgrounds: the app's text is light then, which stays
    // visible on a see-through background (dark text wouldn't). A light app
    // ends up faded as a whole window instead.
    let luma = |c: u32| (299 * (c & 0xff) + 587 * ((c >> 8) & 0xff) + 114 * ((c >> 16) & 0xff)) / 1000;
    v.retain(|c| luma(*c) < 110);
    if let Ok(mut b) = BG_COLORS.lock() {
        if *b != v {
            log(&format!("windows: background colours {}", v.iter().map(|c| format!("#{:02x}{:02x}{:02x}", c & 0xff, (c >> 8) & 0xff, (c >> 16) & 0xff)).collect::<Vec<_>>().join(" ")));
        }
        *b = v;
    }
}

fn is_bg_color(c: u32) -> bool {
    BG_COLORS.try_lock().map(|b| b.contains(&c)).unwrap_or(false)
}

// ----- Paint hooks (import table patches) -----

type FillRectFn = unsafe extern "system" fn(HDC, *const RECT, HBRUSH) -> i32;
type PatBltFn = unsafe extern "system" fn(HDC, i32, i32, i32, i32, u32) -> BOOL;
type BeginPaintFn = unsafe extern "system" fn(HWND, *mut PAINTSTRUCT) -> HDC;
type ExtTextOutFn = unsafe extern "system" fn(HDC, i32, i32, u32, *const RECT, PCWSTR, u32, *const i32) -> BOOL;
type EndPaintFn = unsafe extern "system" fn(HWND, *const PAINTSTRUCT) -> BOOL;

static ORIG_FILL: AtomicIsize = AtomicIsize::new(0);
static ORIG_PATBLT: AtomicIsize = AtomicIsize::new(0);
static ORIG_BEGIN: AtomicIsize = AtomicIsize::new(0);
static ORIG_END: AtomicIsize = AtomicIsize::new(0);
static ORIG_TEXT: AtomicIsize = AtomicIsize::new(0);
static PATCHED: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// The windows being painted on this thread (BeginPaint … EndPaint),
    /// innermost last, and whether each is one of ours.
    static PAINTING: RefCell<Vec<(isize, bool)>> = const { RefCell::new(Vec::new()) };
}

/// Point every loaded module's imports of FillRect / PatBlt / BeginPaint /
/// EndPaint at ours (again: modules loaded since, and delay-loaded imports
/// resolved since, are picked up too).
unsafe fn ensure_paint_hooks() {
    let (Ok(u), Ok(g)) = (GetModuleHandleW(w!("user32.dll")), GetModuleHandleW(w!("gdi32.dll"))) else { return };
    let get = |m: HMODULE, n: PCSTR| GetProcAddress(m, n).map(|f| f as usize).unwrap_or(0);
    let (fill, begin, end, patblt, text) = (get(u, s!("FillRect")), get(u, s!("BeginPaint")), get(u, s!("EndPaint")), get(g, s!("PatBlt")), get(g, s!("ExtTextOutW")));
    if fill == 0 || begin == 0 || end == 0 || patblt == 0 || text == 0 {
        log("windows: user32 / gdi32 functions not found");
        return;
    }
    ORIG_FILL.store(fill as isize, Ordering::Relaxed);
    ORIG_BEGIN.store(begin as isize, Ordering::Relaxed);
    ORIG_END.store(end as isize, Ordering::Relaxed);
    ORIG_PATBLT.store(patblt as isize, Ordering::Relaxed);
    ORIG_TEXT.store(text as isize, Ordering::Relaxed);
    let pairs = [(fill, h_fill_rect as usize), (begin, h_begin_paint as usize), (end, h_end_paint as usize), (patblt, h_patblt as usize), (text, h_ext_text_out as usize)];
    let me = SELF.load(Ordering::Relaxed);
    let mut mods = vec![HMODULE::default(); 2048];
    let mut need = 0u32;
    let _ = windows::Win32::System::ProcessStatus::K32EnumProcessModules(GetCurrentProcess(), mods.as_mut_ptr(), (mods.len() * std::mem::size_of::<HMODULE>()) as u32, &mut need);
    let n = (need as usize / std::mem::size_of::<HMODULE>()).min(mods.len());
    let mut patched = 0;
    for m in &mods[..n] {
        if m.0 as isize == me {
            continue;
        }
        patched += patch_iat(m.0 as *mut u8, &pairs);
    }
    if patched > 0 {
        let total = PATCHED.fetch_add(patched as u32, Ordering::Relaxed) + patched as u32;
        log(&format!("windows: paint hooks set ({patched} more imports, {total} in all)"));
    }
}

/// Point a module's imports (normal and delay-loaded) of the given
/// functions at ours.
unsafe fn patch_iat(base: *mut u8, pairs: &[(usize, usize)]) -> usize {
    let rd32 = |p: *const u8| std::ptr::read_unaligned(p as *const u32);
    if std::ptr::read_unaligned(base as *const u16) != 0x5A4D {
        return 0;
    }
    let nt = base.add(rd32(base.add(0x3C)) as usize);
    if rd32(nt) != 0x4550 || std::ptr::read_unaligned(nt.add(24) as *const u16) != 0x20B {
        return 0;
    }
    let patch_table = |first: usize| -> usize {
        let mut n = 0;
        let mut slot = base.add(first) as *mut usize;
        for _ in 0..8192 {
            let v = std::ptr::read_volatile(slot);
            if v == 0 {
                break;
            }
            if let Some(&(_, to)) = pairs.iter().find(|(from, _)| *from == v) {
                let mut old = PAGE_PROTECTION_FLAGS(0);
                if VirtualProtect(slot as *const c_void, 8, PAGE_READWRITE, &mut old).is_ok() {
                    std::ptr::write_volatile(slot, to);
                    let _ = VirtualProtect(slot as *const c_void, 8, old, &mut old);
                    n += 1;
                }
            }
            slot = slot.add(1);
        }
        n
    };
    let mut n = 0;
    // Imports (data directory 1).
    let imp_rva = rd32(nt.add(24 + 112 + 8)) as usize;
    if imp_rva != 0 {
        let mut desc = base.add(imp_rva);
        for _ in 0..1024 {
            let name = rd32(desc.add(12));
            let first = rd32(desc.add(16)) as usize;
            if name == 0 || first == 0 {
                break;
            }
            n += patch_table(first);
            desc = desc.add(20);
        }
    }
    // Delay-loaded imports (data directory 13): already resolved entries.
    let dl_rva = rd32(nt.add(24 + 112 + 13 * 8)) as usize;
    if dl_rva != 0 {
        let mut desc = base.add(dl_rva);
        for _ in 0..1024 {
            let name = rd32(desc.add(4));
            let iat = rd32(desc.add(12)) as usize;
            if name == 0 || iat == 0 {
                break;
            }
            n += patch_table(iat);
            desc = desc.add(32);
        }
    }
    n
}

/// Is this DC drawing for one of our see-through windows?
unsafe fn dc_ours(hdc: HDC) -> bool {
    let w = WindowFromDC(hdc);
    if !w.is_invalid() {
        return managed(GetAncestor(w, GA_ROOT));
    }
    // A memory DC (double buffering): the window being painted.
    PAINTING.with(|p| p.try_borrow().ok().and_then(|p| p.last().map(|x| x.1)).unwrap_or(false))
}

unsafe extern "system" fn h_begin_paint(hwnd: HWND, ps: *mut PAINTSTRUCT) -> HDC {
    // Pushed before the call: BeginPaint sends WM_ERASEBKGND.
    let mut pushed = false;
    if EX_ACTIVE.load(Ordering::Relaxed) {
        let ours = std::panic::catch_unwind(|| managed(GetAncestor(hwnd, GA_ROOT))).unwrap_or(false);
        PAINTING.with(|p| {
            if let Ok(mut p) = p.try_borrow_mut() {
                if p.len() < 16 {
                    p.push((hwnd.0 as isize, ours));
                    pushed = true;
                }
            }
        });
    }
    let f: BeginPaintFn = std::mem::transmute(ORIG_BEGIN.load(Ordering::Relaxed));
    let dc = f(hwnd, ps);
    if dc.is_invalid() && pushed {
        pop_painting(hwnd);
    }
    dc
}

fn pop_painting(hwnd: HWND) {
    PAINTING.with(|p| {
        if let Ok(mut p) = p.try_borrow_mut() {
            if let Some(i) = p.iter().rposition(|x| x.0 == hwnd.0 as isize) {
                p.truncate(i);
            }
        }
    });
}

unsafe extern "system" fn h_end_paint(hwnd: HWND, ps: *const PAINTSTRUCT) -> BOOL {
    pop_painting(hwnd);
    let f: EndPaintFn = std::mem::transmute(ORIG_END.load(Ordering::Relaxed));
    f(hwnd, ps)
}

unsafe fn brush_color(hdc: HDC, hbr: HBRUSH) -> Option<u32> {
    let v = hbr.0 as usize;
    if v != 0 && v <= 31 {
        // COLOR_xxx + 1
        return Some(GetSysColor(SYS_COLOR_INDEX(v as i32 - 1)));
    }
    if hbr.0 == GetStockObject(DC_BRUSH).0 {
        return Some(GetDCBrushColor(hdc).0);
    }
    let mut lb = LOGBRUSH::default();
    let n = GetObjectW(HGDIOBJ(hbr.0), std::mem::size_of::<LOGBRUSH>() as i32, Some(&mut lb as *mut _ as *mut c_void));
    (n as usize == std::mem::size_of::<LOGBRUSH>() && lb.lbStyle == BS_SOLID).then_some(lb.lbColor.0)
}

/// Is this window an edit box (the caret only inverts colour, so its
/// background stays as it is)?
unsafe fn is_edit(hdc: HDC) -> bool {
    let w = WindowFromDC(hdc);
    !w.is_invalid() && class_of(w).eq_ignore_ascii_case("Edit")
}

unsafe extern "system" fn h_fill_rect(hdc: HDC, rc: *const RECT, hbr: HBRUSH) -> i32 {
    let f: FillRectFn = std::mem::transmute(ORIG_FILL.load(Ordering::Relaxed));
    if !EX_ACTIVE.load(Ordering::Relaxed) || rc.is_null() {
        return f(hdc, rc, hbr);
    }
    let done = std::panic::catch_unwind(|| {
        if !dc_ours(hdc) || is_edit(hdc) {
            return false;
        }
        match brush_color(hdc, hbr) {
            Some(c) if is_bg_color(c) => glass_fill(hdc, &*rc, c, f),
            _ => false,
        }
    });
    if done.unwrap_or(false) {
        1
    } else {
        f(hdc, rc, hbr)
    }
}

unsafe extern "system" fn h_patblt(hdc: HDC, x: i32, y: i32, w: i32, h: i32, rop: u32) -> BOOL {
    let f: PatBltFn = std::mem::transmute(ORIG_PATBLT.load(Ordering::Relaxed));
    if !EX_ACTIVE.load(Ordering::Relaxed) || rop != PATCOPY.0 {
        return f(hdc, x, y, w, h, rop);
    }
    let done = std::panic::catch_unwind(|| {
        if !dc_ours(hdc) || is_edit(hdc) {
            return false;
        }
        let hbr = HBRUSH(GetCurrentObject(hdc, OBJ_BRUSH).0);
        match brush_color(hdc, hbr) {
            Some(c) if is_bg_color(c) => glass_fill(hdc, &RECT { left: x, top: y, right: x + w, bottom: y + h }, c, std::mem::transmute::<isize, FillRectFn>(ORIG_FILL.load(Ordering::Relaxed))),
            _ => false,
        }
    });
    if done.unwrap_or(false) {
        TRUE
    } else {
        f(hdc, x, y, w, h, rop)
    }
}

/// Fill with the background colour at our level: transparent black when
/// fully see-through, otherwise the colour with that alpha (premultiplied,
/// through a buffered paint, as the File Explorer Styler does).
unsafe fn glass_fill(hdc: HDC, r: &RECT, col: u32, f: FillRectFn) -> bool {
    use windows::Win32::UI::Controls::*;
    let a = EX_ALPHA.load(Ordering::Relaxed).min(255);
    if a >= 255 {
        return false;
    }
    let black = HBRUSH(GetStockObject(BLACK_BRUSH).0);
    if a == 0 {
        f(hdc, r, black);
        GLASS_FILLS.fetch_add(1, Ordering::Relaxed);
        return true;
    }
    let blend = BLENDFUNCTION { BlendOp: 0, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: 1 };
    let params = BP_PAINTPARAMS { cbSize: std::mem::size_of::<BP_PAINTPARAMS>() as u32, pBlendFunction: &blend, ..Default::default() };
    // The area underneath must not show through the blend: clear it first.
    f(hdc, r, black);
    let mut mem = HDC::default();
    let hpb = BeginBufferedPaint(hdc, r, BPBF_TOPDOWNDIB, Some(&params), &mut mem);
    if hpb == 0 {
        return true;
    }
    let pm = |c: u32| c * a / 255;
    let brush = CreateSolidBrush(COLORREF(pm(col & 0xff) | (pm((col >> 8) & 0xff) << 8) | (pm((col >> 16) & 0xff) << 16)));
    f(mem, r, brush);
    let _ = DeleteObject(HGDIOBJ(brush.0));
    let _ = BufferedPaintSetAlpha(hpb, Some(r), a as u8);
    let _ = EndBufferedPaint(hpb, true);
    GLASS_FILLS.fetch_add(1, Ordering::Relaxed);
    true
}

/// Text drawn with an opaque background of the window's background colour
/// (list items, labels): that background goes see-through too; the text
/// itself is drawn as before.
unsafe extern "system" fn h_ext_text_out(hdc: HDC, x: i32, y: i32, options: u32, rc: *const RECT, text: PCWSTR, n: u32, dx: *const i32) -> BOOL {
    let f: ExtTextOutFn = std::mem::transmute(ORIG_TEXT.load(Ordering::Relaxed));
    if !EX_ACTIVE.load(Ordering::Relaxed) {
        return f(hdc, x, y, options, rc, text, n, dx);
    }
    const ETO_OPAQUE: u32 = 2;
    let bk = GetBkColor(hdc).0;
    let ours = std::panic::catch_unwind(|| dc_ours(hdc) && !is_edit(hdc) && is_bg_color(bk)).unwrap_or(false);
    if !ours {
        return f(hdc, x, y, options, rc, text, n, dx);
    }
    let mut opts = options;
    if options & ETO_OPAQUE != 0 && !rc.is_null() {
        let fill: FillRectFn = std::mem::transmute(ORIG_FILL.load(Ordering::Relaxed));
        if glass_fill(hdc, &*rc, bk, fill) {
            opts &= !ETO_OPAQUE;
        }
    }
    // An opaque background mode would fill each character cell too.
    let mode = GetBkMode(hdc);
    if mode == OPAQUE.0 as i32 {
        SetBkMode(hdc, TRANSPARENT);
    }
    let r = f(hdc, x, y, opts, rc, text, n, dx);
    if mode == OPAQUE.0 as i32 {
        SetBkMode(hdc, OPAQUE);
    }
    r
}
