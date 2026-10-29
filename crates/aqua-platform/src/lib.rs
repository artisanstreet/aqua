pub mod a11y;
#[cfg(windows)]
pub mod a11y_winit;
pub mod adapters;
pub mod chrome;
#[cfg(windows)]
pub mod chrome_winit;
pub mod dialogs;
pub mod events;
pub mod geometry;
pub mod host;
pub mod material;
pub mod prefs;
pub mod services;
pub mod window;

pub use aqua_protocol::FrameReason;
pub use geometry::{AquaWindowId, LogicalPosition, LogicalSize, PhysicalSize};
pub use material::{MaterialCapability, WindowMaterial};
pub use window::{
    ObservedWindowState, ThemePreference, Titlebar, WindowEvent, WindowOptions, WindowRequest,
};
