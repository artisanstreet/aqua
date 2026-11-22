#![deny(unsafe_op_in_unsafe_fn)]

pub use aqua_shell::{picker, regions, timing};
pub use aqua_shell::{
    Chrome, FileDrag, Handler, ShellError, ShellEvent, WindowKind, WindowOptions,
};

pub mod model;

#[cfg(target_os = "macos")]
pub mod capability;
#[cfg(target_os = "macos")]
pub mod dialogs;
#[cfg(target_os = "macos")]
pub mod dock;
#[cfg(target_os = "macos")]
pub mod icon;
#[cfg(target_os = "macos")]
pub mod notifications;
#[cfg(target_os = "macos")]
pub mod open;
#[cfg(target_os = "macos")]
pub mod prefs;
#[cfg(target_os = "macos")]
pub mod pressure;
#[cfg(target_os = "macos")]
pub mod services;
#[cfg(all(target_os = "macos", feature = "testing"))]
pub mod testing;
#[cfg(target_os = "macos")]
pub mod wake_trace;

#[cfg(target_os = "macos")]
mod access;
#[cfg(target_os = "macos")]
mod app;
#[cfg(target_os = "macos")]
mod chrome;
#[cfg(target_os = "macos")]
mod clipboard;
#[cfg(target_os = "macos")]
mod clock;
#[cfg(target_os = "macos")]
mod cursor;
#[cfg(target_os = "macos")]
mod drop;
#[cfg(target_os = "macos")]
mod input;
#[cfg(target_os = "macos")]
mod lease;
#[cfg(target_os = "macos")]
mod looping;
#[cfg(target_os = "macos")]
mod output;
#[cfg(target_os = "macos")]
mod state;
#[cfg(target_os = "macos")]
mod text_input;
#[cfg(target_os = "macos")]
mod unsupported;
#[cfg(target_os = "macos")]
mod view;
#[cfg(target_os = "macos")]
mod window;
#[cfg(target_os = "macos")]
mod window_class;

#[cfg(target_os = "macos")]
pub use access::AccessHooks;
#[cfg(target_os = "macos")]
pub use clock::{from_media_time, now, since_process_start};
#[cfg(target_os = "macos")]
pub use lease::{LayerGeometry, LayerLease};
#[cfg(target_os = "macos")]
pub use looping::{pump_sent_messages, quit, run, schedule, Waker};
#[cfg(target_os = "macos")]
pub use output::Monitor;
#[cfg(target_os = "macos")]
pub use text_input::{Insertion, ScreenRect, TextInputClient, Utf16Range};
#[cfg(target_os = "macos")]
pub use unsupported::Unsupported;
#[cfg(target_os = "macos")]
pub use window::Window;
