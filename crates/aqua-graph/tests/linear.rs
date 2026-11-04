use aqua_core::geometry::Rect;
use aqua_graph::Linear2;

#[test]
fn inverse_undoes_rotation_and_skew() {
    let linear = Linear2::rotation(0.7).then(Linear2::skew(0.3, -0.1));
    let inverse = linear.inverse().expect("invertible");
    let back = inverse.apply(linear.apply([3.0, -2.0]));
    assert!((back[0] - 3.0).abs() < 1.0e-5 && (back[1] + 2.0).abs() < 1.0e-5);
}

#[test]
fn quarter_turn_cover_swaps_extents_about_the_center() {
    let bounds = Rect {
        x: 10.0,
        y: 20.0,
        w: 40.0,
        h: 10.0,
    };
    let covered = Linear2::rotation(std::f32::consts::FRAC_PI_2).cover(bounds);
    assert!((covered.w - 10.0).abs() < 1.0e-4 && (covered.h - 40.0).abs() < 1.0e-4);
    assert!((covered.center().x - 30.0).abs() < 1.0e-4);
    assert!((covered.center().y - 25.0).abs() < 1.0e-4);
}

#[test]
fn singular_matrices_have_no_inverse() {
    assert!(Linear2::scale(0.0, 1.0).inverse().is_none());
}

#[test]
fn pivot_rotation_moves_the_center() {
    let bounds = Rect {
        x: 10.0,
        y: 0.0,
        w: 2.0,
        h: 2.0,
    };
    let moved = Linear2::rotation(std::f32::consts::FRAC_PI_2).about([0.0, 0.0], bounds);
    assert!((moved.center().x + 1.0).abs() < 1.0e-4);
    assert!((moved.center().y - 11.0).abs() < 1.0e-4);
}
