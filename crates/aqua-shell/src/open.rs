use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalTarget(Target);

#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    Url(url::Url),
    Path(PathBuf),
    Reveal(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination<'a> {
    Url(&'a url::Url),
    Path(&'a Path),
    Reveal(&'a Path),
}

#[derive(Debug)]
pub enum OpenError {
    InvalidUrl,
    UnsupportedScheme(String),
    InvalidPath,
    Io(std::io::Error),
    Native(i32),
    Unsupported(&'static str),
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
            Self::Unsupported(reason) => {
                write!(f, "opening external targets is unsupported: {reason}")
            }
        }
    }
}

impl std::error::Error for OpenError {}

impl ExternalTarget {
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

    pub fn destination(&self) -> Destination<'_> {
        match &self.0 {
            Target::Url(url) => Destination::Url(url),
            Target::Path(path) => Destination::Path(path),
            Target::Reveal(path) => Destination::Reveal(path),
        }
    }

    fn canonical(path: &Path) -> Result<PathBuf, OpenError> {
        if path.as_os_str().is_empty() {
            return Err(OpenError::InvalidPath);
        }
        dunce::canonicalize(path).map_err(OpenError::Io)
    }
}
