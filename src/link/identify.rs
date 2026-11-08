//! Link → Identify: a big number on every screen for a few seconds (the same
//! numbers as on the map, on both PCs).

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS: PCWSTR = w!("PhemioIdentify");
const SHOW_MS: u32 = 3500;

/// Show `(number, what's written under it, screen rectangle in pixels)` (UI thread).
pub fn show(items: &[(usize, String, (i32, i32, i32, i32))]) {
    static REG: std::sync::Once = std::sync::Once::new();
    REG.call_once(|| crate::util::register_class(CLASS, Some(proc_), HICON::default()));
    for (n, label, (x, y, w, h)) in items {
        let side = ((*w).min(*h) as f32 * 0.32).clamp(180.0, 420.0) as i32;
        let (bw, bh) = (side, (side as f32 * 1.12) as i32);
        let title = crate::util::wide(&format!("{n}\n{label}"));
        unsafe {
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
                CLASS,
                PCWSTR(title.as_ptr()),
                WS_POPUP,
                x + (w - bw) / 2,
                y + (h - bh) / 2,
                bw,
                bh,
                None,
                None,
                Some(crate::util::hinstance()),
                None,
            ) else {
                continue;
            };
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 235, LWA_ALPHA);
            let rgn = CreateRoundRectRgn(0, 0, bw + 1, bh + 1, side / 6, side / 6);
            let _ = SetWindowRgn(hwnd, Some(rgn), false);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = SetTimer(Some(hwnd), 1, SHOW_MS, None);
        }
    }
}

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_NCHITTEST => LRESULT(-1), // HTTRANSPARENT
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let bg = CreateSolidBrush(COLORREF(0x00261c1c));
            FillRect(dc, &rc, bg);
            let _ = DeleteObject(bg.into());
            let mut buf = [0u16; 512];
            let n = GetWindowTextW(hwnd, &mut buf) as usize;
            let text = String::from_utf16_lossy(&buf[..n]);
            let (num, label) = text.split_once('\n').unwrap_or((&text, ""));
            let h = rc.bottom - rc.top;
            SetBkMode(dc, TRANSPARENT);
            let font = |px: i32, weight: i32| {
                CreateFontW(-px, 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, (DEFAULT_PITCH.0 | FF_SWISS.0) as u32, w!("Segoe UI"))
            };
            let big = font((h as f32 * 0.6) as i32, 700);
            let old = SelectObject(dc, big.into());
            SetTextColor(dc, COLORREF(0x00ffffff));
            let mut r1 = RECT { left: 0, top: (h as f32 * 0.04) as i32, right: rc.right, bottom: (h as f32 * 0.74) as i32 };
            let mut w1: Vec<u16> = num.encode_utf16().collect();
            DrawTextW(dc, &mut w1, &mut r1, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            let small = font((h as f32 * 0.075).max(13.0) as i32, 400);
            SelectObject(dc, small.into());
            SetTextColor(dc, COLORREF(0x00d8d0d0));
            let mut r2 = RECT { left: 10, top: (h as f32 * 0.74) as i32, right: rc.right - 10, bottom: rc.bottom - 8 };
            let mut w2: Vec<u16> = label.encode_utf16().collect();
            DrawTextW(dc, &mut w2, &mut r2, DT_CENTER | DT_WORDBREAK | DT_END_ELLIPSIS);
            SelectObject(dc, old);
            let _ = DeleteObject(big.into());
            let _ = DeleteObject(small.into());
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
