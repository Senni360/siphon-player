//! Discovery for the owner's browser extension. Requests use the existing
//! private single-instance channel, never a new HTTP listener.

#[derive(Debug)]
pub enum ResolvedItem {
    Track(Box<crate::api::models::Track>),
    Set {
        uri: String,
        name: String,
        tracks: Vec<crate::api::models::Track>,
    },
}

pub async fn resolve_soundcloud(
    client: &crate::soundcloud::api::Client,
    url: &str,
) -> Result<ResolvedItem, String> {
    let item = client
        .resolve(url)
        .await
        .map_err(|_| "Could not resolve SoundCloud link")?;
    match item {
        crate::soundcloud::models::Resolved::Track(track) => {
            Ok(ResolvedItem::Track(Box::new(track.to_track(None))))
        }
        crate::soundcloud::models::Resolved::Playlist(set) => Ok(ResolvedItem::Set {
            uri: crate::soundcloud::playlist_uri(set.id),
            tracks: set.to_tracks(),
            name: set.title,
        }),
        _ => Err("This SoundCloud link is not a track or set".into()),
    }
}

/// Publish this executable on each launch, before the event loop starts.
/// Registry access happens on a worker, and discovery must never prevent startup.
pub fn register_player() {
    #[cfg(windows)]
    if let Ok(worker) = std::thread::Builder::new()
        .name("extension-discovery".into())
        .spawn(register_windows_player)
    {
        let _ = worker.join();
    }
}

#[cfg(windows)]
fn register_windows_player() {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegSetValueExW,
    };

    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let path: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
    let key: Vec<u16> = "Software\\Siphon Player\0".encode_utf16().collect();
    let name: Vec<u16> = "Exe\0".encode_utf16().collect();
    let mut handle = std::ptr::null_mut();
    // SAFETY: Every string is NUL-terminated UTF-16; all pointers remain valid
    // for the calls, and a successfully opened handle is closed on this thread.
    unsafe {
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut handle,
            std::ptr::null_mut(),
        ) != 0
        {
            return;
        }
        let _ = RegSetValueExW(
            handle,
            name.as_ptr(),
            0,
            REG_SZ,
            path.as_ptr().cast(),
            (path.len() * size_of::<u16>()) as u32,
        );
        RegCloseKey(handle);
    }
}
