#![forbid(unsafe_code)]

pub mod app;
pub mod command;
mod curve;
pub mod cycle;
pub mod render;
pub mod track;

pub use app::AppMotion;
pub use command::{Binding, Edge, MotionCommand, MotionEvent, Property, TrackId};
pub use curve::{
    Completion, Easing, MotionError, MotionKind, MotionValue, ReducedMotion, Seconds, SpringParams,
    SpringSegment, SPRING_MAX_DURATION_SECS,
};
pub use cycle::{Cycle, CycleSet};
pub use render::{Presented, Sampled, TrackSet};
pub use track::Track;
