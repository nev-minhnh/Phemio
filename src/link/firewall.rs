//! Windows Firewall and Link.
//!
//! Windows lets a program receive from other PCs only once it has a rule
//! for it (the "Allow access" prompt the first time it listens). The rule is
//! for the program's path, so a renamed or moved Phemio.exe starts with
//! none — or with a block rule, when that prompt was closed or cancelled —
//! and then other PCs' broadcasts never get in: they're found only after
//! this PC asked them directly (much slower). This checks for the rule and,
//! when asked, adds one (that needs an administrator: Windows asks).

use std::sync::atomic::{AtomicU8, Ordering};
use windows::core::{Interface, BSTR};
use windows::Win32::NetworkManagement::WindowsFirewall::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::IEnumVARIANT;
use windows::Win32::System::Variant::VARIANT;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fw {
    /// Not checked yet (or the check failed).
    Unknown,
    /// The firewall is off for the network this PC is on.
    Off,
    /// Phemio may receive from other PCs.
    Allowed,
    /// Nothing for Phemio yet.
    Missing,
    /// A rule blocks Phemio (the prompt was cancelled once).
    Blocked,
}

static STATE: AtomicU8 = AtomicU8::new(0);
static BUSY: AtomicU8 = AtomicU8::new(0);

pub fn state() -> Fw {
    match STATE.load(Ordering::Relaxed) {
        1 => Fw::Off,
        2 => Fw::Allowed,
        3 => Fw::Missing,
        4 => Fw::Blocked,
        _ => Fw::Unknown,
    }
}

fn set(f: Fw) {
    STATE.store(
        match f {
            Fw::Unknown => 0,
            Fw::Off => 1,
            Fw::Allowed => 2,
            Fw::Missing => 3,
            Fw::Blocked => 4,
        },
        Ordering::Relaxed,
    );
}

/// Adding the rule is under way (Windows is asking, or netsh is running).
pub fn busy() -> bool {
    BUSY.load(Ordering::Relaxed) != 0
}

fn exe() -> String {
    std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
}

/// "%LOCALAPPDATA%\x" and the like → the real path.
fn expand(p: &str) -> String {
    let mut out = p.to_string();
    for v in ["LOCALAPPDATA", "APPDATA", "USERPROFILE", "ProgramFiles", "ProgramFiles(x86)", "SystemDrive", "SystemRoot", "windir"] {
        let key = format!("%{v}%");
        if out.to_ascii_lowercase().contains(&key.to_ascii_lowercase()) {
            if let Ok(val) = std::env::var(v) {
                let i = out.to_ascii_lowercase().find(&key.to_ascii_lowercase()).unwrap();
                out.replace_range(i..i + key.len(), &val);
            }
        }
    }
    out
}

fn same_path(a: &str, b: &str) -> bool {
    let n = |s: &str| expand(s.trim().trim_matches('"')).replace('/', "\\").to_lowercase();
    !a.trim().is_empty() && n(a) == n(b)
}

/// Looks the rules up (no administrator needed). Slow-ish (a few hundred
/// rules), so it runs on its own thread; `state()` has the answer.
pub fn check() {
    std::thread::Builder::new()
        .name("link-firewall".into())
        .spawn(|| {
            let f = unsafe { look() }.unwrap_or(Fw::Unknown);
            set(f);
            crate::util::log(&format!("link: Windows Firewall for {}: {f:?}", exe()));
            super::tell_ui();
        })
        .ok();
}

unsafe fn look() -> windows::core::Result<Fw> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let policy: INetFwPolicy2 = CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)?;
        let cur = policy.CurrentProfileTypes()?;
        let mut any_on = false;
        for p in [NET_FW_PROFILE2_DOMAIN, NET_FW_PROFILE2_PRIVATE, NET_FW_PROFILE2_PUBLIC] {
            if cur & p.0 != 0 && policy.get_FirewallEnabled(p).map(|b| b.as_bool()).unwrap_or(true) {
                any_on = true;
            }
        }
        if !any_on {
            return Ok(Fw::Off);
        }
        let me = exe();
        let rules = policy.Rules()?;
        let en: IEnumVARIANT = rules._NewEnum()?.cast()?;
        let (mut allowed, mut blocked) = (false, false);
        loop {
            let mut v = [VARIANT::default()];
            let mut got = 0u32;
            let hr = en.Next(&mut v, &mut got);
            if hr.is_err() || got == 0 {
                break;
            }
            let Ok(d) = IDispatch::try_from(&v[0]) else { continue };
            let Ok(r) = d.cast::<INetFwRule>() else { continue };
            if r.Direction().ok() != Some(NET_FW_RULE_DIR_IN) || !r.Enabled().map(|b| b.as_bool()).unwrap_or(false) {
                continue;
            }
            let app = r.ApplicationName().map(|b| b.to_string()).unwrap_or_default();
            if !same_path(&app, &me) {
                continue;
            }
            let prof = r.Profiles().unwrap_or(0x7fff_ffff);
            if prof & cur == 0 {
                continue;
            }
            match r.Action() {
                Ok(NET_FW_ACTION_BLOCK) => blocked = true,
                Ok(NET_FW_ACTION_ALLOW) => allowed = true,
                _ => {}
            }
        }
        Ok(if blocked {
            Fw::Blocked
        } else if allowed {
            Fw::Allowed
        } else {
            Fw::Missing
        })
    }
}

/// Settings → Allow: runs Phemio.exe --firewall-allow as administrator
/// (Windows asks), which takes out the rules for this exe (block ones
/// included) and adds one that lets it in on every kind of network.
pub fn allow() {
    if BUSY.swap(1, Ordering::Relaxed) != 0 {
        return;
    }
    std::thread::Builder::new()
        .name("link-firewall-allow".into())
        .spawn(|| {
            use windows::Win32::Foundation::CloseHandle;
            use windows::Win32::System::Threading::{WaitForSingleObject, INFINITE};
            use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
            use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
            let file: Vec<u16> = exe().encode_utf16().chain(Some(0)).collect();
            let args: Vec<u16> = "--firewall-allow".encode_utf16().chain(Some(0)).collect();
            let mut info = SHELLEXECUTEINFOW {
                cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask: SEE_MASK_NOCLOSEPROCESS,
                lpVerb: windows::core::w!("runas"),
                lpFile: windows::core::PCWSTR(file.as_ptr()),
                lpParameters: windows::core::PCWSTR(args.as_ptr()),
                nShow: SW_HIDE.0,
                ..Default::default()
            };
            unsafe {
                if ShellExecuteExW(&mut info).is_ok() && !info.hProcess.is_invalid() {
                    WaitForSingleObject(info.hProcess, INFINITE);
                    let _ = CloseHandle(info.hProcess);
                } else {
                    crate::util::log("link: firewall rule not added (administrator prompt declined)");
                }
            }
            BUSY.store(0, Ordering::Relaxed);
            check();
        })
        .ok();
}

/// The elevated side of `allow()` (Phemio.exe --firewall-allow): change
/// the rules and exit.
pub fn allow_elevated() {
    unsafe {
        let r = (|| -> windows::core::Result<()> {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let policy: INetFwPolicy2 = CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)?;
            let rules = policy.Rules()?;
            let me = exe();
            // Rules for this exe, by name (several can share one name).
            let mut gone = Vec::new();
            let en: IEnumVARIANT = rules._NewEnum()?.cast()?;
            loop {
                let mut v = [VARIANT::default()];
                let mut got = 0u32;
                if en.Next(&mut v, &mut got).is_err() || got == 0 {
                    break;
                }
                let Ok(d) = IDispatch::try_from(&v[0]) else { continue };
                let Ok(r) = d.cast::<INetFwRule>() else { continue };
                if r.Direction().ok() == Some(NET_FW_RULE_DIR_IN) && same_path(&r.ApplicationName().map(|b| b.to_string()).unwrap_or_default(), &me) {
                    if let Ok(n) = r.Name() {
                        gone.push(n.to_string());
                    }
                }
            }
            for n in gone {
                // Remove takes one rule per call; repeat while that name still has
                // one for this exe.
                for _ in 0..16 {
                    let still = rules.Item(&BSTR::from(n.as_str())).ok().map(|r| same_path(&r.ApplicationName().map(|b| b.to_string()).unwrap_or_default(), &me)).unwrap_or(false);
                    if !still || rules.Remove(&BSTR::from(n.as_str())).is_err() {
                        break;
                    }
                }
            }
            let rule: INetFwRule = CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER)?;
            rule.SetName(&BSTR::from("Phemio"))?;
            rule.SetDescription(&BSTR::from("Lets Phemio find and link with your other PCs on this network"))?;
            rule.SetApplicationName(&BSTR::from(me.as_str()))?;
            rule.SetDirection(NET_FW_RULE_DIR_IN)?;
            rule.SetAction(NET_FW_ACTION_ALLOW)?;
            rule.SetProfiles(NET_FW_PROFILE2_ALL.0)?;
            rule.SetEnabled(windows::Win32::Foundation::VARIANT_TRUE)?;
            rules.Add(&rule)?;
            Ok(())
        })();
        crate::util::log(&format!("link: firewall rule for Phemio added (as administrator): {r:?}"));
    }
}
