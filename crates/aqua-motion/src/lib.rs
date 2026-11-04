#![forbid(unsafe_code)]

pub mod app;
pub mod command;
mod curve;
pub mod render;
pub mod track;

pub use app::AppMotion;
pub use command::{Binding, Edge, MotionCommand, MotionEvent, Property, TrackId};
pub use curve::{
    Completion, Easing, MotionError, MotionKind, MotionValue, ReducedMotion,
    SPRING_MAX_DURATION_SECS, Seconds, SpringParams, SpringSegment,
};
pub use render::{Presented, Sampled, TrackSet};
pub use track::Track;
