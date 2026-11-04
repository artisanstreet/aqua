use aqua_core::{NodeId, RawId, ScrollDelta};
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
fn pushes_accumulate_clamp_and_reverse() {
    let node = NodeId::from_raw(RawId {
        index: 1,
        generation: 0,
    });
    let first = glide::push(None, node, 100.0, 0.0, 130.0, 0.0);
    assert_eq!(first.remaining, 100.0);
    let second = glide::push(Some(first), node, 100.0, 0.0, 130.0, 0.01);
    assert_eq!(second.remaining, 130.0);
    let reversed = glide::push(Some(second), node, -50.0, 40.0, 130.0, 0.02);
    assert_eq!(reversed.remaining, -40.0);
}

#[test]
fn steps_converge_and_settle_exactly() {
    let mut remaining = 200.0_f32;
    let mut travelled = 0.0;
    for _ in 0..120 {
        let (applied, left) = glide::step(remaining, 1.0 / 60.0);
        travelled += applied;
        remaining = left;
        if remaining == 0.0 {
            break;
        }
    }
    assert_eq!(remaining, 0.0);
    assert!((travelled - 200.0).abs() < 1e-3);
}
