use aqua_core::{NodeId, ScrollDelta};

pub const NOTCH_PIXELS: f32 = 77.7;
const LINE_UNIT: f32 = 16.0;
const TIME_CONSTANT: f32 = 0.075;
const SETTLE_DISTANCE: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glide {
    pub node: NodeId,
    pub remaining: f32,
    pub advanced_at: f64,
}

#[must_use]
pub fn notch_pixels(delta: ScrollDelta) -> Option<f32> {
    let lines = delta.y / LINE_UNIT;
    let whole = delta.x == 0.0 && lines.is_finite() && lines.abs() >= 1.0 && lines.fract() == 0.0;
    whole.then_some(lines * NOTCH_PIXELS)
}

#[must_use]
pub fn push(
    current: Option<Glide>,
    node: NodeId,
    delta: f32,
    offset: f32,
    max: f32,
    now: f64,
) -> Glide {
    let carried = current
        .filter(|glide| glide.node == node && glide.remaining.signum() == delta.signum())
        .map_or(0.0, |glide| glide.remaining);
    let target = (offset + carried + delta).clamp(0.0, max.max(0.0));
    Glide {
        node,
        remaining: target - offset,
        advanced_at: current
            .filter(|glide| glide.node == node)
            .map_or(now, |glide| glide.advanced_at),
    }
}

#[must_use]
pub fn step(remaining: f32, elapsed: f32) -> (f32, f32) {
    let applied = remaining * (1.0 - (-elapsed.max(0.0) / TIME_CONSTANT).exp());
    let left = remaining - applied;
    match left.abs() <= SETTLE_DISTANCE {
        true => (remaining, 0.0),
        false => (applied, left),
    }
}
