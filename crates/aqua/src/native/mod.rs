pub(crate) mod platform;

pub use platform::shell::capability::{material_capability, query_preferences};

#[cfg(target_os = "linux")]
mod attach_wayland;
#[cfg(windows)]
mod attach_win32;
#[cfg(any(windows, target_os = "linux"))]
pub(crate) mod gpu;
#[cfg(windows)]
mod hook;
#[cfg(any(windows, target_os = "linux"))]
mod inspector;
#[cfg(any(windows, target_os = "linux"))]
mod looping;
#[cfg(target_os = "linux")]
mod pacer;
#[cfg(windows)]
mod priority;
#[cfg(any(windows, target_os = "linux"))]
mod wakes;
#[cfg(windows)]
mod window;
#[cfg(target_os = "linux")]
mod window_wayland;

#[cfg(any(windows, target_os = "linux"))]
pub use looping::run;
#[cfg(windows)]
pub use priority::{opt_out_of_throttling, raise_thread_priority};

#[cfg(not(windows))]
pub fn opt_out_of_throttling() {}

#[cfg(not(windows))]
pub fn raise_thread_priority() {}

#[cfg(any(windows, target_os = "linux"))]
pub fn prewarm_gpu() {
    gpu::prewarm();
}

#[cfg(any(windows, target_os = "linux"))]
pub fn startup_mark(label: &str) {
    gpu::startup_mark(label);
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn startup_mark(_label: &str) {}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn prewarm_gpu() {}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn run(
    _aqua: crate::runtime::Aqua,
    _app: impl crate::runtime::Application + 'static,
) -> Result<(), String> {
    Err("the native host runs on Windows and Linux only".into())
}
