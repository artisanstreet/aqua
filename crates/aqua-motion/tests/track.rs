use aqua_motion::{MotionKind, MotionValue, ReducedMotion, SpringParams, Track};

fn scalar(v: f32) -> MotionValue {
    MotionValue::Scalar(v)
}

fn spring_track(params: SpringParams, to: f32) -> Track {
    let mut track = Track::new(
        scalar(0.0),
        MotionKind::Spring(params),
        ReducedMotion::Full,
        0.0,
    );
    track
        .retarget(scalar(to), MotionKind::Spring(params), 0.0)
        .unwrap();
    track
}

#[test]
fn settle_deadline_bounds_the_real_settle_time_for_every_regime() {
    let regimes = [
        SpringParams::new(170.0, 26.0),
        SpringParams::new(320.0, 30.0),
        SpringParams::new(100.0, 20.0),
        SpringParams::new(400.0, 10.0),
        SpringParams::new(100.0, 60.0),
        SpringParams::new(25.0, 10.0),
    ];
    for params in regimes {
        for to in [1.0, -250.0, 0.01] {
            let mut track = spring_track(params, to);
            let deadline = track
                .settle_deadline()
                .expect("unsettled track has deadline");
            assert!(deadline > 0.0 && deadline <= 8.0, "{params:?} {deadline}");
            let mut seen_settled_at = None;
            let mut t = 0.0;
            while t <= deadline + 1e-9 {
                track.sample(t);
                if track.is_settled() && seen_settled_at.is_none() {
                    seen_settled_at = Some(t);
                }
                t += 1.0 / 240.0;
            }
            track.sample(deadline);
            assert!(
                track.is_settled(),
                "{params:?} to {to} not settled by {deadline}"
            );
            assert_eq!(track.value(), scalar(to));
        }
    }
}

#[test]
fn tween_deadline_is_its_end() {
    let mut track = Track::new(
        scalar(0.0),
        MotionKind::Tween {
            duration_secs: 0.25,
            easing: aqua_motion::Easing::Linear,
        },
        ReducedMotion::Full,
        1.0,
    );
    track.retarget(scalar(1.0), track.kind(), 1.0).unwrap();
    assert_eq!(track.settle_deadline(), Some(1.25));
    assert_eq!(track.sample(1.125), scalar(0.5));
}

#[test]
fn retarget_mid_flight_keeps_position_and_velocity() {
    let params = SpringParams::SMOOTH;
    let mut track = spring_track(params, 100.0);
    let before = track.sample(0.2);
    let velocity_before = track.spring_segments()[0].sample(params, 0.2).1;
    track
        .retarget(scalar(-40.0), MotionKind::Spring(params), 0.2)
        .unwrap();
    assert_eq!(track.sample(0.2), before);
    let segment = track.spring_segments()[0];
    assert_eq!(segment.v0, velocity_before);
    assert_eq!(segment.target, -40.0);
}

#[test]
fn reduced_motion_collapses_in_flight_track() {
    let mut track = spring_track(SpringParams::SMOOTH, 10.0);
    track.sample(0.1);
    let settled = track.set_reduced(ReducedMotion::Reduce, 0.1);
    assert!(!settled);
    assert_eq!(track.sample(0.1), scalar(10.0));
    assert!(track.is_settled());
}
