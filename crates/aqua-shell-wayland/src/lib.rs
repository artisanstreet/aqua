#![forbid(unsafe_code)]

pub use aqua_shell::{picker, regions, timing};
pub use aqua_shell::{
    Chrome, FileDrag, Handler, ShellError, ShellEvent, WindowKind, WindowOptions,
};

#[cfg(target_os = "linux")]
pub mod capability;
#[cfg(target_os = "linux")]
pub mod services;
#[cfg(all(target_os = "linux", feature = "testing"))]
pub mod testing;
#[cfg(target_os = "linux")]
pub mod wake_trace;

#[cfg(target_os = "linux")]
mod chrome;
#[cfg(target_os = "linux")]
mod clipboard;
#[cfg(target_os = "linux")]
mod clock;
#[cfg(target_os = "linux")]
mod cursor;
#[cfg(target_os = "linux")]
mod data_device;
#[cfg(target_os = "linux")]
mod decoration;
#[cfg(target_os = "linux")]
mod drop;
#[cfg(target_os = "linux")]
mod formats;
#[cfg(target_os = "linux")]
mod globals;
#[cfg(target_os = "linux")]
mod input;
#[cfg(target_os = "linux")]
mod lease;
#[cfg(target_os = "linux")]
mod looping;
#[cfg(target_os = "linux")]
mod occlusion;
#[cfg(target_os = "linux")]
mod output;
#[cfg(target_os = "linux")]
mod scale;
#[cfg(target_os = "linux")]
mod shm;
#[cfg(target_os = "linux")]
mod state;
#[cfg(target_os = "linux")]
mod toplevel;
#[cfg(target_os = "linux")]
mod unsupported;
#[cfg(target_os = "linux")]
mod window;

#[cfg(target_os = "linux")]
pub use clock::from_monotonic;
#[cfg(target_os = "linux")]
pub use lease::{FrameWait, Presented, SurfaceGeometry, SurfaceLease};
#[cfg(target_os = "linux")]
pub use looping::{now, pump_sent_messages, quit, run, schedule, set_waitables, Waker};
#[cfg(target_os = "linux")]
pub use output::Monitor;
#[cfg(target_os = "linux")]
pub use scale::Scaling;
#[cfg(target_os = "linux")]
pub use unsupported::Unsupported;
#[cfg(target_os = "linux")]
pub use window::Window;
