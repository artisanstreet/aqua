pub mod budget;
pub mod selection;

#[cfg(target_os = "macos")]
mod device;
#[cfg(target_os = "macos")]
mod error;
#[cfg(target_os = "macos")]
mod resource;

#[cfg(target_os = "macos")]
pub(crate) use device::{describe, InFlight};
#[cfg(target_os = "macos")]
pub use device::{Device, DeviceInfo, DeviceOptions, Families};
#[cfg(target_os = "macos")]
pub use error::Error;
#[cfg(target_os = "macos")]
pub use resource::{Buffer, Texture, TextureDesc, View};
