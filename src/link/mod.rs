//! Link: one mouse and keyboard across your PCs (each runs Phemio).
//!
//! The PCs find each other on the local network (a small UDP "here I am"
//! every couple of seconds, also answered directly), pair once with a
//! 6-digit code you compare on both screens, then keep an encrypted TCP
//! connection open while both are on. Every screen of both PCs sits on one
//! shared map (Island → Link page); see `geom` for how crossings work,
//! `proto` for what's said, `input` for the Windows hooks.

pub mod audio;
pub mod clip;
pub mod firewall;
pub mod geom;
pub mod identify;
pub mod input;
pub mod proto;

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use proto::{Msg, ScreenInfo};

const UDP_PORT: u16 = 47830;
const TCP_PORT: u16 = 47831;

/// Developer aid (two copies on one PC): ISLET_LINK_PORTS="udp,tcp,peer_udp".
fn ports() -> (u16, u16, u16) {
    std::env::var("ISLET_LINK_PORTS")
        .ok()
        .and_then(|v| {
            let p: Vec<u16> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            (p.len() == 3).then(|| (p[0], p[1], p[2]))
        })
        .unwrap_or((UDP_PORT, TCP_PORT, UDP_PORT))
}
const BEACON: &str = "MARISLAND-LINK1";

#[derive(Clone, Debug)]
pub struct Peer {
    pub id: u64,
    pub name: String,
    pub addr: SocketAddr,
    pub seen: Instant,
}

#[derive(Clone, Debug)]
pub struct Paired {
    pub id: u64,
    pub name: String,
    secret: [u8; 32],
    /// Where it was last seen: asked directly every round, so finding it
    /// doesn't depend on broadcasts getting through.
    last: Option<std::net::Ipv4Addr>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Pairing {
    None,
    /// Waiting for the other PC to answer.
    Asking { name: String },
    /// Compare the codes; `mine` = what you said here.
    Code { id: u64, name: String, code: u32, mine: Option<bool> },
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Session {
    pub id: u64,
    pub name: String,
    pub screens: Vec<ScreenInfo>,
    /// Recent round trips (ms) and when each was taken.
    pub rtt: VecDeque<(Instant, f32)>,
    tx: Sender<Msg>,
    /// Where the other PC is (the far end of the connection).
    remote: Option<SocketAddr>,
    /// Where it takes calls, when we called it.
    call: Option<SocketAddr>,
}

/// Something only the window's thread can do (clipboard, windows, the shelf).
#[derive(Clone, Debug)]
pub enum UiJob {
    Clip(clip::Clip),
    /// Show the screen numbers here.
    Identify,
    /// Make this display the main one (asked from the other PC).
    MainScreen(String),
    /// Change a display's settings (asked from the other PC): op, a, b, c.
    DispSet(String, u8, i32, i32, i32),
    /// Files from the other PC's shelf landed here (paths, what to call them).
    Shelf(Vec<String>, String),
    /// A short notice on the island (title, details, progress).
    Peek(String, String, Option<f32>),
    /// Developer aid (ISLET_DEMO_CLIP).
    Demo,
}

#[derive(Clone, Debug)]
pub struct Xfer {
    /// Sending (or receiving).
    pub out: bool,
    pub what: String,
    pub done: u64,
    pub total: u64,
    pub started: Instant,
}

impl Session {
    /// (latest, worst over the last 30 s) round trip, ms.
    pub fn delay(&self) -> Option<(f32, f32)> {
        let last = self.rtt.back()?.1;
        let worst = self.rtt.iter().filter(|(t, _)| t.elapsed() < Duration::from_secs(30)).map(|r| r.1).fold(0.0, f32::max);
        Some((last, worst))
    }
}

#[derive(Default)]
struct Layout {
    version: u64,
    places: HashMap<String, (f32, f32)>,
}

struct State {
    me: u64,
    name: String,
    peers: Vec<Peer>,
    paired: Vec<Paired>,
    pairing: Pairing,
    /// Your answer for the pairing being shown.
    decide: Option<Sender<bool>>,
    session: Option<Session>,
    layout: Layout,
    local: Vec<ScreenInfo>,
    started: bool,
    #[allow(dead_code)]
    host: isize,
    #[allow(dead_code)]
    msg: u32,
    /// Something went wrong (shown on the page).
    problem: String,
    /// You pressed Disconnect: don't call, don't take calls, until Connect.
    hold: bool,
    /// The other PC hung up: don't call it (it calls when it wants to).
    quiet: bool,
    /// The address you typed (Connect by address).
    manual: String,
    xfer: Option<Xfer>,
    /// How the last transfer went (shown for a while).
    xfer_msg: Option<(String, Instant)>,
    /// What's on the other PC's shelf, what's in each item, and which of
    /// them are already here.
    remote_shelf: Vec<(u64, String, u64, bool)>,
    /// The other PC's displays' settings, as it last told us (and when).
    remote_disp: HashMap<String, (crate::display::Info, u64, std::time::Instant)>,
    remote_hashes: Vec<(u64, u64)>,
    have: std::collections::HashSet<u64>,
    jobs: Vec<UiJob>,
}

static ST: Mutex<Option<State>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut State) -> T) -> Option<T> {
    let mut g = ST.lock().unwrap_or_else(|e| e.into_inner());
    g.as_mut().map(f)
}

/// Run `f`; a panic in it is logged instead of taking the app down.
pub(crate) fn guard<T>(what: &str, f: impl FnOnce() -> T) -> Option<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => Some(v),
        Err(e) => {
            let m = e.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_default();
            crate::util::log(&format!("link: {what} stopped: {m}"));
            None
        }
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    let n = name.to_string();
    let _ = std::thread::Builder::new().name(format!("link-{name}")).spawn(move || {
        guard(&n, f);
    });
}

/// Linked right now (an atomic, so the mouse hook can ask without waiting).
static LINKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn connected() -> bool {
    LINKED.load(std::sync::atomic::Ordering::SeqCst)
}

/// Send to the PC we're linked with (if we are).
fn send(m: Msg) {
    if let Some(tx) = with(|s| s.session.as_ref().map(|x| x.tx.clone())).flatten() {
        let _ = tx.send(m);
    }
}

fn me() -> (u64, String) {
    with(|s| (s.me, s.name.clone())).unwrap_or_default()
}

fn peer_name() -> String {
    with(|s| s.session.as_ref().map(|x| x.name.clone())).flatten().unwrap_or_default()
}

/// Where to call the linked PC (for files), and its id.
fn session_addr() -> Option<(SocketAddr, u64)> {
    with(|s| {
        let x = s.session.as_ref()?;
        let a = x.call.or_else(|| s.peers.iter().find(|p| p.id == x.id).map(|p| p.addr)).or_else(|| x.remote.map(|r| SocketAddr::new(r.ip(), ports().1)))?;
        Some((a, x.id))
    })
    .flatten()
}

fn ui_job(j: UiJob) {
    with(|s| s.jobs.push(j));
    tell_ui();
}

/// What the window's thread has to do (called on WM_LINK).
pub fn take_jobs() -> Vec<UiJob> {
    UI_PENDING.store(false, std::sync::atomic::Ordering::SeqCst);
    with(|s| std::mem::take(&mut s.jobs)).unwrap_or_default()
}

fn set_xfer(out: bool, what: &str, done: u64, total: u64, started: Instant) {
    let first = with(|s| {
        let first = s.xfer.is_none();
        s.xfer = Some(Xfer { out, what: what.to_string(), done, total, started });
        first
    })
    .unwrap_or(false);
    if first && !out {
        let peer = peer_name();
        ui_job(UiJob::Peek(format!("{} {peer}", crate::i18n::tr("Receiving from")), what.to_string(), Some(0.0)));
    }
    tell_ui();
}

fn set_have(have: std::collections::HashSet<u64>) {
    with(|s| s.have = have);
    tell_ui();
}

/// Look again at which of the other PC's shelf items are here.
fn recheck_have() {
    if let Some((items, hashes)) = with(|s| (s.remote_shelf.iter().map(|x| (x.0, x.1.clone())).collect::<Vec<_>>(), s.remote_hashes.clone())) {
        clip::check_have(items, hashes);
    }
}

/// The shelf folder setting changed: look again.
pub fn shelf_dir_changed() {
    recheck_have();
}

fn set_xfer_done(msg: String) {
    with(|s| {
        s.xfer = None;
        s.xfer_msg = (!msg.is_empty()).then(|| (msg.clone(), Instant::now()));
    });
    tell_ui();
}

/// Where to tell the window (set at start; read without any lock, so the
/// mouse hook never waits).
static HOST: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static HOST_MSG: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// A WM_LINK is already waiting in the window's queue: no need for another.
/// (One at a time keeps the queue clear, so the island's animations and the
/// settings window never starve behind a pile of Link messages.)
static UI_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn tell_ui() {
    use std::sync::atomic::Ordering::SeqCst;
    let h = HOST.load(SeqCst);
    if h == 0 || UI_PENDING.swap(true, SeqCst) {
        return;
    }
    unsafe {
        let ok = windows::Win32::UI::WindowsAndMessaging::PostMessageW(Some(windows::Win32::Foundation::HWND(h as *mut _)), HOST_MSG.load(SeqCst), windows::Win32::Foundation::WPARAM(0), windows::Win32::Foundation::LPARAM(0));
        if ok.is_err() {
            UI_PENDING.store(false, SeqCst);
        }
    }
}

fn now_us() -> u64 {
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_micros() as u64
}

// ---------------------------------------------------------------------------
// What the page shows
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct View {
    pub me: u64,
    pub name: String,
    /// PCs seen on the network that aren't paired.
    pub nearby: Vec<(u64, String)>,
    pub paired: Vec<(u64, String)>,
    pub pairing: Pairing,
    /// (name, latest round trip, worst) while connected.
    pub session: Option<(String, Option<(f32, f32)>)>,
    pub geo: geom::Geo,
    /// The pointer on the map (and whether it's on the other PC).
    pub pointer: Option<((f32, f32), bool)>,
    pub problem: String,
    /// You pressed Disconnect.
    pub hold: bool,
    /// The other PC hung up.
    pub quiet: bool,
    pub manual: String,
    /// The address of the linked (or last seen paired) PC, for Remote Desktop.
    pub peer_ip: Option<String>,
    pub xfer: Option<Xfer>,
    pub xfer_msg: Option<String>,
    /// What's on the other PC's shelf, and which of it is already here.
    pub remote_shelf: Vec<(u64, String, u64, bool)>,
    pub have: std::collections::HashSet<u64>,
}

pub fn view() -> Option<View> {
    let mut v = with(|s| {
        let paired_ids: Vec<u64> = s.paired.iter().map(|p| p.id).collect();
        View {
            me: s.me,
            name: s.name.clone(),
            nearby: s.peers.iter().filter(|p| !paired_ids.contains(&p.id) && p.seen.elapsed() < Duration::from_secs(10)).map(|p| (p.id, p.name.clone())).collect(),
            paired: s.paired.iter().map(|p| (p.id, p.name.clone())).collect(),
            pairing: s.pairing.clone(),
            session: s.session.as_ref().map(|x| (x.name.clone(), x.delay())),
            geo: build_geo(s),
            pointer: None,
            problem: s.problem.clone(),
            hold: s.hold,
            quiet: s.quiet,
            manual: s.manual.clone(),
            peer_ip: peer_ip(s),
            xfer: s.xfer.clone(),
            xfer_msg: s.xfer_msg.as_ref().filter(|m| m.1.elapsed() < Duration::from_secs(12)).map(|m| m.0.clone()),
            remote_shelf: if s.session.is_some() { s.remote_shelf.clone() } else { vec![] },
            have: s.have.clone(),
        }
    })?;
    v.pointer = match input::captured() {
        Some((_, p)) => Some((p, true)),
        None => {
            let mut pt = windows::Win32::Foundation::POINT::default();
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt);
            }
            v.geo.local_at_px(pt.x, pt.y).and_then(|i| v.geo.screens.get(i)).map(|s| (s.to_g(pt.x, pt.y), false))
        }
    };
    Some(v)
}

fn peer_ip(s: &State) -> Option<String> {
    if let Some(x) = &s.session {
        if let Some(a) = x.remote.or(x.call) {
            return Some(a.ip().to_string());
        }
    }
    let ids: Vec<u64> = s.paired.iter().map(|p| p.id).collect();
    if let Some(p) = s.peers.iter().filter(|p| ids.contains(&p.id)).max_by_key(|p| p.seen) {
        return Some(p.addr.ip().to_string());
    }
    let m = s.manual.trim();
    (!m.is_empty()).then(|| m.rsplit_once(':').filter(|(h, _)| !h.contains(':')).map(|(h, _)| h.to_string()).unwrap_or_else(|| m.to_string()))
}

// ---------------------------------------------------------------------------
// The map
// ---------------------------------------------------------------------------

fn build_geo(s: &mut State) -> geom::Geo {
    let mut g = geom::Geo { me: s.me, screens: Vec::new() };
    let add = |g: &mut geom::Geo, m: u64, v: &[ScreenInfo]| {
        for i in v {
            g.screens.push(geom::Screen { machine: m, key: i.key.clone(), label: i.label.clone(), px: (i.x, i.y, i.w, i.h), scale: i.scale.max(0.5), g: (0.0, 0.0) });
        }
    };
    add(&mut g, s.me, &s.local);
    // Not linked: the map is just this PC's screens, exactly as Windows has
    // them (Settings → Display), in proportion to their pixels — and moving
    // one there moves it in Windows (see `arrange_local`).
    if s.session.is_none() {
        let k = g.screens.iter().map(|sc| sc.scale).fold(1.0f32, f32::max);
        for sc in g.screens.iter_mut() {
            sc.scale = k;
            sc.g = (sc.px.0 as f32 / k, sc.px.1 as f32 / k);
        }
        return g;
    }
    if let Some(x) = &s.session {
        add(&mut g, x.id, &x.screens);
    }
    let placed: Vec<bool> = g.screens.iter().map(|sc| s.layout.places.contains_key(&sc.uid())).collect();
    for sc in g.screens.iter_mut() {
        if let Some(p) = s.layout.places.get(&sc.uid()) {
            sc.g = *p;
        }
    }
    if placed.iter().any(|p| !p) {
        g.auto_place(&placed);
        for sc in &g.screens {
            s.layout.places.entry(sc.uid()).or_insert(sc.g);
        }
    }
    g
}

fn push_geo() {
    if let Some(g) = with(build_geo) {
        input::set_geo(g);
    }
}

/// Not linked to another PC right now?
pub fn alone() -> bool {
    with(|s| s.session.is_none()).unwrap_or(true)
}

/// Not linked: one of this PC's screens dropped somewhere new on the map —
/// arrange Windows' displays the same way (as Settings → Display does).
/// The main screen stays at the desktop's origin; screens that touch on the
/// map touch exactly in pixels.
pub fn arrange_local(uid: &str, g: (f32, f32)) -> Result<(), String> {
    let Some(geo) = with(build_geo) else { return Err("Link isn't running".into()) };
    let mut sc: Vec<geom::Screen> = geo.screens.into_iter().filter(|x| x.machine == geo.me).collect();
    let Some(i) = sc.iter().position(|x| x.uid() == uid) else { return Err("screen not found".into()) };
    sc[i].g = g;
    let k = sc.first().map(|x| x.scale).unwrap_or(1.0);
    // Pixels from the map (same scale for all of them here).
    let mut pos: Vec<(i32, i32)> = sc.iter().map(|x| ((x.g.0 * k).round() as i32, (x.g.1 * k).round() as i32)).collect();
    let size: Vec<(i32, i32)> = sc.iter().map(|x| (x.px.2, x.px.3)).collect();
    // Edges the map has touching: exactly touching (rounding can leave a pixel).
    let tol = (geom::TOUCH * k).ceil() as i32 + 1;
    for _ in 0..3 {
        for a in 0..sc.len() {
            for b in 0..sc.len() {
                if a == b {
                    continue;
                }
                let (ax, ay) = pos[a];
                let (aw, ah) = size[a];
                let (bx, by) = pos[b];
                if b == i || a != i {
                    // Move b to sit flush against a.
                    if (bx - (ax + aw)).abs() <= tol {
                        pos[b].0 = ax + aw;
                    } else if ((bx + size[b].0) - ax).abs() <= tol {
                        pos[b].0 = ax - size[b].0;
                    }
                    if (by - (ay + ah)).abs() <= tol {
                        pos[b].1 = ay + ah;
                    } else if ((by + size[b].1) - ay).abs() <= tol {
                        pos[b].1 = ay - size[b].1;
                    }
                    if (bx - ax).abs() <= tol {
                        pos[b].0 = ax;
                    }
                    if (by - ay).abs() <= tol {
                        pos[b].1 = ay;
                    }
                }
            }
        }
    }
    // The main screen (at 0,0 now) stays the origin.
    let main = sc.iter().position(|x| x.px.0 == 0 && x.px.1 == 0).unwrap_or(0);
    let (ox, oy) = pos[main];
    for p in pos.iter_mut() {
        p.0 -= ox;
        p.1 -= oy;
    }
    let list: Vec<(String, i32, i32)> = sc.iter().zip(pos.iter()).map(|(x, p)| (x.key.clone(), p.0, p.1)).collect();
    crate::display::arrange(&list)?;
    // The linked map lays this PC's screens out afresh from the new
    // arrangement next time.
    with(|s| {
        let mine = format!("{:x}/", s.me);
        s.layout.places.retain(|k, _| !k.starts_with(&mine));
        save(s);
    });
    crate::util::log(&format!("link: displays arranged like the map: {:?}", sc.iter().map(|x| x.key.clone()).zip(pos.iter().cloned()).collect::<Vec<_>>()));
    refresh_screens();
    Ok(())
}

/// Make display `key` this PC's main screen (taskbar, new windows and the
/// sign-in screen go there): everything shifts so it sits at 0, 0, the
/// arrangement otherwise unchanged.
pub fn set_main_local(key: &str) -> Result<(), String> {
    let now = local_screens();
    let Some(m) = now.iter().find(|s| s.key == key) else { return Err(format!("{key} isn't connected")) };
    let (ox, oy) = (m.x, m.y);
    if (ox, oy) == (0, 0) {
        return Ok(());
    }
    let list: Vec<(String, i32, i32)> = now.iter().map(|s| (s.key.clone(), s.x - ox, s.y - oy)).collect();
    crate::display::arrange(&list)?;
    crate::util::log(&format!("link: {key} is the main screen now"));
    with(|s| {
        let mine = format!("{:x}/", s.me);
        s.layout.places.retain(|k, _| !k.starts_with(&mine));
        save(s);
    });
    refresh_screens();
    Ok(())
}

/// The other PC's display `key`: its settings as last heard (asking again
/// when that's more than a second old), and seconds left on a change.
pub fn remote_display(key: &str) -> Option<(crate::display::Info, u64)> {
    let have = with(|s| s.remote_disp.get(key).cloned()).flatten();
    if have.as_ref().map(|h| h.2.elapsed().as_millis() > 1000).unwrap_or(true) {
        send(Msg::DispAsk { key: key.to_string() });
    }
    have.map(|h| (h.0, h.1))
}

/// Change one of the other PC's displays (see Msg::DispSet).
pub fn remote_display_set(key: &str, op: u8, a: i32, b: i32, c: i32) {
    send(Msg::DispSet { key: key.to_string(), op, a, b, c });
    // (Its new settings come back with the next answer.)
    with(|s| {
        s.remote_disp.remove(key);
    });
    send(Msg::DispAsk { key: key.to_string() });
}

/// Make a screen of either PC its PC's main screen (the other PC's: asked
/// over the link).
pub fn set_main(machine: u64, key: &str) {
    let me = with(|s| s.me).unwrap_or(0);
    if machine == me {
        if let Err(e) = set_main_local(key) {
            crate::util::log(&format!("link: couldn't change the main screen: {e}"));
        }
    } else {
        send(Msg::MainScreen { key: key.to_string() });
    }
}

/// A screen dropped somewhere new on the map (already snapped).
pub fn place(uid: &str, g: (f32, f32)) {
    let msg = with(|s| {
        s.layout.places.insert(uid.to_string(), g);
        s.layout.version = (s.layout.version + 1).max(unix_ms());
        save(s);
        let m = Msg::Layout { version: s.layout.version, places: s.layout.places.iter().map(|(k, v)| (k.clone(), v.0, v.1)).collect() };
        s.session.as_ref().map(|x| (x.tx.clone(), m))
    })
    .flatten();
    if let Some((tx, m)) = msg {
        let _ = tx.send(m);
    }
    push_geo();
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Saved: this PC's id, pairings (secrets protected by Windows) and the map
// ---------------------------------------------------------------------------

fn path() -> std::path::PathBuf {
    crate::config::dir().join("link.json")
}

fn protect(data: &[u8], unprotect: bool) -> Option<Vec<u8>> {
    use windows::Win32::Security::Cryptography::*;
    unsafe {
        let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB::default();
        let ok = if unprotect {
            CryptUnprotectData(&input, None, None, None, None, 0, &mut out)
        } else {
            CryptProtectData(&input, windows::core::w!("Phemio Link"), None, None, None, 0, &mut out)
        };
        ok.ok()?;
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(out.pbData as *mut _)));
        Some(v)
    }
}

fn load() -> (u64, Vec<Paired>, Layout, String) {
    let v: serde_json::Value = std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(serde_json::Value::Null);
    let me = v["me"].as_str().and_then(|s| u64::from_str_radix(s, 16).ok()).unwrap_or_else(|| u64::from_le_bytes(proto::random::<8>()) | 1);
    let mut paired = Vec::new();
    for p in v["paired"].as_array().cloned().unwrap_or_default() {
        let id = p["id"].as_str().and_then(|s| u64::from_str_radix(s, 16).ok());
        let secret = p["secret"].as_str().and_then(crate::answers::b64_decode).and_then(|b| protect(&b, true)).filter(|b| b.len() == 32);
        if let (Some(id), Some(sec)) = (id, secret) {
            let last = p["last"].as_str().and_then(|a| a.parse().ok());
            paired.push(Paired { id, name: p["name"].as_str().unwrap_or("").to_string(), secret: sec.try_into().unwrap(), last });
        }
    }
    let mut layout = Layout { version: v["layout"]["version"].as_u64().unwrap_or(0), places: HashMap::new() };
    if let Some(o) = v["layout"]["places"].as_object() {
        for (k, p) in o {
            if let (Some(x), Some(y)) = (p[0].as_f64(), p[1].as_f64()) {
                layout.places.insert(k.clone(), (x as f32, y as f32));
            }
        }
    }
    (me, paired, layout, v["manual"].as_str().unwrap_or("").to_string())
}

fn save(s: &State) {
    let paired: Vec<serde_json::Value> = s
        .paired
        .iter()
        .filter_map(|p| {
            protect(&p.secret, false).map(|b| serde_json::json!({"id": format!("{:x}", p.id), "name": p.name, "secret": crate::answers::b64_encode(&b), "last": p.last.map(|a| a.to_string()).unwrap_or_default()}))
        })
        .collect();
    let places: serde_json::Map<String, serde_json::Value> = s.layout.places.iter().map(|(k, v)| (k.clone(), serde_json::json!([v.0, v.1]))).collect();
    let v = serde_json::json!({"me": format!("{:x}", s.me), "paired": paired, "layout": {"version": s.layout.version, "places": places}, "manual": s.manual});
    let tmp = path().with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec_pretty(&v).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, path());
    }
}

// ---------------------------------------------------------------------------
// This PC's screens
// ---------------------------------------------------------------------------

pub fn local_screens() -> Vec<ScreenInfo> {
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::UI::HiDpi::*;
    let mut out: Vec<ScreenInfo> = Vec::new();
    unsafe extern "system" fn each(hm: HMONITOR, _: HDC, _: *mut windows::Win32::Foundation::RECT, lp: windows::Win32::Foundation::LPARAM) -> windows::core::BOOL {
        let out = &mut *(lp.0 as *mut Vec<ScreenInfo>);
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(hm, &mut mi as *mut _ as *mut MONITORINFO).as_bool() {
            let r = mi.monitorInfo.rcMonitor;
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = GetDpiForMonitor(hm, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            let key = crate::util::from_wide(&mi.szDevice);
            // The monitor's own name ("DELL U2723QE"), when Windows knows it.
            let mut dd = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
            let dev = crate::util::wide(&key);
            let mut label = String::new();
            if EnumDisplayDevicesW(windows::core::PCWSTR(dev.as_ptr()), 0, &mut dd, 0).as_bool() {
                label = crate::util::from_wide(&dd.DeviceString);
            }
            let primary = mi.monitorInfo.dwFlags & 1 != 0;
            if label.is_empty() || label.contains("Generic") {
                label = if primary { "Main screen".into() } else { format!("Screen {}", out.len() + 1) };
            }
            out.push(ScreenInfo { key, label, x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top, scale: dx as f32 / 96.0 });
        }
        windows::core::BOOL(1)
    }
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), windows::Win32::Foundation::LPARAM(&mut out as *mut _ as isize));
    }
    out
}

fn computer_name() -> String {
    // (ISLET_PCNAME: developer aid for two copies on one PC.)
    std::env::var("ISLET_PCNAME").or_else(|_| std::env::var("COMPUTERNAME")).unwrap_or_else(|_| "This PC".into())
}

// ---------------------------------------------------------------------------
// Start / stop
// ---------------------------------------------------------------------------

/// Start Link (Settings → General → Link is on). `msg` reaches `host` when
/// the page should redraw (and to show a pairing request).
pub fn start(host: isize, msg: u32) {
    input::show_pointer_now();
    let fresh = {
        let mut g = ST.lock().unwrap_or_else(|e| e.into_inner());
        if g.as_ref().map(|s| s.started).unwrap_or(false) {
            return;
        }
        let (me, paired, layout, manual) = load();
        let s = State {
            me,
            name: computer_name(),
            peers: vec![],
            paired,
            pairing: Pairing::None,
            decide: None,
            session: None,
            layout,
            local: local_screens(),
            started: true,
            host,
            msg,
            problem: String::new(),
            hold: false,
            quiet: false,
            manual,
            xfer: None,
            xfer_msg: None,
            remote_shelf: vec![],
            remote_disp: HashMap::new(),
            remote_hashes: vec![],
            have: Default::default(),
            jobs: vec![],
        };
        save(&s);
        *g = Some(s);
        true
    };
    if !fresh {
        return;
    }
    HOST.store(host, std::sync::atomic::Ordering::SeqCst);
    HOST_MSG.store(msg, std::sync::atomic::Ordering::SeqCst);
    input::set_notify(Box::new(tell_ui));
    push_geo();
    let g = GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    spawn("listen", move || listen(g));
    spawn("find", move || discover(g));
    firewall::check();
    // Developer aid: ISLET_LINK_ADDR connects by address at start.
    if let Ok(a) = std::env::var("ISLET_LINK_ADDR") {
        spawn("demo-addr", move || {
            std::thread::sleep(Duration::from_secs(4));
            connect_address(&a);
        });
    }
    if clip::demo().is_some() {
        spawn("demo", || {
            std::thread::sleep(Duration::from_secs(12));
            ui_job(UiJob::Demo);
        });
    }
}

pub fn stop() {
    let tx = with(|s| {
        s.started = false;
        s.session.as_ref().map(|x| x.tx.clone())
    })
    .flatten();
    if let Some(tx) = tx {
        let _ = tx.send(Msg::Bye);
    }
    input::stop();
    input::show_pointer_now();
    // Wake the listener so it lets go of its port (it's waiting for a call).
    let _ = TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], ports().1)), Duration::from_millis(300));
}

/// Each start of Link is a new generation; threads of an older one stop.
static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn running() -> bool {
    with(|s| s.started).unwrap_or(false)
}

fn running_gen(g: u64) -> bool {
    running() && GEN.load(std::sync::atomic::Ordering::SeqCst) == g
}

// ---------------------------------------------------------------------------
// Finding each other
// ---------------------------------------------------------------------------

/// This PC's IPv4 networks: (its address, prefix length), skipping
/// loopback and link-local (169.254) addresses.
fn local_nets() -> Vec<(u32, u32)> {
    use windows::Win32::NetworkManagement::IpHelper::*;
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};
    let mut out = Vec::new();
    unsafe {
        let mut size = 16 * 1024u32;
        let mut buf: Vec<u64> = Vec::new();
        for _ in 0..3 {
            buf = vec![0u64; size as usize / 8 + 1];
            let r = GetAdaptersAddresses(AF_INET.0 as u32, GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER, None, Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH), &mut size);
            if r == 111 {
                continue;
            }
            if r != 0 {
                return out;
            }
            break;
        }
        let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !p.is_null() {
            let a = &*p;
            p = a.Next;
            if a.OperStatus != IfOperStatusUp || a.IfType == 24 {
                continue;
            }
            let mut u = a.FirstUnicastAddress;
            while !u.is_null() {
                let sa = (*u).Address.lpSockaddr;
                if !sa.is_null() && (*sa).sa_family == AF_INET {
                    let sin = &*(sa as *const SOCKADDR_IN);
                    let ip = u32::from_be_bytes({
                        let b = sin.sin_addr.S_un.S_un_b;
                        [b.s_b1, b.s_b2, b.s_b3, b.s_b4]
                    });
                    let bits = (*u).OnLinkPrefixLength.min(32) as u32;
                    if bits > 0 && bits < 32 && (ip >> 16) != 0xA9FE && (ip >> 24) != 127 {
                        out.push((ip, bits));
                    }
                }
                u = (*u).Next;
            }
        }
    }
    out
}

/// Broadcast addresses of this PC's networks (plus 255.255.255.255).
fn broadcasts() -> Vec<std::net::Ipv4Addr> {
    let mut out = vec![std::net::Ipv4Addr::BROADCAST];
    for (ip, bits) in local_nets() {
        let mask = u32::MAX << (32 - bits);
        out.push(std::net::Ipv4Addr::from(ip | !mask));
    }
    out.dedup();
    out
}

/// Every other address on this PC's networks, for asking one by one when
/// broadcasts don't get through (many Wi-Fi networks, routers and VPN /
/// virtual adapters drop them, while ordinary traffic flows). A network
/// bigger than /22 is narrowed to the /24 this PC is in.
fn sweep_targets() -> Vec<std::net::Ipv4Addr> {
    let mut out = Vec::new();
    for (ip, bits) in local_nets() {
        let bits = if bits < 22 { 24 } else { bits };
        let mask = u32::MAX << (32 - bits);
        let net = ip & mask;
        let last = net | !mask;
        // (Developer aid: with ISLET_LINK_NOBEACON two copies on one PC find
        // each other only this way, so this PC's own address is asked too.)
        let me_too = std::env::var("ISLET_LINK_NOBEACON").is_ok();
        for a in (net + 1)..last {
            if a != ip || me_too {
                out.push(std::net::Ipv4Addr::from(a));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Machines this PC has talked to lately on its local networks (the ARP
/// cache): asked directly too.
fn neighbours() -> Vec<std::net::Ipv4Addr> {
    use windows::Win32::NetworkManagement::IpHelper::*;
    use windows::Win32::Networking::WinSock::AF_INET;
    let mut out = Vec::new();
    unsafe {
        let mut t: *mut MIB_IPNET_TABLE2 = std::ptr::null_mut();
        if GetIpNetTable2(AF_INET, &mut t).is_err() || t.is_null() {
            return out;
        }
        let n = (*t).NumEntries as usize;
        let rows = std::slice::from_raw_parts((*t).Table.as_ptr(), n);
        for r in rows {
            // Reachable / stale / delay / probe / permanent — not unreachable or incomplete.
            // (Delay, stale, reachable, permanent: PCs that really answered once.)
            if r.State.0 < 3 || out.len() >= 64 {
                continue;
            }
            let b = r.Address.Ipv4.sin_addr.S_un.S_un_b;
            let a = std::net::Ipv4Addr::new(b.s_b1, b.s_b2, b.s_b3, b.s_b4);
            if !a.is_multicast() && !a.is_broadcast() && !a.is_unspecified() && !a.is_loopback() && a.octets()[3] != 255 {
                out.push(a);
            }
        }
        FreeMibTable(t as *const _);
    }
    out
}

/// A multicast group too: some networks pass multicast but not broadcast.
const GROUP: std::net::Ipv4Addr = std::net::Ipv4Addr::new(239, 255, 78, 30);

fn beacon() -> String {
    let (me, name) = with(|s| (s.me, s.name.clone())).unwrap_or_default();
    format!("{BEACON}|{me:x}|{}|{}", name.replace('|', " "), ports().1)
}

fn discover(gen: u64) {
    let (udp, _, peer_udp) = ports();
    let mut tries = 0;
    let bound = loop {
        match UdpSocket::bind(("0.0.0.0", udp)) {
            Ok(s) => break Ok(s),
            Err(_) if tries < 10 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(300));
            }
            Err(e) => break Err(e),
        }
    };
    let sock = match bound {
        Ok(s) => s,
        Err(e) => {
            crate::util::log(&format!("link: can't listen for other PCs: {e}"));
            with(|s| s.problem = format!("Can't listen for other PCs ({e})"));
            return;
        }
    };
    let _ = sock.set_broadcast(true);
    let _ = sock.set_read_timeout(Some(Duration::from_millis(400)));
    let _ = sock.set_multicast_loop_v4(false);
    no_conn_reset(&sock);
    let join = |sock: &UdpSocket| {
        for (ip, _) in local_nets() {
            let _ = sock.join_multicast_v4(&GROUP, &std::net::Ipv4Addr::from(ip));
        }
    };
    join(&sock);
    let mut last_beacon = crate::util::ago(Duration::from_secs(10));
    let mut targets = broadcasts();
    let mut last_targets = Instant::now();
    // Asking PCs one by one happens on a thread of its own, so it never holds
    // up the broadcasts and the answers here — but from this same socket
    // (same port): when Windows Firewall has no rule for Phemio yet, a PC
    // only gets in after this one sent to it from this port first.
    match sock.try_clone() {
        Ok(s2) => spawn("link-sweep", move || sweep(gen, peer_udp, s2)),
        Err(e) => crate::util::log(&format!("link: no direct asking ({e})")),
    }
    let mut buf = [0u8; 512];
    while running_gen(gen) {
        if last_beacon.elapsed() > Duration::from_millis(1800) {
            last_beacon = Instant::now();
            if last_targets.elapsed() > Duration::from_secs(30) {
                targets = broadcasts();
                last_targets = Instant::now();
                // (Networks change: Wi-Fi switched, VPN on…)
                join(&sock);
            }
            let b = beacon();
            // (ISLET_LINK_NOBEACON: developer aid, as if broadcasts were blocked.)
            let quiet_net = std::env::var("ISLET_LINK_NOBEACON").is_ok();
            for t in targets.iter().filter(|_| !quiet_net) {
                let _ = sock.send_to(b.as_bytes(), (*t, peer_udp));
            }
            if !quiet_net {
                let _ = sock.send_to(b.as_bytes(), (GROUP, peer_udp));
            }
            // The address you typed, in case broadcasts don't get through.
            if let Some(a) = manual_addr() {
                let _ = sock.send_to(b.as_bytes(), (a.ip(), peer_udp));
            }
            if peer_udp != udp && !quiet_net {
                let _ = sock.send_to(b.as_bytes(), ("127.0.0.1", peer_udp));
            }
            // Screens plugged in / out: tell the other PC.
            refresh_screens();
        }
        if let Ok((n, from)) = sock.recv_from(&mut buf) {
            if n > 300 {
                continue;
            }
            let text = String::from_utf8_lossy(&buf[..n]).to_string();
            let parts: Vec<&str> = text.split('|').collect();
            if parts.len() >= 4 && parts[0] == BEACON {
                let Ok(id) = u64::from_str_radix(parts[1], 16) else { continue };
                let port: u16 = parts[3].parse().unwrap_or(TCP_PORT);
                let me = with(|s| s.me).unwrap_or(0);
                if id == me {
                    continue;
                }
                let addr = SocketAddr::new(from.ip(), port);
                let (new, connect) = with(|s| {
                    // New, or not heard from in a while: answer directly
                    // (it may not be getting our broadcasts).
                    let new = !s.peers.iter().any(|p| p.id == id && p.seen.elapsed() < Duration::from_secs(6));
                    s.peers.retain(|p| p.id != id);
                    s.peers.push(Peer { id, name: parts[2].to_string(), addr, seen: Instant::now() });
                    let mut moved = false;
                    if let (Some(p), std::net::IpAddr::V4(ip)) = (s.paired.iter_mut().find(|p| p.id == id), from.ip()) {
                        if p.last != Some(ip) {
                            p.last = Some(ip);
                            moved = true;
                        }
                    }
                    if moved {
                        save(s);
                    }
                    let paired = s.paired.iter().any(|p| p.id == id);
                    // The PC with the smaller id calls; the other calls now and then too,
                    // in case the first call can't get in.
                    (new, paired && s.session.is_none() && !s.hold && !s.quiet && (me < id || rand_bit()))
                })
                .unwrap_or((false, false));
                if new {
                    // Answer directly so they find us even if broadcasts don't get through.
                    let _ = sock.send_to(beacon().as_bytes(), SocketAddr::new(from.ip(), peer_udp));
                    let _ = sock.send_to(beacon().as_bytes(), from);
                    tell_ui();
                    // Developer aid: ISLET_LINK_AUTOPAIR pairs with the first PC found (smaller id asks).
                    if std::env::var("ISLET_LINK_AUTOPAIR").map(|v| !v.is_empty()).unwrap_or(false) && me < id && secret_for(id).is_none() {
                        pair_with(id);
                    }
                }
                if connect {
                    connect_to(addr);
                }
            }
        }
    }
}

/// Windows reports an ICMP "port unreachable" (any PC asked directly that
/// doesn't run Phemio) as an error on the socket's next receive; switch
/// that off so those never get in the way of real answers.
fn no_conn_reset(sock: &UdpSocket) {
    use std::os::windows::io::AsRawSocket;
    use windows::Win32::Networking::WinSock::{WSAIoctl, SOCKET};
    const SIO_UDP_CONNRESET: u32 = 0x9800_000C;
    let off: u32 = 0;
    let mut got = 0u32;
    unsafe {
        let _ = WSAIoctl(SOCKET(sock.as_raw_socket() as usize), SIO_UDP_CONNRESET, Some(&off as *const u32 as *const _), 4, None, 0, &mut got, None, None);
    }
}

/// For networks (or firewalls) that drop broadcasts: ask PCs directly — a
/// paired PC where it was last seen and every PC this one has talked to
/// lately, every round (cheap: a handful), and, while no other PC has been
/// heard from, every address on this PC's networks (after a second or two,
/// then every 5 s for the first half minute, every 15 s for the first
/// two minutes, then every 45 s). Once one round went out, the PCs that are
/// on are in the neighbour table, so the next rounds reach them anyway.
fn sweep(gen: u64, peer_udp: u16, sock: UdpSocket) {
    let started = Instant::now();
    let mut last_sweep: Option<Instant> = None;
    let mut said = false;
    while running_gen(gen) {
        std::thread::sleep(Duration::from_millis(1500));
        if !running_gen(gen) {
            break;
        }
        let b = beacon();
        let mut direct: Vec<std::net::Ipv4Addr> = with(|s| s.paired.iter().filter_map(|p| p.last).collect()).unwrap_or_default();
        direct.extend(neighbours());
        direct.sort();
        direct.dedup();
        for a in &direct {
            let _ = sock.send_to(b.as_bytes(), (*a, peer_udp));
        }
        let lonely = with(|s| s.peers.iter().all(|p| p.seen.elapsed() > Duration::from_secs(20))).unwrap_or(true);
        let up = started.elapsed();
        let every = if up < Duration::from_secs(30) {
            5
        } else if up < Duration::from_secs(120) {
            15
        } else {
            45
        };
        let due = last_sweep.map(|t| t.elapsed() >= Duration::from_secs(every)).unwrap_or(true);
        if !lonely || !due {
            continue;
        }
        last_sweep = Some(Instant::now());
        let all = sweep_targets();
        if all.is_empty() {
            continue;
        }
        if !said {
            said = true;
            crate::util::log(&format!(
                "link: also asking {} addresses on {:?} directly (and {} known PCs every round)",
                all.len(),
                local_nets().iter().map(|(ip, b)| format!("{}/{b}", std::net::Ipv4Addr::from(*ip))).collect::<Vec<_>>(),
                direct.len()
            ));
        }
        for (i, a) in all.iter().enumerate() {
            if !running_gen(gen) {
                return;
            }
            let _ = sock.send_to(b.as_bytes(), (*a, peer_udp));
            // (A short breather every 64 so the network isn't flooded.)
            if i % 64 == 63 {
                std::thread::sleep(Duration::from_millis(3));
            }
        }
    }
}

/// The bigger-id PC calls too, now and then (in case the call can't get in).
fn rand_bit() -> bool {
    proto::random::<1>()[0] < 24
}

static CONNECTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn connect_to(addr: SocketAddr) {
    if CONNECTING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    spawn("call", move || {
        let r = call(addr);
        CONNECTING.store(false, std::sync::atomic::Ordering::SeqCst);
        if let Err(e) = r {
            crate::util::log(&format!("link: calling {addr}: {e}"));
        }
    });
}

/// Call a paired PC and run the session.
fn call(addr: SocketAddr) -> std::io::Result<()> {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    let (me, name) = me();
    let (id, pname, tx, rx, _) = proto::handshake_for(&mut s, me, &name, None, secret_for, proto::FOR_SESSION)?;
    CONNECTING.store(false, std::sync::atomic::Ordering::SeqCst);
    run_session(s, id, pname, tx, rx, Some(addr));
    Ok(())
}

/// The address you typed, as host:port.
fn manual_addr() -> Option<SocketAddr> {
    use std::net::ToSocketAddrs;
    let m = with(|s| s.manual.trim().to_string()).unwrap_or_default();
    if m.is_empty() {
        return None;
    }
    let with_port = if m.parse::<std::net::IpAddr>().is_ok() || !m.contains(':') { format!("{m}:{}", ports().1) } else { m };
    with_port.to_socket_addrs().ok()?.find(|a| a.is_ipv4())
}

/// Connect by address: call it (paired), or ask to pair.
pub fn connect_address(text: &str) {
    with(|s| {
        s.manual = text.trim().to_string();
        s.hold = false;
        s.quiet = false;
        s.problem.clear();
        save(s);
    });
    tell_ui();
    spawn("manual", || {
        let Some(addr) = manual_addr() else {
            with(|s| s.problem = crate::i18n::tr("Couldn't find that address").to_string());
            tell_ui();
            return;
        };
        if connected() {
            return;
        }
        match call(addr) {
            Ok(()) => {}
            Err(e) if e.to_string().contains("not paired") => pair_at(addr, addr.ip().to_string()),
            Err(e) => {
                with(|s| s.problem = format!("{} {addr}: {e}", crate::i18n::tr("Couldn't reach")));
                tell_ui();
            }
        }
    });
}

/// "Disconnect": hang up and stay apart until "Connect".
pub fn disconnect() {
    with(|s| s.hold = true);
    send(Msg::Bye);
    tell_ui();
}

/// "Connect": call the paired PC again.
pub fn reconnect() {
    let target = with(|s| {
        s.hold = false;
        s.quiet = false;
        let ids: Vec<u64> = s.paired.iter().map(|p| p.id).collect();
        s.peers.iter().filter(|p| ids.contains(&p.id)).max_by_key(|p| p.seen).map(|p| p.addr)
    })
    .flatten()
    .or_else(manual_addr);
    if let Some(a) = target {
        connect_to(a);
    }
    tell_ui();
}

/// Show the screen numbers on both PCs.
pub fn identify() {
    send(Msg::Identify);
    show_identify();
}

/// (UI thread) the numbers on this PC's screens.
pub fn show_identify() {
    let Some(g) = with(build_geo) else { return };
    let name = me().1;
    let nums = g.numbers();
    let items: Vec<_> = g.screens.iter().zip(nums).filter(|(s, _)| s.machine == g.me).map(|(s, n)| (n, format!("{name} · {}", s.label), s.px)).collect();
    identify::show(&items);
}

/// Fetch items from the other PC's shelf (they land in Downloads\Phemio).
pub fn shelf_get(ids: Vec<u64>) {
    send(Msg::ShelfGet(ids));
}

fn secret_for(id: u64) -> Option<[u8; 32]> {
    with(|s| s.paired.iter().find(|p| p.id == id).map(|p| p.secret)).flatten()
}

fn listen(gen: u64) {
    // (Turned off and on again quickly: the old listener may still be letting go.)
    let mut tries = 0;
    let bound = loop {
        match TcpListener::bind(("0.0.0.0", ports().1)) {
            Ok(l) => break Ok(l),
            Err(e) if tries < 10 => {
                tries += 1;
                let _ = e;
                std::thread::sleep(Duration::from_millis(300));
            }
            Err(e) => break Err(e),
        }
    };
    let l = match bound {
        Ok(l) => l,
        Err(e) => {
            crate::util::log(&format!("link: can't accept connections: {e}"));
            with(|s| s.problem = format!("Can't accept connections ({e})"));
            return;
        }
    };
    for c in l.incoming() {
        if !running_gen(gen) {
            break;
        }
        let Ok(mut s) = c else { continue };
        if with(|x| x.hold).unwrap_or(false) {
            continue;
        }
        spawn("incoming", move || {
            let _ = s.set_nodelay(true);
            let _ = s.set_read_timeout(Some(Duration::from_secs(150)));
            let r = (|| -> std::io::Result<()> {
                let (me, name) = with(|x| (x.me, x.name.clone())).unwrap_or_default();
                match proto::read_plain(&mut s)? {
                    Msg::PairReq { proto: pv, id, name: pname, pk } => {
                        if pv != proto::PROTO {
                            return Err(proto::bad("different Phemio version"));
                        }
                        let key = proto::PairKey::new();
                        let (code, secret) = proto::pair_answer(&mut s, me, &name, &key, id, &pk)?;
                        if !confirm_pairing(&mut s, id, &pname, code, secret)? {
                            return Ok(());
                        }
                        let first = proto::read_plain(&mut s)?;
                        let (pid, pn, tx, rx) = proto::handshake(&mut s, me, &name, Some(first), secret_for)?;
                        run_session(s, pid, pn, tx, rx, None);
                    }
                    hello @ Msg::Hello { .. } => {
                        let theirs = if let Msg::Hello { purpose, .. } = &hello { *purpose } else { 0 };
                        let (pid, pn, tx, rx, purpose) = proto::handshake_for(&mut s, me, &name, Some(hello), secret_for, theirs)?;
                        if purpose == proto::FOR_FILES {
                            // Files only from the PC we're linked with.
                            if with(|x| x.session.as_ref().map(|y| y.id == pid)).flatten().unwrap_or(false) {
                                if let Err(e) = clip::receive(s, tx, rx) {
                                    crate::util::log(&format!("link: receiving files: {e}"));
                                    set_xfer_done(format!("{}: {e}", crate::i18n::tr("Transfer failed")));
                                }
                            }
                        } else if purpose == proto::FOR_AUDIO {
                            // Sound only from the PC we're linked with.
                            if with(|x| x.session.as_ref().map(|y| y.id == pid)).flatten().unwrap_or(false) {
                                if let Err(e) = audio::receive(s, tx, rx, pid) {
                                    crate::util::log(&format!("link: sound from the other PC: {e}"));
                                }
                            }
                        } else {
                            run_session(s, pid, pn, tx, rx, None);
                        }
                    }
                    _ => {}
                }
                Ok(())
            })();
            if let Err(e) = r {
                crate::util::log(&format!("link: incoming: {e}"));
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Pairing
// ---------------------------------------------------------------------------

/// "Pair" on the page: ask PC `id`.
pub fn pair_with(id: u64) {
    let Some((addr, pname)) = with(|s| s.peers.iter().find(|p| p.id == id).map(|p| (p.addr, p.name.clone()))).flatten() else { return };
    pair_at(addr, pname);
}

fn pair_at(addr: SocketAddr, pname: String) {
    with(|s| s.pairing = Pairing::Asking { name: pname.clone() });
    tell_ui();
    spawn("pair", move || {
        let r = (|| -> std::io::Result<()> {
            let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(4))?;
            s.set_nodelay(true)?;
            s.set_read_timeout(Some(Duration::from_secs(150)))?;
            let (me, name) = with(|x| (x.me, x.name.clone())).unwrap_or_default();
            let key = proto::PairKey::new();
            let (id, pname, code, secret) = proto::pair_ask(&mut s, me, &name, &key)?;
            if !confirm_pairing(&mut s, id, &pname, code, secret)? {
                return Ok(());
            }
            let (pid, pn, tx, rx) = proto::handshake(&mut s, me, &name, None, secret_for)?;
            run_session(s, pid, pn, tx, rx, Some(addr));
            Ok(())
        })();
        if let Err(e) = r {
            with(|s| {
                if !matches!(s.pairing, Pairing::None) {
                    s.pairing = Pairing::Failed(e.to_string());
                }
            });
            tell_ui();
        }
    });
}

/// Show the code, wait for your answer here and theirs there.
fn confirm_pairing(s: &mut TcpStream, id: u64, pname: &str, code: u32, secret: [u8; 32]) -> std::io::Result<bool> {
    let (tx, rx): (Sender<bool>, Receiver<bool>) = channel();
    with(|st| {
        st.pairing = Pairing::Code { id, name: pname.to_string(), code, mine: None };
        st.decide = Some(tx);
    });
    tell_ui();
    if std::env::var("ISLET_LINK_AUTOPAIR").map(|v| !v.is_empty()).unwrap_or(false) {
        std::thread::sleep(Duration::from_millis(2500));
        decide(true);
    }
    let mine = rx.recv_timeout(Duration::from_secs(120)).unwrap_or(false);
    let both = proto::pair_confirm(s, mine)?;
    with(|st| {
        st.decide = None;
        if both {
            st.paired.retain(|p| p.id != id);
            st.paired.push(Paired { id, name: pname.to_string(), secret, last: None });
            st.pairing = Pairing::None;
            save(st);
        } else {
            st.pairing = Pairing::Failed(if mine { "The other PC said no".into() } else { "Cancelled".into() });
        }
    });
    tell_ui();
    Ok(both)
}

/// Your answer to the code shown.
pub fn decide(ok: bool) {
    let tx = with(|s| {
        if let Pairing::Code { mine, .. } = &mut s.pairing {
            *mine = Some(ok);
        }
        s.decide.clone()
    })
    .flatten();
    match tx {
        Some(tx) => {
            let _ = tx.send(ok);
        }
        None => {
            with(|s| s.pairing = Pairing::None);
        }
    }
    tell_ui();
}

pub fn dismiss() {
    with(|s| {
        if matches!(s.pairing, Pairing::Failed(_)) {
            s.pairing = Pairing::None;
        }
    });
    tell_ui();
}

/// Forget the pairing with PC `id` (and hang up).
pub fn unpair(id: u64) {
    let tx = with(|s| {
        s.paired.retain(|p| p.id != id);
        save(s);
        s.session.as_ref().filter(|x| x.id == id).map(|x| x.tx.clone())
    })
    .flatten();
    if let Some(tx) = tx {
        let _ = tx.send(Msg::Bye);
    }
    tell_ui();
}

/// Put the pointer on the other PC (its biggest screen's middle).
pub fn jump() {
    let g = with(build_geo);
    if let Some(g) = g {
        let best = (0..g.screens.len()).filter(|&i| g.screens[i].machine != g.me).max_by_key(|&i| (g.screens[i].gw() * g.screens[i].gh()).max(0.0) as i64);
        if let Some(i) = best {
            input::jump_to(i);
        }
    }
}

// ---------------------------------------------------------------------------
// A session
// ---------------------------------------------------------------------------

fn refresh_screens() {
    let now = local_screens();
    let changed = with(|s| {
        if s.local != now {
            s.local = now.clone();
            s.session.as_ref().map(|x| x.tx.clone())
        } else {
            None
        }
    })
    .flatten();
    if let Some(tx) = changed {
        let _ = tx.send(Msg::Screens(now));
        push_geo();
    }
}

/// The pointer just went to the other PC (`over`) or came back.
fn crossed(over: bool) {
    if over {
        clip::offer(false);
    }
}

fn run_session(stream: TcpStream, id: u64, name: String, mut tx_c: proto::Cipher, rx_c: proto::Cipher, call: Option<SocketAddr>) {
    let (tx, rx) = channel::<Msg>();
    let remote = stream.peer_addr().ok();
    // One link at a time (and none while you've said Disconnect).
    let ok = with(|s| {
        if s.session.is_some() || s.hold || !s.started {
            return false;
        }
        s.session = Some(Session { id, name: name.clone(), screens: vec![], rtt: VecDeque::new(), tx: tx.clone(), remote, call });
        s.quiet = false;
        s.remote_shelf.clear();
        if let Some(p) = s.paired.iter_mut().find(|p| p.id == id) {
            let ip = match remote.map(|r| r.ip()) {
                Some(std::net::IpAddr::V4(a)) => Some(a),
                _ => p.last,
            };
            if p.name != name || p.last != ip {
                p.name = name.clone();
                p.last = ip;
                save(s);
            }
        }
        s.problem.clear();
        true
    })
    .unwrap_or(false);
    if !ok {
        // Just hang up (a Bye would tell it to stop calling).
        let _ = stream.shutdown(std::net::Shutdown::Both);
        drop(tx_c);
        return;
    }
    LINKED.store(true, std::sync::atomic::Ordering::SeqCst);
    crate::util::log(&format!("link: connected to {name}"));
    let _ = stream.set_read_timeout(Some(Duration::from_secs(6)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(6)));
    let mut writer = match stream.try_clone() {
        Ok(w) => w,
        Err(_) => {
            LINKED.store(false, std::sync::atomic::Ordering::SeqCst);
            with(|s| s.session = None);
            return;
        }
    };
    // Writer: everything to send, sealed, as soon as it's queued.
    let w = std::thread::Builder::new()
        .name("link-send".into())
        .spawn(move || {
            guard("send", || {
                while let Ok(m) = rx.recv() {
                    let bye = m == Msg::Bye;
                    if proto::write_sealed(&mut writer, &mut tx_c, &m).is_err() || bye {
                        break;
                    }
                    let _ = writer.flush();
                }
            });
            let _ = writer.shutdown(std::net::Shutdown::Both);
        })
        .ok();
    // Say what we've got.
    if let Some((screens, layout)) = with(|s| (s.local.clone(), Msg::Layout { version: s.layout.version, places: s.layout.places.iter().map(|(k, v)| (k.clone(), v.0, v.1)).collect() })) {
        let _ = tx.send(Msg::Screens(screens));
        let _ = tx.send(layout);
    }
    let _ = tx.send(clip::shelf_list());
    clip::send_shelf_hashes();
    input::start(tx.clone());
    push_geo();
    tell_ui();
    // Pings: a few a second while the pointer is over there (also the watchdog).
    let alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let last_pong = std::sync::Arc::new(Mutex::new(Instant::now()));
    {
        let (alive, last_pong, tx) = (alive.clone(), last_pong.clone(), tx.clone());
        spawn("ping", move || {
            while alive.load(std::sync::atomic::Ordering::SeqCst) {
                let over = input::captured().is_some();
                let _ = tx.send(Msg::Ping { t: now_us() });
                std::thread::sleep(Duration::from_millis(if over { 250 } else { 1000 }));
                // The other PC stopped answering while you're using it: come home.
                let late = last_pong.lock().map(|t| t.elapsed()).unwrap_or_else(|e| e.into_inner().elapsed());
                if over && late > Duration::from_millis(1500) {
                    crate::util::log("link: the other PC stopped answering — pointer back home");
                    input::come_home(None, false);
                }
            }
        });
    }
    let player = start_player();
    let mut reader = stream;
    let mut rx_c = rx_c;
    let mut said_bye = false;
    loop {
        let m = match proto::read_sealed(&mut reader, &mut rx_c) {
            Ok(m) => m,
            Err(e) => {
                crate::util::log(&format!("link: {name}: {e}"));
                break;
            }
        };
        if m == Msg::Bye {
            said_bye = true;
            break;
        }
        // The other PC's pointer, buttons and keys are played on a thread of
        // their own (so a slow app never holds up the link), in order.
        if matches!(m, Msg::Move { .. } | Msg::Enter { .. } | Msg::Button { .. } | Msg::Wheel { .. } | Msg::Key { .. }) {
            let _ = player.send(m);
            continue;
        }
        // One odd message never ends the link (or the app).
        guard("message", || handle(m, id, &tx, &last_pong));
    }
    alive.store(false, std::sync::atomic::Ordering::SeqCst);
    let _ = tx.send(Msg::Bye);
    drop(tx);
    if let Some(w) = w {
        let _ = w.join();
    }
    input::stop();
    LINKED.store(false, std::sync::atomic::Ordering::SeqCst);
    with(|s| {
        s.session = None;
        s.remote_shelf.clear();
        s.remote_hashes.clear();
        s.have.clear();
        s.xfer = None;
        if said_bye {
            s.quiet = true;
        }
    });
    push_geo();
    crate::util::log(&format!("link: {name} disconnected"));
    tell_ui();
}

/// Plays the other PC's input here. Pointer moves that pile up (a busy
/// moment, a burst from the network) are merged into the latest one, so this
/// PC is never flooded with more moves than it can take.
fn start_player() -> Sender<Msg> {
    let (tx, rx) = channel::<Msg>();
    spawn("play", move || {
        let mut next: Option<Msg> = None;
        loop {
            let m = match next.take() {
                Some(m) => m,
                None => match rx.recv() {
                    Ok(m) => m,
                    Err(_) => break,
                },
            };
            match m {
                Msg::Move { mut x, mut y } | Msg::Enter { mut x, mut y } => {
                    if matches!(m, Msg::Enter { .. }) {
                        // Being driven from there: this PC's own pointer can't be over there too.
                        if input::captured().is_some() {
                            input::come_home(None, false);
                        }
                    }
                    while let Ok(n) = rx.try_recv() {
                        match n {
                            Msg::Move { x: nx, y: ny } => {
                                x = nx;
                                y = ny;
                            }
                            other => {
                                next = Some(other);
                                break;
                            }
                        }
                    }
                    input::play_move(x, y);
                }
                Msg::Button { b, down } => input::play_button(b, down),
                Msg::Wheel { delta, horizontal } => input::play_wheel(delta, horizontal),
                Msg::Key { vk, scan, flags } => input::play_key(vk, scan, flags),
                _ => {}
            }
        }
    });
    tx
}

fn handle(m: Msg, id: u64, tx: &Sender<Msg>, last_pong: &Mutex<Instant>) {
    match m {
        Msg::Leave => {
            input::release_all();
            input::DRIVEN.store(false, std::sync::atomic::Ordering::SeqCst);
            // The pointer went back: whatever was copied here goes with it.
            clip::offer(false);
            tell_ui();
        }
        Msg::Take => {
            input::come_home(None, false);
            tell_ui();
        }
        Msg::Ping { t } => {
            let _ = tx.send(Msg::Pong { t });
        }
        Msg::Pong { t } => {
            *last_pong.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
            let ms = (now_us().saturating_sub(t)) as f32 / 1000.0;
            with(|s| {
                if let Some(x) = s.session.as_mut() {
                    x.rtt.push_back((Instant::now(), ms));
                    while x.rtt.len() > 200 {
                        x.rtt.pop_front();
                    }
                }
            });
            tell_ui();
        }
        Msg::Screens(v) => {
            with(|s| {
                if let Some(x) = s.session.as_mut() {
                    x.screens = v.into_iter().take(16).collect();
                }
            });
            push_geo();
            // The PC with the smaller id settles the map once it knows every
            // screen (new ones get a place), and shares it.
            let me = with(|s| s.me).unwrap_or(0);
            if me < id {
                let m = with(|s| {
                    build_geo(s);
                    s.layout.version += 1;
                    save(s);
                    Msg::Layout { version: s.layout.version, places: s.layout.places.iter().map(|(k, v)| (k.clone(), v.0, v.1)).collect() }
                });
                if let Some(m) = m {
                    let _ = tx.send(m);
                }
            }
            tell_ui();
        }
        Msg::Layout { version, places } => {
            let me = with(|s| s.me).unwrap_or(0);
            with(|s| {
                // Newest wins; on a tie (both just laid out by themselves), the smaller id's.
                if version > s.layout.version || (version == s.layout.version && id < me) {
                    s.layout.version = version;
                    for (k, x, y) in places {
                        if x.is_finite() && y.is_finite() {
                            s.layout.places.insert(k, (x, y));
                        }
                    }
                    save(s);
                }
            });
            push_geo();
            tell_ui();
        }
        Msg::Clip { text } => clip::got_text(text),
        Msg::Identify => ui_job(UiJob::Identify),
        Msg::MainScreen { key } => ui_job(UiJob::MainScreen(key)),
        Msg::DispAsk { key } => {
            if let Some(i) = crate::display::info(&key) {
                let pending = crate::display::pending().filter(|p| p.0 == key).map(|p| p.1 as i32).unwrap_or(0);
                send(Msg::DispInfo {
                    key,
                    scale: i.scale as i32,
                    scale_rec: i.scale_rec as i32,
                    scales: i.scales.iter().map(|v| *v as i32).collect(),
                    res: (i.res.0 as i32, i.res.1 as i32),
                    res_rec: (i.res_rec.0 as i32, i.res_rec.1 as i32),
                    modes: i.modes.iter().map(|m| (m.0 as i32, m.1 as i32)).collect(),
                    orient: i.orient as i32,
                    pending,
                });
            }
        }
        Msg::DispInfo { key, scale, scale_rec, scales, res, res_rec, modes, orient, pending } => {
            let info = crate::display::Info {
                key: key.clone(),
                scale: scale as u32,
                scale_rec: scale_rec as u32,
                scales: scales.into_iter().map(|v| v as u32).collect(),
                res: (res.0 as u32, res.1 as u32),
                res_rec: (res_rec.0 as u32, res_rec.1 as u32),
                modes: modes.into_iter().map(|m| (m.0 as u32, m.1 as u32)).collect(),
                orient: orient as u32,
            };
            with(|s| {
                s.remote_disp.insert(key, (info, pending.max(0) as u64, std::time::Instant::now()));
            });
            tell_ui();
        }
        Msg::DispSet { key, op, a, b, c } => ui_job(UiJob::DispSet(key, op, a, b, c)),
        Msg::ShelfList(v) => {
            with(|s| {
                // Hashes of items no longer listed are forgotten.
                s.remote_hashes.retain(|h| v.iter().any(|x| x.0 == h.0));
                s.have.retain(|id| v.iter().any(|x| x.0 == *id));
                s.remote_shelf = v;
            });
            tell_ui();
        }
        Msg::ShelfHashes(v) => {
            with(|s| s.remote_hashes = v);
            recheck_have();
        }
        Msg::ShelfGet(ids) => clip::shelf_send(ids),
        _ => {}
    }
}
