mod damage;
mod mirror;
mod pacing;

#[cfg(windows)]
mod backend;
#[cfg(windows)]
mod composition;
#[cfg(windows)]
mod cross_adapter;
#[cfg(windows)]
mod idle;
#[cfg(windows)]
mod memory;
#[cfg(windows)]
mod recovery_textures;
#[cfg(windows)]
mod refresh;
#[cfg(windows)]
mod resize;
#[cfg(windows)]
mod scissor;
#[cfg(windows)]
mod shared_device;
#[cfg(windows)]
mod startup;
#[cfg(windows)]
#[path = "../support/mod.rs"]
mod support;
#[cfg(windows)]
#[path = "../hal/window.rs"]
mod test_window;
