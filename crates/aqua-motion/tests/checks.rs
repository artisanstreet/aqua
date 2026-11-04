use aqua_motion::{
    AppMotion, Completion, Easing, MotionCommand, MotionEvent, MotionKind, MotionValue,
    ReducedMotion, SpringParams, Track, TrackSet,
};
use std::cell::RefCell;
use std::rc::Rc;

fn scalar(v: f32) -> MotionValue {
    MotionValue::Scalar(v)
}

fn value_of(track: &mut Track, at: f64) -> f32 {
    match track.sample(at) {
        MotionValue::Scalar(v) => v,
        other => panic!("not a scalar: {other:?}"),
    }
}

fn kinds() -> Vec<MotionKind> {
    let mut kinds = vec![
        MotionKind::Spring(SpringParams::SMOOTH),
        MotionKind::Spring(SpringParams::SNAPPY),
        MotionKind::Spring(SpringParams::new(100.0, 60.0)),
        MotionKind::Spring(SpringParams::new(400.0, 10.0)),
        MotionKind::Spring(SpringParams::from_duration_bounce(0.25, 0.0)),
    ];
    for easing in [
        Easing::Linear,
        Easing::EaseIn,
        Easing::EaseOut,
        Easing::EaseInOut,
    ] {
        kinds.push(MotionKind::Tween {
            duration_secs: 0.4,
            easing,
        });
    }
    kinds
}

fn started(kind: MotionKind, to: f32) -> Track {
    let mut track = Track::new(scalar(0.0), kind, ReducedMotion::Full, 0.0);
    track.retarget(scalar(to), kind, 0.0).unwrap();
    track
}

#[test]
fn retargeting_between_frames_never_jumps_the_presented_value() {
    let frame = 1.0 / 120.0;
    let mut worst = 0.0_f32;
    for first in kinds() {
        for second in kinds() {
            for to in [100.0_f32, -40.0] {
                for step in 1..160 {
                    let mut track = started(first, to);
                    let mut oracle = track.clone();
                    let retarget_at = f64::from(step) * 0.0071;
                    let mut t = 0.0;
                    while t + frame <= retarget_at {
                        t += frame;
                        value_of(&mut track, t);
                    }
                    let before = value_of(&mut oracle, retarget_at);
                    track
                        .retarget(scalar(to * -0.5 + 7.0), second, retarget_at)
                        .unwrap();
                    let after = value_of(&mut track, retarget_at);
                    worst = worst.max((after - before).abs());
                    assert!(
                        (after - before).abs() <= 1e-3 * to.abs().max(1.0),
                        "{first:?} -> {second:?} at {retarget_at}: {before} became {after}"
                    );
                }
            }
        }
    }
    assert!(worst < 0.1);
}

#[test]
fn spring_retarget_carries_velocity() {
    let kind = MotionKind::Spring(SpringParams::SMOOTH);
    let h = 1e-4;
    for step in 1..60 {
        let at = f64::from(step) * 0.0123;
        let mut track = started(kind, 100.0);
        let oracle = track.clone();
        value_of(&mut track, at - 0.004);
        let slope_before =
            (value_of(&mut oracle.clone(), at) - value_of(&mut oracle.clone(), at - h)) / h as f32;
        track.retarget(scalar(-30.0), kind, at).unwrap();
        let x0 = value_of(&mut track, at);
        let x1 = value_of(&mut track, at + h);
        let slope_after = (x1 - x0) / h as f32;
        assert!(
            (slope_after - slope_before).abs()
                <= 0.02 * slope_before.abs().max(10.0) + 30_000.0 * h as f32,
            "at {at}: {slope_before} vs {slope_after}"
        );
    }
}

#[test]
fn shifting_moves_the_presented_value_by_exactly_the_delta() {
    for kind in kinds() {
        for step in 1..80 {
            let at = f64::from(step) * 0.0093;
            let mut track = started(kind, 0.0);
            track.retarget(scalar(50.0), kind, 0.0).unwrap();
            let mut oracle = track.clone();
            let before = value_of(&mut oracle, at);
            track.shift(scalar(-17.5), scalar(50.0), kind, at).unwrap();
            let after = value_of(&mut track, at);
            assert!((after - (before - 17.5)).abs() <= 1e-3, "{kind:?} at {at}");
        }
    }
}

#[test]
fn shifting_settles_on_the_target_and_reports_it_once() {
    let mut app = AppMotion::default();
    let mut set = TrackSet::default();
    let kind = MotionKind::Spring(SpringParams::from_duration_bounce(0.25, 0.0));
    let track = app.spawn(None, None, MotionValue::Pair(0.0, 0.0), kind, 0.0);
    app.shift(
        track,
        MotionValue::Pair(0.0, 77.7),
        MotionValue::Pair(0.0, 0.0),
        kind,
        0.0,
    )
    .unwrap();
    for command in app.drain_commands() {
        set.apply(command);
    }
    let mut events = Vec::new();
    let mut t = 0.0;
    let mut previous = f32::MAX;
    while !set.is_idle() && t < 5.0 {
        t += 1.0 / 144.0;
        let sampled = set.sample(t);
        for presented in &sampled.values {
            if let MotionValue::Pair(_, y) = presented.value {
                assert!(
                    y <= previous + 1e-3,
                    "critically damped glide must not overshoot"
                );
                previous = y;
            }
        }
        events.extend(sampled.events);
    }
    assert_eq!(events.len(), 1);
    assert_eq!(set.value(track), Some(MotionValue::Pair(0.0, 0.0)));
}

#[test]
fn nothing_moves_on_first_mount() {
    for kind in kinds() {
        let mut app = AppMotion::default();
        let mut set = TrackSet::default();
        let track = app.spawn(None, None, scalar(0.7), kind, 0.0);
        for command in app.drain_commands() {
            assert!(matches!(command, MotionCommand::Spawn { .. }));
            set.apply(command);
        }
        let first = set.sample(0.0);
        assert!(first.events.is_empty());
        assert!(first.values.iter().all(|p| p.value == scalar(0.7)));
        assert!(set.is_idle(), "{kind:?}");
        assert!(app.is_settled(track));
        assert!(set.sample(0.5).values.is_empty());
    }
}

#[test]
fn retargeting_to_the_current_value_settles_without_travelling() {
    for kind in kinds() {
        let mut track = Track::new(scalar(4.0), kind, ReducedMotion::Full, 0.0);
        track.retarget(scalar(4.0), kind, 0.1).unwrap();
        let mut t = 0.1;
        let mut ticks = 0;
        while !track.is_settled() && ticks < 600 {
            t += 1.0 / 120.0;
            assert!((value_of(&mut track, t) - 4.0).abs() < 1e-4);
            ticks += 1;
        }
        assert!(track.is_settled(), "{kind:?}");
    }
}

#[test]
fn reduced_motion_snaps_new_and_in_flight_motion_then_full_resumes() {
    for kind in kinds() {
        let mut app = AppMotion::default();
        let mut set = TrackSet::default();
        app.set_reduced_motion(ReducedMotion::Reduce, 0.0);
        let track = app.spawn(None, None, scalar(0.0), kind, 0.0);
        app.set_target(track, scalar(10.0), 0.0).unwrap();
        for command in app.drain_commands() {
            for event in set.apply(command) {
                app.handle(event);
            }
        }
        let sampled = set.sample(0.0);
        assert_eq!(set.value(track), Some(scalar(10.0)), "{kind:?}");
        for event in sampled.events {
            app.handle(event);
        }
        assert!(app.is_settled(track));

        app.set_reduced_motion(ReducedMotion::Full, 1.0);
        app.set_target(track, scalar(20.0), 1.0).unwrap();
        for command in app.drain_commands() {
            set.apply(command);
        }
        set.sample(1.0);
        let mid = set.sample(1.02);
        assert!(!set.is_idle(), "{kind:?} should animate under Full");
        assert!(mid.values.iter().all(|p| p.value != scalar(20.0)));

        app.set_reduced_motion(ReducedMotion::Reduce, 1.03);
        let mut events = Vec::new();
        for command in app.drain_commands() {
            events.extend(set.apply(command));
        }
        events.extend(set.sample(1.03).events);
        assert_eq!(set.value(track), Some(scalar(20.0)));
        assert_eq!(events.len(), 1);
        for event in events {
            app.handle(event);
        }
        assert!(app.is_settled(track));
    }
}

#[test]
fn completion_is_delivered_exactly_once_for_the_current_target_only() {
    let seen: Rc<RefCell<Vec<(u32, Completion)>>> = Rc::default();
    let record = |tag: u32| -> Box<dyn FnMut(Completion)> {
        let seen = Rc::clone(&seen);
        Box::new(move |completion| seen.borrow_mut().push((tag, completion)))
    };
    let kind = MotionKind::Tween {
        duration_secs: 0.2,
        easing: Easing::Linear,
    };
    let mut app = AppMotion::default();
    let mut set = TrackSet::default();
    let track = app.spawn(None, None, scalar(0.0), kind, 0.0);

    app.on_complete(track, record(1));
    app.set_target(track, scalar(1.0), 0.0).unwrap();
    app.on_complete(track, record(2));
    app.shift(track, scalar(5.0), scalar(3.0), kind, 0.05)
        .unwrap();
    for command in app.drain_commands() {
        set.apply(command);
    }
    let mut t = 0.0;
    while !set.is_idle() && t < 3.0 {
        t += 1.0 / 120.0;
        for event in set.sample(t).events {
            app.handle(event);
            app.handle(event);
        }
    }
    assert_eq!(*seen.borrow(), vec![(2, Completion::Completed)]);

    app.on_complete(track, record(3));
    app.set_target(track, scalar(9.0), t).unwrap();
    assert!(app.cancel(track));
    app.handle(MotionEvent::Settled {
        track,
        generation: 99,
    });
    assert_eq!(
        *seen.borrow(),
        vec![(2, Completion::Completed), (3, Completion::Cancelled)]
    );

    app.on_complete(track, record(4));
    app.set_target(track, scalar(1.0), t).unwrap();
    assert!(app.finish(track));
    assert_eq!(seen.borrow().last(), Some(&(4, Completion::Completed)));

    app.on_complete(track, record(5));
    app.set_target(track, scalar(2.0), t).unwrap();
    app.remove(track);
    assert_eq!(seen.borrow().len(), 3);
}

#[test]
fn settle_happens_within_the_declared_bound() {
    for kind in kinds() {
        let mut track = started(kind, 120.0);
        let bound = track.settle_deadline().expect("in flight");
        let mut t = 0.0;
        while !track.is_settled() {
            t += 1.0 / 240.0;
            track.sample(t);
            assert!(
                t <= bound + 1.0 / 120.0,
                "{kind:?} settled after {t} > {bound}"
            );
        }
    }
}
