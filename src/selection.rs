//! The text you've highlighted in any app (for Alt+Q translate).
//! First asks the app politely through UI Automation; apps that don't
//! answer get a quick Ctrl+C, and your clipboard is put back afterwards.

use std::time::{Duration, Instant};

use windows::Win32::System::Com::*;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;

pub fn selected_text() -> Option<String> {
    if let Ok(t) = std::env::var("ISLET_FAKE_SELECTION") {
        return Some(t);
    }
    // Let go of Alt+Q first, or the app would see Ctrl+Alt+C.
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(900) {
        let down = unsafe { GetAsyncKeyState(VK_MENU.0 as i32) < 0 || GetAsyncKeyState(b'Q' as i32) < 0 };
        if !down {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if let Some(t) = via_uia().filter(|t| !t.trim().is_empty()) {
        return Some(t);
    }
    via_clipboard()
}

fn via_uia() -> Option<String> {
    unsafe {
        let ua: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let el = ua.GetFocusedElement().ok()?;
        let tp: IUIAutomationTextPattern = el.GetCurrentPatternAs(UIA_TextPatternId).ok()?;
        let sel = tp.GetSelection().ok()?;
        let mut out = String::new();
        for i in 0..sel.Length().unwrap_or(0) {
            if let Ok(r) = sel.GetElement(i) {
                if let Ok(t) = r.GetText(20_000) {
                    out.push_str(&t.to_string());
                }
            }
        }
        Some(out)
    }
}

fn via_clipboard() -> Option<String> {
    use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
    unsafe {
        let before = GetClipboardSequenceNumber();
        let old = crate::util::clipboard_text();
        let k = |vk: VIRTUAL_KEY, up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, time: 0, dwExtraInfo: 0 } },
        };
        let inputs = [k(VK_CONTROL, false), k(VK_C, false), k(VK_C, true), k(VK_CONTROL, true)];
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(700) {
            if GetClipboardSequenceNumber() != before {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        if GetClipboardSequenceNumber() == before {
            return None;
        }
        // Give the app a moment to finish writing.
        std::thread::sleep(Duration::from_millis(30));
        let text = crate::util::clipboard_text();
        // Put back what was there (text only — other kinds we leave alone).
        if let Some(o) = old {
            crate::util::set_clipboard_text(&o);
        }
        text.filter(|t| !t.trim().is_empty())
    }
}
