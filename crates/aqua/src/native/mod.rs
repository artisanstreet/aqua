#[cfg(windows)]
mod gpu;
#[cfg(windows)]
mod hook;
#[cfg(windows)]
mod inspector;
#[cfg(windows)]
mod looping;
#[cfg(windows)]
mod priority;
#[cfg(windows)]
mod wakes;
#[cfg(windows)]
mod window;

#[cfg(windows)]
pub use looping::run;
#[cfg(windows)]
pub use priority::{opt_out_of_throttling, raise_thread_priority};

#[cfg(not(windows))]
pub fn opt_out_of_throttling() {}

#[cfg(not(windows))]
pub fn raise_thread_priority() {}

#[cfg(windows)]
pub fn prewarm_gpu() {
    gpu::prewarm();
}

#[cfg(windows)]
pub fn startup_mark(label: &str) {
    gpu::startup_mark(label);
}

#[cfg(not(windows))]
pub fn startup_mark(_label: &str) {}

#[cfg(not(windows))]
pub fn prewarm_gpu() {}

#[cfg(not(windows))]
pub fn run(
    _aqua: crate::runtime::Aqua,
    _app: impl crate::runtime::Application + 'static,
) -> Result<(), String> {
    Err("the native host runs on Windows only".into())
}
