//! Google's results page, read the way you'd see it: a hidden Microsoft Edge
//! WebView2 (already part of Windows 11) opens it,
//! and a few lines of script read the results off it. So Google's results
//! are Google's own — same order, titles, sites and snippets.
//!
//! No WebView2Loader.dll is shipped: the runtime's own
//! EmbeddedBrowserWebView.dll is found through the registry and asked for an
//! environment directly. The browser closes itself after a minute of quiet.

#![allow(non_snake_case)]

use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::*;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Registry::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::answers::WebHit;

/// (url, extraction script, reply)
struct Job {
    url: String,
    script: &'static str,
    reply: Sender<std::result::Result<Vec<(WebHit, String, String, String)>, String>>,
}

static TX: Mutex<Option<Sender<Job>>> = Mutex::new(None);
static WORKER: Mutex<isize> = Mutex::new(0);
const WM_JOB: u32 = WM_APP + 1;
const T_IDLE: usize = 1;
const T_RETRY: usize = 2;
/// How long a page may take to show its results.
const GIVE_UP: Duration = Duration::from_secs(9);

/// One web result read off the page: (hit, site name, breadcrumb, icon src)
pub type PageHit = (WebHit, String, String, String);

/// Google's results page → hits. Also says where the page ended up, so a
/// "are you a robot?" or cookie page can be told apart from no results.
pub const GOOGLE_JS: &str = r#"(() => {
  const out = []; const seen = new Set();
  const loc = location.href, title = document.title || '';
  let blocked = '';
  if (/\/sorry\//.test(loc) || document.querySelector('#captcha-form, form[action*="sorry"], iframe[src*="recaptcha"]')) blocked = 'captcha';
  else if (/consent\./.test(location.hostname) || document.querySelector('form[action*="consent"]')) blocked = 'consent';
  for (const h3 of document.querySelectorAll('#rso a h3, #search a h3, #center_col a h3, #main a h3')) {
    const a = h3.closest('a'); if (!a) continue;
    const u = a.href; if (!u || !u.startsWith('http') || seen.has(u) || /google\.[a-z.]+\/(search|url)/.test(u)) continue;
    seen.add(u);
    // This result's own block: the widest box around the link that still
    // holds only its title (a wider one would hand every result the first
    // result's icon and name).
    let box = a;
    while (box.parentElement && box.parentElement.id !== 'rso' && box.parentElement.querySelectorAll('h3').length <= 1) box = box.parentElement;
    const q = s => box.querySelector(s);
    const site = (q('.VuuXrf') || {}).innerText || '';
    const cite = (q('cite') || {}).innerText || '';
    const sn = (q('.VwiC3b') || q('[data-sncf]') || q('.ITZIwc') || q('.lEBKkf') || {}).innerText || '';
    // The site's own icon: next to its name, loaded (not a lazy placeholder).
    const img = q('img.XNo5Ab') || q('.H9lube img') || q('.eqA2re img');
    const isrc = img && img.complete && img.naturalWidth > 1 ? (img.currentSrc || img.src) : '';
    out.push({t: h3.innerText, u, s: site, c: cite, d: sn, i: isrc});
    if (out.length >= 10) break;
  }
  const done = document.readyState === 'complete';
  return JSON.stringify({n: out, loc, title, blocked, done});
})()"#;

/// Open `url` in the hidden browser and run `script` on it.
pub fn read_page(url: &str, script: &'static str) -> std::result::Result<Vec<PageHit>, String> {
    ensure_started()?;
    let (reply, rx) = channel();
    if let Ok(t) = TX.lock() {
        let t = t.as_ref().ok_or("no browser")?;
        t.send(Job { url: url.to_string(), script, reply }).map_err(|e| e.to_string())?;
    }
    let w = *WORKER.lock().map_err(|e| e.to_string())?;
    unsafe {
        let _ = PostMessageW(Some(HWND(w as *mut _)), WM_JOB, WPARAM(0), LPARAM(0));
    }
    rx.recv_timeout(Duration::from_secs(20)).map_err(|_| "the page took too long".to_string())?
}

fn ensure_started() -> std::result::Result<(), String> {
    if *WORKER.lock().map_err(|e| e.to_string())? != 0 {
        return Ok(());
    }
    let dll = runtime_dll().ok_or("WebView2 runtime not found")?;
    let (tx, rx) = channel::<Job>();
    let (ready_tx, ready_rx) = channel::<std::result::Result<isize, String>>();
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let inst = crate::util::hinstance();
            let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: inst, lpszClassName: w!("PhemioWebReader"), ..Default::default() };
            RegisterClassW(&wc);
            let Ok(hwnd) = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, w!("PhemioWebReader"), w!(""), WS_POPUP, -4000, -4000, 1280, 1400, None, None, Some(inst), None) else {
                let _ = ready_tx.send(Err("window".into()));
                return;
            };
            STATE.with(|s| *s.borrow_mut() = Some(State { rx, dll, view: None, pending: None, started: Instant::now(), attempt: 0, seen: String::new(), last: Instant::now(), hwnd }));
            let _ = ready_tx.send(Ok(hwnd.0 as isize));
            let mut m = MSG::default();
            while GetMessageW(&mut m, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        })
        .map_err(|e| e.to_string())?;
    let h = ready_rx.recv_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())??;
    *WORKER.lock().map_err(|e| e.to_string())? = h;
    *TX.lock().map_err(|e| e.to_string())? = Some(tx);
    Ok(())
}

struct View {
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
}

struct State {
    rx: std::sync::mpsc::Receiver<Job>,
    dll: String,
    view: Option<View>,
    pending: Option<Job>,
    /// When the pending page was asked for (gives up after a while).
    started: Instant,
    /// Script runs so far for this page (retried while results draw).
    attempt: u32,
    /// What the page looked like last time (for the log).
    seen: String,
    last: Instant,
    hwnd: HWND,
}

thread_local! {
    static STATE: std::cell::RefCell<Option<State>> = const { std::cell::RefCell::new(None) };
}

/// EmbeddedBrowserWebView.dll of the installed WebView2 runtime.
fn runtime_dll() -> Option<String> {
    const CLIENT: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    let read = |root: HKEY, key: &str, val: &str| -> Option<String> {
        let k = crate::util::wide(key);
        let v = crate::util::wide(val);
        let mut buf = [0u16; 520];
        let mut sz = (buf.len() * 2) as u32;
        unsafe { RegGetValueW(root, PCWSTR(k.as_ptr()), PCWSTR(v.as_ptr()), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut sz)) }.is_ok().then(|| crate::util::from_wide(&buf))
    };
    // What Microsoft's own loader reads: ClientState\{id}\EBWebView = the version folder.
    for (root, key) in [
        (HKEY_LOCAL_MACHINE, format!("SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\ClientState\\{CLIENT}")),
        (HKEY_CURRENT_USER, format!("Software\\Microsoft\\EdgeUpdate\\ClientState\\{CLIENT}")),
    ] {
        if let Some(folder) = read(root, &key, "EBWebView") {
            let p = format!("{}\\EBWebView\\x64\\EmbeddedBrowserWebView.dll", folder.trim_end_matches('\\'));
            if std::path::Path::new(&p).exists() {
                return Some(p);
            }
        }
    }
    for (root, key) in [
        (HKEY_LOCAL_MACHINE, format!("SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{CLIENT}")),
        (HKEY_CURRENT_USER, format!("Software\\Microsoft\\EdgeUpdate\\Clients\\{CLIENT}")),
        (HKEY_LOCAL_MACHINE, format!("SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\{CLIENT}")),
    ] {
        if let (Some(loc), Some(pv)) = (read(root, &key, "location"), read(root, &key, "pv")) {
            let p = format!("{loc}\\{pv}\\EBWebView\\x64\\EmbeddedBrowserWebView.dll");
            if std::path::Path::new(&p).exists() {
                return Some(p);
            }
        }
    }
    // Look in the usual folder for the newest version.
    let base = format!("{}\\Microsoft\\EdgeWebView\\Application", std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| "C:\\Program Files (x86)".into()));
    let mut best: Option<(Vec<u32>, String)> = None;
    for e in std::fs::read_dir(&base).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let ver: Vec<u32> = name.split('.').filter_map(|p| p.parse().ok()).collect();
        if ver.len() != 4 {
            continue;
        }
        let p = format!("{base}\\{name}\\EBWebView\\x64\\EmbeddedBrowserWebView.dll");
        if std::path::Path::new(&p).exists() && best.as_ref().map(|b| ver > b.0).unwrap_or(true) {
            best = Some((ver, p));
        }
    }
    best.map(|b| b.1)
}

type CreateInternal = unsafe extern "system" fn(i32, i32, PCWSTR, *mut core::ffi::c_void, *mut core::ffi::c_void) -> HRESULT;

/// Start the browser (once; again after it was closed for being idle).
fn create_view(hwnd: HWND, dll: &str) -> std::result::Result<View, String> {
    unsafe {
        let d = crate::util::wide(dll);
        let lib = LoadLibraryW(PCWSTR(d.as_ptr())).map_err(|e| e.to_string())?;
        let f = GetProcAddress(lib, s!("CreateWebViewEnvironmentWithOptionsInternal")).ok_or("no entry point")?;
        let create: CreateInternal = std::mem::transmute(f);
        let data = crate::config::dir().join("WebView");
        let _ = std::fs::create_dir_all(&data);
        let data_w = crate::util::wide(&data.to_string_lossy());
        let options = CoreWebView2EnvironmentOptions::default();
        let options: ICoreWebView2EnvironmentOptions = options.into();
        let (tx, rx) = channel();
        let mut last_err = String::new();
        // 0 = the installed runtime (as Microsoft's loader passes it).
        for kind in [0] {
            let tx = tx.clone();
            let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |hr, env| {
                let _ = tx.send(hr.and_then(|_| env.ok_or_else(|| windows::core::Error::from(E_POINTER))));
                Ok(())
            }));
            let hr = create(1, kind, PCWSTR(data_w.as_ptr()), options.as_raw(), handler.as_raw());
            if hr.is_err() {
                last_err = format!("{hr:?}");
                continue;
            }
            let env = wait_with_pump(rx).map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
            let (ctx, crx) = channel();
            let h2 = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |hr, c| {
                let _ = ctx.send(hr.and_then(|_| c.ok_or_else(|| windows::core::Error::from(E_POINTER))));
                Ok(())
            }));
            env.CreateCoreWebView2Controller(hwnd, &h2).map_err(|e| e.to_string())?;
            let controller = wait_with_pump(crx).map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
            let _ = controller.SetBounds(RECT { left: 0, top: 0, right: 1280, bottom: 1400 });
            let webview = controller.CoreWebView2().map_err(|e| e.to_string())?;
            if let Ok(s) = webview.Settings() {
                let _ = s.SetAreDefaultScriptDialogsEnabled(false);
                let _ = s.SetIsStatusBarEnabled(false);
                let _ = s.SetAreDevToolsEnabled(false);
            }
            // Nothing opens new windows from here.
            let _ = webview.add_NewWindowRequested(
                &NewWindowRequestedEventHandler::create(Box::new(|_, args| {
                    if let Some(a) = args {
                        let _ = a.SetHandled(true);
                    }
                    Ok(())
                })),
                &mut 0,
            );
            let _ = webview.add_NavigationCompleted(
                &NavigationCompletedEventHandler::create(Box::new(|wv, _| {
                    on_loaded(wv);
                    Ok(())
                })),
                &mut 0,
            );
            return Ok(View { controller, webview });
        }
        Err(format!("WebView2 couldn't start ({last_err})"))
    }
}

fn on_loaded(wv: Option<ICoreWebView2>) {
    let Some(wv) = wv else { return };
    // Each page load (Google may redirect once or twice) starts reading afresh.
    let ok = STATE.with(|s| {
        let mut b = s.borrow_mut();
        let Some(st) = b.as_mut() else { return false };
        unsafe {
            let _ = KillTimer(Some(st.hwnd), T_RETRY);
        }
        st.attempt = 0;
        st.pending.is_some()
    });
    if ok {
        run_script(&wv);
    }
}

fn run_script(wv: &ICoreWebView2) {
    let Some(script) = STATE.with(|s| s.borrow().as_ref().and_then(|st| st.pending.as_ref().map(|j| j.script))) else { return };
    let js = HSTRING::from(script);
    let handler = ExecuteScriptCompletedHandler::create(Box::new(move |hr, json: String| {
        let parsed = hr.map_err(|e| e.to_string()).and_then(|_| parse(&json));
        STATE.with(|s| {
            let mut b = s.borrow_mut();
            let Some(st) = b.as_mut() else { return };
            if st.pending.is_none() {
                return;
            }
            let reply = match parsed {
                Ok(Page { hits, .. }) if !hits.is_empty() => Some(Ok(hits)),
                Ok(Page { blocked, .. }) if blocked == "captcha" => Some(Err("Google asked to confirm you're not a robot".to_string())),
                Ok(Page { blocked, .. }) if blocked == "consent" => Some(Err("Google showed its cookie page".to_string())),
                Ok(p) => {
                    st.seen = format!("{} — {}", p.title, p.loc.chars().take(120).collect::<String>());
                    if st.started.elapsed() > GIVE_UP {
                        Some(Err(format!("no results on the page ({})", st.seen)))
                    } else {
                        None
                    }
                }
                Err(e) if st.started.elapsed() > GIVE_UP => Some(Err(e)),
                Err(_) => None,
            };
            match reply {
                Some(r) => {
                    if let Some(j) = st.pending.take() {
                        let _ = j.reply.send(r);
                    }
                    st.last = Instant::now();
                }
                // Results not drawn yet: look again shortly, without
                // blocking this thread (the page keeps loading meanwhile).
                None => unsafe {
                    st.attempt += 1;
                    let wait = if st.attempt < 8 { 200 } else { 400 };
                    SetTimer(Some(st.hwnd), T_RETRY, wait, None);
                },
            }
        });
        Ok(())
    }));
    unsafe {
        let _ = wv.ExecuteScript(&js, &handler);
    }
}

struct Page {
    hits: Vec<PageHit>,
    loc: String,
    title: String,
    blocked: String,
}

/// The script's answer (a JSON string inside JSON).
fn parse(json: &str) -> std::result::Result<Page, String> {
    let inner: String = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&inner).map_err(|e| e.to_string())?;
    let list = v["n"].as_array().cloned().unwrap_or_default();
    let hits = list
        .iter()
        .map(|h| {
            let s = |k: &str| h[k].as_str().unwrap_or("").trim().to_string();
            let snippet = s("d").split_whitespace().collect::<Vec<_>>().join(" ");
            (WebHit { title: s("t"), url: s("u"), snippet, ..Default::default() }, s("s"), s("c"), s("i"))
        })
        .filter(|(h, ..)| !h.title.is_empty())
        .collect();
    let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
    Ok(Page { hits, loc: s("loc"), title: s("title"), blocked: s("blocked") })
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_JOB => {
            let job = STATE.with(|s| s.borrow().as_ref().and_then(|st| st.rx.try_recv().ok()));
            let Some(job) = job else { return LRESULT(0) };
            // A newer question replaces one still loading.
            STATE.with(|s| {
                if let Some(st) = s.borrow_mut().as_mut() {
                    if let Some(old) = st.pending.take() {
                        let _ = old.reply.send(Err("replaced".into()));
                    }
                }
            });
            let need = STATE.with(|s| s.borrow().as_ref().map(|st| st.view.is_none()).unwrap_or(false));
            if need {
                let (h, dll) = STATE.with(|s| s.borrow().as_ref().map(|st| (st.hwnd, st.dll.clone())).unwrap());
                match create_view(h, &dll) {
                    Ok(v) => STATE.with(|s| {
                        if let Some(st) = s.borrow_mut().as_mut() {
                            st.view = Some(v);
                        }
                    }),
                    Err(e) => {
                        crate::util::log(&format!("web reader: {e}"));
                        let _ = job.reply.send(Err(e));
                        return LRESULT(0);
                    }
                }
            }
            let wv = STATE.with(|s| {
                let mut b = s.borrow_mut();
                let st = b.as_mut().unwrap();
                let url = job.url.clone();
                st.pending = Some(job);
                st.started = Instant::now();
                st.attempt = 0;
                st.seen.clear();
                st.last = Instant::now();
                st.view.as_ref().map(|v| (v.webview.clone(), url))
            });
            if let Some((wv, url)) = wv {
                let u = HSTRING::from(url);
                let _ = wv.Navigate(&u);
            }
            SetTimer(Some(hwnd), T_IDLE, 15_000, None);
            LRESULT(0)
        }
        WM_TIMER if wp.0 == T_RETRY => {
            let _ = KillTimer(Some(hwnd), T_RETRY);
            let wv = STATE.with(|s| s.borrow().as_ref().filter(|st| st.pending.is_some()).and_then(|st| st.view.as_ref().map(|v| v.webview.clone())));
            if let Some(wv) = wv {
                run_script(&wv);
            }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == T_IDLE => {
            // Free the browser's memory after a minute without questions.
            let close = STATE.with(|s| s.borrow().as_ref().map(|st| st.pending.is_none() && st.view.is_some() && st.last.elapsed() > Duration::from_secs(60)).unwrap_or(false));
            if close {
                let v = STATE.with(|s| s.borrow_mut().as_mut().and_then(|st| st.view.take()));
                if let Some(v) = v {
                    let _ = v.controller.Close();
                }
                let _ = KillTimer(Some(hwnd), T_IDLE);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
