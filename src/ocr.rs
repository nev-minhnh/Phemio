//! Reading the text in a pasted picture (Windows' own OCR, Windows.Media.Ocr).
//! Runs on a worker thread (MTA). Windows reads the languages whose OCR
//! packs are installed (Settings → Time & language → Language).

use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;

/// (See `read_gray`.)
/// The text in the clipboard's picture, as Markdown that keeps its layout.
pub fn read_clipboard() -> Result<String, String> {
    let tr = crate::i18n::tr;
    let (w, h, px) = crate::qr::clipboard_gray().ok_or_else(|| tr("Couldn't read the picture").to_string())?;
    read_gray(w, h, px)
}

pub fn read_gray(w: usize, h: usize, px: Vec<u8>) -> Result<String, String> {
    let tr = crate::i18n::tr;
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .ok()
        .or_else(|| {
            // None of your languages has an OCR pack: any language that has one.
            let langs = OcrEngine::AvailableRecognizerLanguages().ok()?;
            let first = langs.GetAt(0).ok()?;
            OcrEngine::TryCreateFromLanguage(&first).ok()
        })
        .ok_or_else(|| tr("Windows has no text reader (OCR) installed for your languages").to_string())?;
    let lang = engine.RecognizerLanguage().and_then(|l| l.LanguageTag()).map(|t| t.to_string().to_lowercase()).unwrap_or_default();
    let max = OcrEngine::MaxImageDimension().unwrap_or(10_000) as usize;
    let (mut w, mut h, mut px) = (w, h, px);
    // Small screenshots: twice the size reads small text much better.
    if w.max(h) * 2 <= max.min(2400) && w * h <= 1_500_000 {
        (w, h, px) = double(w, h, &px);
    }
    // Too big for the reader: halve until it fits.
    while w.max(h) > max && w > 2 && h > 2 {
        (w, h, px) = halve(w, h, &px);
    }
    let fail = |e: windows_core::Error| format!("{}: {}", tr("Couldn't read the text in the picture"), e.message());
    // As 32-bit BGRA (opaque), the format every OCR engine takes.
    let mut bgra = Vec::with_capacity(px.len() * 4);
    for v in &px {
        bgra.extend_from_slice(&[*v, *v, *v, 255]);
    }
    drop(px);
    let dw = DataWriter::new().map_err(fail)?;
    dw.WriteBytes(&bgra).map_err(fail)?;
    let buf = dw.DetachBuffer().map_err(fail)?;
    let bmp = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(&buf, BitmapPixelFormat::Bgra8, w as i32, h as i32, BitmapAlphaMode::Premultiplied).map_err(fail)?;
    let res = engine.RecognizeAsync(&bmp).map_err(fail)?.join().map_err(fail)?;
    // Chinese and Japanese: no spaces between the words.
    let tight = lang.starts_with("zh") || lang.starts_with("ja");
    let mut lines: Vec<Line> = Vec::new();
    for line in res.Lines().map_err(fail)? {
        let mut words = Vec::new();
        if let Ok(ws) = line.Words() {
            for w in ws {
                let (Ok(t), Ok(r)) = (w.Text(), w.BoundingRect()) else { continue };
                let t = t.to_string();
                if !t.trim().is_empty() {
                    words.push(Word { text: t, x: r.X, y: r.Y, w: r.Width, h: r.Height });
                }
            }
        }
        if !words.is_empty() {
            lines.push(Line { words });
        }
    }
    if lines.is_empty() {
        return Err(tr("No text found in the picture").to_string());
    }
    Ok(layout(lines, tight))
}

struct Word {
    text: String,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

struct Line {
    words: Vec<Word>,
}

/// A run of words on one visual row, with no wide gap inside.
struct Seg {
    text: String,
    x0: f32,
    top: f32,
    bottom: f32,
}

fn join(words: &[&Word], tight: bool) -> String {
    let mut s = String::new();
    for (i, w) in words.iter().enumerate() {
        if i > 0 && !tight {
            s.push(' ');
        }
        s.push_str(&w.text);
    }
    s
}

fn median(mut v: Vec<f32>) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

/// Words with their places → Markdown that keeps the picture's layout:
/// headings (bigger text), paragraphs (lines joined), lists, and tables
/// (rows whose words stand in separate columns).
fn layout(lines: Vec<Line>, tight: bool) -> String {
    let all_h: Vec<f32> = lines.iter().flat_map(|l| l.words.iter().map(|w| w.h)).collect();
    let body_h = median(all_h).max(4.0);
    // 1) Segments: a line's words split where the gap is wide (a column).
    let mut segs: Vec<Seg> = Vec::new();
    for l in &lines {
        let mut ws: Vec<&Word> = l.words.iter().collect();
        ws.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
        let h = median(ws.iter().map(|w| w.h).collect()).max(body_h * 0.6);
        let mut cur: Vec<&Word> = Vec::new();
        let flush = |cur: &mut Vec<&Word>, segs: &mut Vec<Seg>| {
            if cur.is_empty() {
                return;
            }
            let x0 = cur.iter().map(|w| w.x).fold(f32::MAX, f32::min);
            let top = cur.iter().map(|w| w.y).fold(f32::MAX, f32::min);
            let bottom = cur.iter().map(|w| w.y + w.h).fold(f32::MIN, f32::max);
            segs.push(Seg { text: join(cur, tight), x0, top, bottom });
            cur.clear();
        };
        for w in ws {
            if let Some(last) = cur.last() {
                if w.x - (last.x + last.w) > h * 1.6 {
                    flush(&mut cur, &mut segs);
                }
            }
            cur.push(w);
        }
        flush(&mut cur, &mut segs);
    }
    // 2) Visual rows: segments side by side (their middles at the same height).
    segs.sort_by(|a, b| a.top.partial_cmp(&b.top).unwrap_or(std::cmp::Ordering::Equal));
    let mut rows: Vec<Vec<Seg>> = Vec::new();
    for sg in segs {
        let mid = (sg.top + sg.bottom) / 2.0;
        let fit = rows.iter_mut().rev().take(3).find(|r| {
            let t = r.iter().map(|s| s.top).fold(f32::MAX, f32::min);
            let b = r.iter().map(|s| s.bottom).fold(f32::MIN, f32::max);
            mid > t && mid < b
        });
        match fit {
            Some(r) => r.push(sg),
            None => rows.push(vec![sg]),
        }
    }
    for r in rows.iter_mut() {
        r.sort_by(|a, b| a.x0.partial_cmp(&b.x0).unwrap_or(std::cmp::Ordering::Equal));
    }
    rows.sort_by(|a, b| a[0].top.partial_cmp(&b[0].top).unwrap_or(std::cmp::Ordering::Equal));
    // 3) Blocks: runs of rows with several columns become tables; the rest
    // paragraphs, headings and list items.
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    let mut para = String::new();
    let mut last_bottom = f32::MIN;
    let mut last_left = 0.0f32;
    let flush_para = |para: &mut String, out: &mut Vec<String>| {
        if !para.trim().is_empty() {
            out.push(para.trim().to_string());
        }
        para.clear();
    };
    while i < rows.len() {
        // A table: two or more rows in a row with 2+ columns.
        let mut j = i;
        while j < rows.len() && rows[j].len() >= 2 {
            j += 1;
        }
        if j - i >= 2 {
            flush_para(&mut para, &mut out);
            out.push(table(&rows[i..j]));
            last_bottom = rows[j - 1].iter().map(|s| s.bottom).fold(f32::MIN, f32::max);
            i = j;
            continue;
        }
        let row = &rows[i];
        let text = row.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join("    ");
        let top = row[0].top;
        let h = row.iter().map(|s| s.bottom - s.top).fold(0.0f32, f32::max);
        let left = row[0].x0;
        let gap = top - last_bottom;
        let t = text.trim_start();
        let bullet = ["•", "◦", "▪", "■", "●", "-", "–", "*", "·"].iter().find_map(|b| t.strip_prefix(b).map(|r| r.trim_start().to_string()));
        let numbered = t.split_once(['.', ')']).map(|(n, _)| !n.is_empty() && n.len() <= 3 && n.chars().all(|c| c.is_ascii_digit())).unwrap_or(false);
        if h > body_h * 1.45 {
            flush_para(&mut para, &mut out);
            let level = if h > body_h * 2.0 { "#" } else { "##" };
            out.push(format!("{level} {t}"));
        } else if let Some(b) = bullet {
            flush_para(&mut para, &mut out);
            para = format!("- {b}");
        } else if numbered {
            flush_para(&mut para, &mut out);
            para = t.to_string();
        } else if !para.is_empty() && gap < h * 0.9 && (left - last_left).abs() < h * 3.0 {
            // The same paragraph, wrapped onto the next line.
            if para.ends_with('-') && !tight {
                para.pop();
            } else if !tight {
                para.push(' ');
            }
            para.push_str(t);
        } else {
            flush_para(&mut para, &mut out);
            para = t.to_string();
        }
        last_bottom = row.iter().map(|s| s.bottom).fold(f32::MIN, f32::max);
        last_left = left;
        i += 1;
    }
    flush_para(&mut para, &mut out);
    out.join("\n\n")
}

/// Rows of columns → a Markdown table (columns lined up by where they start).
fn table(rows: &[Vec<Seg>]) -> String {
    // Column starts: every segment's left edge, merged when close.
    let mut starts: Vec<f32> = rows.iter().flat_map(|r| r.iter().map(|s| s.x0)).collect();
    starts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let h = median(rows.iter().flat_map(|r| r.iter().map(|s| s.bottom - s.top)).collect()).max(4.0);
    let mut cols: Vec<f32> = Vec::new();
    for x in starts {
        if cols.last().map(|c| x - c > h * 1.5).unwrap_or(true) {
            cols.push(x);
        }
    }
    let n = cols.len().max(1);
    let col_of = |x: f32| -> usize {
        let mut best = 0;
        for (k, c) in cols.iter().enumerate() {
            if x + h * 0.75 >= *c {
                best = k;
            }
        }
        best
    };
    let esc = |s: &str| s.replace('|', "\\|");
    let mut lines = Vec::new();
    for (ri, r) in rows.iter().enumerate() {
        let mut cells = vec![String::new(); n];
        for sg in r {
            let k = col_of(sg.x0);
            if !cells[k].is_empty() {
                cells[k].push(' ');
            }
            cells[k].push_str(&esc(&sg.text));
        }
        lines.push(format!("| {} |", cells.join(" | ")));
        if ri == 0 {
            lines.push(format!("|{}|", vec![" --- "; n].join("|")));
        }
    }
    lines.join("\n")
}

fn double(w: usize, h: usize, px: &[u8]) -> (usize, usize, Vec<u8>) {
    let (nw, nh) = (w * 2, h * 2);
    let mut out = vec![0u8; nw * nh];
    for y in 0..nh {
        let sy = (y as f32 - 0.5) / 2.0;
        let y0 = sy.floor().max(0.0) as usize;
        let y1 = (y0 + 1).min(h - 1);
        let fy = (sy - y0 as f32).clamp(0.0, 1.0);
        for x in 0..nw {
            let sx = (x as f32 - 0.5) / 2.0;
            let x0 = sx.floor().max(0.0) as usize;
            let x1 = (x0 + 1).min(w - 1);
            let fx = (sx - x0 as f32).clamp(0.0, 1.0);
            let a = px[y0 * w + x0] as f32 * (1.0 - fx) + px[y0 * w + x1] as f32 * fx;
            let b = px[y1 * w + x0] as f32 * (1.0 - fx) + px[y1 * w + x1] as f32 * fx;
            out[y * nw + x] = (a * (1.0 - fy) + b * fy).round() as u8;
        }
    }
    (nw, nh, out)
}

fn halve(w: usize, h: usize, px: &[u8]) -> (usize, usize, Vec<u8>) {
    let (nw, nh) = (w / 2, h / 2);
    let mut n = vec![0u8; nw * nh];
    for y in 0..nh {
        for x in 0..nw {
            let s = px[(y * 2) * w + x * 2] as u32 + px[(y * 2) * w + x * 2 + 1] as u32 + px[(y * 2 + 1) * w + x * 2] as u32 + px[(y * 2 + 1) * w + x * 2 + 1] as u32;
            n[y * nw + x] = (s / 4) as u8;
        }
    }
    (nw, nh, n)
}

/// The clipboard's picture for Gemini: (mime type, bytes). A PNG or JPEG file
/// as it is; anything else (a screenshot) as a greyscale PNG.
pub fn clipboard_image() -> Option<(&'static str, Vec<u8>)> {
    if let Some(b) = crate::qr::clipboard_image_bytes() {
        if b.starts_with(&[0x89, b'P', b'N', b'G']) && b.len() < 15 << 20 {
            return Some(("image/png", b));
        }
        if b.starts_with(&[0xFF, 0xD8]) && b.len() < 15 << 20 {
            return Some(("image/jpeg", b));
        }
    }
    let (mut w, mut h, mut px) = crate::qr::clipboard_gray()?;
    while w.max(h) > 3072 && w > 2 && h > 2 {
        (w, h, px) = halve(w, h, &px);
    }
    Some(("image/png", png_gray(w, h, &px)))
}

/// A minimal 8-bit greyscale PNG.
fn png_gray(w: usize, h: usize, px: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut raw = Vec::with_capacity((w + 1) * h);
    for y in 0..h {
        raw.push(0u8);
        raw.extend_from_slice(&px[y * w..(y + 1) * w]);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = z.write_all(&raw);
    let idat = z.finish().unwrap_or_default();
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut chunk = |kind: &[u8], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = Vec::with_capacity(4 + data.len());
        c.extend_from_slice(kind);
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc32(&c).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &idat);
    chunk(b"IEND", &[]);
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}
