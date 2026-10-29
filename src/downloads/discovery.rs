//! Read the same per-user native-host registration as Helium.

#[cfg(any(windows, test))]
use std::path::Path;
use std::path::PathBuf;

#[cfg(any(windows, test))]
use serde::Deserialize;

#[cfg(any(windows, test))]
pub(super) const HOST_NAME: &str = "com.siphon.host";

#[derive(Debug)]
pub(super) struct Helper {
    pub launcher: PathBuf,
}

#[derive(Deserialize)]
#[cfg(any(windows, test))]
struct Manifest {
    name: String,
    #[serde(rename = "type")]
    transport: String,
    path: PathBuf,
}

#[cfg(any(windows, test))]
fn parse_manifest(bytes: &[u8], manifest_path: &Path) -> Option<Helper> {
    let manifest: Manifest = serde_json::from_slice(bytes).ok()?;
    if manifest.name != HOST_NAME || manifest.transport != "stdio" {
        return None;
    }
    let path = if manifest.path.is_absolute() {
        manifest.path
    } else {
        manifest_path.parent()?.join(manifest.path)
    };
    // A registered Windows host may be the installer's .cmd launcher (it
    // supplies FFmpeg/Node's PATH). Never interpolate shell expansions.
    let text = path.to_str()?;
    if text
        .chars()
        .any(|c| c.is_control() || matches!(c, '"' | '%' | '!'))
    {
        return None;
    }
    let extension = path.extension()?.to_str()?;
    if !["exe", "cmd", "bat"]
        .iter()
        .any(|e| extension.eq_ignore_ascii_case(e))
    {
        return None;
    }
    Some(Helper { launcher: path })
}

/// All filesystem and registry access happens on a blocking runtime worker.
#[cfg(windows)]
pub(super) fn discover() -> Option<Helper> {
    use std::fs;
    // installer/common.ps1 registers these in this order. Helium uses the
    // first two; keep the others for installations shared with Edge/Brave.
    for vendor in [
        "Chromium",
        "Google\\Chrome",
        "Microsoft\\Edge",
        "BraveSoftware\\Brave-Browser",
    ] {
        let key = format!("Software\\{vendor}\\NativeMessagingHosts\\{HOST_NAME}");
        let Some(path) = registry_path(&key) else {
            continue;
        };
        if fs::metadata(&path).is_ok_and(|m| m.len() <= 64 * 1024)
            && let Ok(bytes) = fs::read(&path)
            && let Some(helper) = parse_manifest(&bytes, &path)
            && helper.launcher.is_file()
        {
            return Some(helper);
        }
    }
    None
}

#[cfg(windows)]
fn registry_path(key: &str) -> Option<PathBuf> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};

    let key: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    let mut length = 0u32;
    // Only REG_SZ, as written by the installer. A registry value is data,
    // never a command or a path to search on PATH.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            std::ptr::null(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut length,
        )
    };
    if result != 0 || !(2..=65536).contains(&length) || !length.is_multiple_of(2) {
        return None;
    }
    let mut value = vec![0u16; length as usize / 2];
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            std::ptr::null(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            value.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if result != 0 || !length.is_multiple_of(2) {
        return None;
    }
    value.truncate(length as usize / 2);
    let path = registry_value(&value)?;
    path.is_absolute().then_some(path)
}

#[cfg(any(windows, test))]
fn registry_value(value: &[u16]) -> Option<PathBuf> {
    let text = String::from_utf16(value.strip_suffix(&[0])?).ok()?;
    (!text.is_empty() && !text.contains('\0')).then(|| PathBuf::from(text))
}

#[cfg(not(windows))]
pub(super) fn discover() -> Option<Helper> {
    // The extension currently installs its helper only on Windows.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_host_manifest_fixtures_resolve_absolute_and_relative_launchers() {
        let root = std::env::current_dir().unwrap().join("host fixtures");
        let manifest_path = root.join("com.siphon.host.json");
        for path in [
            PathBuf::from("siphon-host.cmd"),
            root.join("siphon-host.exe"),
        ] {
            let bytes = serde_json::to_vec(&json!({
                "name": HOST_NAME, "type": "stdio", "path": path,
                "allowed_origins": ["chrome-extension://bieofegakabfdjplcnpiaegdifmempmf/"]
            }))
            .unwrap();
            let helper = parse_manifest(&bytes, &manifest_path).unwrap();
            assert_eq!(
                helper.launcher,
                if path.is_absolute() {
                    path
                } else {
                    root.join(path)
                }
            );
        }
    }

    #[test]
    fn rejects_other_hosts_transports_and_shell_expansion() {
        let root = std::env::current_dir().unwrap().join("host.json");
        for value in [
            json!({"name": "other.host", "type": "stdio", "path": "host.exe"}),
            json!({"name": HOST_NAME, "type": "socket", "path": "host.exe"}),
            json!({"name": HOST_NAME, "type": "stdio", "path": "%TEMP%/host.cmd"}),
            json!({"name": HOST_NAME, "type": "stdio", "path": "host.cmd\" & echo bad"}),
            json!({"name": HOST_NAME, "type": "stdio", "path": "host.ps1"}),
            json!({"name": HOST_NAME, "type": "stdio", "path": "host\n.exe"}),
            json!({"name": HOST_NAME, "type": "stdio", "path": ""}),
        ] {
            assert!(parse_manifest(&serde_json::to_vec(&value).unwrap(), &root).is_none());
        }
        assert!(parse_manifest(b"not json", &root).is_none());
    }

    #[test]
    fn registry_value_fixtures_need_one_terminated_nonempty_string() {
        let path = "C:\\Users\\Zoë\\Siphon\\com.siphon.host.json";
        let value: Vec<_> = path.encode_utf16().chain(Some(0)).collect();
        assert_eq!(registry_value(&value), Some(PathBuf::from(path)));
        for value in [
            vec![],
            vec![0],
            vec![65],
            vec![65, 0, 66, 0],
            vec![0xd800, 0],
        ] {
            assert!(registry_value(&value).is_none());
        }
    }
}
