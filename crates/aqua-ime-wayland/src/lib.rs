#![forbid(unsafe_code)]

pub mod model;

#[cfg(target_os = "linux")]
mod backend;

#[cfg(target_os = "linux")]
pub use backend::{WaylandIme, WaylandImeError};
