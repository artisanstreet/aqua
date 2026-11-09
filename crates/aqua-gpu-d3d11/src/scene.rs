mod compile;
mod geometry;
mod custom;
pub(crate) mod debug;
mod formats;
mod hint;
mod offscreen;
mod persistent;
mod pipeline;
mod renderer;
mod timestamps;
mod transients;

pub use crate::hal::selection::AdapterPreference;
pub use offscreen::{read_texture, OffscreenBackend, SurfaceFormat, HEAP_ALIGNMENT};
pub use persistent::{PersistentStats, PersistentTextures};
pub use pipeline::{Kind as PipelineKind, PipelineStats};
pub use renderer::{SceneOptions, SceneRenderer, SceneStats, SceneTarget};
pub use timestamps::PassTime;

#[derive(Debug)]
pub enum Error {
    Windows(windows::core::Error),
    NoAdapter,
    MissingShader(&'static str),
    Shader(String),
    Unsupported(&'static str),
    DeviceRemoved(String),
    Texture(&'static str),
    Budget { needed: u64, ceiling: u64 },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Windows(error) => write!(f, "windows error: {error}"),
            Error::NoAdapter => write!(f, "no usable D3D11 adapter"),
            Error::MissingShader(name) => write!(f, "shader blob {name} is missing"),
            Error::Shader(message) => write!(f, "shader compilation: {message}"),
            Error::Unsupported(what) => write!(f, "unsupported: {what}"),
            Error::DeviceRemoved(reason) => write!(f, "device removed: {reason}"),
            Error::Texture(message) => write!(f, "persistent texture: {message}"),
            Error::Budget { needed, ceiling } => write!(
                f,
                "transient textures need {needed} bytes, above the {ceiling} byte ceiling"
            ),
        }
    }
}

impl std::error::Error for Error {}

impl From<windows::core::Error> for Error {
    fn from(error: windows::core::Error) -> Self {
        Error::Windows(error)
    }
}

impl From<crate::hal::Error> for Error {
    fn from(error: crate::hal::Error) -> Self {
        match error {
            crate::hal::Error::Windows(error) => Error::Windows(error),
            crate::hal::Error::NoAdapter => Error::NoAdapter,
            lost @ crate::hal::Error::DeviceLost { .. } => Error::DeviceRemoved(lost.to_string()),
        }
    }
}

impl From<Error> for aqua_protocol::GpuError {
    fn from(error: Error) -> Self {
        match error {
            Error::Unsupported(what) => aqua_protocol::GpuError::Unsupported(what),
            other => aqua_protocol::GpuError::DeviceFailure(other.to_string()),
        }
    }
}
