//! Lists for the Controls page: Wi-Fi networks in range and Bluetooth
//! devices (paired + nearby), and connecting / disconnecting them.
//! Everything slow runs on one worker thread; the island is poked with a
//! message whenever the lists or the status line change.

#![allow(non_snake_case)]

use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Devices::Bluetooth::*;
use windows::Devices::Enumeration::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows_collections::IIterable;

use crate::bt::Kind;

#[derive(Clone, Debug, Default)]
pub struct WifiNet {
    pub ssid: String,
    pub signal: u8,
    pub secure: bool,
    pub connected: bool,
    /// Saved profile name ("" = never joined).
    pub profile: String,
    /// 0 open, 1 WPA/WPA2 personal, 2 WPA3 personal, 3 WPA2 enterprise
    /// (user name + password), 4 WPA3 enterprise, 5 other (Windows' own dialog)
    pub auth: u8,
}

#[derive(Clone, Debug)]
pub struct BtItem {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub paired: bool,
    pub connected: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Lists {
    pub wifi: Vec<WifiNet>,
    pub wifi_scanning: bool,
    /// Windows hides nearby networks until location is allowed for desktop apps.
    pub wifi_denied: bool,
    pub bt: Vec<BtItem>,
    pub bt_scanning: bool,
    /// Name of what is being connected right now.
    pub busy: Option<String>,
    /// One-shot message for the island ("Couldn't connect to …"), success flag.
    pub note: Option<(String, bool)>,
    /// The row that just failed to connect (shown in red for a few seconds).
    pub failed: Option<(String, Instant)>,
}

pub enum Job {
    ScanWifi,
    /// `user`: work / school networks (802.1X) sign in with a user name too.
    WifiConnect { ssid: String, password: Option<String>, user: Option<String> },
    WifiDisconnect,
    ScanBt,
    /// Paired device: connect (true) / disconnect (false) — audio devices.
    BtConnect(String, bool),
    BtPair(String),
}

static LISTS: Mutex<Option<Lists>> = Mutex::new(None);
static DENIED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static TX: Mutex<Option<Sender<Job>>> = Mutex::new(None);
static HOST: Mutex<(isize, u32)> = Mutex::new((0, 0));

pub fn lists() -> Lists {
    LISTS.lock().ok().and_then(|l| l.clone()).unwrap_or_default()
}

pub fn take_note() -> Option<(String, bool)> {
    LISTS.lock().ok().and_then(|mut l| l.as_mut().and_then(|l| l.note.take()))
}

fn edit(f: impl FnOnce(&mut Lists)) {
    if let Ok(mut l) = LISTS.lock() {
        f(l.get_or_insert_with(Lists::default));
    }
    poke();
}

fn poke() {
    let (h, m) = HOST.lock().map(|h| *h).unwrap_or((0, 0));
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), m, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn note(s: String, ok: bool) {
    edit(|l| l.note = Some((s, ok)));
}

/// A row's connect didn't work: say so on the row, and its button comes back.
pub fn failed(name: &str) {
    edit(|l| l.failed = Some((name.to_string(), Instant::now())));
}

/// A list is opening: start it afresh (connected first, then the scan).
pub fn clear(wifi: bool) {
    edit(|l| {
        if wifi {
            l.wifi.clear();
        } else {
            l.bt.clear();
        }
        l.failed = None;
    });
}

pub fn send(j: Job) {
    // "Connecting…" shows at once, not when the worker gets to it.
    let busy = match &j {
        Job::WifiConnect { ssid, .. } => Some(ssid.clone()),
        Job::BtConnect(id, _) | Job::BtPair(id) => lists().bt.into_iter().find(|x| &x.id == id).map(|x| x.name),
        _ => None,
    };
    if let Some(b) = busy {
        edit(|l| {
            l.busy = Some(b);
            l.failed = None;
        });
    }
    if let Ok(t) = TX.lock() {
        if let Some(t) = t.as_ref() {
            let _ = t.send(j);
        }
    }
}

pub fn start(hwnd: HWND, msg: u32) {
    if let Ok(mut h) = HOST.lock() {
        *h = (hwnd.0 as isize, msg);
    }
    let (tx, rx) = channel::<Job>();
    if let Ok(mut t) = TX.lock() {
        *t = Some(tx);
    }
    let _ = std::thread::Builder::new().stack_size(1024 * 1024).spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        while let Ok(j) = rx.recv() {
            match j {
                Job::ScanWifi => scan_wifi(),
                Job::WifiConnect { ssid, password, user } => wifi_connect(&ssid, password, user),
                Job::WifiDisconnect => wifi_disconnect(),
                Job::ScanBt => scan_bt(),
                Job::BtConnect(id, on) => bt_connect(&id, on),
                Job::BtPair(id) => bt_pair(&id),
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Wi-Fi
// ---------------------------------------------------------------------------

fn fake_wifi() -> Option<Vec<WifiNet>> {
    let v = std::env::var("ISLET_FAKE_WIFI").ok()?;
    // "Name:signal:auth:connected:profile;…"
    Some(
        v.split(';')
            .filter_map(|e| {
                let p: Vec<&str> = e.split(':').collect();
                (p.len() >= 3).then(|| {
                    let auth: u8 = p[2].parse().unwrap_or(1);
                    WifiNet {
                        ssid: p[0].into(),
                        signal: p[1].parse().unwrap_or(60),
                        secure: auth != 0,
                        auth,
                        connected: p.get(3) == Some(&"1"),
                        profile: if p.get(4) == Some(&"1") { p[0].into() } else { String::new() },
                    }
                })
            })
            .collect(),
    )
}

struct Wlan {
    h: HANDLE,
    guid: GUID,
}

impl Wlan {
    fn open() -> Option<Wlan> {
        use windows::Win32::NetworkManagement::WiFi::*;
        unsafe {
            let mut ver = 0u32;
            let mut h = HANDLE::default();
            if WlanOpenHandle(2, None, &mut ver, &mut h) != 0 {
                return None;
            }
            let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
            let mut guid = None;
            if WlanEnumInterfaces(h, None, &mut list) == 0 && !list.is_null() {
                let n = (*list).dwNumberOfItems as usize;
                let items = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), n);
                // Prefer a connected adapter.
                guid = items.iter().find(|i| i.isState == wlan_interface_state_connected).or(items.first()).map(|i| i.InterfaceGuid);
                WlanFreeMemory(list as *const _);
            }
            match guid {
                Some(guid) => Some(Wlan { h, guid }),
                None => {
                    WlanCloseHandle(h, None);
                    None
                }
            }
        }
    }

    /// Networks in range; without the location permission, the saved ones.
    fn networks(&self) -> Vec<WifiNet> {
        match self.networks_in_range() {
            Ok(v) => {
                DENIED.store(false, std::sync::atomic::Ordering::Relaxed);
                v
            }
            Err(e) => {
                if e == 5 {
                    DENIED.store(true, std::sync::atomic::Ordering::Relaxed);
                } else {
                    crate::util::log(&format!("wifi: WlanGetAvailableNetworkList failed with {e}"));
                }
                self.saved()
            }
        }
    }

    /// Saved networks (always readable), the connected one marked.
    fn saved(&self) -> Vec<WifiNet> {
        use windows::Win32::NetworkManagement::WiFi::*;
        let mut out = Vec::new();
        let connected = crate::netinfo::nlm_name(&self.guid);
        unsafe {
            let mut list: *mut WLAN_PROFILE_INFO_LIST = std::ptr::null_mut();
            if WlanGetProfileList(self.h, &self.guid, None, &mut list) == 0 && !list.is_null() {
                let n = (*list).dwNumberOfItems as usize;
                let items = std::slice::from_raw_parts((*list).ProfileInfo.as_ptr(), n);
                for it in items {
                    let name = crate::util::from_wide(&it.strProfileName);
                    if name.is_empty() {
                        continue;
                    }
                    let is_conn = connected.as_deref() == Some(name.as_str());
                    out.push(WifiNet { ssid: name.clone(), signal: if is_conn { 70 } else { 0 }, secure: true, connected: is_conn, profile: name, auth: 1 });
                }
                WlanFreeMemory(list as *const _);
            }
        }
        out.sort_by(|a, b| b.connected.cmp(&a.connected));
        out
    }

    fn networks_in_range(&self) -> std::result::Result<Vec<WifiNet>, u32> {
        use windows::Win32::NetworkManagement::WiFi::*;
        let mut out: Vec<WifiNet> = Vec::new();
        unsafe {
            let mut list: *mut WLAN_AVAILABLE_NETWORK_LIST = std::ptr::null_mut();
            let r = WlanGetAvailableNetworkList(self.h, &self.guid, 0, None, &mut list);
            if r != 0 || list.is_null() {
                return Err(r);
            }
            let n = (*list).dwNumberOfItems as usize;
            let items = std::slice::from_raw_parts((*list).Network.as_ptr(), n);
            for it in items {
                let len = (it.dot11Ssid.uSSIDLength as usize).min(32);
                let ssid = String::from_utf8_lossy(&it.dot11Ssid.ucSSID[..len]).into_owned();
                if ssid.trim().is_empty() || ssid.chars().all(|c| c == '\0') {
                    continue;
                }
                let profile = crate::util::from_wide(&it.strProfileName);
                let connected = it.dwFlags & WLAN_AVAILABLE_NETWORK_CONNECTED != 0;
                let a = it.dot11DefaultAuthAlgorithm;
                let auth = if !it.bSecurityEnabled.as_bool() || a == DOT11_AUTH_ALGO_80211_OPEN || a == DOT11_AUTH_ALGO_OWE {
                    0
                } else if a == DOT11_AUTH_ALGO_RSNA_PSK || a == DOT11_AUTH_ALGO_WPA_PSK {
                    1
                } else if a == DOT11_AUTH_ALGO_WPA3_SAE {
                    2
                } else if a == DOT11_AUTH_ALGO_RSNA || a == DOT11_AUTH_ALGO_WPA {
                    3
                } else if a == DOT11_AUTH_ALGO_WPA3_ENT || a == DOT11_AUTH_ALGO_WPA3_ENT_192 {
                    4
                } else {
                    5
                };
                let net = WifiNet { ssid: ssid.clone(), signal: it.wlanSignalQuality.min(100) as u8, secure: auth != 0, connected, profile, auth };
                match out.iter_mut().find(|o| o.ssid == ssid) {
                    Some(o) => {
                        o.signal = o.signal.max(net.signal);
                        o.connected |= net.connected;
                        if o.profile.is_empty() {
                            o.profile = net.profile;
                        }
                    }
                    None => out.push(net),
                }
            }
            WlanFreeMemory(list as *const _);
        }
        out.sort_by(|a, b| b.connected.cmp(&a.connected).then((!b.profile.is_empty()).cmp(&!a.profile.is_empty())).then(b.signal.cmp(&a.signal)));
        Ok(out)
    }

    fn connected_ssid(&self) -> Option<String> {
        self.networks().into_iter().find(|n| n.connected).map(|n| n.ssid)
    }
}

impl Drop for Wlan {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::NetworkManagement::WiFi::WlanCloseHandle(self.h, None);
        }
    }
}

fn scan_wifi() {
    if let Some(v) = fake_wifi() {
        edit(|l| {
            l.wifi = v.iter().filter(|n| n.connected).cloned().collect();
            l.wifi_scanning = true;
        });
        std::thread::sleep(Duration::from_millis(1500));
        edit(|l| {
            l.wifi = v;
            l.wifi_scanning = false;
        });
        return;
    }
    edit(|l| l.wifi_scanning = true);
    let Some(w) = Wlan::open() else {
        edit(|l| {
            l.wifi.clear();
            l.wifi_scanning = false;
        });
        return;
    };
    // What Windows already knows, then a fresh scan (takes a few seconds).
    let mut first = w.networks();
    if DENIED.load(std::sync::atomic::Ordering::Relaxed) && !ASKED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        // Windows 11 24H2+: nearby networks need the location permission.
        // Asking shows Windows' own one-time prompt.
        let _ = windows::Devices::Geolocation::Geolocator::RequestAccessAsync().and_then(|op| op.join());
        first = w.networks();
    }
    let denied = DENIED.load(std::sync::atomic::Ordering::Relaxed);
    // Without the permission only the network you're on is known — show
    // that (to disconnect), not the whole saved list.
    // A list just opened: first just the network you're on; everything in
    // range once the scan is in. (A rescan keeps showing what's there.)
    let fresh = lists().wifi.is_empty();
    let first: Vec<WifiNet> = first.into_iter().filter(|n| n.connected || (!fresh && !denied)).collect();
    edit(|l| {
        l.wifi = first;
        l.wifi_denied = denied;
    });
    if denied {
        edit(|l| l.wifi_scanning = false);
        return;
    }
    unsafe {
        let _ = windows::Win32::NetworkManagement::WiFi::WlanScan(w.h, &w.guid, None, None, None);
    }
    for _ in 0..2 {
        std::thread::sleep(Duration::from_millis(2000));
        let v = w.networks();
        edit(|l| l.wifi = v);
    }
    edit(|l| l.wifi_scanning = false);
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

fn profile_xml(ssid: &str, auth: u8, password: Option<&str>) -> String {
    let hex: String = ssid.as_bytes().iter().map(|b| format!("{b:02X}")).collect();
    let name = xml_escape(ssid);
    let security = match (auth, password) {
        (0, _) | (_, None) => "<authEncryption><authentication>open</authentication><encryption>none</encryption><useOneX>false</useOneX></authEncryption>".to_string(),
        (a, Some(pw)) => format!(
            "<authEncryption><authentication>{}</authentication><encryption>AES</encryption><useOneX>false</useOneX></authEncryption><sharedKey><keyType>passPhrase</keyType><protected>false</protected><keyMaterial>{}</keyMaterial></sharedKey>",
            if a == 2 { "WPA3SAE" } else { "WPA2PSK" },
            xml_escape(pw)
        ),
    };
    format!(
        "<?xml version=\"1.0\"?><WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\"><name>{name}</name><SSIDConfig><SSID><hex>{hex}</hex><name>{name}</name></SSID></SSIDConfig><connectionType>ESS</connectionType><connectionMode>auto</connectionMode><MSM><security>{security}</security></MSM></WLANProfile>"
    )
}

/// Work / school Wi-Fi (802.1X): PEAP with a user name and password
/// (MSCHAPv2) — what almost every such network uses. Windows still checks
/// the network's certificate and asks once if it doesn't know it.
fn profile_xml_ent(ssid: &str, auth: u8) -> String {
    let hex: String = ssid.as_bytes().iter().map(|b| format!("{b:02X}")).collect();
    let name = xml_escape(ssid);
    let a = if auth == 4 { "WPA3ENT" } else { "WPA2" };
    format!(
        "<?xml version=\"1.0\"?><WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\"><name>{name}</name><SSIDConfig><SSID><hex>{hex}</hex><name>{name}</name></SSID></SSIDConfig><connectionType>ESS</connectionType><connectionMode>auto</connectionMode><MSM><security><authEncryption><authentication>{a}</authentication><encryption>AES</encryption><useOneX>true</useOneX></authEncryption><OneX xmlns=\"http://www.microsoft.com/networking/OneX/v1\"><authMode>user</authMode><EAPConfig><EapHostConfig xmlns=\"http://www.microsoft.com/provisioning/EapHostConfig\"><EapMethod><Type xmlns=\"http://www.microsoft.com/provisioning/EapCommon\">25</Type><VendorId xmlns=\"http://www.microsoft.com/provisioning/EapCommon\">0</VendorId><VendorType xmlns=\"http://www.microsoft.com/provisioning/EapCommon\">0</VendorType><AuthorId xmlns=\"http://www.microsoft.com/provisioning/EapCommon\">0</AuthorId></EapMethod><Config xmlns=\"http://www.microsoft.com/provisioning/EapHostConfig\"><Eap xmlns=\"http://www.microsoft.com/provisioning/BaseEapConnectionPropertiesV1\"><Type>25</Type><EapType xmlns=\"http://www.microsoft.com/provisioning/MsPeapConnectionPropertiesV1\"><ServerValidation><DisableUserPromptForServerValidation>false</DisableUserPromptForServerValidation><ServerNames></ServerNames></ServerValidation><FastReconnect>true</FastReconnect><InnerEapOptional>false</InnerEapOptional><Eap xmlns=\"http://www.microsoft.com/provisioning/BaseEapConnectionPropertiesV1\"><Type>26</Type><EapType xmlns=\"http://www.microsoft.com/provisioning/MsChapV2ConnectionPropertiesV1\"><UseWinLogonCredentials>false</UseWinLogonCredentials></EapType></Eap><EnableQuarantineChecks>false</EnableQuarantineChecks><RequireCryptoBinding>false</RequireCryptoBinding></EapType></Eap></Config></EapHostConfig></EAPConfig></OneX></security></MSM></WLANProfile>"
    )
}

/// The user name and password for that profile ("DOMAIN\\user" works too).
fn eap_user_xml(user: &str, password: &str) -> String {
    let (domain, u) = match user.split_once('\\') {
        Some((d, u)) => (d, u),
        None => ("", user),
    };
    format!(
        "<?xml version=\"1.0\"?><EapHostUserCredentials xmlns=\"http://www.microsoft.com/provisioning/EapHostUserCredentials\" xmlns:eapCommon=\"http://www.microsoft.com/provisioning/EapCommon\" xmlns:baseEap=\"http://www.microsoft.com/provisioning/BaseEapMethodUserCredentials\"><EapMethod><eapCommon:Type>25</eapCommon:Type><eapCommon:AuthorId>0</eapCommon:AuthorId></EapMethod><Credentials xmlns:eapUser=\"http://www.microsoft.com/provisioning/EapUserPropertiesV1\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xmlns:baseEap=\"http://www.microsoft.com/provisioning/BaseEapUserPropertiesV1\" xmlns:MsPeap=\"http://www.microsoft.com/provisioning/MsPeapUserPropertiesV1\" xmlns:MsChapV2=\"http://www.microsoft.com/provisioning/MsChapV2UserPropertiesV1\"><baseEap:Eap><baseEap:Type>25</baseEap:Type><MsPeap:EapType><baseEap:Eap><baseEap:Type>26</baseEap:Type><MsChapV2:EapType><MsChapV2:Username>{}</MsChapV2:Username><MsChapV2:Password>{}</MsChapV2:Password><MsChapV2:LogonDomain>{}</MsChapV2:LogonDomain></MsChapV2:EapType></baseEap:Eap></MsPeap:EapType></baseEap:Eap></Credentials></EapHostUserCredentials>",
        xml_escape(u),
        xml_escape(password),
        xml_escape(domain)
    )
}

fn wifi_connect(ssid: &str, password: Option<String>, user: Option<String>) {
    use windows::Win32::NetworkManagement::WiFi::*;
    let tr = crate::i18n::tr;
    if fake_wifi().is_some() {
        // (Developer aid: a password "wrong" fails, anything else joins.)
        std::thread::sleep(Duration::from_millis(1200));
        edit(|l| l.busy = None);
        if password.as_deref() == Some("wrong") {
            failed(ssid);
            note(format!("{} {ssid} · {}", tr("Couldn't connect to"), tr("check the password")), false);
        } else {
            edit(|l| {
                for n in l.wifi.iter_mut() {
                    n.connected = n.ssid == ssid;
                }
            });
            note(format!("{} {ssid}", tr("Connected to")), true);
        }
        return;
    }
    let Some(w) = Wlan::open() else {
        edit(|l| l.busy = None);
        return;
    };
    let Some(net) = w.networks().into_iter().find(|n| n.ssid == ssid) else {
        edit(|l| l.busy = None);
        failed(ssid);
        note(format!("{} {ssid}", tr("Couldn't find")), false);
        return;
    };
    edit(|l| l.busy = Some(ssid.to_string()));
    let mut made = false;
    let enterprise = matches!(net.auth, 3 | 4) && user.is_some();
    let profile = if !net.profile.is_empty() && password.is_none() {
        net.profile.clone()
    } else {
        let xml = HSTRING::from(if enterprise { profile_xml_ent(ssid, net.auth) } else { profile_xml(ssid, net.auth, password.as_deref()) });
        let mut reason = 0u32;
        let r = unsafe { WlanSetProfile(w.h, &w.guid, 0, &xml, PCWSTR::null(), true, None, &mut reason) };
        if r != 0 {
            crate::util::log(&format!("wifi: WlanSetProfile failed {r} reason {reason}"));
            edit(|l| l.busy = None);
            failed(ssid);
            note(format!("{} {ssid}", tr("Couldn't connect to")), false);
            return;
        }
        if enterprise {
            let ux = HSTRING::from(eap_user_xml(user.as_deref().unwrap_or(""), password.as_deref().unwrap_or("")));
            let pn = HSTRING::from(ssid);
            let r = unsafe { WlanSetProfileEapXmlUserData(w.h, &w.guid, &pn, WLAN_SET_EAPHOST_FLAGS(0), &ux, None) };
            if r != 0 {
                crate::util::log(&format!("wifi: WlanSetProfileEapXmlUserData failed {r}"));
            }
        }
        made = true;
        ssid.to_string()
    };
    let name = HSTRING::from(profile.as_str());
    let params = WLAN_CONNECTION_PARAMETERS {
        wlanConnectionMode: wlan_connection_mode_profile,
        strProfile: PCWSTR(name.as_ptr()),
        pDot11Ssid: std::ptr::null_mut(),
        pDesiredBssidList: std::ptr::null_mut(),
        dot11BssType: dot11_BSS_type_infrastructure,
        dwFlags: 0,
    };
    let r = unsafe { WlanConnect(w.h, &w.guid, &params, None) };
    let mut ok = false;
    if r == 0 {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(15) {
            std::thread::sleep(Duration::from_millis(700));
            if w.connected_ssid().as_deref() == Some(ssid) {
                ok = true;
                break;
            }
        }
    }
    if !ok && made {
        // A wrong password shouldn't stay saved.
        unsafe {
            let _ = WlanDeleteProfile(w.h, &w.guid, &name, None);
        }
    }
    let v = w.networks();
    edit(|l| {
        l.busy = None;
        l.wifi = v;
    });
    if ok {
        note(format!("{} {ssid}", tr("Connected to")), true);
    } else if enterprise {
        failed(ssid);
        note(format!("{} {ssid} · {}", tr("Couldn't connect to"), tr("check the user name and password")), false);
    } else if net.secure && password.is_some() {
        failed(ssid);
        note(format!("{} {ssid} · {}", tr("Couldn't connect to"), tr("check the password")), false);
    } else {
        failed(ssid);
        note(format!("{} {ssid}", tr("Couldn't connect to")), false);
    }
}

fn wifi_disconnect() {
    if fake_wifi().is_some() {
        return;
    }
    let Some(w) = Wlan::open() else { return };
    unsafe {
        let _ = windows::Win32::NetworkManagement::WiFi::WlanDisconnect(w.h, &w.guid, None);
    }
    std::thread::sleep(Duration::from_millis(600));
    let v = w.networks();
    edit(|l| l.wifi = v);
}

// ---------------------------------------------------------------------------
// Bluetooth
// ---------------------------------------------------------------------------

fn fake_bt() -> Option<Vec<BtItem>> {
    let v = std::env::var("ISLET_FAKE_BTLIST").ok()?;
    // "Name:paired:connected;…"
    Some(
        v.split(';')
            .filter_map(|e| {
                let p: Vec<&str> = e.split(':').collect();
                (p.len() >= 3).then(|| BtItem { id: p[0].into(), name: p[0].into(), kind: Kind::from_name(p[0]), paired: p[1] == "1", connected: p[2] == "1" })
            })
            .collect(),
    )
}

const P_CONNECTED: &str = "System.Devices.Aep.IsConnected";
const P_PAIRED: &str = "System.Devices.Aep.IsPaired";

fn bool_prop(d: &DeviceInformation, key: &str) -> bool {
    d.Properties()
        .ok()
        .and_then(|m| m.Lookup(&HSTRING::from(key)).ok())
        .and_then(|v| v.cast::<windows::Foundation::IReference<bool>>().ok())
        .and_then(|r| r.Value().ok())
        .unwrap_or(false)
}

fn kind_for(id: &HSTRING, le: bool, name: &str) -> Kind {
    let k = if le {
        BluetoothLEDevice::FromIdAsync(id).and_then(|op| op.join()).ok().map(|d| {
            let k = d.Appearance().and_then(|a| a.RawValue()).map(Kind::from_appearance).unwrap_or(Kind::Other);
            let _ = d.Close();
            k
        })
    } else {
        BluetoothDevice::FromIdAsync(id).and_then(|op| op.join()).ok().map(|d| {
            let k = d.ClassOfDevice().and_then(|c| c.RawValue()).map(Kind::from_cod).unwrap_or(Kind::Other);
            let _ = d.Close();
            k
        })
    };
    match k {
        Some(k) if k != Kind::Other => k,
        _ => Kind::from_name(name),
    }
}

fn sort_bt(v: &mut [BtItem]) {
    v.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.paired.cmp(&a.paired)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
}

fn scan_bt() {
    if let Some(mut v) = fake_bt() {
        sort_bt(&mut v);
        edit(|l| {
            l.bt = v.iter().filter(|x| x.connected).cloned().collect();
            l.bt_scanning = true;
        });
        std::thread::sleep(Duration::from_millis(1500));
        edit(|l| {
            l.bt = v;
            l.bt_scanning = false;
        });
        return;
    }
    edit(|l| l.bt_scanning = true);
    let props: IIterable<HSTRING> = IIterable::from(vec![HSTRING::from(P_CONNECTED), HSTRING::from(P_PAIRED)]);
    // Paired devices first (instant).
    let mut items: Vec<BtItem> = Vec::new();
    for (sel, le) in [(BluetoothDevice::GetDeviceSelectorFromPairingState(true), false), (BluetoothLEDevice::GetDeviceSelectorFromPairingState(true), true)] {
        let Ok(sel) = sel else { continue };
        let Ok(col) = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(&sel, &props, DeviceInformationKind::AssociationEndpoint).and_then(|op| op.join()) else { continue };
        for i in 0..col.Size().unwrap_or(0) {
            let Ok(d) = col.GetAt(i) else { continue };
            let name = d.Name().map(|n| n.to_string()).unwrap_or_default();
            if name.trim().is_empty() {
                continue;
            }
            let id = d.Id().unwrap_or_default();
            let connected = bool_prop(&d, P_CONNECTED);
            match items.iter_mut().find(|x| x.name.eq_ignore_ascii_case(&name)) {
                // Dual-mode devices show up twice; keep one, connected if either is.
                Some(x) => x.connected |= connected,
                None => items.push(BtItem { kind: kind_for(&id, le, &name), id: id.to_string(), name, paired: true, connected }),
            }
        }
    }
    sort_bt(&mut items);
    // First what's connected, then every paired device, then nearby ones.
    if lists().bt.is_empty() {
        let connected: Vec<BtItem> = items.iter().filter(|x| x.connected).cloned().collect();
        edit(|l| l.bt = connected);
        std::thread::sleep(Duration::from_millis(350));
    }
    edit(|l| l.bt = items.clone());

    // Then look for nearby devices that are ready to pair, for ~20 s. Many
    // arrive without a name and get it a moment later (an "Updated" event),
    // so keep every one by id and list those with a name.
    let found: Arc<Mutex<Vec<BtItem>>> = Arc::new(Mutex::new(Vec::new()));
    let pending: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new())); // (id, name)
    let near_props: IIterable<HSTRING> = IIterable::from(vec![HSTRING::from(P_CONNECTED), HSTRING::from(P_PAIRED), HSTRING::from("System.ItemNameDisplay"), HSTRING::from("System.Devices.Aep.IsPresent")]);
    let mut watchers = Vec::new();
    let publish_named = |f: &Arc<Mutex<Vec<BtItem>>>, p: &Arc<Mutex<Vec<(String, String)>>>| {
        let named: Vec<(String, String)> = p.lock().map(|v| v.iter().filter(|(_, n)| !n.trim().is_empty() && !n.starts_with("Bluetooth ")).cloned().collect()).unwrap_or_default();
        if let Ok(mut v) = f.lock() {
            for (id, name) in named {
                if !v.iter().any(|x| x.id == id || x.name.eq_ignore_ascii_case(&name)) {
                    v.push(BtItem { id, kind: Kind::from_name(&name), name, paired: false, connected: false });
                }
            }
        }
        poke_merge();
    };
    for sel in [BluetoothDevice::GetDeviceSelectorFromPairingState(false), BluetoothLEDevice::GetDeviceSelectorFromPairingState(false)] {
        let Ok(sel) = sel else { continue };
        let Ok(w) = DeviceInformation::CreateWatcherWithKindAqsFilterAndAdditionalProperties(&sel, &near_props, DeviceInformationKind::AssociationEndpoint) else { continue };
        let (f, p) = (found.clone(), pending.clone());
        let _ = w.Added(&windows::Foundation::TypedEventHandler::new(move |_, d: windows::core::Ref<DeviceInformation>| {
            if let Some(d) = d.as_ref() {
                let name = d.Name().map(|n| n.to_string()).unwrap_or_default();
                let id = d.Id().unwrap_or_default().to_string();
                if let Ok(mut v) = p.lock() {
                    if !v.iter().any(|(i, _)| *i == id) {
                        v.push((id, name));
                    }
                }
                publish_named(&f, &p);
            }
            Ok(())
        }));
        let (f, p) = (found.clone(), pending.clone());
        let _ = w.Updated(&windows::Foundation::TypedEventHandler::new(move |_, u: windows::core::Ref<DeviceInformationUpdate>| {
            if let Some(u) = u.as_ref() {
                let id = u.Id().unwrap_or_default().to_string();
                let name = u
                    .Properties()
                    .ok()
                    .and_then(|m| m.Lookup(&HSTRING::from("System.ItemNameDisplay")).ok())
                    .and_then(|v| v.cast::<windows::Foundation::IReference<HSTRING>>().ok().and_then(|r| r.Value().ok()).or_else(|| v.cast::<windows::Foundation::IPropertyValue>().ok().and_then(|pv| pv.GetString().ok())))
                    .map(|h| h.to_string())
                    .unwrap_or_default();
                if !name.trim().is_empty() {
                    if let Ok(mut v) = p.lock() {
                        match v.iter_mut().find(|(i, _)| *i == id) {
                            Some(e) => e.1 = name,
                            None => v.push((id, name)),
                        }
                    }
                    publish_named(&f, &p);
                }
            }
            Ok(())
        }));
        let p = pending.clone();
        let f = found.clone();
        let _ = w.Removed(&windows::Foundation::TypedEventHandler::new(move |_, u: windows::core::Ref<DeviceInformationUpdate>| {
            if let Some(u) = u.as_ref() {
                let id = u.Id().unwrap_or_default().to_string();
                if let Ok(mut v) = p.lock() {
                    v.retain(|(i, _)| *i != id);
                }
                if let Ok(mut v) = f.lock() {
                    v.retain(|x| x.id != id);
                }
                poke_merge();
            }
            Ok(())
        }));
        if w.Start().is_ok() {
            watchers.push(w);
        }
    }
    if let Ok(mut n) = NEARBY.lock() {
        *n = Some(found.clone());
    }
    poke_merge();
    std::thread::sleep(Duration::from_secs(20));
    for w in &watchers {
        let _ = w.Stop();
    }
    if let Ok(mut n) = NEARBY.lock() {
        *n = None;
    }
    // Final merge; refine kinds of the nearby ones.
    let mut near = found.lock().map(|v| v.clone()).unwrap_or_default();
    for n in near.iter_mut() {
        n.kind = kind_for(&HSTRING::from(n.id.as_str()), n.id.contains("BluetoothLE"), &n.name);
    }
    edit(|l| {
        merge_nearby(l, &near);
        l.bt_scanning = false;
    });
}

static NEARBY: Mutex<Option<Arc<Mutex<Vec<BtItem>>>>> = Mutex::new(None);

fn merge_nearby(l: &mut Lists, near: &[BtItem]) {
    l.bt.retain(|x| x.paired);
    for n in near {
        if !l.bt.iter().any(|x| x.name.eq_ignore_ascii_case(&n.name)) {
            l.bt.push(n.clone());
        }
    }
    sort_bt(&mut l.bt);
}

fn poke_merge() {
    let near = NEARBY.lock().ok().and_then(|n| n.clone()).and_then(|a| a.lock().ok().map(|v| v.clone())).unwrap_or_default();
    edit(|l| merge_nearby(l, &near));
}

fn refresh_paired_state() {
    let props: IIterable<HSTRING> = IIterable::from(vec![HSTRING::from(P_CONNECTED)]);
    let mut states: Vec<(String, bool)> = Vec::new();
    for sel in [BluetoothDevice::GetDeviceSelectorFromPairingState(true), BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)] {
        let Ok(sel) = sel else { continue };
        let Ok(col) = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(&sel, &props, DeviceInformationKind::AssociationEndpoint).and_then(|op| op.join()) else { continue };
        for i in 0..col.Size().unwrap_or(0) {
            if let Ok(d) = col.GetAt(i) {
                states.push((d.Name().map(|n| n.to_string()).unwrap_or_default(), bool_prop(&d, P_CONNECTED)));
            }
        }
    }
    edit(|l| {
        for x in l.bt.iter_mut().filter(|x| x.paired) {
            x.connected = states.iter().any(|(n, c)| *c && n.eq_ignore_ascii_case(&x.name));
        }
        sort_bt(&mut l.bt);
    });
}

fn bt_connect(id: &str, on: bool) {
    let tr = crate::i18n::tr;
    let item = lists().bt.into_iter().find(|x| x.id == id);
    let Some(item) = item else { return };
    if fake_bt().is_some() {
        // (Developer aid: a name with "fail" in it never connects.)
        std::thread::sleep(Duration::from_millis(1200));
        let fails = item.name.to_lowercase().contains("fail");
        edit(|l| {
            l.busy = None;
            if let Some(x) = l.bt.iter_mut().find(|x| x.id == id).filter(|_| !fails) {
                x.connected = on;
            }
            sort_bt(&mut l.bt);
        });
        if fails {
            failed(&item.name);
            note(format!("{} {}", tr("Couldn't connect to"), item.name), false);
        }
        return;
    }
    edit(|l| l.busy = Some(item.name.clone()));
    let done = crate::audio::bt_audio_connect(&item.name, on);
    // Give the stack a moment, then read the real state back.
    let t0 = Instant::now();
    if done {
        while t0.elapsed() < Duration::from_secs(8) {
            std::thread::sleep(Duration::from_millis(800));
            refresh_paired_state();
            if lists().bt.iter().any(|x| x.id == id && x.connected == on) {
                break;
            }
        }
    }
    edit(|l| l.busy = None);
    if !done {
        // Windows has no public way to connect some devices (mice, keyboards…)
        // on demand: say so here, without opening Settings.
        failed(&item.name);
        note(format!("{} {}", tr("Couldn't connect to"), item.name), false);
    } else if !lists().bt.iter().any(|x| x.id == id && x.connected == on) {
        failed(&item.name);
        note(format!("{} {}", tr(if on { "Couldn't connect to" } else { "Couldn't disconnect" }), item.name), false);
    }
}

fn bt_pair(id: &str) {
    let tr = crate::i18n::tr;
    let name = lists().bt.into_iter().find(|x| x.id == id).map(|x| x.name).unwrap_or_default();
    if fake_bt().is_some() {
        std::thread::sleep(Duration::from_millis(1200));
        let fails = name.to_lowercase().contains("fail");
        edit(|l| {
            l.busy = None;
            if let Some(x) = l.bt.iter_mut().find(|x| x.id == id).filter(|_| !fails) {
                x.paired = true;
                x.connected = true;
            }
            sort_bt(&mut l.bt);
        });
        if fails {
            failed(&name);
            note(format!("{} {name}", tr("Couldn't pair with")), false);
        } else {
            note(format!("{} {name}", tr("Connected to")), true);
        }
        return;
    }
    edit(|l| l.busy = Some(name.clone()));
    let ok = (|| -> Result<bool> {
        let d = DeviceInformation::CreateFromIdAsync(&HSTRING::from(id))?.join()?;
        let custom = d.Pairing()?.Custom()?;
        let nm = name.clone();
        let token = custom.PairingRequested(&windows::Foundation::TypedEventHandler::new(move |_, a: windows::core::Ref<DevicePairingRequestedEventArgs>| {
            if let Some(a) = a.as_ref() {
                let kind = a.PairingKind()?;
                if kind == DevicePairingKinds::ProvidePin {
                    a.AcceptWithPin(&HSTRING::from("0000"))?;
                } else {
                    if kind == DevicePairingKinds::DisplayPin || kind == DevicePairingKinds::ConfirmPinMatch {
                        let pin = a.Pin().map(|p| p.to_string()).unwrap_or_default();
                        note(format!("{nm} · PIN {pin}"), true);
                    }
                    a.Accept()?;
                }
            }
            Ok(())
        }))?;
        let kinds = DevicePairingKinds::ConfirmOnly | DevicePairingKinds::DisplayPin | DevicePairingKinds::ConfirmPinMatch | DevicePairingKinds::ProvidePin;
        let r = custom.PairAsync(kinds)?.join()?;
        let _ = custom.RemovePairingRequested(token);
        let s = r.Status()?;
        Ok(s == DevicePairingResultStatus::Paired || s == DevicePairingResultStatus::AlreadyPaired)
    })()
    .unwrap_or_else(|e| {
        crate::util::log(&format!("bt pair: {e}"));
        false
    });
    edit(|l| l.busy = None);
    if ok {
        note(format!("{} {name}", tr("Paired with")), true);
        std::thread::sleep(Duration::from_millis(1500));
        // Refresh: it moves to the paired part of the list.
        let near: Vec<BtItem> = lists().bt.into_iter().filter(|x| !x.paired && x.id != id).collect();
        scan_paired_only(&near);
    } else {
        failed(&name);
        note(format!("{} {name}", tr("Couldn't pair with")), false);
    }
}

fn scan_paired_only(near: &[BtItem]) {
    let props: IIterable<HSTRING> = IIterable::from(vec![HSTRING::from(P_CONNECTED)]);
    let mut items: Vec<BtItem> = Vec::new();
    for (sel, le) in [(BluetoothDevice::GetDeviceSelectorFromPairingState(true), false), (BluetoothLEDevice::GetDeviceSelectorFromPairingState(true), true)] {
        let Ok(sel) = sel else { continue };
        let Ok(col) = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(&sel, &props, DeviceInformationKind::AssociationEndpoint).and_then(|op| op.join()) else { continue };
        for i in 0..col.Size().unwrap_or(0) {
            let Ok(d) = col.GetAt(i) else { continue };
            let name = d.Name().map(|n| n.to_string()).unwrap_or_default();
            if name.trim().is_empty() || items.iter().any(|x| x.name.eq_ignore_ascii_case(&name)) {
                continue;
            }
            let id = d.Id().unwrap_or_default();
            items.push(BtItem { kind: kind_for(&id, le, &name), id: id.to_string(), connected: bool_prop(&d, P_CONNECTED), name, paired: true });
        }
    }
    edit(|l| {
        l.bt = items;
        merge_nearby(l, near);
    });
}
