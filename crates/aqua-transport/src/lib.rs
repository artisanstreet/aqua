#![forbid(unsafe_code)]

pub mod gpu_host;
pub mod inspector;
pub mod lower;
pub mod ring;
pub mod thread;
pub mod trim;

pub use gpu_host::{
    Clock, GpuRenderHost, HostConfig, HostCounters, MonotonicClock, SharedClock, SurfaceKind,
    TrimHandle,
};
pub use inspector::{
    stats_json, tree_json, FrameRecord, Inspector, InspectorError, InspectorServer,
    InspectorSnapshot,
};
pub use lower::{lower_items, GeometryLowering, LowerContext, Lowered, Lowering, TrimRequest};
pub use ring::{channel, channel_with_notify, Notify, Receiver, RecvError, SendError, Sender};
pub use thread::{spawn_render_thread, spawn_render_thread_with, RenderHost, RenderThread};
pub use trim::{TrimAction, TrimInput, TrimPolicy, IDLE_TRIM_AFTER};
