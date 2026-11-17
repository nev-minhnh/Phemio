//! Where a frame's time goes (part of `Phemio.exe --gfx-test`): the pieces
//! of drawing timed one by one, with the DC render target Phemio uses and
//! with a Direct3D surface that never leaves the graphics card.

use std::fmt::Write;
use std::time::Instant;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::gfx::{g, rf, rgb};

fn median(v: &mut Vec<f32>) -> f32 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

/// `n` runs of `f` (after a few to warm up): the median, in ms.
fn time(n: usize, mut f: impl FnMut()) -> f32 {
    for _ in 0..4 {
        f();
    }
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let t = Instant::now();
        f();
        v.push(t.elapsed().as_secs_f32() * 1000.0);
    }
    median(&mut v)
}

const WORDS: [&str; 6] = ["Weather", "12:45", "Hanoi, Vietnam", "0.00 KB/s", "Play on several", "Phemio"];

/// The same little jobs on any render target (coordinates in pixels).
unsafe fn jobs(rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, font: &IDWriteTextFormat, layouts: &[IDWriteTextLayout], w: f32, h: f32, what: u32) {
    rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
    match what {
        1 => {
            for i in 0..200 {
                let x = (i % 20) as f32 * w / 20.0;
                let y = (i / 20) as f32 * h / 10.0;
                brush.SetColor(&rgb(0x203040 + i * 0x010203, 1.0));
                rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rf(x + 2.0, y + 2.0, w / 20.0 - 4.0, h / 10.0 - 4.0), radiusX: 6.0, radiusY: 6.0 }, brush);
            }
        }
        2 => {
            brush.SetColor(&rgb(0xffffff, 1.0));
            for i in 0..60usize {
                let x = (i % 6) as f32 * w / 6.0;
                let y = (i / 6) as f32 * h / 10.0;
                let t: Vec<u16> = WORDS[i % 6].encode_utf16().collect();
                rt.DrawText(&t, font, &rf(x, y, w / 6.0, h / 10.0), brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            }
        }
        3 => {
            brush.SetColor(&rgb(0xffffff, 1.0));
            for i in 0..60usize {
                let x = (i % 6) as f32 * w / 6.0;
                let y = (i / 6) as f32 * h / 10.0;
                rt.DrawTextLayout(windows_numerics::Vector2 { X: x, Y: y }, &layouts[i % 6], brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
        }
        4 => {
            brush.SetColor(&rgb(0x101014, 1.0));
            rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rf(0.0, 0.0, w, h), radiusX: 40.0, radiusY: 40.0 }, brush);
        }
        _ => {}
    }
}

const JOBS: [&str; 5] = ["an empty frame", "200 rounded boxes", "60 short texts (DrawText)", "60 short texts (layouts kept)", "one box the size of the frame"];

pub fn run() -> String {
    let mut out = String::new();
    let _ = writeln!(out, "\nWhere the time goes (median of 40 frames, ms)");
    let font = g().font("Segoe UI", 21.0, 400, 0);
    let layouts: Vec<IDWriteTextLayout> = WORDS
        .iter()
        .filter_map(|s| {
            let t: Vec<u16> = s.encode_utf16().collect();
            unsafe { g().dw.CreateTextLayout(&t, &font, 400.0, 60.0).ok() }
        })
        .collect();
    if layouts.len() != 6 {
        let _ = writeln!(out, "no text layouts");
        return out;
    }
    let sizes: [(&str, i32, i32); 3] = [("closed island 420×60", 420, 60), ("open island 870×420", 870, 420), ("settings 2094×1692", 2094, 1692)];

    // ---- 1. The DC render target (what Phemio draws with) ----------------
    for hw in [false, true] {
        let kind = if hw { "DC target, graphics card" } else { "DC target, processor" };
        unsafe {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: if hw { D2D1_RENDER_TARGET_TYPE_HARDWARE } else { D2D1_RENDER_TARGET_TYPE_SOFTWARE },
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let Ok(rt) = g().d2d.CreateDCRenderTarget(&props) else {
                let _ = writeln!(out, "{kind}: can't be made");
                continue;
            };
            let Ok(base) = rt.cast::<ID2D1RenderTarget>() else { continue };
            base.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let Ok(brush) = base.CreateSolidColorBrush(&rgb(0, 1.0), None) else { continue };
            let dc = CreateCompatibleDC(None);
            for (label, w, h) in sizes {
                let bmi = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() }, ..Default::default() };
                let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
                let Ok(bmp) = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else { continue };
                let old = SelectObject(dc, bmp.into());
                let r = RECT { left: 0, top: 0, right: w, bottom: h };
                let mut line = format!("{kind}, {label}:");
                for (j, name) in JOBS.iter().enumerate() {
                    let mut bad = 0;
                    let ms = time(40, || {
                        if rt.BindDC(dc, &r).is_err() {
                            bad += 1;
                            return;
                        }
                        base.BeginDraw();
                        jobs(&base, &brush, &font, &layouts, w as f32, h as f32, j as u32);
                        if base.EndDraw(None, None).is_err() {
                            bad += 1;
                        }
                    });
                    let _ = write!(line, " {name} {ms:.2}{};", if bad > 0 { " (FAILED)" } else { "" });
                }
                // Bound once, not for every frame.
                let _ = rt.BindDC(dc, &r);
                let ms = time(40, || {
                    base.BeginDraw();
                    jobs(&base, &brush, &font, &layouts, w as f32, h as f32, 0);
                    let _ = base.EndDraw(None, None);
                });
                let _ = write!(line, " an empty frame without binding again {ms:.2};");
                // A strip of it only.
                let strip = RECT { left: 0, top: h / 2, right: w, bottom: (h / 2 + 40).min(h) };
                let ms = time(40, || {
                    if rt.BindDC(dc, &strip).is_ok() {
                        base.BeginDraw();
                        jobs(&base, &brush, &font, &layouts, w as f32, 40.0, 0);
                        let _ = base.EndDraw(None, None);
                    }
                });
                let _ = writeln!(line, " an empty 40-pixel strip {ms:.2}");
                out.push_str(&line);
                SelectObject(dc, old);
                let _ = DeleteObject(bmp.into());
            }
            let _ = DeleteDC(dc);
        }
    }

    // ---- 2. Putting a frame on the screen (UpdateLayeredWindow) -----------
    unsafe {
        let hwnd = CreateWindowExW(WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT, w!("Static"), w!(""), WS_POPUP, -6000, -6000, 10, 10, None, None, None, None).unwrap_or_default();
        if hwnd.is_invalid() {
            let _ = writeln!(out, "UpdateLayeredWindow: no window");
        } else {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let dc = CreateCompatibleDC(None);
            for (label, w, h) in sizes {
                let bmi = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() }, ..Default::default() };
                let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
                let Ok(bmp) = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else { continue };
                std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize).fill(0x40);
                let old = SelectObject(dc, bmp.into());
                let present = |dirty: Option<RECT>| {
                    let dst = POINT { x: -6000, y: -6000 };
                    let size = SIZE { cx: w, cy: h };
                    let src = POINT { x: 0, y: 0 };
                    let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
                    let dr = dirty.unwrap_or_default();
                    let info = UPDATELAYEREDWINDOWINFO { cbSize: std::mem::size_of::<UPDATELAYEREDWINDOWINFO>() as u32, hdcDst: HDC::default(), pptDst: &dst, psize: &size, hdcSrc: dc, pptSrc: &src, crKey: COLORREF(0), pblend: &blend, dwFlags: ULW_ALPHA, prcDirty: if dirty.is_some() { &dr } else { std::ptr::null() } };
                    let _ = UpdateLayeredWindowIndirect(hwnd, &info);
                };
                let whole = time(40, || present(None));
                let strip = time(40, || present(Some(RECT { left: 0, top: h / 2, right: w, bottom: (h / 2 + 40).min(h) })));
                let _ = writeln!(out, "UpdateLayeredWindow, {label}: the whole frame {whole:.2}; a 40-pixel strip {strip:.2}");
                SelectObject(dc, old);
                let _ = DeleteObject(bmp.into());
            }
            let _ = DeleteDC(dc);
            let _ = DestroyWindow(hwnd);
        }
    }

    // ---- 3. A Direct3D surface (the frame stays on the graphics card) -----
    match unsafe { d3d(&font, &layouts, &sizes) } {
        Ok(s) => out.push_str(&s),
        Err(e) => {
            let _ = writeln!(out, "Direct3D surface: can't be used here ({e})");
        }
    }
    out
}

unsafe fn d3d(font: &IDWriteTextFormat, layouts: &[IDWriteTextLayout], sizes: &[(&str, i32, i32); 3]) -> Result<String> {
    let mut out = String::new();
    let mut dev: Option<ID3D11Device> = None;
    let mut ctx: Option<ID3D11DeviceContext> = None;
    D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut dev), None, Some(&mut ctx))?;
    let (dev, ctx) = (dev.ok_or_else(|| Error::from(E_FAIL))?, ctx.ok_or_else(|| Error::from(E_FAIL))?);
    let dxgi: IDXGIDevice = dev.cast()?;
    if let Ok(ad) = dxgi.GetAdapter() {
        if let Ok(d) = ad.GetDesc() {
            let name = String::from_utf16_lossy(&d.Description);
            let _ = writeln!(out, "Direct3D surface: on {}", name.split('\0').next().unwrap_or(""));
        }
    }
    let f1: ID2D1Factory1 = g().d2d.cast()?;
    let d2dev = f1.CreateDevice(&dxgi)?;
    let dc = d2dev.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
    dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
    let base: ID2D1RenderTarget = dc.cast()?;
    let brush = base.CreateSolidColorBrush(&rgb(0, 1.0), None)?;
    for (label, w, h) in sizes.iter().copied() {
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
        dev.CreateTexture2D(&desc, None, Some(&mut tex))?;
        let tex = tex.ok_or_else(|| Error::from(E_FAIL))?;
        let surf: IDXGISurface = tex.cast()?;
        let bp = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        let target = dc.CreateBitmapFromDxgiSurface(&surf, Some(&bp))?;
        dc.SetTarget(&target);
        // A copy the processor can read (what bringing a frame back costs).
        let staging_desc = D3D11_TEXTURE2D_DESC { Usage: D3D11_USAGE_STAGING, BindFlags: 0, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32, ..desc };
        let mut staging: Option<ID3D11Texture2D> = None;
        dev.CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
        let staging = staging.ok_or_else(|| Error::from(E_FAIL))?;
        let mut line = format!("Direct3D surface, {label}:");
        for (j, name) in JOBS.iter().enumerate() {
            let mut bad = 0;
            let ms = time(40, || {
                base.BeginDraw();
                jobs(&base, &brush, font, layouts, w as f32, h as f32, j as u32);
                if base.EndDraw(None, None).is_err() {
                    bad += 1;
                }
                ctx.Flush();
            });
            let _ = write!(line, " {name} {ms:.2}{};", if bad > 0 { " (FAILED)" } else { "" });
        }
        // The same, waiting until the graphics card has really finished
        // (by reading the frame back).
        let ms = time(40, || {
            base.BeginDraw();
            jobs(&base, &brush, font, layouts, w as f32, h as f32, 1);
            let _ = base.EndDraw(None, None);
            ctx.CopyResource(&staging, &tex);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            if ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m)).is_ok() {
                ctx.Unmap(&staging, 0);
            }
        });
        let _ = writeln!(line, " 200 rounded boxes and the frame read back {ms:.2}");
        out.push_str(&line);
        dc.SetTarget(None);
    }
    Ok(out)
}
