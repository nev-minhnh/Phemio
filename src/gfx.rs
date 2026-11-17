//! Tiny Direct2D/DirectWrite/WIC layer.
//!
//! Every window draws into a 32-bit DIB with a Direct2D DC render target and
//! is pushed to the screen with UpdateLayeredWindowIndirect.
//!
//! Where it can be (Settings → General → Draw with the graphics card), the
//! frame is drawn **on the graphics card**: a Direct2D device context draws
//! into a Direct3D texture, and the finished frame — or just the part that
//! was drawn — is copied from there into the DIB. Measured on a real PC
//! (integrated Radeon, 150 % scaling, a 120 Hz display): the pieces of a
//! frame take 0.1–1 ms there whatever the window's size, and bringing a
//! frame back about a millisecond, where the software DC target needed
//! 1.3 ms for an *empty* open island and over 9 ms for an empty Settings
//! window. (A *hardware DC render target*, tried in 0.39.0, was no faster
//! than the software one: its cost is the DC, not the drawing.)
//!
//! The **software** DC target is the fallback, always there: when no
//! graphics card can be used, when a frame fails there, when it turns out
//! slower on this PC (both are timed once, see `calibrate`), or when the
//! setting is off. Everything after drawing — the DIB, the layered windows,
//! partial redraws — is the same for both, so the two can be swapped while
//! the app runs.

use std::cell::RefCell;
use std::ffi::c_void;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISurface};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub struct Gfx {
    pub d2d: ID2D1Factory,
    pub dw: IDWriteFactory,
    pub wic: IWICImagingFactory,
}

thread_local! {
    static GFX: &'static Gfx = Box::leak(Box::new(Gfx::new().expect("graphics init failed")));
}

// ---------------------------------------------------------------------------
// Graphics card or processor
// ---------------------------------------------------------------------------

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};

/// Targets are hardware ones now.
static HARDWARE: AtomicBool = AtomicBool::new(false);
/// Bumped whenever every target (and everything made with one: brushes,
/// uploaded pictures) has to be made again: the kind changed, or the GPU
/// was reset.
static GEN: AtomicU32 = AtomicU32::new(1);
/// Hardware frames in a row that failed.
static FAILS: AtomicU32 = AtomicU32::new(0);
/// The kind was chosen (first surface made).
static CHOSEN: AtomicBool = AtomicBool::new(false);

/// Drawing with the graphics card right now.
pub fn hardware() -> bool {
    HARDWARE.load(Relaxed)
}

/// "graphics card" / "processor", and what a frame costs (for the log).
pub fn describe() -> String {
    let (n, us) = (COST_N.load(Relaxed), COST_US.load(Relaxed));
    format!("{}{}", if hardware() { "graphics card" } else { "processor" }, if n > 0 { format!(", {:.1} ms a frame", us as f64 / n as f64 / 1000.0) } else { String::new() })
}

/// Frames drawn (and the µs they took) since `take_cost`.
static COST_N: AtomicU32 = AtomicU32::new(0);
static COST_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

thread_local! {
    static CALIB: RefCell<Calib> = const { RefCell::new(Calib { phase: 0, skip: 0, hw: Vec::new(), sw: Vec::new() }) };
}

/// Start counting frames again (after reading `describe`).
pub fn take_cost() {
    COST_N.store(0, Relaxed);
    COST_US.store(0, Relaxed);
}

/// What the user asked for: Settings → General → Draw with the graphics
/// card (on unless switched off), and ISLET_GPU=0/1 for developers.
fn wanted() -> bool {
    match std::env::var("ISLET_GPU").ok().as_deref() {
        Some("0") => false,
        Some(_) => true,
        None => crate::config::cur().b("gpu"),
    }
}

fn set_kind(hw: bool, why: &str) {
    if HARDWARE.swap(hw, Relaxed) != hw || !CHOSEN.load(Relaxed) {
        GEN.fetch_add(1, Relaxed);
    }
    FAILS.store(0, Relaxed);
    crate::util::log(&format!("render: drawing with the {} ({why})", if hw { "graphics card" } else { "processor" }));
}

/// The setting changed (or at start): use what it says. Windows redraw
/// themselves with the new kind the next time they draw.
pub fn apply_setting() {
    thread_local! {
        static LAST: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    }
    let want = wanted();
    if LAST.replace(Some(want)) == Some(want) {
        return;
    }
    if !CHOSEN.swap(true, Relaxed) {
        HARDWARE.store(!want, Relaxed); // (so set_kind sees a change)
    }
    set_kind(want, if want { "Settings: Draw with the graphics card" } else { "Settings: graphics card off" });
    // (ISLET_GPU_NOCAL: developer aid — no timing, the kind asked for stays.)
    let time_it = want && std::env::var("ISLET_GPU_NOCAL").is_err();
    CALIB.with(|c| *c.borrow_mut() = Calib { phase: if time_it { 1 } else { 0 }, skip: 24, hw: Vec::new(), sw: Vec::new() });
}

/// Timing both kinds, once: a hardware target isn't faster on every PC
/// (the finished frame has to come back from the GPU into the DIB), so the
/// first big frames are timed with the graphics card, the next ones with
/// the processor, and the faster kind stays.
struct Calib {
    /// 0 nothing to do · 1 timing hardware · 2 timing software · 3 hardware
    /// kept · 4 software kept (hardware was slower here).
    phase: u8,
    /// Frames still to leave out (the first ones after a change make
    /// everything again — and the very first ones also fill the font and
    /// picture caches, whichever kind draws them).
    skip: u32,
    hw: Vec<f32>,
    sw: Vec<f32>,
}

const CALIB_FRAMES: usize = 40;

/// A whole frame of `px` pixels took `us` to draw.
fn calibrate(px: i64, us: u64) {
    COST_N.fetch_add(1, Relaxed);
    COST_US.fetch_add(us, Relaxed);
    // (Only frames big enough to mean something: the open island, Settings.)
    if px < 120_000 {
        return;
    }
    let per_mp = us as f32 / (px as f32 / 1e6);
    let done = CALIB.with(|c| {
        let mut c = c.borrow_mut();
        if c.phase != 1 && c.phase != 2 {
            return None;
        }
        if c.skip > 0 {
            c.skip -= 1;
            return None;
        }
        if c.phase == 1 {
            c.hw.push(per_mp);
            if c.hw.len() >= CALIB_FRAMES {
                c.phase = 2;
                c.skip = 8;
                return Some(0);
            }
        } else {
            c.sw.push(per_mp);
            if c.sw.len() >= CALIB_FRAMES {
                let med = |v: &mut Vec<f32>| {
                    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    v[v.len() / 2]
                };
                let (h, s) = (med(&mut c.hw), med(&mut c.sw));
                // (The graphics card stays unless the processor is clearly quicker.)
                let keep_hw = h <= s * 1.3;
                c.phase = if keep_hw { 3 } else { 4 };
                crate::util::log(&format!("render: timed both — graphics card {:.1} ms, processor {:.1} ms for a million pixels", h / 1000.0, s / 1000.0));
                return Some(if keep_hw { 1 } else { 2 });
            }
        }
        None
    });
    match done {
        Some(0) => set_kind(false, "timing the processor for comparison"),
        Some(1) => set_kind(true, "it is the faster one here"),
        Some(2) => set_kind(false, "it is the faster one here"),
        _ => {}
    }
}

/// A hardware frame failed (`recreate`: the GPU was reset — make everything
/// again and carry on). Three in a row: the processor draws from now on.
fn hardware_failed(e: &Error, recreate: bool) {
    let n = FAILS.fetch_add(1, Relaxed) + 1;
    if n >= 3 {
        CALIB.with(|c| c.borrow_mut().phase = 4);
        set_kind(false, &format!("the graphics card failed {n} times in a row: {e}"));
    } else {
        if n == 1 {
            crate::util::log(&format!("render: a frame failed on the graphics card ({e}){}", if recreate { " — starting it again" } else { "" }));
        }
        GEN.fetch_add(1, Relaxed);
    }
    // (A new device next time: this one may be gone — driver reset, the
    // card removed, the session locked.)
    GPU.with(|g| *g.borrow_mut() = None);
}

/// Drawing code that leaves clips on (or takes one off too many): said once
/// per kind and window size, and put right on the spot.
fn note_unbalanced(what: &str, w: i32, h: i32) {
    thread_local! {
        static SAID: RefCell<Vec<(usize, i32, i32)>> = const { RefCell::new(Vec::new()) };
    }
    let key = (what.len(), w, h);
    let new = SAID.with(|s| {
        let mut s = s.borrow_mut();
        if s.contains(&key) || s.len() > 16 {
            false
        } else {
            s.push(key);
            true
        }
    });
    if new {
        crate::util::log(&format!("render: {what} (a {w}×{h} surface) — put right"));
    }
}

/// The graphics card, for this thread's surfaces: a Direct3D device and
/// the Direct2D device on it (pictures and brushes made with one surface's
/// context can be used with every other's).
#[derive(Clone)]
struct Gpu {
    dev: ID3D11Device,
    ctx: ID3D11DeviceContext,
    d2: ID2D1Device,
}

thread_local! {
    static GPU: RefCell<Option<Gpu>> = const { RefCell::new(None) };
}

fn gpu() -> Result<Gpu> {
    if let Some(g) = GPU.with(|g| g.borrow().clone()) {
        return Ok(g);
    }
    unsafe {
        let mut dev: Option<ID3D11Device> = None;
        let mut ctx: Option<ID3D11DeviceContext> = None;
        // (A real graphics card only: Direct3D on the processor would be the
        // slowest of all ways to draw.)
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut dev), None, Some(&mut ctx))?;
        let (dev, ctx) = (dev.ok_or_else(|| Error::from(E_FAIL))?, ctx.ok_or_else(|| Error::from(E_FAIL))?);
        let dxgi: IDXGIDevice = dev.cast()?;
        let f1: ID2D1Factory1 = g().d2d.cast()?;
        let d2 = f1.CreateDevice(&dxgi)?;
        let name = dxgi.GetAdapter().and_then(|a| a.GetDesc()).map(|d| String::from_utf16_lossy(&d.Description).split('\0').next().unwrap_or("").to_string()).unwrap_or_default();
        crate::util::log(&format!("render: graphics card: {name}"));
        let g = Gpu { dev, ctx, d2 };
        GPU.with(|x| *x.borrow_mut() = Some(g.clone()));
        Ok(g)
    }
}

/// A surface's picture on the graphics card: Direct2D draws into `tex`
/// there; the finished frame (or the part of it that was drawn) is copied
/// to `staging`, which the processor can read, and from there into the
/// surface's DIB — measured, about a millisecond — so that everything after
/// drawing (layered windows, their see-through parts taking no clicks,
/// partial updates) is exactly as with the software target.
struct Card {
    gpu: Gpu,
    dcx: ID2D1DeviceContext,
    tex: Option<ID3D11Texture2D>,
    staging: Option<ID3D11Texture2D>,
    target: Option<ID2D1Bitmap1>,
    w: i32,
    h: i32,
    /// Nothing has been drawn into `tex` yet (it doesn't hold the DIB's picture).
    fresh: bool,
}

impl Card {
    /// The textures, `w`×`h`.
    unsafe fn ensure(&mut self, w: i32, h: i32) -> Result<()> {
        if self.tex.is_some() && self.w == w && self.h == h {
            return Ok(());
        }
        self.dcx.SetTarget(None);
        self.target = None;
        self.tex = None;
        self.staging = None;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w as u32,
            Height: h as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut tex: Option<ID3D11Texture2D> = None;
        self.gpu.dev.CreateTexture2D(&desc, None, Some(&mut tex))?;
        let tex = tex.ok_or_else(|| Error::from(E_FAIL))?;
        let mut staging: Option<ID3D11Texture2D> = None;
        self.gpu.dev.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Usage: D3D11_USAGE_STAGING, BindFlags: 0, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32, ..desc }, None, Some(&mut staging))?;
        let surf: IDXGISurface = tex.cast()?;
        let bp = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        self.target = Some(self.dcx.CreateBitmapFromDxgiSurface(&surf, Some(&bp))?);
        self.tex = Some(tex);
        self.staging = Some(staging.ok_or_else(|| Error::from(E_FAIL))?);
        self.w = w;
        self.h = h;
        self.fresh = true;
        Ok(())
    }

    /// `r` of the picture, from the graphics card into the DIB at `bits`.
    unsafe fn read_back(&self, r: RECT, bits: *mut u8) -> Result<()> {
        let (Some(tex), Some(staging)) = (self.tex.as_ref(), self.staging.as_ref()) else { return Err(Error::from(E_FAIL)) };
        let r = RECT { left: r.left.clamp(0, self.w), top: r.top.clamp(0, self.h), right: r.right.clamp(0, self.w), bottom: r.bottom.clamp(0, self.h) };
        if r.right <= r.left || r.bottom <= r.top || bits.is_null() {
            return Ok(());
        }
        let b = D3D11_BOX { left: r.left as u32, top: r.top as u32, front: 0, right: r.right as u32, bottom: r.bottom as u32, back: 1 };
        self.gpu.ctx.CopySubresourceRegion(staging, 0, r.left as u32, r.top as u32, 0, tex, 0, Some(&b));
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        self.gpu.ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
        if !m.pData.is_null() {
            let n = ((r.right - r.left) * 4) as usize;
            for y in r.top..r.bottom {
                let src = (m.pData as *const u8).add(y as usize * m.RowPitch as usize + r.left as usize * 4);
                let dst = bits.add((y as usize * self.w as usize + r.left as usize) * 4);
                std::ptr::copy_nonoverlapping(src, dst, n);
            }
        }
        self.gpu.ctx.Unmap(staging, 0);
        Ok(())
    }

    /// The DIB's picture, up to the graphics card (before drawing a part
    /// of a frame into a texture that hasn't seen the rest).
    unsafe fn upload(&mut self, bits: *const u8) {
        if let (Some(tex), false) = (self.tex.as_ref(), bits.is_null()) {
            self.gpu.ctx.UpdateSubresource(tex, 0, None, bits as *const c_void, (self.w * 4) as u32, 0);
            self.fresh = false;
        }
    }
}

/// What a surface draws with: a software DC render target (`rt`), or a
/// device context on the graphics card (`card`).
struct Target {
    rt: Option<ID2D1DCRenderTarget>,
    card: Option<Card>,
    base: ID2D1RenderTarget,
    brush: ID2D1SolidColorBrush,
}

fn make_target(hw: bool) -> Result<Target> {
    unsafe {
        if hw {
            // (ISLET_GPU_FAIL: developer aid — as if no graphics card could be used.)
            if std::env::var("ISLET_GPU_FAIL").is_ok() {
                return Err(Error::from(E_FAIL));
            }
            let gpu = gpu()?;
            let dcx = gpu.d2.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let base: ID2D1RenderTarget = dcx.cast()?;
            base.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let brush = base.CreateSolidColorBrush(&rgb(0, 1.0), None)?;
            return Ok(Target { rt: None, card: Some(Card { gpu, dcx, tex: None, staging: None, target: None, w: 0, h: 0, fresh: true }), base, brush });
        }
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let rt = g().d2d.CreateDCRenderTarget(&props)?;
        let base: ID2D1RenderTarget = rt.cast()?;
        base.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        let brush = base.CreateSolidColorBrush(&rgb(0, 1.0), None)?;
        Ok(Target { rt: Some(rt), card: None, base, brush })
    }
}

/// The target for a new surface (or one whose kind changed): on the
/// graphics card if that is the kind now and it can be made, else software.
fn current_target() -> Result<(Target, bool, u32)> {
    if !CHOSEN.load(Relaxed) {
        apply_setting();
    }
    if hardware() {
        match make_target(true) {
            Ok(t) => return Ok((t, true, GEN.load(Relaxed))),
            Err(e) => {
                CALIB.with(|c| c.borrow_mut().phase = 4);
                set_kind(false, &format!("the graphics card can't be used: {e}"));
            }
        }
    }
    Ok((make_target(false)?, false, GEN.load(Relaxed)))
}

/// The UI-thread graphics factories.
pub fn g() -> &'static Gfx {
    GFX.with(|g| *g)
}

impl Gfx {
    fn new() -> Result<Self> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let wic: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            Ok(Gfx { d2d, dw, wic })
        }
    }

    /// Create a single-line text format. `align`: 0 left, 1 center, 2 right.
    pub fn font(&self, family: &str, size: f32, weight: i32, align: u8) -> IDWriteTextFormat {
        self.font_ex(family, size, weight, align, false)
    }

    pub fn font_ex(&self, family: &str, size: f32, weight: i32, align: u8, italic: bool) -> IDWriteTextFormat {
        unsafe {
            let fam = HSTRING::from(family);
            let f = self
                .dw
                .CreateTextFormat(
                    &fam,
                    None,
                    DWRITE_FONT_WEIGHT(weight),
                    if italic { DWRITE_FONT_STYLE_ITALIC } else { DWRITE_FONT_STYLE_NORMAL },
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!(""),
                )
                .expect("CreateTextFormat");
            let _ = f.SetTextAlignment(match align {
                1 => DWRITE_TEXT_ALIGNMENT_CENTER,
                2 => DWRITE_TEXT_ALIGNMENT_TRAILING,
                _ => DWRITE_TEXT_ALIGNMENT_LEADING,
            });
            let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            if let Ok(sign) = self.dw.CreateEllipsisTrimmingSign(&f) {
                let trim = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                let _ = f.SetTrimming(&trim, &sign);
            }
            f
        }
    }

    /// Word-wrapping, top-aligned multi-line format.
    pub fn font_wrap_ex(&self, family: &str, size: f32, weight: i32, align: u8, italic: bool) -> IDWriteTextFormat {
        let f = self.font_ex(family, size, weight, align, italic);
        unsafe {
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
            let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
        }
        f
    }

    /// A wrapped text layout with styled stretches (bold, italic…), for
    /// finding characters under the pointer and drawing a selection.
    pub fn rich_layout(&self, s: &str, f: &IDWriteTextFormat, w: f32, styles: &[(u32, u32, bool, bool, bool, bool)]) -> Option<IDWriteTextLayout> {
        let t: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let l = self.dw.CreateTextLayout(&t, f, w.max(1.0), 4000.0).ok()?;
            for &(start, len, b, i, _, _) in styles {
                let range = DWRITE_TEXT_RANGE { startPosition: start, length: len };
                if b {
                    let _ = l.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, range);
                }
                if i {
                    let _ = l.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, range);
                }
            }
            Some(l)
        }
    }

    /// A layout's lines (text start, length without the line break, top,
    /// height) and its clusters (text length, width), from DirectWrite's
    /// metrics — so finding characters works the same everywhere.
    fn lines_clusters(l: &IDWriteTextLayout) -> (Vec<(u32, u32, f32, f32)>, Vec<(u32, f32)>) {
        unsafe {
            let mut n = 0u32;
            let _ = l.GetLineMetrics(None, &mut n);
            let mut lm = vec![DWRITE_LINE_METRICS::default(); n as usize];
            if n == 0 || l.GetLineMetrics(Some(&mut lm), &mut n).is_err() {
                return (vec![], vec![]);
            }
            let mut lines = Vec::new();
            let (mut start, mut top) = (0u32, 0.0f32);
            for m in &lm {
                lines.push((start, m.length - m.newlineLength, top, m.height));
                start += m.length;
                top += m.height;
            }
            let mut c = 0u32;
            let _ = l.GetClusterMetrics(None, &mut c);
            let mut cm = vec![DWRITE_CLUSTER_METRICS::default(); c as usize];
            if c == 0 || l.GetClusterMetrics(Some(&mut cm), &mut c).is_err() {
                return (lines, vec![]);
            }
            (lines, cm.iter().map(|m| (m.length as u32, m.width)).collect())
        }
    }

    /// x of text position `pos` (on its line).
    fn x_of(clusters: &[(u32, f32)], pos: u32) -> f32 {
        let (mut at, mut x) = (0u32, 0.0f32);
        for &(len, w) in clusters {
            if at >= pos {
                break;
            }
            at += len;
            x += w;
        }
        x
    }

    /// The character (UTF-16 position, before or after it) at (x, y) in a layout.
    pub fn hit_pos(l: &IDWriteTextLayout, x: f32, y: f32) -> u32 {
        let (lines, clusters) = Self::lines_clusters(l);
        let Some(&(ls, ll, _, _)) = lines.iter().find(|(_, _, t, h)| y < t + h).or(lines.last()) else { return 0 };
        // Where this line starts, in the clusters' running width.
        let base = Self::x_of(&clusters, ls);
        let (mut at, mut acc) = (0u32, 0.0f32);
        for &(len, w) in &clusters {
            if at + len <= ls {
                at += len;
                acc += w;
                continue;
            }
            if at >= ls + ll {
                break;
            }
            if x < acc - base + w / 2.0 {
                return at;
            }
            at += len;
            acc += w;
        }
        ls + ll
    }

    /// The boxes covering characters `start..start+len` of a layout.
    pub fn range_rects(l: &IDWriteTextLayout, start: u32, len: u32) -> Vec<(f32, f32, f32, f32)> {
        let (lines, clusters) = Self::lines_clusters(l);
        let end = start + len;
        let mut out = Vec::new();
        for &(ls, ll, top, h) in &lines {
            let (a, b) = (start.max(ls), end.min(ls + ll));
            if a >= b {
                // (An empty stretch of a line break inside the selection.)
                if start <= ls && end > ls + ll && ll == 0 {
                    out.push((0.0, top, 4.0, h));
                }
                continue;
            }
            let base = Self::x_of(&clusters, ls);
            let x0 = Self::x_of(&clusters, a) - base;
            let x1 = Self::x_of(&clusters, b) - base;
            out.push((x0, top, (x1 - x0).max(1.0), h));
        }
        out
    }

    /// Height of wrapped text at a given width.
    pub fn measure_h(&self, s: &str, f: &IDWriteTextFormat, width: f32) -> f32 {
        let w: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            if let Ok(l) = self.dw.CreateTextLayout(&w, f, width, 2000.0) {
                let mut m = DWRITE_TEXT_METRICS::default();
                if l.GetMetrics(&mut m).is_ok() {
                    return m.height;
                }
            }
        }
        20.0
    }

    /// Width of a string in DIPs.
    pub fn measure(&self, s: &str, f: &IDWriteTextFormat) -> f32 {
        let w: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            if let Ok(l) = self.dw.CreateTextLayout(&w, f, 4000.0, 200.0) {
                let mut m = DWRITE_TEXT_METRICS::default();
                if l.GetMetrics(&mut m).is_ok() {
                    return m.widthIncludingTrailingWhitespace;
                }
            }
        }
        0.0
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

pub const UI_FONT: &str = "Segoe UI Variable Text";
pub const DISPLAY_FONT: &str = "Segoe UI Variable Display";
pub const ICON_FONT: &str = "Segoe Fluent Icons";

pub fn rgb(hex: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a,
    }
}

pub fn with_alpha(mut c: D2D1_COLOR_F, a: f32) -> D2D1_COLOR_F {
    c.a *= a;
    c
}

pub fn rf(x: f32, y: f32, w: f32, h: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }
}

pub fn pt(x: f32, y: f32) -> windows_numerics::Vector2 {
    windows_numerics::Vector2 { X: x, Y: y }
}

pub fn inside(r: &D2D_RECT_F, x: f32, y: f32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

// ---------------------------------------------------------------------------
// Images (decoded once to premultiplied BGRA, uploaded lazily)
// ---------------------------------------------------------------------------

pub struct Image {
    pub w: u32,
    pub h: u32,
    px: Vec<u8>,
    /// Uploaded for the targets of this generation (see `GEN`).
    cache: RefCell<Option<(u32, ID2D1Bitmap)>>,
}

impl Image {
    /// Decode any WIC source, center-crop to square when `square`, scale to fit
    /// `max` px and optionally round the corners (`round` = radius / size).
    pub fn from_source(src: &IWICBitmapSource, max: u32, square: bool, round: f32) -> Result<Image> {
        let g = g();
        unsafe {
            let (mut w, mut h) = (0u32, 0u32);
            src.GetSize(&mut w, &mut h)?;
            if w == 0 || h == 0 {
                return Err(E_FAIL.into());
            }
            let mut cur: IWICBitmapSource = src.clone();
            if square && w != h {
                let s = w.min(h);
                let clip = g.wic.CreateBitmapClipper()?;
                let r = WICRect { X: ((w - s) / 2) as i32, Y: ((h - s) / 2) as i32, Width: s as i32, Height: s as i32 };
                clip.Initialize(&cur, &r)?;
                cur = clip.cast()?;
                w = s;
                h = s;
            }
            let k = (max as f32 / w.max(h) as f32).min(1.0);
            let (tw, th) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
            if tw != w || th != h {
                let sc = g.wic.CreateBitmapScaler()?;
                sc.Initialize(&cur, tw, th, WICBitmapInterpolationModeFant)?;
                cur = sc.cast()?;
            }
            let conv = g.wic.CreateFormatConverter()?;
            conv.Initialize(&cur, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
            let stride = tw * 4;
            let mut px = vec![0u8; (stride * th) as usize];
            conv.CopyPixels(std::ptr::null(), stride, &mut px)?;
            if round > 0.0 {
                round_corners(&mut px, tw, th, round * tw.min(th) as f32);
            }
            Ok(Image { w: tw, h: th, px, cache: RefCell::new(None) })
        }
    }

    pub fn from_bytes(bytes: &[u8], max: u32, round: f32) -> Result<Image> {
        let g = g();
        unsafe {
            let stream = g.wic.CreateStream()?;
            stream.InitializeFromMemory(bytes)?;
            let dec = g.wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
            let frame = dec.GetFrame(0)?;
            Image::from_source(&frame.cast()?, max, true, round)
        }
    }

    /// A soft, blurred square made from the picture (to fill the space around
    /// a wide picture): shrunk to a few pixels, then smoothly enlarged.
    pub fn blurred_square(bytes: &[u8], max: u32, round: f32) -> Result<Image> {
        let g = g();
        unsafe {
            let stream = g.wic.CreateStream()?;
            stream.InitializeFromMemory(bytes)?;
            let dec = g.wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
            let frame = dec.GetFrame(0)?;
            let src: IWICBitmapSource = frame.cast()?;
            let (mut w, mut h) = (0u32, 0u32);
            src.GetSize(&mut w, &mut h)?;
            if w == 0 || h == 0 {
                return Err(E_FAIL.into());
            }
            let s = w.min(h);
            let clip = g.wic.CreateBitmapClipper()?;
            clip.Initialize(&src, &WICRect { X: ((w - s) / 2) as i32, Y: ((h - s) / 2) as i32, Width: s as i32, Height: s as i32 })?;
            // A few pixels, then enlarged smoothly by hand (bilinear, the same on
            // every machine) — that's the blur.
            let tiny = Image::from_source(&clip.cast()?, 6, true, 0.0)?;
            let (tw, th) = (tiny.w as usize, tiny.h as usize);
            let n = max.max(8) as usize;
            let mut px = vec![0u8; n * n * 4];
            for y in 0..n {
                let fy = ((y as f32 + 0.5) / n as f32 * th as f32 - 0.5).clamp(0.0, (th - 1) as f32);
                let (y0, ty) = (fy.floor() as usize, fy.fract());
                let y1 = (y0 + 1).min(th - 1);
                for x in 0..n {
                    let fx = ((x as f32 + 0.5) / n as f32 * tw as f32 - 0.5).clamp(0.0, (tw - 1) as f32);
                    let (x0, tx) = (fx.floor() as usize, fx.fract());
                    let x1 = (x0 + 1).min(tw - 1);
                    let (sx, sy) = (tx, ty);
                    for c in 0..4 {
                        let at = |xx: usize, yy: usize| tiny.px[(yy * tw + xx) * 4 + c] as f32;
                        let top = at(x0, y0) * (1.0 - sx) + at(x1, y0) * sx;
                        let bot = at(x0, y1) * (1.0 - sx) + at(x1, y1) * sx;
                        px[(y * n + x) * 4 + c] = (top * (1.0 - sy) + bot * sy).round() as u8;
                    }
                }
            }
            if round > 0.0 {
                round_corners(&mut px, n as u32, n as u32, round * n as f32);
            }
            Ok(Image { w: n as u32, h: n as u32, px, cache: RefCell::new(None) })
        }
    }

    /// Like `from_bytes`, keeping the picture's shape (no square crop).
    pub fn from_bytes_fit(bytes: &[u8], max: u32, round: f32) -> Result<Image> {
        let g = g();
        unsafe {
            let stream = g.wic.CreateStream()?;
            stream.InitializeFromMemory(bytes)?;
            let dec = g.wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
            let frame = dec.GetFrame(0)?;
            Image::from_source(&frame.cast()?, max, false, round)
        }
    }

    pub fn from_hbitmap(hbmp: HBITMAP, max: u32) -> Result<Image> {
        let g = g();
        unsafe {
            let b = g.wic.CreateBitmapFromHBITMAP(hbmp, windows::Win32::Graphics::Gdi::HPALETTE::default(), WICBitmapUsePremultipliedAlpha)?;
            Image::from_source(&b.cast()?, max, false, 0.0)
        }
    }

    pub fn from_hicon(icon: HICON, max: u32) -> Result<Image> {
        let g = g();
        unsafe {
            let b = g.wic.CreateBitmapFromHICON(icon)?;
            Image::from_source(&b.cast()?, max, false, 0.0)
        }
    }

    pub fn pixels(&self) -> &[u8] {
        &self.px
    }

    /// True when all pixels are fully transparent (shell sometimes returns that).
    pub fn is_blank(&self) -> bool {
        !self.px.chunks_exact(4).any(|p| p[3] != 0)
    }

    fn bitmap(&self, rt: &ID2D1RenderTarget) -> Option<ID2D1Bitmap> {
        let gen = GEN.load(Relaxed);
        if let Some((g, b)) = self.cache.borrow().as_ref() {
            if *g == gen {
                return Some(b.clone());
            }
        }
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        let b = unsafe {
            rt.CreateBitmap(
                D2D_SIZE_U { width: self.w, height: self.h },
                Some(self.px.as_ptr() as *const c_void),
                self.w * 4,
                &props,
            )
            .ok()?
        };
        *self.cache.borrow_mut() = Some((gen, b.clone()));
        Some(b)
    }
}

fn round_corners(px: &mut [u8], w: u32, h: u32, r: f32) {
    let r = r.max(1.0);
    for y in 0..h {
        for x in 0..w {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let cx = if fx < r { r } else if fx > w as f32 - r { w as f32 - r } else { fx };
            let cy = if fy < r { r } else if fy > h as f32 - r { h as f32 - r } else { fy };
            let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            let cov = (r - d + 0.5).clamp(0.0, 1.0);
            if cov < 1.0 {
                let i = ((y * w + x) * 4) as usize;
                for c in 0..4 {
                    px[i + c] = (px[i + c] as f32 * cov) as u8;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Surface: DIB + software D2D target + layered window presentation
// ---------------------------------------------------------------------------

pub struct Surface {
    /// The software target (None on the graphics card: see `card`).
    rt: Option<ID2D1DCRenderTarget>,
    /// This surface's picture on the graphics card.
    card: Option<Card>,
    /// The part being drawn (pixels), in a frame begun with `begin_part`.
    part: std::cell::Cell<Option<RECT>>,
    base: ID2D1RenderTarget,
    brush: ID2D1SolidColorBrush,
    /// `rt` is a hardware target, made for this generation (see `GEN`).
    hw: bool,
    gen: u32,
    /// When this frame was begun (a whole frame, not a part).
    began: std::cell::Cell<Option<std::time::Instant>>,
    /// The frame being drawn has failed already (see `end`).
    broken: std::cell::Cell<bool>,
    /// The clips pushed and not popped yet, each with the transform it was
    /// pushed under: a frame must end with none left (Direct2D fails the
    /// whole frame otherwise), and ending a frame in the middle (see
    /// `backdrop_blur`) has to take them off and put them back.
    clips: RefCell<Vec<(D2D_RECT_F, windows_numerics::Matrix3x2)>>,
    /// The blurred band kept by `backdrop_blur`.
    blur: RefCell<Option<Blurred>>,
    /// How long the last whole frame took to draw (µs).
    pub last_cost: std::cell::Cell<u32>,
    round_cap: ID2D1StrokeStyle,
    dash: ID2D1StrokeStyle,
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    pub w: i32,
    pub h: i32,
    pub scale: f32,
    /// A line too long for its place scrolls (see `text`): seconds until the
    /// next frame it needs (0: right away), the soonest of this frame's.
    marquee: std::cell::Cell<Option<f32>>,
    /// Where this frame's scrolling lines are (DIPs, as drawn): a window
    /// can draw just that again for their next frame.
    mq_area: std::cell::Cell<Option<D2D_RECT_F>>,
}

/// A blurred band of a surface's picture (see `backdrop_blur`).
struct Blurred {
    gen: u32,
    w: i32,
    h: i32,
    scale: f32,
    x0: i32,
    y0: i32,
    y1: i32,
    made: std::time::Instant,
    bmp: ID2D1Bitmap,
}

/// Long lines that don't fit scroll sideways: still for a moment, then slowly
/// to their end, a pause, and back to the start.
const MQ_PAUSE: f32 = 2.0;
const MQ_END: f32 = 1.6;
const MQ_SPEED: f32 = 30.0;

thread_local! {
    /// Natural width of (format, text).
    static MQ_W: RefCell<std::collections::HashMap<(usize, String), f32>> = RefCell::new(std::collections::HashMap::new());
    /// Each scrolling line, by where it is drawn: when it started, when it
    /// was last seen, and what it said.
    static MQ_T: RefCell<std::collections::HashMap<u64, (std::time::Instant, std::time::Instant, String)>> = RefCell::new(std::collections::HashMap::new());
}

impl Surface {
    pub fn new() -> Result<Surface> {
        let g = g();
        unsafe {
            let (Target { rt, card, base, brush }, hw, gen) = current_target()?;
            let dc = CreateCompatibleDC(None);
            let caps = D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 10.0,
                dashStyle: D2D1_DASH_STYLE_SOLID,
                dashOffset: 0.0,
            };
            let round_cap = g.d2d.CreateStrokeStyle(&caps, None)?;
            let dash = g.d2d.CreateStrokeStyle(&D2D1_STROKE_STYLE_PROPERTIES { dashStyle: D2D1_DASH_STYLE_DASH, ..caps }, None)?;
            Ok(Surface {
                rt,
                card,
                part: std::cell::Cell::new(None),
                base,
                brush,
                hw,
                gen,
                began: std::cell::Cell::new(None),
                broken: std::cell::Cell::new(false),
                clips: RefCell::new(Vec::new()),
                blur: RefCell::new(None),
                last_cost: std::cell::Cell::new(0),
                round_cap,
                dash,
                dc,
                bmp: HBITMAP::default(),
                old: HGDIOBJ::default(),
                bits: std::ptr::null_mut(),
                w: 0,
                h: 0,
                scale: 1.0,
                marquee: std::cell::Cell::new(None),
                mq_area: std::cell::Cell::new(None),
            })
        }
    }

    /// The target of the kind in use now (made again when that changed, or
    /// after the GPU was reset). False: no target could be made.
    fn sync(&mut self) -> bool {
        if self.gen == GEN.load(Relaxed) {
            return true;
        }
        match current_target() {
            Ok((Target { rt, card, base, brush }, hw, gen)) => {
                self.rt = rt;
                self.card = card;
                self.base = base;
                self.brush = brush;
                self.hw = hw;
                self.gen = gen;
                true
            }
            Err(_) => false,
        }
    }

    pub fn resize(&mut self, w: i32, h: i32) {
        let (w, h) = (w.max(1), h.max(1));
        if w == self.w && h == self.h && !self.bmp.is_invalid() {
            return;
        }
        unsafe {
            self.free_bitmap();
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            if let Ok(bmp) = CreateDIBSection(Some(self.dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                self.old = SelectObject(self.dc, bmp.into());
                self.bmp = bmp;
                self.bits = bits as *mut u8;
                self.w = w;
                self.h = h;
            }
        }
    }

    unsafe fn free_bitmap(&mut self) {
        if !self.bmp.is_invalid() {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp.into());
            self.bmp = HBITMAP::default();
            self.bits = std::ptr::null_mut();
        }
    }

    /// Bind, begin drawing in DIPs (scaled by `scale`), clear to transparent.
    pub fn begin(&mut self, scale: f32) -> bool {
        if self.bmp.is_invalid() {
            return false;
        }
        if !self.sync() {
            return false;
        }
        self.scale = scale;
        self.marquee.set(None);
        self.mq_area.set(None);
        self.broken.set(false);
        self.clips.borrow_mut().clear();
        self.began.set(Some(std::time::Instant::now()));
        self.part.set(None);
        unsafe {
            let r = RECT { left: 0, top: 0, right: self.w, bottom: self.h };
            if let Some(card) = self.card.as_mut() {
                if let Err(e) = card.ensure(r.right, r.bottom) {
                    hardware_failed(&e, true);
                    return false;
                }
                if let Some(t) = card.target.as_ref() {
                    card.dcx.SetTarget(t);
                }
                card.fresh = false;
            } else if let Some(rt) = self.rt.as_ref() {
                if rt.BindDC(self.dc, &r).is_err() {
                    return false;
                }
            } else {
                return false;
            }
            self.base.SetDpi(96.0 * scale, 96.0 * scale);
            self.base.BeginDraw();
            self.base.SetTransform(&windows_numerics::Matrix3x2::identity());
            self.base.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        }
        true
    }

    /// Draw into just `part` (DIPs) of what's there: the rest of the picture
    /// is left exactly as it was (only that strip is written back).
    pub fn begin_part(&mut self, scale: f32, part: D2D_RECT_F) -> bool {
        if self.bmp.is_invalid() {
            return false;
        }
        if !self.sync() {
            return false;
        }
        self.scale = scale;
        self.marquee.set(None);
        self.mq_area.set(None);
        self.broken.set(false);
        self.clips.borrow_mut().clear();
        self.began.set(None);
        let r = RECT {
            left: ((part.left * scale).floor() as i32).clamp(0, self.w),
            top: ((part.top * scale).floor() as i32).clamp(0, self.h),
            right: ((part.right * scale).ceil() as i32).clamp(0, self.w),
            bottom: ((part.bottom * scale).ceil() as i32).clamp(0, self.h),
        };
        if r.right <= r.left || r.bottom <= r.top {
            return false;
        }
        self.part.set(None);
        unsafe {
            if let Some(card) = self.card.as_mut() {
                // On the graphics card: the whole picture is there; a clip
                // keeps the drawing to the part, and only the part comes back.
                if let Err(e) = card.ensure(self.w, self.h) {
                    hardware_failed(&e, true);
                    return false;
                }
                if card.fresh {
                    card.upload(self.bits);
                }
                if let Some(t) = card.target.as_ref() {
                    card.dcx.SetTarget(t);
                }
                self.base.SetDpi(96.0 * scale, 96.0 * scale);
                self.base.BeginDraw();
                self.base.SetTransform(&windows_numerics::Matrix3x2::identity());
                self.base.PushAxisAlignedClip(&Self::part_dips(r, scale), D2D1_ANTIALIAS_MODE_ALIASED);
                self.part.set(Some(r));
                return true;
            }
            let Some(rt) = self.rt.as_ref() else { return false };
            if rt.BindDC(self.dc, &r).is_err() {
                return false;
            }
            self.base.SetDpi(96.0 * scale, 96.0 * scale);
            self.base.BeginDraw();
            // Same coordinates as a full draw.
            self.base.SetTransform(&windows_numerics::Matrix3x2::translation(-r.left as f32 / scale, -r.top as f32 / scale));
        }
        true
    }

    /// A part's rectangle in pixels, as DIPs.
    fn part_dips(r: RECT, scale: f32) -> D2D_RECT_F {
        D2D_RECT_F { left: r.left as f32 / scale, top: r.top as f32 / scale, right: r.right as f32 / scale, bottom: r.bottom as f32 / scale }
    }

    /// On the graphics card: end the frame there and bring `what` of it
    /// (pixels; None: the part being drawn, or all of it) into the DIB.
    unsafe fn card_end(&self, what: Option<RECT>) -> Result<()> {
        let Some(card) = self.card.as_ref() else { return Err(Error::from(E_FAIL)) };
        if self.part.get().is_some() {
            // (The part's clip, under the plain transform it was pushed with.)
            self.base.PopAxisAlignedClip();
        }
        self.base.EndDraw(None, None)?;
        let all = RECT { left: 0, top: 0, right: self.w, bottom: self.h };
        card.read_back(what.or(self.part.get()).unwrap_or(all), self.bits)
    }

    /// The frame is finished (and, from a hardware target, copied into the
    /// DIB). False: it failed and what's in the DIB isn't a picture — draw
    /// it again (the next try uses a new target, or the processor).
    pub fn end(&self) -> bool {
        // (Clips left on: a mistake in the drawing code, not in the frame.)
        let left = self.lift_clips();
        if left > 0 {
            self.clips.borrow_mut().clear();
            note_unbalanced("clips were left on at the end of a frame", self.w, self.h);
        }
        let r = unsafe {
            if self.hw {
                let r = self.card_end(None);
                self.part.set(None);
                r
            } else {
                self.base.EndDraw(None, None)
            }
        };
        let began = self.began.take();
        match r {
            Ok(()) if !self.broken.get() => {
                if self.hw {
                    // The first big frames from the graphics card: is there a
                    // picture in the DIB at all? (A driver that draws nothing
                    // and reports no error would leave Phemio invisible.)
                    static CHECKED: AtomicU32 = AtomicU32::new(0);
                    if began.is_some() && self.w as i64 * self.h as i64 >= 120_000 && !self.bits.is_null() && CHECKED.load(Relaxed) < 30 {
                        CHECKED.fetch_add(1, Relaxed);
                        let px = unsafe { std::slice::from_raw_parts(self.bits, (self.w * self.h * 4) as usize) };
                        if !px.chunks_exact(4).step_by(7).any(|p| p[3] != 0) {
                            hardware_failed(&Error::new(E_FAIL, "the frame came back empty"), false);
                            return false;
                        }
                    }
                    FAILS.store(0, Relaxed);
                }
                if let Some(t) = began {
                    let us = t.elapsed().as_micros() as u64;
                    self.last_cost.set(us.min(u32::MAX as u64) as u32);
                    calibrate(self.w as i64 * self.h as i64, us);
                }
                true
            }
            Ok(()) => false,
            Err(e) => {
                if self.hw {
                    // (Unbalanced clips are this program's mistake; anything
                    // else: the device may be gone — start it again.)
                    hardware_failed(&e, e.code() != D2DERR_PUSH_POP_UNBALANCED);
                } else if e.code() == D2DERR_RECREATE_TARGET {
                    GEN.fetch_add(1, Relaxed);
                }
                false
            }
        }
    }

    /// Size of the surface in DIPs.
    pub fn dip_w(&self) -> f32 {
        self.w as f32 / self.scale
    }
    pub fn dip_h(&self) -> f32 {
        self.h as f32 / self.scale
    }

    /// Push pixels to a layered window at screen position (x, y).
    pub fn present(&self, hwnd: HWND, x: i32, y: i32, dirty: Option<RECT>) {
        unsafe {
            let dst = POINT { x, y };
            let size = SIZE { cx: self.w, cy: self.h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            let dr = dirty.unwrap_or_default();
            let info = UPDATELAYEREDWINDOWINFO {
                cbSize: std::mem::size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
                hdcDst: HDC::default(),
                pptDst: &dst,
                psize: &size,
                hdcSrc: self.dc,
                pptSrc: &src,
                crKey: COLORREF(0),
                pblend: &blend,
                dwFlags: ULW_ALPHA,
                prcDirty: if dirty.is_some() { &dr } else { std::ptr::null() },
            };
            let _ = UpdateLayeredWindowIndirect(hwnd, &info);
        }
        self.debug_dump(hwnd);
    }

    /// Present a layered *child* window (position is set with SetWindowPos).
    pub fn present_child(&self, hwnd: HWND) {
        unsafe {
            let size = SIZE { cx: self.w, cy: self.h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            let info = UPDATELAYEREDWINDOWINFO {
                cbSize: std::mem::size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
                hdcDst: HDC::default(),
                pptDst: std::ptr::null(),
                psize: &size,
                hdcSrc: self.dc,
                pptSrc: &src,
                crKey: COLORREF(0),
                pblend: &blend,
                dwFlags: ULW_ALPHA,
                prcDirty: std::ptr::null(),
            };
            let _ = UpdateLayeredWindowIndirect(hwnd, &info);
        }
        self.debug_dump(hwnd);
    }

    /// Developer aid: with ISLET_DUMP=<folder>, every presented frame is also
    /// written as raw premultiplied BGRA (`<class>_<w>x<h>.bgra`).
    pub fn debug_dump(&self, hwnd: HWND) {
        static DIR: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        let Some(dir) = DIR.get_or_init(|| std::env::var("ISLET_DUMP").ok()) else { return };
        if self.bits.is_null() {
            return;
        }
        let name = crate::util::class_name(hwnd);
        let data = unsafe { std::slice::from_raw_parts(self.bits, (self.w * self.h * 4) as usize) };
        let _ = std::fs::write(format!("{dir}\\{name}_{}x{}.bgra", self.w, self.h), data);
        // ISLET_DUMP_SEQ: a line per frame: window, where it is, and where
        // the opaque shape is in it (to find frames that jump or go blank).
        if std::env::var("ISLET_DUMP_SEQ").is_ok() {
            let (w, h) = (self.w as usize, self.h as usize);
            let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
            for y in 0..h {
                for x in 0..w {
                    if data[(y * w + x) * 4 + 3] > 200 {
                        x0 = x0.min(x);
                        x1 = x1.max(x);
                        y0 = y0.min(y);
                        y1 = y1.max(y);
                    }
                }
            }
            let mut r = RECT::default();
            unsafe {
                let _ = GetWindowRect(hwnd, &mut r);
            }
            let vis = unsafe { IsWindowVisible(hwnd).as_bool() } as u8;
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{dir}\\seq.txt")) {
                let _ = writeln!(f, "{t} {name} {w}x{h} win={},{} vis={vis} shape={x0},{y0}-{x1},{y1}", r.left, r.top);
            }
        }
    }

    /// Paint the DIB into a regular window DC (used by the settings window).
    pub fn blit(&self, hdc: HDC) {
        unsafe {
            let _ = BitBlt(hdc, 0, 0, self.w, self.h, Some(self.dc), 0, 0, SRCCOPY);
        }
    }

    /// Fill the DIB with an opaque colour (for non-layered windows).
    pub fn clear(&self, c: D2D1_COLOR_F) {
        unsafe { self.base.Clear(Some(&c)) }
    }

    // ----- drawing primitives (all coordinates in DIPs) -----

    fn b(&self, c: D2D1_COLOR_F) -> &ID2D1SolidColorBrush {
        unsafe { self.brush.SetColor(&c) };
        &self.brush
    }

    pub fn fill_rect(&self, r: D2D_RECT_F, c: D2D1_COLOR_F) {
        unsafe { self.base.FillRectangle(&r, self.b(c)) }
    }

    pub fn fill_rr(&self, r: D2D_RECT_F, rad: f32, c: D2D1_COLOR_F) {
        let rr = D2D1_ROUNDED_RECT { rect: r, radiusX: rad, radiusY: rad };
        unsafe { self.base.FillRoundedRectangle(&rr, self.b(c)) }
    }

    pub fn stroke_rr(&self, r: D2D_RECT_F, rad: f32, c: D2D1_COLOR_F, width: f32) {
        let rr = D2D1_ROUNDED_RECT { rect: r, radiusX: rad, radiusY: rad };
        unsafe { self.base.DrawRoundedRectangle(&rr, self.b(c), width, None) }
    }

    /// A rounded rectangle shaded from `top` to `bottom`.
    #[allow(dead_code)]
    pub fn fill_rr_grad(&self, r: D2D_RECT_F, rad: f32, top: D2D1_COLOR_F, bottom: D2D1_COLOR_F) {
        unsafe {
            let stops = [D2D1_GRADIENT_STOP { position: 0.0, color: top }, D2D1_GRADIENT_STOP { position: 1.0, color: bottom }];
            let Ok(coll) = self.base.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) else { return };
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: pt(r.left, r.top), endPoint: pt(r.left, r.bottom) };
            let Ok(br) = self.base.CreateLinearGradientBrush(&props, None, &coll) else { return };
            let rr = D2D1_ROUNDED_RECT { rect: r, radiusX: rad, radiusY: rad };
            self.base.FillRoundedRectangle(&rr, &br);
        }
    }

    pub fn fill_circle(&self, cx: f32, cy: f32, r: f32, c: D2D1_COLOR_F) {
        let e = D2D1_ELLIPSE { point: pt(cx, cy), radiusX: r, radiusY: r };
        unsafe { self.base.FillEllipse(&e, self.b(c)) }
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: D2D1_COLOR_F, w: f32) {
        unsafe { self.base.DrawLine(pt(x0, y0), pt(x1, y1), self.b(c), w, Some(&self.round_cap)) }
    }

    /// Progress ring: full track + arc from 12 o'clock, clockwise.
    pub fn ring(&self, cx: f32, cy: f32, r: f32, w: f32, frac: f32, track: D2D1_COLOR_F, c: D2D1_COLOR_F) {
        unsafe {
            let e = D2D1_ELLIPSE { point: pt(cx, cy), radiusX: r, radiusY: r };
            self.base.DrawEllipse(&e, self.b(track), w, None);
        }
        let frac = frac.clamp(0.0, 1.0);
        if frac <= 0.001 {
            return;
        }
        if frac >= 0.999 {
            unsafe {
                let e = D2D1_ELLIPSE { point: pt(cx, cy), radiusX: r, radiusY: r };
                self.base.DrawEllipse(&e, self.b(c), w, None);
            }
            return;
        }
        // Polyline arc: exact everywhere and plenty smooth at these sizes.
        let n = ((frac * 96.0).ceil() as usize).max(2);
        let pts: Vec<_> = (0..=n)
            .map(|i| {
                let a = -std::f32::consts::FRAC_PI_2 + frac * std::f32::consts::TAU * i as f32 / n as f32;
                pt(cx + r * a.cos(), cy + r * a.sin())
            })
            .collect();
        unsafe {
            let Ok(geo) = g().d2d.CreatePathGeometry() else { return };
            let Ok(sink) = geo.Open() else { return };
            sink.BeginFigure(pts[0], D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddLines(&pts[1..]);
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            let _ = sink.Close();
            self.base.DrawGeometry(&geo, self.b(c), w, Some(&self.round_cap));
        }
    }

    pub fn dashed_rr(&self, r: D2D_RECT_F, rad: f32, c: D2D1_COLOR_F, w: f32) {
        let rr = D2D1_ROUNDED_RECT { rect: r, radiusX: rad, radiusY: rad };
        unsafe { self.base.DrawRoundedRectangle(&rr, self.b(c), w, Some(&self.dash)) }
    }

    pub fn fill_poly(&self, pts: &[(f32, f32)], c: D2D1_COLOR_F) {
        if pts.len() < 3 {
            return;
        }
        unsafe {
            let Ok(geo) = g().d2d.CreatePathGeometry() else { return };
            let Ok(sink) = geo.Open() else { return };
            sink.BeginFigure(pt(pts[0].0, pts[0].1), D2D1_FIGURE_BEGIN_FILLED);
            let rest: Vec<_> = pts[1..].iter().map(|p| pt(p.0, p.1)).collect();
            sink.AddLines(&rest);
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            let _ = sink.Close();
            self.base.FillGeometry(&geo, self.b(c), None);
        }
    }

    /// Multi-line text (word wrap), returns nothing.
    pub fn text_wrap(&self, s: &str, f: &IDWriteTextFormat, r: D2D_RECT_F, c: D2D1_COLOR_F) {
        if s.is_empty() || c.a <= 0.003 {
            return;
        }
        let w: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            self.base.DrawText(&w, f, &r, self.b(c), D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }
    }

    /// Open or closed polyline with round caps and joins.
    pub fn stroke_poly(&self, pts: &[(f32, f32)], closed: bool, c: D2D1_COLOR_F, w: f32) {
        // Drop repeated points: zero-length segments make spiky joins.
        let mut v: Vec<(f32, f32)> = Vec::with_capacity(pts.len());
        for p in pts {
            if v.last().map(|q: &(f32, f32)| (q.0 - p.0).abs() + (q.1 - p.1).abs() > 0.05).unwrap_or(true) {
                v.push(*p);
            }
        }
        if closed && v.len() > 2 {
            let (f, l) = (v[0], v[v.len() - 1]);
            if (f.0 - l.0).abs() + (f.1 - l.1).abs() <= 0.05 {
                v.pop();
            }
        }
        let pts = &v[..];
        if pts.len() < 2 {
            return;
        }
        unsafe {
            let Ok(geo) = g().d2d.CreatePathGeometry() else { return };
            let Ok(sink) = geo.Open() else { return };
            sink.BeginFigure(pt(pts[0].0, pts[0].1), D2D1_FIGURE_BEGIN_HOLLOW);
            let rest: Vec<_> = pts[1..].iter().map(|p| pt(p.0, p.1)).collect();
            sink.AddLines(&rest);
            sink.EndFigure(if closed { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
            let _ = sink.Close();
            self.base.DrawGeometry(&geo, self.b(c), w, Some(&self.round_cap));
        }
    }

    pub fn fill_geo(&self, geo: &ID2D1Geometry, c: D2D1_COLOR_F) {
        unsafe { self.base.FillGeometry(geo, self.b(c), None) }
    }

    pub fn text(&self, s: &str, f: &IDWriteTextFormat, r: D2D_RECT_F, c: D2D1_COLOR_F) {
        if s.is_empty() || c.a <= 0.003 {
            return;
        }
        let w: Vec<u16> = s.encode_utf16().collect();
        if self.marquee_text(s, &w, f, r, c) {
            return;
        }
        unsafe {
            self.base.DrawText(&w, f, &r, self.b(c), D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        }
    }

    /// Seconds until the next frame a scrolling line needs (0: now), if any
    /// was drawn since `begin`.
    pub fn take_marquee(&self) -> Option<f32> {
        self.marquee.take()
    }

    /// The rectangle (DIPs) around every line that is scrolling right now.
    pub fn marquee_area(&self) -> Option<D2D_RECT_F> {
        self.mq_area.get()
    }

    /// A one-line text longer than its place: instead of "…", it scrolls to
    /// show the rest. Returns false when it fits (drawn the usual way).
    fn marquee_text(&self, s: &str, w: &[u16], f: &IDWriteTextFormat, r: D2D_RECT_F, c: D2D1_COLOR_F) -> bool {
        let rw = r.right - r.left;
        if rw < 24.0 || s.contains('\n') || c.a < 0.05 {
            return false;
        }
        unsafe {
            if f.GetWordWrapping() != DWRITE_WORD_WRAPPING_NO_WRAP {
                return false;
            }
        }
        let key = (f.as_raw() as usize, s.to_string());
        let natural = MQ_W.with(|m| {
            let mut m = m.borrow_mut();
            if let Some(v) = m.get(&key) {
                return *v;
            }
            if m.len() > 4000 {
                m.clear();
            }
            let v = g().measure(s, f);
            m.insert(key, v);
            v
        });
        let over = natural - rw;
        if over <= 1.5 {
            return false;
        }
        use std::hash::{Hash, Hasher};
        // A line is the place it is drawn in, not what it says: a number
        // in it ticking over ("working set 98 MB" → "99 MB") must not send
        // it back to its start once a second.
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let mut m = windows_numerics::Matrix3x2::identity();
        unsafe { self.base.GetTransform(&mut m) };
        ((r.left + m.M31).round() as i32, (r.top + m.M32).round() as i32, rw as i32, self.w, self.h).hash(&mut h);
        (f.as_raw() as usize).hash(&mut h);
        let id = h.finish();
        let now = std::time::Instant::now();
        let start = MQ_T.with(|m| {
            let mut m = m.borrow_mut();
            if m.len() > 300 {
                m.retain(|_, (_, seen, _)| now.duration_since(*seen).as_secs_f32() < 10.0);
            }
            let e = m.entry(id).or_insert_with(|| (now, now, s.to_string()));
            // Gone from sight for a while, or a different line altogether
            // in this place (not just a few characters of it): from the
            // beginning.
            let same = e.2 == s || {
                let n = e.2.chars().count().max(s.chars().count()).max(1);
                let head = e.2.chars().zip(s.chars()).take_while(|(a, b)| a == b).count();
                let tail = e.2.chars().rev().zip(s.chars().rev()).take_while(|(a, b)| a == b).count();
                (head + tail) * 10 >= n * 6
            };
            if now.duration_since(e.1).as_secs_f32() > MQ_PAUSE + 3.0 || !same {
                e.0 = now;
            }
            if e.2 != s {
                e.2 = s.to_string();
            }
            e.1 = now;
            e.0
        });
        let scroll = over / MQ_SPEED;
        let cycle = MQ_PAUSE + scroll + MQ_END;
        let t = now.duration_since(start).as_secs_f32() % cycle;
        let (off, wake) = if t < MQ_PAUSE {
            (0.0, MQ_PAUSE - t)
        } else if t < MQ_PAUSE + scroll {
            ((t - MQ_PAUSE) * MQ_SPEED, 0.0)
        } else {
            (over, cycle - t)
        };
        self.marquee.set(Some(self.marquee.get().map(|x: f32| x.min(wake)).unwrap_or(wake)));
        unsafe {
            let Ok(l) = g().dw.CreateTextLayout(w, f, natural + 8.0, (r.bottom - r.top).max(1.0)) else { return false };
            let _ = l.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
            self.base.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED);
            // While it glides: exactly where the clock says, between pixels
            // too (NO_SNAP) — snapped to whole pixels, slow text moves in
            // uneven little steps, which is what showed as judder. At rest
            // (the pauses at either end) it sits on the pixels, crisp.
            let moving = wake <= 0.0;
            let (off, how) = if moving { (off, D2D1_DRAW_TEXT_OPTIONS_NO_SNAP) } else { ((off * self.scale).round() / self.scale.max(0.01), D2D1_DRAW_TEXT_OPTIONS_NONE) };
            self.base.DrawTextLayout(windows_numerics::Vector2 { X: r.left - off, Y: r.top }, &l, self.b(c), how);
            self.base.PopAxisAlignedClip();
            // (Where it is, for drawing just this again.)
            let mut m = windows_numerics::Matrix3x2::identity();
            self.base.GetTransform(&mut m);
            let q = D2D_RECT_F { left: r.left + m.M31, top: r.top + m.M32, right: r.right + m.M31, bottom: r.bottom + m.M32 };
            self.mq_area.set(Some(match self.mq_area.get() {
                Some(a) => D2D_RECT_F { left: a.left.min(q.left), top: a.top.min(q.top), right: a.right.max(q.right), bottom: a.bottom.max(q.bottom) },
                None => q,
            }));
        }
        true
    }

    /// Wrapped text with styled stretches: (start, length in UTF-16 units,
    /// bold, italic, underline, strikethrough). Returns the text's height.
    pub fn text_rich(&self, s: &str, f: &IDWriteTextFormat, r: D2D_RECT_F, c: D2D1_COLOR_F, styles: &[(u32, u32, bool, bool, bool, bool)]) -> f32 {
        let w: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let Ok(l) = g().dw.CreateTextLayout(&w, f, (r.right - r.left).max(1.0), (r.bottom - r.top).max(1.0)) else { return 0.0 };
            for &(start, len, b, i, u, st) in styles {
                let range = DWRITE_TEXT_RANGE { startPosition: start, length: len };
                if b {
                    let _ = l.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, range);
                }
                if i {
                    let _ = l.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, range);
                }
                if u {
                    let _ = l.SetUnderline(true, range);
                }
                if st {
                    let _ = l.SetStrikethrough(true, range);
                }
            }
            if c.a > 0.003 {
                self.base.DrawTextLayout(windows_numerics::Vector2 { X: r.left, Y: r.top }, &l, self.b(c), D2D1_DRAW_TEXT_OPTIONS_CLIP);
            }
            let mut m = DWRITE_TEXT_METRICS::default();
            if l.GetMetrics(&mut m).is_ok() {
                return m.height;
            }
            0.0
        }
    }

    /// Text with some stretches in another colour (and bolder): `marks` are
    /// (start, length) in UTF-16 units. Wraps if the format does.
    pub fn text_marked(&self, s: &str, f: &IDWriteTextFormat, r: D2D_RECT_F, c: D2D1_COLOR_F, mark: D2D1_COLOR_F, marks: &[(u32, u32)]) {
        if s.is_empty() || c.a <= 0.003 {
            return;
        }
        let w: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let Ok(l) = g().dw.CreateTextLayout(&w, f, (r.right - r.left).max(1.0), (r.bottom - r.top).max(1.0)) else { return };
            let brush = self.base.CreateSolidColorBrush(&mark, None).ok();
            for &(start, len) in marks {
                let range = DWRITE_TEXT_RANGE { startPosition: start, length: len };
                if let Some(b) = &brush {
                    let _ = l.SetDrawingEffect(b, range);
                }
                let _ = l.SetFontWeight(DWRITE_FONT_WEIGHT_SEMI_BOLD, range);
            }
            self.base.DrawTextLayout(windows_numerics::Vector2 { X: r.left, Y: r.top }, &l, self.b(c), D2D1_DRAW_TEXT_OPTIONS_CLIP);
        }
    }

    /// Where the text ends (for a caret): (x, y, line height) of the last character.
    pub fn text_end(&self, s: &str, f: &IDWriteTextFormat, width: f32) -> (f32, f32, f32) {
        let w: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            if let Ok(l) = g().dw.CreateTextLayout(&w, f, width.max(1.0), 4000.0) {
                let (mut x, mut y) = (0.0f32, 0.0f32);
                let mut m = DWRITE_HIT_TEST_METRICS::default();
                // After the last character (a trailing line break starts a new line).
                if w.is_empty() {
                    if l.HitTestTextPosition(0, false, &mut x, &mut y, &mut m).is_ok() {
                        return (0.0, y, m.height);
                    }
                } else if w.last() == Some(&10) {
                    if l.HitTestTextPosition(w.len() as u32 - 1, false, &mut x, &mut y, &mut m).is_ok() {
                        return (0.0, y + m.height, m.height);
                    }
                } else if l.HitTestTextPosition(w.len() as u32 - 1, true, &mut x, &mut y, &mut m).is_ok() {
                    return (x, y, m.height);
                }
            }
        }
        // Hit-testing unavailable: work it out by measuring (last paragraph,
        // wrapped word by word).
        let gg = g();
        let lh = gg.measure_h("Ag", f, 4000.0).max(1.0);
        let (before, last) = match s.rfind('\n') {
            Some(i) => (&s[..i + 1], &s[i + 1..]),
            None => ("", s),
        };
        let mut y = if before.is_empty() { 0.0 } else { gg.measure_h(&format!("{before}x"), f, width) - lh };
        let mut line = String::new();
        for word in last.split_inclusive(' ') {
            let t = format!("{line}{word}");
            if !line.is_empty() && gg.measure(t.trim_end(), f) > width {
                y += lh;
                line = word.to_string();
            } else {
                line = t;
            }
        }
        (gg.measure(&line, f).min(width), y.max(0.0), lh)
    }

    pub fn image(&self, img: &Image, r: D2D_RECT_F, opacity: f32) {
        if opacity <= 0.003 {
            return;
        }
        if let Some(b) = img.bitmap(&self.base) {
            unsafe {
                self.base.DrawBitmap(&b, Some(&r), opacity, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
            }
        }
    }

    /// Frosted glass: what's already drawn under `r` (DIPs), blurred
    /// (`radius` DIPs), shown inside a rounded rectangle.
    ///
    /// Reading the picture back and blurring it is the dearest thing a
    /// frame can do (on the graphics card the frame has to be ended and
    /// brought back first), and a panel that slides in and grows asks for
    /// it on every frame. So a whole band — the surface's width, a little
    /// taller than `r` — is blurred once and kept for a third of a second:
    /// frames in between reuse it, wherever in the band the panel is.
    pub fn backdrop_blur(&self, r: D2D_RECT_F, corner: f32, radius: f32, opacity: f32) {
        if self.bits.is_null() {
            return;
        }
        let sc = self.scale;
        let gen = GEN.load(Relaxed);
        let px = |v: f32| (v * sc).round() as i32;
        let (ry0, ry1) = (px(r.top).clamp(0, self.h), px(r.bottom).clamp(0, self.h));
        let fresh = self.blur.borrow().as_ref().map(|b| b.gen == gen && b.w == self.w && b.h == self.h && b.scale == sc && ry0 >= b.y0 && ry1 <= b.y1 && b.made.elapsed().as_millis() < 330).unwrap_or(false);
        let _d = crate::util::diag::Span::new("blur.draw");
        if !fresh {
            let _d = crate::util::diag::Span::new("blur.make");
            let margin = px(14.0);
            let (x0, x1) = (0, self.w);
            let (y0, y1) = ((ry0 - margin).clamp(0, self.h), (ry1 + margin).clamp(0, self.h));
            let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
            if w < 4 || h < 4 {
                return;
            }
            unsafe {
                // What's been drawn so far, into the bitmap. A hardware
                // target only copies its picture there when the frame ends:
                // end it here and begin again (the picture, the transform
                // and the DIB all stay as they are).
                if self.hw {
                    self.lift_clips();
                    // (Only the band comes back from the graphics card.)
                    let band = RECT { left: x0, top: y0, right: x1, bottom: y1 };
                    let r = self.card_end(Some(band));
                    self.base.BeginDraw();
                    if let Some(p) = self.part.get() {
                        let mut now = windows_numerics::Matrix3x2::identity();
                        self.base.GetTransform(&mut now);
                        self.base.SetTransform(&windows_numerics::Matrix3x2::identity());
                        self.base.PushAxisAlignedClip(&Self::part_dips(p, sc), D2D1_ANTIALIAS_MODE_ALIASED);
                        self.base.SetTransform(&now);
                    }
                    self.restore_clips();
                    if let Err(e) = r {
                        self.broken.set(true);
                        hardware_failed(&e, e.code() != D2DERR_PUSH_POP_UNBALANCED);
                        return;
                    }
                } else {
                    let _ = self.base.Flush(None, None);
                }
            }
            let src = unsafe { std::slice::from_raw_parts(self.bits, (self.w * self.h * 4) as usize) };
            let stride = self.w as usize * 4;
            let mut buf = vec![0u8; w * h * 4];
            for y in 0..h {
                let s0 = (y0 as usize + y) * stride + x0 as usize * 4;
                buf[y * w * 4..(y + 1) * w * 4].copy_from_slice(&src[s0..s0 + w * 4]);
            }
            // Three box blurs ≈ a gaussian.
            let rad = ((radius * sc) / 2.0).round().max(1.0) as usize;
            let mut tmp = vec![0u8; buf.len()];
            for _ in 0..3 {
                box_blur(&buf, &mut tmp, w, h, rad, true);
                box_blur(&tmp, &mut buf, w, h, rad, false);
            }
            let props = D2D1_BITMAP_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0 * sc,
                dpiY: 96.0 * sc,
            };
            let Ok(bmp) = (unsafe { self.base.CreateBitmap(D2D_SIZE_U { width: w as u32, height: h as u32 }, Some(buf.as_ptr() as *const _), (w * 4) as u32, &props) }) else { return };
            *self.blur.borrow_mut() = Some(Blurred { gen, w: self.w, h: self.h, scale: sc, x0, y0, y1, made: std::time::Instant::now(), bmp });
        }
        let b = self.blur.borrow();
        let Some(b) = b.as_ref() else { return };
        unsafe {
            let Ok(brush) = self.base.CreateBitmapBrush(&b.bmp, None, None) else { return };
            brush.SetOpacity(opacity);
            brush.SetTransform(&windows_numerics::Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: b.x0 as f32 / sc, M32: b.y0 as f32 / sc });
            let rr = D2D1_ROUNDED_RECT { rect: r, radiusX: corner, radiusY: corner };
            self.base.FillRoundedRectangle(&rr, &brush);
        }
    }

    /// Scale everything drawn from now on by `s` around (ox, oy).
    pub fn scale_about(&self, ox: f32, oy: f32, s: f32) {
        let m = windows_numerics::Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: ox * (1.0 - s), M32: oy * (1.0 - s) };
        unsafe { self.base.SetTransform(&m) }
    }
    /// Back to no scaling.
    pub fn reset_scale(&self) {
        let m = windows_numerics::Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 };
        unsafe { self.base.SetTransform(&m) }
    }

    pub fn clip(&self, r: D2D_RECT_F) {
        unsafe {
            let mut m = windows_numerics::Matrix3x2::identity();
            self.base.GetTransform(&mut m);
            self.clips.borrow_mut().push((r, m));
            self.base.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED)
        }
    }
    pub fn unclip(&self) {
        // (One pop too many would fail the whole frame.)
        if self.clips.borrow_mut().pop().is_none() {
            note_unbalanced("a clip was taken off that was never put on", self.w, self.h);
            return;
        }
        unsafe { self.base.PopAxisAlignedClip() }
    }

    /// Take every clip off (before ending a frame) …
    fn lift_clips(&self) -> usize {
        let n = self.clips.borrow().len();
        for _ in 0..n {
            unsafe { self.base.PopAxisAlignedClip() }
        }
        n
    }
    /// … and put them back, each under the transform it had.
    fn restore_clips(&self) {
        unsafe {
            let mut now = windows_numerics::Matrix3x2::identity();
            self.base.GetTransform(&mut now);
            for (r, m) in self.clips.borrow().iter() {
                self.base.SetTransform(m);
                self.base.PushAxisAlignedClip(r, D2D1_ANTIALIAS_MODE_ALIASED);
            }
            self.base.SetTransform(&now);
        }
    }

    /// Soft drop shadow faked with a few translucent rounded rects (software
    /// targets have no blur effect, and this costs almost nothing).
    pub fn shadow(&self, r: D2D_RECT_F, rad: f32, strength: f32) {
        for i in 1..=6 {
            let s = i as f32 * 1.6;
            let rr = D2D_RECT_F { left: r.left - s, top: r.top - s + 2.0, right: r.right + s, bottom: r.bottom + s + 3.0 };
            self.fill_rr(rr, rad + s, rgb(0, strength * 0.05));
        }
    }

    /// Build a path: a shape with independent top/bottom radii and optional
    /// "ears" (the flared top corners of a hardware notch). Corners are cubic
    /// Béziers (circle approximation) so they render identically everywhere.
    /// `geo` upside down about the line y = `cy` (a notch rising from a
    /// taskbar at the bottom).
    pub fn flip_y(&self, geo: &ID2D1Geometry, cy: f32) -> Option<ID2D1Geometry> {
        let m = windows_numerics::Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: -1.0, M31: 0.0, M32: 2.0 * cy };
        unsafe { g().d2d.CreateTransformedGeometry(geo, &m).ok().and_then(|t| t.cast().ok()) }
    }

    pub fn notch_geo(&self, x: f32, y: f32, w: f32, h: f32, rt: f32, rb: f32, ear: f32) -> Option<ID2D1Geometry> {
        const K: f32 = 0.5523;
        unsafe {
            let geo = g().d2d.CreatePathGeometry().ok()?;
            let sink = geo.Open().ok()?;
            let rt = rt.min(h / 2.0).min(w / 2.0);
            let rb = rb.min(h / 2.0).min(w / 2.0);
            let bez = |sink: &ID2D1GeometrySink, p1: (f32, f32), p2: (f32, f32), p3: (f32, f32)| {
                sink.AddBezier(&D2D1_BEZIER_SEGMENT { point1: pt(p1.0, p1.1), point2: pt(p2.0, p2.1), point3: pt(p3.0, p3.1) });
            };
            let (r, b) = (x + w, y + h);
            if ear > 0.0 {
                sink.BeginFigure(pt(x - ear, y), D2D1_FIGURE_BEGIN_FILLED);
                sink.AddLine(pt(r + ear, y));
                // concave flare into the right side
                bez(&sink, (r + ear * (1.0 - K), y), (r, y + ear * (1.0 - K)), (r, y + ear));
            } else {
                sink.BeginFigure(pt(x + rt, y), D2D1_FIGURE_BEGIN_FILLED);
                sink.AddLine(pt(r - rt, y));
                if rt > 0.0 {
                    bez(&sink, (r - rt + rt * K, y), (r, y + rt - rt * K), (r, y + rt));
                }
            }
            sink.AddLine(pt(r, b - rb));
            if rb > 0.0 {
                bez(&sink, (r, b - rb + rb * K), (r - rb + rb * K, b), (r - rb, b));
            }
            sink.AddLine(pt(x + rb, b));
            if rb > 0.0 {
                bez(&sink, (x + rb - rb * K, b), (x, b - rb + rb * K), (x, b - rb));
            }
            if ear > 0.0 {
                sink.AddLine(pt(x, y + ear));
                bez(&sink, (x, y + ear * (1.0 - K)), (x - ear * (1.0 - K), y), (x - ear, y));
            } else {
                sink.AddLine(pt(x, y + rt));
                if rt > 0.0 {
                    bez(&sink, (x, y + rt - rt * K), (x + rt - rt * K, y), (x + rt, y));
                }
            }
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close().ok()?;
            geo.cast().ok()
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            self.free_bitmap();
            let _ = DeleteDC(self.dc);
        }
    }
}

/// Render a 32x32 app icon (a small pill) for the tray and window class.
pub fn make_app_icon(size: i32) -> HICON {
    unsafe {
        let Ok(mut s) = Surface::new() else { return LoadIconW(None, IDI_APPLICATION).unwrap_or_default() };
        s.resize(size, size);
        if !s.begin(size as f32 / 32.0) {
            return HICON::default();
        }
        draw_logo(&s, 0.0, 0.0, 32.0, 1.0, 1.45);
        let _ = s.end();
        let mask = CreateBitmap(size, size, 1, 1, None);
        let info = ICONINFO { fIcon: TRUE, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: s.bmp };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(mask.into());
        icon
    }
}

/// Phemio's logo (assets/phemio.svg: a neon trumpet whose tube becomes the
/// island's pill, on a black rounded square) in an `sz` square at (x, y).
/// Small logos use the bolder small drawing, as the icon file does at
/// 16–32 px; the picture is scaled once per size (and per window).
pub fn draw_logo(s: &Surface, x: f32, y: f32, sz: f32, a: f32, _zoom: f32) {
    static BIG: &[u8] = include_bytes!("../assets/phemio-256.png");
    static SMALL: &[u8] = include_bytes!("../assets/phemio-small-64.png");
    thread_local! {
        static CACHE: RefCell<Vec<(usize, u32, Image)>> = const { RefCell::new(Vec::new()) };
    }
    let px = ((sz * s.scale).round() as u32).clamp(8, 256);
    let rt = s.base.as_raw() as usize;
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if !c.iter().any(|e| e.0 == rt && e.1 == px) {
            let bytes = if px <= 40 { SMALL } else { BIG };
            let Ok(img) = Image::from_bytes(bytes, px, 0.0) else { return };
            if c.len() > 12 {
                c.remove(0);
            }
            c.push((rt, px, img));
        }
        if let Some(e) = c.iter().find(|e| e.0 == rt && e.1 == px) {
            s.image(&e.2, rf(x, y, sz, sz), a);
        }
    });
}

/// One pass of a box blur over premultiplied BGRA (along rows, or columns).
fn box_blur(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize, rows: bool) {
    let (n, lines) = if rows { (w, h) } else { (h, w) };
    let at = |line: usize, i: usize| if rows { (line * w + i) * 4 } else { (i * w + line) * 4 };
    let div = (2 * r + 1) as u32;
    for line in 0..lines {
        let mut sum = [0u32; 4];
        for k in 0..=2 * r {
            let i = k.saturating_sub(r).min(n - 1);
            let o = at(line, i);
            for c in 0..4 {
                sum[c] += src[o + c] as u32;
            }
        }
        for i in 0..n {
            let o = at(line, i);
            for c in 0..4 {
                dst[o + c] = (sum[c] / div) as u8;
            }
            let out = at(line, i.saturating_sub(r));
            let inn = at(line, (i + r + 1).min(n - 1));
            for c in 0..4 {
                sum[c] = sum[c] + src[inn + c] as u32 - src[out + c] as u32;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Self-test (Phemio.exe --gfx-test)
// ---------------------------------------------------------------------------

/// Draws the same pictures with the graphics card and with the processor —
/// plain shapes and text, clips, the frosted panel (which ends a frame in
/// the middle), a part of a frame, a picture — and reports for each whether
/// it worked, whether the two came out alike, and how long frames take.
/// Needs no window. The report goes to `gfx-test.txt` in the data folder.
pub fn self_test() -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "Phemio {} graphics self-test", env!("CARGO_PKG_VERSION"));
    CHOSEN.store(true, Relaxed);
    CALIB.with(|c| c.borrow_mut().phase = 0);
    let font = g().font("Segoe UI", 15.0, 400, 0);
    let icon = Image::from_bytes(include_bytes!("../assets/phemio-256.png"), 96, 0.0).ok();

    // One frame of a made-up island: `step` moves things a little.
    fn scene(s: &mut Surface, scale: f32, step: u32, what: u32, font: &IDWriteTextFormat, icon: Option<&Image>) -> bool {
        if !s.begin(scale) {
            return false;
        }
        let (w, h) = (s.dip_w(), s.dip_h());
        let t = step as f32;
        s.fill_rr(rf(0.0, 0.0, w, h), 28.0, rgb(0x101014, 1.0));
        s.fill_rr_grad(rf(12.0, 12.0, w - 24.0, 60.0), 14.0, rgb(0x22c8ff, 0.9), rgb(0x0a5a8a, 0.9));
        for i in 0..24 {
            let x = 16.0 + (i % 8) as f32 * (w - 40.0) / 8.0;
            let y = 90.0 + (i / 8) as f32 * 44.0;
            s.fill_rr(rf(x, y, 36.0, 36.0), 9.0, rgb(0x2a2a30 + i * 0x050301, 1.0));
            s.text("Abc 123", font, rf(x, y + 8.0, 60.0, 20.0), rgb(0xffffff, 0.9));
        }
        s.fill_circle(40.0 + t, 42.0, 14.0, rgb(0xff453a, 1.0));
        if let Some(i) = icon {
            s.image(i, rf(w - 80.0, 16.0, 52.0, 52.0), 1.0);
        }
        s.text("The quick brown fox jumps over the lazy dog — 0123456789", font, rf(70.0, 30.0, w - 170.0, 22.0), rgb(0xffffff, 1.0));
        if what >= 1 {
            // Clips, nested, with a transform in between.
            s.clip(rf(20.0, 80.0, w - 40.0, h - 100.0));
            s.scale_about(w / 2.0, h / 2.0, 1.0);
            s.clip(rf(40.0, 100.0, w - 80.0, 80.0));
            s.fill_rect(rf(0.0, 0.0, w, h), rgb(0x30d158, 0.25));
            if what >= 2 {
                // The frosted panel, inside both clips.
                s.backdrop_blur(rf(60.0, 104.0 + t * 0.2, w - 120.0, 56.0), 28.0, 18.0, 1.0);
                s.fill_rr(rf(60.0, 104.0 + t * 0.2, w - 120.0, 56.0), 28.0, rgb(0x1c1c1e, 0.6));
            }
            s.unclip();
            s.reset_scale();
            s.fill_rect(rf(0.0, h - 30.0, w, 30.0), rgb(0xffd60a, 0.3));
            s.unclip();
        }
        s.end()
    }

    let sizes: [(&str, i32, i32, f32); 2] = [("island", 870, 420, 1.5), ("settings", 2094, 1692, 1.5)];
    let names = ["plain", "clips", "frosted panel", "part of a frame"];
    let mut shots: Vec<Vec<Option<Vec<u8>>>> = Vec::new();
    for hw in [true, false] {
        let kind = if hw { "graphics card" } else { "processor" };
        HARDWARE.store(hw, Relaxed);
        FAILS.store(0, Relaxed);
        GEN.fetch_add(1, Relaxed);
        let mut mine: Vec<Option<Vec<u8>>> = Vec::new();
        let Ok(mut s) = Surface::new() else {
            let _ = writeln!(out, "{kind}: no surface could be made");
            shots.push(mine);
            continue;
        };
        if s.hw != hw {
            let _ = writeln!(out, "{kind}: NOT AVAILABLE (a {} target was made instead)", if s.hw { "hardware" } else { "software" });
            shots.push(mine);
            continue;
        }
        s.resize(sizes[0].1, sizes[0].2);
        for (i, name) in names.iter().enumerate() {
            let ok = if i < 3 {
                scene(&mut s, 1.5, 3, i as u32, &font, icon.as_ref())
            } else {
                // A whole frame, then only a strip of it drawn again.
                let a = scene(&mut s, 1.5, 3, 1, &font, icon.as_ref());
                let b = s.begin_part(1.5, rf(0.0, 100.0, 580.0, 40.0));
                if b {
                    s.fill_rect(rf(0.0, 100.0, 580.0, 40.0), rgb(0x0a84ff, 1.0));
                    s.text("part", &font, rf(20.0, 108.0, 200.0, 20.0), rgb(0xffffff, 1.0));
                }
                a && b && s.end()
            };
            let px = unsafe { std::slice::from_raw_parts(s.bits, (s.w * s.h * 4) as usize) }.to_vec();
            let lit = px.chunks_exact(4).filter(|p| p[3] != 0).count();
            let still = s.hw == hw && hardware() == hw && s.gen == GEN.load(Relaxed);
            let _ = writeln!(out, "{kind}: {name}: {}{} ({} of {} pixels drawn)", if ok { "ok" } else { "FAILED" }, if still { "" } else { " — and the target was given up" }, lit, px.len() / 4);
            mine.push(if ok { Some(px) } else { None });
            if !still {
                // (Carry on with a new target of the kind being tested.)
                HARDWARE.store(hw, Relaxed);
                FAILS.store(0, Relaxed);
                GEN.fetch_add(1, Relaxed);
            }
        }
        // How long frames take (the picture moves a little each frame).
        for (label, w, h, sc) in sizes {
            s.resize(w, h);
            for what in [1u32, 2] {
                let mut times: Vec<f32> = Vec::new();
                let mut failed = 0;
                for step in 0..60u32 {
                    let t = std::time::Instant::now();
                    if !scene(&mut s, sc, step, what, &font, icon.as_ref()) {
                        failed += 1;
                        HARDWARE.store(hw, Relaxed);
                        FAILS.store(0, Relaxed);
                    }
                    times.push(t.elapsed().as_secs_f32() * 1000.0);
                }
                let first = times[0];
                let mut rest = times[5..].to_vec();
                rest.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let _ = writeln!(
                    out,
                    "{kind}: {label} {w}×{h}{}: median {:.2} ms, slowest {:.2} ms, first frame {:.2} ms{}",
                    if what == 2 { " with the frosted panel" } else { "" },
                    rest[rest.len() / 2],
                    rest[rest.len() - 1],
                    first,
                    if failed > 0 { format!(", {failed} FAILED") } else { String::new() }
                );
            }
        }
        shots.push(mine);
    }
    // Alike?
    if shots.len() == 2 {
        for (i, name) in names.iter().enumerate() {
            match (shots[0].get(i).and_then(|x| x.as_ref()), shots[1].get(i).and_then(|x| x.as_ref())) {
                (Some(a), Some(b)) if a.len() == b.len() => {
                    let (mut far, mut worst) = (0usize, 0u8);
                    for (p, q) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
                        let d = (0..4).map(|c| p[c].abs_diff(q[c])).max().unwrap_or(0);
                        worst = worst.max(d);
                        if d > 40 {
                            far += 1;
                        }
                    }
                    let _ = writeln!(out, "alike: {name}: {far} of {} pixels differ clearly (biggest difference {worst} of 255)", a.len() / 4);
                }
                _ => {
                    let _ = writeln!(out, "alike: {name}: not compared (one of the two failed)");
                }
            }
        }
    }
    out
}
