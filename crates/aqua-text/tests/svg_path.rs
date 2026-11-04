use aqua_text::svg::path::Polyline;

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1.0e-3
}

#[test]
fn polylines_measure_and_sample_straight_runs() {
    let line = Polyline::parse("M0 0 L10 0 v10 h-10 z").unwrap();
    assert!(near(line.length(), 40.0));
    let on_top = line.sample(5.0).unwrap();
    assert!(near(on_top.point[0], 5.0) && near(on_top.angle, 0.0));
    let on_side = line.sample(15.0).unwrap();
    assert!(near(on_side.point[0], 10.0) && near(on_side.point[1], 5.0));
    assert!(near(on_side.angle, std::f32::consts::FRAC_PI_2));
    assert!(line.sample(40.5).is_none() && line.sample(-1.0).is_none());
}

#[test]
fn arcs_and_curves_approximate_their_true_length() {
    let half_circle = Polyline::parse("M0 0 A10 10 0 0 1 20 0").unwrap();
    assert!((half_circle.length() - std::f32::consts::PI * 10.0).abs() < 0.2);
    let quarter = Polyline::parse("M0 0 C 0 5.5228 4.4772 10 10 10").unwrap();
    assert!((quarter.length() - std::f32::consts::FRAC_PI_2 * 10.0).abs() < 0.1);
    let reflected = Polyline::parse("M0 0 Q5 5 10 0 T20 0").unwrap();
    assert!(reflected.sample(reflected.length() * 0.75).unwrap().point[1] < 0.0);
}

#[test]
fn compact_arc_flags_and_implicit_commands_parse() {
    let line = Polyline::parse("M0,0 10,0 10,10").unwrap();
    assert!(near(line.length(), 20.0));
    assert!(Polyline::parse("M0 0a5 5 0 1010 0").is_some());
    assert!(Polyline::parse("L0 0 10 10").is_none());
    assert!(Polyline::parse("M0 0").is_none());
}
