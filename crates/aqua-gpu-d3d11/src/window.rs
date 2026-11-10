pub mod damage;
pub mod mirror;
pub mod pacing;

#[cfg(windows)]
mod backend;
#[cfg(windows)]
pub mod timing;
#[cfg(windows)]
mod vblank;

#[cfg(windows)]
pub use backend::{
    MemoryBreakdown, PhaseTimes, RecoveryNotice, SurfaceKind, TextureRecovery, WindowBackend,
    WindowOptions, WindowStats,
};
