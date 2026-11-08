//! QR codes for the search: make one from text (QR(text)), and read one from
//! an image you paste (a screenshot or a copied picture / image file).

use std::sync::Arc;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::CF_DIB;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

/// A QR code: `n`×`n` modules, true = dark.
#[derive(Debug)]
pub struct Matrix {
    pub n: usize,
    pub dark: Vec<bool>,
}

pub fn make(text: &str) -> Option<Arc<Matrix>> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M).ok()?;
    let n = code.width();
    let dark = code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect();
    Some(Arc::new(Matrix { n, dark }))
}

/// Put the code on the clipboard as a picture (8 px per module, white border).
pub fn copy_image(m: &Matrix) -> bool {
    let scale = 8usize;
    let quiet = 4usize;
    let side = (m.n + quiet * 2) * scale;
    let stride = side * 4;
    let header = 40usize;
    let total = header + stride * side;
    unsafe {
        let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, total) else { return false };
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            return false;
        }
        let buf = std::slice::from_raw_parts_mut(p, total);
        // BITMAPINFOHEADER, 32 bpp, top-down.
        buf[..header].fill(0);
        buf[0..4].copy_from_slice(&(header as u32).to_le_bytes());
        buf[4..8].copy_from_slice(&(side as i32).to_le_bytes());
        buf[8..12].copy_from_slice(&(-(side as i32)).to_le_bytes());
        buf[12..14].copy_from_slice(&1u16.to_le_bytes());
        buf[14..16].copy_from_slice(&32u16.to_le_bytes());
        buf[20..24].copy_from_slice(&((stride * side) as u32).to_le_bytes());
        for y in 0..side {
            for x in 0..side {
                let (mx, my) = ((x / scale) as isize - quiet as isize, (y / scale) as isize - quiet as isize);
                let dark = mx >= 0 && my >= 0 && (mx as usize) < m.n && (my as usize) < m.n && m.dark[my as usize * m.n + mx as usize];
                let v = if dark { 0u8 } else { 255u8 };
                let o = header + y * stride + x * 4;
                buf[o..o + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        let _ = GlobalUnlock(h);
        if OpenClipboard(None).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let ok = SetClipboardData(CF_DIB.0 as u32, Some(HANDLE(h.0))).is_ok();
        let _ = CloseClipboard();
        ok
    }
}

/// Is there a picture (or an image file) on the clipboard?
pub fn clipboard_has_image() -> bool {
    unsafe { IsClipboardFormatAvailable(CF_DIB.0 as u32).is_ok() || image_file_on_clipboard().is_some() }
}

fn image_file_on_clipboard() -> Option<String> {
    unsafe {
        if IsClipboardFormatAvailable(15).is_err() {
            return None;
        }
        OpenClipboard(None).ok()?;
        let mut out = None;
        if let Ok(h) = GetClipboardData(15) {
            let drop = HDROP(h.0);
            let len = DragQueryFileW(drop, 0, None);
            let mut b = vec![0u16; len as usize + 1];
            DragQueryFileW(drop, 0, Some(&mut b));
            let path = crate::util::from_wide(&b);
            let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
            if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp" | "tif" | "tiff" | "jfif") {
                out = Some(path);
            }
        }
        let _ = CloseClipboard();
        out
    }
}

/// Grey pixels of the clipboard picture (w, h, bytes).
pub(crate) fn clipboard_gray() -> Option<(usize, usize, Vec<u8>)> {
    if let Some(path) = image_file_on_clipboard() {
        return file_gray(&path);
    }
    unsafe {
        OpenClipboard(None).ok()?;
        let r = (|| {
            let h = GetClipboardData(CF_DIB.0 as u32).ok()?;
            let size = GlobalSize(HGLOBAL(h.0));
            let p = GlobalLock(HGLOBAL(h.0)) as *const u8;
            if p.is_null() || size < 40 {
                return None;
            }
            let d = std::slice::from_raw_parts(p, size).to_vec();
            let _ = GlobalUnlock(HGLOBAL(h.0));
            dib_gray(&d)
        })();
        let _ = CloseClipboard();
        r
    }
}

fn dib_gray(d: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let rd32 = |o: usize| i32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
    let hs = rd32(0) as usize;
    let w = rd32(4);
    let h = rd32(8);
    let bpp = u16::from_le_bytes([d[14], d[15]]) as usize;
    let comp = rd32(16);
    let colors = rd32(32) as usize;
    if w <= 0 || h == 0 || !(bpp == 24 || bpp == 32) {
        return None;
    }
    let (w, top_down, hh) = (w as usize, h < 0, h.unsigned_abs() as usize);
    // BI_BITFIELDS keeps three masks after a 40-byte header.
    let masks = if comp == 3 && hs == 40 { 12 } else { 0 };
    let off = hs + masks + colors * 4;
    let stride = (w * bpp / 8 + 3) & !3;
    let mut out = vec![255u8; w * hh];
    for y in 0..hh {
        let row = if top_down { y } else { hh - 1 - y };
        let base = off + row * stride;
        for x in 0..w {
            let o = base + x * (bpp / 8);
            if o + 2 >= d.len() {
                return None;
            }
            let (b, g, r) = (d[o] as u32, d[o + 1] as u32, d[o + 2] as u32);
            out[y * w + x] = ((r * 299 + g * 587 + b * 114) / 1000) as u8;
        }
    }
    Some((w, hh, out))
}

fn file_gray(path: &str) -> Option<(usize, usize, Vec<u8>)> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let p = crate::util::wide(path);
        let dec = wic.CreateDecoderFromFilename(PCWSTR(p.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()?;
        let frame = dec.GetFrame(0).ok()?;
        let conv = wic.CreateFormatConverter().ok()?;
        conv.Initialize(&frame, &GUID_WICPixelFormat8bppGray, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        conv.GetSize(&mut w, &mut h).ok()?;
        let mut buf = vec![0u8; (w * h) as usize];
        conv.CopyPixels(std::ptr::null(), w, &mut buf).ok()?;
        Some((w as usize, h as usize, buf))
    }
}

/// The clipboard picture as an image file (for the thumbnail in the box).
pub fn clipboard_image_bytes() -> Option<Vec<u8>> {
    if let Some(path) = image_file_on_clipboard() {
        return std::fs::read(path).ok();
    }
    unsafe {
        OpenClipboard(None).ok()?;
        let r = (|| {
            let h = GetClipboardData(CF_DIB.0 as u32).ok()?;
            let size = GlobalSize(HGLOBAL(h.0));
            let p = GlobalLock(HGLOBAL(h.0)) as *const u8;
            if p.is_null() || size < 40 {
                return None;
            }
            let dib = std::slice::from_raw_parts(p, size).to_vec();
            let _ = GlobalUnlock(HGLOBAL(h.0));
            // A .bmp is the DIB with a 14-byte file header in front.
            let hs = u32::from_le_bytes([dib[0], dib[1], dib[2], dib[3]]) as usize;
            let comp = u32::from_le_bytes([dib[16], dib[17], dib[18], dib[19]]);
            let colors = u32::from_le_bytes([dib[32], dib[33], dib[34], dib[35]]) as usize;
            let masks = if comp == 3 && hs == 40 { 12 } else { 0 };
            let off = 14 + hs + masks + colors * 4;
            let mut bmp = Vec::with_capacity(14 + dib.len());
            bmp.extend_from_slice(b"BM");
            bmp.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
            bmp.extend_from_slice(&[0, 0, 0, 0]);
            bmp.extend_from_slice(&(off as u32).to_le_bytes());
            bmp.extend_from_slice(&dib);
            Some(bmp)
        })();
        let _ = CloseClipboard();
        r
    }
}

/// Read the QR code in the pasted picture.
pub fn read_clipboard() -> std::result::Result<String, String> {
    let tr = crate::i18n::tr;
    let (w, h, px) = clipboard_gray().ok_or_else(|| tr("Couldn't read the picture").to_string())?;
    // Huge screenshots: halve until it's a sensible size (QR codes stay readable).
    let (mut w, mut h, mut px) = (w, h, px);
    while w * h > 4_000_000 && w > 400 && h > 400 {
        let (nw, nh) = (w / 2, h / 2);
        let mut n = vec![0u8; nw * nh];
        for y in 0..nh {
            for x in 0..nw {
                let s = px[(y * 2) * w + x * 2] as u32 + px[(y * 2) * w + x * 2 + 1] as u32 + px[(y * 2 + 1) * w + x * 2] as u32 + px[(y * 2 + 1) * w + x * 2 + 1] as u32;
                n[y * nw + x] = (s / 4) as u8;
            }
        }
        (w, h, px) = (nw, nh, n);
    }
    let mut img = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| px[y * w + x]);
    let grids = img.detect_grids();
    for g in grids {
        if let Ok((_, text)) = g.decode() {
            return Ok(text);
        }
    }
    Err(tr("No QR code found in the picture").to_string())
}
