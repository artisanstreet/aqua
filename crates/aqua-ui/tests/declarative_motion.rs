use aqua_core::{NodeId, Oklch, Size};
use aqua_motion::{
    AppMotion, Easing, Edge, MotionCommand, MotionKind, MotionValue, Property, ReducedMotion,
    SpringParams, TrackSet,
};
use aqua_style::Paint;
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::motion::runtime::MotionRuntime;
use aqua_ui::motion::{
    Animate, AnimatedProperty, Motion, MotionSpec, MotionTheme, MotionToken, Presence,
};
use aqua_ui::style::{Axis, LayoutStyle, Length};
use aqua_ui::tree::{BoxContent, NodeKind, NodeSpec, UiTree};
use std::rc::Rc;

const FADE: Motion = Motion::tween(0.2, Easing::Linear);
const CARD: Motion = Motion::spring(SpringParams::SNAPPY);
const SLOW: MotionToken = MotionToken::new("slow");

fn red() -> Oklch {
    Oklch::oklch(0.6, 0.2, 25.0, 1.0)
}

fn blue() -> Oklch {
    Oklch::oklch(0.6, 0.2, 260.0, 1.0)
}

struct Row {
    key: u32,
    width: f32,
    fill: Option<Oklch>,
    opacity: Option<f32>,
    animate: Animate,
    presence: Presence,
}

impl Row {
    fn new(key: u32) -> Self {
        Self {
            key,
            width: 100.0,
            fill: None,
            opacity: None,
            animate: Animate::new(),
            presence: Presence::new(),
        }
    }

    fn spec(&self) -> NodeSpec {
        let paint = Paint {
            fill: self.fill,
            opacity: self.opacity,
            motion: Some(Rc::new(MotionSpec {
                animate: self.animate.clone(),
                presence: self.presence,
            })),
            ..Paint::default()
        };
        let style = LayoutStyle {
            width: Length::Px(self.width),
            height: Length::Px(30.0),
            ..LayoutStyle::default()
        };
        let mut spec = NodeSpec::new(
            NodeKind::Box(BoxContent {
                paint: paint.clone(),
            }),
            style,
        )
        .keyed(self.key);
        spec.paint = paint;
        spec
    }
}

struct Harness {
    tree: UiTree,
    root: NodeId,
    runtime: MotionRuntime,
    app: AppMotion,
    tracks: TrackSet,
    removed: Vec<NodeId>,
}

impl Harness {
    fn new(theme: MotionTheme) -> Self {
        let mut tree = UiTree::new();
        let root = tree.mount_spec(NodeSpec::new(
            NodeKind::Box(BoxContent::default()),
            LayoutStyle {
                axis: Axis::Column,
                width: Length::Px(400.0),
                height: Length::Px(400.0),
                ..LayoutStyle::default()
            },
        ));
        Self {
            tree,
            root,
            runtime: MotionRuntime::new(theme),
            app: AppMotion::new(ReducedMotion::Full),
            tracks: TrackSet::default(),
            removed: Vec::new(),
        }
    }

    fn render(&mut self, rows: &[Row], at: f64) -> Vec<MotionCommand> {
        let specs = rows.iter().map(Row::spec).collect();
        self.tree.sync_children(self.root, specs);
        self.tree
            .compute_layout(Size::new(400.0, 400.0), &NaiveMeasurer)
            .unwrap();
        self.runtime.reconcile(&mut self.app, &self.tree, at);
        let commands = self.app.drain_commands();
        for command in &commands {
            self.tracks.apply(*command);
        }
        commands
    }

    fn sample(&mut self, at: f64) {
        let sampled = self.tracks.sample(at);
        for event in sampled.events {
            self.removed
                .extend(self.runtime.handle(&mut self.app, &mut self.tree, event));
        }
    }

    fn node(&self, key: u32) -> NodeId {
        self.tree
            .find_by_key(aqua_ui::hash_key(key))
            .expect("keyed node")
    }

    fn value(&self, node: NodeId, property: AnimatedProperty) -> MotionValue {
        let track = self.runtime.track_of(node, property).expect("track");
        self.tracks.value(track).expect("render track")
    }
}

fn set_targets(commands: &[MotionCommand]) -> usize {
    commands
        .iter()
        .filter(|c| matches!(c, MotionCommand::SetTarget { .. }))
        .count()
}

#[test]
fn first_mount_spawns_tracks_without_motion() {
    let mut h = Harness::new(MotionTheme::default());
    let row = Row {
        fill: Some(red()),
        animate: Animate::new().fill(FADE).opacity(FADE),
        ..Row::new(1)
    };
    let commands = h.render(&[row], 0.0);
    assert_eq!(commands.len(), 2);
    assert!(commands
        .iter()
        .all(|c| matches!(c, MotionCommand::Spawn { .. })));
    assert_eq!(set_targets(&commands), 0);
    h.sample(0.0);
    assert!(h.app.all_settled() || h.tracks.is_idle());
}

#[test]
fn changed_value_retargets_and_interpolates_on_the_render_side() {
    let mut h = Harness::new(MotionTheme::default());
    let mut row = Row {
        fill: Some(red()),
        animate: Animate::new().fill(FADE),
        ..Row::new(1)
    };
    h.render(&[Row { ..Row::new(1) }], 0.0);
    h.render(std::slice::from_ref(&row), 0.0);
    h.sample(0.0);
    row.fill = Some(blue());
    let commands = h.render(std::slice::from_ref(&row), 1.0);
    assert_eq!(set_targets(&commands), 1);
    let node = h.node(1);
    h.sample(1.1);
    let MotionValue::Color(mid) = h.value(node, AnimatedProperty::Fill) else {
        panic!("color track")
    };
    let expected = red().lerp(blue(), 0.5);
    assert!((mid.l - expected.l).abs() < 1e-4 && (mid.h - expected.h).abs() < 1e-2);
    h.sample(1.5);
    assert_eq!(
        h.value(node, AnimatedProperty::Fill),
        MotionValue::Color(blue())
    );
    assert_eq!(
        h.runtime.target_of(node, AnimatedProperty::Fill),
        Some(MotionValue::Color(blue()))
    );
}

#[test]
fn unchanged_and_undeclared_values_emit_nothing() {
    let mut h = Harness::new(MotionTheme::default());
    let mut row = Row {
        fill: Some(red()),
        animate: Animate::new().fill(FADE),
        ..Row::new(1)
    };
    h.render(std::slice::from_ref(&row), 0.0);
    h.sample(0.0);
    assert!(h.render(std::slice::from_ref(&row), 1.0).is_empty());
    row.opacity = Some(0.2);
    assert!(h.render(std::slice::from_ref(&row), 2.0).is_empty());
}

#[test]
fn frame_animation_tracks_only_the_edges_that_moved() {
    let mut h = Harness::new(MotionTheme::default());
    let mut row = Row {
        animate: Animate::new().frame(FADE),
        ..Row::new(1)
    };
    let commands = h.render(std::slice::from_ref(&row), 0.0);
    assert!(commands.is_empty());
    row.width = 220.0;
    let commands = h.render(std::slice::from_ref(&row), 1.0);
    let edges: Vec<Edge> = commands
        .iter()
        .filter_map(|c| match c {
            MotionCommand::Spawn {
                binding: Some(binding),
                ..
            } => match binding.property {
                Property::Edge(edge) => Some(edge),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(edges, vec![Edge::Right]);
    assert_eq!(set_targets(&commands), 1);
}

#[test]
fn presence_enter_skips_first_mount_and_fades_later_arrivals() {
    let mut h = Harness::new(MotionTheme::default());
    let first = Row {
        presence: Presence::new().enter(FADE),
        ..Row::new(1)
    };
    let commands = h.render(std::slice::from_ref(&first), 0.0);
    assert!(commands.is_empty());

    let second = Row {
        presence: Presence::new().enter(FADE),
        ..Row::new(2)
    };
    let commands = h.render(&[first, second], 1.0);
    assert_eq!(commands.len(), 2);
    assert!(matches!(
        commands[0],
        MotionCommand::Spawn {
            initial: MotionValue::Scalar(0.0),
            ..
        }
    ));
    assert!(matches!(
        commands[1],
        MotionCommand::SetTarget {
            to: MotionValue::Scalar(1.0),
            ..
        }
    ));
    let node = h.node(2);
    h.sample(1.1);
    assert_eq!(
        h.value(node, AnimatedProperty::Opacity),
        MotionValue::Scalar(0.5)
    );
}

#[test]
fn exit_keeps_the_node_until_its_tracks_settle() {
    let mut h = Harness::new(MotionTheme::default());
    let rows = |keys: &[u32]| -> Vec<Row> {
        keys.iter()
            .map(|key| Row {
                presence: Presence::new().exit(FADE),
                ..Row::new(*key)
            })
            .collect()
    };
    h.render(&rows(&[1, 2, 3]), 0.0);
    let leaving = h.node(2);
    let before = h.tree.rect(leaving).unwrap();

    h.render(&rows(&[1, 3]), 1.0);
    assert!(h.tree.get(leaving).is_some());
    assert!(h.tree.is_exiting(leaving));
    assert_eq!(h.tree.rect(leaving).unwrap(), before);
    let third = h.node(3);
    assert!(h.tree.rect(third).unwrap().y < before.y + 2.0 * before.h);

    h.sample(1.1);
    assert_eq!(
        h.value(leaving, AnimatedProperty::Opacity),
        MotionValue::Scalar(0.5)
    );
    assert!(h.tree.get(leaving).is_some());
    assert!(h.removed.is_empty());

    h.sample(1.3);
    assert_eq!(h.removed, vec![leaving]);
    assert!(h.tree.get(leaving).is_none());
    assert!(h.tree.exiting_nodes().is_empty());
    assert!(h
        .runtime
        .track_of(leaving, AnimatedProperty::Opacity)
        .is_none());
    assert_eq!(h.app.len(), 0);
}

#[test]
fn nodes_never_unmount_above_zero_opacity() {
    let mut h = Harness::new(MotionTheme::default());
    let row = Row {
        presence: Presence::new().exit(FADE),
        ..Row::new(1)
    };
    h.render(std::slice::from_ref(&row), 0.0);
    let node = h.node(1);
    h.render(&[], 1.0);
    let track = h.runtime.track_of(node, AnimatedProperty::Opacity).unwrap();
    for step in 1..=30 {
        let at = 1.0 + f64::from(step) * 0.01;
        h.sample(at);
        if h.tree.get(node).is_none() {
            break;
        }
        let MotionValue::Scalar(opacity) = h.tracks.value(track).unwrap() else {
            panic!("scalar")
        };
        assert!(opacity > 0.0, "still mounted at opacity {opacity} (t={at})");
    }
    assert!(h.tree.get(node).is_none());
}

#[test]
fn key_returning_mid_exit_revives_from_the_presented_state() {
    let mut h = Harness::new(MotionTheme::default());
    let row = || Row {
        presence: Presence::new().enter(FADE).exit(FADE),
        ..Row::new(1)
    };
    h.render(&[row()], 0.0);
    let node = h.node(1);
    h.render(&[], 1.0);
    h.sample(1.1);
    let track = h.runtime.track_of(node, AnimatedProperty::Opacity).unwrap();
    assert_eq!(h.tracks.value(track), Some(MotionValue::Scalar(0.5)));

    h.render(&[row()], 1.1);
    assert_eq!(h.node(1), node);
    assert!(!h.tree.is_exiting(node));
    h.sample(1.1);
    assert_eq!(h.tracks.value(track), Some(MotionValue::Scalar(0.5)));
    h.sample(1.2);
    assert_eq!(h.tracks.value(track), Some(MotionValue::Scalar(0.75)));
    h.sample(1.5);
    assert_eq!(h.tracks.value(track), Some(MotionValue::Scalar(1.0)));
    assert!(h.removed.is_empty());
    assert!(h.tree.get(node).is_some());
}

#[test]
fn exit_without_declared_motion_removes_immediately() {
    let mut h = Harness::new(MotionTheme::default());
    h.render(&[Row::new(1), Row::new(2)], 0.0);
    let gone = h.node(1);
    h.render(&[Row::new(2)], 1.0);
    assert!(h.tree.get(gone).is_none());
}

#[test]
fn theme_token_change_retargets_in_flight_tracks() {
    let slow = MotionTheme::default().with(SLOW, Motion::tween(1.0, Easing::Linear));
    let fast = MotionTheme::default().with(SLOW, Motion::tween(0.1, Easing::Linear));
    let mut h = Harness::new(slow);
    let mut row = Row {
        opacity: Some(1.0),
        animate: Animate::new().opacity(SLOW),
        ..Row::new(1)
    };
    h.render(std::slice::from_ref(&row), 0.0);
    row.opacity = Some(0.0);
    h.render(std::slice::from_ref(&row), 1.0);
    h.sample(1.2);
    let node = h.node(1);
    let track = h.runtime.track_of(node, AnimatedProperty::Opacity).unwrap();
    let MotionValue::Scalar(before) = h.tracks.value(track).unwrap() else {
        panic!()
    };
    assert!((before - 0.8).abs() < 1e-4);

    h.runtime.set_theme(&mut h.app, fast, 1.2);
    let commands = h.app.drain_commands();
    assert_eq!(commands.len(), 1);
    let MotionCommand::SetTarget { to, kind, .. } = commands[0] else {
        panic!("retarget")
    };
    assert_eq!(to, MotionValue::Scalar(0.0));
    assert_eq!(
        kind,
        MotionKind::Tween {
            duration_secs: 0.1,
            easing: Easing::Linear
        }
    );
    for command in commands {
        h.tracks.apply(command);
    }
    h.sample(1.2);
    assert_eq!(h.tracks.value(track), Some(MotionValue::Scalar(before)));
    h.sample(1.4);
    assert_eq!(h.tracks.value(track), Some(MotionValue::Scalar(0.0)));
}

#[test]
fn literal_motions_ignore_theme_changes() {
    let mut h = Harness::new(MotionTheme::default());
    let mut row = Row {
        opacity: Some(1.0),
        animate: Animate::new().opacity(CARD),
        ..Row::new(1)
    };
    h.render(std::slice::from_ref(&row), 0.0);
    row.opacity = Some(0.0);
    h.render(std::slice::from_ref(&row), 1.0);
    h.app.drain_commands();
    h.runtime
        .set_theme(&mut h.app, MotionTheme::default().with(SLOW, FADE), 1.0);
    assert!(h.app.drain_commands().is_empty());
}

#[test]
fn builder_methods_carry_motion_through_lowering() {
    use aqua_ui::element::{lower_specs, Box as UiBox};
    let element = UiBox::new()
        .fill(red())
        .animate(Animate::new().fill(CARD))
        .presence(Presence::new().exit(FADE))
        .build()
        .with_key(7u32);
    let specs = lower_specs(&element).unwrap();
    let NodeKind::Box(content) = &specs[0].kind else {
        panic!("box")
    };
    let spec = content.paint.motion.as_ref().expect("motion on box paint");
    assert_eq!(
        spec.animate.motion_of(AnimatedProperty::Fill),
        Some(CARD.into())
    );
    assert_eq!(spec.presence.exit_motion(), Some(FADE.into()));
    assert_eq!(spec.animate.motion_of(AnimatedProperty::Opacity), None);
}
