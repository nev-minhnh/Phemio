//! Link: the clipboard (text, pictures, files) and the shelf, between linked PCs.
//!
//! Text goes over the session as soon as it's copied. Pictures and files go
//! over a second connection (encrypted the same way) so the pointer never
//! waits behind a big transfer: pictures as soon as they're copied, files
//! when the pointer crosses to the other PC (so copying a folder you only
//! paste here doesn't send it). Received files land in a folder of their own
//! and are put on the clipboard, ready for Ctrl+V.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND, POINT};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::UI::Shell::{DragQueryFileW, DROPFILES, HDROP};

use super::proto::{self, Msg};

const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const CF_DIB: u32 = 8;

/// Files bigger than this (all together) aren't sent by copy & paste.
pub const MAX_CLIP_FILES: u64 = 4 << 30;
/// Text bigger than this isn't sent (it has to fit in one message).
const MAX_TEXT: usize = 600 * 1024;
const CHUNK: usize = 256 * 1024;

#[derive(Clone, Debug)]
pub enum Clip {
    Text(String),
    /// A picture: the clipboard's CF_DIB bytes.
    Image(Vec<u8>),
    Files(Vec<PathBuf>),
}

/// The clipboard change we made ourselves (so it isn't sent back).
static OURS: AtomicU32 = AtomicU32::new(0);
/// The last clipboard change sent to the other PC.
static SENT: AtomicU32 = AtomicU32::new(0);
/// The last text that went either way (the same text isn't sent back and forth).
static LAST_TEXT: Mutex<u64> = Mutex::new(0);

fn text_hash(t: &str) -> u64 {
    bytes_hash(t.as_bytes())
}

fn bytes_hash(b: &[u8]) -> u64 {
    use sha2::Digest;
    let h = sha2::Sha256::digest(b);
    u64::from_le_bytes(h[..8].try_into().unwrap_or([0; 8]))
}

/// The last picture / file list that went either way (never sent back and forth).
static LAST_IMAGE: Mutex<u64> = Mutex::new(0);
static LAST_FILES: Mutex<u64> = Mutex::new(0);

fn files_hash(v: &[PathBuf]) -> u64 {
    let mut s = String::new();
    for p in v {
        let md = std::fs::metadata(p).ok();
        s.push_str(&format!("{}|{}|{:?}\n", p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default(), md.as_ref().map(|m| m.len()).unwrap_or(0), md.and_then(|m| m.modified().ok())));
    }
    bytes_hash(s.as_bytes())
}

/// One outgoing transfer at a time.
static SENDING: Mutex<()> = Mutex::new(());

pub fn seq() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

fn open(owner: Option<HWND>) -> bool {
    // (Short: on the window's thread this must never hold things up.)
    let tries = if owner.is_some() { 4 } else { 10 };
    for _ in 0..tries {
        if unsafe { OpenClipboard(owner) }.is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(if owner.is_some() { 5 } else { 15 }));
    }
    false
}

fn global_bytes(h: HANDLE) -> Option<Vec<u8>> {
    unsafe {
        let g = HGLOBAL(h.0);
        let n = GlobalSize(g);
        let p = GlobalLock(g) as *const u8;
        if p.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(p, n).to_vec();
        let _ = GlobalUnlock(g);
        Some(v)
    }
}

/// What's on the clipboard now (files first, then text, then a picture).
pub fn read() -> Option<Clip> {
    unsafe {
        let has = |f: u32| IsClipboardFormatAvailable(f).is_ok();
        let (files, text, image) = (has(CF_HDROP), has(CF_UNICODETEXT), has(CF_DIB));
        if !files && !text && !image {
            return None;
        }
        if !open(None) {
            return None;
        }
        let mut out = None;
        if files {
            if let Ok(h) = GetClipboardData(CF_HDROP) {
                let drop = HDROP(h.0);
                let n = DragQueryFileW(drop, u32::MAX, None);
                let mut v = Vec::new();
                for i in 0..n.min(10_000) {
                    let len = DragQueryFileW(drop, i, None);
                    let mut b = vec![0u16; len as usize + 1];
                    DragQueryFileW(drop, i, Some(&mut b));
                    v.push(PathBuf::from(crate::util::from_wide(&b)));
                }
                if !v.is_empty() {
                    out = Some(Clip::Files(v));
                }
            }
        }
        if out.is_none() && text {
            if let Ok(h) = GetClipboardData(CF_UNICODETEXT) {
                if let Some(b) = global_bytes(h) {
                    let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
                    let t = String::from_utf16_lossy(&w);
                    if !t.is_empty() {
                        out = Some(Clip::Text(t));
                    }
                }
            }
        }
        if out.is_none() && image {
            if let Ok(h) = GetClipboardData(CF_DIB) {
                if let Some(b) = global_bytes(h) {
                    if b.len() > 40 {
                        out = Some(Clip::Image(b));
                    }
                }
            }
        }
        let _ = CloseClipboard();
        out
    }
}

fn put(fmt: u32, bytes: &[u8]) -> bool {
    unsafe {
        let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) else { return false };
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(h);
        SetClipboardData(fmt, Some(HANDLE(h.0))).is_ok()
    }
}

/// Developer aid: ISLET_DEMO_CLIP="text:hello" or "files:C:\\a.txt|C:\\dir" is
/// "copied" here a few seconds after start (as if you had copied it).
pub fn demo() -> Option<Clip> {
    let v = std::env::var("ISLET_DEMO_CLIP").ok()?;
    let (k, rest) = v.split_once(':')?;
    Some(match k {
        "text" => Clip::Text(rest.to_string()),
        _ => Clip::Files(rest.split('|').map(PathBuf::from).collect()),
    })
}

/// `set`, as if copied here (so it goes to the other PC).
pub fn set_local(owner: HWND, c: &Clip) {
    set(owner, c);
    OURS.store(0, Ordering::SeqCst);
}

/// Put what the other PC copied on this clipboard (UI thread).
pub fn set(owner: HWND, c: &Clip) {
    if !open(Some(owner)) {
        crate::util::log("link: couldn't open the clipboard");
        return;
    }
    unsafe {
        let _ = EmptyClipboard();
    }
    match c {
        Clip::Text(t) => {
            let b: Vec<u8> = t.encode_utf16().chain(std::iter::once(0)).flat_map(|c| c.to_le_bytes()).collect();
            put(CF_UNICODETEXT, &b);
        }
        Clip::Image(b) => {
            put(CF_DIB, b);
        }
        Clip::Files(v) => {
            let mut b = Vec::new();
            let head = DROPFILES { pFiles: std::mem::size_of::<DROPFILES>() as u32, pt: POINT::default(), fNC: false.into(), fWide: true.into() };
            b.extend_from_slice(unsafe { std::slice::from_raw_parts(&head as *const _ as *const u8, std::mem::size_of::<DROPFILES>()) });
            for p in v {
                for c in p.to_string_lossy().encode_utf16().chain(std::iter::once(0)) {
                    b.extend_from_slice(&c.to_le_bytes());
                }
            }
            b.extend_from_slice(&[0, 0]);
            put(CF_HDROP, &b);
            // Paste copies (never moves) them.
            let name = crate::util::wide("Preferred DropEffect");
            let f = unsafe { RegisterClipboardFormatW(PCWSTR(name.as_ptr())) };
            if f != 0 {
                put(f, &1u32.to_le_bytes());
            }
        }
    }
    unsafe {
        let _ = CloseClipboard();
    }
    OURS.store(seq(), Ordering::SeqCst);
}

/// The clipboard changed here (`eager`) or the pointer just went to the
/// other PC: send what's new. Text and pictures go right away; files only
/// when the pointer crosses.
pub fn offer(eager: bool) {
    if !super::connected() {
        return;
    }
    let _ = std::thread::Builder::new().name("link-clip".into()).spawn(move || {
        super::guard("clipboard", || {
            let _one = SENDING.lock().unwrap_or_else(|e| e.into_inner());
            let s = seq();
            if s == OURS.load(Ordering::SeqCst) || s == SENT.load(Ordering::SeqCst) {
                return;
            }
            let Some(c) = read() else { return };
            match c {
                Clip::Text(t) => {
                    let h = text_hash(&t);
                    let mut last = LAST_TEXT.lock().unwrap_or_else(|e| e.into_inner());
                    if *last == h {
                        SENT.store(s, Ordering::SeqCst);
                        return;
                    }
                    if t.len() <= MAX_TEXT {
                        *last = h;
                        SENT.store(s, Ordering::SeqCst);
                        crate::util::log(&format!("link: copied text sent ({} characters)", t.chars().count()));
                        super::send(Msg::Clip { text: t });
                    }
                }
                Clip::Image(b) => {
                    SENT.store(s, Ordering::SeqCst);
                    let h = bytes_hash(&b);
                    {
                        let mut last = LAST_IMAGE.lock().unwrap_or_else(|e| e.into_inner());
                        if *last == h || b.len() > 256 << 20 {
                            return;
                        }
                        *last = h;
                    }
                    if let Err(e) = send_batch(proto::BATCH_IMAGE, Source::Bytes("picture.dib".into(), b)) {
                        super::set_xfer_done(format!("{}: {e}", crate::i18n::tr("Couldn't send the picture")));
                    }
                }
                Clip::Files(v) if !eager => {
                    SENT.store(s, Ordering::SeqCst);
                    // (Files that came from the other PC aren't sent back.)
                    let h = files_hash(&v);
                    {
                        let mut last = LAST_FILES.lock().unwrap_or_else(|e| e.into_inner());
                        if *last == h {
                            return;
                        }
                        *last = h;
                    }
                    if let Err(e) = send_batch(proto::BATCH_CLIPBOARD, Source::Paths(v)) {
                        super::set_xfer_done(format!("{}: {e}", crate::i18n::tr("Couldn't send the files")));
                    }
                }
                Clip::Files(_) => {}
            }
        });
    });
}

/// Text the other PC copied.
pub fn got_text(t: String) {
    crate::util::log(&format!("link: copied text received ({} characters)", t.chars().count()));
    *LAST_TEXT.lock().unwrap_or_else(|e| e.into_inner()) = text_hash(&t);
    super::ui_job(super::UiJob::Clip(Clip::Text(t)));
}

// ---------------------------------------------------------------------------
// Sending a batch
// ---------------------------------------------------------------------------

pub enum Source {
    Paths(Vec<PathBuf>),
    Bytes(String, Vec<u8>),
}

struct Entry {
    rel: String,
    src: Option<PathBuf>,
    size: u64,
    dir: bool,
}

fn walk(root: &Path, rel: &str, out: &mut Vec<Entry>, depth: u32) {
    let Ok(md) = std::fs::metadata(root) else { return };
    if md.is_dir() {
        out.push(Entry { rel: rel.to_string(), src: None, size: 0, dir: true });
        if depth > 32 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(root) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            walk(&e.path(), &format!("{rel}/{name}"), out, depth + 1);
        }
    } else {
        out.push(Entry { rel: rel.to_string(), src: Some(root.to_path_buf()), size: md.len(), dir: false });
    }
}

/// A connection for files to the PC we're linked with.
fn files_connection() -> std::io::Result<(TcpStream, proto::Cipher, proto::Cipher)> {
    let (addr, peer) = super::session_addr().ok_or_else(|| proto::bad("not linked"))?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(4))?;
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(Duration::from_secs(30)))?;
    s.set_write_timeout(Some(Duration::from_secs(30)))?;
    let (me, name) = super::me();
    let (id, _, tx, rx, purpose) = proto::handshake_for(&mut s, me, &name, None, super::secret_for, proto::FOR_FILES)?;
    if id != peer || purpose != proto::FOR_FILES {
        return Err(proto::bad("the other PC didn't take the files"));
    }
    Ok((s, tx, rx))
}

pub fn send_batch(kind: u8, src: Source) -> std::io::Result<()> {
    let mut list = Vec::new();
    let mut bytes = None;
    match src {
        Source::Paths(v) => {
            for p in &v {
                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
                walk(p, &name, &mut list, 0);
            }
        }
        Source::Bytes(name, b) => {
            list.push(Entry { rel: name, src: None, size: b.len() as u64, dir: false });
            bytes = Some(b);
        }
    }
    let total: u64 = list.iter().map(|e| e.size).sum();
    if list.is_empty() {
        return Ok(());
    }
    if kind == proto::BATCH_CLIPBOARD && total > MAX_CLIP_FILES {
        return Err(proto::bad(&format!("{:.1} GB is too much for copy & paste — put them on the shelf instead", total as f64 / (1u64 << 30) as f64)));
    }
    let files = list.iter().filter(|e| !e.dir).count();
    let what = describe(&list);
    let (mut s, mut tx, mut rx) = files_connection()?;
    let id = u64::from_le_bytes(proto::random::<8>());
    proto::write_sealed(&mut s, &mut tx, &Msg::Batch { id, kind, count: list.len() as u32, total })?;
    let mut done = 0u64;
    let started = Instant::now();
    let mut last_tell = crate::util::ago(Duration::from_secs(1));
    let mut buf = vec![0u8; CHUNK];
    for e in &list {
        proto::write_sealed(&mut s, &mut tx, &Msg::FileHead { rel: e.rel.clone(), size: e.size, dir: e.dir })?;
        if e.dir {
            continue;
        }
        let mut left = e.size;
        if let Some(b) = &bytes {
            for c in b.chunks(CHUNK) {
                proto::write_sealed(&mut s, &mut tx, &Msg::Data(c.to_vec()))?;
                done += c.len() as u64;
                left -= c.len() as u64;
                if kind != proto::BATCH_IMAGE && last_tell.elapsed() > Duration::from_millis(150) {
                    last_tell = Instant::now();
                    super::set_xfer(true, &what, done, total, started);
                }
            }
        } else if let Some(p) = &e.src {
            let mut f = std::fs::File::open(p)?;
            while left > 0 {
                let want = (left as usize).min(CHUNK);
                let n = f.read(&mut buf[..want])?;
                if n == 0 {
                    // The file got shorter while we were sending it: pad it.
                    buf[..want].fill(0);
                    proto::write_sealed(&mut s, &mut tx, &Msg::Data(buf[..want].to_vec()))?;
                    left -= want as u64;
                    done += want as u64;
                    continue;
                }
                proto::write_sealed(&mut s, &mut tx, &Msg::Data(buf[..n].to_vec()))?;
                // (Developer aid: ISLET_LINK_SLOW=<ms> per chunk, to watch the progress.)
                if let Some(ms) = std::env::var("ISLET_LINK_SLOW").ok().and_then(|v| v.parse::<u64>().ok()) {
                    std::thread::sleep(Duration::from_millis(ms));
                }
                left -= n as u64;
                done += n as u64;
                if last_tell.elapsed() > Duration::from_millis(150) {
                    last_tell = Instant::now();
                    super::set_xfer(true, &what, done, total, started);
                }
            }
        }
    }
    proto::write_sealed(&mut s, &mut tx, &Msg::BatchEnd)?;
    let _ = s.flush();
    s.set_read_timeout(Some(Duration::from_secs(120)))?;
    let ok = matches!(proto::read_sealed(&mut s, &mut rx)?, Msg::BatchAck { ok: true });
    let peer = super::peer_name();
    if !ok {
        super::set_xfer_done(format!("{peer}: {}", crate::i18n::tr("couldn't save them")));
        return Ok(());
    }
    if kind == proto::BATCH_IMAGE {
        super::set_xfer_done(String::new());
    } else {
        let _ = files;
        super::set_xfer_done(format!("{} {what} → {peer}", crate::i18n::tr("Sent")));
    }
    Ok(())
}

fn describe(list: &[Entry]) -> String {
    let top: Vec<&Entry> = list.iter().filter(|e| !e.rel.contains('/')).collect();
    let files = list.iter().filter(|e| !e.dir).count();
    if top.len() == 1 {
        let n = &top[0].rel;
        if top[0].dir {
            format!("{n} ({files} {})", crate::i18n::tr(if files == 1 { "file" } else { "files" }))
        } else {
            n.clone()
        }
    } else {
        format!("{} {}", top.len(), crate::i18n::tr("items"))
    }
}

// ---------------------------------------------------------------------------
// Receiving a batch
// ---------------------------------------------------------------------------

/// A name that's safe to create here (no "..", drive letters or odd characters).
fn clean_rel(rel: &str) -> Option<PathBuf> {
    let mut p = PathBuf::new();
    for part in rel.split(['/', '\\']) {
        let c: String = part.chars().filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') && !c.is_control()).collect();
        let c = c.trim_end_matches(['.', ' ']).trim_start();
        if c.is_empty() || c == "." || c == ".." {
            continue;
        }
        p.push(c);
    }
    (!p.as_os_str().is_empty()).then_some(p)
}

/// `name` in `dir`, or "name (2)" if that's taken.
fn free_name(dir: &Path, name: &Path) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let stem = name.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = name.extension().map(|s| format!(".{}", s.to_string_lossy())).unwrap_or_default();
    for i in 2..1000 {
        let p = dir.join(format!("{stem} ({i}){ext}"));
        if !p.exists() {
            return p;
        }
    }
    dir.join(format!("{stem} {}{ext}", super::unix_ms()))
}

pub fn downloads() -> PathBuf {
    use windows::Win32::UI::Shell::{FOLDERID_Downloads, SHGetKnownFolderPath, KNOWN_FOLDER_FLAG};
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Downloads, KNOWN_FOLDER_FLAG(0), None) {
            let s = p.to_string().unwrap_or_default();
            windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
            if !s.is_empty() {
                return PathBuf::from(s);
            }
        }
    }
    std::env::var("USERPROFILE").map(|h| PathBuf::from(h).join("Downloads")).unwrap_or_else(|_| crate::config::dir())
}

/// Where shelf items from the other PC go (Settings → Pages → Shelf).
pub fn shelf_dir() -> PathBuf {
    let d = crate::config::cur().s("shelf_dir");
    if d.trim().is_empty() {
        downloads().join("Phemio")
    } else {
        PathBuf::from(d.trim())
    }
}

/// Where pasted files go: a folder per paste, cleared after a day.
fn clip_dir() -> PathBuf {
    let base = crate::config::dir().join("link-clipboard");
    if let Ok(rd) = std::fs::read_dir(&base) {
        for e in rd.flatten() {
            let old = e.metadata().and_then(|m| m.modified()).map(|t| t.elapsed().map(|d| d > Duration::from_secs(86_400)).unwrap_or(false)).unwrap_or(false);
            if old {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    base.join(format!("{}", super::unix_ms()))
}

/// The other PC opened a connection for files: take the batch.
pub fn receive(mut s: TcpStream, mut tx: proto::Cipher, mut rx: proto::Cipher) -> std::io::Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(60)))?;
    let Msg::Batch { kind, count, total, .. } = proto::read_sealed(&mut s, &mut rx)? else { return Err(proto::bad("expected a batch")) };
    crate::util::log(&format!("link: receiving {count} item(s), {total} bytes (kind {kind})"));
    let peer = super::peer_name();
    let base = match kind {
        proto::BATCH_SHELF => shelf_dir(),
        _ => clip_dir(),
    };
    let image = kind == proto::BATCH_IMAGE;
    if !image {
        std::fs::create_dir_all(&base)?;
    }
    let started = Instant::now();
    let mut last_tell = crate::util::ago(Duration::from_secs(1));
    let mut done = 0u64;
    let mut tops: Vec<PathBuf> = Vec::new();
    // A top-level name → where it really went (if renamed to stay clear of what's there).
    let mut renamed: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut pic = Vec::new();
    let mut ok = true;
    let mut what = String::new();
    let mut seen = 0u32;
    loop {
        match proto::read_sealed(&mut s, &mut rx)? {
            Msg::FileHead { rel, size, dir } => {
                seen += 1;
                if seen > count.saturating_add(1) {
                    return Err(proto::bad("too many files"));
                }
                if image {
                    if size > 512 << 20 {
                        return Err(proto::bad("picture too big"));
                    }
                    pic.reserve(size as usize);
                    let mut left = size;
                    while left > 0 {
                        let Msg::Data(b) = proto::read_sealed(&mut s, &mut rx)? else { return Err(proto::bad("expected data")) };
                        left = left.saturating_sub(b.len() as u64);
                        pic.extend_from_slice(&b);
                    }
                    continue;
                }
                let Some(rel) = clean_rel(&rel) else {
                    skip(&mut s, &mut rx, if dir { 0 } else { size })?;
                    continue;
                };
                // Top-level names don't overwrite what's there (shelf downloads share one folder).
                let mut comps = rel.components();
                let top = PathBuf::from(comps.next().map(|c| c.as_os_str()).unwrap_or_default());
                let rest: PathBuf = comps.collect();
                let top_real = match renamed.iter().find(|(a, _)| *a == top) {
                    Some((_, b)) => b.clone(),
                    None => {
                        let real = free_name(&base, &top);
                        renamed.push((top.clone(), real.clone()));
                        tops.push(real.clone());
                        if what.is_empty() {
                            what = top.to_string_lossy().to_string();
                        }
                        real
                    }
                };
                let path = if rest.as_os_str().is_empty() { top_real } else { top_real.join(rest) };
                if dir {
                    let _ = std::fs::create_dir_all(&path);
                    continue;
                }
                if let Some(p) = path.parent() {
                    let _ = std::fs::create_dir_all(p);
                }
                let mut f = match std::fs::File::create(&path) {
                    Ok(f) => Some(std::io::BufWriter::with_capacity(CHUNK, f)),
                    Err(e) => {
                        crate::util::log(&format!("link: can't save {}: {e}", path.display()));
                        ok = false;
                        None
                    }
                };
                let mut left = size;
                while left > 0 {
                    let Msg::Data(b) = proto::read_sealed(&mut s, &mut rx)? else { return Err(proto::bad("expected data")) };
                    let n = (b.len() as u64).min(left) as usize;
                    if let Some(w) = f.as_mut() {
                        if w.write_all(&b[..n]).is_err() {
                            ok = false;
                            f = None;
                        }
                    }
                    left -= n as u64;
                    done += n as u64;
                    if last_tell.elapsed() > Duration::from_millis(150) {
                        last_tell = Instant::now();
                        super::set_xfer(false, &what, done, total, started);
                    }
                }
                if let Some(mut w) = f {
                    if w.flush().is_err() {
                        ok = false;
                    }
                }
            }
            Msg::BatchEnd => break,
            _ => return Err(proto::bad("unexpected message in a batch")),
        }
    }
    let _ = proto::write_sealed(&mut s, &mut tx, &Msg::BatchAck { ok });
    let _ = s.flush();
    if image {
        super::set_xfer_done(String::new());
        *LAST_IMAGE.lock().unwrap_or_else(|e| e.into_inner()) = bytes_hash(&pic);
        super::ui_job(super::UiJob::Clip(Clip::Image(pic)));
        return Ok(());
    }
    let label = if tops.len() == 1 { what.clone() } else { format!("{} {}", tops.len(), crate::i18n::tr("items")) };
    match kind {
        proto::BATCH_SHELF => {
            super::set_xfer_done(format!("{} {label} ← {peer}", crate::i18n::tr("Got")));
            super::recheck_have();
            super::ui_job(super::UiJob::Shelf(tops.iter().map(|p| p.to_string_lossy().to_string()).collect(), label));
        }
        _ => {
            super::set_xfer_done(format!("{} {label} ← {peer} · Ctrl+V", crate::i18n::tr("Copied")));
            *LAST_FILES.lock().unwrap_or_else(|e| e.into_inner()) = files_hash(&tops);
            super::ui_job(super::UiJob::Clip(Clip::Files(tops)));
        }
    }
    Ok(())
}

fn skip(s: &mut TcpStream, rx: &mut proto::Cipher, mut left: u64) -> std::io::Result<()> {
    while left > 0 {
        let Msg::Data(b) = proto::read_sealed(s, rx)? else { return Err(proto::bad("expected data")) };
        left = left.saturating_sub(b.len() as u64);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The shelf
// ---------------------------------------------------------------------------

static SHELF: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// A stable id for a shelf item (its path).
pub fn shelf_id(path: &str) -> u64 {
    use sha2::Digest;
    let h = sha2::Sha256::digest(path.to_lowercase().as_bytes());
    u64::from_le_bytes(h[..8].try_into().unwrap_or([0; 8]))
}

/// The shelf here changed (also called at start-up).
pub fn shelf_changed(paths: Vec<String>) {
    *SHELF.lock().unwrap_or_else(|e| e.into_inner()) = paths;
    if super::connected() {
        super::send(shelf_list());
        send_shelf_hashes();
    }
}

/// Content hashes, remembered by (path, size, modified) so nothing is read twice.
static HASHES: Mutex<Option<std::collections::HashMap<(PathBuf, u64, u64), u64>>> = Mutex::new(None);

fn file_key(p: &Path) -> Option<(PathBuf, u64, u64)> {
    let md = std::fs::metadata(p).ok()?;
    let m = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    Some((p.to_path_buf(), if md.is_dir() { 0 } else { md.len() }, m))
}

/// What's in a file (its bytes) or a folder (every file's name and bytes):
/// the same number on both PCs when the contents match.
pub fn content_hash(p: &Path) -> Option<u64> {
    use sha2::Digest;
    let md = std::fs::metadata(p).ok()?;
    if md.is_dir() {
        let mut list = Vec::new();
        walk(p, "", &mut list, 0);
        let mut files: Vec<&Entry> = list.iter().filter(|e| !e.dir).collect();
        files.sort_by(|a, b| a.rel.to_lowercase().cmp(&b.rel.to_lowercase()));
        let mut h = sha2::Sha256::new();
        for e in files {
            let fh = e.src.as_deref().and_then(content_hash).unwrap_or(0);
            h.update(e.rel.to_lowercase().as_bytes());
            h.update(fh.to_le_bytes());
        }
        let d = h.finalize();
        return Some(u64::from_le_bytes(d[..8].try_into().ok()?));
    }
    let key = file_key(p)?;
    if let Some(v) = HASHES.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&key).copied()) {
        return Some(v);
    }
    let mut f = std::fs::File::open(p).ok()?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    let d = h.finalize();
    let v = u64::from_le_bytes(d[..8].try_into().ok()?);
    let mut g = HASHES.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(Default::default);
    if m.len() > 5000 {
        m.clear();
    }
    m.insert(key, v);
    Some(v)
}

/// Work out what's in each shelf item here and tell the other PC (so it
/// knows which ones it already has).
pub fn send_shelf_hashes() {
    let paths = SHELF.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let _ = std::thread::Builder::new().name("link-shelf-hash".into()).spawn(move || {
        super::guard("shelf hashes", || {
            let v: Vec<(u64, u64)> = paths.iter().take(200).filter_map(|p| content_hash(Path::new(p)).map(|h| (shelf_id(p), h))).collect();
            if super::connected() {
                super::send(Msg::ShelfHashes(v));
            }
        });
    });
}

/// Which of the other PC's shelf items are already in the shelf folder here
/// (same name — or "name (2)" — and the same contents).
/// Where each of the other PC's shelf items already is here (to open it).
static HAVE_PATHS: Mutex<Vec<(u64, PathBuf)>> = Mutex::new(Vec::new());

/// The local copy of the other PC's shelf item `id`, if it's here.
pub fn have_path(id: u64) -> Option<PathBuf> {
    HAVE_PATHS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|x| x.0 == id).map(|x| x.1.clone())
}

pub fn check_have(items: Vec<(u64, String)>, hashes: Vec<(u64, u64)>) {
    let _ = std::thread::Builder::new().name("link-shelf-have".into()).spawn(move || {
        super::guard("shelf check", || {
            let dir = shelf_dir();
            let mut have = std::collections::HashSet::new();
            let mut paths = Vec::new();
            for (id, name) in &items {
                let Some(h) = hashes.iter().find(|x| x.0 == *id).map(|x| x.1) else { continue };
                let Some(rel) = clean_rel(name) else { continue };
                let stem = rel.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let ext = rel.extension().map(|s| format!(".{}", s.to_string_lossy())).unwrap_or_default();
                let mut cands = vec![dir.join(&rel)];
                for k in 2..10 {
                    cands.push(dir.join(format!("{stem} ({k}){ext}")));
                }
                if let Some(c) = cands.iter().filter(|c| c.exists()).find(|c| content_hash(c) == Some(h)) {
                    have.insert(*id);
                    paths.push((*id, c.clone()));
                }
            }
            *HAVE_PATHS.lock().unwrap_or_else(|e| e.into_inner()) = paths;
            super::set_have(have);
        });
    });
}

/// What's on the shelf here, for the other PC.
pub fn shelf_list() -> Msg {
    let paths = SHELF.lock().unwrap_or_else(|e| e.into_inner()).clone();
    Msg::ShelfList(
        paths
            .iter()
            .take(200)
            .map(|p| {
                let md = std::fs::metadata(p).ok();
                let dir = md.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                let size = md.map(|m| if m.is_dir() { 0 } else { m.len() }).unwrap_or(0);
                let name = Path::new(p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.clone());
                (shelf_id(p), name, size, dir)
            })
            .collect(),
    )
}

/// The other PC wants these shelf items.
pub fn shelf_send(ids: Vec<u64>) {
    let paths: Vec<PathBuf> = SHELF.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|p| ids.contains(&shelf_id(p))).map(PathBuf::from).collect();
    if paths.is_empty() {
        return;
    }
    let _ = std::thread::Builder::new().name("link-shelf".into()).spawn(move || {
        super::guard("shelf", || {
            let _one = SENDING.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = send_batch(proto::BATCH_SHELF, Source::Paths(paths)) {
                super::set_xfer_done(format!("{}: {e}", crate::i18n::tr("Couldn't send the files")));
            }
        });
    });
}
