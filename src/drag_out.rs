//! Dragging a downloaded file out of the window into another app, such as
//! a DAW's playlist or browser, the way a file is dragged out of Explorer.
//!
//! On Windows the shell builds the drag data for the file (the same
//! `CF_HDROP` and shell formats Explorer offers), and `SHDoDragDrop` runs the
//! drag with the shell's own drop source and drag image until the button is
//! released. That loop is modal, so it runs on the UI thread, started while
//! the button that began the drag is still held.
//!
//! Elsewhere there is no drag: the caller shows the file in its folder.

use std::path::Path;

#[cfg(windows)]
use std::sync::atomic::{AtomicIsize, Ordering};

/// The main window, set once it exists.
#[cfg(windows)]
static WINDOW: AtomicIsize = AtomicIsize::new(0);

/// Remembers the main window so drags start from it.
#[cfg(windows)]
pub fn set_window(hwnd: isize) {
    WINDOW.store(hwnd, Ordering::Relaxed);
}

/// Whether this platform can drag a file out of the window.
pub const SUPPORTED: bool = cfg!(windows);

/// Drags `path` out of the window until the button is released. Returns
/// whether a drag ran; the caller falls back to showing the file when it
/// did not.
#[cfg(not(windows))]
pub fn start(_path: &Path) -> bool {
    false
}

#[cfg(windows)]
pub fn start(path: &Path) -> bool {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows_sys::Win32::System::Ole::{DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK};
    use windows_sys::Win32::UI::Shell::{
        ILClone, ILCreateFromPathW, ILFindLastID, ILFree, ILRemoveLastID, SHCreateDataObject,
        SHDoDragDrop,
    };
    use windows_sys::core::GUID;

    /// `IID_IDataObject`.
    const IID_IDATAOBJECT: GUID = GUID::from_u128(0x0000010e_0000_0000_c000_000000000046);

    if !path.is_file() {
        return false;
    }
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // The UI thread already holds an apartment for drag and drop (winit's
    // OleInitialize); this only adds a reference when it does.
    unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
    unsafe {
        let full = ILCreateFromPathW(wide.as_ptr());
        if full.is_null() {
            return false;
        }
        // The shell wants the folder and the item relative to it.
        let folder = ILClone(full);
        if folder.is_null() {
            ILFree(full);
            return false;
        }
        ILRemoveLastID(folder);
        let child = ILFindLastID(full).cast_const();
        let mut data: *mut c_void = std::ptr::null_mut();
        let created = SHCreateDataObject(
            folder,
            1,
            &child,
            std::ptr::null_mut(),
            &IID_IDATAOBJECT,
            &mut data,
        );
        let mut ran = false;
        if created >= 0 && !data.is_null() {
            let mut effect: DROPEFFECT = 0;
            let hwnd = WINDOW.load(Ordering::Relaxed) as *mut c_void;
            let result = SHDoDragDrop(
                hwnd,
                data,
                std::ptr::null_mut(),
                DROPEFFECT_COPY | DROPEFFECT_LINK,
                &mut effect,
            );
            ran = result >= 0;
            release(data);
        }
        ILFree(folder);
        ILFree(full);
        ran
    }
}

/// `IUnknown::Release` on a COM object pointer.
#[cfg(windows)]
unsafe fn release(object: *mut std::ffi::c_void) {
    #[repr(C)]
    struct Unknown {
        query_interface: usize,
        add_ref: usize,
        release: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32,
    }
    unsafe {
        let vtable = *(object as *const *const Unknown);
        ((*vtable).release)(object);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_missing_file_never_starts_a_drag() {
        assert!(!super::start(std::path::Path::new(
            "/this/file/does/not/exist.wav"
        )));
    }
}
