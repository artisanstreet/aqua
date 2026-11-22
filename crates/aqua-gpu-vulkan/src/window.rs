pub mod damage;
pub mod pacing;

#[cfg(target_os = "linux")]
mod backend;
#[cfg(target_os = "linux")]
mod swapchain;

#[cfg(target_os = "linux")]
pub use backend::{
    MemoryBreakdown, PhaseTimes, RecoveryNotice, TextureRecovery, WindowBackend, WindowOptions,
    WindowStats,
};
#[cfg(target_os = "linux")]
pub use swapchain::Alpha;
