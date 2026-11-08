//! Tiny Direct2D/DirectWrite/WIC layer.
//!
//! Every window draws into a 32-bit DIB with a *software* Direct2D DC render
//! target and is pushed to the screen with UpdateLayeredWindowIndirect. The
//! software target means no Direct3D device and no GPU driver DLLs get loaded,
//! which is the single biggest reason the app stays far below 50 MB.

use std::cell::RefCell;
use std::ffi::c_void;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
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
    cache: RefCell<Option<ID2D1Bitmap>>,
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
        if let Some(b) = self.cache.borrow().as_ref() {
            return Some(b.clone());
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
        *self.cache.borrow_mut() = Some(b.clone());
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
    pub rt: ID2D1DCRenderTarget,
    base: ID2D1RenderTarget,
    brush: ID2D1SolidColorBrush,
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
}

/// Long lines that don't fit scroll sideways: still for a moment, then slowly
/// to their end, a pause, and back to the start.
const MQ_PAUSE: f32 = 2.0;
const MQ_END: f32 = 1.6;
const MQ_SPEED: f32 = 30.0;

thread_local! {
    /// Natural width of (format, text).
    static MQ_W: RefCell<std::collections::HashMap<(usize, String), f32>> = RefCell::new(std::collections::HashMap::new());
    /// When each scrolling line was first seen (and last).
    static MQ_T: RefCell<std::collections::HashMap<u64, (std::time::Instant, std::time::Instant)>> = RefCell::new(std::collections::HashMap::new());
}

impl Surface {
    pub fn new() -> Result<Surface> {
        let g = g();
        unsafe {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let rt = g.d2d.CreateDCRenderTarget(&props)?;
            let base: ID2D1RenderTarget = rt.cast()?;
            base.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let brush = base.CreateSolidColorBrush(&rgb(0, 1.0), None)?;
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
            Ok(Surface { rt, base, brush, round_cap, dash, dc, bmp: HBITMAP::default(), old: HGDIOBJ::default(), bits: std::ptr::null_mut(), w: 0, h: 0, scale: 1.0, marquee: std::cell::Cell::new(None) })
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
        self.scale = scale;
        self.marquee.set(None);
        unsafe {
            let r = RECT { left: 0, top: 0, right: self.w, bottom: self.h };
            if self.rt.BindDC(self.dc, &r).is_err() {
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
        self.scale = scale;
        self.marquee.set(None);
        let r = RECT {
            left: ((part.left * scale).floor() as i32).clamp(0, self.w),
            top: ((part.top * scale).floor() as i32).clamp(0, self.h),
            right: ((part.right * scale).ceil() as i32).clamp(0, self.w),
            bottom: ((part.bottom * scale).ceil() as i32).clamp(0, self.h),
        };
        if r.right <= r.left || r.bottom <= r.top {
            return false;
        }
        unsafe {
            if self.rt.BindDC(self.dc, &r).is_err() {
                return false;
            }
            self.base.SetDpi(96.0 * scale, 96.0 * scale);
            self.base.BeginDraw();
            // Same coordinates as a full draw.
            self.base.SetTransform(&windows_numerics::Matrix3x2::translation(-r.left as f32 / scale, -r.top as f32 / scale));
        }
        true
    }

    pub fn end(&self) {
        unsafe {
            let _ = self.base.EndDraw(None, None);
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
        let mut h = std::collections::hash_map::DefaultHasher::new();
        s.hash(&mut h);
        (rw as i32).hash(&mut h);
        (f.as_raw() as usize).hash(&mut h);
        let id = h.finish();
        let now = std::time::Instant::now();
        let start = MQ_T.with(|m| {
            let mut m = m.borrow_mut();
            if m.len() > 300 {
                m.retain(|_, (_, seen)| now.duration_since(*seen).as_secs_f32() < 10.0);
            }
            let e = m.entry(id).or_insert((now, now));
            // Gone from sight for a while: starts over from the beginning.
            if now.duration_since(e.1).as_secs_f32() > MQ_PAUSE + 3.0 {
                e.0 = now;
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
            self.base.DrawTextLayout(windows_numerics::Vector2 { X: r.left - off.round(), Y: r.top }, &l, self.b(c), D2D1_DRAW_TEXT_OPTIONS_NONE);
            self.base.PopAxisAlignedClip();
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
    pub fn backdrop_blur(&self, r: D2D_RECT_F, corner: f32, radius: f32, opacity: f32) {
        if self.bits.is_null() {
            return;
        }
        let sc = self.scale;
        let x0 = ((r.left * sc).floor() as i32).clamp(0, self.w);
        let y0 = ((r.top * sc).floor() as i32).clamp(0, self.h);
        let x1 = ((r.right * sc).ceil() as i32).clamp(0, self.w);
        let y1 = ((r.bottom * sc).ceil() as i32).clamp(0, self.h);
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        if w < 4 || h < 4 {
            return;
        }
        unsafe {
            // What's been drawn so far, into the bitmap.
            let _ = self.base.Flush(None, None);
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
        unsafe {
            let props = D2D1_BITMAP_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0 * sc,
                dpiY: 96.0 * sc,
            };
            let Ok(bmp) = self.base.CreateBitmap(D2D_SIZE_U { width: w as u32, height: h as u32 }, Some(buf.as_ptr() as *const _), (w * 4) as u32, &props) else { return };
            let Ok(brush) = self.base.CreateBitmapBrush(&bmp, None, None) else { return };
            brush.SetOpacity(opacity);
            brush.SetTransform(&windows_numerics::Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: x0 as f32 / sc, M32: y0 as f32 / sc });
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
        unsafe { self.base.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED) }
    }
    pub fn unclip(&self) {
        unsafe { self.base.PopAxisAlignedClip() }
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
        s.end();
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
