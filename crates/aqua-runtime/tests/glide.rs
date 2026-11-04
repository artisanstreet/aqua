use aqua_core::ScrollDelta;
use aqua_motion::{MotionKind, Track};
use aqua_runtime::glide::{self, NOTCH_PIXELS};

fn delta(x: f32, y: f32) -> ScrollDelta {
    ScrollDelta { x, y }
}

#[test]
fn whole_line_deltas_are_notches_and_fractions_are_precise() {
    assert_eq!(glide::notch_pixels(delta(0.0, 16.0)), Some(NOTCH_PIXELS));
    assert_eq!(
        glide::notch_pixels(delta(0.0, -32.0)),
        Some(-2.0 * NOTCH_PIXELS)
    );
    assert_eq!(glide::notch_pixels(delta(0.0, 5.3)), None);
    assert_eq!(glide::notch_pixels(delta(4.0, 16.0)), None);
    assert_eq!(glide::notch_pixels(delta(0.0, 0.0)), None);
}

#[test]
fn the_glide_kind_is_a_critically_damped_spring_that_never_overshoots() {
    let MotionKind::Spring(params) = glide::kind() else {
        panic!("glide must be a spring");
    };
    let zeta = params.damping / (2.0 * params.stiffness.sqrt());
    assert!((zeta - 1.0).abs() < 1e-3);

    let mut track = Track::new(
        aqua_motion::MotionValue::Scalar(0.0),
        glide::kind(),
        aqua_motion::ReducedMotion::Full,
        0.0,
    );
    track
        .shift(
            aqua_motion::MotionValue::Scalar(200.0),
            aqua_motion::MotionValue::Scalar(0.0),
            glide::kind(),
            0.0,
        )
        .unwrap();
    let mut t = 0.0;
    let mut previous = 200.0_f32;
    while !track.is_settled() && t < 3.0 {
        t += 1.0 / 240.0;
        let aqua_motion::MotionValue::Scalar(value) = track.sample(t) else {
            unreachable!();
        };
        assert!(
            (0.0..=previous + 1e-4).contains(&value),
            "{value} after {previous}"
        );
        previous = value;
    }
    assert!(track.is_settled());
    assert!(t < 1.0, "settles within a second, took {t}");
}
