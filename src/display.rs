//! This PC's displays, changed the way Settings → System → Display does:
//! where each one sits (and which is the main one), its scale, resolution and
//! orientation. Resolution and orientation changes go back by themselves
//! after 15 seconds unless kept (like Windows' own "Keep changes?").

use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::Win32::Devices::Display::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;

/// Scales Windows offers (percent).
pub const SCALES: [u32; 12] = [100, 125, 150, 175, 200, 225, 250, 300, 350, 400, 450, 500];

/// What a display is set to, and what it could be.
#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub struct Info {
    pub key: String,
    /// Scale now, the one Windows recommends, and the ones on offer.
    pub scale: u32,
    pub scale_rec: u32,
    pub scales: Vec<u32>,
    /// Resolution now (as shown, landscape), the largest (recommended), and all.
    pub res: (u32, u32),
    pub res_rec: (u32, u32),
    pub modes: Vec<(u32, u32)>,
    /// 0 landscape, 1 portrait, 2 landscape (flipped), 3 portrait (flipped).
    pub orient: u32,
}

pub const ORIENTS: [&str; 4] = ["Landscape", "Portrait", "Landscape (flipped)", "Portrait (flipped)"];

/// A change waiting to be kept: (display, resolution, orientation before, by when).
struct Pending {
    key: String,
    res: (u32, u32),
    orient: u32,
    until: Instant,
}
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);
static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn wide(s: &str) -> Vec<u16> {
    crate::util::wide(s)
}

// ---------------------------------------------------------------------------
// Where they sit (Windows' own display configuration, as Settings uses)
// ---------------------------------------------------------------------------

/// Active paths and modes, with each source's GDI name (\\.\DISPLAY1).
fn query() -> Result<(Vec<DISPLAYCONFIG_PATH_INFO>, Vec<DISPLAYCONFIG_MODE_INFO>), String> {
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        let r = GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm);
        if r != ERROR_SUCCESS {
            return Err(format!("GetDisplayConfigBufferSizes {}", r.0));
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        let r = QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None);
        if r != ERROR_SUCCESS {
            return Err(format!("QueryDisplayConfig {}", r.0));
        }
        paths.truncate(np as usize);
        modes.truncate(nm as usize);
        Ok((paths, modes))
    }
}

fn source_name(p: &DISPLAYCONFIG_PATH_INFO) -> String {
    unsafe {
        let mut n = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
        n.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
        n.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
        n.header.adapterId = p.sourceInfo.adapterId;
        n.header.id = p.sourceInfo.id;
        if DisplayConfigGetDeviceInfo(&mut n.header) != 0 {
            return String::new();
        }
        crate::util::from_wide(&n.viewGdiDeviceName)
    }
}

/// Put displays at these desktop positions (device name → x, y); the one at
/// 0, 0 becomes the main display. Applied at once and saved.
pub fn arrange(list: &[(String, i32, i32)]) -> Result<(), String> {
    let _keep = KeepPointer::new();
    match arrange_ccd(list) {
        Ok(()) => Ok(()),
        Err(e) => {
            crate::util::log(&format!("display: arranging the modern way didn't work ({e}); trying the classic way"));
            arrange_classic(list)
        }
    }
}

fn arrange_ccd(list: &[(String, i32, i32)]) -> Result<(), String> {
    let (paths, mut modes) = query()?;
    let mut done = 0;
    for p in &paths {
        let name = source_name(p);
        let Some((_, x, y)) = list.iter().find(|(k, _, _)| k.eq_ignore_ascii_case(&name)) else { continue };
        let idx = unsafe { p.sourceInfo.Anonymous.modeInfoIdx } as usize;
        let Some(m) = modes.get_mut(idx) else { continue };
        if m.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE {
            continue;
        }
        m.Anonymous.sourceMode.position = POINTL { x: *x, y: *y };
        done += 1;
    }
    if done == 0 {
        return Err("no display matched".into());
    }
    let r = unsafe { SetDisplayConfig(Some(&paths), Some(&modes), SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_SAVE_TO_DATABASE | SDC_ALLOW_CHANGES) };
    if r != 0 {
        return Err(format!("SetDisplayConfig {r}"));
    }
    Ok(())
}

fn arrange_classic(list: &[(String, i32, i32)]) -> Result<(), String> {
    unsafe {
        // The new main display first, then the rest.
        let mut order: Vec<&(String, i32, i32)> = list.iter().collect();
        order.sort_by_key(|(_, x, y)| (*x, *y) != (0, 0));
        for (key, x, y) in order {
            let dev = wide(key);
            let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
            if !EnumDisplaySettingsW(windows::core::PCWSTR(dev.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm).as_bool() {
                return Err(format!("couldn't read {key}"));
            }
            dm.dmFields = DM_POSITION;
            dm.Anonymous1.Anonymous2.dmPosition = POINTL { x: *x, y: *y };
            let mut flags = CDS_UPDATEREGISTRY | CDS_NORESET;
            if (*x, *y) == (0, 0) {
                flags |= CDS_SET_PRIMARY;
            }
            let r = ChangeDisplaySettingsExW(windows::core::PCWSTR(dev.as_ptr()), Some(&dm), None, flags, None);
            if r != DISP_CHANGE_SUCCESSFUL {
                return Err(format!("Windows refused the new place of {key} ({})", r.0));
            }
        }
        let r = ChangeDisplaySettingsExW(windows::core::PCWSTR::null(), None, None, CDS_TYPE(0), None);
        if r != DISP_CHANGE_SUCCESSFUL {
            return Err(format!("Windows didn't apply it ({})", r.0));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Scale (the per-display setting Settings changes; not in Windows' public API,
// the same calls Settings and display tools use)
// ---------------------------------------------------------------------------

#[repr(C)]
struct DpiGet {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    min_rel: i32,
    cur_rel: i32,
    max_rel: i32,
}

#[repr(C)]
struct DpiSet {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    rel: i32,
}

fn source_of(key: &str) -> Option<(LUID, u32)> {
    let (paths, _) = query().map_err(|e| crate::util::log(&format!("display: {e}"))).ok()?;
    let found = paths.iter().find(|p| source_name(p).eq_ignore_ascii_case(key)).map(|p| (p.sourceInfo.adapterId, p.sourceInfo.id));
    if found.is_none() {
        let names: Vec<String> = paths.iter().map(source_name).collect();
        crate::util::log(&format!("display: {key} not among Windows' displays {names:?}"));
    }
    found
}

/// (scale now, recommended, the ones on offer) in percent.
fn scale_of(key: &str) -> Option<(u32, u32, Vec<u32>)> {
    let (adapter, id) = source_of(key)?;
    unsafe {
        let mut g = DpiGet { header: DISPLAYCONFIG_DEVICE_INFO_HEADER::default(), min_rel: 0, cur_rel: 0, max_rel: 0 };
        g.header.r#type = DISPLAYCONFIG_DEVICE_INFO_TYPE(-3);
        g.header.size = std::mem::size_of::<DpiGet>() as u32;
        g.header.adapterId = adapter;
        g.header.id = id;
        let r = DisplayConfigGetDeviceInfo(&mut g.header);
        if r != 0 {
            crate::util::log(&format!("display: {key}: Windows didn't give its scales ({r})"));
            return None;
        }
        let rec = (-g.min_rel).max(0) as usize;
        let at = |rel: i32| SCALES.get((rec as i32 + rel).clamp(0, SCALES.len() as i32 - 1) as usize).copied().unwrap_or(100);
        let lo = (rec as i32 + g.min_rel).max(0) as usize;
        let hi = ((rec as i32 + g.max_rel).max(0) as usize).min(SCALES.len() - 1);
        Some((at(g.cur_rel), at(0), SCALES[lo..=hi].to_vec()))
    }
}

/// Set a display's scale (percent).
pub fn set_scale(key: &str, pct: u32) -> Result<(), String> {
    let _keep = KeepPointer::new();
    let (adapter, id) = source_of(key).ok_or("display not found")?;
    unsafe {
        let mut g = DpiGet { header: DISPLAYCONFIG_DEVICE_INFO_HEADER::default(), min_rel: 0, cur_rel: 0, max_rel: 0 };
        g.header.r#type = DISPLAYCONFIG_DEVICE_INFO_TYPE(-3);
        g.header.size = std::mem::size_of::<DpiGet>() as u32;
        g.header.adapterId = adapter;
        g.header.id = id;
        if DisplayConfigGetDeviceInfo(&mut g.header) != 0 {
            return Err("Windows didn't say this display's scales".into());
        }
        let rec = (-g.min_rel).max(0) as i32;
        let want = SCALES.iter().position(|s| *s == pct).ok_or("no such scale")? as i32;
        let rel = (want - rec).clamp(g.min_rel, g.max_rel);
        let mut s = DpiSet { header: DISPLAYCONFIG_DEVICE_INFO_HEADER::default(), rel };
        s.header.r#type = DISPLAYCONFIG_DEVICE_INFO_TYPE(-4);
        s.header.size = std::mem::size_of::<DpiSet>() as u32;
        s.header.adapterId = adapter;
        s.header.id = id;
        let r = DisplayConfigSetDeviceInfo(&s.header);
        if r != 0 {
            return Err(format!("Windows refused the scale ({r})"));
        }
    }
    crate::util::log(&format!("display: {key} scale {pct}%"));
    Ok(())
}

// ---------------------------------------------------------------------------
// Resolution and orientation
// ---------------------------------------------------------------------------

fn current(key: &str) -> Option<DEVMODEW> {
    let dev = wide(key);
    let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
    unsafe { EnumDisplaySettingsW(windows::core::PCWSTR(dev.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm).as_bool().then_some(dm) }
}

fn orient_of(dm: &DEVMODEW) -> u32 {
    unsafe { dm.Anonymous1.Anonymous2.dmDisplayOrientation.0 }
}

/// Everything about one of this PC's displays.
pub fn info(key: &str) -> Option<Info> {
    let dm = current(key)?;
    let orient = orient_of(&dm);
    let land = |w: u32, h: u32| if w >= h { (w, h) } else { (h, w) };
    let res = land(dm.dmPelsWidth, dm.dmPelsHeight);
    let mut modes: Vec<(u32, u32)> = Vec::new();
    let dev = wide(key);
    let mut i = 0u32;
    loop {
        let mut m = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
        let ok = unsafe { EnumDisplaySettingsW(windows::core::PCWSTR(dev.as_ptr()), ENUM_DISPLAY_SETTINGS_MODE(i), &mut m).as_bool() };
        if !ok || i > 4000 {
            break;
        }
        i += 1;
        if m.dmBitsPerPel != dm.dmBitsPerPel || m.dmPelsWidth < 640 || m.dmPelsHeight < 480 {
            continue;
        }
        let r = land(m.dmPelsWidth, m.dmPelsHeight);
        if !modes.contains(&r) {
            modes.push(r);
        }
    }
    if !modes.contains(&res) {
        modes.push(res);
    }
    modes.sort_by(|a, b| (b.0 * b.1).cmp(&(a.0 * a.1)).then(b.0.cmp(&a.0)));
    let res_rec = modes.first().copied().unwrap_or(res);
    let (scale, scale_rec, scales) = scale_of(key).unwrap_or_else(|| {
        // (Windows didn't say: what this display runs at now.)
        let s = crate::link::local_screens().iter().find(|s| s.key == key).map(|s| (s.scale * 100.0).round() as u32).unwrap_or(100);
        (s, s, vec![s])
    });
    Some(Info { key: key.to_string(), scale, scale_rec, scales, res, res_rec, modes, orient })
}

/// Change the resolution (landscape size) and orientation; goes back after
/// 15 seconds unless `keep` is called.
pub fn set_mode(key: &str, res: (u32, u32), orient: u32) -> Result<(), String> {
    let before = current(key).ok_or("display not found")?;
    let old_res = if before.dmPelsWidth >= before.dmPelsHeight { (before.dmPelsWidth, before.dmPelsHeight) } else { (before.dmPelsHeight, before.dmPelsWidth) };
    let old_orient = orient_of(&before);
    apply_mode(key, res, orient)?;
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    // A newer change keeps the first "before": that's what going back means.
    if let Ok(mut p) = PENDING.lock() {
        let (r0, o0) = match p.as_ref().filter(|x| x.key == key) {
            Some(x) => (x.res, x.orient),
            None => (old_res, old_orient),
        };
        *p = Some(Pending { key: key.to_string(), res: r0, orient: o0, until: Instant::now() + Duration::from_secs(15) });
    }
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(15));
        if SEQ.load(std::sync::atomic::Ordering::SeqCst) == seq {
            let _ = revert();
        }
    });
    Ok(())
}

fn apply_mode(key: &str, res: (u32, u32), orient: u32) -> Result<(), String> {
    let _keep = KeepPointer::new();
    let mut dm = current(key).ok_or("display not found")?;
    let portrait = orient % 2 == 1;
    let (w, h) = if portrait { (res.1, res.0) } else { (res.0, res.1) };
    dm.dmPelsWidth = w;
    dm.dmPelsHeight = h;
    dm.Anonymous1.Anonymous2.dmDisplayOrientation = DEVMODE_DISPLAY_ORIENTATION(orient);
    dm.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYORIENTATION;
    let dev = wide(key);
    let r = unsafe { ChangeDisplaySettingsExW(windows::core::PCWSTR(dev.as_ptr()), Some(&dm), None, CDS_UPDATEREGISTRY, None) };
    if r != DISP_CHANGE_SUCCESSFUL {
        return Err(format!("Windows refused {w}×{h} ({})", r.0));
    }
    crate::util::log(&format!("display: {key} {w}×{h}, orientation {orient}"));
    Ok(())
}

/// A change is waiting to be kept: (display, seconds left).
pub fn pending() -> Option<(String, u64)> {
    PENDING.lock().ok().and_then(|p| p.as_ref().map(|x| (x.key.clone(), x.until.saturating_duration_since(Instant::now()).as_secs() + 1)))
}

pub fn keep() {
    SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if let Ok(mut p) = PENDING.lock() {
        *p = None;
    }
}

/// Back to how it was before the change.
pub fn revert() -> Result<(), String> {
    SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let p = PENDING.lock().ok().and_then(|mut p| p.take());
    match p {
        Some(p) => apply_mode(&p.key, p.res, p.orient),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// The pointer stays where it was
// ---------------------------------------------------------------------------

/// Windows puts the pointer in the middle of a display when its mode changes
/// (and again as the change settles). Held across a change: the pointer goes
/// back where it was (kept on a screen), unless you've moved it since.
struct KeepPointer(Option<POINT>);

impl KeepPointer {
    fn new() -> KeepPointer {
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut p = POINT::default();
        KeepPointer(unsafe { GetCursorPos(&mut p) }.ok().map(|_| p))
    }
}

impl Drop for KeepPointer {
    fn drop(&mut self) {
        let Some(was) = self.0 else { return };
        put_back(was);
        // Windows may move it again a moment later, as the displays settle.
        std::thread::spawn(move || {
            for ms in [120u64, 250, 400, 700, 1200] {
                std::thread::sleep(Duration::from_millis(ms));
                put_back(was);
            }
        });
    }
}

/// Is `p` the middle of one of the displays (where Windows parks the pointer)?
fn at_a_middle(p: POINT) -> bool {
    crate::link::local_screens().iter().any(|s| (p.x - (s.x + s.w / 2)).abs() <= 2 && (p.y - (s.y + s.h / 2)).abs() <= 2)
}

fn put_back(was: POINT) {
    use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};
    let mut now = POINT::default();
    if unsafe { GetCursorPos(&mut now) }.is_err() || (now.x == was.x && now.y == was.y) {
        return;
    }
    // Only where Windows put it: if you've moved it yourself, leave it.
    if !at_a_middle(now) {
        return;
    }
    // Still on a screen (a display may have changed size or turned).
    let screens = crate::link::local_screens();
    let on = |p: POINT| screens.iter().any(|s| p.x >= s.x && p.x < s.x + s.w && p.y >= s.y && p.y < s.y + s.h);
    let to = if on(was) {
        was
    } else {
        // The nearest point of the nearest screen.
        screens
            .iter()
            .map(|s| POINT { x: was.x.clamp(s.x, s.x + s.w - 1), y: was.y.clamp(s.y, s.y + s.h - 1) })
            .min_by_key(|p| (p.x - was.x).abs() + (p.y - was.y).abs())
            .unwrap_or(was)
    };
    unsafe {
        let _ = SetCursorPos(to.x, to.y);
    }
}
