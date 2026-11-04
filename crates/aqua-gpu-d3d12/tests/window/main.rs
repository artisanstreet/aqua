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
mod recovery_textures;
#[cfg(windows)]
mod refresh;
#[cfg(windows)]
mod scissor;
#[cfg(windows)]
mod shared_device;
#[cfg(windows)]
#[path = "../hal/window.rs"]
mod test_window;
