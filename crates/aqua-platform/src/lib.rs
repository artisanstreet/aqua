pub mod a11y;
#[cfg(windows)]
pub mod a11y_winit;
mod access_tree;
pub mod adapters;
#[cfg(windows)]
mod apartment;
pub mod badge;
pub mod chrome;
#[cfg(windows)]
pub mod chrome_winit;
pub mod dialogs;
pub mod events;
#[cfg(windows)]
pub mod file_drag_windows;
pub mod geometry;
pub mod host;
pub mod ime;
#[cfg(windows)]
pub mod ime_windows;
#[cfg(windows)]
pub mod local_time;
pub mod material;
pub mod monitor;
pub mod notifications;
pub mod prefs;
pub mod services;
pub mod shell;
pub mod window;

pub use aqua_protocol::FrameReason;
pub use geometry::{AquaWindowId, LogicalPosition, LogicalSize, PhysicalSize};
pub use material::{MaterialCapability, WindowMaterial};
pub use window::{
    ObservedWindowState, ThemePreference, Titlebar, WindowEvent, WindowOptions, WindowRequest,
};
