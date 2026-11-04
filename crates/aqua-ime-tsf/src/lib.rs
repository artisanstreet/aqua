pub mod keystroke;
pub mod model;

#[cfg(all(windows, feature = "double"))]
pub mod double;
#[cfg(windows)]
mod manager;
#[cfg(windows)]
mod session;
#[cfg(windows)]
mod store;

#[cfg(windows)]
pub use keystroke::Keystrokes;
pub use keystroke::{key_phase, KeyPhase};
#[cfg(windows)]
pub use manager::{TsfError, TsfIme};
