//! Moves a freshly downloaded copy somewhere it can stay.
//!
//! The release is a zip, and most people open `siphon.exe` from Downloads or
//! straight out of the zip, which Windows unpacks into a temporary folder. Links,
//! the start at login and the profile all attach to the folder a copy runs from,
//! so before any of them exist the first plain launch moves to
//! `%LOCALAPPDATA%\Programs\Encore` and adds a Start menu entry. A copy someone
//! placed elsewhere, or one that already holds data, stays where it is.

use std::path::{Path, PathBuf};

/// Whether a copy in `folder` should move: it sits in one of the `passing`
/// folders, such as Downloads, and is not already in `home`. Windows paths ignore
/// case, so give these already in one case.
fn should_move(folder: &Path, home: &Path, passing: &[PathBuf]) -> bool {
    !folder.starts_with(home) && passing.iter().any(|place| folder.starts_with(place))
}

/// Puts this copy in `home`. The executable arrives last and all at once, so its
/// presence means a whole copy. Writing new files instead of copying leaves the
/// browser's download mark behind, as an installer's files do, so someone who
/// already chose to run Encore is not asked again by the shortcut.
fn copy_into(exe: &Path, home: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(home)?;
    std::fs::write(home.join("siphon-portable.txt"), "siphon-portable-v1\n")?;
    let staged = home.join("siphon.exe.partial");
    std::fs::write(&staged, std::fs::read(exe)?)?;
    let installed = home.join("siphon.exe");
    std::fs::rename(&staged, &installed)?;
    Ok(installed)
}

#[cfg(windows)]
pub use windows::settle_in;

#[cfg(windows)]
mod windows {
    use std::ffi::OsString;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};

    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Downloads, SHGetKnownFolderPath};
    use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

    /// Folders that resolve fully and in one case, so they compare by their parts.
    fn comparable(path: &Path) -> PathBuf {
        let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        resolved.to_string_lossy().to_lowercase().into()
    }

    /// Asks Windows, because the folder is often moved to another drive.
    fn downloads() -> Option<PathBuf> {
        let mut text = std::ptr::null_mut();
        // SAFETY: The folder id outlives the call, which writes one string
        // pointer that is read up to its NUL and then freed as documented.
        unsafe {
            let result =
                SHGetKnownFolderPath(&FOLDERID_Downloads, 0, std::ptr::null_mut(), &mut text);
            let path = (result == 0 && !text.is_null()).then(|| {
                let length = (0..).take_while(|at| *text.add(*at) != 0).count();
                let units = std::slice::from_raw_parts(text, length);
                PathBuf::from(String::from_utf16_lossy(units))
            });
            CoTaskMemFree(text.cast());
            path
        }
    }

    /// Adds the Start menu entry. Windows only offers shortcuts through COM, so
    /// PowerShell makes this one, with the paths in its environment rather than
    /// its script. Encore works without one, so a failure changes nothing.
    fn add_to_start_menu(exe: &Path) {
        const SCRIPT: &str = "$s=(New-Object -ComObject WScript.Shell).CreateShortcut($env:ENCORE_LINK);$s.TargetPath=$env:ENCORE_EXE;$s.Save()";
        let Some(roaming) = std::env::var_os("APPDATA") else {
            return;
        };
        let link =
            PathBuf::from(roaming).join("Microsoft\\Windows\\Start Menu\\Programs\\Encore.lnk");
        let _ = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
            .env("ENCORE_LINK", link)
            .env("ENCORE_EXE", exe)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }

    /// Moves a fresh copy to its permanent folder and continues from there, which
    /// ends this process. Anything that goes wrong leaves this copy to run where
    /// it is, because a player in Downloads beats one that does not open.
    pub fn settle_in(arguments: &[OsString]) {
        // A development build stays put, and anything with arguments is a
        // command or a link rather than someone opening the app.
        if cfg!(debug_assertions) || arguments.len() != 1 {
            return;
        }
        let (Ok(exe), Some(local)) = (std::env::current_exe(), std::env::var_os("LOCALAPPDATA"))
        else {
            return;
        };
        let Some(folder) = exe.parent() else {
            return;
        };
        let home = PathBuf::from(local).join("Programs\\Encore");
        let passing: Vec<_> = downloads()
            .into_iter()
            .chain(Some(std::env::temp_dir()))
            .map(|place| comparable(&place))
            .collect();
        if !super::should_move(&comparable(folder), &comparable(&home), &passing)
            || folder.join("siphon-data").exists()
            || folder.join("spotifast-data").exists()
        {
            return;
        }
        // A copy already there is the one to use. It updates itself, and
        // replacing it could put an older version over a newer one.
        let mut installed = home.join("siphon.exe");
        if !installed.is_file() {
            match super::copy_into(&exe, &home) {
                Ok(copy) => installed = copy,
                Err(_) => return,
            }
            add_to_start_menu(&installed);
        }
        // SAFETY: Takes no pointers. The double click's right to come to the
        // front passes to the copy that opens the window.
        unsafe { AllowSetForegroundWindow(ASFW_ANY) };
        if std::process::Command::new(&installed)
            .current_dir(&home)
            .spawn()
            .is_ok()
        {
            std::process::exit(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_copy_in_a_passing_folder_moves() {
        let home = Path::new(r"c:\users\a\appdata\local\programs\encore");
        let passing = [
            PathBuf::from(r"d:\downloads"),
            PathBuf::from(r"c:\users\a\appdata\local\temp"),
        ];
        for folder in [
            r"d:\downloads\siphon-v1-x86_64-pc-windows-msvc",
            r"d:\downloads",
            r"c:\users\a\appdata\local\temp\temp1_siphon.zip\siphon-v1",
        ] {
            assert!(should_move(Path::new(folder), home, &passing), "{folder}");
        }
        for folder in [
            r"c:\users\a\appdata\local\programs\encore",
            r"d:\apps\encore",
            r"c:\users\a\desktop\encore",
            // Only a whole folder name counts.
            r"d:\downloads-archive\siphon",
        ] {
            assert!(!should_move(Path::new(folder), home, &passing), "{folder}");
        }
    }

    #[test]
    fn a_home_inside_a_passing_folder_still_counts_as_home() {
        let home = Path::new(r"d:\downloads\encore");
        let passing = [PathBuf::from(r"d:\downloads")];
        assert!(!should_move(home, home, &passing));
    }

    #[test]
    fn the_copy_is_whole_and_runs_as_portable() {
        let root = std::env::temp_dir().join(format!("encore-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let exe = root.join("download.exe");
        std::fs::write(&exe, b"the program").unwrap();
        let home = root.join("Programs").join("Encore");

        let installed = copy_into(&exe, &home).unwrap();

        assert_eq!(installed, home.join("siphon.exe"));
        assert_eq!(std::fs::read(&installed).unwrap(), b"the program");
        assert_eq!(
            std::fs::read_to_string(home.join("siphon-portable.txt")).unwrap(),
            "siphon-portable-v1\n"
        );
        assert!(!home.join("siphon.exe.partial").exists());
        assert!(
            exe.exists(),
            "the download stays until its owner removes it"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
