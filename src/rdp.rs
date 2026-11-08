//! Remote Desktop: the island only takes the computer, user name and
//! password; Windows' own Remote Desktop client (mstsc) does the rest.
//!
//! The password goes into Windows Credential Manager (TERMSRV/<computer>,
//! the same place mstsc keeps "saved credentials"), never into Phemio's
//! files. Without "Remember", it's removed again a minute later.

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Security::Credentials::*;

use crate::config;

#[derive(Clone, Debug, Default)]
pub struct Target {
    pub host: String,
    pub user: String,
    pub pass: String,
    pub remember: bool,
    pub full: bool,
    pub all_screens: bool,
}

fn cred_name(host: &str) -> Vec<u16> {
    crate::util::wide(&format!("TERMSRV/{}", host_only(host)))
}

/// "pc.local:3390" → "pc.local" (credentials are kept per computer).
fn host_only(host: &str) -> String {
    let h = host.trim();
    match h.rsplit_once(':') {
        Some((a, p)) if !a.contains(':') && p.chars().all(|c| c.is_ascii_digit()) => a.to_string(),
        _ => h.to_string(),
    }
}

/// A password is saved for this computer (in Windows).
pub fn has_saved(host: &str) -> bool {
    if host.trim().is_empty() {
        return false;
    }
    let name = cred_name(host);
    unsafe {
        let mut p: *mut CREDENTIALW = std::ptr::null_mut();
        if CredReadW(PCWSTR(name.as_ptr()), CRED_TYPE_DOMAIN_PASSWORD, None, &mut p).is_ok() {
            CredFree(p as *const _);
            return true;
        }
    }
    false
}

fn save_cred(host: &str, user: &str, pass: &str, remember: bool) -> Result<(), String> {
    let mut name = cred_name(host);
    let mut u = crate::util::wide(user);
    let blob: Vec<u8> = pass.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    let c = CREDENTIALW {
        Type: CRED_TYPE_DOMAIN_PASSWORD,
        TargetName: PWSTR(name.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_ptr() as *mut u8,
        Persist: if remember { CRED_PERSIST_LOCAL_MACHINE } else { CRED_PERSIST_SESSION },
        UserName: PWSTR(u.as_mut_ptr()),
        ..Default::default()
    };
    unsafe { CredWriteW(&c, 0).map_err(|e| e.message().to_string()) }
}

pub fn forget_password(host: &str) {
    let name = cred_name(host);
    unsafe {
        let _ = CredDeleteW(PCWSTR(name.as_ptr()), CRED_TYPE_DOMAIN_PASSWORD, None);
    }
}

/// Recent computers (newest first): (computer, user name).
pub fn recents() -> Vec<(String, String)> {
    config::cur()
        .s("rdp_recent")
        .split(';')
        .filter_map(|e| {
            let (h, u) = e.split_once('|').unwrap_or((e, ""));
            (!h.trim().is_empty()).then(|| (h.to_string(), u.to_string()))
        })
        .take(8)
        .collect()
}

fn remember_recent(host: &str, user: &str) {
    let mut v = recents();
    v.retain(|(h, _)| !h.eq_ignore_ascii_case(host));
    v.insert(0, (host.to_string(), user.to_string()));
    v.truncate(8);
    let s: Vec<String> = v.iter().map(|(h, u)| format!("{}|{}", h.replace([';', '|'], ""), u.replace([';', '|'], ""))).collect();
    config::update(|c| c.set_s("rdp_recent", &s.join(";")));
}

pub fn forget_recent(host: &str) {
    let mut v = recents();
    v.retain(|(h, _)| !h.eq_ignore_ascii_case(host));
    let s: Vec<String> = v.iter().map(|(h, u)| format!("{h}|{u}")).collect();
    config::update(|c| c.set_s("rdp_recent", &s.join(";")));
    forget_password(host);
}

/// Hand the session to Windows' Remote Desktop client.
pub fn connect(t: &Target) -> Result<(), String> {
    let host = t.host.trim();
    if host.is_empty() {
        return Err(crate::i18n::tr("Type the computer's name or IP address").to_string());
    }
    if !t.user.trim().is_empty() && !t.pass.is_empty() {
        save_cred(host, t.user.trim(), &t.pass, t.remember)?;
        if !t.remember {
            // Long enough for mstsc to sign in, then gone.
            let h = host.to_string();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(90));
                forget_password(&h);
            });
        }
    }
    // A small .rdp file says how: screen mode, all monitors, who signs in.
    let dir = config::dir().join("rdp");
    let _ = std::fs::create_dir_all(&dir);
    let safe: String = host.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
    let file = dir.join(format!("{safe}.rdp"));
    let mut rdp = format!(
        "full address:s:{host}\r\nscreen mode id:i:{}\r\nuse multimon:i:{}\r\nprompt for credentials:i:0\r\nauthentication level:i:2\r\nsmart sizing:i:1\r\ndynamic resolution:i:1\r\nredirectclipboard:i:1\r\n",
        if t.full { 2 } else { 1 },
        t.all_screens as u8
    );
    if !t.user.trim().is_empty() {
        rdp.push_str(&format!("username:s:{}\r\n", t.user.trim()));
    }
    // mstsc reads .rdp files as UTF-16 with a BOM.
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(rdp.encode_utf16().flat_map(|c| c.to_le_bytes()));
    std::fs::write(&file, bytes).map_err(|e| e.to_string())?;
    remember_recent(host, t.user.trim());
    let args = crate::util::wide(&format!("\"{}\"", file.to_string_lossy()));
    unsafe {
        let r = windows::Win32::UI::Shell::ShellExecuteW(None, windows::core::w!("open"), windows::core::w!("mstsc.exe"), PCWSTR(args.as_ptr()), PCWSTR::null(), windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL);
        if (r.0 as isize) <= 32 {
            return Err(crate::i18n::tr("Couldn't open Remote Desktop").to_string());
        }
    }
    Ok(())
}
