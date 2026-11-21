mod client;
mod compositor;
mod ipc;
mod keyboard;
mod pointer;
mod screencopy;

pub use compositor::{Compositor, Options, Rect};
pub use ipc::IpcError;
pub use keyboard::VirtualKeyboard;
pub use pointer::VirtualPointer;
pub use screencopy::{capture, Capture, DamageWatch};

#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("starting {binary}: {source}")]
    Spawn {
        binary: String,
        source: std::io::Error,
    },
    #[error("{binary} exited before its sockets appeared; log:\n{log}")]
    Exited { binary: String, log: String },
    #[error("{binary} created no sockets within {waited:?}; log:\n{log}")]
    NoSockets {
        binary: String,
        waited: std::time::Duration,
        log: String,
    },
    #[error("preparing the runtime directory {path}: {source}")]
    RuntimeDir {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("test client {client}: {reason}")]
    Client {
        client: &'static str,
        reason: String,
    },
}
