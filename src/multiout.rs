//! Play on several outputs at once (Devices → Sound output → "Play on
//! several"). Windows plays every app to one device, the *main* output (the
//! Windows default — pick another in the list to change it). While this is
//! on, Phemio records what the main output plays (WASAPI loopback, no
//! driver needed) and copies it to each ticked device, each with its own
//! delay and volume, so they can be lined up with the main one.
//!
//! Off = no audio thread, nothing recorded, no CPU.
//!
//! Lining up:
//! - **Auto-sync** estimates each device's delay (Bluetooth speakers run
//!   ~170 ms behind, wired ones ~10 ms) plus the copy's own path.
//! - **Calibrate** plays a short chirp on the main output and listens with
//!   the microphone: once with the device silent (when the main output is
//!   heard), once with the device on and pushed 400 ms later (when its copy
//!   is heard). The difference is the delay that lines it up.
//! - The sliders set it by hand.
//!
//! **The linked PC's speakers** (Link) are one more row in the list while the
//! two PCs are linked: its copy travels over the network (`link::audio`) and
//! plays on that PC's default output. Its delay and volume are applied over
//! there; it can't be the main output.
//!
//! The main output can't be delayed (apps play to it directly), so the copies
//! are always a little later than it: a device that is slower than the main
//! one (a Bluetooth speaker copied from built-in speakers) can't be lined up —
//! make that one the main output instead (the page says so).

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::*;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

use crate::config;
use crate::util;

/// A device that gets a copy: whether it's ticked, its delay (ms) and volume.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub id: String,
    pub on: bool,
    pub delay: i32,
    pub vol: f32,
}

/// What the page shows.
#[derive(Clone, Debug, Default)]
pub struct Status {
    /// The copying is running.
    pub running: bool,
    /// A line under the list ("Calibrating…", "Lined up", an error), and
    /// whether it's a problem.
    pub msg: String,
    pub bad: bool,
    /// Devices that are slower than the main output (can't be lined up): id → ms behind.
    pub behind: Vec<(String, i32)>,
    pub busy: bool,
}

static TARGETS: Mutex<Vec<Target>> = Mutex::new(Vec::new());
static LOADED: AtomicBool = AtomicBool::new(false);
static STATUS: Mutex<Option<Status>> = Mutex::new(None);
/// Bumped to start a new engine (the old one sees it and stops).
static RUN: AtomicU64 = AtomicU64::new(0);
/// Devices silenced for a moment (calibration).
static MUTED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
/// Extra delay for a moment (calibration): id → ms.
static EXTRA: Mutex<Vec<(String, i32)>> = Mutex::new(Vec::new());
/// Tell the window to repaint when something changes.
static HOST: AtomicU64 = AtomicU64::new(0);
static HOST_MSG: AtomicU64 = AtomicU64::new(0);

pub const MAX_DELAY: i32 = 800;

use crate::link::audio::is_remote;

/// The linked PC's row is muted (its volume stays where it was).
static REMOTE_MUTE: AtomicBool = AtomicBool::new(false);
/// The linked PC's volume was moved: saved once the slider rests.
static VOL_MOVED: Mutex<Option<Instant>> = Mutex::new(None);

/// The outputs on the page: this PC's, and — while Play on several is on and
/// the PCs are linked — the linked PC's speakers.
pub fn outputs() -> Vec<crate::audio::Output> {
    let mut v = crate::audio::list_outputs();
    if config::cur().b("mo_on") {
        if let Some((id, name)) = crate::link::audio::peer() {
            v.push(crate::audio::Output { id, name: format!("{name} · Link"), form: FORM_LINKED_PC, default: false });
        }
    }
    v
}

/// Developer aid (ISLET_MO_LINK=1): play on the linked PC without ticking its row.
fn dev_link() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("ISLET_MO_LINK").is_ok())
}

/// `Output::form` of the linked PC's row (not one of Windows' form factors).
pub const FORM_LINKED_PC: u32 = 1000;

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

fn load() {
    if LOADED.swap(true, Ordering::AcqRel) {
        return;
    }
    let s = config::cur().s("mo_targets");
    let v: Vec<Target> = s
        .split(';')
        .filter_map(|e| {
            let p: Vec<&str> = e.split('|').collect();
            (p.len() == 4 && !p[0].is_empty()).then(|| Target { id: p[0].to_string(), on: p[1] == "1", delay: p[2].parse().unwrap_or(0), vol: p[3].parse().unwrap_or(1.0) })
        })
        .collect();
    if let Ok(mut t) = TARGETS.lock() {
        *t = v;
    }
}

fn save() {
    let s: Vec<String> = targets().iter().map(|t| format!("{}|{}|{}|{:.2}", t.id.replace([';', '|'], ""), t.on as u8, t.delay, t.vol)).collect();
    config::update(|c| c.set_s("mo_targets", &s.join(";")));
}

pub fn targets() -> Vec<Target> {
    load();
    TARGETS.lock().map(|t| t.clone()).unwrap_or_default()
}

pub fn target(id: &str) -> Target {
    targets().into_iter().find(|t| t.id == id).unwrap_or(Target { id: id.to_string(), on: false, delay: 0, vol: 1.0 })
}

fn edit(id: &str, f: impl FnOnce(&mut Target)) {
    load();
    if let Ok(mut v) = TARGETS.lock() {
        if !v.iter().any(|t| t.id == id) {
            v.push(Target { id: id.to_string(), on: false, delay: 0, vol: 1.0 });
        }
        if let Some(t) = v.iter_mut().find(|t| t.id == id) {
            f(t);
        }
    }
}

pub fn set_on(id: &str, on: bool) {
    edit(id, |t| t.on = on);
    save();
    apply();
}

/// Delay (ms); `keep` saves it (on release of the slider).
pub fn set_delay(id: &str, ms: i32, keep: bool) {
    edit(id, |t| t.delay = ms.clamp(0, MAX_DELAY));
    if keep {
        save();
    }
}


pub fn status() -> Status {
    STATUS.lock().ok().and_then(|s| s.clone()).unwrap_or_default()
}

fn set_status(f: impl FnOnce(&mut Status)) {
    if let Ok(mut s) = STATUS.lock() {
        let st = s.get_or_insert_with(Status::default);
        f(st);
    }
    poke();
}

fn note(msg: &str, bad: bool) {
    let m = msg.to_string();
    set_status(|s| {
        s.msg = m;
        s.bad = bad;
    });
}

fn poke() {
    let h = HOST.load(Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(Some(HWND(h as isize as *mut _)), HOST_MSG.load(Ordering::Relaxed) as u32, WPARAM(0), LPARAM(0));
        }
    }
}

/// Where to send "repaint" (the island's host window and message).
pub fn init(host: HWND, msg: u32) {
    HOST.store(host.0 as isize as u64, Ordering::Relaxed);
    HOST_MSG.store(msg as u64, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Start / stop
// ---------------------------------------------------------------------------

/// Start, restart or stop the copying to match the settings (call after the
/// switch, the ticks or the main output change).
pub fn apply() {
    let on = config::cur().b("mo_on") && (targets().iter().any(|t| t.on) || dev_link());
    // The volume keys move all the speakers while they play together.
    crate::sysmon::VOL_KEYS.store(on && cluster().len() > 1, Ordering::Relaxed);
    let g = RUN.fetch_add(1, Ordering::AcqRel) + 1;
    if !on {
        set_status(|s| s.running = false);
        return;
    }
    let _ = std::thread::Builder::new().name("multi-out".into()).stack_size(256 * 1024).spawn(move || {
        // Let a previous engine notice and close its streams first.
        std::thread::sleep(Duration::from_millis(120));
        if RUN.load(Ordering::Acquire) != g {
            return;
        }
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let r = std::panic::catch_unwind(|| engine(g));
        if let Err(_) | Ok(Err(_)) = &r {
            let e = match &r {
                Ok(Err(e)) => e.message().to_string(),
                _ => "stopped".to_string(),
            };
            util::log(&format!("multi-out: {e}"));
            if RUN.load(Ordering::Acquire) == g {
                note(&format!("{}: {e}", crate::i18n::tr("Couldn't play on several outputs")), true);
            }
        }
        if RUN.load(Ordering::Acquire) == g {
            set_status(|s| s.running = false);
        }
        unsafe { CoUninitialize() };
    });
}

/// Stop for good (app closing).
pub fn stop() {
    RUN.fetch_add(1, Ordering::AcqRel);
}

// ---------------------------------------------------------------------------
// Device helpers
// ---------------------------------------------------------------------------

/// DEVPKEY_Device_EnumeratorName ("BTHENUM", "BTHLEDEVICE", "HDAUDIO", "USB" …).
const PKEY_ENUMERATOR: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0), pid: 24 };

/// Is this output a Bluetooth device?
pub fn is_bluetooth(id: &str) -> bool {
    unsafe {
        let Some(en) = crate::audio::enumerator() else { return false };
        let Ok(dev) = en.GetDevice(&HSTRING::from(id)) else { return false };
        let Ok(store) = dev.OpenPropertyStore(STGM_READ) else { return false };
        let name = store
            .GetValue(&PKEY_ENUMERATOR)
            .ok()
            .and_then(|v| PropVariantToStringAlloc(&v).ok())
            .map(|p| {
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as *const _));
                s
            })
            .unwrap_or_default()
            .to_uppercase();
        name.starts_with("BTH")
    }
}

fn main_id() -> Option<String> {
    crate::audio::list_outputs().into_iter().find(|o| o.default).map(|o| o.id)
}

fn qpc_100ns() -> i64 {
    unsafe {
        let (mut c, mut f) = (0i64, 0i64);
        let _ = QueryPerformanceCounter(&mut c);
        let _ = QueryPerformanceFrequency(&mut f);
        if f == 0 {
            return 0;
        }
        (c as i128 * 10_000_000 / f as i128) as i64
    }
}

/// Is the stream format 32-bit float (or 16-bit PCM)? (bits, float)
unsafe fn format_kind(fmt: *const WAVEFORMATEX) -> (u16, bool) {
    let f = &*fmt;
    let bits = f.wBitsPerSample;
    let tag = f.wFormatTag as u32;
    let float = if tag == 0xFFFE {
        let ext = &*(fmt as *const WAVEFORMATEXTENSIBLE);
        let sub = ext.SubFormat;
        sub == windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
    } else {
        tag == 3
    };
    (bits, float)
}

// ---------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------

/// Frees a format Windows allocated, whatever path we leave by.
struct Fmt(*mut WAVEFORMATEX);
impl Drop for Fmt {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CoTaskMemFree(Some(self.0 as *const _)) };
        }
    }
}

/// An opened copy, handed over from the thread that opened it (both threads
/// are in COM's multithreaded apartment, where these objects may move).
struct Opened(String, Result<Out>);
unsafe impl Send for Opened {}

struct Out {
    id: String,
    client: IAudioClient,
    render: IAudioRenderClient,
    volume: Option<ISimpleAudioVolume>,
    buf: u32,
    queue: VecDeque<u8>,
    /// Delay (frames) the queue currently holds on purpose.
    delay: usize,
    vol_set: f32,
}

fn engine(g: u64) -> Result<()> {
    unsafe {
        let en = crate::audio::enumerator().ok_or_else(|| Error::from(E_FAIL))?;
        let main = en.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
        let main_id = {
            let p = main.GetId()?;
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        };
        // (Developer aid, ISLET_MO_CAPTURE=1: record the default microphone
        // instead — for systems without loopback recording, like Wine.)
        let dev_cap = std::env::var("ISLET_MO_CAPTURE").is_ok();
        let cap: IAudioClient = if dev_cap { en.GetDefaultAudioEndpoint(eCapture, eMultimedia)?.Activate(CLSCTX_ALL, None)? } else { main.Activate(CLSCTX_ALL, None)? };
        let fmt_g = Fmt(cap.GetMixFormat()?);
        let fmt = fmt_g.0;
        let rate = (*fmt).nSamplesPerSec as usize;
        let block = (*fmt).nBlockAlign as usize;
        // A full second of room, so a busy moment never loses sound.
        cap.Initialize(AUDCLNT_SHAREMODE_SHARED, if dev_cap { 0 } else { AUDCLNT_STREAMFLAGS_LOOPBACK }, 10_000_000, 0, fmt, None)?;
        let cc: IAudioCaptureClient = cap.GetService()?;
        cap.Start()?;
        util::log(&format!("multi-out: copying {main_id} ({rate} Hz, {block} B/frame)"));
        // (Only if this run is still the wanted one.)
        if let Ok(mut st) = STATUS.lock() {
            if RUN.load(Ordering::Acquire) == g {
                let s = st.get_or_insert_with(Status::default);
                s.running = true;
                s.msg.clear();
                s.bad = false;
            }
        }
        poke();
        // The format, as bytes, for the threads that open the copies.
        let fmt_bytes: Vec<u8> = std::slice::from_raw_parts(fmt as *const u8, std::mem::size_of::<WAVEFORMATEX>() + (*fmt).cbSize as usize).to_vec();
        let (open_tx, open_rx) = std::sync::mpsc::channel::<Opened>();
        let mut opening: HashSet<String> = HashSet::new();
        let ms = |m: i64| (m.max(0) as usize * rate) / 1000;
        let mut outs: Vec<Out> = Vec::new();
        let mut retry_at: Vec<(String, Instant)> = Vec::new();
        // The copy that goes to the linked PC (if its row is ticked).
        let mut remote: Option<Remote> = None;
        let mut remote_tried = crate::util::ago(Duration::from_secs(10));
        let t0 = Instant::now();
        let mut captured: u64 = 0;
        let mut last_sync = crate::util::ago(Duration::from_secs(10));
        let fill_target = ms(50);
        let mut result: Result<()> = Ok(());
        'run: while RUN.load(Ordering::Acquire) == g {
            // ---- Keep the list of copies in line with the settings ----
            if last_sync.elapsed() >= Duration::from_millis(100) {
                last_sync = Instant::now();
                // (Developer aid: the linked PC's row counts as ticked.)
                if dev_link() {
                    if let Some((id, _)) = crate::link::audio::peer() {
                        if !target(&id).on {
                            edit(&id, |t| t.on = true);
                        }
                    }
                }
                let all: Vec<Target> = targets().into_iter().filter(|t| t.on && t.id != main_id).collect();
                let want: Vec<Target> = all.iter().filter(|t| !is_remote(&t.id)).cloned().collect();
                let muted = MUTED.lock().ok().and_then(|m| m.clone()).unwrap_or_default();
                let extra = EXTRA.lock().map(|e| e.clone()).unwrap_or_default();
                // ---- The linked PC: only the one linked now, and never while
                // it is playing its own sound here (that would go round in a loop).
                {
                    use crate::link::audio as la;
                    let peer = la::peer().map(|p| p.0);
                    let t = all.iter().find(|t| Some(&t.id) == peer.as_ref()).filter(|_| !la::receiving());
                    if remote.as_ref().map(|r| t.map(|t| t.id != r.id).unwrap_or(true) || r.tx.state() == la::DEAD).unwrap_or(false) {
                        remote = None;
                        remote_tried = Instant::now();
                    }
                    if let Some(t) = t {
                        match remote.as_mut() {
                            Some(r) => {
                                if !r.live && r.tx.state() == la::PLAYING {
                                    r.live = true;
                                    r.set = (u16::MAX, -1.0);
                                }
                                let delay = (t.delay + extra.iter().find(|e| e.0 == t.id).map(|e| e.1).unwrap_or(0)).clamp(0, MAX_DELAY + 1000) as u16;
                                let vol = if muted.contains(&t.id) || REMOTE_MUTE.load(Ordering::Relaxed) { 0.0 } else { t.vol.clamp(0.0, 1.0) };
                                if r.live && (delay != r.set.0 || (vol - r.set.1).abs() > 0.001) {
                                    r.set = (delay, vol);
                                    r.tx.send(la::Out::Set { delay, vol });
                                }
                            }
                            None if remote_tried.elapsed() >= Duration::from_secs(3) => {
                                remote_tried = Instant::now();
                                remote = Some(Remote { id: t.id.clone(), tx: la::open(fmt_bytes.clone()), live: false, set: (u16::MAX, -1.0), buf: Vec::new(), gap: 0 });
                            }
                            None => {}
                        }
                    }
                    // (Its volume slider came to rest: keep it.)
                    let rested = VOL_MOVED.lock().map(|m| m.map(|t| t.elapsed() > Duration::from_millis(800)).unwrap_or(false)).unwrap_or(false);
                    if rested {
                        if let Ok(mut m) = VOL_MOVED.lock() {
                            *m = None;
                        }
                        save();
                    }
                }
                outs.retain(|o| {
                    let keep = want.iter().any(|t| t.id == o.id);
                    if !keep {
                        let _ = o.client.Stop();
                    }
                    keep
                });
                for t in &want {
                    let want_delay = ms((t.delay + extra.iter().find(|e| e.0 == t.id).map(|e| e.1).unwrap_or(0)) as i64);
                    match outs.iter_mut().find(|o| o.id == t.id) {
                        Some(o) => {
                            // (The copy is moved to its new delay below, where it's fed.)
                            o.delay = want_delay;
                            let v = if muted.contains(&t.id) { 0.0 } else { 1.0 };
                            if (v - o.vol_set).abs() > 0.001 {
                                if let Some(sv) = &o.volume {
                                    let _ = sv.SetMasterVolume(v, std::ptr::null());
                                }
                                o.vol_set = v;
                            }
                        }
                        None => {
                            if opening.contains(&t.id) || retry_at.iter().any(|(id, at)| id == &t.id && at.elapsed() < Duration::from_secs(3)) {
                                continue;
                            }
                            // Opening a Bluetooth speaker can take half a second: do it
                            // on the side so the other copies keep playing.
                            opening.insert(t.id.clone());
                            let (id, tx, fb) = (t.id.clone(), open_tx.clone(), fmt_bytes.clone());
                            std::thread::spawn(move || {
                                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                                let r = match crate::audio::enumerator() {
                                    Some(en) => open_out(&en, &id, fb.as_ptr() as *const WAVEFORMATEX),
                                    None => Err(Error::from(E_FAIL)),
                                };
                                let _ = tx.send(Opened(id, r));
                                CoUninitialize();
                            });
                        }
                    }
                }
            }
            // Copies that finished opening.
            while let Ok(Opened(id, r)) = open_rx.try_recv() {
                opening.remove(&id);
                match r {
                    Ok(mut o) if RUN.load(Ordering::Acquire) == g => {
                        if let Some(t) = targets().into_iter().find(|t| t.id == id && t.on) {
                            let extra = EXTRA.lock().map(|e| e.iter().find(|e| e.0 == id).map(|e| e.1).unwrap_or(0)).unwrap_or(0);
                            o.delay = ms((t.delay + extra) as i64);
                            let muted = MUTED.lock().ok().and_then(|m| m.as_ref().map(|m| m.contains(&id))).unwrap_or(false);
                            let v = if muted { 0.0 } else { 1.0 };
                            if let Some(sv) = &o.volume {
                                let _ = sv.SetMasterVolume(v, std::ptr::null());
                            }
                            o.vol_set = v;
                            retry_at.retain(|(i, _)| i != &id);
                            util::log(&format!("multi-out: + {id}"));
                            outs.push(o);
                        } else {
                            let _ = o.client.Stop();
                        }
                    }
                    Ok(o) => {
                        let _ = o.client.Stop();
                    }
                    Err(e) => {
                        util::log(&format!("multi-out: can't open {id}: {}", e.message()));
                        retry_at.retain(|(i, _)| i != &id);
                        retry_at.push((id, Instant::now()));
                    }
                }
            }
            // ---- Take what the main output played ----
            let mut got = 0u64;
            loop {
                let n = match cc.GetNextPacketSize() {
                    Ok(n) => n,
                    Err(e) => {
                        // The main output went away (or changed): start over.
                        result = Err(e);
                        break 'run;
                    }
                };
                if n == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let (mut frames, mut flags) = (0u32, 0u32);
                if let Err(e) = cc.GetBuffer(&mut data, &mut frames, &mut flags, None, None) {
                    result = Err(e);
                    break 'run;
                }
                let bytes = frames as usize * block;
                let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null();
                let src = if silent { &[][..] } else { std::slice::from_raw_parts(data, bytes) };
                for o in outs.iter_mut() {
                    if silent {
                        o.queue.extend(std::iter::repeat_n(0u8, bytes));
                    } else {
                        o.queue.extend(src.iter().copied());
                    }
                }
                if let Some(r) = remote.as_mut().filter(|r| r.live) {
                    if silent {
                        r.silence(frames);
                    } else {
                        r.data(src);
                    }
                }
                let _ = cc.ReleaseBuffer(frames);
                got += frames as u64;
            }
            // Nothing playing: Windows sends nothing at all, so keep time with silence.
            let expected = (t0.elapsed().as_secs_f64() * rate as f64) as u64;
            if got > 0 {
                captured = expected;
            } else if expected > captured + ms(30) as u64 {
                let fill = (expected - captured) as usize;
                for o in outs.iter_mut() {
                    o.queue.extend(std::iter::repeat_n(0u8, fill * block));
                }
                if let Some(r) = remote.as_mut().filter(|r| r.live) {
                    r.silence(fill as u32);
                }
                captured = expected;
            }
            if let Some(r) = remote.as_mut() {
                r.flush();
            }
            // ---- Hand it to each copy ----
            let mut dead: Vec<String> = Vec::new();
            for o in outs.iter_mut() {
                let pad = match o.client.GetCurrentPadding() {
                    Ok(p) => p as usize,
                    Err(_) => {
                        dead.push(o.id.clone());
                        continue;
                    }
                };
                // How late this copy runs = waiting here + waiting in the device's
                // buffer. Keep it at its delay: clocks drift a little, sound can
                // go missing, and the delay itself may have been changed.
                let lat = o.queue.len() / block + pad;
                let tol = ms(15);
                if lat > o.delay + tol {
                    let cut = ((lat - o.delay) * block).min(o.queue.len() / block * block);
                    o.queue.drain(..cut);
                } else if lat + tol < o.delay {
                    for _ in 0..(o.delay - lat) * block {
                        o.queue.push_front(0);
                    }
                }
                let qf = o.queue.len() / block;
                let room = (o.buf as usize).saturating_sub(pad).min(fill_target.saturating_sub(pad));
                let n = room.min(qf);
                if n == 0 {
                    continue;
                }
                match o.render.GetBuffer(n as u32) {
                    Ok(p) => {
                        let dst = std::slice::from_raw_parts_mut(p, n * block);
                        for (d, s) in dst.iter_mut().zip(o.queue.drain(..n * block)) {
                            *d = s;
                        }
                        let _ = o.render.ReleaseBuffer(n as u32, 0);
                    }
                    Err(_) => dead.push(o.id.clone()),
                }
            }
            for id in dead {
                util::log(&format!("multi-out: lost {id}"));
                outs.retain(|o| o.id != id);
                retry_at.retain(|(i, _)| i != &id);
                retry_at.push((id, Instant::now()));
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        let _ = cap.Stop();
        for o in &outs {
            let _ = o.client.Stop();
        }
        drop(fmt_g);
        if result.is_err() && RUN.load(Ordering::Acquire) == g {
            // Main output changed or vanished: try again in a moment.
            std::thread::sleep(Duration::from_millis(800));
            if RUN.load(Ordering::Acquire) == g {
                apply();
                return Ok(());
            }
        }
        result
    }
}

/// The copy for the linked PC: sound as it was recorded, silence as a count
/// of frames (so nothing much is sent while nothing plays).
struct Remote {
    id: String,
    tx: crate::link::audio::Tx,
    /// The other PC answered: sound is going there.
    live: bool,
    /// (delay ms, volume) last sent.
    set: (u16, f32),
    buf: Vec<u8>,
    gap: u32,
}

impl Remote {
    fn data(&mut self, src: &[u8]) {
        if self.gap > 0 {
            self.flush();
        }
        self.buf.extend_from_slice(src);
        if self.buf.len() >= 48 * 1024 {
            self.flush();
        }
    }
    fn silence(&mut self, frames: u32) {
        if !self.buf.is_empty() {
            self.flush();
        }
        self.gap = self.gap.saturating_add(frames);
    }
    fn flush(&mut self) {
        if !self.buf.is_empty() {
            self.tx.send(crate::link::audio::Out::Data(std::mem::take(&mut self.buf)));
        }
        if self.gap > 0 {
            self.tx.send(crate::link::audio::Out::Gap(self.gap));
            self.gap = 0;
        }
    }
}

unsafe fn open_out(en: &windows::Win32::Media::Audio::IMMDeviceEnumerator, id: &str, fmt: *const WAVEFORMATEX) -> Result<Out> {
    let dev = en.GetDevice(&HSTRING::from(id))?;
    let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
    // Same format as the recording; Windows converts to what the device wants.
    client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, 2_000_000, 0, fmt, None)?;
    let render: IAudioRenderClient = client.GetService()?;
    let volume: Option<ISimpleAudioVolume> = client.GetService().ok();
    let buf = client.GetBufferSize()?;
    client.Start()?;
    Ok(Out { id: id.to_string(), client, render, volume, buf, queue: VecDeque::new(), delay: 0, vol_set: -1.0 })
}

// ---------------------------------------------------------------------------
// Auto-sync (estimate)
// ---------------------------------------------------------------------------

/// Typical delay of the device itself (ms): Bluetooth speakers buffer a lot.
/// `bt`: outputs the app already knows are Bluetooth (matched to paired devices).
fn device_ms(id: &str, bt: &[String]) -> i32 {
    if is_remote(id) {
        // The network and the other PC's speakers (its own buffer is about
        // what a copy here has, so COPY_MS covers that).
        return 25;
    }
    if bt.iter().any(|b| b == id) || is_bluetooth(id) {
        170
    } else {
        12
    }
}

/// What the copy itself adds: recording packets + the copy's own buffer.
const COPY_MS: i32 = 60;

/// Set every ticked device's delay (`only`: just this one) from estimates.
pub fn auto_sync(only: Option<String>) {
    // Which outputs are Bluetooth, by matching them to the paired devices (here, on the UI thread).
    let d = crate::data();
    let bt: Vec<String> = crate::audio::list_outputs().into_iter().filter(|o| crate::bt::match_endpoint(&o.name, &d.bt).is_some()).map(|o| o.id).collect();
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let Some(main) = main_id() else {
            note(crate::i18n::tr("No sound output"), true);
            return;
        };
        // The slowest of them all should be the main output (it can't be delayed).
        let ticked: Vec<String> = targets().into_iter().filter(|t| t.on && t.id != main).map(|t| t.id).collect();
        let slowest = ticked.iter().filter(|id| !is_remote(id) && device_ms(id, &bt) + COPY_MS > device_ms(&main, &bt)).max_by_key(|id| device_ms(id, &bt)).cloned();
        let main = match slowest {
            Some(id) if only.is_none() || only.as_deref() == Some(id.as_str()) => {
                make_main(&id);
                id
            }
            _ => main,
        };
        let m = device_ms(&main, &bt);
        let mut behind = Vec::new();
        for t in targets().into_iter().filter(|t| t.on && t.id != main && only.as_ref().map(|o| o == &t.id).unwrap_or(true)) {
            let need = m - (COPY_MS + device_ms(&t.id, &bt));
            if need < 0 {
                behind.push((t.id.clone(), -need));
            }
            set_delay(&t.id, need.max(0), true);
        }
        let n = behind.len();
        set_status(|s| {
            s.behind = behind;
            s.msg = if n == 0 { crate::i18n::tr("Estimated — fine-tune with the sliders, or Calibrate with the mic").to_string() } else { String::new() };
            s.bad = false;
        });
        unsafe { CoUninitialize() };
    });
}

// ---------------------------------------------------------------------------
// Calibrate with the microphone
// ---------------------------------------------------------------------------

/// Analysis rate for the chirp search.
const AR: f32 = 16_000.0;
const CHIRP_S: f32 = 0.4;
/// Extra delay given to the device being measured, so its chirp comes clearly after the main one's.
const PUSH_MS: i32 = 700;

/// A short rising tone (1–4 kHz), soft at both ends.
fn chirp(rate: f32) -> Vec<f32> {
    let n = (CHIRP_S * rate) as usize;
    let (f0, f1) = (1000.0f32, 4000.0f32);
    (0..n)
        .map(|i| {
            let t = i as f32 / rate;
            let ph = 2.0 * std::f32::consts::PI * (f0 * t + (f1 - f0) * t * t / (2.0 * CHIRP_S));
            let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos();
            ph.sin() * w
        })
        .collect()
}

/// Correlation of `sig` with the chirp at each offset (analysis rate).
fn correlate(sig: &[f32]) -> Vec<f32> {
    let tpl = chirp(AR);
    if sig.len() <= tpl.len() {
        return Vec::new();
    }
    let n = sig.len() - tpl.len();
    (0..n).map(|k| tpl.iter().zip(&sig[k..]).map(|(a, b)| a * b).sum::<f32>().abs()).collect()
}

/// The strongest chirp in [lo, hi) (seconds from the recording's start), and how strong vs the noise.
fn peak(c: &[f32], lo: f32, hi: f32) -> Option<(f32, f32)> {
    let (a, b) = (((lo * AR) as usize).min(c.len()), ((hi * AR) as usize).min(c.len()));
    if b <= a {
        return None;
    }
    let (k, m) = c[a..b].iter().enumerate().fold((0usize, 0f32), |acc, (i, v)| if *v > acc.1 { (i, *v) } else { acc });
    let mean = c.iter().sum::<f32>() / c.len() as f32;
    Some(((a + k) as f32 / AR, m / mean.max(1e-9)))
}

/// Line up every ticked device (`only`: just this one) using the microphone.
/// A device found slower than the main output becomes the main output, and
/// the others are lined up to it again.
pub fn calibrate(only: Option<String>) {
    if status().busy {
        return;
    }
    set_status(|s| s.busy = true);
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let mut r = calibrate_all(only.clone());
        // Someone was slower than the main output: make the slowest one the
        // main output, then line everybody up to it.
        if r.is_ok() {
            let slow = status().behind.iter().max_by_key(|b| b.1).map(|b| b.0.clone());
            if let Some(id) = slow {
                make_main(&id);
                std::thread::sleep(Duration::from_millis(1500));
                r = calibrate_all(None);
                if r.is_ok() {
                    note(crate::i18n::tr("The slowest speaker is now the main output; the others are lined up to it"), false);
                }
            }
        }
        if let Err(e) = r {
            note(&e, true);
        }
        if let Ok(mut m) = MUTED.lock() {
            *m = None;
        }
        if let Ok(mut e) = EXTRA.lock() {
            e.clear();
        }
        set_status(|s| s.busy = false);
        unsafe { CoUninitialize() };
    });
}

fn calibrate_all(only: Option<String>) -> std::result::Result<(), String> {
    let tr = crate::i18n::tr;
    // (Just after the main output changed, the copying takes a moment to restart.)
    let w = Instant::now();
    while !status().running && w.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(100));
    }
    if !status().running {
        return Err(tr("Turn on Play on several and tick a device first").to_string());
    }
    let main = main_id().ok_or(tr("No sound output"))?;
    if mic_is_bluetooth() {
        return Err(tr("The microphone in use belongs to a Bluetooth speaker — pick another one under Sound input").to_string());
    }
    let list = targets().into_iter().filter(|t| t.on && t.id != main).collect::<Vec<_>>();
    if list.is_empty() {
        return Err(tr("Tick a device to line up first").to_string());
    }
    let todo: Vec<Target> = list.iter().filter(|t| only.as_ref().map(|o| o == &t.id).unwrap_or(true)).cloned().collect();
    if todo.is_empty() {
        return Err(tr("Tick this speaker first (the main output is the one the others follow)").to_string());
    }
    let names = outputs();
    let name = |id: &str| names.iter().find(|o| o.id == id).map(|o| short_name(&names, &o.name)).unwrap_or_default();
    let mut behind = Vec::new();
    for t in &todo {
        note(&format!("{} {}…", tr("Listening for"), name(&t.id)), false);
        // 1. Only the main output plays the chirp.
        if let Ok(mut m) = MUTED.lock() {
            *m = Some(list.iter().map(|x| x.id.clone()).collect());
        }
        std::thread::sleep(Duration::from_millis(250));
        let a = measure(&main)?;
        let main_at = peak(&a, 0.0, 1.0).filter(|p| p.1 > 6.0).ok_or(tr("Couldn't hear the chirp — turn the volume up, or check the microphone"))?.0;
        // 2. This device too, pushed later so the two can't be confused.
        if let Ok(mut m) = MUTED.lock() {
            *m = Some(list.iter().filter(|x| x.id != t.id).map(|x| x.id.clone()).collect());
        }
        if let Ok(mut e) = EXTRA.lock() {
            *e = vec![(t.id.clone(), PUSH_MS)];
        }
        std::thread::sleep(Duration::from_millis(350));
        let b = measure(&main)?;
        if let Ok(mut e) = EXTRA.lock() {
            e.clear();
        }
        // Both chirps from the same recording, so they share one timeline.
        let main_b = peak(&b, (main_at - 0.05).max(0.0), main_at + 0.05).map(|p| p.0).unwrap_or(main_at);
        let cur = target(&t.id).delay;
        let lo = main_b + 0.08;
        let hi = lo + (cur + PUSH_MS + 500) as f32 / 1000.0;
        let dev_at = peak(&b, lo, hi).filter(|p| p.1 > 4.0).ok_or(format!("{} {}", tr("Couldn't hear"), name(&t.id)))?.0;
        // When it plays relative to the main output, without the push.
        let late_ms = ((dev_at - main_b) * 1000.0).round() as i32 - PUSH_MS;
        let want = cur - late_ms;
        util::log(&format!("multi-out: calibrate {}: main {:.0} ms, copy {:.0} ms (pushed), delay {cur} → {want}", t.id, main_at * 1000.0, dev_at * 1000.0));
        if want < 0 {
            behind.push((t.id.clone(), -want));
        }
        set_delay(&t.id, want.max(0), true);
    }
    let n = behind.len();
    set_status(|s| {
        s.behind = behind;
        s.msg = if n == 0 { tr("Lined up with the microphone").to_string() } else { String::new() };
        s.bad = false;
    });
    Ok(())
}

/// Play the chirp on the main output and record the microphone around it.
/// Returns the recording at the analysis rate, starting when the chirp was
/// due to start playing.
fn measure(main: &str) -> std::result::Result<Vec<f32>, String> {
    let tr = crate::i18n::tr;
    unsafe {
        let en = crate::audio::enumerator().ok_or("COM")?;
        // Microphone.
        let mic = en.GetDefaultAudioEndpoint(eCapture, eConsole).map_err(|_| tr("No microphone").to_string())?;
        let mc: IAudioClient = mic.Activate(CLSCTX_ALL, None).map_err(|e| e.message().to_string())?;
        let mfmt_g = Fmt(mc.GetMixFormat().map_err(|e| e.message().to_string())?);
        let mfmt = mfmt_g.0;
        let (mbits, mfloat) = format_kind(mfmt);
        let mrate = (*mfmt).nSamplesPerSec as f32;
        let mch = (*mfmt).nChannels as usize;
        let mblock = (*mfmt).nBlockAlign as usize;
        mc.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 10_000_000, 0, mfmt, None).map_err(|e| e.message().to_string())?;
        let mcc: IAudioCaptureClient = mc.GetService().map_err(|e| e.message().to_string())?;
        // Main output.
        let dev = en.GetDevice(&HSTRING::from(main)).map_err(|e| e.message().to_string())?;
        let rc: IAudioClient = dev.Activate(CLSCTX_ALL, None).map_err(|e| e.message().to_string())?;
        let rfmt_g = Fmt(rc.GetMixFormat().map_err(|e| e.message().to_string())?);
        let rfmt = rfmt_g.0;
        if !matches!(format_kind(rfmt), (32, true) | (16, _)) {
            return Err("unsupported sound format".into());
        }
        let (rbits, rfloat) = format_kind(rfmt);
        let rrate = (*rfmt).nSamplesPerSec as f32;
        let rch = (*rfmt).nChannels as usize;
        rc.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 20_000_000, 0, rfmt, None).map_err(|e| e.message().to_string())?;
        let rr: IAudioRenderClient = rc.GetService().map_err(|e| e.message().to_string())?;
        mc.Start().map_err(|e| e.message().to_string())?;
        rc.Start().map_err(|e| e.message().to_string())?;
        // Record a little first (the microphone takes a moment to start).
        let mut samples: Vec<f32> = Vec::new();
        let mut t_first: Option<i64> = None;
        let grab = |samples: &mut Vec<f32>, t_first: &mut Option<i64>| {
            while let Ok(n) = mcc.GetNextPacketSize() {
                if n == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let (mut frames, mut flags, mut qpc) = (0u32, 0u32, 0u64);
                if mcc.GetBuffer(&mut data, &mut frames, &mut flags, None, Some(&mut qpc)).is_err() {
                    break;
                }
                if t_first.is_none() {
                    *t_first = Some(qpc as i64);
                }
                let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null();
                for f in 0..frames as usize {
                    let v = if silent {
                        0.0
                    } else {
                        let p = data.add(f * mblock);
                        // First channel is enough.
                        match (mbits, mfloat) {
                            (32, true) => *(p as *const f32),
                            (16, _) => *(p as *const i16) as f32 / 32768.0,
                            (32, false) => *(p as *const i32) as f32 / 2147483648.0,
                            _ => 0.0,
                        }
                    };
                    samples.push(v);
                }
                let _ = mcc.ReleaseBuffer(frames);
                let _ = mch;
            }
        };
        let w0 = Instant::now();
        while w0.elapsed() < Duration::from_millis(300) {
            grab(&mut samples, &mut t_first);
            std::thread::sleep(Duration::from_millis(5));
        }
        // The chirp: when it will start playing = now + what's already queued.
        let tone = chirp(rrate);
        let pad = rc.GetCurrentPadding().unwrap_or(0) as i64;
        let due = qpc_100ns() + pad * 10_000_000 / rrate as i64;
        // Fed in pieces as room frees up (the tone may be longer than the buffer).
        let rbuf = rc.GetBufferSize().map_err(|e| e.message().to_string())? as usize;
        let mut done = 0usize;
        let w_feed = Instant::now();
        while done < tone.len() && w_feed.elapsed() < Duration::from_secs(3) {
            let pad = rc.GetCurrentPadding().unwrap_or(rbuf as u32) as usize;
            let n = rbuf.saturating_sub(pad).min(tone.len() - done);
            if n > 0 {
                let p = rr.GetBuffer(n as u32).map_err(|e| e.message().to_string())?;
                for i in 0..n {
                    let v = tone[done + i] * 0.6;
                    for c in 0..rch {
                        match (rbits, rfloat) {
                            (32, true) => *(p as *mut f32).add(i * rch + c) = v,
                            (16, _) => *(p as *mut i16).add(i * rch + c) = (v * 32767.0) as i16,
                            _ => {}
                        }
                    }
                }
                let _ = rr.ReleaseBuffer(n as u32, 0);
                done += n;
            }
            grab(&mut samples, &mut t_first);
            std::thread::sleep(Duration::from_millis(5));
        }
        let w1 = Instant::now();
        while w1.elapsed() < Duration::from_millis(3400) {
            grab(&mut samples, &mut t_first);
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = mc.Stop();
        let _ = rc.Stop();
        let t_first = t_first.ok_or(tr("The microphone sent nothing"))?;
        // Skip to when the chirp was due, then resample to the analysis rate.
        let skip = (((due - t_first) as f64 / 1e7) * mrate as f64).max(0.0) as usize;
        let sig = &samples[skip.min(samples.len())..];
        let n_out = (sig.len() as f32 * AR / mrate) as usize;
        Ok((0..n_out)
            .map(|i| {
                let x = i as f32 * mrate / AR;
                let k = x as usize;
                let f = x - k as f32;
                let a = sig.get(k).copied().unwrap_or(0.0);
                let b = sig.get(k + 1).copied().unwrap_or(a);
                a + (b - a) * f
            })
            .collect::<Vec<f32>>())
        .map(|v| correlate(&v))
    }
}

// ---------------------------------------------------------------------------
// The main output, each speaker's own volume, and the cluster's volume keys
// ---------------------------------------------------------------------------

/// Make `id` the main output (Windows' default). The old main one joins the
/// ticked devices so it keeps playing.
pub fn make_main(id: &str) {
    // (Apps here can only play to a device of this PC.)
    if is_remote(id) {
        return;
    }
    let old = main_id();
    if old.as_deref() == Some(id) {
        return;
    }
    if let Some(o) = &old {
        edit(o, |t| t.on = true);
    }
    edit(id, |t| t.on = false);
    save();
    if !crate::audio::set_default_output(id) {
        util::log("multi-out: couldn't change the main output");
    }
    apply();
}

fn endpoint_volume(id: &str) -> Option<windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume> {
    unsafe {
        let en = crate::audio::enumerator()?;
        let dev = en.GetDevice(&HSTRING::from(id)).ok()?;
        dev.Activate(CLSCTX_ALL, None).ok()
    }
}

/// A speaker's own volume (0…1) and mute.
pub fn dev_volume(id: &str) -> Option<(f32, bool)> {
    if is_remote(id) {
        return Some((target(id).vol.clamp(0.0, 1.0), REMOTE_MUTE.load(Ordering::Relaxed)));
    }
    let v = endpoint_volume(id)?;
    unsafe { Some((v.GetMasterVolumeLevelScalar().ok()?, v.GetMute().map(|m| m.as_bool()).unwrap_or(false))) }
}

/// Main volume we set ourselves (so the change notice isn't applied twice).
static OWN_MAIN: Mutex<Option<(f32, Instant)>> = Mutex::new(None);

pub fn set_dev_volume(id: &str, v: f32) {
    let v = v.clamp(0.0, 1.0);
    if is_remote(id) {
        // The engine passes it on (and saves it once the slider rests).
        edit(id, |t| t.vol = v);
        if let Ok(mut m) = VOL_MOVED.lock() {
            *m = Some(Instant::now());
        }
        return;
    }
    if main_id().as_deref() == Some(id) {
        if let Ok(mut o) = OWN_MAIN.lock() {
            *o = Some((v, Instant::now()));
        }
    }
    if let Some(e) = endpoint_volume(id) {
        unsafe {
            let _ = e.SetMasterVolumeLevelScalar(v, std::ptr::null());
        }
    }
}

/// Was this main-volume change one we just made?
pub fn own_main_change(v: f32) -> bool {
    OWN_MAIN.lock().ok().and_then(|o| *o).map(|(x, t)| (x - v).abs() < 0.006 && t.elapsed() < Duration::from_millis(800)).unwrap_or(false)
}

/// The speakers playing together: the main output first, then the ticked
/// ones that are connected (id, short name). Empty when the feature is off.
pub fn cluster() -> Vec<(String, String)> {
    if !config::cur().b("mo_on") {
        return Vec::new();
    }
    let outs = outputs();
    let mut v: Vec<(String, String)> = outs.iter().filter(|o| o.default).map(|o| (o.id.clone(), short_name(&outs, &o.name))).collect();
    for t in targets().into_iter().filter(|t| t.on) {
        if let Some(o) = outs.iter().find(|o| o.id == t.id && !o.default) {
            v.push((o.id.clone(), short_name(&outs, &o.name)));
        }
    }
    if v.len() < 2 {
        return Vec::new();
    }
    v
}

/// A device's short name ("JBL Flip 6" from "Speakers (JBL Flip 6)"); when two
/// share it (two Realtek outputs), the endpoint's own name instead ("Speakers").
pub fn short_name(outs: &[crate::audio::Output], name: &str) -> String {
    let inner = crate::bt::inner(name);
    let twins = outs.iter().filter(|o| crate::bt::inner(&o.name) == inner).count();
    if twins > 1 {
        if let Some(a) = name.find(" (") {
            return name[..a].trim().to_string();
        }
    }
    inner.to_string()
}

/// Volume up / down for all of them by the step in Settings (each keeps its
/// own level: 55 / 65 / 40 → 58 / 68 / 43). Returns (name, level) for the alert.
pub fn cluster_step(dir: i32) -> Vec<(String, f32)> {
    let step = config::cur().i("mo_step").clamp(1, 10) as f32 / 100.0;
    // Like Windows: the volume keys unmute.
    for (id, _) in cluster() {
        if dev_volume(&id).map(|v| v.1).unwrap_or(false) {
            set_dev_mute(&id, false);
        }
    }
    cluster_shift(step * dir as f32, true)
}

/// Move every speaker of the cluster by `delta`; `with_main`: the main one too
/// (false when it already moved, e.g. Windows' own volume slider).
pub fn cluster_shift(delta: f32, with_main: bool) -> Vec<(String, f32)> {
    let c = cluster();
    let mut out = Vec::new();
    for (i, (id, name)) in c.iter().enumerate() {
        let cur = dev_volume(id).map(|v| v.0).unwrap_or(0.0);
        let v = if i == 0 && !with_main { cur } else { (cur + delta).clamp(0.0, 1.0) };
        if i > 0 || with_main {
            set_dev_volume(id, v);
        }
        out.push((name.clone(), v));
    }
    out
}


/// (volume, muted) of each of these speakers.
pub fn levels(ids: &[String]) -> Vec<(f32, bool)> {
    ids.iter().map(|id| dev_volume(id).unwrap_or((0.0, false))).collect()
}

/// The group's volume: the average of its speakers' levels (muted when all are).
pub fn group_level(l: &[(f32, bool)]) -> (f32, bool) {
    if l.is_empty() {
        return (0.0, false);
    }
    (l.iter().map(|x| x.0).sum::<f32>() / l.len() as f32, l.iter().all(|x| x.1))
}

/// New levels for the group set to `v`, from the levels `base` it had when
/// the slider was grabbed: below their average every speaker scales down
/// toward 0, above it up toward 100 % — the average lands on `v`, each
/// keeps its place, and both ends reach every speaker.
pub fn group_target(base: &[f32], v: f32) -> Vec<f32> {
    let n = base.len().max(1) as f32;
    let avg = base.iter().sum::<f32>() / n;
    let v = v.clamp(0.0, 1.0);
    base.iter()
        .map(|&d| {
            if avg <= 0.0005 {
                v
            } else if v <= avg {
                d * v / avg
            } else if avg >= 0.9995 {
                d
            } else {
                d + (1.0 - d) * (v - avg) / (1.0 - avg)
            }
            .clamp(0.0, 1.0)
        })
        .collect()
}

/// Mute / unmute one speaker.
pub fn set_dev_mute(id: &str, m: bool) {
    if is_remote(id) {
        REMOTE_MUTE.store(m, Ordering::Relaxed);
        return;
    }
    if let Some(e) = endpoint_volume(id) {
        unsafe {
            let _ = e.SetMute(m, std::ptr::null());
        }
    }
}

/// Microphone in use for calibration is a Bluetooth one (it would switch that
/// speaker to call quality while listening).
pub fn mic_is_bluetooth() -> bool {
    crate::audio::list_inputs().iter().find(|i| i.default).map(|i| is_bluetooth(&i.id) || i.name.to_lowercase().contains("hands-free")).unwrap_or(false)
}
