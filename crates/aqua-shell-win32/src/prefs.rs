use aqua_protocol::ColorScheme;
use aqua_protocol::prefs::{PrefState, PreferenceSource, SystemPrefs};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETCLIENTAREAANIMATION, SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_ACTION,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};
use windows::core::{BOOL, PCWSTR};

pub use aqua_protocol::prefs::{prefers_opaque_frame, should_reduce_motion};

const PERSONALIZE: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";

fn system_parameter(
    action: SYSTEM_PARAMETERS_INFO_ACTION,
    param: u32,
    data: *mut core::ffi::c_void,
) -> bool {
    unsafe {
        SystemParametersInfoW(
            action,
            param,
            Some(data),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_ok()
}

fn high_contrast() -> PrefState {
    let mut info = HIGHCONTRASTW {
        cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    let read = system_parameter(
        SPI_GETHIGHCONTRAST,
        info.cbSize,
        (&mut info as *mut HIGHCONTRASTW).cast(),
    );
    match (read, info.dwFlags.contains(HCF_HIGHCONTRASTON)) {
        (false, _) => PrefState::Unknown,
        (true, true) => PrefState::On,
        (true, false) => PrefState::Off,
    }
}

fn reduce_motion() -> PrefState {
    let mut animated = BOOL::from(false);
    let read = system_parameter(
        SPI_GETCLIENTAREAANIMATION,
        0,
        (&mut animated as *mut BOOL).cast(),
    );
    match (read, animated.as_bool()) {
        (false, _) => PrefState::Unknown,
        (true, true) => PrefState::Off,
        (true, false) => PrefState::On,
    }
}

fn reduce_transparency() -> PrefState {
    match dword(HKEY_CURRENT_USER, PERSONALIZE, "EnableTransparency") {
        None => PrefState::Unknown,
        Some(0) => PrefState::On,
        Some(_) => PrefState::Off,
    }
}

fn scheme() -> ColorScheme {
    match apps_use_dark_theme() {
        Some(true) => ColorScheme::Dark,
        Some(false) => ColorScheme::Light,
        None => ColorScheme::Unknown,
    }
}

pub fn query() -> SystemPrefs {
    let overridden = SystemPrefs::fallback().with_overrides(|name| std::env::var(name).ok());
    match overridden.source {
        PreferenceSource::EnvOverride => SystemPrefs {
            theme: scheme(),
            ..overridden
        },
        _ => SystemPrefs {
            reduce_motion: reduce_motion(),
            reduce_transparency: reduce_transparency(),
            high_contrast: high_contrast(),
            theme: scheme(),
            source: PreferenceSource::WindowsSystem,
        },
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn dword(root: HKEY, subkey: &str, value: &str) -> Option<u32> {
    let subkey = wide(subkey);
    let value = wide(value);
    let mut data = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut data as *mut u32).cast()),
            Some(&mut size),
        )
    };
    (status.0 == 0).then_some(data)
}

pub fn apps_use_dark_theme() -> Option<bool> {
    dword(HKEY_CURRENT_USER, PERSONALIZE, "AppsUseLightTheme").map(|light| light == 0)
}

pub fn build_number() -> Option<u32> {
    let subkey = wide("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
    let value = wide("CurrentBuildNumber");
    let mut buffer = [0u16; 32];
    let mut size = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    (status.0 == 0)
        .then(|| {
            let len = (size as usize / 2).min(buffer.len());
            String::from_utf16_lossy(&buffer[..len])
                .trim_matches('\0')
                .trim()
                .parse()
                .ok()
        })
        .flatten()
}
