use aqua_core::ScrollDelta;
use aqua_motion::{MotionKind, SpringParams};

pub const NOTCH_PIXELS: f32 = 77.7;
pub const GLIDE_SECS: f32 = 0.25;
const LINE_UNIT: f32 = 16.0;

#[must_use]
pub fn notch_pixels(delta: ScrollDelta) -> Option<f32> {
    let lines = delta.y / LINE_UNIT;
    let whole = delta.x == 0.0 && lines.is_finite() && lines.abs() >= 1.0 && lines.fract() == 0.0;
    whole.then_some(lines * NOTCH_PIXELS)
}

#[must_use]
pub fn kind() -> MotionKind {
    MotionKind::Spring(SpringParams::from_duration_bounce(GLIDE_SECS, 0.0))
}
