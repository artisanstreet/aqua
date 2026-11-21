pub mod pacing;

#[cfg(target_os = "macos")]
mod backend;
#[cfg(target_os = "macos")]
mod layer;

#[cfg(target_os = "macos")]
pub use backend::{
    LayerTarget, MemoryBreakdown, PhaseTimes, RecoveryNotice, TextureRecovery, WindowBackend,
    WindowOptions, WindowStats,
};
#[cfg(target_os = "macos")]
pub use layer::Alpha;
