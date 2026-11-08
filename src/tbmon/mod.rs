//! Taskbar monitor: live network / disk / CPU / GPU / RAM / NPU stats drawn
//! inside the Windows taskbar (a child window of Shell_TrayWnd, the same
//! technique TrafficMonitor uses). Port of TaskbarMonitor, redesigned so it
//! fits a normal taskbar (two lines), a small taskbar (one line) and a
//! taskbar on the left/right edge (stacked icon-over-value cells).
//!
//! Sampling runs on its own thread; the widget only redraws when a value or
//! the taskbar geometry actually changed.

pub mod metrics;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config;
use crate::gfx::{self, rf, rgb, Surface, UI_FONT};
use crate::util;
use metrics::{Sampler, Snapshot};

const CLASS: PCWSTR = w!("PhemioTaskbarMonitor");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Metric {
    Net,
    DiskIo,
    Cpu,
    CpuSpeed,
    Gpu,
    Ram,
    DiskSpace,
    Npu,
}

pub const METRICS: [(Metric, &str, &str, &str); 8] = [
    (Metric::Net, "net", "Network ↑ / ↓", "Upload and download speed"),
    (Metric::Cpu, "cpu", "CPU usage", "Same number as Task Manager"),
    (Metric::Ram, "ram", "Memory", "RAM in use"),
    (Metric::CpuSpeed, "spd", "CPU speed", "Current clock in GHz"),
    (Metric::Gpu, "gpu", "GPU usage", "Busiest engine of the graphics card"),
    (Metric::DiskIo, "dio", "Disk read / write", "All disks"),
    (Metric::DiskSpace, "dsk", "Disk space", "Used / total of the chosen drives"),
    (Metric::Npu, "npu", "NPU usage", "AI accelerator (if Windows reports one)"),
];

/// Per-item padding keys (left, top, right, bottom), in METRICS order.
pub const PAD_KEYS: [[&str; 4]; 8] = [
    ["tbm_pad_net_l", "tbm_pad_net_t", "tbm_pad_net_r", "tbm_pad_net_b"],
    ["tbm_pad_cpu_l", "tbm_pad_cpu_t", "tbm_pad_cpu_r", "tbm_pad_cpu_b"],
    ["tbm_pad_ram_l", "tbm_pad_ram_t", "tbm_pad_ram_r", "tbm_pad_ram_b"],
    ["tbm_pad_spd_l", "tbm_pad_spd_t", "tbm_pad_spd_r", "tbm_pad_spd_b"],
    ["tbm_pad_gpu_l", "tbm_pad_gpu_t", "tbm_pad_gpu_r", "tbm_pad_gpu_b"],
    ["tbm_pad_dio_l", "tbm_pad_dio_t", "tbm_pad_dio_r", "tbm_pad_dio_b"],
    ["tbm_pad_dsk_l", "tbm_pad_dsk_t", "tbm_pad_dsk_r", "tbm_pad_dsk_b"],
    ["tbm_pad_npu_l", "tbm_pad_npu_t", "tbm_pad_npu_r", "tbm_pad_npu_b"],
];

pub fn pad_keys(m: Metric) -> [&'static str; 4] {
    PAD_KEYS[METRICS.iter().position(|x| x.0 == m).unwrap_or(0)]
}

/// (left, top, right, bottom) extra space around an item, in DIPs.
fn pad(m: Metric) -> (f32, f32, f32, f32) {
    let c = config::cur();
    let k = pad_keys(m);
    let v = |i: usize| c.i(k[i]).clamp(-20, 40) as f32;
    (v(0), v(1), v(2), v(3))
}

pub fn key_of(m: Metric) -> &'static str {
    METRICS.iter().find(|x| x.0 == m).map(|x| x.1).unwrap_or("")
}

/// Metrics in the user's order.
pub fn order() -> Vec<Metric> {
    let s = config::cur().s("tbm_order");
    let mut out: Vec<Metric> = Vec::new();
    for m in s.split(',').filter_map(|k| METRICS.iter().find(|x| x.1 == k.trim()).map(|x| x.0)) {
        if !out.contains(&m) {
            out.push(m);
        }
    }
    for (m, ..) in METRICS {
        if !out.contains(&m) {
            out.push(m);
        }
    }
    out
}

pub fn move_metric(m: Metric, dir: i32) {
    let mut o = order();
    if let Some(i) = o.iter().position(|x| *x == m) {
        let j = i as i32 + dir;
        if j >= 0 && (j as usize) < o.len() {
            o.swap(i, j as usize);
        }
    }
    let s: Vec<&str> = o.iter().map(|m| key_of(*m)).collect();
    config::update(|c| c.set_s("tbm_order", &s.join(",")));
}

fn shown(m: Metric) -> bool {
    config::cur().b(&format!("tbm_{}", key_of(m)))
}

// ---------------------------------------------------------------------------
// Sampler thread
// ---------------------------------------------------------------------------

static SNAP: Mutex<Option<Snapshot>> = Mutex::new(None);
static INTERVAL: AtomicU32 = AtomicU32::new(1000);
static WANT: AtomicU8 = AtomicU8::new(0); // bit0 gpu, bit1 npu, bit2 disk space

struct Worker {
    stop: Arc<AtomicBool>,
}

impl Worker {
    fn start(host: HWND, msg: u32) -> Worker {
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let h = host.0 as isize;
        let _ = std::thread::Builder::new().name("tbmon".into()).stack_size(512 * 1024).spawn(move || {
            let w = WANT.load(Ordering::Relaxed);
            let mut sampler = Sampler::new(w & 1 != 0, w & 2 != 0);
            let mut last_want = w;
            while !st.load(Ordering::Relaxed) {
                let w = WANT.load(Ordering::Relaxed);
                if w & 3 != last_want & 3 {
                    sampler = Sampler::new(w & 1 != 0, w & 2 != 0);
                }
                last_want = w;
                let drives: Vec<char> = DRIVES.lock().map(|d| d.clone()).unwrap_or_default();
                let snap = sampler.sample(&drives, w & 1 != 0, w & 2 != 0, w & 4 != 0);
                if let Ok(mut s) = SNAP.lock() {
                    *s = Some(snap);
                }
                unsafe {
                    let _ = PostMessageW(Some(HWND(h as *mut _)), msg, WPARAM(0), LPARAM(0));
                }
                // Sleep in small steps so turning it off is quick.
                let mut left = INTERVAL.load(Ordering::Relaxed);
                while left > 0 && !st.load(Ordering::Relaxed) {
                    let step = left.min(250);
                    std::thread::sleep(std::time::Duration::from_millis(step as u64));
                    left -= step;
                }
            }
        });
        Worker { stop }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

static DRIVES: Mutex<Vec<char>> = Mutex::new(Vec::new());

fn push_options() {
    let c = config::cur();
    INTERVAL.store((c.i("tbm_interval").clamp(1, 10) as u32) * 1000, Ordering::Relaxed);
    let mut w = 0u8;
    if shown(Metric::Gpu) {
        w |= 1;
    }
    if shown(Metric::Npu) {
        w |= 2;
    }
    if shown(Metric::DiskSpace) {
        w |= 4;
    }
    WANT.store(w, Ordering::Relaxed);
    let drives: Vec<char> = c.s("tbm_drives").to_uppercase().chars().filter(|c| c.is_ascii_uppercase()).collect();
    if let Ok(mut d) = DRIVES.lock() {
        *d = drives;
    }
}

// ---------------------------------------------------------------------------
// Icons (16×16 design grid, drawn with Direct2D — crisp at any DPI)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
enum Glyph {
    Up,
    Down,
    Cpu,
    Speed,
    Gpu,
    Ram,
    Disk,
    Npu,
    Read,
    Write,
}

fn draw_glyph(s: &Surface, g: Glyph, x: f32, y: f32, size: f32, c: D2D1_COLOR_F) {
    let k = size / 16.0;
    let p = |gx: f32, gy: f32| (x + gx * k, y + gy * k);
    let w = (1.25 * k).max(1.0);
    let line = |a: (f32, f32), b: (f32, f32)| s.line(a.0, a.1, b.0, b.1, c, w);
    let rect = |a: (f32, f32), b: (f32, f32), r: f32| s.stroke_rr(rf(a.0, a.1, b.0 - a.0, b.1 - a.1), r * k, c, w);
    let chip = || {
        rect(p(4.0, 4.0), p(12.0, 12.0), 1.2);
        for t in [6.5, 9.5] {
            line(p(t, 4.0), p(t, 1.8));
            line(p(t, 12.0), p(t, 14.2));
            line(p(4.0, t), p(1.8, t));
            line(p(12.0, t), p(14.2, t));
        }
    };
    let arrow = |up: bool, small: bool| {
        let (top, bot) = if small { (3.0, 13.0) } else { (2.0, 14.0) };
        if up {
            line(p(8.0, bot), p(8.0, 6.5));
            s.fill_poly(&[p(8.0, top), p(4.2, 7.0), p(11.8, 7.0)], c);
        } else {
            line(p(8.0, top), p(8.0, 9.5));
            s.fill_poly(&[p(8.0, bot), p(4.2, 9.0), p(11.8, 9.0)], c);
        }
    };
    match g {
        Glyph::Up => arrow(true, false),
        Glyph::Down => arrow(false, false),
        Glyph::Read | Glyph::Write => {
            // Little disk with an arrow
            rect(p(2.0, 9.0), p(14.0, 14.0), 1.0);
            let (cx, cy) = p(11.6, 11.5);
            s.fill_circle(cx, cy, 0.9 * k, c);
            if g == Glyph::Read {
                line(p(8.0, 1.5), p(8.0, 6.0));
                s.fill_poly(&[p(8.0, 7.6), p(5.6, 4.8), p(10.4, 4.8)], c);
            } else {
                line(p(8.0, 7.6), p(8.0, 3.2));
                s.fill_poly(&[p(8.0, 1.2), p(5.6, 4.0), p(10.4, 4.0)], c);
            }
        }
        Glyph::Cpu => chip(),
        Glyph::Npu => {
            chip();
            let n = [p(6.0, 6.6), p(10.0, 6.6), p(8.0, 9.8)];
            line(n[0], n[1]);
            line(n[0], n[2]);
            line(n[1], n[2]);
            for q in n {
                s.fill_circle(q.0, q.1, 0.8 * k, c);
            }
        }
        Glyph::Speed => {
            // Gauge: half circle + needle
            let (cx, cy) = p(8.0, 10.0);
            let r = 6.2 * k;
            let n = 14;
            for i in 0..n {
                let a0 = std::f32::consts::PI + std::f32::consts::PI * i as f32 / n as f32;
                let a1 = std::f32::consts::PI + std::f32::consts::PI * (i + 1) as f32 / n as f32;
                s.line(cx + r * a0.cos(), cy + r * a0.sin(), cx + r * a1.cos(), cy + r * a1.sin(), c, w);
            }
            line((cx, cy), p(11.6, 6.2));
            s.fill_circle(cx, cy, 1.2 * k, c);
        }
        Glyph::Gpu => {
            rect(p(1.0, 4.0), p(15.0, 12.0), 1.2);
            for gx in [5.2, 10.8] {
                let (cx, cy) = p(gx, 8.0);
                s.stroke_rr(rf(cx - 1.8 * k, cy - 1.8 * k, 3.6 * k, 3.6 * k), 1.8 * k, c, w);
            }
        }
        Glyph::Ram => {
            rect(p(1.5, 4.0), p(14.5, 10.5), 1.0);
            for gx in [4.5, 8.0, 11.5] {
                line(p(gx, 10.5), p(gx, 13.2));
            }
            for gx in [4.5, 8.0, 11.5] {
                s.fill_rect(rf(x + (gx - 1.0) * k, y + 6.0 * k, 2.0 * k, 2.5 * k), c);
            }
        }
        Glyph::Disk => {
            rect(p(2.0, 3.0), p(14.0, 13.0), 1.2);
            line(p(2.0, 9.0), p(14.0, 9.0));
            let (cx, cy) = p(11.3, 11.0);
            s.fill_circle(cx, cy, 0.9 * k, c);
        }
    }
}

// ---------------------------------------------------------------------------
// Layout model
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Line {
    metric: Metric,
    glyph: Glyph,
    label: &'static str,
    value: String,
    short: String,
    template: &'static str,
}

/// One or two lines stacked (two-line horizontal mode).
struct Column {
    top: Line,
    bottom: Option<Line>,
}

fn build_lines(s: &Snapshot) -> (Vec<Column>, Vec<Line>) {
    let mut cols: Vec<Column> = Vec::new();
    let mut flat: Vec<Line> = Vec::new();
    let mut pending: Option<Line> = None;
    let push_single = |l: Line, cols: &mut Vec<Column>, pending: &mut Option<Line>| match pending.take() {
        Some(p) => cols.push(Column { top: p, bottom: Some(l) }),
        None => *pending = Some(l),
    };
    for m in order() {
        if !shown(m) {
            continue;
        }
        match m {
            Metric::Net => {
                let up = Line { metric: m, glyph: Glyph::Up, label: "↑", value: metrics::rate(s.tx), short: metrics::rate_short(s.tx), template: "8.8KB/s" };
                let dn = Line { metric: m, glyph: Glyph::Down, label: "↓", value: metrics::rate(s.rx), short: metrics::rate_short(s.rx), template: "8.8KB/s" };
                if let Some(p) = pending.take() {
                    cols.push(Column { top: p, bottom: None });
                }
                flat.push(up.clone());
                flat.push(dn.clone());
                cols.push(Column { top: up, bottom: Some(dn) });
            }
            Metric::DiskIo => {
                let r = Line { metric: m, glyph: Glyph::Read, label: "R", value: metrics::rate(s.disk_read), short: metrics::rate_short(s.disk_read), template: "8.8KB/s" };
                let w = Line { metric: m, glyph: Glyph::Write, label: "W", value: metrics::rate(s.disk_write), short: metrics::rate_short(s.disk_write), template: "8.8KB/s" };
                if let Some(p) = pending.take() {
                    cols.push(Column { top: p, bottom: None });
                }
                flat.push(r.clone());
                flat.push(w.clone());
                cols.push(Column { top: r, bottom: Some(w) });
            }
            _ => {
                let l = match m {
                    Metric::Cpu => Some(Line { metric: m, glyph: Glyph::Cpu, label: "CPU", value: format!("{:.0}%", s.cpu), short: format!("{:.0}%", s.cpu), template: "88%" }),
                    Metric::Ram => Some(Line { metric: m, glyph: Glyph::Ram, label: "RAM", value: format!("{:.0}%", s.ram), short: format!("{:.0}%", s.ram), template: "88%" }),
                    Metric::CpuSpeed => s.mhz.map(|mhz| Line { metric: m, glyph: Glyph::Speed, label: "SPD", value: format!("{:.2}", mhz / 1000.0), short: format!("{:.1}", mhz / 1000.0), template: "8.88" }),
                    Metric::Gpu => s.gpu.map(|g| Line { metric: m, glyph: Glyph::Gpu, label: "GPU", value: format!("{g:.0}%"), short: format!("{g:.0}%"), template: "88%" }),
                    Metric::Npu => s.npu.map(|n| Line { metric: m, glyph: Glyph::Npu, label: "NPU", value: format!("{n:.0}%"), short: format!("{n:.0}%"), template: "88%" }),
                    Metric::DiskSpace if s.disk_total > 0 => Some(Line {
                        metric: m,
                        glyph: Glyph::Disk,
                        label: "SSD",
                        value: format!("{}/{}", metrics::size(s.disk_used), metrics::size(s.disk_total)),
                        short: format!("{:.0}%", 100.0 * s.disk_used as f64 / s.disk_total as f64),
                        template: "888GB/8.8TB",
                    }),
                    _ => None,
                };
                if let Some(l) = l {
                    flat.push(l.clone());
                    push_single(l, &mut cols, &mut pending);
                }
            }
        }
    }
    if let Some(p) = pending.take() {
        cols.push(Column { top: p, bottom: None });
    }
    if flat.is_empty() {
        let l = Line { metric: Metric::Cpu, glyph: Glyph::Cpu, label: "CPU", value: format!("{:.0}%", s.cpu), short: format!("{:.0}%", s.cpu), template: "100%" };
        flat.push(l.clone());
        cols.push(Column { top: l, bottom: None });
    }
    (cols, flat)
}

// ---------------------------------------------------------------------------
// The widget window
// ---------------------------------------------------------------------------

pub struct Monitor {
    pub hwnd: HWND,
    taskbar: HWND,
    surf: Surface,
    worker: Option<Worker>,
    snap: Option<Snapshot>,
    last_key: String,
    scale: f32,
    vertical: bool,
    drag: Option<(i32, i32)>,
    /// The press didn't move: a click (opens settings), not a drag.
    clicked: bool,
    /// Taskbar colour sampled from the screen next to the widget, and when.
    bg: Option<(u32, std::time::Instant)>,
    shift: i32,
    pub status: String,
    /// Width each value has needed lately: grows at once, shrinks after a
    /// while, so items don't jiggle but gaps stay even.
    sticky: Vec<(&'static str, f32, std::time::Instant)>,
}

pub unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONDOWN | WM_MOUSEMOVE | WM_LBUTTONUP | WM_RBUTTONUP | WM_LBUTTONDBLCLK | WM_CAPTURECHANGED => {
            if let Some(m) = crate::app().tbmon.as_mut() {
                if m.hwnd == hwnd {
                    m.mouse(msg);
                    return LRESULT(0);
                }
            }
        }
        _ => {}
    }
    let _ = (wp, lp);
    DefWindowProcW(hwnd, msg, wp, lp)
}

pub fn register() {
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: util::hinstance(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
}

fn find_taskbar() -> HWND {
    unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()).unwrap_or_default() }
}

impl Monitor {
    pub fn new(host: HWND, msg: u32) -> Box<Monitor> {
        push_options();
        let mut m = Box::new(Monitor {
            hwnd: HWND::default(),
            taskbar: HWND::default(),
            surf: Surface::new().expect("surface"),
            worker: Some(Worker::start(host, msg)),
            snap: None,
            last_key: String::new(),
            scale: 1.0,
            vertical: false,
            drag: None,
            shift: config::cur().i("tbm_shift"),
            status: "Starting".into(),
            sticky: Vec::new(),
            clicked: false,
            bg: None,
        });
        m.attach();
        m
    }

    fn attached(&self) -> bool {
        unsafe { !self.hwnd.is_invalid() && !self.taskbar.is_invalid() && IsWindow(Some(self.taskbar)).as_bool() && IsWindow(Some(self.hwnd)).as_bool() }
    }

    /// Create the child window inside the taskbar (again after Explorer restarts).
    pub fn attach(&mut self) {
        if self.attached() {
            return;
        }
        unsafe {
            if !self.hwnd.is_invalid() {
                let _ = DestroyWindow(self.hwnd);
            }
            self.hwnd = HWND::default();
            self.taskbar = find_taskbar();
            if self.taskbar.is_invalid() {
                self.status = "Taskbar (Shell_TrayWnd) not found".into();
                return;
            }
            match CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOPARENTNOTIFY,
                CLASS,
                w!("Phemio Taskbar Monitor"),
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
                0,
                0,
                1,
                1,
                Some(self.taskbar),
                None,
                Some(util::hinstance()),
                None,
            ) {
                Ok(h) => {
                    self.hwnd = h;
                    self.status = "Attached to the taskbar".into();
                }
                Err(e) => self.status = format!("Couldn't attach to the taskbar: {e}"),
            }
        }
        self.last_key.clear();
    }

    pub fn destroy(&mut self) {
        self.worker = None;
        unsafe {
            if !self.hwnd.is_invalid() {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }

    /// Settings changed.
    pub fn refresh(&mut self) {
        push_options();
        self.shift = config::cur().i("tbm_shift");
        self.last_key.clear();
        self.render();
    }

    pub fn on_snapshot(&mut self) {
        if let Some(s) = SNAP.lock().ok().and_then(|mut s| s.take()) {
            self.snap = Some(s);
        }
        self.render();
        self.keep_on_top();
    }

    /// Stay above everything else in the taskbar: if any of Explorer's own
    /// windows (the icons, Start, widgets, the tray) has been stacked over
    /// the widget, raise it again. Checked with every update (once a
    /// second); it only re-stacks when something actually covers it, so the
    /// taskbar doesn't flicker.
    fn keep_on_top(&mut self) {
        if !self.attached() {
            return;
        }
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
            let mut mine = RECT::default();
            let _ = GetWindowRect(self.hwnd, &mut mine);
            let mut covered = !IsWindowVisible(self.hwnd).as_bool();
            let mut h = GetWindow(self.hwnd, GW_HWNDPREV).unwrap_or_default();
            let mut n = 0;
            while !covered && !h.is_invalid() && n < 64 {
                let mut r = RECT::default();
                // (The island docked in the taskbar is ours: no tug of war with it.)
                let mut pid = 0u32;
                GetWindowThreadProcessId(h, Some(&mut pid));
                if pid != std::process::id() && IsWindowVisible(h).as_bool() && GetWindowRect(h, &mut r).is_ok() && r.left < mine.right && r.right > mine.left && r.top < mine.bottom && r.bottom > mine.top {
                    covered = true;
                }
                h = GetWindow(h, GW_HWNDPREV).unwrap_or_default();
                n += 1;
            }
            if covered {
                let _ = SetWindowPos(self.hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            }
        }
    }

    fn render(&mut self) {
        self.attach();
        if !self.attached() {
            return;
        }
        let Some(snap) = self.snap.clone() else { return };
        let c = config::cur();
        let mut tb = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.taskbar, &mut tb);
        }
        let (mut tbw, mut tbh) = (tb.right - tb.left, tb.bottom - tb.top);
        let mut dpi = unsafe { GetDpiForWindow(self.taskbar) };
        // Developer aid: ISLET_TB_FAKE=w,h,dpi renders for another taskbar shape.
        let fake = std::env::var("ISLET_TB_FAKE").ok().and_then(|v| {
            let p: Vec<i32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            (p.len() == 3).then(|| (p[0], p[1], p[2]))
        });
        if let Some((w, h, d)) = fake {
            tbw = w;
            tbh = h;
            dpi = d as u32;
        }
        if tbw <= 0 || tbh <= 0 {
            return;
        }
        let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
        self.scale = scale;
        self.vertical = tbh > tbw;
        let light = !util::is_dark_taskbar();
        // Tray (Wi-Fi / battery / clock) rectangle, relative to the taskbar.
        let tray = if fake.is_some() { None } else { unsafe {
            let t = FindWindowExW(Some(self.taskbar), None, w!("TrayNotifyWnd"), PCWSTR::null()).unwrap_or_default();
            let mut r = RECT::default();
            if !t.is_invalid() && GetWindowRect(t, &mut r).is_ok() && r.right > r.left {
                Some(RECT { left: r.left - tb.left, top: r.top - tb.top, right: r.right - tb.left, bottom: r.bottom - tb.top })
            } else {
                None
            }
        } };
        let (cols, flat) = build_lines(&snap);
        let text_key: String = flat.iter().map(|l| format!("{}{}", l.value, l.short)).collect::<Vec<_>>().join("|");
        let key = format!(
            "{text_key}|{tbw}x{tbh}|{tray:?}|{dpi}|{light}|{}|{}|{}|{}|{}|{}",
            c.i("tbm_icons"),
            c.i("tbm_layout"),
            c.i("tbm_anchor"),
            self.shift + 10000 * c.i("tbm_edge"),
            c.i("tbm_size"),
            c.s("tbm_order")
        ) + &format!("|{:?}|{}|{}|{}|{}|{}", self.bg.map(|b| b.0), c.i("tbm_bg") + 10 * c.i("glass_tb"), c.i("tbm_bg_h"), c.i("tbm_bg_r"), c.i("tbm_gap"), PAD_KEYS.iter().flatten().map(|k| c.i(k).to_string()).collect::<Vec<_>>().join(","));
        if key == self.last_key {
            return;
        }
        self.last_key = key;

        // Opaque background in the taskbar's own colour (so clicks land on the
        // widget, not the taskbar behind it), or nearly invisible if chosen.
        // A see-through taskbar (Settings → Transparency): no background of
        // our own either, so the taskbar's shows through, in step with it.
        let solid = c.i("tbm_bg") == 0 && !c.b("glass_tb");
        let bgc = if solid { self.taskbar_color(tb, light) } else { 0 };
        let bg = if solid { rgb(bgc, 1.0) } else { rgb(0x000000, 0.004) };
        let lum = |c: u32| (0.299 * ((c >> 16) & 0xff) as f32 + 0.587 * ((c >> 8) & 0xff) as f32 + 0.114 * (c & 0xff) as f32) / 255.0;
        let light_text_bg = if solid { lum(bgc) > 0.55 } else { light };
        let fg = if light_text_bg { rgb(0x1c1c1c, 1.0) } else { rgb(0xffffff, 1.0) };
        let k = c.i("tbm_size").clamp(70, 140) as f32 / 100.0;
        let icons = c.b("tbm_icons");
        let g = gfx::g();
        let tbw_d = tbw as f32 / scale;
        let tbh_d = tbh as f32 / scale;

        if !self.vertical {
            // ---------------- horizontal taskbar ----------------
            let two = match c.i("tbm_layout") {
                1 => false,
                2 => true,
                _ => tbh_d >= 40.0,
            };
            let line_h = if two { ((tbh_d - 6.0) / 2.0).min(17.0) } else { (tbh_d - 8.0).min(18.0) };
            let fsize = (line_h * 0.74 * k).clamp(8.0, 16.0);
            let font = g.font(UI_FONT, fsize, 400, 0);
            let lh = (fsize * 1.34).round();
            let glyph_sz = (lh * 0.86).round();
            let lab_w = |l: &Line| if icons { glyph_sz + 3.0 } else { g.measure(l.label, &font) + 4.0 };
            let pad_x = 4.0;
            let gap = c.i("tbm_gap").clamp(0, 40) as f32 * k;
            // Value width: what it needs now, kept while it was needed recently.
            let now = std::time::Instant::now();
            let mut val_w = |l: &Line| -> f32 {
                let need = g.measure(&l.value, &font).max(g.measure(l.template, &font)).ceil();
                match self.sticky.iter_mut().find(|e| e.0 == l.label) {
                    Some(e) => {
                        if need >= e.1 {
                            e.1 = need;
                            e.2 = now;
                        } else if now.duration_since(e.2).as_secs() >= 8 {
                            e.1 = need;
                            e.2 = now;
                        }
                        e.1
                    }
                    None => {
                        self.sticky.push((l.label, need, now));
                        need
                    }
                }
            };
            // One entry per drawn item: (lines, label width, value width, left pad, right pad).
            struct Item<'a> {
                lines: Vec<&'a Line>,
                lw: f32,
                vw: f32,
                pl: f32,
                pr: f32,
            }
            let mut items: Vec<Item> = Vec::new();
            if two {
                for col in &cols {
                    let mut lines = vec![&col.top];
                    if let Some(b) = &col.bottom {
                        lines.push(b);
                    }
                    let lw = lines.iter().map(|l| lab_w(l)).fold(0.0, f32::max);
                    let vw = lines.iter().map(|l| val_w(l)).fold(0.0, f32::max);
                    let pl = lines.iter().map(|l| pad(l.metric).0).fold(f32::MIN, f32::max);
                    let pr = lines.iter().map(|l| pad(l.metric).2).fold(f32::MIN, f32::max);
                    items.push(Item { lines, lw, vw, pl, pr });
                }
            } else {
                for l in &flat {
                    let (pl, _, pr, _) = pad(l.metric);
                    items.push(Item { lines: vec![l], lw: lab_w(l), vw: val_w(l), pl, pr });
                }
            }
            let item_w = |it: &Item| (it.pl + it.lw + it.vw + it.pr).max(8.0);
            let width_d = pad_x * 2.0 + items.iter().map(item_w).sum::<f32>() + gap * (items.len().saturating_sub(1)) as f32;
            let w_px = (width_d * scale).ceil() as i32;
            let h_px = tbh;
            self.surf.resize(w_px, h_px);
            if !self.surf.begin(scale) {
                return;
            }
            // Background band: a chosen share of the taskbar's height, rounded.
            let bh = (tbh_d * c.i("tbm_bg_h").clamp(30, 100) as f32 / 100.0).max(if two { lh * 2.0 } else { lh } + 2.0).min(tbh_d);
            let rad = c.i("tbm_bg_r").clamp(0, 20) as f32;
            self.surf.fill_rect(rf(0.0, 0.0, width_d, tbh_d), rgb(0x000000, 0.004));
            self.surf.fill_rr(rf(0.0, (tbh_d - bh) / 2.0, width_d, bh), rad.min(bh / 2.0), bg);
            let draw_line = |surf: &Surface, l: &Line, x: f32, y: f32, lw: f32| {
                let (_, pt, _, pb) = pad(l.metric);
                let y = y + pt - pb;
                if icons {
                    draw_glyph(surf, l.glyph, x, y + (lh - glyph_sz) / 2.0, glyph_sz, fg);
                } else {
                    surf.text(l.label, &font, rf(x, y, lw, lh), fg);
                }
                surf.text(&l.value, &font, rf(x + lw, y, 200.0, lh), fg);
            };
            let mut x = pad_x;
            let top2 = (tbh_d - 2.0 * lh) / 2.0;
            let top1 = (tbh_d - lh) / 2.0;
            for it in &items {
                let lx = x + it.pl;
                if it.lines.len() == 2 {
                    draw_line(&self.surf, it.lines[0], lx, top2, it.lw);
                    draw_line(&self.surf, it.lines[1], lx, top2 + lh, it.lw);
                } else {
                    draw_line(&self.surf, it.lines[0], lx, top1, it.lw);
                }
                x += item_w(it) + gap;
            }
            self.surf.end();
            // Position: next to the tray (default), or pinned to the far
            // left / far right end (there, dragging doesn't move it).
            let margin = (6.0 * scale) as i32;
            let edge = (c.i("tbm_edge").clamp(0, 400) as f32 * scale) as i32;
            let x_px = match c.i("tbm_anchor") {
                1 => edge,
                2 => tbw - w_px - edge,
                _ => tray.map(|t| t.left).unwrap_or(tbw - (260.0 * scale) as i32) - w_px - margin + (self.shift as f32 * scale) as i32,
            }
            .clamp(0, (tbw - w_px).max(0));
            self.place(x_px, 0, w_px, h_px);
        } else {
            // ---------------- vertical taskbar (left / right edge) ----------------
            let wide = tbw_d >= 96.0;
            let cell_w = tbw_d - 4.0;
            let vsize = (if wide { 11.5 } else { 10.0 } * k).clamp(8.0, 14.0);
            let font = g.font(UI_FONT, vsize, 600, if wide { 0 } else { 1 });
            let glyph_sz = (13.0 * k).round();
            let cell_h = if wide { (vsize * 1.5).max(glyph_sz + 4.0) } else { glyph_sz + vsize * 1.35 + 6.0 };
            let total_h = 6.0 + flat.iter().map(|l| { let (_, pt, _, pb) = pad(l.metric); (cell_h + pt + pb).max(4.0) }).sum::<f32>();
            let w_px = tbw;
            let h_px = (total_h * scale).ceil() as i32;
            self.surf.resize(w_px, h_px);
            if !self.surf.begin(scale) {
                return;
            }
            // On a side taskbar the band's "height" is its thickness across.
            let bw = (tbw_d * c.i("tbm_bg_h").clamp(30, 100) as f32 / 100.0).max(24.0).min(tbw_d);
            let rad = c.i("tbm_bg_r").clamp(0, 20) as f32;
            self.surf.fill_rect(rf(0.0, 0.0, tbw_d, total_h), rgb(0x000000, 0.004));
            self.surf.fill_rr(rf((tbw_d - bw) / 2.0, 0.0, bw, total_h), rad.min(bw / 2.0), bg);
            let mut y = 3.0;
            for l in &flat {
                let (pl, pt, pr, pb) = pad(l.metric);
                let ox = pl - pr;
                y += pt;
                if wide {
                    let gx = 8.0 + ox;
                    if icons {
                        draw_glyph(&self.surf, l.glyph, gx, y + (cell_h - glyph_sz) / 2.0, glyph_sz, fg);
                    } else {
                        self.surf.text(l.label, &font, rf(gx, y, 30.0, cell_h), fg);
                    }
                    self.surf.text(&l.value, &font, rf(gx + if icons { glyph_sz + 5.0 } else { 32.0 }, y, cell_w - 40.0, cell_h), fg);
                } else {
                    if icons {
                        draw_glyph(&self.surf, l.glyph, (tbw_d - glyph_sz) / 2.0 + ox, y + 2.0, glyph_sz, fg);
                    } else {
                        let lf = g.font(UI_FONT, vsize * 0.9, 700, 1);
                        self.surf.text(l.label, &lf, rf(2.0 + ox, y, cell_w, glyph_sz + 2.0), fg);
                    }
                    self.surf.text(&l.short, &font, rf(2.0 + ox, y + glyph_sz + 3.0, cell_w, vsize * 1.35), fg);
                }
                y += cell_h + pb;
            }
            self.surf.end();
            let margin = (6.0 * scale) as i32;
            let edge = (c.i("tbm_edge").clamp(0, 400) as f32 * scale) as i32;
            // Side taskbar: far left = top end, far right = bottom end.
            let y_px = match c.i("tbm_anchor") {
                1 => edge,
                2 => tbh - h_px - edge,
                _ => tray.map(|t| t.top).unwrap_or(tbh - (300.0 * scale) as i32) - h_px - margin + (self.shift as f32 * scale) as i32,
            }
            .clamp(0, (tbh - h_px).max(0));
            self.place(0, y_px, w_px, h_px);
        }
    }

    /// The taskbar's colour right next to the widget (screen pixels), so the
    /// widget's background blends in. Sampled at most every 3 s.
    fn taskbar_color(&mut self, tb: RECT, light: bool) -> u32 {
        if let Some((c, t)) = self.bg {
            if t.elapsed().as_secs() < 3 {
                return c;
            }
        }
        let fallback = if light { 0xf3f3f3 } else { 0x202020 };
        let c = unsafe {
            let mut me = RECT::default();
            let _ = GetWindowRect(self.hwnd, &mut me);
            let have_me = me.right - me.left > 2;
            // Points on the bare taskbar just outside the widget.
            let pts: Vec<(i32, i32)> = if self.vertical {
                let y = if have_me && me.top - 6 > tb.top { me.top - 6 } else { me.bottom + 6 };
                vec![(tb.left + 3, y), (tb.right - 4, y), ((tb.left + tb.right) / 2, y)]
            } else {
                let x = if have_me && me.left - 6 > tb.left { me.left - 6 } else { me.right + 6 };
                vec![(x, tb.top + 3), (x, tb.bottom - 4), (x - 3, tb.top + 3)]
            };
            use windows::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC};
            let dc = GetDC(None);
            let mut got: Vec<u32> = Vec::new();
            for (x, y) in pts {
                let p = GetPixel(dc, x, y);
                if p.0 != 0xFFFF_FFFF {
                    // COLORREF is 0x00BBGGRR.
                    let v = p.0;
                    got.push(((v & 0xff) << 16) | (v & 0xff00) | ((v >> 16) & 0xff));
                }
            }
            ReleaseDC(None, dc);
            // Most common sample (an icon or hover highlight can spoil one).
            got.iter().copied().max_by_key(|c| got.iter().filter(|d| *d == c).count()).unwrap_or(fallback)
        };
        self.bg = Some((c, std::time::Instant::now()));
        c
    }

    fn place(&mut self, x: i32, y: i32, w: i32, h: i32) {
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOP), x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
        self.surf.present_child(self.hwnd);
        self.status = format!("Showing in the taskbar ({}×{} px)", w, h);
    }

    fn mouse(&mut self, msg: u32) {
        unsafe {
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            let along = if self.vertical { p.y } else { p.x };
            match msg {
                WM_LBUTTONDOWN => {
                    self.drag = Some((along, self.shift));
                    self.clicked = true;
                    SetCapture(self.hwnd);
                }
                WM_MOUSEMOVE => {
                    if let Some((start, shift0)) = self.drag {
                        if (along - start).abs() > 4 {
                            self.clicked = false;
                        }
                        if self.clicked {
                            return;
                        }
                        let ns = shift0 + ((along - start) as f32 / self.scale).round() as i32;
                        if ns != self.shift {
                            self.shift = ns;
                            self.render();
                        }
                    }
                }
                WM_LBUTTONUP => {
                    let click = self.clicked;
                    self.clicked = false;
                    let _ = ReleaseCapture();
                    // A plain click opens the monitor's settings; dragging moves it.
                    if click {
                        crate::open_settings(settings_tab());
                    }
                }
                WM_CAPTURECHANGED => {
                    if self.drag.take().is_some() {
                        let s = self.shift;
                        config::update(|c| c.set("tbm_shift", s));
                    }
                }
                WM_LBUTTONDBLCLK => crate::open_settings(settings_tab()),
                WM_RBUTTONUP => self.menu(),
                _ => {}
            }
        }
    }

    fn menu(&mut self) {
        unsafe {
            let Ok(m) = CreatePopupMenu() else { return };
            let _ = AppendMenuW(m, MF_STRING, 1, &HSTRING::from(crate::i18n::tr("Taskbar monitor settings…")));
            let _ = AppendMenuW(m, MF_STRING, 2, &HSTRING::from(crate::i18n::tr("Reset position")));
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(m, MF_STRING, 3, &HSTRING::from(crate::i18n::tr("Hide taskbar monitor")));
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            let host = crate::app().host;
            let _ = SetForegroundWindow(host);
            let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, p.x, p.y, None, host, None).0;
            let _ = DestroyMenu(m);
            match cmd {
                1 => crate::open_settings(settings_tab()),
                2 => {
                    config::update(|c| c.set("tbm_shift", 0));
                    self.refresh();
                }
                3 => {
                    config::update(|c| c.set("tbm", 0));
                    crate::request_apply();
                }
                _ => {}
            }
        }
    }
}

/// Index of the "Taskbar" tab in the settings window.
pub fn settings_tab() -> usize {
    4
}
