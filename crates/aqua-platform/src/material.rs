//! Native window materials (Mica / Acrylic on Windows).
//!
//! Semantic intent in, honest capability out. In-app blur is the renderer's
//! job; this module only covers the OS compositor material *behind* the
//! window. Non-Windows always reports an explicit fallback — never silent
//! success.

/// Semantic OS material request. This is intent, not a pixel promise:
/// the compositor decides the exact appearance.
///
/// Role mapping (verified against
/// `DWM_SYSTEMBACKDROP_TYPE` docs, min client Windows 11 Build 22621):
/// - `Mica` = `DWMSBT_MAINWINDOW`: default Mica for long-lived windows.
/// - `MicaAlt` = `DWMSBT_TABBEDWINDOW`: Mica *alternate* variant with
///   stronger tinting, designed for tabbed title bars. NOT an
///   "inactive-state" variant — inactivity fallback is a separate OS
///   behavior (neutral solid color), handled via [`crate::prefs`].
/// - `Acrylic` = `DWMSBT_TRANSIENTWINDOW`: Desktop (Background) Acrylic for
///   transient windows, brightest variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowMaterial {
    /// Opaque / default frame. Always available.
    #[default]
    None,
    /// Wallpaper-derived material for long-lived windows (Windows 11 Mica).
    Mica,
    /// Stronger-tinted Mica alternate for tabbed title bars (Mica Alt).
    MicaAlt,
    /// Frosted live-blur of content behind the window (Desktop Acrylic).
    Acrylic,
}

/// Minimum Windows 11 build for `DWMWA_SYSTEMBACKDROP_TYPE`
/// (per `DWM_SYSTEMBACKDROP_TYPE` requirements table).
pub const DWM_BACKDROP_MIN_BUILD: u32 = 22621;

/// Whether a material can show through without a transparent window +
/// alpha surface + transparent content layers (per Mica guidance: "set the
/// background to transparent for all layers where you want to see Mica").
///
/// The facade MUST create the window with
/// [`WindowOptions::transparent`](crate::window::WindowOptions::transparent)
/// set and clear the GPU surface transparently, or the material is silently
/// hidden behind opaque content. Compile-only checks cannot prove the
/// visual — verify on a real Windows 11 host.
pub fn material_needs_transparent_window(material: WindowMaterial) -> bool {
    material != WindowMaterial::None
}

/// Resolve a material request against live OS preferences: high contrast or
/// transparency-off forces the solid fallback (`None`), matching the OS
/// behavior documented in the Mica guidance. Pure — the integration point
/// between [`crate::prefs`] and the DWM apply path.
pub fn select_material(
    requested: WindowMaterial,
    prefs: &crate::prefs::OsPreferences,
) -> WindowMaterial {
    if requested != WindowMaterial::None && crate::prefs::prefers_opaque_frame(prefs) {
        WindowMaterial::None
    } else {
        requested
    }
}

/// What applying a material would actually do on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialCapability {
    /// The request as understood here.
    pub requested: WindowMaterial,
    /// Whether the OS/compositor path exists on this platform+build.
    pub supported: bool,
    /// What will actually be used. Equals `requested` iff supported.
    pub active: WindowMaterial,
    /// Human-readable reason (build number, compositor, log line).
    pub reason: &'static str,
}

impl MaterialCapability {
    pub fn fallback(requested: WindowMaterial, reason: &'static str) -> Self {
        Self {
            requested,
            supported: false,
            active: WindowMaterial::None,
            reason,
        }
    }
}

/// Report the capability for `requested` on the *compiling* platform.
///
/// Windows: Mica/MicaAlt/Acrylic have real DWM paths (see
/// [`adapters::windows`]); runtime build support is checked at apply time
/// and any failure falls back explicitly. Other platforms: honest fallback.
pub fn material_capability(requested: WindowMaterial) -> MaterialCapability {
    if requested == WindowMaterial::None {
        return MaterialCapability {
            requested,
            supported: true,
            active: WindowMaterial::None,
            reason: "default opaque frame",
        };
    }
    #[cfg(windows)]
    {
        return MaterialCapability {
            requested,
            supported: true,
            active: requested,
            reason: "Windows 11 DWM system-backdrop path; runtime build check at apply",
        };
    }
    #[cfg(not(windows))]
    {
        MaterialCapability::fallback(
            requested,
            "native material path is Windows-only in this adapter",
        )
    }
}

/// DWM `DWM_SYSTEMBACKDROP_TYPE` values (Win32 `dwmapi`, attribute 38).
/// Documented at learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwm_systembackdrop_type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum DwmSystemBackdrop {
    Auto = 0,
    None = 1,
    MainWindow = 2,      // Mica
    TransientWindow = 3, // Acrylic-like transient
    TabbedWindow = 4,    // Mica Alt
}

impl DwmSystemBackdrop {
    pub fn of(material: WindowMaterial) -> Self {
        match material {
            WindowMaterial::None => DwmSystemBackdrop::None,
            WindowMaterial::Mica => DwmSystemBackdrop::MainWindow,
            WindowMaterial::MicaAlt => DwmSystemBackdrop::TabbedWindow,
            WindowMaterial::Acrylic => DwmSystemBackdrop::TransientWindow,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_always_supported() {
        let c = material_capability(WindowMaterial::None);
        assert!(c.supported);
        assert_eq!(c.active, WindowMaterial::None);
    }

    #[test]
    fn non_windows_falls_back_explicitly() {
        #![allow(clippy::assertions_on_constants)]
        let c = material_capability(WindowMaterial::Mica);
        #[cfg(not(windows))]
        {
            assert!(!c.supported);
            assert_eq!(c.active, WindowMaterial::None);
            assert!(!c.reason.is_empty());
        }
        #[cfg(windows)]
        {
            assert!(c.supported);
            assert_eq!(c.active, WindowMaterial::Mica);
        }
    }

    #[test]
    fn backdrop_mapping_covers_all_materials() {
        assert_eq!(DwmSystemBackdrop::of(WindowMaterial::Mica) as i32, 2);
        assert_eq!(DwmSystemBackdrop::of(WindowMaterial::Acrylic) as i32, 3);
        assert_eq!(DwmSystemBackdrop::of(WindowMaterial::MicaAlt) as i32, 4);
        assert_eq!(DwmSystemBackdrop::of(WindowMaterial::None) as i32, 1);
    }

    #[test]
    fn preferences_drive_material_fallback() {
        use crate::prefs::{OsPreferences, PrefState};
        let plain = OsPreferences::fallback();
        assert_eq!(
            select_material(WindowMaterial::Mica, &plain),
            WindowMaterial::Mica
        );
        assert!(material_needs_transparent_window(WindowMaterial::MicaAlt));
        assert!(!material_needs_transparent_window(WindowMaterial::None));
        let mut hc = OsPreferences::fallback();
        hc.high_contrast = PrefState::On;
        assert_eq!(
            select_material(WindowMaterial::Acrylic, &hc),
            WindowMaterial::None,
            "high contrast must force the solid fallback"
        );
        let mut tr = OsPreferences::fallback();
        tr.reduce_transparency = PrefState::On;
        assert_eq!(
            select_material(WindowMaterial::Mica, &tr),
            WindowMaterial::None
        );
    }
}
