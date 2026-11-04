mod barriers;
mod bindless;
mod device;
mod formats;
mod offscreen;
mod persistent;
mod pipeline;
mod renderer;
mod sprites;
mod tiers;
mod timestamps;
mod transients;
pub(crate) mod util;

pub use device::AdapterPreference;
pub use offscreen::{OffscreenBackend, SurfaceFormat};
pub use persistent::{PersistentStats, PersistentTextures};
pub use pipeline::PipelineStats;
pub use renderer::{PersistentView, SceneRenderer, SceneStats, SceneTarget};
pub use tiers::{SceneOptions, TierLimit};

#[derive(Debug)]
pub enum Error {
    Windows(windows::core::Error),
    NoAdapter,
    MissingShader(&'static str),
    Unsupported(&'static str),
    RootSignature(String),
    DeviceRemoved(String),
    Texture(&'static str),
    Budget { needed: u64, ceiling: u64 },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Windows(error) => write!(f, "windows error: {error}"),
            Error::NoAdapter => write!(f, "no usable D3D12 adapter"),
            Error::MissingShader(name) => write!(f, "shader blob {name} is missing"),
            Error::Unsupported(what) => write!(f, "unsupported: {what}"),
            Error::RootSignature(message) => write!(f, "root signature: {message}"),
            Error::DeviceRemoved(reason) => write!(f, "device removed: {reason}"),
            Error::Texture(message) => write!(f, "persistent texture: {message}"),
            Error::Budget { needed, ceiling } => write!(
                f,
                "transient heap needs {needed} bytes, above the {ceiling} byte ceiling"
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

impl From<Error> for aqua_protocol::GpuError {
    fn from(error: Error) -> Self {
        match error {
            Error::Unsupported(what) => aqua_protocol::GpuError::Unsupported(what),
            other => aqua_protocol::GpuError::DeviceFailure(other.to_string()),
        }
    }
}
