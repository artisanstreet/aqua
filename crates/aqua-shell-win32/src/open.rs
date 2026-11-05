use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenTarget(Target);

#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    Url(url::Url),
    Path(PathBuf),
    Reveal(PathBuf),
}

#[derive(Debug)]
pub enum OpenError {
    InvalidUrl,
    UnsupportedScheme(String),
    InvalidPath,
    Io(std::io::Error),
    Native(i32),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUrl => f.write_str("invalid external URL"),
            Self::UnsupportedScheme(scheme) => {
                write!(f, "unsupported external URL scheme: {scheme}")
            }
            Self::InvalidPath => f.write_str("invalid external path"),
            Self::Io(error) => error.fmt(f),
            Self::Native(code) => write!(f, "native shell failed: {code}"),
        }
    }
}

impl std::error::Error for OpenError {}

impl OpenTarget {
    pub fn url(input: &str) -> Result<Self, OpenError> {
        if input.chars().any(char::is_control) {
            return Err(OpenError::InvalidUrl);
        }
        let url = url::Url::parse(input).map_err(|_| OpenError::InvalidUrl)?;
        match url.scheme() {
            "http" | "https" if url.host_str().is_some() => Ok(Self(Target::Url(url))),
            "mailto" if !url.path().is_empty() => Ok(Self(Target::Url(url))),
            "http" | "https" | "mailto" => Err(OpenError::InvalidUrl),
            scheme => Err(OpenError::UnsupportedScheme(scheme.to_owned())),
        }
    }

    pub fn path(path: &Path) -> Result<Self, OpenError> {
        Self::canonical(path).map(|path| Self(Target::Path(path)))
    }

    pub fn reveal(path: &Path) -> Result<Self, OpenError> {
        Self::canonical(path).map(|path| Self(Target::Reveal(path)))
    }

    fn canonical(path: &Path) -> Result<PathBuf, OpenError> {
        if path.as_os_str().is_empty() {
            return Err(OpenError::InvalidPath);
        }
        dunce::canonicalize(path).map_err(OpenError::Io)
    }

    #[cfg(windows)]
    pub fn execute(&self) -> Result<(), OpenError> {
        execute(self)
    }
}

#[cfg(windows)]
fn execute(target: &OpenTarget) -> Result<(), OpenError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{
        SHOpenFolderAndSelectItems, SHParseDisplayName, ShellExecuteW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{PCWSTR, w};

    let _apartment =
        crate::apartment::Apartment::new().map_err(|error| OpenError::Native(error.code().0))?;
    let value = match &target.0 {
        Target::Url(url) => std::ffi::OsStr::new(url.as_str()),
        Target::Path(path) | Target::Reveal(path) => path.as_os_str(),
    };
    let mut wide: Vec<u16> = value.encode_wide().collect();
    if wide.contains(&0) {
        return Err(OpenError::InvalidPath);
    }
    wide.push(0);
    if matches!(&target.0, Target::Reveal(_)) {
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
