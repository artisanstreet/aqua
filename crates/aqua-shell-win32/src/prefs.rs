use aqua_platform::prefs::query_preferences;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::core::PCWSTR;

pub use aqua_platform::prefs::{OsPreferences, PrefState, should_reduce_motion};

pub fn query() -> OsPreferences {
    query_preferences()
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
    dword(
        HKEY_CURRENT_USER,
        "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
        "AppsUseLightTheme",
    )
    .map(|light| light == 0)
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
