//! Quick controls: Wi-Fi and Bluetooth radios, airplane mode, microphone
//! mute and screen brightness. A worker thread watches them (once a second —
//! these are cheap reads) and carries out toggles, so nothing slow ever runs
//! on the UI thread. Changes are posted to the host window.
#![allow(non_snake_case)] // IRadioManager keeps Windows' method names

use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Devices::Radios::*;
use windows::Win32::Foundation::*;
use windows::Win32::Media::Audio::Endpoints::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Variant::*;
use windows::Win32::System::Wmi::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// None: no such radio.
    pub wifi: Option<bool>,
    pub bt: Option<bool>,
    pub airplane: Option<bool>,
    /// Microphone muted? None: no microphone.
    pub mic_muted: Option<bool>,
    pub mic_name: String,
    /// Built-in screen brightness 0–100 (None: not adjustable this way).
    pub brightness: Option<u8>,
    /// Apps use the dark theme.
    pub dark: Option<bool>,
}

pub enum Cmd {
    ToggleWifi,
    ToggleBt,
    ToggleMic,
    ToggleAirplane,
    ToggleDark,
    Brightness(u8),
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static TX: Mutex<Option<Sender<Cmd>>> = Mutex::new(None);
static HOST: Mutex<Option<(isize, u32)>> = Mutex::new(None);

/// A toggle shown before Windows has done it: [Wi-Fi, Bluetooth, airplane,
/// mic muted] → (the state it's going to, give up after). Readings keep
/// showing it until Windows gets there; a toggle that fails drops it, so
/// the real state comes back at once.
static PENDING: Mutex<[Option<(bool, Instant)>; 4]> = Mutex::new([None; 4]);
const WIFI: usize = 0;
const BT: usize = 1;
const AIR: usize = 2;
const MIC: usize = 3;

fn field(st: &mut State, k: usize) -> &mut Option<bool> {
    match k {
        WIFI => &mut st.wifi,
        BT => &mut st.bt,
        AIR => &mut st.airplane,
        _ => &mut st.mic_muted,
    }
}

pub fn state() -> State {
    STATE.lock().ok().and_then(|s| s.clone()).unwrap_or_default()
}

fn post(first: bool) {
    if let Some((h, msg)) = HOST.lock().ok().and_then(|h| *h) {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), msg, WPARAM(first as usize), LPARAM(0));
        }
    }
}

/// Show the toggle straight away (it's carried out on the worker).
fn flip_now(k: usize) {
    let changed = STATE.lock().map(|mut s| {
        let Some(st) = s.as_mut() else { return false };
        let f = field(st, k);
        let Some(v) = *f else { return false };
        *f = Some(!v);
        if let Ok(mut p) = PENDING.lock() {
            p[k] = Some((!v, Instant::now() + Duration::from_secs(8)));
        }
        true
    });
    if changed.unwrap_or(false) {
        post(false);
    }
}

/// The toggle didn't happen: show the real state again.
fn failed(k: usize) {
    if let Ok(mut p) = PENDING.lock() {
        p[k] = None;
    }
}

pub fn send(c: Cmd) {
    match c {
        Cmd::ToggleWifi => flip_now(WIFI),
        Cmd::ToggleBt => flip_now(BT),
        Cmd::ToggleAirplane => flip_now(AIR),
        Cmd::ToggleMic => flip_now(MIC),
        _ => {}
    }
    if let Ok(t) = TX.lock() {
        if let Some(tx) = t.as_ref() {
            let _ = tx.send(c);
        }
    }
}

/// Start the worker; `msg` is posted to `hwnd` with the previous state's
/// change flags whenever something changes (wparam 1 = first reading).
pub fn start(hwnd: HWND, msg: u32) {
    let (tx, rx) = channel::<Cmd>();
    if let Ok(mut t) = TX.lock() {
        *t = Some(tx);
    }
    let h = hwnd.0 as isize;
    if let Ok(mut x) = HOST.lock() {
        *x = Some((h, msg));
    }
    let _ = std::thread::Builder::new().name("controls".into()).stack_size(512 * 1024).spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let mut radios: Vec<Radio> = Vec::new();
        let mut radios_age = 999u32;
        let mut wmi = Wmi::open();
        let mut first = true;
        let mut tick = 0u32;
        let mut bright: Option<u8> = None;
        loop {
            // Radios can appear (USB dongle) — re-list now and then.
            if radios_age >= 30 {
                radios = list_radios();
                radios_age = 0;
            }
            radios_age += 1;
            let mut st = State {
                wifi: radio_state(&radios, RadioKind::WiFi),
                bt: radio_state(&radios, RadioKind::Bluetooth),
                airplane: airplane(),
                dark: dark_mode(),
                ..Default::default()
            };
            if let Some((m, name)) = mic() {
                st.mic_muted = Some(m);
                st.mic_name = name;
            }
            // WMI is the slow one: brightness every 4 s (and right after a change).
            if tick % 4 == 0 {
                bright = wmi.as_ref().and_then(|w| w.brightness());
            }
            tick = tick.wrapping_add(1);
            st.brightness = bright;
            fake(&mut st);
            // Toggles still on their way: keep showing where they're going.
            if let Ok(mut p) = PENDING.lock() {
                for k in 0..4 {
                    if let Some((to, until)) = p[k] {
                        let f = field(&mut st, k);
                        if *f == Some(to) || f.is_none() || Instant::now() >= until {
                            p[k] = None;
                        } else {
                            *f = Some(to);
                        }
                    }
                }
            }
            let changed = STATE.lock().map(|mut s| {
                let ch = s.as_ref() != Some(&st);
                *s = Some(st.clone());
                ch
            });
            if changed.unwrap_or(false) {
                let _ = PostMessageW(Some(HWND(h as *mut _)), msg, WPARAM(first as usize), LPARAM(0));
            }
            first = false;
            match rx.recv_timeout(Duration::from_millis(1000)) {
                Ok(cmd) => match cmd {
                    // Developer aid: ISLET_SLOW_TOGGLE=<ms> — a radio that takes its time.
                    _ if std::env::var("ISLET_SLOW_TOGGLE").is_ok() && !matches!(cmd, Cmd::Brightness(_) | Cmd::ToggleDark) && {
                        std::thread::sleep(Duration::from_millis(std::env::var("ISLET_SLOW_TOGGLE").ok().and_then(|v| v.parse().ok()).unwrap_or(2000)));
                        false
                    } => {}
                    Cmd::ToggleWifi => {
                        if !toggle_radio(&radios, RadioKind::WiFi) {
                            failed(WIFI);
                        }
                    }
                    Cmd::ToggleBt => {
                        if !toggle_radio(&radios, RadioKind::Bluetooth) {
                            failed(BT);
                        }
                    }
                    Cmd::ToggleMic => {
                        if !toggle_mic() {
                            failed(MIC);
                        }
                    }
                    Cmd::ToggleAirplane => {
                        if !toggle_airplane(&radios) {
                            failed(AIR);
                        }
                    }
                    Cmd::ToggleDark => toggle_dark(),
                    Cmd::Brightness(b) => {
                        if wmi.is_none() {
                            wmi = Wmi::open();
                        }
                        if let Some(w) = wmi.as_ref() {
                            w.set_brightness(b);
                            bright = Some(b);
                            tick = 1; // re-read a few seconds later
                        }
                    }
                },
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        CoUninitialize();
    });
}

/// Developer aid: ISLET_FAKE_CTRL="wifi=1;bt=0;air=0;mic=1;bright=70"
fn fake(st: &mut State) {
    let Ok(v) = std::env::var("ISLET_FAKE_CTRL") else { return };
    for part in v.split(';') {
        let Some((k, val)) = part.split_once('=') else { continue };
        let b = val.trim() == "1";
        match k.trim() {
            "wifi" => st.wifi = Some(b),
            "bt" => st.bt = Some(b),
            "air" => st.airplane = Some(b),
            "mic" => {
                st.mic_muted = Some(b);
                st.mic_name = "Microphone Array".into();
            }
            "bright" => st.brightness = val.trim().parse().ok(),
            "dark" => st.dark = Some(b),
            _ => {}
        }
    }
}

fn list_radios() -> Vec<Radio> {
    let Ok(op) = Radio::GetRadiosAsync() else { return vec![] };
    let Ok(v) = op.join() else { return vec![] };
    (0..v.Size().unwrap_or(0)).filter_map(|i| v.GetAt(i).ok()).collect()
}

fn radio_state(radios: &[Radio], kind: RadioKind) -> Option<bool> {
    let mut found = None;
    for r in radios {
        if r.Kind().ok() == Some(kind) {
            let on = r.State().ok() == Some(RadioState::On);
            found = Some(found.unwrap_or(false) || on);
        }
    }
    found
}

fn toggle_radio(radios: &[Radio], kind: RadioKind) -> bool {
    // Desktop apps may change radios when "Let apps control device radios"
    // is allowed; otherwise open the matching Settings page instead.
    let allowed = Radio::RequestAccessAsync().ok().and_then(|op| op.join().ok()) == Some(RadioAccessStatus::Allowed);
    let on = radio_state(radios, kind).unwrap_or(false);
    let mut done = false;
    if allowed {
        for r in radios.iter().filter(|r| r.Kind().ok() == Some(kind)) {
            let to = if on { RadioState::Off } else { RadioState::On };
            if r.SetStateAsync(to).ok().and_then(|op| op.join().ok()) == Some(RadioAccessStatus::Allowed) {
                done = true;
            }
        }
    }
    if !done {
        crate::util::shell_open(if kind == RadioKind::WiFi { "ms-settings:network-wifi" } else { "ms-settings:bluetooth" });
    }
    done
}

/// Windows' radio manager (what the Airplane mode tile in Quick Settings
/// talks to). Undocumented but stable since Windows 8.
#[windows::core::interface("db3afbfb-08e6-46c6-aa70-bf9a34c30ab7")]
unsafe trait IRadioManager: windows::core::IUnknown {
    unsafe fn IsRMSupported(&self, supported: *mut i32) -> HRESULT;
    unsafe fn GetUIRadioInstances(&self, instances: *mut *mut core::ffi::c_void) -> HRESULT;
    unsafe fn GetSystemRadioState(&self, enabled: *mut i32, policy: *mut i32, reason: *mut i32) -> HRESULT;
    unsafe fn SetSystemRadioState(&self, enabled: i32) -> HRESULT;
    unsafe fn Refresh(&self) -> HRESULT;
    unsafe fn OnHardwareSliderChange(&self, a: i32, b: i32) -> HRESULT;
}

const CLSID_RADIO_MANAGER: GUID = GUID::from_u128(0x581333f6_28db_41be_bc7a_ff201f12f3f6);

/// Created once per thread (this is read every second).
fn radio_manager() -> Option<IRadioManager> {
    thread_local!(static RM: std::cell::RefCell<Option<Option<IRadioManager>>> = const { std::cell::RefCell::new(None) });
    RM.with(|c| {
        c.borrow_mut()
            .get_or_insert_with(|| unsafe { CoCreateInstance(&CLSID_RADIO_MANAGER, None, CLSCTX_ALL).ok() })
            .clone()
    })
}

/// Airplane mode on/off directly. If the radio manager isn't reachable,
/// switch every radio instead (same effect for Wi-Fi and Bluetooth).
fn toggle_airplane(radios: &[Radio]) -> bool {
    let now_on = airplane().unwrap_or(false);
    if let Some(rm) = radio_manager() {
        // "System radio state enabled" means radios on, i.e. airplane mode off.
        if unsafe { rm.SetSystemRadioState(if now_on { 1 } else { 0 }) }.is_ok() {
            return true;
        }
    }
    let allowed = Radio::RequestAccessAsync().ok().and_then(|op| op.join().ok()) == Some(RadioAccessStatus::Allowed);
    if allowed {
        let to = if now_on { RadioState::On } else { RadioState::Off };
        let mut any = false;
        for r in radios {
            any |= r.SetStateAsync(to).ok().and_then(|op| op.join().ok()) == Some(RadioAccessStatus::Allowed);
        }
        any
    } else {
        crate::util::shell_open("ms-settings:network-airplanemode");
        false
    }
}

const PERSONALIZE: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");

fn dark_mode() -> Option<bool> {
    unsafe {
        let mut v = 1u32;
        let mut sz = 4u32;
        let r = RegGetValueW(HKEY_CURRENT_USER, PERSONALIZE, w!("AppsUseLightTheme"), RRF_RT_REG_DWORD, None, Some(&mut v as *mut u32 as *mut core::ffi::c_void), Some(&mut sz));
        (r == ERROR_SUCCESS).then_some(v == 0)
    }
}

/// Switch apps and Windows between light and dark, then tell everyone.
fn toggle_dark() {
    let to_light: u32 = if dark_mode().unwrap_or(false) { 1 } else { 0 };
    unsafe {
        for name in [w!("AppsUseLightTheme"), w!("SystemUsesLightTheme")] {
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, PERSONALIZE, name, REG_DWORD.0, Some(&to_light as *const u32 as *const core::ffi::c_void), 4);
        }
        let msg = crate::util::wide("ImmersiveColorSet");
        let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageTimeoutW(
            windows::Win32::UI::WindowsAndMessaging::HWND_BROADCAST,
            windows::Win32::UI::WindowsAndMessaging::WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(msg.as_ptr() as isize),
            windows::Win32::UI::WindowsAndMessaging::SMTO_ABORTIFHUNG,
            200,
            None,
        );
    }
}

/// Airplane mode, from the radio manager (or the state it records).
fn airplane() -> Option<bool> {
    if let Some(rm) = radio_manager() {
        let (mut en, mut pol, mut why) = (0i32, 0i32, 0i32);
        if unsafe { rm.GetSystemRadioState(&mut en, &mut pol, &mut why) }.is_ok() {
            return Some(en == 0);
        }
    }
    unsafe {
        let mut v = 0u32;
        let mut sz = 4u32;
        let r = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SYSTEM\\CurrentControlSet\\Control\\RadioManagement\\SystemRadioState"),
            PCWSTR::null(),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut core::ffi::c_void),
            Some(&mut sz),
        );
        (r == ERROR_SUCCESS).then_some(v == 1)
    }
}

fn mic_endpoint() -> Option<(IAudioEndpointVolume, String)> {
    unsafe {
        let dev = crate::audio::enumerator()?.GetDefaultAudioEndpoint(eCapture, eCommunications).ok()?;
        let ep: IAudioEndpointVolume = dev.Activate(CLSCTX_ALL, None).ok()?;
        let name = dev
            .OpenPropertyStore(STGM_READ)
            .ok()
            .and_then(|st| st.GetValue(&windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName).ok())
            .and_then(|v| windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc(&v).ok())
            .map(|p| {
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as *const _));
                s
            })
            .unwrap_or_default();
        Some((ep, name))
    }
}

fn mic() -> Option<(bool, String)> {
    let (ep, name) = mic_endpoint()?;
    let m = unsafe { ep.GetMute().ok()?.as_bool() };
    Some((m, name))
}

fn toggle_mic() -> bool {
    let Some((ep, _)) = mic_endpoint() else { return false };
    unsafe {
        let m = ep.GetMute().map(|b| b.as_bool()).unwrap_or(false);
        ep.SetMute(!m, std::ptr::null()).is_ok()
    }
}

// ---------------------------------------------------------------------------
// Built-in display brightness (WMI, ROOT\WMI)
// ---------------------------------------------------------------------------

struct Wmi {
    svc: IWbemServices,
}

impl Wmi {
    fn open() -> Option<Wmi> {
        unsafe {
            let loc: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER).ok()?;
            let empty = BSTR::new();
            let svc = loc.ConnectServer(&BSTR::from("ROOT\\WMI"), &empty, &empty, &empty, 0, &empty, None).ok()?;
            let _ = CoSetProxyBlanket(&svc, 10, 0, PCWSTR::null(), RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, None, EOAC_NONE);
            let w = Wmi { svc };
            // Only keep it if there is a brightness-capable screen.
            w.brightness().map(|_| w)
        }
    }

    fn first(&self, query: &str) -> Option<IWbemClassObject> {
        unsafe {
            let en = self.svc.ExecQuery(&BSTR::from("WQL"), &BSTR::from(query), WBEM_FLAG_RETURN_IMMEDIATELY | WBEM_FLAG_FORWARD_ONLY, None).ok()?;
            let mut objs: [Option<IWbemClassObject>; 1] = [None];
            let mut n = 0u32;
            let _ = en.Next(2000, &mut objs, &mut n);
            if n == 1 { objs[0].take() } else { None }
        }
    }

    fn brightness(&self) -> Option<u8> {
        unsafe {
            let o = self.first("SELECT CurrentBrightness FROM WmiMonitorBrightness WHERE Active=TRUE")?;
            let mut v = VARIANT::default();
            o.Get(w!("CurrentBrightness"), 0, &mut v, None, None).ok()?;
            Some(v.Anonymous.Anonymous.Anonymous.bVal.min(100))
        }
    }

    fn set_brightness(&self, b: u8) {
        unsafe {
            let Some(inst) = self.first("SELECT * FROM WmiMonitorBrightnessMethods WHERE Active=TRUE") else { return };
            let mut pv = VARIANT::default();
            if inst.Get(w!("__PATH"), 0, &mut pv, None, None).is_err() {
                return;
            }
            let path: BSTR = (*pv.Anonymous.Anonymous.Anonymous.bstrVal).clone();
            let mut class: Option<IWbemClassObject> = None;
            if self.svc.GetObject(&BSTR::from("WmiMonitorBrightnessMethods"), WBEM_FLAG_RETURN_WBEM_COMPLETE, None, Some(&mut class), None).is_err() {
                return;
            }
            let Some(class) = class else { return };
            let mut ins: Option<IWbemClassObject> = None;
            let mut outs: Option<IWbemClassObject> = None;
            if class.GetMethod(w!("WmiSetBrightness"), 0, &mut ins, &mut outs).is_err() {
                return;
            }
            let Some(ins) = ins else { return };
            let Ok(args) = ins.SpawnInstance(0) else { return };
            let mut t = VARIANT::default();
            (*t.Anonymous.Anonymous).vt = VT_I4;
            (*t.Anonymous.Anonymous).Anonymous.lVal = 0;
            let mut bv = VARIANT::default();
            (*bv.Anonymous.Anonymous).vt = VT_UI1;
            (*bv.Anonymous.Anonymous).Anonymous.bVal = b.min(100);
            let _ = args.Put(w!("Timeout"), 0, &t, 0);
            let _ = args.Put(w!("Brightness"), 0, &bv, 0);
            let _ = self.svc.ExecMethod(&path, &BSTR::from("WmiSetBrightness"), WBEM_GENERIC_FLAG_TYPE(0), None, &args, None, None);
        }
    }
}
