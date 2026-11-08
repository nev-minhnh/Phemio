//! System volume (change callback + set) and default output device changes
//! (headphones / Bluetooth earbuds connected). Windows calls us; nothing polls.
#![allow(non_snake_case)] // IPolicyConfig keeps Windows' method names

use windows::core::*;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::*;
use windows::Win32::Media::Audio::Endpoints::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[implement(IAudioEndpointVolumeCallback)]
struct VolCb {
    hwnd: isize,
    msg: u32,
}

impl IAudioEndpointVolumeCallback_Impl for VolCb_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        unsafe {
            if let Some(d) = data.as_ref() {
                let v = (d.fMasterVolume * 1000.0).round() as usize;
                let _ = PostMessageW(Some(HWND(self.hwnd as *mut _)), self.msg, WPARAM(v), LPARAM(d.bMuted.as_bool() as isize));
            }
        }
        Ok(())
    }
}

pub(crate) fn enumerator() -> Option<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok() }
}

pub struct Volume {
    ep: IAudioEndpointVolume,
    cb: Option<IAudioEndpointVolumeCallback>,
}

impl Volume {
    pub fn open(notify: Option<(HWND, u32)>) -> Option<Volume> {
        unsafe {
            let dev = enumerator()?.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
            let ep: IAudioEndpointVolume = dev.Activate(CLSCTX_ALL, None).ok()?;
            let mut v = Volume { ep, cb: None };
            if let Some((hwnd, msg)) = notify {
                let cb: IAudioEndpointVolumeCallback = VolCb { hwnd: hwnd.0 as isize, msg }.into();
                if v.ep.RegisterControlChangeNotify(&cb).is_ok() {
                    v.cb = Some(cb);
                }
            }
            Some(v)
        }
    }

    pub fn get(&self) -> (f32, bool) {
        unsafe {
            let v = self.ep.GetMasterVolumeLevelScalar().unwrap_or(0.0);
            let m = self.ep.GetMute().map(|b| b.as_bool()).unwrap_or(false);
            (v, m)
        }
    }

    pub fn set(&self, level: f32) {
        unsafe {
            let (_, m) = self.get();
            if m && level > 0.0 {
                let _ = self.ep.SetMute(false, std::ptr::null());
            }
            let _ = self.ep.SetMasterVolumeLevelScalar(level.clamp(0.0, 1.0), std::ptr::null());
        }
    }

    pub fn toggle_mute(&self) {
        unsafe {
            let (_, m) = self.get();
            let _ = self.ep.SetMute(!m, std::ptr::null());
        }
    }

    pub fn nudge(&self, delta: f32) {
        unsafe {
            let (v, m) = self.get();
            if m && delta > 0.0 {
                let _ = self.ep.SetMute(false, std::ptr::null());
            }
            let _ = self.ep.SetMasterVolumeLevelScalar((v + delta).clamp(0.0, 1.0), std::ptr::null());
        }
    }
}

impl Drop for Volume {
    fn drop(&mut self) {
        if let Some(cb) = self.cb.take() {
            unsafe {
                let _ = self.ep.UnregisterControlChangeNotify(&cb);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Default output device changes
// ---------------------------------------------------------------------------

#[implement(IMMNotificationClient)]
struct DevCb {
    hwnd: isize,
    msg: u32,
}

impl IMMNotificationClient_Impl for DevCb_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> Result<()> {
        // A headset/speaker connected or disconnected (Bluetooth devices do this).
        unsafe {
            let _ = PostMessageW(Some(HWND(self.hwnd as *mut _)), self.msg, WPARAM(1), LPARAM(0));
        }
        Ok(())
    }
    fn OnDeviceAdded(&self, _id: &PCWSTR) -> Result<()> {
        Ok(())
    }
    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> Result<()> {
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _id: &PCWSTR) -> Result<()> {
        if flow == eRender && role == eConsole {
            unsafe {
                let _ = PostMessageW(Some(HWND(self.hwnd as *mut _)), self.msg, WPARAM(0), LPARAM(0));
            }
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> Result<()> {
        Ok(())
    }
}

pub struct DeviceWatcher {
    en: IMMDeviceEnumerator,
    cb: IMMNotificationClient,
}

impl DeviceWatcher {
    pub fn start(hwnd: HWND, msg: u32) -> Option<DeviceWatcher> {
        unsafe {
            let en = enumerator()?;
            let cb: IMMNotificationClient = DevCb { hwnd: hwnd.0 as isize, msg }.into();
            en.RegisterEndpointNotificationCallback(&cb).ok()?;
            Some(DeviceWatcher { en, cb })
        }
    }
}

impl Drop for DeviceWatcher {
    fn drop(&mut self) {
        unsafe {
            let _ = self.en.UnregisterEndpointNotificationCallback(&self.cb);
        }
    }
}

/// Friendly name of the default output ("Headphones (AirBuds Pro)").
pub fn default_output_name() -> Option<String> {
    unsafe {
        let dev = enumerator()?.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        let store = dev.OpenPropertyStore(STGM_READ).ok()?;
        let v = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
        let p = PropVariantToStringAlloc(&v).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

/// Play a short system sound (focus timer).
pub fn chime() {
    unsafe {
        let _ = PlaySoundW(w!("SystemNotification"), None, SND_ALIAS | SND_ASYNC | SND_NODEFAULT);
    }
}

// ---------------------------------------------------------------------------
// Choosing the sound output
// ---------------------------------------------------------------------------

/// A playback device ("Headphones (Bose QC Ultra 2 HP)", "Speakers (Realtek(R) Audio)").
#[derive(Clone, Debug)]
pub struct Output {
    pub id: String,
    pub name: String,
    /// EndpointFormFactor: 1 speakers, 3 headphones, 4 headset, 9 HDMI/display …
    pub form: u32,
    pub default: bool,
}

/// Active microphones, in Windows' order (the default one marked).
pub fn list_inputs() -> Vec<Output> {
    if let Ok(v) = std::env::var("ISLET_FAKE_INPUTS") {
        return v
            .split(';')
            .filter_map(|e| {
                let p: Vec<&str> = e.rsplitn(2, ':').collect();
                (p.len() == 2).then(|| Output { id: p[1].to_string(), name: p[1].to_string(), form: 0, default: p[0] == "1" })
            })
            .collect();
    }
    list_endpoints(eCapture)
}

/// Active playback devices, in Windows' order.
pub fn list_outputs() -> Vec<Output> {
    // Developer aid: ISLET_FAKE_OUTPUTS="Name:form:default;…"
    if let Ok(v) = std::env::var("ISLET_FAKE_OUTPUTS") {
        return v
            .split(';')
            .filter_map(|e| {
                let p: Vec<&str> = e.rsplitn(3, ':').collect();
                (p.len() == 3).then(|| Output { id: p[2].to_string(), name: p[2].to_string(), form: p[1].parse().unwrap_or(1), default: p[0] == "1" })
            })
            .collect();
    }
    list_endpoints(eRender)
}

fn list_endpoints(flow: EDataFlow) -> Vec<Output> {
    let mut out = Vec::new();
    unsafe {
        let Some(en) = enumerator() else { return out };
        let def = en.GetDefaultAudioEndpoint(flow, eMultimedia).ok().and_then(|d| d.GetId().ok()).map(|p| {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        });
        let Ok(col) = en.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) else { return out };
        for i in 0..col.GetCount().unwrap_or(0) {
            let Ok(dev) = col.Item(i) else { continue };
            let Ok(p) = dev.GetId() else { continue };
            let id = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            let store = dev.OpenPropertyStore(STGM_READ).ok();
            let name = store
                .as_ref()
                .and_then(|st| st.GetValue(&PKEY_Device_FriendlyName).ok())
                .and_then(|v| PropVariantToStringAlloc(&v).ok())
                .map(|p| {
                    let s = p.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(p.0 as *const _));
                    s
                })
                .unwrap_or_default();
            let form = store
                .as_ref()
                .and_then(|st| st.GetValue(&PKEY_AudioEndpoint_FormFactor).ok())
                .and_then(|v| windows::Win32::System::Com::StructuredStorage::PropVariantToUInt32(&v).ok())
                .unwrap_or(1);
            out.push(Output { default: def.as_deref() == Some(id.as_str()), id, name, form });
        }
    }
    out
}

/// Windows' own (undocumented, but long-stable) interface for picking the
/// default device — the same one the volume flyout and tools like EarTrumpet use.
#[windows::core::interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: windows::core::IUnknown {
    unsafe fn GetMixFormat(&self, id: PCWSTR, fmt: *mut *mut core::ffi::c_void) -> HRESULT;
    unsafe fn GetDeviceFormat(&self, id: PCWSTR, default: i32, fmt: *mut *mut core::ffi::c_void) -> HRESULT;
    unsafe fn ResetDeviceFormat(&self, id: PCWSTR) -> HRESULT;
    unsafe fn SetDeviceFormat(&self, id: PCWSTR, endpoint: *mut core::ffi::c_void, mix: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn GetProcessingPeriod(&self, id: PCWSTR, default: i32, def_period: *mut i64, min_period: *mut i64) -> HRESULT;
    unsafe fn SetProcessingPeriod(&self, id: PCWSTR, period: *mut i64) -> HRESULT;
    unsafe fn GetShareMode(&self, id: PCWSTR, mode: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn SetShareMode(&self, id: PCWSTR, mode: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn GetPropertyValue(&self, id: PCWSTR, store: i32, key: *const core::ffi::c_void, value: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn SetPropertyValue(&self, id: PCWSTR, store: i32, key: *const core::ffi::c_void, value: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn SetDefaultEndpoint(&self, id: PCWSTR, role: i32) -> HRESULT;
    unsafe fn SetEndpointVisibility(&self, id: PCWSTR, visible: i32) -> HRESULT;
}

const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

/// Make `id` the default device (playback or microphone — for music, apps and calls).
pub fn set_default_output(id: &str) -> bool {
    unsafe {
        let Ok(pc) = CoCreateInstance::<_, IPolicyConfig>(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL) else { return false };
        let w = crate::util::wide(id);
        let mut ok = true;
        for role in [eConsole.0, eMultimedia.0, eCommunications.0] {
            ok &= pc.SetDefaultEndpoint(PCWSTR(w.as_ptr()), role).is_ok();
        }
        ok
    }
}

/// Connect (or disconnect) a paired Bluetooth headset / speaker by name, the
/// way the Sound settings "Connect" button does: a one-shot request on the
/// device's kernel-streaming filter. Returns false when no Bluetooth audio
/// endpoint matches `name` (mice, keyboards… can't be connected this way).
pub fn bt_audio_connect(name: &str, on: bool) -> bool {
    use windows::Win32::Media::KernelStreaming::*;
    let want = name.to_lowercase();
    let mut done = false;
    unsafe {
        let Some(en) = enumerator() else { return false };
        let mask = DEVICE_STATE(DEVICE_STATE_ACTIVE.0 | DEVICE_STATE_UNPLUGGED.0);
        let mut filters: Vec<String> = Vec::new();
        for flow in [eRender, eCapture] {
            let Ok(col) = en.EnumAudioEndpoints(flow, mask) else { continue };
            for i in 0..col.GetCount().unwrap_or(0) {
                let Ok(dev) = col.Item(i) else { continue };
                let ename = dev
                    .OpenPropertyStore(STGM_READ)
                    .ok()
                    .and_then(|st| st.GetValue(&PKEY_Device_FriendlyName).ok())
                    .and_then(|v| PropVariantToStringAlloc(&v).ok())
                    .map(|p| {
                        let s = p.to_string().unwrap_or_default();
                        CoTaskMemFree(Some(p.0 as *const _));
                        s
                    })
                    .unwrap_or_default();
                let inner = crate::bt::inner(&ename).to_lowercase();
                if !(inner == want || inner.contains(&want) || (!inner.is_empty() && want.contains(&inner))) {
                    continue;
                }
                let filter = (|| -> Result<String> {
                    let topo: IDeviceTopology = dev.Activate(CLSCTX_ALL, None)?;
                    let other = topo.GetConnector(0)?.GetConnectedTo()?;
                    let part: IPart = other.cast()?;
                    let ft = part.GetTopologyObject()?;
                    let p = ft.GetDeviceId()?;
                    let s = p.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(p.0 as *const _));
                    Ok(s)
                })();
                let Ok(filter) = filter else { continue };
                if !filter.to_lowercase().contains("bth") || filters.contains(&filter) {
                    continue;
                }
                filters.push(filter);
            }
        }
        for f in &filters {
            let r = (|| -> Result<()> {
                let fid = HSTRING::from(f.as_str());
                let fdev = en.GetDevice(&fid)?;
                let ks: IKsControl = fdev.Activate(CLSCTX_ALL, None)?;
                let prop = KSIDENTIFIER {
                    Anonymous: KSIDENTIFIER_0 {
                        Anonymous: KSIDENTIFIER_0_0 {
                            Set: KSPROPSETID_BtAudio,
                            Id: if on { KSPROPERTY_ONESHOT_RECONNECT.0 as u32 } else { KSPROPERTY_ONESHOT_DISCONNECT.0 as u32 },
                            Flags: KSPROPERTY_TYPE_GET,
                        },
                    },
                };
                let mut ret = 0u32;
                ks.KsProperty(&prop, std::mem::size_of::<KSIDENTIFIER>() as u32, std::ptr::null_mut(), 0, &mut ret)
            })();
            match r {
                Ok(()) => done = true,
                Err(e) => util_log(&format!("bt audio {}: {e}", if on { "connect" } else { "disconnect" })),
            }
        }
    }
    done
}

fn util_log(s: &str) {
    crate::util::log(s);
}
