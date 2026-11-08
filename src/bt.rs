//! Connected Bluetooth devices (classic + LE) and their battery level when
//! Windows reports one (hands-free headsets / earbuds / speakers usually do).
//! Runs on a short-lived worker thread; results are posted to the host.

use std::sync::Mutex;

use windows::core::*;
use windows::Devices::Bluetooth::*;
use windows::Devices::Enumeration::*;
use windows_collections::IIterable;
use windows::Foundation::IReference;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Headphones,
    Headset,
    Speaker,
    Mouse,
    Keyboard,
    Gamepad,
    Pen,
    Phone,
    Computer,
    Watch,
    Other,
}

impl Kind {
    /// Palette colour for this kind of device.
    pub fn color(self) -> u32 {
        use crate::palette::*;
        match self {
            Kind::Headphones | Kind::Headset => PINK,
            Kind::Speaker => ORANGE,
            Kind::Mouse => CYAN,
            Kind::Keyboard => INDIGO,
            Kind::Gamepad => GREEN,
            Kind::Pen => YELLOW,
            Kind::Phone => MINT,
            Kind::Computer => TEAL,
            Kind::Watch => PURPLE,
            Kind::Other => BLUE,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Headphones => "Headphones",
            Kind::Headset => "Headset",
            Kind::Speaker => "Speaker",
            Kind::Mouse => "Mouse",
            Kind::Keyboard => "Keyboard",
            Kind::Gamepad => "Controller",
            Kind::Pen => "Pen",
            Kind::Phone => "Phone",
            Kind::Computer => "Computer",
            Kind::Watch => "Watch",
            Kind::Other => "Device",
        }
    }
    /// Segoe Fluent Icons glyph.
    pub fn glyph(self) -> &'static str {
        match self {
            Kind::Headphones | Kind::Headset => "\u{E7F6}",
            Kind::Speaker => "\u{E7F5}",
            Kind::Mouse => "\u{E962}",
            Kind::Keyboard => "\u{E765}",
            Kind::Gamepad => "\u{E7FC}",
            Kind::Pen => "\u{EDC6}",
            Kind::Phone => "\u{E8EA}",
            Kind::Computer => "\u{E7F8}",
            Kind::Watch => "\u{E916}",
            Kind::Other => "\u{E702}",
        }
    }
    pub fn audio(self) -> bool {
        matches!(self, Kind::Headphones | Kind::Headset | Kind::Speaker)
    }

    /// Classic Bluetooth class of device (major/minor bits).
    pub(crate) fn from_cod(raw: u32) -> Kind {
        let major = (raw >> 8) & 0x1f;
        let minor = (raw >> 2) & 0x3f;
        match major {
            1 => Kind::Computer,
            2 => Kind::Phone,
            4 => match minor {
                1 | 2 => Kind::Headset,
                5 | 7 | 8 | 10 => Kind::Speaker,
                _ => Kind::Headphones,
            },
            5 => match (raw >> 6) & 3 {
                1 | 3 => Kind::Keyboard,
                2 => Kind::Mouse,
                _ => match minor & 0xf {
                    1 | 2 => Kind::Gamepad,
                    5 | 8 => Kind::Pen,
                    _ => Kind::Other,
                },
            },
            7 => Kind::Watch,
            _ => Kind::Other,
        }
    }

    /// Bluetooth LE GAP appearance (category << 6 | subcategory).
    pub(crate) fn from_appearance(raw: u16) -> Kind {
        let (cat, sub) = (raw >> 6, raw & 0x3f);
        match cat {
            0x01 => Kind::Phone,
            0x02 => Kind::Computer,
            0x03 => Kind::Watch,
            0x0f => match sub {
                1 => Kind::Keyboard,
                2 => Kind::Mouse,
                3 | 4 => Kind::Gamepad,
                5 | 7 => Kind::Pen,
                _ => Kind::Other,
            },
            0x21 => Kind::Speaker,
            0x25 => if sub == 2 { Kind::Headset } else { Kind::Headphones },
            _ => Kind::Other,
        }
    }

    /// Last resort: guess from the name.
    pub fn from_name(name: &str) -> Kind {
        let n = name.to_lowercase();
        let has = |w: &[&str]| w.iter().any(|x| n.contains(x));
        if has(&["mouse", "mx master", "mx anywhere", "trackball", "trackpad"]) {
            Kind::Mouse
        } else if has(&["keyboard", "keys", "mx mechanical", "k380", "k780"]) {
            Kind::Keyboard
        } else if has(&["controller", "gamepad", "xbox", "dualsense", "dualshock", "joy-con"]) {
            Kind::Gamepad
        } else if has(&["buds", "airpods", "headphone", "earbud", "wh-", "wf-", "quietcomfort", "freebuds", "beats"]) {
            Kind::Headphones
        } else if has(&["headset", "hands-free"]) {
            Kind::Headset
        } else if has(&["speaker", "soundlink", "jbl", "boom", "soundcore", "flex"]) {
            Kind::Speaker
        } else if has(&["pen", "stylus"]) {
            Kind::Pen
        } else if has(&["watch", "band"]) {
            Kind::Watch
        } else if has(&["phone", "iphone", "galaxy", "pixel"]) {
            Kind::Phone
        } else {
            Kind::Other
        }
    }
}

#[derive(Clone, Debug)]
pub struct BtDevice {
    pub name: String,
    pub battery: Option<u8>,
    pub kind: Kind,
    /// Seen through the Bluetooth LE stack only.
    pub le: bool,
    /// "AA:BB:CC:DD:EE:FF", when Windows gives it.
    pub address: String,
}

impl BtDevice {
    pub fn simple(name: &str, battery: Option<u8>) -> BtDevice {
        BtDevice { name: name.to_string(), battery, kind: Kind::from_name(name), le: false, address: String::new() }
    }
}

static RESULT: Mutex<Option<(Vec<BtDevice>, crate::netinfo::NetInfo)>> = Mutex::new(None);
static BUSY: Mutex<bool> = Mutex::new(false);

pub fn take() -> Option<(Vec<BtDevice>, crate::netinfo::NetInfo)> {
    RESULT.lock().ok().and_then(|mut r| r.take())
}

/// Devnode property Windows uses for Bluetooth hands-free battery level.
const BATTERY_KEY: &str = "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2";
const CONTAINER_KEY: &str = "System.Devices.ContainerId";
const AEP_CONTAINER_KEY: &str = "System.Devices.Aep.ContainerId";

pub fn spawn_scan(hwnd: HWND, msg: u32) {
    {
        let Ok(mut b) = BUSY.lock() else { return };
        if *b {
            return;
        }
        *b = true;
    }
    let h = hwnd.0 as isize;
    let _ = std::thread::Builder::new().stack_size(512 * 1024).spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let list = std::panic::catch_unwind(|| scan().unwrap_or_default()).unwrap_or_default();
        let net = crate::netinfo::query();
        if let Ok(mut r) = RESULT.lock() {
            *r = Some((list, net));
        }
        if let Ok(mut b) = BUSY.lock() {
            *b = false;
        }
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), msg, WPARAM(0), LPARAM(0));
            CoUninitialize();
        }
    });
}

struct Found {
    id: HSTRING,
    name: String,
    le: bool,
}

fn found_for(aqs: Result<HSTRING>, le: bool, out: &mut Vec<Found>) {
    let Ok(aqs) = aqs else { return };
    let Ok(op) = DeviceInformation::FindAllAsyncAqsFilter(&aqs) else { return };
    let Ok(col) = op.join() else { return };
    for i in 0..col.Size().unwrap_or(0) {
        let Ok(d) = col.GetAt(i) else { continue };
        let Ok(name) = d.Name() else { continue };
        let name = name.to_string();
        if name.trim().is_empty() || out.iter().any(|f| f.name.eq_ignore_ascii_case(&name)) {
            continue;
        }
        out.push(Found { id: d.Id().unwrap_or_default(), name, le });
    }
}

fn fmt_addr(a: u64) -> String {
    if a == 0 {
        return String::new();
    }
    (0..6).rev().map(|i| format!("{:02X}", (a >> (i * 8)) & 0xff)).collect::<Vec<_>>().join(":")
}

/// Kind and address from the typed Bluetooth objects.
fn details(f: &Found) -> (Kind, String) {
    if f.le {
        if let Ok(dev) = BluetoothLEDevice::FromIdAsync(&f.id).and_then(|op| op.join()) {
            let kind = dev.Appearance().and_then(|a| a.RawValue()).map(Kind::from_appearance).unwrap_or(Kind::Other);
            let addr = fmt_addr(dev.BluetoothAddress().unwrap_or(0));
            let _ = dev.Close();
            return (kind, addr);
        }
    } else if let Ok(dev) = BluetoothDevice::FromIdAsync(&f.id).and_then(|op| op.join()) {
        let kind = dev.ClassOfDevice().and_then(|c| c.RawValue()).map(Kind::from_cod).unwrap_or(Kind::Other);
        let addr = fmt_addr(dev.BluetoothAddress().unwrap_or(0));
        let _ = dev.Close();
        return (kind, addr);
    }
    (Kind::Other, String::new())
}

fn scan() -> Result<Vec<BtDevice>> {
    let mut found = Vec::new();
    found_for(BluetoothDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected), false, &mut found);
    found_for(BluetoothLEDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected), true, &mut found);
    if found.is_empty() {
        return Ok(vec![]);
    }
    // Battery levels live on device nodes ("Bose Flex 2 SoundLink Hands-Free AG",
    // the LE device of a mouse…). Match them to the device by its container
    // (one physical device), falling back to the name.
    let mut levels: Vec<(String, Option<GUID>, u8)> = Vec::new();
    let props: IIterable<HSTRING> = IIterable::from(vec![HSTRING::from(BATTERY_KEY), HSTRING::from(CONTAINER_KEY)]);
    // Narrow query first ("property is set"); fall back to all devices.
    let narrow = HSTRING::from(format!("{BATTERY_KEY}:<>[]"));
    let all = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(&narrow, &props, DeviceInformationKind::Device)
        .and_then(|op| op.join())
        .or_else(|_| DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(&HSTRING::new(), &props, DeviceInformationKind::Device).and_then(|op| op.join()));
    if let Ok(col) = all {
        for i in 0..col.Size().unwrap_or(0) {
            let Ok(d) = col.GetAt(i) else { continue };
            let Ok(map) = d.Properties() else { continue };
            let Ok(v) = map.Lookup(&HSTRING::from(BATTERY_KEY)) else { continue };
            let Some(b) = v.cast::<IReference<u8>>().ok().and_then(|r| r.Value().ok()) else { continue };
            let cont = map.Lookup(&HSTRING::from(CONTAINER_KEY)).ok().and_then(|c| c.cast::<IReference<GUID>>().ok()).and_then(|r| r.Value().ok());
            levels.push((d.Name().map(|n| n.to_string()).unwrap_or_default(), cont, b.min(100)));
        }
    }
    let container_of = |id: &HSTRING| -> Option<GUID> {
        let keys: IIterable<HSTRING> = IIterable::from(vec![HSTRING::from(AEP_CONTAINER_KEY), HSTRING::from(CONTAINER_KEY)]);
        let di = DeviceInformation::CreateFromIdAsyncAdditionalProperties(id, &keys).and_then(|op| op.join()).ok()?;
        let map = di.Properties().ok()?;
        for k in [AEP_CONTAINER_KEY, CONTAINER_KEY] {
            if let Some(g) = map.Lookup(&HSTRING::from(k)).ok().and_then(|c| c.cast::<IReference<GUID>>().ok()).and_then(|r| r.Value().ok()) {
                return Some(g);
            }
        }
        None
    };
    let mut out: Vec<BtDevice> = found
        .iter()
        .map(|f| {
            let ln = f.name.to_lowercase();
            let cont = container_of(&f.id);
            // Same physical device: the freshest node is the one Windows keeps
            // updated, and a stale node never shows more than a fresh one did
            // once it's charged, so take the highest.
            let by_container = cont.and_then(|c| levels.iter().filter(|(_, lc, _)| *lc == Some(c)).map(|l| l.2).max());
            let battery = by_container.or_else(|| {
                levels
                    .iter()
                    .filter(|(dn, _, _)| {
                        let d = dn.to_lowercase();
                        !d.is_empty() && (d.contains(&ln) || ln.contains(&d))
                    })
                    .map(|l| l.2)
                    .max()
            });
            let (mut kind, address) = details(f);
            if kind == Kind::Other {
                kind = Kind::from_name(&f.name);
            }
            BtDevice { name: f.name.clone(), battery, kind, le: f.le, address }
        })
        .collect();
    // Audio first, then input devices, then the rest; stable by name inside.
    let rank = |k: Kind| match k {
        k if k.audio() => 0,
        Kind::Mouse | Kind::Keyboard | Kind::Pen | Kind::Gamepad => 1,
        _ => 2,
    };
    out.sort_by(|a, b| rank(a.kind).cmp(&rank(b.kind)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(out)
}

/// Is an audio endpoint name ("Headphones (Bose Flex 2 SoundLink)") one of these devices?
pub fn match_endpoint<'a>(endpoint: &str, devices: &'a [BtDevice]) -> Option<&'a BtDevice> {
    let e = endpoint.to_lowercase();
    devices.iter().find(|d| {
        let n = d.name.to_lowercase();
        !n.is_empty() && (e.contains(&n) || n.contains(inner(&e)))
    })
}

/// "Headphones (Bose Flex 2 SoundLink)" → "Bose Flex 2 SoundLink"
pub fn inner(endpoint: &str) -> &str {
    match (endpoint.find('('), endpoint.rfind(')')) {
        (Some(a), Some(b)) if b > a + 1 => &endpoint[a + 1..b],
        _ => endpoint,
    }
}

// ---------------------------------------------------------------------------
// Watch paired devices so connects / disconnects are noticed at once
// ---------------------------------------------------------------------------

static WATCHERS: Mutex<Vec<DeviceWatcher>> = Mutex::new(Vec::new());

/// Watches every paired Bluetooth device (classic and LE) for changes of
/// its connection state; posts `msg` to `hwnd` on any change.
pub fn start_watcher(hwnd: HWND, msg: u32) {
    let h = hwnd.0 as isize;
    let _ = std::thread::Builder::new().stack_size(256 * 1024).spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let post = move || unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), msg, WPARAM(0), LPARAM(0));
        };
        let props: IIterable<HSTRING> = IIterable::from(vec![HSTRING::from("System.Devices.Aep.IsConnected")]);
        let mut list = Vec::new();
        for sel in [BluetoothDevice::GetDeviceSelectorFromPairingState(true), BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)] {
            let Ok(sel) = sel else { continue };
            let Ok(w) = DeviceInformation::CreateWatcherWithKindAqsFilterAndAdditionalProperties(&sel, &props, DeviceInformationKind::AssociationEndpoint) else { continue };
            let p1 = post;
            let p2 = post;
            let p3 = post;
            let mut started_up = false;
            let _ = w.Added(&windows::Foundation::TypedEventHandler::new(move |_, _| {
                p1();
                Ok(())
            }));
            let _ = w.Removed(&windows::Foundation::TypedEventHandler::new(move |_, _| {
                p2();
                Ok(())
            }));
            let _ = w.Updated(&windows::Foundation::TypedEventHandler::new(move |_, _| {
                p3();
                Ok(())
            }));
            if w.Start().is_ok() {
                started_up = true;
            }
            if started_up {
                list.push(w);
            }
        }
        if let Ok(mut v) = WATCHERS.lock() {
            *v = list;
        }
    });
}
