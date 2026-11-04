pub mod backdrop;
pub mod dialog_protocol;
pub mod input;
pub mod regions;
pub mod timing;

mod event;

pub use event::{Chrome, Handler, ShellError, ShellEvent, WindowKind, WindowOptions};

#[cfg(windows)]
pub mod clipboard;
#[cfg(windows)]
pub mod dialogs;
#[cfg(windows)]
mod handle;
#[cfg(windows)]
mod looping;
#[cfg(windows)]
mod material;
#[cfg(windows)]
pub mod prefs;
#[cfg(windows)]
mod state;
#[cfg(windows)]
mod window;
#[cfg(windows)]
mod wndproc;

#[cfg(windows)]
pub use handle::NativeHandle;
#[cfg(windows)]
pub use looping::{Waker, now, quit, run, schedule, set_waitables};
#[cfg(windows)]
pub use material::MaterialOutcome;
#[cfg(windows)]
pub use window::{Monitor, Window};
