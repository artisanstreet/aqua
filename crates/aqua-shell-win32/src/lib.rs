pub mod capability;
pub mod dialog_protocol;
pub mod hooks;
pub mod input;
pub mod notifications;
pub mod occlusion;
pub mod open;
pub mod picker;
pub mod pointer;
pub mod regions;
pub mod services;
pub mod timing;
pub mod wake_trace;

mod event;

pub use event::{Chrome, FileDrag, Handler, ShellError, ShellEvent, WindowKind, WindowOptions};
pub use hooks::{MessageHook, RawMessage, set_message_hook};

#[cfg(windows)]
mod apartment;
#[cfg(windows)]
pub mod clipboard;
#[cfg(windows)]
mod composition;
#[cfg(windows)]
pub mod dialogs;
#[cfg(windows)]
mod drop_target;
#[cfg(windows)]
mod gaps;
#[cfg(windows)]
mod handle;
#[cfg(windows)]
pub mod local_time;
#[cfg(windows)]
mod looping;
#[cfg(windows)]
mod material;
#[cfg(windows)]
pub mod prefs;
#[cfg(windows)]
mod state;
#[cfg(windows)]
pub mod taskbar;
#[cfg(windows)]
mod window;
#[cfg(windows)]
mod wndproc;

#[cfg(windows)]
pub use handle::NativeHandle;
#[cfg(windows)]
pub use looping::{Waker, now, pump_sent_messages, quit, run, schedule, set_waitables};
#[cfg(windows)]
pub use material::MaterialOutcome;
#[cfg(windows)]
pub use window::{Monitor, Window};
