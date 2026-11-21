use aqua_core::ScrollDelta;
use aqua_shell::input::{scroll, NOTCH_UNITS, WHEEL_NOTCH};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Wheel {
    Precise { dx: f64, dy: f64 },
    Lines { dx: f64, dy: f64 },
}

fn clamp_i16(value: f64) -> i16 {
    value
        .round()
        .clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16
}

fn off_the_notch_grid(value: f32) -> f32 {
    let lines = value / NOTCH_UNITS;
    match value != 0.0 && lines.fract() == 0.0 {
        true => f32::from_bits(value.to_bits() + 1),
        false => value,
    }
}

pub fn delta(wheel: Wheel) -> Option<ScrollDelta> {
    let delta = match wheel {
        Wheel::Precise { dx, dy } => ScrollDelta {
            x: -dx as f32,
            y: off_the_notch_grid(-dy as f32),
        },
        Wheel::Lines { dx, dy } => ScrollDelta {
            x: scroll(false, clamp_i16(-dx * f64::from(WHEEL_NOTCH))).x,
            y: scroll(true, clamp_i16(dy * f64::from(WHEEL_NOTCH))).y,
        },
    };
    (delta.x != 0.0 || delta.y != 0.0).then_some(delta)
}
