//! Windows 11 DWM integration: real Mica / Acrylic system backdrops.
//!
//! Applies `DWMWA_SYSTEMBACKDROP_TYPE` (attribute 38) to a live `HWND` via
//! the `windows` crate. Verified against Microsoft docs:
//! - Value semantics + min client **Windows 11 Build 22621**:
//!   `DWM_SYSTEMBACKDROP_TYPE` reference (`MAINWINDOW` = Mica default,
//!   `TRANSIENTWINDOW` = Desktop Acrylic, `TABBEDWINDOW` = Mica Alt).
//! - "Mica material" guidance: set backgrounds **transparent** on every
//!   layer where the material should show; high-contrast / transparency-off
//!   / battery-saver / deactivated windows fall back to solid colors (see
//!   [`crate::prefs`]).
//!
//! Complete setup is more than the attribute call. The DWM draws the
//! backdrop behind the *entire window bounds*, so NO
//! `DwmExtendFrameIntoClientArea` call is needed on this path (that is the
//! legacy glass route). What IS required, and lives outside this function:
//! 1. Window created with `transparent: true`
//!    ([`WindowOptions::transparent`](crate::window::WindowOptions::transparent),
//!    forwarded by [`crate::host::window_attributes`]).
//! 2. GPU surface with an alpha-capable format, cleared transparent every
//!    frame (facade/render duty — see `docs/PLATFORM.md`).
//! 3. UI content layers transparent where the material should show.
//! 4. Dark title bars need `DWMWA_USE_IMMERSIVE_DARK_MODE` separately —
//!    provided here as [`apply_titlebar_theme`] since the backdrop call
//!    does not theme the non-client frame.
//!
//! Compile-only checks cannot prove the visual. Verify on a real Windows 11
//! (build >= 22621) host; [`windows_build_number`] reports the build so
//! failures on older systems get an accurate reason instead of a guess.
//!
//! Handle safety (enforced, not just documented):
//! - `HWND` must be non-null; null is rejected with [`MaterialError::NullHandle`].
//! - The window must be a live `winit` window owned by the calling process;
//!   call on the GUI thread right after window creation / on request.
//! - DWM failures are returned, never panicked; fall back to
//!   [`WindowMaterial::None`] using the reported reason.

use crate::material::{
    DwmSystemBackdrop, MaterialCapability, WindowMaterial, DWM_BACKDROP_MIN_BUILD,
};

/// Human-readable backend id for diagnostics.
pub fn backend_name() -> &'static str {
    "windows11-dwm (winit + DwmSetWindowAttribute)"
}

/// Failures applying a native material. All are honest runtime results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaterialError {
    NullHandle,
    DwmCallFailed(&'static str),
}

/// Apply `requested` to the window identified by a raw `HWND` pointer.
///
/// # Safety
/// `hwnd_raw` must be a valid, live `HWND` of a window owned by this process
/// (as produced by `raw-window-handle` for a `winit` window). Null is
/// rejected; dangling handles are a caller bug and may fail in the DWM call.
pub unsafe fn apply_material_hwnd(
    hwnd_raw: *mut core::ffi::c_void,
    requested: WindowMaterial,
) -> Result<MaterialCapability, MaterialError> {
    if hwnd_raw.is_null() {
        return Err(MaterialError::NullHandle);
    }
    if requested == WindowMaterial::None {
        // Still push DWM none so a previous material is cleared.
        let _ = set_backdrop(hwnd_raw, DwmSystemBackdrop::None);
        return Ok(MaterialCapability {
            requested,
            supported: true,
            active: WindowMaterial::None,
            reason: "default opaque frame (backdrop cleared)",
        });
    }
    match windows_build_number() {
        Some(build) if build < DWM_BACKDROP_MIN_BUILD => {
            return Ok(MaterialCapability::fallback(
                requested,
                "backdrop attribute needs Windows 11 build >= 22621",
            ));
        }
        _ => {}
    }
    set_backdrop(hwnd_raw, DwmSystemBackdrop::of(requested)).map(|_| MaterialCapability {
        requested,
        supported: true,
        active: requested,
        reason: "DWM system backdrop applied (verify visually: needs transparent window + alpha surface)",
    })
}

/// Dark/light non-client frame (`DWMWA_USE_IMMERSIVE_DARK_MODE`, attr 20).
/// The backdrop call does not theme the title bar; the facade should call
/// this on [`crate::window::ThemePreference`] changes.
///
/// # Safety
/// Same `HWND` rules as [`apply_material_hwnd`].
pub unsafe fn apply_titlebar_theme(
    hwnd_raw: *mut core::ffi::c_void,
    dark: bool,
) -> Result<(), MaterialError> {
    if hwnd_raw.is_null() {
        return Err(MaterialError::NullHandle);
    }
    set_titlebar_theme(hwnd_raw, dark)
}

/// Portable entry used by shared code on Windows: resolves the `HWND` from
/// a `winit` window and applies the material.
pub fn apply_material_to_window(
    window: &winit::window::Window,
    requested: WindowMaterial,
) -> Result<MaterialCapability, MaterialError> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = window
        .window_handle()
        .map_err(|_| MaterialError::DwmCallFailed("no window handle (window destroyed?)"))?;
    match handle.as_raw() {
        RawWindowHandle::Win32(h) => unsafe {
            apply_material_hwnd(h.hwnd.get() as *mut _, requested)
        },
        _ => Err(MaterialError::DwmCallFailed(
            "non-Win32 handle on Windows build",
        )),
    }
}

/// Facade-facing alias: apply to a live winit window.
pub fn apply_material(
    window: &winit::window::Window,
    requested: WindowMaterial,
) -> Result<MaterialCapability, MaterialError> {
    apply_material_to_window(window, requested)
}

/// Dark/light non-client frame for a live winit window (facade calls this
/// on [`crate::window::ThemePreference`] changes).
pub fn apply_titlebar_theme_to_window(
    window: &winit::window::Window,
    dark: bool,
) -> Result<(), MaterialError> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = window
        .window_handle()
        .map_err(|_| MaterialError::DwmCallFailed("no window handle (window destroyed?)"))?;
    match handle.as_raw() {
        RawWindowHandle::Win32(h) => unsafe { apply_titlebar_theme(h.hwnd.get() as *mut _, dark) },
        _ => Err(MaterialError::DwmCallFailed(
            "non-Win32 handle on Windows build",
        )),
    }
}

/// Current Windows build number (e.g. 22621), read from
/// `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\CurrentBuildNumber`.
/// `None` when unreadable — callers treat that as "unknown", never as
/// proof of support.
#[cfg(windows)]
pub fn windows_build_number() -> Option<u32> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let subkey = HSTRING::from("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
    let value = HSTRING::from("CurrentBuildNumber");
    let mut buf = [0u16; 32];
    let mut size_bytes = (buf.len() * 2) as u32;
    // SAFETY: buffer + length describe a live stack array; registry only writes.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &subkey,
            &value,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            Some(&mut size_bytes),
        )
    };
    if status.0 != 0 {
        return None;
    }
    let len = (size_bytes as usize / 2).min(buf.len());
    String::from_utf16_lossy(&buf[..len])
        .trim_matches('\0')
        .trim()
        .parse::<u32>()
        .ok()
}

/// Headless/test stub: no registry off Windows.
#[cfg(not(windows))]
pub fn windows_build_number() -> Option<u32> {
    None
}

#[cfg(windows)]
fn set_backdrop(
    hwnd_raw: *mut core::ffi::c_void,
    backdrop: DwmSystemBackdrop,
) -> Result<(), MaterialError> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE};
    let value = backdrop as i32;
    let hr = unsafe {
        DwmSetWindowAttribute(
            HWND(hwnd_raw),
            DWMWA_SYSTEMBACKDROP_TYPE,
            &value as *const i32 as *const core::ffi::c_void,
            core::mem::size_of::<i32>() as u32,
        )
    };
    if hr.is_ok() {
        Ok(())
    } else {
        Err(MaterialError::DwmCallFailed(
            "DwmSetWindowAttribute failed (pre-22621 build, policy-blocked, or invalid HWND?)",
        ))
    }
}

#[cfg(windows)]
fn set_titlebar_theme(hwnd_raw: *mut core::ffi::c_void, dark: bool) -> Result<(), MaterialError> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
    let value = BOOL::from(dark);
    let hr = unsafe {
        DwmSetWindowAttribute(
            HWND(hwnd_raw),
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &value as *const BOOL as *const core::ffi::c_void,
            core::mem::size_of::<BOOL>() as u32,
        )
    };
    if hr.is_ok() {
        Ok(())
    } else {
        Err(MaterialError::DwmCallFailed(
            "DWMWA_USE_IMMERSIVE_DARK_MODE failed",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_handle_rejected_without_syscall() {
        let r = unsafe { apply_material_hwnd(core::ptr::null_mut(), WindowMaterial::Mica) };
        assert_eq!(r, Err(MaterialError::NullHandle));
        let t = unsafe { apply_titlebar_theme(core::ptr::null_mut(), true) };
        assert_eq!(t, Err(MaterialError::NullHandle));
    }

    #[test]
    fn build_number_absent_off_windows() {
        #[cfg(not(windows))]
        assert_eq!(windows_build_number(), None);
        #[cfg(windows)]
        let _ = windows_build_number();
    }
}
