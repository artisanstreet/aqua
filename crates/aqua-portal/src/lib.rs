#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
mod bus;
#[cfg(target_os = "linux")]
pub mod notifications;
#[cfg(target_os = "linux")]
pub mod open;
#[cfg(target_os = "linux")]
pub mod prefs;
#[cfg(target_os = "linux")]
pub mod taskbar;
#[cfg(all(target_os = "linux", feature = "testing"))]
pub mod testing;

#[cfg(target_os = "linux")]
pub use bus::{Bus, Method, PortalError, Wake};
