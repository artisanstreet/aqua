pub mod activation;

pub use aqua_shell::notifications::{
    native_tag, AppIdentity, Notification, NotificationAction, NotificationError,
    NotificationResponse, ResponseHandler,
};

#[cfg(windows)]
mod native;

#[cfg(windows)]
pub use native::NotificationCenter;
