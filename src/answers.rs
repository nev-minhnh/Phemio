//! Instant answers for the search: base64 (local), currency conversion
//! (Google Finance, falling back to open.er-api.com) and translation
//! (Google Translate). The network ones run on worker threads.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Base64 (standard and URL-safe alphabets)
// ---------------------------------------------------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut buf = 0u32;
    let mut bits = 0;
    let mut out = Vec::new();
    for ch in s.trim().bytes() {
        let v = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        } as u32;
        buf = buf << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

/// Readable text from decoded bytes (no binary junk).
pub fn b64_text(s: &str) -> Option<String> {
    let b = b64_decode(s)?;
    let t = String::from_utf8(b).ok()?;
    let printable = t.chars().filter(|c| !c.is_control() || c.is_whitespace()).count();
    (!t.is_empty() && printable * 100 >= t.chars().count() * 95).then_some(t)
}

/// Looks like base64 on its own (long enough, right alphabet, right length).
pub fn looks_b64(q: &str) -> bool {
    let q = q.trim();
    q.len() >= 8
        && !q.contains(' ')
        && q.len() % 4 == 0
        && q.bytes().all(|c| c.is_ascii_alphanumeric() || b"+/=-_".contains(&c))
        && q.bytes().any(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        && q.bytes().any(|c| c.is_ascii_lowercase())
}

// ---------------------------------------------------------------------------
// Currency
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Fx {
    pub amount: f64,
    pub from: String,
    pub to: String,
}

fn code_of(word: &str) -> Option<&'static str> {
    let w = word.trim().to_lowercase();
    let w = w.trim_end_matches('s');
    Some(match w {
        "$" | "usd" | "dollar" | "us$" | "đô" | "đô la" => "USD",
        "€" | "eur" | "euro" => "EUR",
        "£" | "gbp" | "pound" => "GBP",
        "¥" | "jpy" | "yen" => "JPY",
        "cny" | "rmb" | "yuan" | "nhân dân tệ" | "tệ" => "CNY",
        "₫" | "vnd" | "đ" | "dong" | "đồng" | "vnđ" => "VND",
        "a$" | "aud" => "AUD",
        "c$" | "cad" => "CAD",
        "s$" | "sgd" => "SGD",
        "₩" | "krw" | "won" => "KRW",
        "฿" | "thb" | "baht" => "THB",
        "chf" | "franc" => "CHF",
        "hkd" | "nzd" | "inr" | "idr" | "myr" | "php" | "twd" | "sek" | "nok" | "dkk" | "pln" | "czk" | "huf" | "try" | "rub" | "brl" | "mxn" | "zar" | "aed" | "sar" | "ils" | "lak" | "khr" | "mmk" | "btc" => {
            return Some(match w {
                "hkd" => "HKD",
                "nzd" => "NZD",
                "inr" => "INR",
                "idr" => "IDR",
                "myr" => "MYR",
                "php" => "PHP",
                "twd" => "TWD",
                "sek" => "SEK",
                "nok" => "NOK",
                "dkk" => "DKK",
                "pln" => "PLN",
                "czk" => "CZK",
                "huf" => "HUF",
                "try" => "TRY",
                "rub" => "RUB",
                "brl" => "BRL",
                "mxn" => "MXN",
                "zar" => "ZAR",
                "aed" => "AED",
                "sar" => "SAR",
                "ils" => "ILS",
                "lak" => "LAK",
                "khr" => "KHR",
                "mmk" => "MMK",
                _ => "BTC",
            })
        }
        _ => return None,
    })
}

fn parse_amount(s: &str) -> Option<f64> {
    let s = s.trim().to_lowercase();
    let (num, mult) = if let Some(n) = s.strip_suffix('k') {
        (n, 1e3)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 1e6)
    } else if let Some(n) = s.strip_suffix("tr") {
        (n, 1e6)
    } else {
        (s.as_str(), 1.0)
    };
    // "1,234.5" and "1.234,5" both work; a lone comma is a decimal point.
    let n = num.trim();
    let clean = if n.contains(',') && n.contains('.') {
        if n.rfind(',') > n.rfind('.') { n.replace('.', "").replace(',', ".") } else { n.replace(',', "") }
    } else if n.matches(',').count() == 1 && n.split(',').nth(1).map(|d| d.len() != 3).unwrap_or(false) {
        n.replace(',', ".")
    } else {
        n.replace(',', "")
    };
    clean.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0).map(|v| v * mult)
}

/// "100 usd to vnd", "$25 in eur", "1.5m vnd usd", "50 eur" (→ your currency).
pub fn parse_fx(q: &str, home: &str) -> Option<Fx> {
    let q = q.trim().to_lowercase();
    // Split off a leading currency symbol: "$100" → "$ 100"
    let mut s = q.clone();
    for sym in ["us$", "a$", "c$", "s$", "$", "€", "£", "¥", "₫", "₩", "฿"] {
        if let Some(rest) = s.strip_prefix(sym) {
            if rest.starts_with(|c: char| c.is_ascii_digit()) {
                s = format!("{rest} {sym}");
            }
            break;
        }
    }
    // Split "100usd" → "100 usd"
    let mut spaced = String::new();
    let mut prev_digit = false;
    for c in s.chars() {
        if prev_digit && c.is_alphabetic() && !matches!(c, 'k' | 'm') {
            spaced.push(' ');
        }
        prev_digit = c.is_ascii_digit();
        spaced.push(c);
    }
    let words: Vec<&str> = spaced.split_whitespace().filter(|w| !matches!(*w, "to" | "in" | "=" | "sang" | "ra" | "->" | "→")).collect();
    if words.len() < 2 || words.len() > 4 {
        return None;
    }
    let amount = parse_amount(words[0])?;
    // Currency names can be two words ("đô la").
    let (from, rest) = if words.len() >= 3 && code_of(&format!("{} {}", words[1], words[2])).is_some() {
        (code_of(&format!("{} {}", words[1], words[2]))?, &words[3..])
    } else {
        (code_of(words[1])?, &words[2..])
    };
    let to = match rest {
        [] => {
            if from == home {
                if home == "USD" { "EUR" } else { "USD" }
            } else {
                home
            }
        }
        [t] => code_of(t)?,
        [a, b] => code_of(&format!("{a} {b}"))?,
        _ => return None,
    };
    (from != to).then(|| Fx { amount, from: from.to_string(), to: to.to_string() })
}

static RATES: Mutex<Option<HashMap<String, (f64, Instant, &'static str)>>> = Mutex::new(None);

/// Rate for 1 `from` in `to`, and where it came from.
pub fn rate(from: &str, to: &str) -> Result<(f64, &'static str), String> {
    let key = format!("{from}-{to}");
    if let Some((r, at, src)) = RATES.lock().ok().and_then(|m| m.as_ref().and_then(|m| m.get(&key).cloned())) {
        if at.elapsed() < Duration::from_secs(600) {
            return Ok((r, src));
        }
    }
    let ua = [("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36"), ("Accept-Language", "en-US,en;q=0.8")];
    let google = || -> Option<f64> {
        let b = crate::net::get_with(&format!("https://www.google.com/finance/quote/{from}-{to}"), &ua).ok()?;
        let html = String::from_utf8_lossy(&b);
        let i = html.find("data-last-price=\"")? + "data-last-price=\"".len();
        let end = html[i..].find('"')? + i;
        html[i..end].replace(',', "").parse::<f64>().ok().filter(|v| *v > 0.0)
    };
    let (r, src) = match google() {
        Some(r) => (r, "Google Finance"),
        None => {
            let j = crate::net::get_json(&format!("https://open.er-api.com/v6/latest/{from}"))?;
            let r = j["rates"][to].as_f64().ok_or_else(|| format!("no rate for {to}"))?;
            (r, "ExchangeRate-API")
        }
    };
    if let Ok(mut m) = RATES.lock() {
        m.get_or_insert_with(HashMap::new).insert(key, (r, Instant::now(), src));
    }
    Ok((r, src))
}

/// "2,634,500" / "23.45" / "0.000038"
pub fn fmt_money(v: f64, code: &str) -> String {
    let decimals = if matches!(code, "VND" | "JPY" | "KRW" | "IDR" | "LAK" | "KHR" | "MMK") || v >= 1000.0 {
        if v >= 1000.0 && !matches!(code, "VND" | "JPY" | "KRW" | "IDR" | "LAK" | "KHR" | "MMK") { 2 } else { 0 }
    } else if v >= 1.0 {
        2
    } else {
        6
    };
    let s = format!("{v:.decimals$}");
    let (int, frac) = s.split_once('.').map(|(a, b)| (a.to_string(), Some(b.to_string()))).unwrap_or((s.clone(), None));
    let mut grouped = String::new();
    for (i, c) in int.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    let int: String = grouped.chars().rev().collect();
    match frac {
        Some(f) => {
            let f = f.trim_end_matches('0');
            if f.is_empty() { int } else { format!("{int}.{f}") }
        }
        None => int,
    }
}

// ---------------------------------------------------------------------------
// Translation
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Translation {
    pub text: String,
    /// Detected source language ("en").
    pub from: String,
    pub to: String,
}

/// "tr hello", "translate hello", "dịch hello", "tr ja: hello", "tr hello > ja"
pub fn parse_translate(q: &str) -> Option<(String, Option<String>)> {
    let t = q.trim_start();
    let lower = t.to_lowercase();
    let rest = ["translate ", "dịch ", "tr ", "t: "].iter().find_map(|p| lower.starts_with(p).then(|| t[p.len()..].trim().to_string()))?;
    if rest.is_empty() {
        return None;
    }
    // "ja: text"
    if let Some((code, text)) = rest.split_once(':') {
        let c = code.trim();
        if (2..=5).contains(&c.len()) && c.chars().all(|ch| ch.is_ascii_alphabetic() || ch == '-') && !text.trim().is_empty() {
            return Some((text.trim().to_string(), Some(c.to_lowercase())));
        }
    }
    // "text > ja"
    if let Some((text, code)) = rest.rsplit_once('>') {
        let c = code.trim();
        if (2..=5).contains(&c.len()) && c.chars().all(|ch| ch.is_ascii_alphabetic() || ch == '-') && !text.trim().is_empty() {
            return Some((text.trim().to_string(), Some(c.to_lowercase())));
        }
    }
    Some((rest, None))
}

/// Languages offered in Settings → General → Translate into.
pub const LANGS: [(&str, &str); 7] = [("vi", "Tiếng Việt"), ("en", "English"), ("ja", "日本語"), ("ko", "한국어"), ("zh-CN", "中文"), ("fr", "Français"), ("es", "Español")];
pub const LANG_NAMES: [&str; 7] = ["Tiếng Việt", "English", "日本語", "한국어", "中文", "Français", "Español"];

/// Currencies offered in Settings → General → Your currency.
pub const CURRENCIES: [&str; 7] = ["VND", "USD", "AUD", "EUR", "GBP", "JPY", "CNY"];

pub fn home_currency() -> &'static str {
    CURRENCIES[crate::config::cur().i("currency_home").clamp(0, CURRENCIES.len() as i32 - 1) as usize]
}

/// The language translations go to (Settings → General), Vietnamese by default.
pub fn home_lang() -> String {
    LANGS[crate::config::cur().i("translate_lang").clamp(0, LANGS.len() as i32 - 1) as usize].0.to_string()
}

/// Google Translate. With no target: into your language, or English when
/// the text is already in your language.
pub fn translate(text: &str, to: Option<&str>) -> Result<Translation, String> {
    let home = home_lang();
    let want = to.map(|s| s.to_string()).unwrap_or_else(|| home.clone());
    let call = |tl: &str| -> Result<Translation, String> {
        let url = format!("https://translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl={}&dt=t&q={}", crate::util::url_encode(tl), crate::util::url_encode(text));
        let j = crate::net::get_json_with(&url, &[("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")])?;
        let mut out = String::new();
        if let Some(parts) = j[0].as_array() {
            for p in parts {
                if let Some(s) = p[0].as_str() {
                    out.push_str(s);
                }
            }
        }
        if out.trim().is_empty() {
            return Err("no translation".into());
        }
        Ok(Translation { text: out, from: j[2].as_str().unwrap_or("").to_string(), to: tl.to_string() })
    };
    let r = call(&want)?;
    if to.is_none() && r.from.split('-').next() == want.split('-').next() && want != "en" {
        return call("en");
    }
    Ok(r)
}

/// A dictionary look-up (Google Translate's dictionary data).
#[derive(Default, Debug)]
pub struct Dict {
    pub word: String,
    /// The plain translation.
    pub text: String,
    pub from: String,
    pub to: String,
    /// How the word is read (when Google gives it): "rʌn".
    pub sound: String,
    /// Part of speech → its translations, most common first.
    pub senses: Vec<(String, Vec<String>)>,
    /// Each other translation: (part of speech, the word, what it means back
    /// in the word's language), as Google lists them.
    pub entries: Vec<(String, String, Vec<String>)>,
    /// Part of speech, definition, example (in the word's own language).
    pub defs: Vec<(String, String, String)>,
    /// Example sentences.
    pub examples: Vec<String>,
}

/// "dict run", "def run", "từ điển run", "dict run > ja": the word and the
/// language asked for.
pub fn parse_dict(q: &str) -> Option<(String, Option<String>)> {
    let t = q.trim_start();
    let lower = t.to_lowercase();
    let rest = ["dict ", "def ", "từ điển ", "tudien "].iter().find_map(|p| lower.starts_with(p).then(|| t[p.len()..].trim().to_string()))?;
    if rest.is_empty() {
        return None;
    }
    if let Some((text, code)) = rest.rsplit_once('>') {
        let c = code.trim();
        if (2..=5).contains(&c.len()) && c.chars().all(|ch| ch.is_ascii_alphabetic() || ch == '-') && !text.trim().is_empty() {
            return Some((text.trim().to_string(), Some(c.to_lowercase())));
        }
    }
    Some((rest, None))
}

/// Google Translate as a dictionary: the translation and the word's other
/// translations by part of speech, into the "Translate into" language.
pub fn dictionary(word: &str, to: Option<&str>) -> Result<Dict, String> {
    let home = home_lang();
    let want = to.map(|s| s.to_string()).unwrap_or_else(|| home.clone());
    let call = |tl: &str| -> Result<Dict, String> {
        let url = format!(
            "https://translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl={}&hl={}&dt=t&dt=bd&dt=rm&q={}",
            crate::util::url_encode(tl),
            crate::util::url_encode(tl),
            crate::util::url_encode(word)
        );
        let j = crate::net::get_json_with(&url, &[("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")])?;
        Ok(parse_dict_json(word, tl, &j))
    };
    // Always into the language set in Settings → General → Translate into.
    let d = call(&want)?;
    if d.text.trim().is_empty() && d.senses.is_empty() {
        return Err("no translation".into());
    }
    Ok(d)
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' if tag => tag = false,
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&#39;", "'").replace("&quot;", "\"").replace("&amp;", "&")
}

pub(crate) fn parse_dict_json(word: &str, tl: &str, j: &serde_json::Value) -> Dict {
    let mut d = Dict { word: word.to_string(), to: tl.to_string(), from: j[2].as_str().unwrap_or("").to_string(), ..Default::default() };
    if let Some(parts) = j[0].as_array() {
        for p in parts {
            match p[0].as_str() {
                Some(t) => d.text.push_str(t),
                // The reading line (dt=rm): [null, null, target's, source's].
                None => {
                    if let Some(r) = p[3].as_str().filter(|r| !r.trim().is_empty()) {
                        d.sound = r.trim().to_string();
                    }
                }
            }
        }
    }
    if let Some(bd) = j[1].as_array() {
        for e in bd {
            let pos = e[0].as_str().unwrap_or("").to_string();
            let mut words: Vec<String> = e[1].as_array().map(|a| a.iter().filter_map(|w| w.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
            if words.is_empty() {
                words = e[2].as_array().map(|a| a.iter().filter_map(|w| w[0].as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
            }
            if let Some(list) = e[2].as_array() {
                for x in list {
                    let Some(w) = x[0].as_str() else { continue };
                    let back: Vec<String> = x[1].as_array().map(|a| a.iter().filter_map(|b| b.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
                    d.entries.push((pos.clone(), w.to_string(), back));
                }
            }
            if !words.is_empty() {
                d.senses.push((pos, words));
            }
        }
    }
    if let Some(md) = j[12].as_array() {
        for e in md {
            let pos = e[0].as_str().unwrap_or("").to_string();
            if let Some(list) = e[1].as_array() {
                for x in list.iter().take(3) {
                    if let Some(def) = x[0].as_str().filter(|s| !s.trim().is_empty()) {
                        d.defs.push((pos.clone(), def.trim().to_string(), x[2].as_str().map(|s| strip_tags(s).trim().to_string()).unwrap_or_default()));
                    }
                }
            }
        }
    }
    if let Some(ex) = j[13][0].as_array() {
        for x in ex.iter().take(4) {
            if let Some(t) = x[0].as_str() {
                let t = strip_tags(t).trim().to_string();
                if !t.is_empty() {
                    d.examples.push(t);
                }
            }
        }
    }
    d
}

/// "English", "Vietnamese"… in the app's language.
pub fn lang_label(code: &str) -> String {
    let c = code.split('-').next().unwrap_or(code).to_lowercase();
    let vi = crate::i18n::vi();
    let (en, v) = match c.as_str() {
        "en" => ("English", "Tiếng Anh"),
        "vi" => ("Vietnamese", "Tiếng Việt"),
        "ja" => ("Japanese", "Tiếng Nhật"),
        "ko" => ("Korean", "Tiếng Hàn"),
        "zh" => ("Chinese", "Tiếng Trung"),
        "fr" => ("French", "Tiếng Pháp"),
        "de" => ("German", "Tiếng Đức"),
        "es" => ("Spanish", "Tiếng Tây Ban Nha"),
        "ru" => ("Russian", "Tiếng Nga"),
        "th" => ("Thai", "Tiếng Thái"),
        "it" => ("Italian", "Tiếng Ý"),
        "pt" => ("Portuguese", "Tiếng Bồ Đào Nha"),
        "id" => ("Indonesian", "Tiếng Indonesia"),
        "ar" => ("Arabic", "Tiếng Ả Rập"),
        "hi" => ("Hindi", "Tiếng Hindi"),
        _ => return code.to_uppercase(),
    };
    if vi {
        v.into()
    } else if crate::i18n::lang() == 0 {
        en.into()
    } else if en == "English" {
        // ("English" alone is the Language setting's own name, never translated.)
        crate::i18n::tr("English (the language)").into()
    } else {
        crate::i18n::tr(en).into()
    }
}

// ---------------------------------------------------------------------------
// Web search: Google's top results
// ---------------------------------------------------------------------------

/// Google's own results page (to open in the browser).
pub fn google_url(q: &str) -> String {
    format!("https://www.google.com/search?q={}", crate::util::url_encode(q))
}

#[derive(Clone, Debug, Default)]
pub struct WebHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    /// As the engine shows them (empty when unknown).
    pub site: String,
    pub crumb: String,
    /// The engine's icon for the site: a data: URI or a link.
    pub icon: String,
}

const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";

/// Google's top results, read off Google's own page in a hidden Edge view,
/// so they're the same as in your browser.
pub fn web_search(q: &str) -> Result<Vec<WebHit>, String> {
    if std::env::var("ISLET_NET_MOCK").is_ok() {
        return mock_hits();
    }
    let url = format!("https://www.google.com/search?q={}", crate::util::url_encode(q));
    match crate::webview::read_page(&url, crate::webview::GOOGLE_JS) {
        Ok(v) if !v.is_empty() => Ok(v.into_iter().map(|(h, site, crumb, icon)| WebHit { site, crumb, icon, ..h }).collect()),
        Ok(_) => {
            crate::util::log("web search: Google page had no results");
            Err(crate::i18n::tr("Google's page had no results").to_string())
        }
        Err(e) => {
            crate::util::log(&format!("web search: Google page: {e}"));
            Err(e)
        }
    }
}

/// Developer aid: ISLET_NET_MOCK serves `google_results.json` ([{title,url,snippet}]), or nothing.
fn mock_hits() -> Result<Vec<WebHit>, String> {
    let dir = std::env::var("ISLET_NET_MOCK").unwrap_or_default();
    let b = std::fs::read(format!("{dir}\\google_results.json")).map_err(|_| "Google's page didn't load (mock)".to_string())?;
    let v: serde_json::Value = serde_json::from_slice(&b).map_err(|e| e.to_string())?;
    Ok(v.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|h| {
            let s = |k: &str| h[k].as_str().unwrap_or("").to_string();
            WebHit { title: s("title"), url: s("url"), snippet: s("snippet"), ..Default::default() }
        })
        .collect())
}

/// The site behind a link, Google-style: "Wikipedia", "GitHub", "W3Schools".
pub fn site_name(u: &str) -> String {
    let host = host_of(u);
    let parts: Vec<&str> = host.split('.').collect();
    let main = if parts.len() >= 3 && matches!(parts[parts.len() - 2], "co" | "com" | "org" | "net" | "edu" | "gov" | "ac") {
        parts[parts.len() - 3]
    } else if parts.len() >= 2 {
        parts[parts.len() - 2]
    } else {
        host.as_str()
    };
    let known = [("github", "GitHub"), ("youtube", "YouTube"), ("w3schools", "W3Schools"), ("stackoverflow", "Stack Overflow"), ("linkedin", "LinkedIn"), ("reddit", "Reddit"), ("wikipedia", "Wikipedia"), ("mit", "MIT"), ("bbc", "BBC"), ("cnn", "CNN"), ("vnexpress", "VnExpress")];
    if let Some((_, k)) = known.iter().find(|(d, _)| *d == main) {
        return k.to_string();
    }
    let mut c = main.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

pub fn host_of(u: &str) -> String {
    let s = u.split("://").nth(1).unwrap_or(u);
    s.split(['/', '?', '#']).next().unwrap_or("").trim_start_matches("www.").to_lowercase()
}

/// Site icons for the results, fetched side by side: the icon the results
/// page showed for it (inline pictures decoded here), else the site's own
/// (the one its home page names, or its favicon.ico), else Google's by domain.
/// A picture that turns out the same for different sites is a stand-in, not
/// theirs: those sites get their own looked up, and failing that, None (the
/// row then shows its number).
pub fn favicons(hits: &[WebHit]) -> Vec<Option<Vec<u8>>> {
    let hosts: Vec<String> = hits.iter().map(|h| host_of(&h.url)).collect();
    std::thread::scope(|sc| {
        // 1. What the results page gave, else the site's own, else Google's.
        let hs: Vec<_> = hits
            .iter()
            .zip(&hosts)
            .map(|(h, host)| {
                let (icon, host) = (h.icon.clone(), host.clone());
                sc.spawn(move || page_icon(&icon).or_else(|| own_icon(&host)).or_else(|| google_icon(&host)))
            })
            .collect();
        let mut got: Vec<Option<Vec<u8>>> = hs.into_iter().map(|h| h.join().ok().flatten()).collect();
        // 2. Shared pictures: the site's own instead (no page icon, no Google).
        let bad = shared_pictures(&got, &hosts);
        let again: Vec<_> = (0..got.len())
            .filter(|&i| bad[i] && !hosts[i].is_empty())
            .map(|i| {
                let host = hosts[i].clone();
                (i, sc.spawn(move || own_icon(&host)))
            })
            .collect();
        for (i, h) in again {
            got[i] = h.join().ok().flatten();
        }
        // 3. Still the same picture for different sites: none of them.
        let bad = shared_pictures(&got, &hosts);
        for (i, b) in bad.into_iter().enumerate() {
            if b {
                got[i] = None;
            }
        }
        got
    })
}

fn icon_ok(b: Vec<u8>) -> Option<Vec<u8>> {
    // Real picture bytes, not an HTML error page (or an SVG, which can't be drawn here).
    let text = b.iter().find(|c| !c.is_ascii_whitespace()).map(|c| *c == b'<').unwrap_or(true);
    (b.len() > 60 && !text).then_some(b)
}

/// The icon the results page showed (inline, or its address).
fn page_icon(icon: &str) -> Option<Vec<u8>> {
    if let Some(rest) = icon.strip_prefix("data:") {
        let (_, b64) = rest.split_once("base64,")?;
        return b64_decode(b64).and_then(icon_ok);
    }
    if icon.starts_with("https://") {
        return crate::net::get_with(icon, &[("User-Agent", BROWSER_UA)]).ok().and_then(icon_ok);
    }
    None
}

/// The site's own icon: the one its home page names (`<link rel="icon">`,
/// the biggest it offers), else /favicon.ico.
fn own_icon(host: &str) -> Option<Vec<u8>> {
    if host.is_empty() {
        return None;
    }
    for base in [format!("https://{host}/"), format!("https://www.{host}/")] {
        if let Ok(html) = crate::net::get_with(&base, &[("User-Agent", BROWSER_UA), ("Accept", "text/html")]) {
            let head = String::from_utf8_lossy(&html[..html.len().min(300_000)]).into_owned();
            for href in declared_icons(&head) {
                if let Some(u) = absolute(&base, &href) {
                    if let Some(b) = crate::net::get_with(&u, &[("User-Agent", BROWSER_UA)]).ok().and_then(icon_ok) {
                        return Some(b);
                    }
                }
            }
        }
        if let Some(b) = crate::net::get_with(&format!("{base}favicon.ico"), &[("User-Agent", BROWSER_UA)]).ok().and_then(icon_ok) {
            return Some(b);
        }
    }
    None
}

fn google_icon(host: &str) -> Option<Vec<u8>> {
    if host.is_empty() {
        return None;
    }
    crate::net::get_with(&format!("https://www.google.com/s2/favicons?domain={host}&sz=64"), &[("User-Agent", BROWSER_UA)]).ok().and_then(icon_ok)
}

/// The icons a page names in its `<link rel="…icon…" href="…">` tags, the
/// most useful first (sized 32–192 px, then apple-touch, then any; SVG left
/// out — it can't be drawn here).
pub(crate) fn declared_icons(html: &str) -> Vec<String> {
    let lower = html.to_lowercase();
    let mut found: Vec<(i32, String)> = Vec::new();
    let mut pos = 0;
    while let Some(i) = lower[pos..].find("<link") {
        let start = pos + i;
        let end = lower[start..].find('>').map(|e| start + e).unwrap_or(lower.len());
        let tag = &html[start..end];
        let tl = &lower[start..end];
        pos = end;
        let attr = |name: &str| -> Option<String> {
            let k = tl.find(&format!(" {name}="))? + 1;
            let rest = &tag[k + name.len() + 1..];
            let q = rest.chars().next()?;
            if q == '"' || q == '\'' {
                rest[1..].split(q).next().map(|v| v.to_string())
            } else {
                rest.split([' ', '>']).next().map(|v| v.trim_end_matches('/').to_string())
            }
        };
        let rel = attr("rel").unwrap_or_default().to_lowercase();
        if !rel.contains("icon") || rel.contains("mask") {
            continue;
        }
        let Some(href) = attr("href").filter(|h| !h.is_empty()) else { continue };
        let href = href.replace("&amp;", "&");
        let size: i32 = attr("sizes").and_then(|v| v.split('x').next().and_then(|n| n.trim().parse().ok())).unwrap_or(0);
        let svg = href.to_lowercase().contains(".svg") || attr("type").map(|t| t.contains("svg")).unwrap_or(false);
        let rank = if svg {
            0
        } else if (32..=192).contains(&size) {
            300 + size
        } else if rel.contains("apple-touch") {
            250
        } else {
            100 + size.min(100)
        };
        found.push((rank, href));
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().filter(|f| f.0 > 0).map(|f| f.1).collect()
}

/// `href` as an https address, from the page at `base` ("https://site/").
pub(crate) fn absolute(base: &str, href: &str) -> Option<String> {
    let h = href.trim();
    if h.starts_with("data:") || h.is_empty() {
        return None;
    }
    Some(if let Some(r) = h.strip_prefix("//") {
        format!("https://{r}")
    } else if let Some(r) = h.strip_prefix("http://") {
        format!("https://{r}")
    } else if h.starts_with("https://") {
        h.to_string()
    } else if let Some(r) = h.strip_prefix('/') {
        format!("{base}{r}")
    } else {
        format!("{base}{h}")
    })
}

/// Which pictures appear for two or more different sites (compared by what
/// they look like, not their bytes: a stand-in can come back re-encoded).
fn shared_pictures(got: &[Option<Vec<u8>>], hosts: &[String]) -> Vec<bool> {
    let prints: Vec<Option<u64>> = got.iter().map(|g| g.as_deref().and_then(picture_print)).collect();
    (0..got.len())
        .map(|i| match prints[i] {
            Some(p) => (0..got.len()).any(|j| j != i && hosts[j] != hosts[i] && prints[j] == Some(p)),
            None => false,
        })
        .collect()
}

/// A fingerprint of what a picture looks like: shrunk to 12×12 and hashed
/// (the bytes themselves when it can't be read).
fn picture_print(bytes: &[u8]) -> Option<u64> {
    use windows::core::Interface;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::*;
    let fnv = |data: &[u8]| data.iter().fold(0xcbf29ce484222325u64, |h, x| (h ^ *x as u64).wrapping_mul(0x100000001b3));
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let pixels = (|| -> windows::core::Result<Vec<u8>> {
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let stream = wic.CreateStream()?;
            stream.InitializeFromMemory(bytes)?;
            let dec = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
            let frame = dec.GetFrame(0)?;
            let conv = wic.CreateFormatConverter()?;
            conv.Initialize(&frame.cast::<IWICBitmapSource>()?, &GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
            let sc = wic.CreateBitmapScaler()?;
            sc.Initialize(&conv.cast::<IWICBitmapSource>()?, 12, 12, WICBitmapInterpolationModeFant)?;
            let mut px = vec![0u8; 12 * 12 * 4];
            sc.CopyPixels(std::ptr::null(), 48, &mut px)?;
            Ok(px)
        })();
        Some(match pixels {
            // Rounded a little, so the same picture re-encoded still matches.
            Ok(px) => fnv(&px.iter().map(|v| v >> 3).collect::<Vec<u8>>()),
            Err(_) => fnv(bytes),
        })
    }
}

/// "en.wikipedia.org › wiki › Rust"
pub fn pretty_url(u: &str) -> String {
    let s = u.trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("www.");
    let s = s.split(['?', '#']).next().unwrap_or(s).trim_end_matches('/');
    s.split('/').filter(|p| !p.is_empty()).take(4).collect::<Vec<_>>().join(" › ")
}
