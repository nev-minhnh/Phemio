//! Alt+S search: apps (Start menu, incl. Store apps) built in, files and
//! folders through Everything (voidtools) — spoken to directly over its
//! IPC window, so neither es.exe nor the Everything SDK DLL is needed.
//! Only Everything itself has to be installed and running.

#![allow(non_snake_case)]

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const DOWNLOAD_URL: &str = "https://www.voidtools.com/downloads/";

#[derive(Debug)]
pub struct WebMeta {
    pub site: String,
    pub crumb: String,
    pub snippet: String,
    pub icon: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    App,
    Folder,
    File,
    /// "12*7" → 84
    Calc,
    /// "100 usd to vnd"
    Currency,
    /// "b64 …" / "b64d …" / a pasted base64 string
    Base64,
    /// "tr …"
    Translate,
    /// "dict …" (Alt+W): a word's meanings, from Google Translate.
    Dict,
    /// The text read from a pasted picture (OCR).
    OcrText,
    /// A pasted picture's text with its layout, as Markdown (shown as a
    /// document; Enter copies it).
    OcrDoc,
    /// Waiting for the internet (currency / translation) or reading a picture.
    Thinking,
    /// QR(text) → a code to copy.
    QrCode,
    /// What a pasted QR code says.
    QrText,
    /// A web search result (Enter opens it).
    Web,
    /// "Search the web for …" / "Ask Gemini …" (Enter runs it here).
    WebAction,
    /// Gemini's live answer (the search's slot in `gemini`; Enter copies it).
    Gemini,
    /// Something didn't work (the reason, and what to try).
    Error,
}

impl Kind {
    /// An answer row: Enter copies it.
    pub fn is_answer(&self) -> bool {
        matches!(self, Kind::Calc | Kind::Currency | Kind::Base64 | Kind::Translate | Kind::Dict | Kind::QrCode | Kind::QrText | Kind::OcrText)
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    /// Searching inside files: the passage around the words ("…the vat code is…").
    pub snip: Option<String>,
    /// A web result's details (site, link, snippet, icon).
    pub web: Option<std::sync::Arc<WebMeta>>,
    /// QR(text): the code to draw.
    pub qr: Option<std::sync::Arc<crate::qr::Matrix>>,
    pub name: String,
    /// Folder for files, "" for apps.
    pub dir: String,
    /// What to open: a full path, or "shell:AppsFolder\…" for apps.
    pub target: String,
    pub kind: Kind,
    pub score: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Engine {
    Ready,
    /// Installed, not running: path to Everything.exe
    NotRunning(String),
    Missing,
}

#[derive(Clone, Debug)]
pub struct Results {
    pub query: String,
    pub items: Vec<Item>,
    pub engine: Engine,
    /// How many files Everything found in all.
    pub total: u32,
    /// Still waiting for Everything's answer.
    pub pending: bool,
}

static RESULT: Mutex<Option<Results>> = Mutex::new(None);
static PENDING: Mutex<Option<String>> = Mutex::new(None);
static WORKER: AtomicIsize = AtomicIsize::new(0);
static HOST: AtomicIsize = AtomicIsize::new(0);
static HOST_MSG: AtomicU32 = AtomicU32::new(0);

const WM_QUERY: u32 = WM_APP + 1;
const T_TIMEOUT: usize = 1;
const REPLY_BASE: usize = 0x4D41_0000;

const QUERY2W: usize = 18;
const REQ_NAME: u32 = 0x1;
const REQ_PATH: u32 = 0x2;
const REQ_RUN_COUNT: u32 = 0x400;
const SORT_RUN_COUNT_DESC: u32 = 20;
const FOLDER: u32 = 0x1;

pub fn take() -> Option<Results> {
    RESULT.lock().ok().and_then(|r| r.clone())
}

/// Start the worker (once).
pub fn start(host: HWND, msg: u32) {
    HOST.store(host.0 as isize, Ordering::Relaxed);
    HOST_MSG.store(msg, Ordering::Relaxed);
    if WORKER.load(Ordering::Relaxed) != 0 {
        return;
    }
    let _ = std::thread::Builder::new().stack_size(1024 * 1024).spawn(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let inst = crate::util::hinstance();
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: inst, lpszClassName: w!("PhemioSearchReply"), ..Default::default() };
        RegisterClassW(&wc);
        let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE(0), w!("PhemioSearchReply"), w!(""), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(inst), None) else { return };
        WORKER.store(hwnd.0 as isize, Ordering::Relaxed);
        let mut m = MSG::default();
        while GetMessageW(&mut m, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    });
}

/// Ask for results for `q` (the newest query wins).
pub fn query(q: &str) {
    if let Ok(mut p) = PENDING.lock() {
        *p = Some(q.to_string());
    }
    if let Ok(mut l) = LATEST.lock() {
        *l = q.to_string();
    }
    let w = WORKER.load(Ordering::Relaxed);
    if w != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(w as *mut _)), WM_QUERY, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn clear() {
    if let Ok(mut r) = RESULT.lock() {
        *r = None;
    }
}

fn publish(r: Results) {
    if let Ok(mut x) = RESULT.lock() {
        *x = Some(r);
    }
    let h = HOST.load(Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), HOST_MSG.load(Ordering::Relaxed), WPARAM(0), LPARAM(0));
        }
    }
}

struct Waiting {
    seq: usize,
    query: String,
    apps: Vec<Item>,
    /// A look inside files: its search generation.
    content: Option<u64>,
}

/// Searching inside files: each new search (or Cancel) bumps the generation,
/// and work for an older one stops at its next check.
static CONTENT_GEN: AtomicU64 = AtomicU64::new(0);
static CONTENT_BUSY: AtomicBool = AtomicBool::new(false);
/// Windows' index wasn't there: (generation, query, rows) for Everything to take over.
static EV_CONTENT: Mutex<Option<(u64, String, Vec<Item>)>> = Mutex::new(None);
const WM_EVCONTENT: u32 = WM_APP + 2;
const WM_EVSTOP: u32 = WM_APP + 3;

/// Still looking inside files?
pub fn content_busy() -> bool {
    CONTENT_BUSY.load(Ordering::SeqCst)
}

/// Stop looking inside files (Cancel, the search closing, the mode switched):
/// what's found so far stays, and nothing keeps working in the background.
pub fn cancel_content() {
    CONTENT_GEN.fetch_add(1, Ordering::SeqCst);
    if !CONTENT_BUSY.swap(false, Ordering::SeqCst) {
        return;
    }
    if let Some(mut r) = take() {
        r.pending = false;
        publish(r);
    }
    let w = WORKER.load(Ordering::Relaxed);
    if w != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(w as *mut _)), WM_EVSTOP, WPARAM(0), LPARAM(0));
        }
    }
}

/// Fill in each file's passage around the words, a few files at a time,
/// showing them as they come; stops when the generation moves on.
/// Only files that showed their passage are results (a file whose words
/// can't be shown isn't one).
fn with_passages(r: &Results) -> Results {
    let mut v = r.clone();
    v.items.retain(|i| !matches!(i.kind, Kind::File | Kind::Folder) || i.snip.is_some());
    v
}

fn fill_excerpts(mut r: Results, gen: u64) {
    let alive = move || CONTENT_GEN.load(Ordering::SeqCst) == gen;
    r.pending = true;
    if !alive() {
        return;
    }
    publish(with_passages(&r));
    let words = crate::contentsearch::words_of(&r.query);
    let todo: Vec<(usize, String)> = r.items.iter().enumerate().filter(|(_, i)| i.kind == Kind::File).take(200).map(|(k, i)| (k, i.target.clone())).collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let (tx, rx) = std::sync::mpsc::channel::<(usize, String)>();
    std::thread::scope(|sc| {
        for _ in 0..3 {
            let (tx, todo, next, words) = (tx.clone(), &todo, &next, &words);
            sc.spawn(move || {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                }
                loop {
                    let k = next.fetch_add(1, Ordering::SeqCst);
                    let Some((row, path)) = todo.get(k) else { break };
                    if !alive() {
                        break;
                    }
                    // Developer aid: ISLET_SLOW_EXCERPT=<ms> per file (to watch it work / cancel it).
                    if let Some(ms) = std::env::var("ISLET_SLOW_EXCERPT").ok().and_then(|v| v.parse::<u64>().ok()) {
                        for _ in 0..ms / 50 {
                            if !alive() {
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                    }
                    match crate::contentsearch::excerpt_why(path, words, &|| !alive()) {
                        Ok(p) => {
                            let _ = tx.send((*row, p));
                        }
                        Err(why) if why != "stopped" => crate::util::log(&format!("search: no passage from {path}: {why}")),
                        Err(_) => {}
                    }
                }
            });
        }
        drop(tx);
        for (row, p) in rx {
            if !alive() {
                continue;
            }
            if let Some(it) = r.items.get_mut(row) {
                it.snip = Some(p);
            }
            publish(with_passages(&r));
        }
    });
    if alive() {
        r.pending = false;
        CONTENT_BUSY.store(false, Ordering::SeqCst);
        publish(with_passages(&r));
    }
}

thread_local! {
    static APPS: RefCell<Option<Vec<Item>>> = const { RefCell::new(None) };
    static WAIT: RefCell<Option<Waiting>> = const { RefCell::new(None) };
    static SEQ: RefCell<usize> = const { RefCell::new(0) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_QUERY => {
            let Some(q) = PENDING.lock().ok().and_then(|mut p| p.take()) else { return LRESULT(0) };
            run_query(hwnd, &q);
            LRESULT(0)
        }
        WM_EVCONTENT => {
            let Some((gen, q, items)) = EV_CONTENT.lock().ok().and_then(|mut x| x.take()) else { return LRESULT(0) };
            if CONTENT_GEN.load(Ordering::SeqCst) != gen {
                return LRESULT(0);
            }
            let Some(ev) = everything_window() else {
                CONTENT_BUSY.store(false, Ordering::SeqCst);
                publish(merge(&q, items, vec![], 0, engine_state(), false));
                return LRESULT(0);
            };
            let safe: String = q.chars().filter(|c| *c != '"').collect();
            send_everything(hwnd, ev, &q, &format!("{CONTENT_EXTS} size:<20mb content:\"{safe}\""), items, Some(gen), 60_000);
            LRESULT(0)
        }
        WM_EVSTOP => {
            // Everything stops looking inside files when it's given a new search.
            if WAIT.with(|w| w.borrow().as_ref().map(|w| w.content.is_some()).unwrap_or(false)) {
                WAIT.with(|w| *w.borrow_mut() = None);
                let _ = KillTimer(Some(hwnd), T_TIMEOUT);
                if let Some(ev) = everything_window() {
                    let text: Vec<u16> = "\u{1}\u{1}".encode_utf16().chain(std::iter::once(0)).collect();
                    let mut buf: Vec<u8> = Vec::new();
                    for v in [hwnd.0 as usize as u32, (REPLY_BASE + 0xfffe) as u32, 0, 0, 1, REQ_NAME, 0] {
                        buf.extend_from_slice(&v.to_le_bytes());
                    }
                    for c in &text {
                        buf.extend_from_slice(&c.to_le_bytes());
                    }
                    let cds = COPYDATASTRUCT { dwData: QUERY2W, cbData: buf.len() as u32, lpData: buf.as_ptr() as *mut _ };
                    let mut res = 0usize;
                    let _ = SendMessageTimeoutW(ev, WM_COPYDATA, WPARAM(hwnd.0 as usize), LPARAM(&cds as *const _ as isize), SMTO_ABORTIFHUNG, 1500, Some(&mut res));
                }
            }
            LRESULT(0)
        }
        WM_COPYDATA => {
            let cds = &*(lp.0 as *const COPYDATASTRUCT);
            let w = WAIT.with(|w| w.borrow_mut().take());
            match w {
                Some(w) if cds.dwData == REPLY_BASE + w.seq => {
                    let _ = KillTimer(Some(hwnd), T_TIMEOUT);
                    let data = std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize);
                    let (files, total) = parse_list2(data);
                    match w.content {
                        // Inside files: then each one's passage.
                        Some(gen) => {
                            let r = merge(&w.query, w.apps, files, total, Engine::Ready, true);
                            let _ = std::thread::Builder::new().name("search-excerpts".into()).spawn(move || fill_excerpts(r, gen));
                        }
                        None => publish(merge(&w.query, w.apps, files, total, Engine::Ready, false)),
                    }
                }
                other => WAIT.with(|x| *x.borrow_mut() = other),
            }
            LRESULT(1)
        }
        WM_TIMER if wp.0 == T_TIMEOUT => {
            let _ = KillTimer(Some(hwnd), T_TIMEOUT);
            if let Some(w) = WAIT.with(|w| w.borrow_mut().take()) {
                if w.content.is_some() {
                    CONTENT_BUSY.store(false, Ordering::SeqCst);
                }
                // Everything is busy (still indexing?) — show the apps.
                publish(merge(&w.query, w.apps, vec![], 0, Engine::Ready, false));
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn run_query(hwnd: HWND, q: &str) {
    let q = q.trim().to_string();
    if q.is_empty() {
        WAIT.with(|w| *w.borrow_mut() = None);
        publish(Results { query: q, items: vec![], engine: engine_state(), total: 0, pending: false });
        return;
    }
    let apps = APPS.with(|a| {
        let mut a = a.borrow_mut();
        if a.is_none() {
            *a = Some(load_apps());
        }
        match_apps(a.as_ref().unwrap(), &q)
    });
    // Instant answers. Prefixed ones ("tr …", "b64 …", "100 usd …") show alone.
    let (answers, mut only) = answers_for(&q);
    // A sum shows just its result (no files, no web rows) — like every other
    // answer. (A date such as 2024-10-23 stays a search.)
    let sum = if looks_date(&q) { None } else { calc(&q) };
    only |= sum.is_some();
    let mut items = if only { vec![] } else { apps.clone() };
    if let Some(v) = sum {
        items.insert(0, Item { snip: None, qr: None, web: None, name: v.clone(), dir: format!("{q} ="), target: v, kind: Kind::Calc, score: 10_000 });
    }
    for a in answers.into_iter().rev() {
        items.insert(0, a);
    }
    // Last row: take the question to the web.
    if !only && q.chars().count() >= 2 {
        let tr = crate::i18n::tr;
        items.push(Item { snip: None, qr: None, web: None, name: format!("{} “{q}”", tr("Search Google for")), target: format!("> {q}"), dir: tr("Top 10 results right here").to_string(), kind: Kind::WebAction, score: -100_000 });
        if crate::gemini::has_key() {
            items.push(Item { snip: None, qr: None, web: None, name: format!("{} “{q}”", tr("Ask Gemini")), target: format!("? {q}"), dir: tr("An answer right here, from Google AI").to_string(), kind: Kind::WebAction, score: -100_001 });
        }
    }
    if only {
        publish(Results { query: q, items, engine: Engine::Ready, total: 0, pending: false });
        return;
    }
    // Inside files (the search box's Names / Contents switch): Windows' search
    // index, best match first, then the passage around the words from each
    // file; Everything's content: search if the index is off. All on the
    // side, and stopped by a newer search, Cancel, or closing the search.
    if crate::config::cur().b("srch_content") {
        items.retain(|i| i.kind != Kind::App);
        let gen = CONTENT_GEN.fetch_add(1, Ordering::SeqCst) + 1;
        CONTENT_BUSY.store(true, Ordering::SeqCst);
        publish(Results { query: q.clone(), items: items.clone(), engine: Engine::Ready, total: 0, pending: true });
        let worker = hwnd.0 as isize;
        let _ = std::thread::Builder::new().name("search-inside".into()).spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            // Phemio's own index (Settings → General → Content search):
            // answers at once, every file with its passage.
            if crate::contentindex::enabled() && std::env::var("ISLET_FAKE_FILES").is_err() {
                let alive = move || CONTENT_GEN.load(Ordering::SeqCst) == gen;
                match crate::contentindex::search(&q, FIRST_PAGE, alive) {
                    Ok(found) => {
                        if !alive() {
                            return;
                        }
                        let full = found.len() >= FIRST_PAGE;
                        CONTENT_BUSY.store(false, Ordering::SeqCst);
                        publish(indexed_results(&q, items.clone(), found));
                        // The first page is up; now the rest (more pages).
                        if full {
                            if let Ok(more) = crate::contentindex::search(&q, MAX_CONTENT, alive) {
                                if alive() {
                                    publish(indexed_results(&q, items, more));
                                }
                            }
                        }
                        return;
                    }
                    Err(e) => crate::util::log(&format!("search: Phemio's index didn't answer ({e}); asking Windows' index")),
                }
            }
            // Developer aid: ISLET_FAKE_FILES (as for names) stands in for the index.
            let found = match std::env::var("ISLET_FAKE_FILES") {
                Ok(v) => Ok(v.split(';').filter(|p| !p.is_empty()).map(|p| {
                    let (dir, name) = p.rsplit_once('\\').unwrap_or(("", p));
                    (name.to_string(), dir.to_string())
                }).collect()),
                Err(_) => crate::contentsearch::search(&q, 200),
            };
            if CONTENT_GEN.load(Ordering::SeqCst) != gen {
                return;
            }
            match found {
                Ok(found) => fill_excerpts(content_results(&q, items, found), gen),
                Err(e) => {
                    crate::util::log(&format!("search: Windows' index didn't answer ({e}); asking Everything to look inside files"));
                    if let Ok(mut x) = EV_CONTENT.lock() {
                        *x = Some((gen, q, items));
                    }
                    unsafe {
                        let _ = PostMessageW(Some(HWND(worker as *mut _)), WM_EVCONTENT, WPARAM(0), LPARAM(0));
                    }
                }
            }
        });
        return;
    }
    // Developer aid: ISLET_FAKE_FILES="C:\a\b.txt;C:\c\d;…" (folders end without an extension)
    if let Ok(v) = std::env::var("ISLET_FAKE_FILES") {
        let files: Vec<(String, String, bool, u32)> = v
            .split(';')
            .filter(|p| p.to_lowercase().contains(&q.to_lowercase().split(' ').next().unwrap_or("").to_string()))
            .map(|p| {
                let (dir, name) = p.rsplit_once('\\').unwrap_or(("", p));
                (name.to_string(), dir.to_string(), !name.contains('.'), 0)
            })
            .collect();
        let n = files.len() as u32;
        publish(merge(&q, items, files, n, Engine::Ready, false));
        return;
    }
    let Some(ev) = everything_window() else {
        publish(merge(&q, items, vec![], 0, engine_state(), false));
        return;
    };
    // Apps right away; files when Everything answers.
    publish(merge(&q, items.clone(), vec![], 0, Engine::Ready, true));
    send_everything(hwnd, ev, &q, &q, items, None, 2500);
}

/// Ask Everything for `text` (shown as results for `q`); the answer arrives
/// as WM_COPYDATA. `content`: a look inside files (its search generation).
fn send_everything(hwnd: HWND, ev: HWND, q: &str, text: &str, items: Vec<Item>, content: Option<u64>, wait_ms: u32) {
    let q = q.to_string();
    let seq = SEQ.with(|s| {
        let mut s = s.borrow_mut();
        *s = (*s + 1) % 0xffff;
        *s
    });
    // EVERYTHING_IPC_QUERY2 + the search text (UTF-16, NUL-terminated).
    let text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf: Vec<u8> = Vec::with_capacity(28 + text.len() * 2);
    for v in [hwnd.0 as usize as u32, (REPLY_BASE + seq) as u32, 0, 0, MAX_FILES as u32, REQ_NAME | REQ_PATH | REQ_RUN_COUNT, SORT_RUN_COUNT_DESC] {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    for c in &text {
        buf.extend_from_slice(&c.to_le_bytes());
    }
    let cds = COPYDATASTRUCT { dwData: QUERY2W, cbData: buf.len() as u32, lpData: buf.as_ptr() as *mut _ };
    WAIT.with(|w| *w.borrow_mut() = Some(Waiting { seq, query: q.clone(), apps: items.clone(), content }));
    let mut res = 0usize;
    let ok = unsafe { SendMessageTimeoutW(ev, WM_COPYDATA, WPARAM(hwnd.0 as usize), LPARAM(&cds as *const _ as isize), SMTO_ABORTIFHUNG, 1500, Some(&mut res)) };
    if ok.0 == 0 || res == 0 {
        WAIT.with(|w| *w.borrow_mut() = None);
        if content.is_some() {
            CONTENT_BUSY.store(false, Ordering::SeqCst);
        }
        publish(merge(&q, items, vec![], 0, Engine::Ready, false));
        return;
    }
    unsafe { SetTimer(Some(hwnd), T_TIMEOUT, wait_ms, None) };
}

/// Most results asked for (shown 50 to a page).
const MAX_FILES: usize = 1000;
/// Inside files, from Phemio's index: the first page at once, then the rest.
const FIRST_PAGE: usize = 50;
const MAX_CONTENT: usize = 500;

/// File types Everything reads when asked to look inside files.
const CONTENT_EXTS: &str = "ext:txt;md;log;csv;tsv;json;xml;yaml;yml;ini;cfg;conf;toml;html;htm;css;js;ts;tsx;jsx;py;rs;c;h;cpp;hpp;cs;java;kt;go;rb;php;sql;sh;bat;ps1;tex;rtf;srt";

/// Results from Phemio's own index: (path, passage), best first.
fn indexed_results(q: &str, items: Vec<Item>, found: Vec<(String, String)>) -> Results {
    let (actions, mut out): (Vec<Item>, Vec<Item>) = items.into_iter().partition(|i| i.kind == Kind::WebAction);
    let total = found.len() as u32;
    for (k, (path, snip)) in found.into_iter().enumerate() {
        let (dir, name) = path.rsplit_once('\\').map(|(d, n)| (d.to_string(), n.to_string())).unwrap_or((String::new(), path.clone()));
        out.push(Item { snip: Some(snip), qr: None, web: None, name, dir, target: path, kind: Kind::File, score: 5_000 - k as i32 });
    }
    out.extend(actions);
    Results { query: q.to_string(), items: out, engine: Engine::Ready, total, pending: false }
}

/// Content results in the index's own order (best match first), after the
/// answer rows and before "Search Google for …".
fn content_results(q: &str, items: Vec<Item>, found: Vec<(String, String)>) -> Results {
    let (actions, mut out): (Vec<Item>, Vec<Item>) = items.into_iter().partition(|i| i.kind == Kind::WebAction);
    let total = found.len() as u32;
    for (k, (name, dir)) in found.into_iter().enumerate() {
        let full = if dir.is_empty() { name.clone() } else { format!("{dir}\\{name}") };
        let folder = std::path::Path::new(&full).is_dir();
        out.push(Item { snip: None, qr: None, web: None, name, dir, target: full, kind: if folder { Kind::Folder } else { Kind::File }, score: 5_000 - k as i32 });
    }
    out.truncate(MAX_CONTENT);
    out.extend(actions);
    Results { query: q.to_string(), items: out, engine: Engine::Ready, total, pending: false }
}

// ---------------------------------------------------------------------------
// Answers (base64 here; currency and translation from the internet)
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum Net {
    Pending,
    Done(Vec<Item>),
    Failed(String),
}

static NET: Mutex<Option<std::collections::HashMap<String, Net>>> = Mutex::new(None);
static LATEST: Mutex<String> = Mutex::new(String::new());

fn answer(name: String, dir: String, kind: Kind) -> Item {
    Item { snip: None, qr: None, web: None, target: name.clone(), name, dir, kind, score: 20_000 }
}

/// Look up `key` on the internet once (after a short pause, only if the user
/// is still asking it); meanwhile a "thinking" row stands in.
fn net_answer(key: String, q: &str, label: String, job: impl FnOnce() -> std::result::Result<Vec<Item>, String> + Send + 'static) -> Vec<Item> {
    let state = NET.lock().ok().and_then(|m| m.as_ref().and_then(|m| m.get(&key).cloned()));
    match state {
        Some(Net::Done(it)) => it,
        Some(Net::Failed(e)) => {
            // Shown once; asking again (Enter) tries again.
            if let Ok(mut m) = NET.lock() {
                if let Some(m) = m.as_mut() {
                    m.remove(&key);
                }
            }
            let tr = crate::i18n::tr;
            let what = if key.starts_with("ai:") {
                tr("Gemini couldn't answer")
            } else if key.starts_with("web:") {
                tr("Google search didn't work")
            } else {
                tr("Couldn't reach the internet")
            };
            vec![answer(what.to_string(), e, Kind::Error)]
        }
        Some(Net::Pending) => vec![answer(label, String::new(), Kind::Thinking)],
        None => {
            if let Ok(mut m) = NET.lock() {
                m.get_or_insert_with(Default::default).insert(key.clone(), Net::Pending);
            }
            let q = q.to_string();
            std::thread::spawn(move || {
                // Typing on: don't ask the server about every half-typed word.
                std::thread::sleep(std::time::Duration::from_millis(350));
                let still = LATEST.lock().map(|l| l.trim() == q.trim()).unwrap_or(false);
                if !still {
                    if let Ok(mut m) = NET.lock() {
                        if let Some(m) = m.as_mut() {
                            m.remove(&key);
                        }
                    }
                    return;
                }
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                }
                let r = job();
                if let Ok(mut m) = NET.lock() {
                    let m = m.get_or_insert_with(Default::default);
                    if m.len() > 200 {
                        m.clear();
                    }
                    m.insert(key, match r {
                        Ok(it) => Net::Done(it),
                        Err(e) => Net::Failed(e),
                    });
                }
                // Ask again: this time the answer is ready.
                if LATEST.lock().map(|l| l.trim() == q.trim()).unwrap_or(false) {
                    query(&q);
                }
            });
            vec![answer(label, String::new(), Kind::Thinking)]
        }
    }
}

/// "> question": Google's results. "? question": ask Gemini.
pub const WEB_PREFIXES: [&str; 4] = [">", "g ", "web ", "google "];
pub const AI_PREFIXES: [&str; 4] = ["?", "ai ", "gemini ", "ask "];

/// The question after one of `prefixes`, if `q` starts with one.
pub fn after_prefix(q: &str, prefixes: &[&str]) -> Option<String> {
    let lower = q.to_lowercase();
    prefixes.iter().find_map(|p| lower.starts_with(p).then(|| q[p.len()..].trim().to_string())).filter(|w| !w.is_empty())
}

/// The row that shows Gemini's live answer to `q`.
fn gemini_row(q: &str) -> Item {
    Item { snip: None, qr: None, web: None, name: q.to_string(), target: format!("gemini:{q}"), dir: String::new(), kind: Kind::Gemini, score: 30_000 }
}

/// Ask Gemini `q` in the search's slot (unless it's already answering that).
fn ask_gemini(q: &str, note: String) {
    use crate::gemini::*;
    let choice = ask_choice();
    let again = live(ASK).map(|l| l.query != q || l.choice != choice || (l.done && !l.error.is_empty())).unwrap_or(true);
    if again {
        start_with_note(ASK, true, q, None, choice, HOST.load(Ordering::Relaxed), HOST_MSG.load(Ordering::Relaxed), note);
    }
}

/// Answer rows for `q`, and whether they replace the normal results.
fn answers_for(q: &str) -> (Vec<Item>, bool) {
    use crate::answers::*;
    let tr = crate::i18n::tr;
    let lower = q.to_lowercase();
    // "<": waiting for a picture to read the text in it.
    if q.trim() == "<" {
        return (vec![Item { snip: None, qr: None, web: None, name: tr("Paste a picture (Ctrl+V) to read the text in it").to_string(), target: String::new(), dir: tr("Text reader (OCR)").to_string(), kind: Kind::OcrText, score: 20_000 }], true);
    }
    // "dict" (Alt+W) with no word yet.
    if ["dict", "def", "từ điển"].contains(&lower.trim()) {
        return (vec![Item { snip: None, qr: None, web: None, name: tr("Type a word to look it up").to_string(), target: String::new(), dir: tr("Dictionary · Google Translate").to_string(), kind: Kind::Dict, score: 20_000 }], true);
    }
    // QR(text) → a QR code
    if lower.starts_with("qr(") {
        let inner = q[3..].trim_end();
        let inner = inner.strip_suffix(')').unwrap_or(inner);
        if inner.is_empty() {
            return (vec![answer(tr("Type the text inside QR( )").to_string(), String::new(), Kind::Thinking)], true);
        }
        return match crate::qr::make(inner) {
            Some(m) => (vec![Item { snip: None, qr: Some(m), web: None, name: inner.to_string(), target: inner.to_string(), dir: tr("QR code · Enter copies the picture").to_string(), kind: Kind::QrCode, score: 20_000 }], true),
            None => (vec![answer(tr("Too long for a QR code").to_string(), String::new(), Kind::Thinking)], true),
        };
    }
    // B64(text) → base64
    if lower.starts_with("b64(") || lower.starts_with("base64(") {
        let start = q.find('(').unwrap_or(0) + 1;
        let inner = &q[start..];
        let inner = inner.strip_suffix(')').unwrap_or(inner);
        return (vec![answer(b64_encode(inner.as_bytes()), tr("Base64 encoded").to_string(), Kind::Base64)], true);
    }
    // Base64
    for p in ["b64d ", "base64d ", "base64 decode ", "b64 decode "] {
        if lower.starts_with(p) {
            let rest = &q[p.len()..];
            let it = match b64_text(rest) {
                Some(t) => answer(t, tr("Base64 decoded").to_string(), Kind::Base64),
                None => answer(tr("Not valid base64 text").to_string(), String::new(), Kind::Base64),
            };
            return (vec![it], true);
        }
    }
    for p in ["b64 ", "base64 ", "b64e ", "base64 encode "] {
        if lower.starts_with(p) {
            let rest = &q[p.len()..];
            return (vec![answer(b64_encode(rest.as_bytes()), tr("Base64 encoded").to_string(), Kind::Base64)], true);
        }
    }
    // Gemini: "? question" — written out as it thinks.
    if let Some(aq) = after_prefix(q, &AI_PREFIXES) {
        ask_gemini(&aq, String::new());
        return (vec![gemini_row(&aq)], true);
    }
    // Web: "> question", "g question", "web question"
    if let Some(wq) = after_prefix(q, &WEB_PREFIXES) {
        let key = format!("web:{wq}");
        let label = tr("Searching Google…").to_string();
        let wq2 = wq.clone();
        let items = net_answer(key, q, label, move || {
            // The hidden browser's first start sometimes misses the page: try once more.
            let first = web_search(&wq2).or_else(|e| if e.contains("robot") || e.contains("cookie") { Err(e) } else { web_search(&wq2) });
            let hits = match first {
                Ok(h) => h,
                // Google's page wouldn't load: Gemini looks it up on Google instead.
                Err(_) if crate::gemini::has_key() => {
                    ask_gemini(&wq2, tr("Google's page didn't load, so Gemini answered").to_string());
                    return Ok(vec![gemini_row(&wq2)]);
                }
                Err(e) => return Err(e),
            };
            // Each result shows its place in the list (1, 2, 3…), not a site icon.
            let v: Vec<Item> = hits
                .into_iter()
                .map(|h| (h, None))
                .enumerate()
                .map(|(i, (h, icon))| {
                    let site = if h.site.is_empty() { site_name(&h.url) } else { h.site.clone() };
                    let crumb = if h.crumb.is_empty() { format!("https://{}", pretty_url(&h.url)) } else { h.crumb.replace('\n', " ") };
                    let meta = WebMeta { site, crumb, snippet: h.snippet.clone(), icon };
                    Item { snip: None, qr: None, web: Some(std::sync::Arc::new(meta)), name: h.title, target: h.url, dir: "Google".to_string(), kind: Kind::Web, score: 20_000 - i as i32 }
                })
                .collect();
            Ok(v)
        });
        return (items, true);
    }
    // Dictionary (Alt+W): "dict word"
    if let Some((word, to)) = crate::answers::parse_dict(q) {
        let key = format!("dict:{}:{word}", to.clone().unwrap_or_default());
        let label = tr("Looking it up…").to_string();
        let it = net_answer(key, q, label, move || {
            let d = crate::answers::dictionary(&word, to.as_deref())?;
            Ok(dict_rows(&d))
        });
        return (it, true);
    }
    // Translation
    if let Some((text, to)) = parse_translate(q) {
        let key = format!("tr:{}:{text}", to.clone().unwrap_or_default());
        let label = tr("Translating…").to_string();
        let it = net_answer(key, q, label, move || {
            let t = translate(&text, to.as_deref())?;
            let dir = format!("{} → {} · Google Translate", lang_label(&t.from), lang_label(&t.to));
            Ok(vec![answer(t.text, dir, Kind::Translate)])
        });
        return (it, true);
    }
    // Currency
    if let Some(fx) = parse_fx(q, home_currency()) {
        let key = format!("fx:{}:{}:{}", fx.amount, fx.from, fx.to);
        let label = tr("Checking the rate…").to_string();
        let it = net_answer(key, q, label, move || {
            let (r, src) = rate(&fx.from, &fx.to)?;
            let v = fx.amount * r;
            let dir = format!("{} {}  ·  1 {} = {} {}  ·  {src}", fmt_money(fx.amount, &fx.from), fx.from, fx.from, fmt_money(r, &fx.to), fx.to);
            Ok(vec![Item { snip: None, qr: None, web: None, name: format!("{} {}", fmt_money(v, &fx.to), fx.to), target: fmt_money(v, &fx.to).replace(',', ""), dir, kind: Kind::Currency, score: 20_000 }])
        });
        return (it, true);
    }
    // A pasted base64 string: offer its text (files still searched).
    if looks_b64(q) {
        if let Some(t) = b64_text(q) {
            return (vec![answer(t, tr("Base64 decoded").to_string(), Kind::Base64)], true);
        }
    }
    (vec![], false)
}

/// A dictionary look-up as rows: the translation, then each part of speech
/// with its translations, definitions and examples. Enter copies a row.
fn dict_rows(d: &crate::answers::Dict) -> Vec<Item> {
    let mut v = Vec::new();
    let sound = if d.sound.is_empty() { String::new() } else { format!(" /{}/", d.sound) };
    let head = format!("{}{sound}  ·  {} → {}  ·  Google Translate", d.word, crate::answers::lang_label(&d.from), crate::answers::lang_label(&d.to));
    v.push(Item { snip: None, qr: None, web: None, name: d.text.clone(), target: d.text.clone(), dir: head, kind: Kind::Dict, score: 20_000 });
    // "Other translations", as Google Translate lists them: the word, its
    // part of speech, and what it means back (carried in `snip`).
    let cap = |p: &str| {
        let mut c = p.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    };
    if d.entries.is_empty() {
        for (pos, words) in d.senses.iter().take(6) {
            let list = words.iter().take(10).cloned().collect::<Vec<_>>().join(", ");
            v.push(Item { snip: None, qr: None, web: None, name: list.clone(), target: list, dir: cap(pos), kind: Kind::Dict, score: 19_999 });
        }
    }
    for (k, (pos, word, back)) in d.entries.iter().enumerate() {
        v.push(Item { snip: Some(back.join("\u{1}")), qr: None, web: None, name: word.clone(), target: word.clone(), dir: cap(pos), kind: Kind::Dict, score: 19_999 - k as i32 });
    }
    v
}

/// What the box shows after an image is pasted (QR code read).
pub const IMAGE_MARK: &str = "[image]";
/// …pasted after a "<": its text read (OCR).
pub const OCR_MARK: &str = "<[image]";

pub fn is_image_mark(s: &str) -> bool {
    s == IMAGE_MARK || s == OCR_MARK
}

/// Ctrl+V with a picture on the clipboard: read the QR code in it, or with
/// `ocr` (a "<" typed first) the text in it.
pub fn read_pasted(ocr: bool) {
    let mark = if ocr { OCR_MARK } else { IMAGE_MARK };
    if let Ok(mut l) = LATEST.lock() {
        *l = mark.to_string();
    }
    let tr = crate::i18n::tr;
    if ocr {
        // The document view at once; its text comes in as it's read.
        let id = OCR_ID.fetch_add(1, Ordering::SeqCst) + 1;
        let engine = if crate::gemini::has_key() { format!("Gemini {}", crate::gemini::short_name(&crate::gemini::MODELS[crate::gemini::ask_choice().min(2)].2)) } else { "Windows OCR".to_string() };
        if let Ok(mut d) = OCR_DOC.lock() {
            *d = Some(OcrDoc { id, text: String::new(), done: false, error: String::new(), engine, note: String::new(), started: std::time::Instant::now() });
        }
        publish(doc_results(mark, id));
    } else {
        publish(Results { query: mark.into(), items: vec![answer(tr("Reading the QR code…").to_string(), String::new(), Kind::Thinking)], engine: Engine::Ready, total: 0, pending: false });
    }
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let items = if ocr {
            read_doc(mark);
            return;
        } else {
            vec![match crate::qr::read_clipboard() {
                Ok(t) => {
                    let url = t.starts_with("http://") || t.starts_with("https://");
                    let dir = tr(if url { "QR code · Enter opens the link, Ctrl+Enter copies" } else { "QR code · Enter copies" }).to_string();
                    // A link: the site's own icon, sharp (64 px), instead of the browser's.
                    let web = url.then(|| {
                        let hit = crate::answers::WebHit { url: t.clone(), ..Default::default() };
                        let icon = crate::answers::favicons(&[hit]).pop().flatten();
                        std::sync::Arc::new(WebMeta { site: crate::answers::site_name(&t), crumb: String::new(), snippet: String::new(), icon })
                    });
                    Item { snip: None, qr: None, web, name: t.clone(), target: t, dir, kind: Kind::QrText, score: 20_000 }
                }
                // No code in it: say how to read its text instead.
                Err(e) => answer(e, tr("Type < and paste again to read the text in it").to_string(), Kind::Error),
            }]
        };
        if LATEST.lock().map(|l| *l == mark).unwrap_or(false) {
            publish(Results { query: mark.into(), items, engine: Engine::Ready, total: 0, pending: false });
        }
    });
}

/// A pasted picture's text (layout kept, as Markdown), as it's read.
#[derive(Clone, Debug)]
pub struct OcrDoc {
    pub id: u64,
    pub text: String,
    pub done: bool,
    pub error: String,
    /// Who read it: "Gemini 3.6 Flash" or "Windows OCR".
    pub engine: String,
    /// Shown above it (why Windows read it instead of Gemini).
    pub note: String,
    pub started: std::time::Instant,
}

static OCR_DOC: Mutex<Option<OcrDoc>> = Mutex::new(None);
static OCR_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Show the last picture's text again (after its translation closed).
pub fn show_doc_again() {
    let Some(d) = ocr_doc() else { return };
    if let Ok(mut l) = LATEST.lock() {
        *l = OCR_MARK.to_string();
    }
    publish(doc_results(OCR_MARK, d.id));
}

pub fn ocr_doc() -> Option<OcrDoc> {
    OCR_DOC.lock().ok().and_then(|d| d.clone())
}

fn doc_results(mark: &str, id: u64) -> Results {
    let tr = crate::i18n::tr;
    let it = Item { snip: None, qr: None, web: None, name: tr("Text in the picture").to_string(), target: format!("ocr:{id}"), dir: String::new(), kind: Kind::OcrDoc, score: 30_000 };
    Results { query: mark.into(), items: vec![it], engine: Engine::Ready, total: 0, pending: false }
}

/// Change the document being read (false once a newer paste replaced it).
fn doc_update(id: u64, f: impl FnOnce(&mut OcrDoc)) -> bool {
    if OCR_ID.load(Ordering::SeqCst) != id {
        return false;
    }
    if let Ok(mut d) = OCR_DOC.lock() {
        if let Some(d) = d.as_mut().filter(|d| d.id == id) {
            f(d);
            return true;
        }
    }
    false
}

/// (On the paste's thread.) Read the picture's text keeping its layout:
/// Gemini when there's a key (tables, columns, headings, bold, any
/// language), Windows' own OCR otherwise (or when Gemini can't).
fn read_doc(mark: &'static str) {
    let id = OCR_ID.load(Ordering::SeqCst);
    let shown = move || LATEST.lock().map(|l| *l == mark).unwrap_or(false) && OCR_ID.load(Ordering::SeqCst) == id;
    let mut why = String::new();
    if crate::gemini::has_key() {
        match crate::ocr::clipboard_image() {
            Some((mime, img)) => {
                let mut last = std::time::Instant::now();
                let r = crate::gemini::ocr_stream(mime, &img, &mut |t| {
                    if !doc_update(id, |d| d.text.push_str(t)) {
                        return false;
                    }
                    if last.elapsed().as_millis() >= 60 && shown() {
                        last = std::time::Instant::now();
                        publish(doc_results(mark, id));
                    }
                    true
                });
                match r {
                    Ok(model) => {
                        doc_update(id, |d| {
                            d.done = true;
                            d.engine = format!("Gemini {model}");
                        });
                        if shown() {
                            publish(doc_results(mark, id));
                        }
                        return;
                    }
                    Err(e) => {
                        crate::util::log(&format!("ocr: Gemini couldn't read the picture ({e}); Windows reads it"));
                        why = e;
                    }
                }
            }
            None => why = crate::i18n::tr("Couldn't read the picture").to_string(),
        }
    }
    let r = crate::ocr::read_clipboard();
    doc_update(id, |d| {
        d.engine = "Windows OCR".into();
        d.done = true;
        if !why.is_empty() {
            d.note = format!("Gemini: {why}");
        }
        match r {
            Ok(t) => d.text = t,
            Err(e) => d.error = e,
        }
    });
    if shown() {
        publish(doc_results(mark, id));
    }
}

fn everything_window() -> Option<HWND> {
    for class in [w!("EVERYTHING_TASKBAR_NOTIFICATION"), w!("EVERYTHING_TASKBAR_NOTIFICATION_(1.5a)")] {
        if let Ok(h) = unsafe { FindWindowW(class, PCWSTR::null()) } {
            if !h.is_invalid() {
                return Some(h);
            }
        }
    }
    None
}

/// Everything installed but not running? Where is it?
fn engine_state() -> Engine {
    if everything_window().is_some() {
        return Engine::Ready;
    }
    let mut dirs = Vec::new();
    for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "LOCALAPPDATA"] {
        if let Ok(p) = std::env::var(var) {
            dirs.push(format!("{p}\\Everything\\Everything.exe"));
            dirs.push(format!("{p}\\Everything 1.5a\\Everything64.exe"));
            dirs.push(format!("{p}\\Programs\\Everything\\Everything.exe"));
        }
    }
    match dirs.into_iter().find(|p| std::path::Path::new(p).exists()) {
        Some(p) => Engine::NotRunning(p),
        None => Engine::Missing,
    }
}

/// Start Everything in the background (no window).
pub fn start_everything(exe: &str) {
    unsafe {
        let f = crate::util::wide(exe);
        ShellExecuteW(None, w!("open"), PCWSTR(f.as_ptr()), w!("-startup"), PCWSTR::null(), SW_SHOWNOACTIVATE);
    }
}

fn read_u32(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// EVERYTHING_IPC_LIST2 → (name, folder, is_folder, run count)
fn parse_list2(d: &[u8]) -> (Vec<(String, String, bool, u32)>, u32) {
    let mut out = Vec::new();
    let (Some(total), Some(num), Some(req)) = (read_u32(d, 0), read_u32(d, 4), read_u32(d, 12)) else { return (out, 0) };
    let read_str = |at: &mut usize| -> Option<String> {
        let len = read_u32(d, *at)? as usize;
        *at += 4;
        let bytes = d.get(*at..*at + len * 2)?;
        *at += (len + 1) * 2;
        let w: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        Some(String::from_utf16_lossy(&w))
    };
    for i in 0..num.min(MAX_FILES as u32) as usize {
        let base = 20 + i * 8;
        let (Some(flags), Some(off)) = (read_u32(d, base), read_u32(d, base + 4)) else { break };
        let mut at = off as usize;
        let mut name = String::new();
        let mut dir = String::new();
        let mut run = 0;
        if req & REQ_NAME != 0 {
            name = read_str(&mut at).unwrap_or_default();
        }
        if req & REQ_PATH != 0 {
            dir = read_str(&mut at).unwrap_or_default();
        }
        if req & REQ_RUN_COUNT != 0 {
            run = read_u32(d, at).unwrap_or(0);
        }
        if !name.is_empty() {
            out.push((name, dir, flags & FOLDER != 0, run));
        }
    }
    (out, total)
}

// ---------------------------------------------------------------------------
// Ranking
// ---------------------------------------------------------------------------

/// How well a name answers the query (0 = not at all).
fn name_score(name: &str, q: &str) -> i32 {
    let n = name.to_lowercase();
    let q = q.to_lowercase();
    let stem = n.rsplit_once('.').map(|(s, _)| s).unwrap_or(&n);
    if n == q || stem == q {
        return 100;
    }
    if n.starts_with(&q) {
        return 70;
    }
    // Every word of the query starts a word of the name ("vs code" → "Visual Studio Code" no, "stu co" → yes).
    let words: Vec<&str> = n.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let qs: Vec<&str> = q.split_whitespace().collect();
    if !qs.is_empty() && qs.iter().all(|qw| words.iter().any(|w| w.starts_with(qw))) {
        return 50;
    }
    // Initials: "vsc" → Visual Studio Code
    if q.len() >= 2 && !q.contains(' ') {
        let initials: String = words.iter().filter_map(|w| w.chars().next()).collect();
        if initials.starts_with(&q) {
            return 40;
        }
    }
    if qs.iter().all(|qw| n.contains(qw)) {
        return 20;
    }
    0
}

fn merge(q: &str, mut items: Vec<Item>, files: Vec<(String, String, bool, u32)>, total: u32, engine: Engine, pending: bool) -> Results {
    for (name, dir, folder, run) in files {
        let full = if dir.is_empty() { name.clone() } else { format!("{dir}\\{name}") };
        let lower = full.to_lowercase();
        // Start-menu shortcuts are already listed as apps.
        if items.iter().any(|i| i.kind == Kind::App && i.dir.eq_ignore_ascii_case(&full)) {
            continue;
        }
        let mut s = name_score(&name, q).max(5);
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
        if matches!(ext.as_str(), "exe" | "lnk" | "appref-ms") {
            s += 15;
        }
        s += (run.min(20) * 4) as i32;
        if lower.contains("\\windows\\winsxs") || lower.contains("\\$recycle.bin") || lower.contains("\\appdata\\local\\temp") || lower.contains("\\.git\\") || lower.contains("\\node_modules\\") {
            s -= 60;
        }
        s -= lower.matches('\\').count() as i32;
        items.push(Item { snip: None, qr: None, web: None, name, dir, target: full, kind: if folder { Kind::Folder } else { Kind::File }, score: s });
    }
    items.sort_by(|a, b| b.score.cmp(&a.score));
    // "Search the web for …" stays where it's seen: 8th row at most.
    let actions: Vec<Item> = items.iter().filter(|i| i.kind == Kind::WebAction).cloned().collect();
    items.retain(|i| i.kind != Kind::WebAction);
    items.truncate(MAX_FILES);
    let at = items.len().min(8 - actions.len().min(8));
    for (k, a) in actions.into_iter().enumerate() {
        items.insert(at + k, a);
    }
    Results { query: q.to_string(), items, engine, total, pending }
}

// ---------------------------------------------------------------------------
// Apps
// ---------------------------------------------------------------------------

fn match_apps(all: &[Item], q: &str) -> Vec<Item> {
    let mut v: Vec<Item> = all
        .iter()
        .filter_map(|a| {
            let s = name_score(&a.name, q);
            (s > 0).then(|| Item { snip: None, qr: None, web: None, score: s + 40, ..a.clone() })
        })
        .collect();
    v.sort_by(|a, b| b.score.cmp(&a.score).then(a.name.len().cmp(&b.name.len())));
    v.truncate(6);
    v
}

/// Everything in Start's "All apps" (desktop and Store apps alike).
fn load_apps() -> Vec<Item> {
    let mut out = Vec::new();
    if let Ok(v) = std::env::var("ISLET_FAKE_APPS") {
        for n in v.split(';') {
            out.push(Item { snip: None, qr: None, web: None, name: n.to_string(), dir: String::new(), target: n.to_string(), kind: Kind::App, score: 0 });
        }
        return out;
    }
    unsafe {
        let r = (|| -> Result<()> {
            let folder: IShellItem = SHCreateItemFromParsingName(w!("shell:AppsFolder"), None)?;
            let en: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems)?;
            loop {
                let mut got = [None];
                let mut n = 0u32;
                if en.Next(&mut got, Some(&mut n)).is_err() || n == 0 {
                    break;
                }
                let Some(it) = got[0].take() else { break };
                let name = it.GetDisplayName(SIGDN_NORMALDISPLAY).ok().map(|p| {
                    let s = p.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(p.0 as *const _));
                    s
                });
                let id = it.GetDisplayName(SIGDN_PARENTRELATIVEPARSING).ok().map(|p| {
                    let s = p.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(p.0 as *const _));
                    s
                });
                if let (Some(name), Some(id)) = (name, id) {
                    let l = name.to_lowercase();
                    if name.is_empty() || l.starts_with("uninstall") || l.contains("readme") {
                        continue;
                    }
                    // Desktop apps' ids are often their exe path: keep it to skip duplicates from Everything.
                    let dir = if id.contains(":\\") { id.clone() } else { String::new() };
                    out.push(Item { snip: None, qr: None, web: None, name, dir, target: format!("shell:AppsFolder\\{id}"), kind: Kind::App, score: 0 });
                }
            }
            Ok(())
        })();
        if let Err(e) = r {
            crate::util::log(&format!("search: apps folder: {e}"));
        }
    }
    if out.is_empty() {
        // Older systems / Wine: the Start menu folders themselves.
        let mut dirs = Vec::new();
        if let Ok(p) = std::env::var("ProgramData") {
            dirs.push(format!("{p}\\Microsoft\\Windows\\Start Menu\\Programs"));
        }
        if let Ok(p) = std::env::var("APPDATA") {
            dirs.push(format!("{p}\\Microsoft\\Windows\\Start Menu\\Programs"));
        }
        for d in dirs {
            walk(std::path::Path::new(&d), 0, &mut out);
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    out
}

fn walk(dir: &std::path::Path, depth: u32, out: &mut Vec<Item>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth < 3 {
                walk(&p, depth + 1, out);
            }
            continue;
        }
        let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !matches!(ext.as_str(), "lnk" | "url" | "appref-ms") {
            continue;
        }
        let name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if name.to_lowercase().starts_with("uninstall") {
            continue;
        }
        let full = p.to_string_lossy().into_owned();
        out.push(Item { snip: None, qr: None, web: None, name, dir: full.clone(), target: full, kind: Kind::App, score: 0 });
    }
}

// ---------------------------------------------------------------------------
// Calculator: "12*(3+4)", "2^10", "15%"
// ---------------------------------------------------------------------------

/// "2024-10-23", "23/10/2024", "10-23": a date, not a sum.
fn looks_date(q: &str) -> bool {
    let t = q.trim();
    for sep in ['-', '/', '.'] {
        let parts: Vec<&str> = t.split(sep).collect();
        let digits = parts.iter().all(|p| !p.is_empty() && p.len() <= 4 && p.chars().all(|c| c.is_ascii_digit()));
        if digits && (parts.len() == 3 || (parts.len() == 2 && parts.iter().any(|p| p.len() == 4))) {
            return true;
        }
    }
    false
}

pub fn calc(q: &str) -> Option<String> {
    let s: String = q.chars().filter(|c| !c.is_whitespace()).collect::<String>().replace(',', ".").replace('×', "*").replace('÷', "/");
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit() || "+-*/^%().".contains(c)) || !s.chars().any(|c| "+-*/^%".contains(c)) || !s.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let b = s.as_bytes();
    let mut i = 0;
    let v = expr(b, &mut i)?;
    if i != b.len() || !v.is_finite() {
        return None;
    }
    let r = (v * 1e10).round() / 1e10;
    Some(if r.fract() == 0.0 && r.abs() < 1e15 { format!("{}", r as i64) } else { format!("{r}") })
}

fn expr(b: &[u8], i: &mut usize) -> Option<f64> {
    let mut v = term(b, i)?;
    while *i < b.len() && (b[*i] == b'+' || b[*i] == b'-') {
        let op = b[*i];
        *i += 1;
        let r = term(b, i)?;
        v = if op == b'+' { v + r } else { v - r };
    }
    Some(v)
}

fn term(b: &[u8], i: &mut usize) -> Option<f64> {
    let mut v = power(b, i)?;
    while *i < b.len() && (b[*i] == b'*' || b[*i] == b'/') {
        let op = b[*i];
        *i += 1;
        let r = power(b, i)?;
        v = if op == b'*' { v * r } else { v / r };
    }
    Some(v)
}

fn power(b: &[u8], i: &mut usize) -> Option<f64> {
    let base = unary(b, i)?;
    if *i < b.len() && b[*i] == b'^' {
        *i += 1;
        let e = power(b, i)?;
        return Some(base.powf(e));
    }
    Some(base)
}

fn unary(b: &[u8], i: &mut usize) -> Option<f64> {
    if *i < b.len() && b[*i] == b'-' {
        *i += 1;
        return unary(b, i).map(|v| -v);
    }
    let mut v = atom(b, i)?;
    if *i < b.len() && b[*i] == b'%' {
        *i += 1;
        v /= 100.0;
    }
    Some(v)
}

fn atom(b: &[u8], i: &mut usize) -> Option<f64> {
    if *i < b.len() && b[*i] == b'(' {
        *i += 1;
        let v = expr(b, i)?;
        if *i < b.len() && b[*i] == b')' {
            *i += 1;
            return Some(v);
        }
        return None;
    }
    let st = *i;
    while *i < b.len() && (b[*i].is_ascii_digit() || b[*i] == b'.') {
        *i += 1;
    }
    std::str::from_utf8(&b[st..*i]).ok()?.parse().ok()
}

/// Open a result (Enter / click). `folder`: show it in Explorer instead.
pub fn open(it: &Item, folder: bool) {
    unsafe {
        match it.kind {
            Kind::Dict | Kind::OcrText if it.target.is_empty() => {}
            Kind::OcrDoc => {
                if let Some(d) = ocr_doc() {
                    crate::util::set_clipboard_text(&d.text);
                }
            }
            Kind::Calc | Kind::Currency | Kind::Base64 | Kind::Translate | Kind::Dict | Kind::OcrText => crate::util::set_clipboard_text(&it.target),
            Kind::Gemini => {
                if let Some(l) = crate::gemini::live(crate::gemini::ASK) {
                    crate::util::set_clipboard_text(&l.shown());
                }
            }
            Kind::QrCode => {
                if let Some(m) = &it.qr {
                    crate::qr::copy_image(m);
                }
            }
            Kind::QrText => {
                let t = it.target.trim();
                if !folder && (t.starts_with("http://") || t.starts_with("https://")) {
                    crate::util::shell_open(t);
                } else {
                    crate::util::set_clipboard_text(t);
                }
            }
            Kind::Thinking | Kind::WebAction | Kind::Error => {}
            Kind::Web if folder => crate::util::set_clipboard_text(&it.target),
            Kind::Web => crate::util::shell_open(&it.target),
            Kind::App if it.target.starts_with("shell:") => {
                let t = crate::util::wide(&it.target);
                ShellExecuteW(None, w!("open"), w!("explorer.exe"), PCWSTR(t.as_ptr()), PCWSTR::null(), SW_SHOWNORMAL);
            }
            _ if folder => {
                let arg = crate::util::wide(&format!("/select,\"{}\"", it.target));
                ShellExecuteW(None, w!("open"), w!("explorer.exe"), PCWSTR(arg.as_ptr()), PCWSTR::null(), SW_SHOWNORMAL);
            }
            _ => crate::util::shell_open(&it.target),
        }
    }
    // Tell Everything, so things you open often rank first next time.
    if matches!(it.kind, Kind::File | Kind::Folder) {
        if let Some(ev) = everything_window() {
            let w: Vec<u16> = it.target.encode_utf16().chain(std::iter::once(0)).collect();
            let cds = COPYDATASTRUCT { dwData: 24, cbData: (w.len() * 2) as u32, lpData: w.as_ptr() as *mut _ };
            let host = HOST.load(Ordering::Relaxed);
            unsafe {
                let _ = SendMessageTimeoutW(ev, WM_COPYDATA, WPARAM(host as usize), LPARAM(&cds as *const _ as isize), SMTO_ABORTIFHUNG, 300, None);
            }
        }
    }
}
