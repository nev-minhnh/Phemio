//! Searching inside files: Windows' own search index (the one Explorer and
//! Start use — it already reads Word, PDF, text and code files), asked
//! through its standard ADO provider. Talked to by name over IDispatch, so no
//! extra libraries are needed.

use windows::core::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Variant::VARIANT;

/// Up to `max` files whose text (not their name) contains every word of `q`, best match
/// first: (name, folder). Err when the index can't be asked (service off).
pub fn search(q: &str, max: usize) -> std::result::Result<Vec<(String, String)>, String> {
    // Terms → '"word" AND "a phrase"' (Windows' index ignores accents; the
    // passage finder then keeps only exact matches).
    let words = words_of(q);
    if words.is_empty() {
        return Ok(vec![]);
    }
    let cond = words.iter().map(|w| format!("\"{w}\"")).collect::<Vec<_>>().join(" AND ");
    let sql = format!(
        "SELECT TOP {max} System.ItemNameDisplay, System.ItemFolderPathDisplay FROM SystemIndex \
         WHERE SCOPE='file:' AND CONTAINS(System.Search.Contents, '{cond}') ORDER BY System.Search.Rank DESC"
    );
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        run(&sql).map_err(|e| format!("{} ({:#010x})", e.message(), e.code().0))
    }
}

unsafe fn run(sql: &str) -> Result<Vec<(String, String)>> {
    let clsid = CLSIDFromProgID(w!("ADODB.Connection"))?;
    let conn: IDispatch = CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER)?;
    call(&conn, "Open", DISPATCH_METHOD, vec![VARIANT::from("Provider=Search.CollatorDSO;Extended Properties='Application=Windows';")])?;
    let rs = call(&conn, "Execute", DISPATCH_METHOD, vec![VARIANT::from(sql)])?;
    let rs = IDispatch::try_from(&rs)?;
    let mut out = Vec::new();
    for _ in 0..500 {
        let eof = call(&rs, "EOF", DISPATCH_PROPERTYGET, vec![])?;
        if bool::try_from(&eof).unwrap_or(true) {
            break;
        }
        let fields = IDispatch::try_from(&call(&rs, "Fields", DISPATCH_PROPERTYGET, vec![])?)?;
        let text = |i: i32| -> String {
            call(&fields, "Item", DISPATCH_PROPERTYGET | DISPATCH_METHOD, vec![VARIANT::from(i)])
                .ok()
                .and_then(|f| IDispatch::try_from(&f).ok())
                .and_then(|f| call(&f, "Value", DISPATCH_PROPERTYGET, vec![]).ok())
                .and_then(|v| BSTR::try_from(&v).ok())
                .map(|b| b.to_string())
                .unwrap_or_default()
        };
        let (name, dir) = (text(0), text(1));
        if !name.is_empty() {
            out.push((name, dir));
        }
        call(&rs, "MoveNext", DISPATCH_METHOD, vec![])?;
    }
    let _ = call(&rs, "Close", DISPATCH_METHOD, vec![]);
    let _ = call(&conn, "Close", DISPATCH_METHOD, vec![]);
    Ok(out)
}

/// obj.name(args…) by name (late bound).
unsafe fn call(obj: &IDispatch, name: &str, flags: DISPATCH_FLAGS, mut args: Vec<VARIANT>) -> Result<VARIANT> {
    let wname: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let names = [PCWSTR(wname.as_ptr())];
    let mut id = 0i32;
    obj.GetIDsOfNames(&GUID::zeroed(), names.as_ptr(), 1, 0x0400, &mut id)?;
    // IDispatch takes its arguments last first.
    args.reverse();
    let params = DISPPARAMS { rgvarg: if args.is_empty() { std::ptr::null_mut() } else { args.as_mut_ptr() }, rgdispidNamedArgs: std::ptr::null_mut(), cArgs: args.len() as u32, cNamedArgs: 0 };
    let mut result = VARIANT::default();
    obj.Invoke(id, &GUID::zeroed(), 0x0400, flags, &params, Some(&mut result), None, None)?;
    Ok(result)
}

// ---------------------------------------------------------------------------
// The passage around the words ("…the vat code is the best one…")
// ---------------------------------------------------------------------------

/// Kinds read straight as text (others go through Windows' document filters).
const TEXT_EXTS: &[&str] = &[
    "txt", "md", "log", "csv", "tsv", "json", "xml", "yaml", "yml", "ini", "cfg", "conf", "toml", "html", "htm", "css", "js", "ts", "tsx", "jsx", "py", "rs", "c", "h",
    "cpp", "hpp", "cs", "java", "kt", "go", "rb", "php", "sql", "sh", "bat", "ps1", "tex", "srt", "vtt",
];

/// What a Content search looks for: each word on its own, or what's in
/// "quotes" as one phrase — exactly as typed (any case, but accents count:
/// "qui" isn't "quỉ"). Each term comes back lower case, its words separated
/// by single spaces.
pub fn words_of(q: &str) -> Vec<String> {
    let q = nfc(q);
    let mut out = Vec::new();
    for (k, part) in q.split('"').enumerate() {
        let quoted = k % 2 == 1;
        let toks: Vec<String> = part.split_whitespace().map(|w| {
            let words: Vec<String> = term_tokens(w);
            words.join(" ")
        }).filter(|w| !w.is_empty()).collect();
        if quoted {
            let all = toks.join(" ");
            if !all.is_empty() {
                out.push(all);
            }
        } else {
            out.extend(toks);
        }
    }
    out.truncate(8);
    out
}

fn lc(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FFFF)
}

/// The words in a piece of typed text (lower case): runs of letters and
/// digits; each Chinese / Japanese character a word of its own.
pub fn term_tokens(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().map(lc).collect();
    word_spans(&chars).into_iter().map(|(a, b)| chars[a..b].iter().collect()).collect()
}

/// Where the words are in `chars`: (start, end) character positions.
fn word_spans(chars: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if is_cjk(c) {
            out.push((i, i + 1));
            i += 1;
        } else if c.is_alphanumeric() {
            let s = i;
            while i < chars.len() && !is_cjk(chars[i]) && (chars[i].is_alphanumeric() || is_mark(chars[i])) {
                i += 1;
            }
            out.push((s, i));
        } else {
            i += 1;
        }
    }
    out
}

fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF)
}

/// Every place one of the search's terms (from `words_of`) is in `text`:
/// (start, end) character positions and which term — whole words only,
/// exactly as typed except for case.
pub fn find_terms(text: &str, terms: &[String]) -> Vec<(usize, usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| lc(*c)).collect();
    let spans = word_spans(&lower);
    let words: Vec<&[char]> = spans.iter().map(|(a, b)| &lower[*a..*b]).collect();
    let pats: Vec<Vec<Vec<char>>> = terms.iter().map(|t| t.split(' ').map(|w| w.chars().collect()).collect()).collect();
    let mut out = Vec::new();
    for i in 0..words.len() {
        for (ti, p) in pats.iter().enumerate() {
            if p.is_empty() || i + p.len() > words.len() {
                continue;
            }
            if p.iter().enumerate().all(|(k, w)| words[i + k] == &w[..]) {
                out.push((spans[i].0, spans[i + p.len() - 1].1, ti));
            }
        }
    }
    out
}

/// Text in its composed form ("e" + U+0302 → "ê"): some files keep accents
/// as separate marks, which would never match what you type.
pub fn nfc(s: &str) -> String {
    if s.is_ascii() {
        return s.to_string();
    }
    let src: Vec<u16> = s.encode_utf16().collect();
    unsafe {
        use windows::Win32::Globalization::{NormalizeString, NormalizationC};
        let n = NormalizeString(NormalizationC, &src, None);
        if n <= 0 {
            return s.to_string();
        }
        let mut dst = vec![0u16; n as usize + 16];
        let n = NormalizeString(NormalizationC, &src, Some(&mut dst));
        if n <= 0 {
            return s.to_string();
        }
        String::from_utf16_lossy(&dst[..n as usize])
    }
}

/// The passage of `path` around the first of `words` (None when it can't be
/// read or isn't there). `stop()` true: give up now (cancelled / closed).
/// The passage, or why there's none ("no reader", "not in its text"…).
pub fn excerpt_why(path: &str, words: &[String], stop: &dyn Fn() -> bool) -> std::result::Result<String, &'static str> {
    if words.is_empty() {
        return Err("no words");
    }
    let ext = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let text = if TEXT_EXTS.contains(&ext.as_str()) {
        read_text(path).ok_or("couldn't read it")?
    } else {
        match unsafe { filter_text(path, words, stop) } {
            Some(t) => t,
            None if stop() => return Err("stopped"),
            None => read_text(path).filter(|t| !t.contains('\u{0}')).ok_or("Windows has no reader for this kind of file")?,
        }
    };
    passage(&nfc(&text), words).ok_or("the words aren't in the text Windows' reader gives")
}

/// A text file's contents (UTF-8, or UTF-16 with its mark), up to 8 MB.
pub(crate) fn read_text(path: &str) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut b = Vec::new();
    f.by_ref().take(8 << 20).read_to_end(&mut b).ok()?;
    if b.starts_with(&[0xFF, 0xFE]) || b.starts_with(&[0xFE, 0xFF]) {
        let le = b[0] == 0xFF;
        let u: Vec<u16> = b[2..].chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
        return Some(String::from_utf16_lossy(&u));
    }
    if b.iter().take(2048).any(|c| *c == 0) {
        return None;
    }
    Some(String::from_utf8_lossy(&b).into_owned())
}

/// The text Windows' filter for this kind of file gives (Word, PDF, Excel,
/// PowerPoint, …): read chunk by chunk, stopping once a word turned up with
/// some text after it, at 2 million characters, or when `stop()` says so.
pub(crate) unsafe fn filter_text(path: &str, words: &[String], stop: &dyn Fn() -> bool) -> Option<String> {
    use windows::Win32::Storage::IndexServer::*;
    let wpath = HSTRING::from(path);
    let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
    LoadIFilter(PCWSTR(wpath.as_ptr()), None, &mut raw).ok()?;
    if raw.is_null() {
        return None;
    }
    let unk = IUnknown::from_raw(raw);
    let f: IFilter = unk.cast().ok()?;
    let mut flags = 0u32;
    let init = IFILTER_INIT_CANON_PARAGRAPHS.0 | IFILTER_INIT_HARD_LINE_BREAKS.0 | IFILTER_INIT_CANON_HYPHENS.0 | IFILTER_INIT_CANON_SPACES.0;
    if f.Init(init as u32, &[], &mut flags) < 0 {
        return None;
    }
    let mut out = String::new();
    let mut lower = String::new();
    let (mut seen, mut lower_checked) = (0usize, 0usize);
    let mut found: Option<usize> = None;
    let mut buf = vec![0u16; 8192];
    let mut chunks = 0;
    loop {
        if stop() {
            return None;
        }
        chunks += 1;
        if chunks > 20_000 {
            break;
        }
        let mut st = STAT_CHUNK::default();
        let hr = f.GetChunk(&mut st);
        if hr == FILTER_E_END_OF_CHUNKS.0 {
            break;
        }
        if hr < 0 {
            // A picture or link it can't read: go on; anything else: stop.
            if hr as u32 == 0x80041707 || hr as u32 == 0x80041708 {
                continue;
            }
            break;
        }
        if st.flags.0 & CHUNK_TEXT.0 == 0 {
            continue;
        }
        loop {
            let mut n = buf.len() as u32;
            let hr = f.GetText(&mut n, PWSTR(buf.as_mut_ptr()));
            if hr < 0 {
                break;
            }
            out.push_str(&String::from_utf16_lossy(&buf[..(n as usize).min(buf.len())]));
            // FILTER_S_LAST_TEXT: that was the chunk's last.
            if hr == 0x0004_1709 {
                break;
            }
        }
        out.push(' ');
        if out.len() > 4_000_000 {
            break;
        }
        // Found, with enough after it for the passage: done. (Only the new
        // text is folded each time; nothing to look for when reading it all.)
        if !words.is_empty() {
            lower.push_str(&out[seen..].to_lowercase());
            seen = out.len();
            if found.is_none() {
                let from = lower_checked.saturating_sub(64);
                let from = (0..=from).rev().find(|&k| lower.is_char_boundary(k)).unwrap_or(0);
                found = words.iter().filter_map(|w| lower[from..].find(w.as_str()).map(|i| i + from)).min();
                lower_checked = lower.len();
            }
            if let Some(i) = found {
                if lower.len() > i + 400 {
                    break;
                }
            }
        }
    }
    Some(out)
}

/// "…a few words before **the word** and a line after…", whitespace folded.
pub fn passage(text: &str, words: &[String]) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    // Every term must be there, exactly; the passage is around the first.
    let found = find_terms(text, words);
    if (0..words.len()).any(|t| !found.iter().any(|f| f.2 == t)) {
        return None;
    }
    let (i, e, _) = found.iter().min_by_key(|f| f.0).copied()?;
    let n = e - i;
    let mut start = i.saturating_sub(40);
    let mut end = (i + n + 110).min(chars.len());
    // Whole words at the edges.
    while start > 0 && start < i && !chars[start - 1].is_whitespace() {
        start += 1;
    }
    while end < chars.len() && end > i + n && !chars[end].is_whitespace() {
        end -= 1;
    }
    let mut s = String::new();
    let mut space = false;
    for c in &chars[start..end] {
        if c.is_whitespace() || c.is_control() {
            space = true;
        } else {
            if space && !s.is_empty() {
                s.push(' ');
            }
            space = false;
            s.push(*c);
        }
    }
    let pre = if start > 0 { "…" } else { "" };
    let post = if end < chars.len() { "…" } else { "" };
    Some(format!("{pre}{s}{post}"))
}
