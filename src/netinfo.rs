//! Network and radio status for the Devices page: Wi-Fi (name, signal, band),
//! wired Ethernet (link speed), VPNs, the local IPv4 address and whether the
//! Bluetooth radio is on. Queried on the device worker thread.

use windows::Win32::Foundation::*;
use windows::Win32::NetworkManagement::IpHelper::*;
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::WinSock::{AF_INET, AF_UNSPEC, SOCKADDR_IN};

#[derive(Clone, Debug, Default)]
pub struct Wifi {
    pub ssid: String,
    pub signal: u8,
    /// "2.4 GHz" / "5 GHz" when known.
    pub band: String,
}

#[derive(Clone, Debug, Default)]
pub struct NetInfo {
    pub wifi: Option<Wifi>,
    /// Wired links that are up: (name, bits per second).
    pub ethernet: Vec<(String, u64)>,
    /// VPN / tunnel connections that are up.
    pub vpn: Vec<String>,
    pub ip: Option<String>,
    /// The MAC address of the adapter `ip` is on ("3C:52:82:1A:BE:07").
    pub mac: Option<String>,
    /// None when there is no Bluetooth radio (or Windows won't say).
    pub bt_on: Option<bool>,
}

pub fn query() -> NetInfo {
    if let Ok(v) = std::env::var("ISLET_FAKE_NET") {
        return fake(&v);
    }
    let mut n = NetInfo { wifi: wifi(), bt_on: bt_radio(), ..Default::default() };
    adapters(&mut n);
    n
}

/// Developer aid: ISLET_FAKE_NET="wifi=Name:82:5;eth=1000;vpn=Tailscale;ip=192.168.1.23;bt=1"
fn fake(v: &str) -> NetInfo {
    let mut n = NetInfo::default();
    for part in v.split(';') {
        let Some((k, val)) = part.split_once('=') else { continue };
        match k.trim() {
            "wifi" => {
                let p: Vec<&str> = val.split(':').collect();
                n.wifi = Some(Wifi {
                    ssid: p.first().unwrap_or(&"").to_string(),
                    signal: p.get(1).and_then(|s| s.parse().ok()).unwrap_or(80),
                    band: p.get(2).map(|b| format!("{b} GHz")).unwrap_or_default(),
                });
            }
            "eth" => n.ethernet.push(("Ethernet".into(), val.parse::<u64>().unwrap_or(1000) * 1_000_000)),
            "vpn" => n.vpn.push(val.to_string()),
            "ip" => n.ip = Some(val.to_string()),
            "mac" => n.mac = Some(val.to_string()),
            "bt" => n.bt_on = Some(val == "1"),
            _ => {}
        }
    }
    n
}

pub fn speed_label(bps: u64) -> String {
    if bps >= 1_000_000_000 {
        let g = bps as f64 / 1e9;
        if (g - g.round()).abs() < 0.05 { format!("{:.0} Gbps", g) } else { format!("{:.1} Gbps", g) }
    } else if bps >= 1_000_000 {
        format!("{} Mbps", bps / 1_000_000)
    } else {
        String::new()
    }
}

fn wide_str(p: windows::core::PWSTR) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { p.to_string().unwrap_or_default() }
    }
}

const VPN_WORDS: &[&str] = &[
    "vpn", "wireguard", "wintun", "tap-windows", "openvpn", "tailscale", "zerotier", "nordlynx", "fortinet", "forticlient",
    "anyconnect", "globalprotect", "pangp", "cloudflare warp", "proton", "expressvpn", "surfshark", "mullvad", "softether", "pulse secure",
];
const VIRTUAL_WORDS: &[&str] = &["virtual", "vmware", "virtualbox", "hyper-v", "vethernet", "bluetooth", "loopback", "pseudo", "wsl", "docker"];

fn adapters(n: &mut NetInfo) {
    unsafe {
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER | GAA_FLAG_INCLUDE_GATEWAYS;
        let mut size = 16 * 1024u32;
        let mut buf: Vec<u64> = Vec::new();
        for _ in 0..3 {
            buf = vec![0u64; size as usize / 8 + 1];
            let r = GetAdaptersAddresses(AF_UNSPEC.0 as u32, flags, None, Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH), &mut size);
            if r == ERROR_BUFFER_OVERFLOW.0 {
                continue;
            }
            if r != 0 {
                return;
            }
            break;
        }
        let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        let mut best_ip: Option<(u32, String, Option<String>)> = None; // (metric, ip, mac)
        while !p.is_null() {
            let a = &*p;
            p = a.Next;
            if a.OperStatus != IfOperStatusUp {
                continue;
            }
            let name = wide_str(a.FriendlyName);
            let desc = wide_str(a.Description);
            let both = format!("{name} {desc}").to_lowercase();
            const ETHERNET: u32 = 6;
            const PPP: u32 = 23;
            const LOOPBACK: u32 = 24;
            const WIFI: u32 = 71;
            const TUNNEL: u32 = 131;
            const PROP_VIRTUAL: u32 = 53;
            if a.IfType == LOOPBACK {
                continue;
            }
            let is_vpn = a.IfType == PPP || VPN_WORDS.iter().any(|w| both.contains(w));
            if is_vpn {
                if !n.vpn.contains(&name) && !name.is_empty() {
                    n.vpn.push(name.clone());
                }
                continue;
            }
            if a.IfType == TUNNEL || a.IfType == PROP_VIRTUAL || VIRTUAL_WORDS.iter().any(|w| both.contains(w)) {
                continue;
            }
            if a.IfType == ETHERNET {
                n.ethernet.push((name.clone(), a.TransmitLinkSpeed));
            }
            // Local IPv4 of the adapter that actually routes (has a gateway).
            if (a.IfType == ETHERNET || a.IfType == WIFI) && !a.FirstGatewayAddress.is_null() {
                let mut u = a.FirstUnicastAddress;
                while !u.is_null() {
                    let sa = (*u).Address.lpSockaddr;
                    if !sa.is_null() && (*sa).sa_family == AF_INET {
                        let sin = &*(sa as *const SOCKADDR_IN);
                        let b = sin.sin_addr.S_un.S_un_b;
                        let ip = format!("{}.{}.{}.{}", b.s_b1, b.s_b2, b.s_b3, b.s_b4);
                        if !ip.starts_with("169.254.") && best_ip.as_ref().map(|(m, _, _)| a.Ipv4Metric < *m).unwrap_or(true) {
                            let len = (a.PhysicalAddressLength as usize).min(8);
                            let mac = (len >= 6).then(|| a.PhysicalAddress[..len].iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":"));
                            best_ip = Some((a.Ipv4Metric, ip, mac));
                        }
                        break;
                    }
                    u = (*u).Next;
                }
            }
        }
        if let Some((_, ip, mac)) = best_ip {
            n.ip = Some(ip);
            n.mac = mac;
        }
    }
}

fn wifi() -> Option<Wifi> {
    use windows::Win32::NetworkManagement::WiFi::*;
    unsafe {
        let mut ver = 0u32;
        let mut h = HANDLE::default();
        if WlanOpenHandle(2, None, &mut ver, &mut h) != 0 {
            return None;
        }
        let mut out = None;
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        if WlanEnumInterfaces(h, None, &mut list) == 0 && !list.is_null() {
            let n = (*list).dwNumberOfItems as usize;
            let items = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), n);
            for it in items {
                if it.isState != wlan_interface_state_connected {
                    continue;
                }
                let mut size = 0u32;
                let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
                let r = WlanQueryInterface(h, &it.InterfaceGuid, wlan_intf_opcode_current_connection, None, &mut size, &mut data, None);
                if r != 0 || data.is_null() {
                    // Windows 11 24H2+ hides the connection details from apps
                    // until location is allowed for desktop apps. Being
                    // connected isn't hidden: name it after the network profile
                    // (from the Network List Manager) and read the RSSI.
                    let ssid = nlm_name(&it.InterfaceGuid).unwrap_or_else(|| "Wi-Fi".to_string());
                    let mut signal = 0u8;
                    let (mut rs, mut rd): (u32, *mut core::ffi::c_void) = (0, std::ptr::null_mut());
                    if WlanQueryInterface(h, &it.InterfaceGuid, wlan_intf_opcode_rssi, None, &mut rs, &mut rd, None) == 0 && !rd.is_null() {
                        let dbm = *(rd as *const i32);
                        signal = ((dbm + 100) * 2).clamp(0, 100) as u8;
                        WlanFreeMemory(rd);
                    }
                    let mut band = String::new();
                    let (mut cs, mut cd): (u32, *mut core::ffi::c_void) = (0, std::ptr::null_mut());
                    if WlanQueryInterface(h, &it.InterfaceGuid, wlan_intf_opcode_channel_number, None, &mut cs, &mut cd, None) == 0 && !cd.is_null() {
                        let ch = *(cd as *const u32);
                        band = if ch == 0 { String::new() } else if ch <= 14 { "2.4 GHz".into() } else { "5 GHz".into() };
                        WlanFreeMemory(cd);
                    }
                    out = Some(Wifi { ssid, signal, band });
                    break;
                }
                if r == 0 && !data.is_null() {
                    let attr = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                    let s = &attr.wlanAssociationAttributes.dot11Ssid;
                    let len = (s.uSSIDLength as usize).min(32);
                    let mut ssid = String::from_utf8_lossy(&s.ucSSID[..len]).into_owned();
                    if ssid.trim().is_empty() {
                        ssid = crate::util::from_wide(&attr.strProfileName);
                    }
                    let signal = attr.wlanAssociationAttributes.wlanSignalQuality.min(100) as u8;
                    WlanFreeMemory(data);
                    // Channel → band.
                    let mut band = String::new();
                    let mut csize = 0u32;
                    let mut cdata: *mut core::ffi::c_void = std::ptr::null_mut();
                    if WlanQueryInterface(h, &it.InterfaceGuid, wlan_intf_opcode_channel_number, None, &mut csize, &mut cdata, None) == 0 && !cdata.is_null() {
                        let ch = *(cdata as *const u32);
                        band = if ch == 0 { String::new() } else if ch <= 14 { "2.4 GHz".into() } else { "5 GHz".into() };
                        WlanFreeMemory(cdata);
                    }
                    out = Some(Wifi { ssid, signal, band });
                    break;
                }
            }
            WlanFreeMemory(list as *const _);
        }
        WlanCloseHandle(h, None);
        out
    }
}

/// Name of the network an adapter is on (the Wi-Fi profile name, i.e. the
/// SSID for nearly everyone) — not hidden behind the location permission.
pub fn nlm_name(adapter: &windows::core::GUID) -> Option<String> {
    use windows::Win32::Networking::NetworkListManager::*;
    use windows::Win32::System::Com::*;
    unsafe {
        let nlm: INetworkListManager = CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL).ok()?;
        let conns = nlm.GetNetworkConnections().ok()?;
        loop {
            let mut got = [None];
            let mut n = 0u32;
            if conns.Next(&mut got, Some(&mut n)).is_err() || n == 0 {
                break;
            }
            let Some(c) = got[0].take() else { break };
            if c.GetAdapterId().ok().as_ref() == Some(adapter) {
                let name = c.GetNetwork().ok()?.GetName().ok()?.to_string();
                return (!name.trim().is_empty()).then_some(name);
            }
        }
        None
    }
}

fn bt_radio() -> Option<bool> {
    use windows::Devices::Radios::*;
    let radios = Radio::GetRadiosAsync().ok()?.join().ok()?;
    let mut found = None;
    for i in 0..radios.Size().unwrap_or(0) {
        let Ok(r) = radios.GetAt(i) else { continue };
        if r.Kind().ok() == Some(RadioKind::Bluetooth) {
            let on = r.State().ok() == Some(RadioState::On);
            found = Some(found.unwrap_or(false) || on);
        }
    }
    found
}
