//! Phemio's own index of what's inside your files, for the search's
//! Content mode (Settings → General → Content search).
//!
//! The drives you pick are read once in the background — Word, Excel,
//! PowerPoint, OpenDocument, PDF, e-books, text and code files (and
//! anything else Windows has a reader for) — and the text goes into a
//! full-text index (SQLite FTS5) in %LOCALAPPDATA%\Phemio. After that,
//! Windows tells Phemio when files change on those drives and they're
//! read a few seconds later; every half hour (and when Phemio starts)
//! everything is checked again for anything missed. A search then answers at once,
//! with the passage around your words from each file: exact words (any
//! case, but accents count), "quoted" text as one phrase.
//!
//! Skipped: Windows' own folders, programs, app data, hidden and system
//! folders, code dependency folders, cloud-only files (they'd be
//! downloaded), and files too big to be documents.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

use crate::config;
use crate::contentsearch::{nfc, passage, term_tokens, words_of};

/// Text kept per file (characters): the index and the passages come from it.
const MAX_TEXT: usize = 300_000;
/// Files bigger than this are skipped (MB): not documents.
const MAX_MB: u64 = 150;
/// Time between looks for new or changed files.
const EVERY: Duration = Duration::from_secs(30 * 60);

const ZIP_KINDS: &[&str] = &["docx", "docm", "dotx", "dotm", "xlsx", "xlsm", "xltx", "pptx", "pptm", "potx", "ppsx", "odt", "ods", "odp", "epub"];
const PDF: &str = "pdf";
/// Kinds read with Windows' own readers (filters), if it has one.
const FILTER_KINDS: &[&str] = &["doc", "dot", "xls", "xlt", "ppt", "pps", "rtf", "msg", "eml", "vsd", "vsdx", "one", "wpd", "xps"];
const TEXT_KINDS: &[&str] = &[
    "txt", "md", "markdown", "log", "csv", "tsv", "json", "xml", "yaml", "yml", "ini", "cfg", "conf", "toml", "html", "htm", "css", "scss", "js", "ts", "tsx", "jsx", "py",
    "ipynb", "rs", "c", "h", "cpp", "hpp", "cc", "cs", "java", "kt", "go", "rb", "php", "sql", "sh", "bat", "cmd", "ps1", "tex", "bib", "srt", "vtt", "r", "m", "swift", "lua",
    "pl", "vb", "dart", "scala", "vue", "svelte",
];

const SKIP_DIRS: &[&str] = &[
    "windows", "program files", "program files (x86)", "programdata", "$recycle.bin", "system volume information", "recovery", "perflogs", "msocache", "$winreagent",
    "$windows.~bt", "$windows.~ws", "windows.old", "appdata", "node_modules", ".git", ".svn", ".hg", "target", "__pycache__", ".venv", "venv", "env", ".cache", ".gradle",
    ".m2", ".nuget", ".vs", ".idea", ".vscode", "site-packages", "dist-packages", "bower_components", "$sysreset", "onedrivetemp", "config.msi", "steamapps",
    "epic games", "riot games", "xboxgames", "windowsapps", "wpsystem", "msys64", "cygwin64", "anaconda3", "miniconda3",
];
/// Skipped only right at a drive's root (driver files, not documents).
const SKIP_ROOT: &[&str] = &["intel", "amd", "nvidia", "drivers", "swsetup", "dell", "hp"];
/// PDFs bigger than this (MB) are skipped: reading one whole takes a lot of memory.
const MAX_PDF_MB: u64 = 40;

/// What the indexer is doing (for Settings).
#[derive(Clone, Default)]
pub struct Status {
    pub running: bool,
    /// Files read in this pass, and where it is now.
    pub read: u64,
    pub at: String,
    /// Files in the index (with text), and when the last pass finished.
    pub files: u64,
    pub last: Option<i64>,
    pub error: String,
}

static STATUS: Mutex<Option<Status>> = Mutex::new(None);
static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
static STOP: AtomicBool = AtomicBool::new(false);
/// Bumped to stop the pass under way (drives changed, Stop pressed).
static PASS: AtomicU64 = AtomicU64::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);
/// Settings → Delete: done by the indexer itself (never on the UI thread).
static CLEAR: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Reading a file that made a reader panic: that's expected now and then
    /// (broken PDFs) and isn't a crash worth recording.
    static IN_READER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Is this thread inside a file reader right now? (the panic log skips those)
pub fn in_reader() -> bool {
    IN_READER.with(|r| r.get())
}

pub fn status() -> Status {
    if let Some(s) = STATUS.lock().ok().and_then(|s| s.clone()) {
        return s;
    }
    // First look this run: what the index on disk holds from before.
    let mut s = Status::default();
    if let Some((n, last)) = counts() {
        s.files = n;
        s.last = last;
    }
    if let Ok(mut g) = STATUS.lock() {
        g.get_or_insert(s.clone());
    }
    s
}

fn set_status(f: impl FnOnce(&mut Status)) {
    if let Ok(mut s) = STATUS.lock() {
        let st = s.get_or_insert_with(Status::default);
        f(st);
    }
    crate::settings::refresh();
}

fn db_path() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|_| config::dir());
    let dir = base.join("Phemio");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("contentindex.db")
}

/// A copy of the index, refreshed after each full look when it changed: the
/// index survives a damaged file (or a lost one) without reading every
/// document again. (Updating Phemio never touches either.)
fn backup_path() -> PathBuf {
    db_path().with_file_name("contentindex.backup.db")
}

fn mtime(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Copy the index to the backup when it has changed since the last copy (at
/// most every 10 minutes).
fn backup() {
    let (db, bak) = (db_path(), backup_path());
    if !db.exists() {
        return;
    }
    let wal = db.with_file_name("contentindex.db-wal");
    let changed = [mtime(&db), mtime(&wal)].into_iter().flatten().max();
    let last = mtime(&bak);
    if let (Some(c), Some(l)) = (changed, last) {
        let recent = l.elapsed().map(|e| e < Duration::from_secs(600)).unwrap_or(true);
        if c <= l || recent {
            return;
        }
    }
    let tmp = db.with_file_name("contentindex.backup.tmp");
    let _ = std::fs::remove_file(&tmp);
    let r = open(false).map_err(|e| e.to_string()).and_then(|c| {
        let q = format!("VACUUM INTO '{}'", tmp.to_string_lossy().replace('\'', "''"));
        c.execute_batch(&q).map_err(|e| e.to_string())
    });
    match r {
        Ok(()) => {
            if std::fs::rename(&tmp, &bak).is_err() {
                let _ = std::fs::remove_file(&bak);
                let _ = std::fs::rename(&tmp, &bak);
            }
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            crate::util::log(&format!("content index: couldn't back it up: {e}"));
        }
    }
}

/// At start: the index gone or damaged, but a backup there? Back from it.
fn restore() {
    let (db, bak) = (db_path(), backup_path());
    if !bak.exists() {
        return;
    }
    let healthy = db.exists()
        && Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .and_then(|c| c.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0)))
            .map(|v| v == "ok")
            .unwrap_or(false);
    if healthy {
        return;
    }
    if db.exists() {
        let _ = std::fs::rename(&db, db.with_file_name("contentindex.damaged.db"));
    }
    for ext in ["contentindex.db-wal", "contentindex.db-shm"] {
        let _ = std::fs::remove_file(db.with_file_name(ext));
    }
    match std::fs::copy(&bak, &db) {
        Ok(_) => crate::util::log("content index: brought back from its backup (only files changed since are read again)"),
        Err(e) => crate::util::log(&format!("content index: couldn't bring back the backup: {e}")),
    }
}

/// The drives chosen (letters).
pub fn drives() -> Vec<char> {
    config::cur().s("cs_drives").chars().filter(|c| c.is_ascii_alphabetic()).map(|c| c.to_ascii_uppercase()).collect()
}

/// Use Phemio's index for Content searches (a drive is chosen)?
pub fn enabled() -> bool {
    !drives().is_empty()
}

pub fn set_drive(d: char, on: bool) {
    let mut v: Vec<char> = drives().into_iter().filter(|x| *x != d).collect();
    if on {
        v.push(d);
    }
    v.sort();
    let s: String = v.iter().collect();
    config::update(|c| c.set_s("cs_drives", &s));
    // Start over with the new set (a removed drive's files leave the index).
    update_now();
}

/// The PC's own (fixed) drives: (letter, name, size in GB, free GB).
pub fn fixed_drives() -> Vec<(char, String, f64, f64)> {
    // Asked at most every half minute (Settings draws often; a sleeping
    // disk can take a moment to answer).
    thread_local!(static CACHE: std::cell::RefCell<Option<(Instant, Vec<(char, String, f64, f64)>)>> = const { std::cell::RefCell::new(None) });
    if let Some(v) = CACHE.with(|c| c.borrow().as_ref().filter(|(t, _)| t.elapsed() < Duration::from_secs(30)).map(|(_, v)| v.clone())) {
        return v;
    }
    let v = read_drives();
    CACHE.with(|c| *c.borrow_mut() = Some((Instant::now(), v.clone())));
    v
}

fn read_drives() -> Vec<(char, String, f64, f64)> {
    use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
    let mut out = Vec::new();
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let d = (b'A' + i as u8) as char;
        let root = crate::util::wide(&format!("{d}:\\"));
        unsafe {
            // DRIVE_FIXED = 3
            if GetDriveTypeW(windows::core::PCWSTR(root.as_ptr())) != 3 {
                continue;
            }
            let mut name = [0u16; 128];
            let _ = GetVolumeInformationW(windows::core::PCWSTR(root.as_ptr()), Some(&mut name), None, None, None, None);
            let (mut free, mut total) = (0u64, 0u64);
            let _ = GetDiskFreeSpaceExW(windows::core::PCWSTR(root.as_ptr()), None, Some(&mut total), Some(&mut free));
            let gb = |b: u64| b as f64 / 1e9;
            out.push((d, crate::util::from_wide(&name), gb(total), gb(free)));
        }
    }
    out
}

/// Start the background indexer (once, at start-up).
pub fn start() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    // A big stack: some PDFs nest deeply.
    let _ = std::thread::Builder::new().name("content-index".into()).stack_size(64 << 20).spawn(run);
}

/// Look for new and changed files now (and start over if a pass is running).
pub fn update_now() {
    PASS.fetch_add(1, Ordering::SeqCst);
    let (m, cv) = &WAKE;
    if let Ok(mut w) = m.lock() {
        *w = true;
        cv.notify_all();
    }
}

/// Stop the pass under way (Settings → Stop); the next one is in half an hour.
pub fn stop_pass() {
    PASS.fetch_add(1, Ordering::SeqCst);
}

/// Phemio is closing.
pub fn shutdown() {
    STOP.store(true, Ordering::SeqCst);
    update_now();
}

/// Forget everything in the index (the indexer does it, right away).
pub fn clear() {
    CLEAR.store(true, Ordering::SeqCst);
    set_status(|s| {
        s.files = 0;
        s.last = None;
        s.read = 0;
    });
    update_now();
}

fn do_clear() -> Result<(), String> {
    let db = open(false).map_err(|e| e.to_string())?;
    db.execute_batch("BEGIN; DELETE FROM files; DELETE FROM texts; DELETE FROM meta WHERE k <> 'schema'; INSERT INTO ft(ft) VALUES ('delete-all'); COMMIT;").map_err(|e| e.to_string())?;
    let _ = db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;");
    let _ = std::fs::remove_file(backup_path());
    crate::util::log("content index: emptied");
    Ok(())
}

fn open(read_only: bool) -> rusqlite::Result<Connection> {
    let flags = if read_only { OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX } else { OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX };
    let db = Connection::open_with_flags(db_path(), flags)?;
    db.busy_timeout(Duration::from_millis(3000))?;
    if !read_only {
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS files(id INTEGER PRIMARY KEY AUTOINCREMENT, path TEXT NOT NULL UNIQUE, mtime INTEGER, size INTEGER, ok INTEGER);
             CREATE TABLE IF NOT EXISTS texts(id INTEGER PRIMARY KEY, text BLOB);
             CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT);
             CREATE VIRTUAL TABLE IF NOT EXISTS ft USING fts5(body, content='', contentless_delete=1, tokenize='unicode61 remove_diacritics 0');",
        )?;
    }
    Ok(db)
}

/// (files with text, last pass finished) from the index on disk.
fn counts() -> Option<(u64, Option<i64>)> {
    if !db_path().exists() {
        return None;
    }
    let db = open(true).ok()?;
    let n: i64 = db.query_row("SELECT COUNT(*) FROM files WHERE ok = 1", [], |r| r.get(0)).ok()?;
    let last: Option<String> = db.query_row("SELECT v FROM meta WHERE k = 'last'", [], |r| r.get(0)).optional().ok().flatten();
    Some((n as u64, last.and_then(|v| v.parse().ok())))
}

// ---------------------------------------------------------------------------
// Searching
// ---------------------------------------------------------------------------

/// Chinese / Japanese characters: each one is a word of its own in the index
/// (they're written without spaces).
fn cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FFFF)
}

/// Text as it goes into the index: as it is (accents count; the index
/// ignores case), CJK characters spaced out.
fn index_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for c in text.chars() {
        if cjk(c) {
            out.push(' ');
            out.push(c);
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

/// The index from before 0.33.6 ignored accents: made again from the texts
/// it keeps (no file needs reading again).
const SCHEMA: &str = "2";

fn migrate(db: &Connection) -> Result<(), String> {
    let v: Option<String> = db.query_row("SELECT v FROM meta WHERE k = 'schema'", [], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
    if v.as_deref() == Some(SCHEMA) {
        return Ok(());
    }
    crate::util::log("content index: making the index accent-exact (from the stored texts)");
    db.execute_batch("BEGIN; DROP TABLE IF EXISTS ft; CREATE VIRTUAL TABLE ft USING fts5(body, content='', contentless_delete=1, tokenize='unicode61 remove_diacritics 0');").map_err(|e| e.to_string())?;
    {
        let mut st = db.prepare("SELECT id, text FROM texts").map_err(|e| e.to_string())?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))).map_err(|e| e.to_string())?;
        let mut ins = db.prepare("INSERT INTO ft(rowid, body) VALUES (?1, ?2)").map_err(|e| e.to_string())?;
        for (id, b) in rows.flatten() {
            let _ = ins.execute(params![id, index_text(&inflate(&b))]);
        }
    }
    db.execute_batch(&format!("INSERT OR REPLACE INTO meta(k, v) VALUES ('schema', '{SCHEMA}'); COMMIT;")).map_err(|e| e.to_string())?;
    Ok(())
}

/// What you typed as an index query: every word (as the start of a word),
/// a run of CJK characters as those characters in a row.
fn index_query(words: &[String]) -> String {
    // Each term exactly: a word, or a phrase (its words in a row).
    let mut parts = Vec::new();
    for w in words {
        let toks = term_tokens(w);
        if !toks.is_empty() {
            parts.push(format!("\"{}\"", toks.join(" ")));
        }
    }
    parts.join(" AND ")
}

/// Files whose text has every word of `q`, best first, each with the
/// passage around them: (path, passage). Only files that still exist.
pub fn search<F: Fn() -> bool + Send + Clone + 'static>(q: &str, max: usize, alive: F) -> Result<Vec<(String, String)>, String> {
    let words = words_of(q);
    if words.is_empty() {
        return Ok(vec![]);
    }
    let fq = index_query(&words);
    if fq.is_empty() {
        return Ok(vec![]);
    }
    let db = open(true).map_err(|e| e.to_string())?;
    // A newer search (or closing it) stops this one even mid-query.
    let a2 = alive.clone();
    db.progress_handler(2000, Some(move || !a2()));
    // The best matches first, from the index alone; only those rows' texts
    // are read.
    let mut st = db
        .prepare(
            "WITH hits AS (SELECT rowid, rank FROM ft WHERE ft MATCH ?1 ORDER BY rank LIMIT ?2) \
             SELECT f.path, t.text FROM hits JOIN files f ON f.id = hits.rowid JOIN texts t ON t.id = hits.rowid ORDER BY hits.rank",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map(params![fq, (max * 3) as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        if !alive() {
            break;
        }
        let Ok((path, blob)) = row else { continue };
        if !Path::new(&path).exists() {
            continue;
        }
        let text = inflate(&blob);
        if let Some(p) = passage(&text, &words) {
            out.push((path, p));
            if out.len() >= max {
                break;
            }
        }
    }
    Ok(out)
}

fn deflate(s: &str) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    let _ = e.write_all(s.as_bytes());
    e.finish().unwrap_or_default()
}

fn inflate(b: &[u8]) -> String {
    let mut out = String::new();
    let _ = flate2::read::DeflateDecoder::new(b).read_to_string(&mut out);
    out
}

// ---------------------------------------------------------------------------
// Indexing
// ---------------------------------------------------------------------------

fn run() {
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};
        // Low CPU and disk priority: never in the way of what you're doing.
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    restore();
    // Let Phemio finish starting first (dev: ISLET_INDEX_NOW starts at once).
    wait(Duration::from_secs(if std::env::var("ISLET_INDEX_NOW").is_ok() { 1 } else { 45 }));
    while !STOP.load(Ordering::SeqCst) {
        sync_watchers();
        if CLEAR.swap(false, Ordering::SeqCst) {
            if let Err(e) = do_clear() {
                crate::util::log(&format!("content index: couldn't empty it: {e}"));
                set_status(|s| s.error = e);
            }
        }
        if enabled() {
            let pass = PASS.load(Ordering::SeqCst);
            let r = std::panic::catch_unwind(|| index_pass(pass)).unwrap_or_else(|_| Err("stopped by a problem (see crash.log)".into()));
            if let Err(e) = r {
                crate::util::log(&format!("content index: {e}"));
                set_status(|s| {
                    s.running = false;
                    s.error = e;
                });
            }
        } else {
            set_status(|s| s.running = false);
        }
        backup();
        sync_watchers();
        // Until the next full look: read files as they change.
        let until = Instant::now() + EVERY;
        while Instant::now() < until && !STOP.load(Ordering::SeqCst) {
            if wait(Duration::from_secs(2).min(until.saturating_duration_since(Instant::now()))) {
                // Asked for a full look now (Update now, drives changed, Delete).
                break;
            }
            if OVERFLOW.swap(false, Ordering::SeqCst) {
                break;
            }
            if enabled() {
                let pass = PASS.load(Ordering::SeqCst);
                let alive = move || PASS.load(Ordering::SeqCst) == pass && !STOP.load(Ordering::SeqCst);
                let r = std::panic::catch_unwind(|| take_changes(&alive)).unwrap_or_else(|_| Err("stopped by a problem (see crash.log)".into()));
                if let Err(e) = r {
                    crate::util::log(&format!("content index: {e}"));
                }
            }
        }
    }
    sync_watchers();
}

/// Sleep until `d` passes or someone asks for a pass now.
fn wait(d: Duration) -> bool {
    let (m, cv) = &WAKE;
    let Ok(mut w) = m.lock() else { return false };
    let until = Instant::now() + d;
    while !*w && !STOP.load(Ordering::SeqCst) {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match cv.wait_timeout(w, left) {
            Ok((g, _)) => w = g,
            Err(_) => return false,
        }
    }
    let woken = *w;
    *w = false;
    woken
}

struct Known {
    id: i64,
    mtime: i64,
    size: i64,
}

fn index_pass(pass: u64) -> Result<(), String> {
    let alive = || PASS.load(Ordering::SeqCst) == pass && !STOP.load(Ordering::SeqCst);
    let mut db = open(false).map_err(|e| e.to_string())?;
    migrate(&db)?;
    let drives = drives();
    set_status(|s| {
        s.running = true;
        s.read = 0;
        s.at.clear();
        s.error.clear();
    });
    crate::util::log(&format!("content index: looking at {}", drives.iter().map(|d| format!("{d}:")).collect::<Vec<_>>().join(" ")));
    // Everything known, by path; what's still there is crossed off as the
    // drives are walked, and what's left at the end is gone.
    let mut known: HashMap<String, Known> = HashMap::new();
    {
        let mut st = db.prepare("SELECT id, path, mtime, size FROM files").map_err(|e| e.to_string())?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?))).map_err(|e| e.to_string())?;
        for r in rows.flatten() {
            known.insert(r.1, Known { id: r.0, mtime: r.2, size: r.3 });
        }
    }
    let own = config::dir();
    let own_local = db_path().parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let mut tx = db.transaction().map_err(|e| e.to_string())?;
    let mut since_commit = Instant::now();
    let mut pending = 0u32;
    let mut read = 0u64;
    let mut last_status = Instant::now();
    // Drives that can't be read now (unplugged, locked): their files stay.
    let mut away: Vec<char> = Vec::new();
    for d in &drives {
        let root = PathBuf::from(format!("{d}:\\"));
        if std::fs::read_dir(&root).is_err() {
            away.push(*d);
            continue;
        }
        let mut stack: Vec<PathBuf> = vec![root];
        while let Some(dir) = stack.pop() {
            if !alive() {
                break;
            }
            if dir == own || dir == own_local {
                continue;
            }
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let Ok(md) = e.metadata() else { continue };
                use std::os::windows::fs::MetadataExt;
                let attr = md.file_attributes();
                // Links and junctions (not OneDrive's own files, which are
                // reparse points too), offline and cloud-only files.
                if e.file_type().map(|t| t.is_symlink()).unwrap_or(true) || attr & (0x1000 | 0x40000 | 0x400000) != 0 {
                    continue;
                }
                let name = e.file_name().to_string_lossy().into_owned();
                if md.is_dir() {
                    let low = name.to_lowercase();
                    // Hidden or system folders, and the usual places that hold no documents.
                    let at_root = dir.parent().is_none();
                    if attr & (0x2 | 0x4) != 0 || SKIP_DIRS.contains(&low.as_str()) || (at_root && SKIP_ROOT.contains(&low.as_str())) || low.starts_with('.') || low.starts_with('$') {
                        continue;
                    }
                    stack.push(e.path());
                    continue;
                }
                let ext = Path::new(&name).extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
                if !wanted(&ext) || md.len() > MAX_MB << 20 || (ext == PDF && md.len() > MAX_PDF_MB << 20) || md.len() == 0 || name.starts_with("~$") {
                    continue;
                }
                let path = e.path().to_string_lossy().into_owned();
                let mtime = md.last_write_time() as i64;
                let size = md.len() as i64;
                let prev = known.remove(&path);
                if let Some(k) = &prev {
                    if k.mtime == mtime && k.size == size {
                        continue;
                    }
                }
                if !alive() {
                    break;
                }
                // New or changed: read it.
                let text = read_file(&path, &ext, &alive);
                if !alive() {
                    break;
                }
                // Couldn't even be opened (in use): tried again next time.
                let mtime = if text.is_none() && std::fs::File::open(&path).is_err() { 0 } else { mtime };
                if !store(&tx, prev.as_ref(), &path, mtime, size, text) {
                    continue;
                }
                read += 1;
                pending += 1;
                if pending >= 40 || since_commit.elapsed() > Duration::from_secs(4) {
                    tx.commit().map_err(|e| e.to_string())?;
                    tx = db.transaction().map_err(|e| e.to_string())?;
                    pending = 0;
                    since_commit = Instant::now();
                }
                if last_status.elapsed() > Duration::from_millis(700) {
                    last_status = Instant::now();
                    let at = path.clone();
                    set_status(|s| {
                        s.read = read;
                        s.at = at;
                    });
                }
            }
        }
    }
    let finished = alive();
    if finished {
        // What wasn't found any more (deleted, moved, or on a drive no longer
        // chosen) leaves the index — but not what's on a drive that's away.
        for (path, k) in known.drain() {
            if away.iter().any(|d| path.starts_with(&format!("{d}:"))) {
                continue;
            }
            let _ = tx.execute("DELETE FROM files WHERE id = ?1", params![k.id]);
            let _ = tx.execute("DELETE FROM ft WHERE rowid = ?1", params![k.id]);
            let _ = tx.execute("DELETE FROM texts WHERE id = ?1", params![k.id]);
        }
        let now = crate::util::unix_now();
        let _ = tx.execute("INSERT OR REPLACE INTO meta(k, v) VALUES ('last', ?1)", params![now.to_string()]);
    }
    tx.commit().map_err(|e| e.to_string())?;
    if finished {
        // Tidy the index a little (not a full rewrite every time).
        let _ = db.execute_batch("INSERT INTO ft(ft, rank) VALUES ('merge', 500);");
    }
    let (n, last) = counts().unwrap_or((0, None));
    crate::util::log(&format!("content index: {} — read {read} files; {n} in the index", if finished { "done" } else { "stopped" }));
    set_status(|s| {
        s.running = false;
        s.read = read;
        s.at.clear();
        s.files = n;
        s.last = last;
    });
    Ok(())
}

/// Put a file's text in the index (new, or replacing what was there).
fn store(tx: &rusqlite::Transaction, prev: Option<&Known>, path: &str, mtime: i64, size: i64, text: Option<String>) -> bool {
    let ok = text.as_ref().map(|t| !t.trim().is_empty()).unwrap_or(false);
    let id = match prev {
        Some(k) => {
            let _ = tx.prepare_cached("UPDATE files SET mtime = ?1, size = ?2, ok = ?3 WHERE id = ?4").and_then(|mut st| st.execute(params![mtime, size, ok as i32, k.id]));
            let _ = tx.prepare_cached("DELETE FROM ft WHERE rowid = ?1").and_then(|mut st| st.execute(params![k.id]));
            let _ = tx.prepare_cached("DELETE FROM texts WHERE id = ?1").and_then(|mut st| st.execute(params![k.id]));
            k.id
        }
        None => {
            if tx.prepare_cached("INSERT INTO files(path, mtime, size, ok) VALUES (?1, ?2, ?3, ?4)").and_then(|mut st| st.execute(params![path, mtime, size, ok as i32])).is_err() {
                return false;
            }
            tx.last_insert_rowid()
        }
    };
    if let Some(t) = text.filter(|t| !t.trim().is_empty()) {
        let _ = tx.prepare_cached("INSERT INTO ft(rowid, body) VALUES (?1, ?2)").and_then(|mut st| st.execute(params![id, index_text(&t)]));
        let _ = tx.prepare_cached("INSERT OR REPLACE INTO texts(id, text) VALUES (?1, ?2)").and_then(|mut st| st.execute(params![id, deflate(&t)]));
    }
    true
}

// ---------------------------------------------------------------------------
// Watching: Windows says when files change on the chosen drives; those are
// read a few seconds later (once they've stopped changing). The half-hourly
// look stays, in case Windows had too much to tell at once.
// ---------------------------------------------------------------------------

/// Changed paths and when they last changed.
static CHANGED: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
/// Windows had more changes than it could pass on: look at everything soon.
/// When each file was last read again because it changed: a file that keeps
/// changing (a log, an app's settings) is read at most once a minute.
static DONE: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
const COOL: Duration = Duration::from_secs(60);
/// Files updated since the last log line about it (one line every 10 min at most).
static LOGGED: Mutex<Option<(Instant, usize)>> = Mutex::new(None);
static OVERFLOW: AtomicBool = AtomicBool::new(false);
/// Watched drives: (letter, directory handle).
static WATCHERS: Mutex<Vec<(char, isize)>> = Mutex::new(Vec::new());

/// Worth looking at? (Skipped folders, wanted kinds; a name without an
/// extension may be a folder.)
fn watch_wanted(path: &str) -> bool {
    let p = Path::new(path);
    for c in p.components().rev().skip(1) {
        let n = c.as_os_str().to_string_lossy().to_lowercase();
        if SKIP_DIRS.contains(&n.as_str()) || n.starts_with('.') || (n.starts_with('$') && !n.ends_with(':')) {
            return false;
        }
    }
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name.starts_with("~$") || name.starts_with('.') {
        return false;
    }
    match p.extension().map(|x| x.to_string_lossy().to_lowercase()) {
        Some(ext) => wanted(&ext) || !ext.chars().all(|c| c.is_ascii_alphanumeric()) || ext.len() > 5,
        None => true,
    }
}

/// Watch the chosen drives (and stop watching the others).
fn sync_watchers() {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Storage::FileSystem::*;
    use windows::Win32::System::IO::CancelIoEx;
    let want: Vec<char> = if enabled() && !STOP.load(Ordering::SeqCst) { drives() } else { Vec::new() };
    let Ok(mut w) = WATCHERS.lock() else { return };
    w.retain(|(d, h)| {
        if want.contains(d) {
            return true;
        }
        unsafe {
            let _ = CancelIoEx(HANDLE(*h as *mut _), None);
            let _ = CloseHandle(HANDLE(*h as *mut _));
        }
        false
    });
    for d in want {
        if w.iter().any(|(x, _)| *x == d) {
            continue;
        }
        let root = format!("{d}:\\");
        let wr = crate::util::wide(&root);
        let h = unsafe {
            CreateFileW(
                windows::core::PCWSTR(wr.as_ptr()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        };
        let Ok(h) = h else {
            crate::util::log(&format!("content index: can't watch {d}: (it's looked at every half hour instead)"));
            continue;
        };
        let raw = h.0 as isize;
        w.push((d, raw));
        let _ = std::thread::Builder::new().name(format!("content-watch-{d}")).spawn(move || watch(root, raw));
    }
}

fn watch(root: String, raw: isize) {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::*;
    let h = HANDLE(raw as *mut _);
    // DWORD-aligned buffer.
    let mut buf = vec![0u32; 16 * 1024];
    let filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE;
    loop {
        let mut got = 0u32;
        let r = unsafe { ReadDirectoryChangesW(h, buf.as_mut_ptr() as *mut _, (buf.len() * 4) as u32, true, filter, Some(&mut got), None, None) };
        if r.is_err() || STOP.load(Ordering::SeqCst) {
            // Stopped watching (drive no longer chosen, closing, drive gone).
            return;
        }
        if got == 0 {
            OVERFLOW.store(true, Ordering::SeqCst);
            continue;
        }
        let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, got as usize) };
        let now = Instant::now();
        let mut off = 0usize;
        let mut paths = Vec::new();
        while off + 12 <= bytes.len() {
            let next = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
            let len = u32::from_le_bytes(bytes[off + 8..off + 12].try_into().unwrap()) as usize;
            let name_at = off + 12;
            if name_at + len <= bytes.len() {
                let w: Vec<u16> = bytes[name_at..name_at + len].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                let path = format!("{root}{}", String::from_utf16_lossy(&w));
                if watch_wanted(&path) {
                    paths.push(path);
                }
            }
            if next == 0 {
                break;
            }
            off += next;
        }
        if !paths.is_empty() {
            if let Ok(mut c) = CHANGED.lock() {
                let m = c.get_or_insert_with(HashMap::new);
                for p in paths {
                    m.insert(p, now);
                }
            }
        }
    }
}

/// Read what changed (and has been still for a few seconds).
fn take_changes(alive: &dyn Fn() -> bool) -> Result<usize, String> {
    let ready: Vec<String> = {
        let Ok(mut c) = CHANGED.lock() else { return Ok(0) };
        let Some(m) = c.as_mut() else { return Ok(0) };
        let mut done = DONE.lock().unwrap_or_else(|e| e.into_inner());
        let done = done.get_or_insert_with(HashMap::new);
        done.retain(|_, t| t.elapsed() < COOL);
        let ready: Vec<String> = m.iter().filter(|(p, t)| t.elapsed() > Duration::from_secs(3) && !done.contains_key(*p)).map(|(p, _)| p.clone()).collect();
        for p in &ready {
            m.remove(p);
            done.insert(p.clone(), Instant::now());
        }
        ready
    };
    if ready.is_empty() {
        return Ok(0);
    }
    let mut db = open(false).map_err(|e| e.to_string())?;
    migrate(&db)?;
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let mut n = 0;
    // A folder that appeared (made, moved in, renamed): its files too.
    let mut todo: Vec<PathBuf> = ready.iter().map(PathBuf::from).collect();
    let mut seen = 0usize;
    while let Some(p) = todo.pop() {
        if !alive() {
            break;
        }
        seen += 1;
        if seen > 20_000 {
            // A huge folder moved in: the full look handles it.
            OVERFLOW.store(true, Ordering::SeqCst);
            break;
        }
        let path = p.to_string_lossy().into_owned();
        let Ok(md) = std::fs::symlink_metadata(&p) else {
            // Gone (deleted, or moved / renamed away): it and anything under it.
            let like = format!("{}\\%", path.replace('%', "\\%").replace('_', "\\_"));
            let ids: Vec<i64> = tx
                .prepare_cached("SELECT id FROM files WHERE path = ?1 OR path LIKE ?2 ESCAPE '\\'")
                .and_then(|mut st| st.query_map(params![path, like], |r| r.get::<_, i64>(0)).map(|r| r.flatten().collect()))
                .unwrap_or_default();
            for id in ids {
                let _ = tx.execute("DELETE FROM files WHERE id = ?1", params![id]);
                let _ = tx.execute("DELETE FROM ft WHERE rowid = ?1", params![id]);
                let _ = tx.execute("DELETE FROM texts WHERE id = ?1", params![id]);
                n += 1;
            }
            continue;
        };
        use std::os::windows::fs::MetadataExt;
        let attr = md.file_attributes();
        if md.file_type().is_symlink() || attr & (0x1000 | 0x40000 | 0x400000) != 0 {
            continue;
        }
        if md.is_dir() {
            if attr & (0x2 | 0x4) != 0 {
                continue;
            }
            if let Ok(rd) = std::fs::read_dir(&p) {
                for e in rd.flatten() {
                    let ep = e.path();
                    if watch_wanted(&ep.to_string_lossy()) {
                        todo.push(ep);
                    }
                }
            }
            continue;
        }
        let name = p.file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !wanted(&ext) || md.len() > MAX_MB << 20 || (ext == PDF && md.len() > MAX_PDF_MB << 20) || md.len() == 0 || name.starts_with("~$") {
            continue;
        }
        let mtime = md.last_write_time() as i64;
        let size = md.len() as i64;
        let prev: Option<Known> = tx
            .prepare_cached("SELECT id, mtime, size FROM files WHERE path = ?1")
            .and_then(|mut st| st.query_row(params![path], |r| Ok(Known { id: r.get(0)?, mtime: r.get(1)?, size: r.get(2)? })).optional())
            .ok()
            .flatten();
        if prev.as_ref().map(|k| k.mtime == mtime && k.size == size).unwrap_or(false) {
            continue;
        }
        let text = read_file(&path, &ext, alive);
        let mtime = if text.is_none() && std::fs::File::open(&path).is_err() { 0 } else { mtime };
        if store(&tx, prev.as_ref(), &path, mtime, size, text) {
            n += 1;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    if n > 0 {
        let mut lg = LOGGED.lock().unwrap_or_else(|e| e.into_inner());
        let (at, k) = lg.get_or_insert((crate::util::ago(Duration::from_secs(3600)), 0));
        *k += n;
        if at.elapsed() >= Duration::from_secs(600) {
            crate::util::log(&format!("content index: {k} changed file(s) updated as they changed"));
            *at = Instant::now();
            *k = 0;
        }
        drop(lg);
        let (files, last) = counts().unwrap_or((0, None));
        set_status(|s| {
            s.files = files;
            if last.is_some() {
                s.last = last;
            }
        });
    }
    Ok(n)
}

fn wanted(ext: &str) -> bool {
    ZIP_KINDS.contains(&ext) || ext == PDF || FILTER_KINDS.contains(&ext) || TEXT_KINDS.contains(&ext)
}

/// A file's text (composed form, at most MAX_TEXT characters).
fn read_file(path: &str, ext: &str, alive: &dyn Fn() -> bool) -> Option<String> {
    IN_READER.with(|r| r.set(true));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if TEXT_KINDS.contains(&ext) {
            crate::contentsearch::read_text(path)
        } else if ZIP_KINDS.contains(&ext) {
            zip_text(path, ext)
        } else if ext == PDF {
            pdf_text(path)
        } else {
            unsafe { crate::contentsearch::filter_text(path, &[], &|| !alive()) }
        }
    }));
    IN_READER.with(|r| r.set(false));
    let t = r.ok().flatten()?;
    let t: String = if t.chars().count() > MAX_TEXT { t.chars().take(MAX_TEXT).collect() } else { t };
    Some(nfc(&t))
}

/// A PDF's text. Letters are put in reading order by where they sit on
/// the page (PDFs often draw a line's letters out of order, and accents as
/// separate marks — Vietnamese especially), lines top to bottom.
fn pdf_text(path: &str) -> Option<String> {
    let b = std::fs::read(path).ok()?;
    let mut doc = pdf_extract::Document::load_mem(&b).ok()?;
    if doc.is_encrypted() {
        // Most "protected" PDFs only have an owner password.
        doc.decrypt("").ok()?;
    }
    let mut out = PdfText { h: 0.0, glyphs: Vec::new(), text: String::new() };
    pdf_extract::output_doc(&doc, &mut out).ok()?;
    Some(out.text)
}

struct Glyph {
    x: f64,
    y: f64,
    size: f64,
    end: f64,
    ch: String,
}

struct PdfText {
    h: f64,
    glyphs: Vec<Glyph>,
    text: String,
}

fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

impl pdf_extract::OutputDev for PdfText {
    fn begin_page(&mut self, _n: u32, mb: &pdf_extract::MediaBox, _art: Option<(f64, f64, f64, f64)>) -> Result<(), pdf_extract::OutputError> {
        self.h = mb.ury - mb.lly;
        self.glyphs.clear();
        Ok(())
    }
    fn end_page(&mut self) -> Result<(), pdf_extract::OutputError> {
        let mut g = std::mem::take(&mut self.glyphs);
        g.retain(|x| x.size > 0.0 && x.size.is_finite() && x.x.is_finite() && x.y.is_finite());
        g.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal));
        // Lines: letters whose baselines are close.
        let mut lines: Vec<Vec<Glyph>> = Vec::new();
        for x in g {
            match lines.last_mut() {
                Some(l) if (x.y - l[0].y).abs() < l[0].size.max(x.size) * 0.45 => l.push(x),
                _ => lines.push(vec![x]),
            }
        }
        for mut l in lines {
            // Marks go with the letter under them; letters left to right.
            let (marks, mut bases): (Vec<Glyph>, Vec<Glyph>) = l.drain(..).partition(|x| x.ch.chars().all(is_mark));
            bases.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
            let mut attached: Vec<String> = vec![String::new(); bases.len()];
            for m in marks {
                let k = bases.iter().rposition(|b| b.x <= m.x + m.size * 0.15).unwrap_or(0);
                if let Some(a) = attached.get_mut(k) {
                    a.push_str(&m.ch);
                }
            }
            let mut line = String::new();
            let mut prev_end: Option<f64> = None;
            for (b, marks) in bases.iter().zip(attached) {
                if let Some(e) = prev_end {
                    if b.x > e + b.size * 0.15 {
                        line.push(' ');
                    }
                }
                line.push_str(&b.ch);
                line.push_str(&marks);
                prev_end = Some(prev_end.map(|p: f64| p.max(b.end)).unwrap_or(b.end));
            }
            self.text.push_str(line.trim_end());
            self.text.push('\n');
        }
        self.text.push('\n');
        Ok(())
    }
    fn output_character(&mut self, trm: &pdf_extract::Transform, width: f64, _spacing: f64, font_size: f64, ch: &str) -> Result<(), pdf_extract::OutputError> {
        let ch: String = ch.chars().filter(|c| !c.is_control()).collect();
        if ch.is_empty() || self.text.len() > MAX_TEXT * 4 {
            return Ok(());
        }
        // The text's size on the page (any rotation).
        let size = font_size * (trm.m11 * trm.m22 - trm.m12 * trm.m21).abs().sqrt();
        let x = trm.m31;
        let y = self.h - trm.m32;
        self.glyphs.push(Glyph { x, y, size, end: x + width * size, ch });
        Ok(())
    }
    fn begin_word(&mut self) -> Result<(), pdf_extract::OutputError> {
        Ok(())
    }
    fn end_word(&mut self) -> Result<(), pdf_extract::OutputError> {
        Ok(())
    }
    fn end_line(&mut self) -> Result<(), pdf_extract::OutputError> {
        Ok(())
    }
}

/// Office Open XML, OpenDocument and EPUB files are zip archives of XML.
fn zip_text(path: &str, ext: &str) -> Option<String> {
    let f = std::fs::File::open(path).ok()?;
    let mut z = zip::ZipArchive::new(f).ok()?;
    let mut names: Vec<String> = z.file_names().map(|s| s.to_string()).collect();
    let pick = |n: &str| -> bool {
        let n = n.to_lowercase();
        match ext {
            "docx" | "docm" | "dotx" | "dotm" => n == "word/document.xml" || n.starts_with("word/footnotes") || n.starts_with("word/endnotes") || (n.starts_with("word/header") && n.ends_with(".xml")) || (n.starts_with("word/footer") && n.ends_with(".xml")),
            "xlsx" | "xlsm" | "xltx" => n == "xl/sharedstrings.xml" || (n.starts_with("xl/worksheets/sheet") && n.ends_with(".xml")),
            "pptx" | "pptm" | "potx" | "ppsx" => (n.starts_with("ppt/slides/slide") || n.starts_with("ppt/notesslides/notesslide")) && n.ends_with(".xml"),
            "epub" => n.ends_with(".xhtml") || n.ends_with(".html") || n.ends_with(".htm"),
            _ => n == "content.xml",
        }
    };
    names.retain(|n| pick(n));
    // Slides and sheets in their own order (slide2 before slide10).
    let num = |s: &str| -> u64 { s.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0) };
    names.sort_by(|a, b| (a.contains("notes"), num(a), a.as_str()).cmp(&(b.contains("notes"), num(b), b.as_str())));
    let mut out = String::new();
    for n in names {
        let Ok(e) = z.by_name(&n) else { continue };
        // At most 16 MB of XML per part (a zip can claim any size).
        let mut xml = String::new();
        if e.take(16 << 20).read_to_string(&mut xml).is_err() && xml.is_empty() {
            continue;
        }
        xml_text(&xml, &mut out);
        out.push('\n');
        if out.len() > MAX_TEXT * 4 {
            break;
        }
    }
    Some(out)
}

/// The text of an XML document: text between tags, a line break after
/// paragraphs, rows and slides' text boxes, a tab for tabs and cells.
fn xml_text(xml: &str, out: &mut String) {
    let mut rest = xml;
    // Inside <w:instrText> (field codes) and <style>/<script>: not text.
    let mut skip = 0u32;
    while let Some(lt) = rest.find('<') {
        if skip == 0 {
            unescape_into(&rest[..lt], out);
        }
        let Some(gt) = rest[lt..].find('>') else { break };
        let tag = &rest[lt + 1..lt + gt];
        rest = &rest[lt + gt + 1..];
        if tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        let closing = tag.starts_with('/');
        let self_closing = tag.ends_with('/');
        let name = tag.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("").to_lowercase();
        if matches!(name.as_str(), "w:instrtext" | "style" | "script" | "w:deltext") {
            if closing {
                skip = skip.saturating_sub(1);
            } else if !self_closing {
                skip += 1;
            }
            continue;
        }
        match name.as_str() {
            "w:tab" | "text:tab" | "c" | "td" | "th" | "table:table-cell" => out.push('\t'),
            "w:br" | "w:cr" | "a:br" | "text:line-break" | "br" => out.push('\n'),
            "text:s" => out.push(' '),
            _ => {}
        }
        if closing {
            match name.as_str() {
                "w:p" | "a:p" | "text:p" | "text:h" | "si" | "row" | "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "table:table-row" | "title" => out.push('\n'),
                _ => {}
            }
        }
    }
    if skip == 0 {
        unescape_into(rest, out);
    }
}

fn unescape_into(s: &str, out: &mut String) {
    if !s.contains('&') {
        out.push_str(s);
        return;
    }
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(semi) = tail.find(';').filter(|&k| k < 12) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let ent = &tail[1..semi];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => out.push(c),
            None => out.push_str(&tail[..=semi]),
        }
        rest = &tail[semi + 1..];
    }
    out.push_str(rest);
}

/// Settings line: what the index is doing.
pub fn status_line() -> String {
    use crate::i18n::tr;
    let s = status();
    if s.running {
        let at = Path::new(&s.at).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        return format!("{} {} · {at}", tr("Reading files…"), fmt_n(s.read));
    }
    if !s.error.is_empty() {
        return format!("{} {}", tr("Couldn't update:"), s.error);
    }
    match s.last {
        None if s.files == 0 => tr("Not built yet: choose a drive").to_string(),
        None => format!("{} {}", fmt_n(s.files), tr("files")),
        Some(t) => {
            let mins = (crate::util::unix_now() - t).max(0) / 60;
            let ago = if mins < 1 {
                tr("just now").to_string()
            } else if mins < 60 {
                format!("{mins} {}", tr("min ago"))
            } else if mins < 48 * 60 {
                format!("{} {}", mins / 60, tr("h ago"))
            } else {
                format!("{} {}", mins / 1440, tr("days ago"))
            };
            let live = WATCHERS.lock().map(|w| !w.is_empty()).unwrap_or(false);
            let watching = if live { format!(" · {}", tr("new and changed files added as they're saved")) } else { String::new() };
            format!("{} {} · {} {ago}{watching}", fmt_n(s.files), tr("files"), tr("updated"))
        }
    }
}

fn fmt_n(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn xml() {
        let mut o = String::new();
        xml_text(r#"<w:document><w:body><w:p><w:r><w:t>Hợp đồng</w:t></w:r><w:r><w:t xml:space="preserve"> thuê nhà</w:t></w:r></w:p><w:p><w:r><w:instrText>PAGE</w:instrText><w:t>A&amp;B</w:t></w:r></w:p></w:body></w:document>"#, &mut o);
        assert_eq!(o, "Hợp đồng thuê nhà\nA&B\n");
    }
    #[test]
    fn query() {
        assert_eq!(index_query(&["vat".into(), "中文".into()]), "\"vat\" AND \"中 文\"");
    }
}
