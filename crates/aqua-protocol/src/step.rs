use aqua_core::{InputEvent, Rect, Size};

use crate::{A11yActionRequest, FrameReason, Instant};

#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Tick(Instant),
    Viewport { size: Size, scale: f32 },
    Event(InputEvent),
    Access(A11yActionRequest),
    Frame(Instant),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub redraw: FrameReason,
    pub wake_at: Option<Instant>,
    pub ime_area: Option<Rect>,
}

impl Output {
    pub const IDLE: Output = Output {
        redraw: FrameReason::empty(),
        wake_at: None,
        ime_area: None,
    };
}
