//! OS accessibility/display preferences: reduced motion, reduced
//! transparency, high contrast.
//!
//! Sources, in priority order:
//! 1. `AQUA_PREF_*` env overrides (`1`/`true` = on, `0`/`false` = off) —
//!    deterministic headless tests and CI.
//! 2. Windows system state via documented `SystemParametersInfoW` /
//!    Personalize registry reads (`cfg(windows)` only).
//! 3. Explicit `Unknown` fallback elsewhere — never guessed.
//!
//! These directly drive honest behavior: reduced motion pauses non-essential
//! animation, high contrast / transparency-off switch native materials to
//! solid fallbacks (per Mica guidance), and the animation scheduler skips
//! purely decorative deadlines when motion is reduced.

use crate::window::ThemePreference;

/// One preference reading. `Unknown` means "not observable here" and must
/// be treated as off by behavior code, but REPORTED as unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrefState {
    On,
    Off,
    #[default]
    Unknown,
}

impl PrefState {
    pub fn is_on(self) -> bool {
        matches!(self, PrefState::On)
    }
}

/// Where the readings came from (diagnostics, not behavior).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreferenceSource {
    EnvOverride,
    WindowsSystem,
    Fallback,
}

/// Snapshot of OS preferences + last known OS theme hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsPreferences {
    pub reduce_motion: PrefState,
    pub reduce_transparency: PrefState,
    pub high_contrast: PrefState,
    /// Last `ThemeChanged` value forwarded by the facade; `Unknown` until
    /// the OS reports one (winit: Windows/macOS only).
    pub theme: ThemePreference,
    pub source: PreferenceSource,
}

impl OsPreferences {
    pub fn fallback() -> Self {
        Self {
            reduce_motion: PrefState::Unknown,
            reduce_transparency: PrefState::Unknown,
            high_contrast: PrefState::Unknown,
            theme: ThemePreference::Unknown,
            source: PreferenceSource::Fallback,
        }
    }
}

/// Query the OS now. Pure except for the system calls it documents.
pub fn query_preferences() -> OsPreferences {
    let mut prefs = OsPreferences::fallback();
    let mut overridden = false;
    for (slot, var) in [
        (&mut prefs.reduce_motion, "AQUA_PREF_REDUCED_MOTION"),
        (
            &mut prefs.reduce_transparency,
            "AQUA_PREF_REDUCED_TRANSPARENCY",
        ),
        (&mut prefs.high_contrast, "AQUA_PREF_HIGH_CONTRAST"),
    ] {
        if let Some(v) = read_env_pref(var) {
            *slot = v;
            overridden = true;
        }
    }
    if overridden {
        prefs.source = PreferenceSource::EnvOverride;
        return prefs;
    }
    #[cfg(windows)]
    {
        let sys = query_windows();
        prefs.reduce_motion = sys.0;
        prefs.reduce_transparency = sys.1;
        prefs.high_contrast = sys.2;
        prefs.source = PreferenceSource::WindowsSystem;
    }
    prefs
}

fn read_env_pref(var: &str) -> Option<PrefState> {
    match std::env::var(var).ok()?.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Some(PrefState::On),
        "0" | "false" | "no" => Some(PrefState::Off),
        _ => None,
    }
}

/// Whether decorative motion should be suppressed. `Unknown` counts as off
/// (apps must not freeze repayable motion on a guess).
pub fn should_reduce_motion(p: &OsPreferences) -> bool {
    p.reduce_motion.is_on()
}

/// Whether native materials must use their solid fallback: high contrast
/// replaces Mica with the user's solid color, and transparency-off does the
/// same (Mica guidance). Unknown counts as composited.
pub fn prefers_opaque_frame(p: &OsPreferences) -> bool {
    p.high_contrast.is_on() || p.reduce_transparency.is_on()
}

/// Windows system query. Each probe fails independently to `Unknown` —
/// one unreadable value never poisons the others.
#[cfg(windows)]
fn query_windows() -> (PrefState, PrefState, PrefState) {
    (
        windows_reduce_motion(),
        windows_reduce_transparency(),
        windows_high_contrast(),
    )
}

#[cfg(windows)]
fn system_parameters_info(
    action: windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_ACTION,
    param: u32,
    data: *mut core::ffi::c_void,
) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };
    // SAFETY: action/param/data triples below match the documented SPI contract.
    unsafe {
        SystemParametersInfoW(
            action,
            param,
            Some(data),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_ok()
    }
}

/// `SPI_GETHIGHCONTRAST` -> `HIGHCONTRASTW.dwFlags & HCF_HIGHCONTRASTON`.
#[cfg(windows)]
fn windows_high_contrast() -> PrefState {
    use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows::Win32::UI::WindowsAndMessaging::SPI_GETHIGHCONTRAST;
    let mut hc = HIGHCONTRASTW::default();
    hc.cbSize = core::mem::size_of::<HIGHCONTRASTW>() as u32;
    let ok = system_parameters_info(
        SPI_GETHIGHCONTRAST,
        hc.cbSize,
        &mut hc as *mut HIGHCONTRASTW as *mut core::ffi::c_void,
    );
    if !ok {
        return PrefState::Unknown;
    }
    if hc.dwFlags.contains(HCF_HIGHCONTRASTON) {
        PrefState::On
    } else {
        PrefState::Off
    }
}

/// `SPI_GETCLIENTAREAANIMATION` reports whether client-area animations are
/// enabled; reduced motion is its inverse. (The Settings "animation effects"
/// toggle drives this SPI; title-bar/OS-chrome animation is out of scope.)
#[cfg(windows)]
fn windows_reduce_motion() -> PrefState {
    use windows::core::BOOL;
    use windows::Win32::UI::WindowsAndMessaging::SPI_GETCLIENTAREAANIMATION;
    let mut enabled = BOOL::from(false);
    let ok = system_parameters_info(
        SPI_GETCLIENTAREAANIMATION,
        0,
        &mut enabled as *mut BOOL as *mut core::ffi::c_void,
    );
    if !ok {
        return PrefState::Unknown;
    }
    if enabled.as_bool() {
        PrefState::Off
    } else {
        PrefState::On
    }
}

/// `HKCU\...\Personalize\EnableTransparency == 0` means the user turned
/// transparency off (Mica falls back to solid). Missing/unreadable = Unknown.
#[cfg(windows)]
fn windows_reduce_transparency() -> PrefState {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let subkey = HSTRING::from("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let value = HSTRING::from("EnableTransparency");
    let mut data: u32 = 0;
    let mut size = core::mem::size_of::<u32>() as u32;
    // SAFETY: data/size describe a live stack slot; registry only writes.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &subkey,
            &value,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    if status.0 != 0 {
        return PrefState::Unknown;
    }
    if data == 0 {
        PrefState::On
    } else {
        PrefState::Off
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_overrides_win_deterministically() {
        std::env::set_var("AQUA_PREF_REDUCED_MOTION", "1");
        std::env::set_var("AQUA_PREF_HIGH_CONTRAST", "0");
        std::env::remove_var("AQUA_PREF_REDUCED_TRANSPARENCY");
        let p = query_preferences();
        assert_eq!(p.source, PreferenceSource::EnvOverride);
        assert!(should_reduce_motion(&p));
        assert!(!prefers_opaque_frame(&p));
        std::env::remove_var("AQUA_PREF_REDUCED_MOTION");
        std::env::remove_var("AQUA_PREF_HIGH_CONTRAST");
    }

    #[test]
    fn fallback_is_unknown_never_on() {
        let p = OsPreferences::fallback();
        assert!(!should_reduce_motion(&p));
        assert!(!prefers_opaque_frame(&p));
    }

    #[test]
    fn opaque_frame_policy() {
        let mut p = OsPreferences::fallback();
        p.high_contrast = PrefState::On;
        assert!(prefers_opaque_frame(&p));
        p.high_contrast = PrefState::Off;
        p.reduce_transparency = PrefState::On;
        assert!(prefers_opaque_frame(&p));
    }
}
