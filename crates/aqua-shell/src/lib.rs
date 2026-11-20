#![forbid(unsafe_code)]

pub mod fallback;
pub mod icon;
pub mod notifications;
pub mod open;
pub mod picker;
pub mod regions;
pub mod services;
pub mod timing;

mod app_id;
mod event;

pub use app_id::{AppId, AppIdError};
pub use event::{Chrome, FileDrag, Handler, ShellError, ShellEvent, WindowKind, WindowOptions};
