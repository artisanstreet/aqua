pub use aqua_protocol::prefs::{
    prefers_opaque_frame, should_reduce_motion, PrefState, PreferenceSource,
};

pub type OsPreferences = aqua_protocol::prefs::SystemPrefs;

pub fn query_preferences() -> OsPreferences {
    let overridden = OsPreferences::fallback().with_overrides(|name| std::env::var(name).ok());
    match (overridden.source, cfg!(windows)) {
        (PreferenceSource::EnvOverride, _) | (_, false) => overridden,
        #[cfg(windows)]
        (_, true) => {
            let (reduce_motion, reduce_transparency, high_contrast) = query_windows();
            OsPreferences {
                reduce_motion,
                reduce_transparency,
                high_contrast,
                source: PreferenceSource::WindowsSystem,
                ..overridden
            }
        }
        #[cfg(not(windows))]
        (_, true) => overridden,
    }
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
    let mut hc = HIGHCONTRASTW {
        cbSize: core::mem::size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
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
