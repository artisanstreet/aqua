mod custom;
pub mod dxc;
mod formats;
mod frames;
mod hint;
#[path = "../../aqua-shaders/msl/map.rs"]
mod map;
mod offscreen;
mod persistent;
mod pipeline;
mod renderer;
pub mod spirv_cross;
mod transients;

use aqua_graph::TextureHandle;

pub use crate::hal::selection::AdapterPreference;
pub use crate::hal::DeviceOptions;
pub use custom::compiler::{
    Attempt, LibrarySource, LoadError, RuntimeCompiler, ShaderCompiler, Toolchain,
};
pub use formats::SurfaceFormat;
pub(crate) use frames::Frames;
pub(crate) use offscreen::capabilities;
pub use offscreen::{OffscreenBackend, OffscreenOptions, HEAP_ALIGNMENT};
pub use persistent::PersistentStats;
pub(crate) use persistent::{staging_bytes, PersistentTextures};
pub use pipeline::{Kind as PipelineKind, PipelineStats};
pub(crate) use renderer::SceneRenderer;
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
    #[error("shader compilation: {0}")]
    Shader(#[from] dxc::CompileError),
    #[error("shader translation: {0}")]
    Translation(#[from] spirv_cross::TranslateError),
    #[error("shader compilation: Metal rejected the MSL for {entry}: {reason}")]
    Msl { entry: String, reason: String },
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
