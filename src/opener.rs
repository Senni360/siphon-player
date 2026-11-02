//! Opens links and folders with the desktop.
//!
//! Other platforms use the `open` crate. Windows uses `ShellExecuteW` because
//! the crate's PowerShell launcher can be unavailable (#107), while
//! `cmd /c start` expands percent-encoded URL segments as environment
//! variables.

use std::ffi::OsStr;
use std::io;

/// Opens a URL or a path in whatever the desktop uses for it.
#[cfg(not(windows))]
pub fn open(target: impl AsRef<OsStr>) -> io::Result<()> {
    open::that(target.as_ref())
}

/// Opens a URL or a path in whatever the desktop uses for it.
///
/// When Encore itself is the desktop's handler for web links, a web address
/// goes straight to the browser: asked to open one, Encore means the page, and
/// the handler would send a music link right back to Encore.
#[cfg(windows)]
pub fn open(target: impl AsRef<OsStr>) -> io::Result<()> {
    if let Some(url) = target.as_ref().to_str()
        && (url.starts_with("https://") || url.starts_with("http://"))
        && crate::web_links::replaces_browser()
    {
        return crate::web_links::open_in_browser(url);
    }
    open_shell(target)
}

/// Hands a target to the desktop's own handler for it, whatever that is.
#[cfg(windows)]
pub(crate) fn open_shell(target: impl AsRef<OsStr>) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(text: &OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }

    // Shell extensions may require COM. Keep an existing apartment mode if
    // the worker thread was already initialized.
    unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };

    let file = wide(target.as_ref());
    let verb = wide(OsStr::new("open"));
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Anything above 32 opened. At or below it the value is one of the
    // old SE_ERR_ codes, which the thread's last error carries too.
    if result as isize > 32 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Shows a file in its folder: selected in Explorer on Windows, the folder
/// opened elsewhere. Spawns and returns at once.
pub fn reveal(path: &std::path::Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // explorer.exe parses its own command line, so the path is passed
        // raw, quoted, after /select.
        let mut argument = std::ffi::OsString::from("/select,\"");
        argument.push(path.as_os_str());
        argument.push("\"");
        std::process::Command::new("explorer.exe")
            .raw_arg(argument)
            .spawn()
            .map(drop)
    }
    #[cfg(not(windows))]
    {
        open(path.parent().unwrap_or(path))
    }
}
