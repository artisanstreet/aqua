#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
mod adapter;

pub use aqua_a11y::{Connection, WakeFn};

#[cfg(target_os = "linux")]
pub use adapter::AtspiAdapter;
