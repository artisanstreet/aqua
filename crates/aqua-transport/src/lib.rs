#![forbid(unsafe_code)]

pub mod ring;
pub mod thread;

pub use ring::{Notify, Receiver, RecvError, SendError, Sender, channel, channel_with_notify};
pub use thread::{RenderHost, RenderThread, spawn_render_thread};
