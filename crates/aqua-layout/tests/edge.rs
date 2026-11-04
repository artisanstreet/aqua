use aqua_core::{Arena, NodeId, Rect};
use aqua_layout::edge::{moved_edges, presented_rect, EdgeAnimator};
use aqua_motion::{
    AppMotion, Easing, Edge, MotionKind, MotionValue, Presented, Property, TrackSet,
};

const LINEAR: MotionKind = MotionKind::Tween {
    duration_secs: 1.0,
    easing: Easing::Linear,
};

fn node() -> NodeId {
    let mut arena: Arena<()> = Arena::new();
    NodeId::from_raw(arena.insert(()))
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

fn edges_of(values: &[Presented]) -> Vec<(Edge, f32)> {
    values
        .iter()
        .filter_map(|presented| match (presented.binding, presented.value) {
            (Some(binding), MotionValue::Scalar(v)) => match binding.property {
                Property::Edge(edge) => Some((edge, v)),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn flush(motion: &mut AppMotion, tracks: &mut TrackSet) {
    for command in motion.drain_commands() {
        tracks.apply(command);
    }
}

#[test]
fn first_observation_creates_no_tracks() {
    let mut motion = AppMotion::default();
    let mut animator = EdgeAnimator::new();
    let node = node();
    let moved = animator.observe(&mut motion, node, rect(0.0, 0.0, 100.0, 50.0), LINEAR, 0.0);
    assert!(moved.is_empty());
    assert!(motion.is_empty());
    assert!(motion.drain_commands().is_empty());
}

#[test]
fn only_moved_edges_get_tracks() {
    let mut motion = AppMotion::default();
    let mut animator = EdgeAnimator::new();
    let node = node();
    animator.observe(
        &mut motion,
        node,
        rect(10.0, 20.0, 100.0, 50.0),
        LINEAR,
        0.0,
    );
    let moved = animator.observe(
        &mut motion,
        node,
        rect(10.0, 20.0, 160.0, 50.0),
        LINEAR,
        1.0,
    );
    assert_eq!(moved, vec![Edge::Right]);
    assert_eq!(motion.len(), 1);
    assert!(animator.track(node, Edge::Right).is_some());
    for edge in [Edge::Left, Edge::Top, Edge::Bottom] {
        assert!(animator.track(node, edge).is_none());
    }
}

#[test]
fn unchanged_layout_creates_nothing() {
    let mut motion = AppMotion::default();
    let mut animator = EdgeAnimator::new();
    let node = node();
    let frame = rect(1.0, 2.0, 30.0, 40.0);
    animator.observe(&mut motion, node, frame, LINEAR, 0.0);
    animator.observe(&mut motion, node, frame, LINEAR, 1.0);
    assert!(motion.is_empty());
}

#[test]
fn presented_frame_interpolates_moving_edges_only() {
    let mut motion = AppMotion::default();
    let mut tracks = TrackSet::default();
    let mut animator = EdgeAnimator::new();
    let node = node();
    let before = rect(10.0, 20.0, 100.0, 50.0);
    let after = rect(10.0, 20.0, 200.0, 90.0);
    animator.observe(&mut motion, node, before, LINEAR, 0.0);
    animator.observe(&mut motion, node, after, LINEAR, 1.0);
    flush(&mut motion, &mut tracks);

    let sampled = tracks.sample(1.5);
    let frame = presented_rect(after, edges_of(&sampled.values));
    assert!((frame.x - 10.0).abs() < 1e-4 && (frame.y - 20.0).abs() < 1e-4);
    assert!((frame.w - 150.0).abs() < 1e-3, "{frame:?}");
    assert!((frame.h - 70.0).abs() < 1e-3, "{frame:?}");

    let sampled = tracks.sample(2.0);
    let frame = presented_rect(after, edges_of(&sampled.values));
    assert_eq!(frame, after);
    assert_eq!(sampled.events.len(), 2);
}

#[test]
fn retarget_mid_flight_does_not_jump() {
    let mut motion = AppMotion::default();
    let mut tracks = TrackSet::default();
    let mut animator = EdgeAnimator::new();
    let node = node();
    animator.observe(&mut motion, node, rect(0.0, 0.0, 100.0, 10.0), LINEAR, 0.0);
    animator.observe(&mut motion, node, rect(0.0, 0.0, 200.0, 10.0), LINEAR, 1.0);
    flush(&mut motion, &mut tracks);
    let right = animator.track(node, Edge::Right).unwrap();

    tracks.sample(1.5);
    let mid = tracks.value(right).unwrap();
    assert_eq!(mid, MotionValue::Scalar(150.0));

    animator.observe(&mut motion, node, rect(0.0, 0.0, 50.0, 10.0), LINEAR, 1.5);
    flush(&mut motion, &mut tracks);
    let sampled = tracks.sample(1.5);
    let value = sampled
        .values
        .iter()
        .find(|presented| presented.track == right)
        .unwrap()
        .value;
    assert_eq!(value, MotionValue::Scalar(150.0));
    assert_eq!(motion.target(right), Some(MotionValue::Scalar(50.0)));
}

#[test]
fn forget_disposes_tracks() {
    let mut motion = AppMotion::default();
    let mut animator = EdgeAnimator::new();
    let node = node();
    animator.observe(&mut motion, node, rect(0.0, 0.0, 10.0, 10.0), LINEAR, 0.0);
    animator.observe(&mut motion, node, rect(5.0, 5.0, 20.0, 20.0), LINEAR, 1.0);
    assert_eq!(motion.len(), 4);
    let removed = animator.forget(&mut motion, node);
    assert_eq!(removed.len(), 4);
    assert!(motion.is_empty());
    assert!(animator.target(node).is_none());
}

#[test]
fn moved_edges_ignores_subpixel_noise() {
    let a = rect(0.0, 0.0, 10.0, 10.0);
    let b = rect(0.0004, 0.0, 10.0, 10.0);
    assert!(moved_edges(a, b).is_empty());
    assert_eq!(
        moved_edges(a, rect(0.0, 0.0, 12.0, 10.0)),
        vec![Edge::Right]
    );
}

#[test]
fn crossing_edges_clamp_to_zero_size() {
    let frame = presented_rect(rect(0.0, 0.0, 10.0, 10.0), [(Edge::Right, -5.0)]);
    assert_eq!(frame.w, 0.0);
}
