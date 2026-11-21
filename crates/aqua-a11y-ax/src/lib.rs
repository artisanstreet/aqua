#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(target_os = "macos")]
mod adapter;
#[cfg(all(target_os = "macos", feature = "testing"))]
pub mod testing;

pub use aqua_a11y::{Connection, WakeFn};

#[cfg(target_os = "macos")]
pub use adapter::AxAdapter;
