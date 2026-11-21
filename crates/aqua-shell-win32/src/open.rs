use aqua_shell::open::ExternalTarget;
use std::path::Path;

pub use aqua_shell::open::OpenError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenTarget(ExternalTarget);

impl OpenTarget {
    pub fn url(input: &str) -> Result<Self, OpenError> {
        ExternalTarget::url(input).map(Self)
    }

    pub fn path(path: &Path) -> Result<Self, OpenError> {
        ExternalTarget::path(path).map(Self)
    }

    pub fn reveal(path: &Path) -> Result<Self, OpenError> {
        ExternalTarget::reveal(path).map(Self)
    }

    #[cfg(windows)]
    pub fn execute(&self) -> Result<(), OpenError> {
        execute(&self.0)
    }
}

#[cfg(windows)]
fn execute(target: &ExternalTarget) -> Result<(), OpenError> {
    use aqua_shell::open::Destination;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{w, PCWSTR};
    use windows::Win32::UI::Shell::{
        SHOpenFolderAndSelectItems, SHParseDisplayName, ShellExecuteW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let _apartment =
        crate::apartment::Apartment::new().map_err(|error| OpenError::Native(error.code().0))?;
    let destination = target.destination();
    let value = match destination {
        Destination::Url(url) => std::ffi::OsStr::new(url.as_str()),
        Destination::Path(path) | Destination::Reveal(path) => path.as_os_str(),
    };
    let mut wide: Vec<u16> = value.encode_wide().collect();
    if wide.contains(&0) {
        return Err(OpenError::InvalidPath);
    }
    wide.push(0);
    if matches!(destination, Destination::Reveal(_)) {
        let mut item = std::ptr::null_mut();
        unsafe {
            SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut item, 0, None)
                .map_err(|error| OpenError::Native(error.code().0))?;
            let result = SHOpenFolderAndSelectItems(item, None, 0);
            windows::Win32::System::Com::CoTaskMemFree(Some(item.cast()));
            result.map_err(|error| OpenError::Native(error.code().0))
        }
    } else {
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(wide.as_ptr()),
                None,
                None,
                SW_SHOWNORMAL,
            )
        };
        let code = result.0 as isize;
        if code > 32 {
            Ok(())
        } else {
            Err(OpenError::Native(code as i32))
        }
    }
}
