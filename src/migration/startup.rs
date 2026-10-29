//! Retire the old executable's login registration after adopting its choice.

use std::{io, path::Path};

use fastframe_autostart::{Autostart, Start, State};

pub(crate) fn migrate(state: &Path) -> io::Result<()> {
    // The portal owns a Flatpak registration under the old sandbox ID. The
    // new sandbox cannot revoke it. Its owner must disable that login item.
    #[cfg(target_os = "linux")]
    if std::env::var_os("FLATPAK_ID").is_some() {
        return Ok(());
    }
    let marker = state.join("siphon-autostart-migration-v1");
    if marker.try_exists()? {
        return Ok(());
    }
    let current = crate::backend::autostart();
    let old = Autostart::new("spotifast", "Spotifast");
    replace(
        current_exists()?,
        old.state(),
        |start| current.set(start),
        || old.set(None),
    )?;
    fastframe_file::write(&marker, "complete\n")
}

fn replace(
    current_exists: bool,
    old: State,
    set: impl FnOnce(Option<Start>) -> io::Result<State>,
    remove_old: impl FnOnce() -> io::Result<State>,
) -> io::Result<()> {
    // An existing Siphon entry wins even if Task Manager disabled it. After
    // a crash between set and remove, this also resumes without resetting it.
    if !current_exists && let Some(start) = old.start() {
        let saved = set(Some(start))?;
        if saved.start() != Some(start) {
            return Err(io::Error::other(
                "could not verify the new login registration",
            ));
        }
    }
    remove_old()?;
    Ok(())
}

#[cfg(windows)]
#[expect(unsafe_code)]
fn current_exists() -> io::Result<bool> {
    use windows_sys::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW},
    };
    let key: Vec<u16> = r"Software\Microsoft\Windows\CurrentVersion\Run"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let value: Vec<u16> = crate::brand::NAME.encode_utf16().chain(Some(0)).collect();
    let mut size = 0;
    // SAFETY: terminated UTF-16 names and a valid length pointer; a null
    // data buffer asks only whether the current registration is present.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    match status {
        ERROR_SUCCESS => Ok(true),
        ERROR_FILE_NOT_FOUND => Ok(false),
        error => Err(io::Error::from_raw_os_error(error as i32)),
    }
}

#[cfg(target_os = "linux")]
fn current_exists() -> io::Result<bool> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::home_dir().map(|home| home.join(".config")))
        .ok_or(io::ErrorKind::NotFound)?;
    config.join("autostart/siphon.desktop").try_exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn adopts_both_login_modes_before_removing_the_old_executable() {
        for start in [Start::Hidden, Start::Window] {
            let saved = Cell::new(false);
            replace(
                false,
                State::On(start),
                |asked| {
                    assert_eq!(asked, Some(start));
                    saved.set(true);
                    Ok(State::On(start))
                },
                || {
                    assert!(saved.get());
                    Ok(State::Off)
                },
            )
            .unwrap();
        }
    }

    #[test]
    fn failed_or_unverified_registration_keeps_the_old_entry_for_retry() {
        for result in [Err(io::ErrorKind::PermissionDenied.into()), Ok(State::Off)] {
            assert!(
                replace(
                    false,
                    State::On(Start::Hidden),
                    |_| result,
                    || panic!("keep the old executable registered")
                )
                .is_err()
            );
        }
    }

    #[test]
    fn existing_or_disabled_current_entry_wins_on_restart() {
        replace(
            true,
            State::On(Start::Hidden),
            |_| panic!("do not change a newer choice"),
            || Ok(State::Off),
        )
        .unwrap();
        replace(
            false,
            State::Off,
            |_| panic!("do not enable a disabled legacy choice"),
            || Ok(State::Off),
        )
        .unwrap();
        assert!(
            replace(
                true,
                State::On(Start::Hidden),
                |_| panic!("already saved before the crash"),
                || Err(io::ErrorKind::PermissionDenied.into())
            )
            .is_err()
        );
    }
}
