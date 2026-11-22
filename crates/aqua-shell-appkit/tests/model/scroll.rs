use aqua_core::ScrollDelta;
use aqua_runtime::glide::notch_pixels;
use aqua_shell_appkit::model::scroll::{delta, Wheel};

#[test]
fn one_mouse_wheel_notch_scrolls_sixteen_units_like_windows_and_linux() {
    assert_eq!(
        delta(Wheel::Lines { dx: 0.0, dy: 1.0 }),
        Some(ScrollDelta { x: 0.0, y: -16.0 })
    );
    assert_eq!(
        delta(Wheel::Lines { dx: -2.0, dy: 0.0 }),
        Some(ScrollDelta { x: 32.0, y: 0.0 })
    );
}

#[test]
fn a_mouse_wheel_notch_glides_but_trackpad_points_do_not() {
    let wheel = delta(Wheel::Lines { dx: 0.0, dy: -3.0 }).expect("a notch scrolls");
    assert!(notch_pixels(wheel).is_some(), "{wheel:?} should glide");
    for points in [-16.0, -32.0, 48.0, -7.5, 3.0] {
        let precise = delta(Wheel::Precise {
            dx: 0.0,
            dy: points,
        })
        .expect("points scroll");
        assert!(
            notch_pixels(precise).is_none(),
            "a trackpad delta of {points} points became a notch glide: {precise:?}"
        );
        assert!(
            (f64::from(precise.y) + points).abs() < 1e-5,
            "trackpad {points} points arrived as {precise:?}"
        );
    }
}

#[test]
fn trackpad_deltas_follow_the_content_direction_in_points() {
    assert_eq!(
        delta(Wheel::Precise {
            dx: 4.5,
            dy: -12.25
        }),
        Some(ScrollDelta { x: -4.5, y: 12.25 })
    );
    assert_eq!(delta(Wheel::Precise { dx: 0.0, dy: 0.0 }), None);
}
