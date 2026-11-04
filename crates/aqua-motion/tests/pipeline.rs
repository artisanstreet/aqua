use std::cell::RefCell;
use std::rc::Rc;

use aqua_core::NodeId;
use aqua_core::id::RawId;
use aqua_motion::{
    AppMotion, Completion, Easing, MotionCommand, MotionEvent, MotionKind, MotionValue,
    ReducedMotion, TrackSet,
};

fn scalar(v: f32) -> MotionValue {
    MotionValue::Scalar(v)
}

fn linear(secs: f64) -> MotionKind {
    MotionKind::Tween {
        duration_secs: secs,
        easing: Easing::Linear,
    }
}

fn node(index: u32) -> NodeId {
    NodeId::from_raw(RawId {
        index,
        generation: 0,
    })
}

type Seen = Rc<RefCell<Vec<Completion>>>;

fn recorder() -> (Box<dyn FnMut(Completion)>, Seen) {
    let seen: Seen = Rc::default();
    let inner = Rc::clone(&seen);
    (Box::new(move |c| inner.borrow_mut().push(c)), seen)
}

fn pump(app: &mut AppMotion, set: &mut TrackSet) -> Vec<MotionEvent> {
    app.drain_commands()
        .into_iter()
        .flat_map(|command| set.apply(command))
        .collect()
}

fn run_until_settled(
    app: &mut AppMotion,
    set: &mut TrackSet,
    mut now: f64,
) -> (f64, usize, Vec<MotionEvent>) {
    let mut frames = 0;
    let mut events = Vec::new();
    while !set.is_idle() && frames < 10_000 {
        now += 1.0 / 120.0;
        let sampled = set.sample(now);
        frames += 1;
        for event in sampled.events {
            app.handle(event);
            events.push(event);
        }
    }
    (now, frames, events)
}

#[test]
fn app_side_is_untouched_while_a_value_animates() {
    let mut app = AppMotion::default();
    let mut set = TrackSet::default();
    let track = app.spawn(None, None, scalar(0.0), linear(0.5), 0.0);
    app.set_target(track, scalar(1.0), 0.0).unwrap();
    assert_eq!(pump(&mut app, &mut set), vec![]);
    let mut now = 0.0;
    let mut mid_values = Vec::new();
    for _ in 0..30 {
        now += 1.0 / 120.0;
        let sampled = set.sample(now);
        assert!(sampled.events.is_empty());
        mid_values.extend(sampled.values.iter().map(|p| p.value));
        assert!(app.drain_commands().is_empty());
    }
    assert!(!app.is_settled(track));
    assert!(mid_values.windows(2).all(|w| match (w[0], w[1]) {
        (MotionValue::Scalar(a), MotionValue::Scalar(b)) => b >= a,
        _ => false,
    }));
    let (_, _, events) = run_until_settled(&mut app, &mut set, now);
    assert_eq!(events.len(), 1);
    assert!(app.is_settled(track));
    assert!(set.is_idle());
}

#[test]
fn completion_fires_once_when_the_settle_event_arrives() {
    let mut app = AppMotion::default();
    let mut set = TrackSet::default();
    let track = app.spawn(None, None, scalar(0.0), linear(0.1), 0.0);
    let (callback, seen) = recorder();
    app.on_complete(track, callback);
    app.set_target(track, scalar(1.0), 0.0).unwrap();
    pump(&mut app, &mut set);
    run_until_settled(&mut app, &mut set, 0.0);
    assert_eq!(*seen.borrow(), vec![Completion::Completed]);
    app.handle(MotionEvent::Settled {
        track,
        generation: 1,
    });
    assert_eq!(seen.borrow().len(), 1);
}

#[test]
fn superseded_completion_never_fires_and_stale_settle_is_dropped() {
    let mut app = AppMotion::default();
    let mut set = TrackSet::default();
    let track = app.spawn(None, None, scalar(0.0), linear(0.1), 0.0);
    let (first, first_seen) = recorder();
    app.on_complete(track, first);
    app.set_target(track, scalar(1.0), 0.0).unwrap();
    let (second, second_seen) = recorder();
    app.on_complete(track, second);
    app.set_target(track, scalar(2.0), 0.02).unwrap();
    app.handle(MotionEvent::Settled {
        track,
        generation: 1,
    });
    assert!(!app.is_settled(track));
    pump(&mut app, &mut set);
    run_until_settled(&mut app, &mut set, 0.02);
    assert!(first_seen.borrow().is_empty());
    assert_eq!(*second_seen.borrow(), vec![Completion::Completed]);
}

#[test]
fn cancel_reports_cancelled_and_halts_the_render_track() {
    let mut app = AppMotion::default();
    let mut set = TrackSet::default();
    let track = app.spawn(None, None, scalar(0.0), linear(1.0), 0.0);
    let (callback, seen) = recorder();
    app.on_complete(track, callback);
    app.set_target(track, scalar(1.0), 0.0).unwrap();
    pump(&mut app, &mut set);
    set.sample(0.25);
    assert!(app.cancel(track));
    pump(&mut app, &mut set);
    assert_eq!(*seen.borrow(), vec![Completion::Cancelled]);
    let sampled = set.sample(0.3);
    assert!(sampled.events.is_empty());
    set.sample(0.31);
    assert!(set.is_idle());
}

#[test]
fn dispose_owner_removes_tracks_on_both_sides_silently() {
    let mut app = AppMotion::default();
    let mut set = TrackSet::default();
    let owner = node(7);
    let (callback, seen) = recorder();
    let track = app.spawn(Some(owner), None, scalar(0.0), linear(1.0), 0.0);
    app.on_complete(track, callback);
    app.set_target(track, scalar(1.0), 0.0).unwrap();
    let other = app.spawn(None, None, scalar(0.0), linear(1.0), 0.0);
    pump(&mut app, &mut set);
    assert_eq!(app.dispose_owner(owner), vec![track]);
    let commands = app.drain_commands();
    assert_eq!(commands, vec![MotionCommand::Remove { track }]);
    for command in commands {
        set.apply(command);
    }
    assert_eq!(set.len(), 1);
    assert!(set.value(other).is_some());
    assert!(seen.borrow().is_empty());
    assert!(app.set_target(track, scalar(1.0), 0.1).is_err());
}

#[test]
fn reduced_motion_settles_in_flight_tracks_with_current_generations() {
    let mut app = AppMotion::default();
    let mut set = TrackSet::default();
    let track = app.spawn(None, None, scalar(0.0), linear(5.0), 0.0);
    let (callback, seen) = recorder();
    app.on_complete(track, callback);
    app.set_target(track, scalar(1.0), 0.0).unwrap();
    pump(&mut app, &mut set);
    set.sample(1.0);
    app.set_reduced_motion(ReducedMotion::Reduce, 1.0);
    let mut events = pump(&mut app, &mut set);
    events.extend(set.sample(1.0).events);
    for event in events {
        app.handle(event);
    }
    assert_eq!(set.value(track), Some(scalar(1.0)));
    assert_eq!(*seen.borrow(), vec![Completion::Completed]);
    assert!(app.is_settled(track));
}

#[test]
fn first_sample_after_spawn_reports_the_initial_value_without_motion() {
    let mut app = AppMotion::default();
    let mut set = TrackSet::default();
    let track = app.spawn(None, None, scalar(3.0), linear(1.0), 0.0);
    pump(&mut app, &mut set);
    let first = set.sample(0.0);
    assert_eq!(first.values.len(), 1);
    assert_eq!(first.values[0].track, track);
    assert_eq!(first.values[0].value, scalar(3.0));
    assert!(first.events.is_empty());
    assert!(set.sample(0.1).values.is_empty());
    assert!(set.is_idle());
}
