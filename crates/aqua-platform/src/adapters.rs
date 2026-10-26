//! Adapter selection: Windows DWM path vs portable fallback.
//!
//! Target-selected modules. Shared code calls [`apply_material`] and
//! [`backend_name`]; per-OS reality lives behind them.

#[cfg(windows)]
#[path = "windows.rs"]
pub mod windows;
#[cfg(windows)]
pub use windows::{apply_material, backend_name};

#[cfg(not(windows))]
#[path = "fallback.rs"]
mod fallback;
#[cfg(not(windows))]
pub use fallback::{apply_material, backend_name};
