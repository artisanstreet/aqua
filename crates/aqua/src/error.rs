//! Typed errors for the root [`crate::App`] builder pipeline.
//!
//! Every failure out of [`crate::App::run`] is an [`Error`]: authoring
//! [`Config`](crate::Config) resolution, [`UiApp`](crate::UiApp) mounting,
//! low-level initialization, or the event-loop run itself. Display strings
//! are human-readable; `source()` chains the structured cause where one
//! exists. The low-level [`Aqua`](crate::Aqua) compatibility entry points
//! keep their `Result<_, String>` signatures unchanged.

use std::fmt;

use crate::config::ConfigError;
use crate::ui_app::UiAppError;

/// Failure of the root [`crate::App`] pipeline.
#[derive(Debug)]
pub enum Error {
    /// Authoring [`Config`](crate::Config) resolution failed (invalid window
    /// sizes, bounds ordering, non-zero FPS, ...). The setup factory is
    /// never executed in this case.
    Config(ConfigError),
    /// Mounting the setup factory's view failed, or a headless one-frame
    /// build failed. The partially built [`UiApp`](crate::UiApp) is
    /// disposed before this is returned.
    Mount(UiAppError),
    /// Low-level initialization failed after a successful resolve
    /// (runtime construction, or a panicking setup factory — the host is
    /// disposed first and the panic message is preserved).
    Init(String),
    /// The GUI event loop itself failed. The running [`UiApp`](crate::UiApp)
    /// is dropped (which disposes scopes and views) as the loop unwinds.
    Runtime(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Config(e) => write!(f, "invalid app config: {e}"),
            Error::Mount(e) => write!(f, "ui mount failed: {e}"),
            Error::Init(msg) => write!(f, "app init failed: {msg}"),
            Error::Runtime(msg) => write!(f, "app runtime failed: {msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Config(e) => Some(e),
            Error::Mount(e) => Some(e),
            Error::Init(_) | Error::Runtime(_) => None,
        }
    }
}

impl From<ConfigError> for Error {
    fn from(e: ConfigError) -> Self {
        Error::Config(e)
    }
}

impl From<UiAppError> for Error {
    fn from(e: UiAppError) -> Self {
        Error::Mount(e)
    }
}
