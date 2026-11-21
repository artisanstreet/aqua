mod formats;
mod frames;
mod hint;
#[path = "../../aqua-shaders/msl/map.rs"]
#[allow(dead_code)]
mod map;
mod offscreen;
mod persistent;
mod pipeline;
mod renderer;
mod transients;

use aqua_graph::TextureHandle;

pub use crate::hal::selection::AdapterPreference;
pub use crate::hal::DeviceOptions;
pub use formats::SurfaceFormat;
pub use offscreen::{OffscreenBackend, OffscreenOptions, HEAP_ALIGNMENT};
pub use persistent::PersistentStats;
pub use pipeline::{Kind as PipelineKind, PipelineStats};
pub use renderer::{SceneOptions, SceneStats};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Device(#[from] crate::hal::Error),
    #[error("unsupported by this backend: {0}")]
    Unsupported(&'static str),
    #[error("texture {} is not resident", .0.0)]
    UnknownTexture(TextureHandle),
    #[error("persistent texture: {0}")]
    Texture(&'static str),
    #[error("transient textures need {needed} bytes, above the {ceiling} byte ceiling")]
    Budget { needed: u64, ceiling: u64 },
}

impl From<Error> for aqua_protocol::GpuError {
    fn from(error: Error) -> Self {
        match error {
            Error::Unsupported(what) => aqua_protocol::GpuError::Unsupported(what),
            Error::UnknownTexture(handle) => aqua_protocol::GpuError::UnknownTexture(handle),
            other => aqua_protocol::GpuError::DeviceFailure(other.to_string()),
        }
    }
}
