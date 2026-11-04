pub mod damage;

#[cfg(windows)]
mod backend;

#[cfg(windows)]
pub use backend::{RecoveryNotice, WindowBackend, WindowOptions, WindowStats};
