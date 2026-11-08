//! Minimal HTTPS requests over WinHTTP (built into Windows, no TLS library needed).
//! Only ever called from short-lived worker threads.

use windows::core::*;
use windows::Win32::Networking::WinHttp::*;

use crate::util::wide;

/// GET with extra request headers, e.g. `("Referer", "https://…")`.
pub fn get_with(url: &str, headers: &[(&str, &str)]) -> std::result::Result<Vec<u8>, String> {
    let (status, body) = send("GET", url, headers, None)?;
    if status != 200 {
        return Err(format!("HTTP {status}"));
    }
    Ok(body)
}

/// Any request; the status comes back with the body (errors carry details).
pub fn send(method: &str, url: &str, headers: &[(&str, &str)], body: Option<&[u8]>) -> std::result::Result<(u32, Vec<u8>), String> {
    request(method, url, headers, body, None)
}

/// Like `send`, but a successful (200) body is handed to `sink` piece by
/// piece as it arrives (answers written as you wait). `sink` returns false
/// to stop early. Other statuses come back whole, as from `send`.
pub fn stream(method: &str, url: &str, headers: &[(&str, &str)], body: Option<&[u8]>, sink: &mut dyn FnMut(&[u8]) -> bool) -> std::result::Result<(u32, Vec<u8>), String> {
    request(method, url, headers, body, Some(sink))
}

fn request(method: &str, url: &str, headers: &[(&str, &str)], body: Option<&[u8]>, mut sink: Option<&mut dyn FnMut(&[u8]) -> bool>) -> std::result::Result<(u32, Vec<u8>), String> {
    if let Some(dir) = std::env::var_os("ISLET_NET_MOCK") {
        let b = mock(&dir.to_string_lossy(), url)?;
        if let Some(sink) = sink.as_mut() {
            // Played back event by event, as a server would write them.
            let text = String::from_utf8_lossy(&b).into_owned();
            let gap = std::env::var("ISLET_NET_STEP").ok().and_then(|v| v.parse().ok()).unwrap_or(90);
            for ev in text.split_inclusive("\n\n") {
                std::thread::sleep(std::time::Duration::from_millis(gap));
                if !sink(ev.as_bytes()) {
                    break;
                }
            }
            return Ok((200, vec![]));
        }
        return Ok((200, b));
    }
    let rest = url.strip_prefix("https://").ok_or("https only")?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    unsafe {
        let agent = wide("Phemio (Windows)");
        let mut session = WinHttpOpen(PCWSTR(agent.as_ptr()), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
        if session.is_null() {
            session = WinHttpOpen(PCWSTR(agent.as_ptr()), WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
        }
        if session.is_null() {
            return Err("WinHttpOpen failed".into());
        }
        // Answers that are written as you wait (Gemini) get longer.
        let receive = if body.is_some() { 60_000 } else { 15_000 };
        let _ = WinHttpSetTimeouts(session, 8000, 8000, 15_000, receive);
        let h = wide(host);
        let conn = WinHttpConnect(session, PCWSTR(h.as_ptr()), 443, 0);
        let result = (|| {
            if conn.is_null() {
                return Err("connect failed".to_string());
            }
            let p = wide(path);
            let m = wide(method);
            let req = WinHttpOpenRequest(conn, PCWSTR(m.as_ptr()), PCWSTR(p.as_ptr()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE);
            if req.is_null() {
                return Err("open request failed".to_string());
            }
            let r = (|| {
                for (k, v) in headers {
                    let h: Vec<u16> = format!("{k}: {v}\r\n").encode_utf16().collect();
                    let _ = WinHttpAddRequestHeaders(req, &h, WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE);
                }
                match body {
                    Some(b) => WinHttpSendRequest(req, None, Some(b.as_ptr() as *const _), b.len() as u32, b.len() as u32, 0),
                    None => WinHttpSendRequest(req, None, None, 0, 0, 0),
                }
                .map_err(|e| format!("send: {e}"))?;
                WinHttpReceiveResponse(req, std::ptr::null_mut()).map_err(|e| format!("receive: {e}"))?;
                let mut status = 0u32;
                let mut len = 4u32;
                let _ = WinHttpQueryHeaders(
                    req,
                    WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                    PCWSTR::null(),
                    Some(&mut status as *mut u32 as *mut _),
                    &mut len,
                    std::ptr::null_mut(),
                );
                let mut out = Vec::new();
                let live = status == 200 && sink.is_some();
                loop {
                    let mut avail = 0u32;
                    if WinHttpQueryDataAvailable(req, &mut avail).is_err() || avail == 0 {
                        break;
                    }
                    let start = out.len();
                    out.resize(start + avail as usize, 0);
                    let mut read = 0u32;
                    if WinHttpReadData(req, out[start..].as_mut_ptr() as *mut _, avail, &mut read).is_err() {
                        break;
                    }
                    out.truncate(start + read as usize);
                    if live {
                        let go = sink.as_mut().map(|s| s(&out)).unwrap_or(true);
                        out.clear();
                        if !go {
                            break;
                        }
                    } else if out.len() > 4 * 1024 * 1024 {
                        break;
                    }
                }
                Ok((status, out))
            })();
            let _ = WinHttpCloseHandle(req);
            r
        })();
        if !conn.is_null() {
            let _ = WinHttpCloseHandle(conn);
        }
        let _ = WinHttpCloseHandle(session);
        result
    }
}

/// Developer aid: ISLET_NET_MOCK=<folder> serves `<host><path with / → _>.json`.
fn mock(dir: &str, url: &str) -> std::result::Result<Vec<u8>, String> {
    if let Some(ms) = std::env::var("ISLET_NET_DELAY").ok().and_then(|v| v.parse::<u64>().ok()) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    let rest = url.trim_start_matches("https://");
    let no_query = rest.split('?').next().unwrap_or(rest);
    let name = no_query.replace(['/', ':'], "_");
    std::fs::read(format!("{dir}\\{name}.json")).map_err(|_| "HTTP 404".to_string())
}

pub fn get_json(url: &str) -> std::result::Result<serde_json::Value, String> {
    get_json_with(url, &[])
}

pub fn get_json_with(url: &str, headers: &[(&str, &str)]) -> std::result::Result<serde_json::Value, String> {
    let b = get_with(url, headers)?;
    let text = String::from_utf8_lossy(&b);
    // Some APIs wrap JSON in a JSONP callback: name({...})
    let t = text.trim();
    let t = if !t.starts_with('{') && !t.starts_with('[') {
        match (t.find('('), t.rfind(')')) {
            (Some(a), Some(b)) if b > a => &t[a + 1..b],
            _ => t,
        }
    } else {
        t
    };
    serde_json::from_str(t).map_err(|e| format!("bad JSON: {e}"))
}
