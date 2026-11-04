#![forbid(unsafe_code)]

pub mod emitter;
pub mod glide;
pub mod render;
mod runtime;
pub mod scheduler;

pub use emitter::emit_scene_diff;
pub use render::RenderSide;
pub use runtime::{
    FrameWork, GpuState, InputDispatchOutcome, NativeTextService, PopupError, PopupFocus,
    PopupOptions, Runtime, RuntimeError, TextFrontier,
};
pub use scheduler::{FrameStats, Scheduler};
