//! Daylight-saving changes, from the time-zone data Windows ships (ICU in
//! `icu.dll`, Windows 10 1903 and later). Without it nothing is shown.

use std::sync::OnceLock;

use windows::core::{s, w};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

type UCal = *mut std::ffi::c_void;
type FnOpen = unsafe extern "C" fn(*const u16, i32, *const u8, i32, *mut i32) -> UCal;
type FnClose = unsafe extern "C" fn(UCal);
type FnSetMillis = unsafe extern "C" fn(UCal, f64, *mut i32);
type FnTransition = unsafe extern "C" fn(UCal, i32, *mut f64, *mut i32) -> i8;
type FnGet = unsafe extern "C" fn(UCal, i32, *mut i32) -> i32;

struct Icu {
    open: FnOpen,
    close: FnClose,
    set_millis: FnSetMillis,
    transition: FnTransition,
    get: FnGet,
}

fn icu() -> Option<&'static Icu> {
    static ICU: OnceLock<Option<Icu>> = OnceLock::new();
    ICU.get_or_init(|| unsafe {
        let m = LoadLibraryW(w!("icu.dll")).ok()?;
        Some(Icu {
            open: std::mem::transmute::<_, FnOpen>(GetProcAddress(m, s!("ucal_open"))?),
            close: std::mem::transmute::<_, FnClose>(GetProcAddress(m, s!("ucal_close"))?),
            set_millis: std::mem::transmute::<_, FnSetMillis>(GetProcAddress(m, s!("ucal_setMillis"))?),
            transition: std::mem::transmute::<_, FnTransition>(GetProcAddress(m, s!("ucal_getTimeZoneTransitionDate"))?),
            get: std::mem::transmute::<_, FnGet>(GetProcAddress(m, s!("ucal_get"))?),
        })
    })
    .as_ref()
}

/// The zone's next clock change within a year: (when, unix seconds; how much
/// the clocks move, seconds — positive when daylight saving starts).
/// `zone` is an IANA name ("Australia/Melbourne"); `None` = this PC's zone.
pub fn next_change(zone: Option<&str>, now: i64) -> Option<(i64, i32)> {
    const ZONE_OFFSET: i32 = 15;
    const DST_OFFSET: i32 = 16;
    // Developer aid (screenshots where icu.dll is missing): ISLET_FAKE_DST=<days>,<seconds>.
    if let Some((d, s)) = std::env::var("ISLET_FAKE_DST").ok().and_then(|v| v.split_once(',').and_then(|(a, b)| Some((a.parse::<i64>().ok()?, b.parse::<i32>().ok()?)))) {
        return zone.is_some().then_some((now + d * 86400, s));
    }
    let icu = icu()?;
    let id: Vec<u16> = zone.unwrap_or("").encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut st = 0i32;
        let cal = (icu.open)(if zone.is_some() { id.as_ptr() } else { std::ptr::null() }, -1, b"en\0".as_ptr(), 0, &mut st);
        if cal.is_null() || st > 0 {
            return None;
        }
        let off = |ms: f64, st: &mut i32| {
            (icu.set_millis)(cal, ms, st);
            (icu.get)(cal, ZONE_OFFSET, st) + (icu.get)(cal, DST_OFFSET, st)
        };
        let now_ms = now as f64 * 1000.0;
        let before = off(now_ms, &mut st);
        let mut t = 0f64;
        let found = (icu.transition)(cal, 0, &mut t, &mut st) != 0;
        let after = if found { off(t + 1000.0, &mut st) } else { before };
        (icu.close)(cal);
        if !found || st > 0 || after == before {
            return None;
        }
        let when = (t / 1000.0) as i64;
        (when - now < 366 * 86400).then_some((when, (after - before) / 1000))
    }
}
