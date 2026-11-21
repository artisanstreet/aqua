#![forbid(unsafe_code)]

pub mod model;

#[cfg(target_os = "macos")]
mod backend;

#[cfg(target_os = "macos")]
pub use backend::{AppKitIme, AppKitImeError};
