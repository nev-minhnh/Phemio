//! The Windows side of Link: watching this PC's mouse and keyboard (and
//! sending them over while the pointer is on the other PC), and playing the
//! other PC's input back here.
//!
//! Low-level hooks on their own thread see every pointer move before Windows
//! applies it. On this PC the shared map decides where the pointer goes; when
//! it crosses to the other PC this PC's pointer is parked, hidden, and each
//! move / click / key is sent instead of used here.
//!
//! Safety: nothing stays pressed on the other PC when the pointer comes back
//! or the link drops; if the other PC stops answering, the pointer comes home
//! by itself; Scroll Lock always brings it home; the hidden pointer is
//! restored on exit and at the next start.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard};

use windows::Win32::Foundation::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::geom::{Geo, Local, Remote};
use super::proto::Msg;

static GEO: Mutex<Option<Arc<Geo>>> = Mutex::new(None);
static OUT: Mutex<Option<Sender<Msg>>> = Mutex::new(None);
static CAP: Mutex<Capture> = Mutex::new(Capture { on: false, screen: 0, p: (0.0, 0.0), park: (0, 0), park_scale: 1.0, last: None });
static THREAD: AtomicU32 = AtomicU32::new(0);
/// A nearly invisible window under the parked pointer while it's on the other
/// PC. A touchpad's two-finger scroll never passes through the mouse hook —
/// Windows sends it straight to the window under the pointer — so this
/// catches it there and sends it over instead of scrolling this PC.
static CATCHER: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
const CATCH_SIZE: i32 = 240;
/// The other PC is driving this one right now.
pub static DRIVEN: AtomicBool = AtomicBool::new(false);
/// Something to tell the UI about (the pointer crossed).
static NOTIFY: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

struct Capture {
    /// The pointer is on the other PC.
    on: bool,
    /// Where on the map (screen index, point).
    screen: usize,
    p: (f32, f32),
    /// Where this PC's (hidden) pointer waits, and that screen's scaling.
    park: (i32, i32),
    park_scale: f32,
    /// The last pointer position seen here (to tell which way it moved).
    last: Option<(i32, i32)>,
}

/// A lock that keeps working even if a thread once panicked holding it.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn set_geo(g: Geo) {
    *lock(&GEO) = Some(Arc::new(g));
}

pub fn set_notify(f: Box<dyn Fn() + Send>) {
    *lock(&NOTIFY) = Some(f);
}

fn notify() {
    if let Some(f) = lock(&NOTIFY).as_ref() {
        f();
    }
}

fn send(m: Msg) {
    if let Some(tx) = lock(&OUT).as_ref() {
        let _ = tx.send(m);
    }
}

/// Where the pointer is on the map while it's on the other PC.
pub fn captured() -> Option<(usize, (f32, f32))> {
    let c = lock(&CAP);
    c.on.then_some((c.screen, c.p))
}

/// Start watching (a session began): input goes to `tx` while the pointer is over there.
pub fn start(tx: Sender<Msg>) {
    *lock(&OUT) = Some(tx);
    if THREAD.load(Ordering::SeqCst) != 0 {
        return;
    }
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new().name("link-input".into()).spawn(move || unsafe {
        use windows::Win32::System::Threading::*;
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        let inst = crate::util::hinstance();
        let m = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(inst.into()), 0);
        let k = SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_proc), Some(inst.into()), 0);
        if m.is_err() || k.is_err() {
            crate::util::log("link: couldn't watch the mouse / keyboard");
        }
        THREAD.store(windows::Win32::System::Threading::GetCurrentThreadId(), Ordering::SeqCst);
        make_catcher();
        let _ = ready_tx.send(());
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let cw = CATCHER.swap(0, Ordering::SeqCst);
        if cw != 0 {
            let _ = DestroyWindow(HWND(cw as *mut _));
        }
        if let Ok(h) = m {
            let _ = UnhookWindowsHookEx(h);
        }
        if let Ok(h) = k {
            let _ = UnhookWindowsHookEx(h);
        }
        let me = windows::Win32::System::Threading::GetCurrentThreadId();
        let _ = THREAD.compare_exchange(me, 0, Ordering::SeqCst, Ordering::SeqCst);
    });
    let _ = ready_rx.recv_timeout(std::time::Duration::from_secs(2));
}

/// The session ended: bring the pointer home, let go of everything, stop watching.
pub fn stop() {
    come_home(None, false);
    release_all();
    DRIVEN.store(false, Ordering::SeqCst);
    *lock(&OUT) = None;
    let t = THREAD.swap(0, Ordering::SeqCst);
    if t != 0 {
        unsafe {
            let _ = PostThreadMessageW(t, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

unsafe fn make_catcher() {
    use windows::core::w;
    static REG: std::sync::Once = std::sync::Once::new();
    let class = w!("PhemioLinkCatcher");
    REG.call_once(|| crate::util::register_class(class, Some(catcher_proc), HICON::default()));
    if let Ok(h) = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
        class,
        w!(""),
        WS_POPUP,
        0,
        0,
        CATCH_SIZE,
        CATCH_SIZE,
        None,
        None,
        Some(crate::util::hinstance()),
        None,
    ) {
        // Alpha 1 of 255: invisible, yet it still receives the pointer's messages.
        let _ = SetLayeredWindowAttributes(h, COLORREF(0), 1, LWA_ALPHA);
        CATCHER.store(h.0 as isize, Ordering::SeqCst);
    }
}

/// Put the catcher under the parked pointer (or take it away).
fn place_catcher(at: Option<(i32, i32)>) {
    let h = CATCHER.load(Ordering::SeqCst);
    if h == 0 {
        return;
    }
    let h = HWND(h as *mut _);
    unsafe {
        match at {
            Some((x, y)) => {
                let _ = SetWindowPos(h, Some(HWND_TOPMOST), x - CATCH_SIZE / 2, y - CATCH_SIZE / 2, CATCH_SIZE, CATCH_SIZE, SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_ASYNCWINDOWPOS);
            }
            None => {
                let _ = SetWindowPos(h, None, 0, 0, 0, 0, SWP_NOACTIVATE | SWP_HIDEWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_ASYNCWINDOWPOS);
            }
        }
    }
}

unsafe extern "system" fn catcher_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            if lock(&CAP).on {
                send(Msg::Wheel { delta: ((wp.0 >> 16) & 0xffff) as u16 as i16, horizontal: msg == WM_MOUSEHWHEEL });
            }
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// The pointer comes back to this PC: at `to` (pixel), or where it left.
/// `tell`: let the other PC know (it lets go of anything held).
pub fn come_home(to: Option<(i32, i32)>, tell: bool) {
    let park = {
        let mut c = lock(&CAP);
        if !c.on {
            return;
        }
        c.on = false;
        c.last = to;
        c.park
    };
    place_catcher(None);
    show_pointer();
    let (x, y) = to.unwrap_or(park);
    unsafe {
        let _ = SetCursorPos(x, y);
    }
    if tell {
        send(Msg::Leave);
    }
    super::crossed(false);
    notify();
}

/// "Go to the other PC": put the pointer on the middle of screen `i` there.
pub fn jump_to(i: usize) {
    let Some(g) = lock(&GEO).clone() else { return };
    let Some(s) = g.screens.get(i) else { return };
    if s.machine == g.me {
        return;
    }
    let (l, t, r, b) = s.grect();
    let p = ((l + r) / 2.0, (t + b) / 2.0);
    let (x, y) = s.to_px(p);
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    let here = g.local_at_px(pt.x, pt.y).or_else(|| g.screens.iter().position(|s| s.machine == g.me));
    if let Some(h) = here {
        go_over(&g, h, i, p, x, y);
    }
}

/// Start driving the other PC: park and hide this pointer, tell it where to appear.
fn go_over(g: &Geo, from: usize, to: usize, p: (f32, f32), x: i32, y: i32) {
    let Some(fs) = g.screens.get(from).or_else(|| g.screens.iter().find(|s| s.machine == g.me)) else { return };
    if g.screens.get(to).is_none() {
        return;
    }
    let park = (fs.px.0 + fs.px.2 / 2, fs.px.1 + fs.px.3 / 2);
    {
        let mut c = lock(&CAP);
        c.on = true;
        c.screen = to;
        c.p = p;
        c.park = park;
        c.park_scale = fs.scale.max(0.5);
        c.last = None;
    }
    hide_pointer();
    unsafe {
        let _ = SetCursorPos(park.0, park.1);
    }
    place_catcher(Some(park));
    send(Msg::Enter { x, y });
    super::crossed(true);
    notify();
}

/// Hook procedures must never unwind into Windows: a panic in one just lets
/// the event through (and is logged by the panic hook).
fn guarded(f: impl FnOnce() -> Option<LRESULT>) -> Option<LRESULT> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or(None)
}

unsafe extern "system" fn mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return CallNextHookEx(None, code, wp, lp);
    }
    match guarded(|| mouse_inner(code, wp, lp)) {
        Some(r) => r,
        None => CallNextHookEx(None, code, wp, lp),
    }
}

/// Some(result) when we handled the event; None lets it through.
unsafe fn mouse_inner(code: i32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    let info = &*(lp.0 as *const MSLLHOOKSTRUCT);
    // Input the other PC is playing back here: leave it alone.
    let _ = code;
    if info.flags & LLMHF_INJECTED != 0 {
        return None;
    }
    // Your own mouse moved while the other PC was driving: it's yours again.
    if DRIVEN.swap(false, Ordering::SeqCst) {
        send(Msg::Take);
        notify();
    }
    let pt = (info.pt.x, info.pt.y);
    let msg = wp.0 as u32;
    let geo = lock(&GEO).clone()?;
    let mut cap = lock(&CAP);
    if cap.on {
        match msg {
            WM_MOUSEMOVE => {
                let d = (pt.0 - cap.park.0, pt.1 - cap.park.1);
                if d != (0, 0) {
                    let dg = (d.0 as f32 / cap.park_scale, d.1 as f32 / cap.park_scale);
                    match geo.remote_move(cap.screen, cap.p, dg) {
                        Remote::Move(s, p, x, y) => {
                            cap.screen = s;
                            cap.p = p;
                            drop(cap);
                            send(Msg::Move { x, y });
                        }
                        Remote::Exit(_, x, y) => {
                            drop(cap);
                            come_home(Some((x, y)), true);
                        }
                    }
                }
            }
            WM_LBUTTONDOWN | WM_LBUTTONUP => send(Msg::Button { b: 0, down: msg == WM_LBUTTONDOWN }),
            WM_RBUTTONDOWN | WM_RBUTTONUP => send(Msg::Button { b: 1, down: msg == WM_RBUTTONDOWN }),
            WM_MBUTTONDOWN | WM_MBUTTONUP => send(Msg::Button { b: 2, down: msg == WM_MBUTTONDOWN }),
            WM_XBUTTONDOWN | WM_XBUTTONUP => {
                let which = (info.mouseData >> 16) as u16;
                send(Msg::Button { b: if which == 1 { 3 } else { 4 }, down: msg == WM_XBUTTONDOWN });
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => send(Msg::Wheel { delta: (info.mouseData >> 16) as u16 as i16, horizontal: msg == WM_MOUSEHWHEEL }),
            _ => {}
        }
        // Over there: nothing happens here.
        return Some(LRESULT(1));
    }
    if msg != WM_MOUSEMOVE {
        return None;
    }
    let last = cap.last.unwrap_or_else(|| {
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        (p.x, p.y)
    });
    match geo.local_move(last, pt) {
        Local::Pass => {
            cap.last = Some(pt);
            None
        }
        Local::Warp(_, x, y) => {
            cap.last = Some((x, y));
            drop(cap);
            let _ = SetCursorPos(x, y);
            Some(LRESULT(1))
        }
        Local::Enter(t, p, x, y) => {
            let from = geo.local_at_px(last.0, last.1).or_else(|| geo.local_at_px(pt.0, pt.1)).unwrap_or(0);
            drop(cap);
            go_over(&geo, from, t, p, x, y);
            Some(LRESULT(1))
        }
    }
}

unsafe extern "system" fn key_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return CallNextHookEx(None, code, wp, lp);
    }
    match guarded(|| key_inner(lp)) {
        Some(r) => r,
        None => CallNextHookEx(None, code, wp, lp),
    }
}

unsafe fn key_inner(lp: LPARAM) -> Option<LRESULT> {
    let info = &*(lp.0 as *const KBDLLHOOKSTRUCT);
    if info.flags.0 & LLKHF_INJECTED.0 != 0 {
        return None;
    }
    if DRIVEN.swap(false, Ordering::SeqCst) {
        send(Msg::Take);
        notify();
    }
    if !lock(&CAP).on {
        return None;
    }
    let up = info.flags.0 & LLKHF_UP.0 != 0;
    // Power, sleep and wake keys stay on this PC (never sent over).
    if is_power_key(info.vkCode as u16, info.scanCode as u16, info.flags.0 & LLKHF_EXTENDED.0 != 0) {
        return None;
    }
    // Scroll Lock: always brings the pointer home.
    if info.vkCode == VK_SCROLL.0 as u32 {
        if !up {
            come_home(None, true);
        }
        return Some(LRESULT(1));
    }
    let mut flags = 0u8;
    if info.flags.0 & LLKHF_EXTENDED.0 != 0 {
        flags |= 1;
    }
    if up {
        flags |= 2;
    }
    send(Msg::Key { vk: info.vkCode as u16, scan: info.scanCode as u16, flags });
    Some(LRESULT(1))
}

// ---------------------------------------------------------------------------
// Hiding this PC's pointer while it's over there
// ---------------------------------------------------------------------------

static HIDDEN: AtomicBool = AtomicBool::new(false);
/// The pointer-hiding worker (Windows takes a moment to swap every cursor,
/// far too long for a mouse hook, so it's done off the hook thread).
static CURSOR_TX: Mutex<Option<Sender<bool>>> = Mutex::new(None);

const CURSORS: [u32; 14] = [32512, 32513, 32514, 32515, 32516, 32642, 32643, 32644, 32645, 32646, 32648, 32649, 32650, 32651];

fn cursor_worker() -> Sender<bool> {
    let mut g = lock(&CURSOR_TX);
    if let Some(tx) = g.as_ref() {
        return tx.clone();
    }
    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    let _ = std::thread::Builder::new().name("link-cursor".into()).spawn(move || {
        let mut shown = true;
        while let Ok(mut want_hidden) = rx.recv() {
            // Only the latest wish counts.
            while let Ok(w) = rx.try_recv() {
                want_hidden = w;
            }
            if want_hidden == !shown {
                continue;
            }
            unsafe {
                if want_hidden {
                    let and = [0xffu8; 32 * 4];
                    let xor = [0u8; 32 * 4];
                    for id in CURSORS {
                        if let Ok(c) = CreateCursor(Some(crate::util::hinstance()), 0, 0, 32, 32, and.as_ptr() as *const _, xor.as_ptr() as *const _) {
                            let _ = SetSystemCursor(c, SYSTEM_CURSOR_ID(id));
                        }
                    }
                } else {
                    let _ = SystemParametersInfoW(SPI_SETCURSORS, 0, None, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
                }
            }
            shown = !want_hidden;
        }
    });
    *g = Some(tx.clone());
    tx
}

fn hide_pointer() {
    if HIDDEN.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = cursor_worker().send(true);
}

/// Put Windows' pointers back.
pub fn show_pointer() {
    if HIDDEN.swap(false, Ordering::SeqCst) {
        let _ = cursor_worker().send(false);
    }
}

/// Put Windows' pointers back right now (start-up, exit, a crash), in case
/// the app stopped while the pointer was hidden.
pub fn show_pointer_now() {
    HIDDEN.store(false, Ordering::SeqCst);
    unsafe {
        let _ = SystemParametersInfoW(SPI_SETCURSORS, 0, None, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
}


// ---------------------------------------------------------------------------
// Playing the other PC's input back here
// ---------------------------------------------------------------------------

/// Keys and buttons pressed here on the other PC's behalf (to let go of).
static HELD: Mutex<Vec<(bool, u16, u16, u8)>> = Mutex::new(Vec::new());

fn input_mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) {
    let i = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 } } };
    unsafe {
        SendInput(&[i], std::mem::size_of::<INPUT>() as i32);
    }
}

/// The pointer, at a pixel of this PC's desktop.
pub fn play_move(x: i32, y: i32) {
    DRIVEN.store(true, Ordering::SeqCst);
    unsafe {
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2);
        let nx = (((x - vx) as i64 * 65535 + (vw as i64 - 1) / 2) / (vw as i64 - 1)) as i32;
        let ny = (((y - vy) as i64 * 65535 + (vh as i64 - 1) / 2) / (vh as i64 - 1)) as i32;
        input_mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, nx, ny, 0);
    }
}

pub fn play_button(b: u8, down: bool) {
    let (flags, data) = match (b, down) {
        (0, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (0, false) => (MOUSEEVENTF_LEFTUP, 0),
        (1, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (1, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (2, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (2, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (3 | 4, d) => (if d { MOUSEEVENTF_XDOWN } else { MOUSEEVENTF_XUP }, if b == 3 { 1 } else { 2 }),
        _ => return,
    };
    track(true, b as u16, 0, 0, down);
    input_mouse(flags, 0, 0, data);
}

pub fn play_wheel(delta: i16, horizontal: bool) {
    input_mouse(if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL }, 0, 0, delta as i32);
}

/// Sleep / power / wake: keys that must never be pressed on the other PC's behalf.
fn is_power_key(vk: u16, scan: u16, ext: bool) -> bool {
    vk == VK_SLEEP.0 || (ext && matches!(scan, 0x5E | 0x5F | 0x63)) || (vk == 0 && matches!(scan, 0x5E | 0x5F | 0x63))
}

pub fn play_key(vk: u16, scan: u16, flags: u8) {
    if is_power_key(vk, scan, flags & 1 != 0) {
        return;
    }
    let up = flags & 2 != 0;
    track(false, vk, scan, flags & 1, !up);
    key_event(vk, scan, flags);
}

fn key_event(vk: u16, scan: u16, flags: u8) {
    let mut f = KEYBD_EVENT_FLAGS(0);
    if flags & 1 != 0 {
        f |= KEYEVENTF_EXTENDEDKEY;
    }
    if flags & 2 != 0 {
        f |= KEYEVENTF_KEYUP;
    }
    // By scan code, so it types what the key means in this PC's keyboard
    // layout; keys without one (media keys) by virtual key.
    let (wvk, wscan) = if scan != 0 {
        f |= KEYEVENTF_SCANCODE;
        (0, scan)
    } else {
        (vk, 0)
    };
    let i = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(wvk), wScan: wscan, dwFlags: f, time: 0, dwExtraInfo: 0 } } };
    unsafe {
        SendInput(&[i], std::mem::size_of::<INPUT>() as i32);
    }
}

fn track(button: bool, a: u16, b: u16, ext: u8, down: bool) {
    let mut h = lock(&HELD);
    h.retain(|x| !(x.0 == button && x.1 == a && x.2 == b));
    if down {
        h.push((button, a, b, ext));
    }
}

/// Let go of every key and button pressed here on the other PC's behalf.
pub fn release_all() {
    let held: Vec<_> = std::mem::take(&mut *lock(&HELD));
    for (button, a, b, ext) in held.into_iter().rev() {
        if button {
            let (flags, data) = match a {
                0 => (MOUSEEVENTF_LEFTUP, 0),
                1 => (MOUSEEVENTF_RIGHTUP, 0),
                2 => (MOUSEEVENTF_MIDDLEUP, 0),
                3 => (MOUSEEVENTF_XUP, 1),
                _ => (MOUSEEVENTF_XUP, 2),
            };
            input_mouse(flags, 0, 0, data);
        } else {
            key_event(a, b, ext | 2);
        }
    }
}
