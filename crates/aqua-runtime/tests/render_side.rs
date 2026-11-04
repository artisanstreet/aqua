use aqua_core::{DisplayItem, NodeId, Oklch, Rect};
use aqua_motion::{AppMotion, Binding, Easing, MotionKind, MotionValue, Property, SpringParams};
use aqua_protocol::{
    Feedback, Instant, RenderCommand, RenderEvent, RenderInput, RenderOutput, RenderWake,
};
use aqua_runtime::{emit_scene_diff, RenderSide};
use aqua_scene::Scene;

fn red() -> Oklch {
    Oklch::from_hex("#ff0000").unwrap()
}

fn blue() -> Oklch {
    Oklch::from_hex("#0000ff").unwrap()
}

fn quad(x: f32, fill: Oklch) -> DisplayItem {
    DisplayItem::Rect {
        bounds: Rect {
            x,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        },
        radius: 0.0,
        fill,
        border_width: 0.0,
        border_color: red(),
        shadow: None,
    }
}

fn at(secs: f64) -> Instant {
    Instant::from_secs_f64(secs)
}

fn input(commands: Vec<RenderCommand>, secs: f64) -> RenderInput {
    RenderInput {
        commands,
        feedback: Vec::new(),
        present_at: at(secs),
    }
}

fn fill_of(item: &DisplayItem) -> Oklch {
    match item {
        DisplayItem::Rect { fill, .. } => *fill,
        other => panic!("not a rect: {other:?}"),
    }
}

fn x_of(item: &DisplayItem) -> f32 {
    match item {
        DisplayItem::Rect { bounds, .. } => bounds.x,
        other => panic!("not a rect: {other:?}"),
    }
}

struct Pair {
    app_scene: Scene,
    app: AppMotion,
    render: RenderSide,
}

impl Pair {
    fn new() -> Self {
        Self {
            app_scene: Scene::new(),
            app: AppMotion::default(),
            render: RenderSide::new(),
        }
    }

    fn sync(&mut self, secs: f64) -> RenderOutput {
        let mut commands = Vec::new();
        let diff = emit_scene_diff(&mut self.app_scene);
        if !diff.is_empty() {
            commands.push(RenderCommand::Scene(diff));
        }
        commands.extend(
            self.app
                .drain_commands()
                .into_iter()
                .map(RenderCommand::Motion),
        );
        self.deliver(commands, secs)
    }

    fn deliver(&mut self, commands: Vec<RenderCommand>, secs: f64) -> RenderOutput {
        let output = self.render.step(input(commands, secs));
        for event in &output.events {
            if let RenderEvent::Motion(event) = event {
                self.app.handle(*event);
            }
        }
        output
    }
}

#[test]
fn scene_diff_mirrors_the_app_scene_in_order() {
    let mut pair = Pair::new();
    let ids: Vec<NodeId> = (0..5)
        .map(|i| pair.app_scene.insert(quad(i as f32, red())))
        .collect();
    let output = pair.sync(0.0);
    assert!(output.frame.is_some());
    let mirrored: Vec<f32> = pair
        .render
        .scene()
        .ordered()
        .map(|(_, i)| x_of(i))
        .collect();
    assert_eq!(mirrored, vec![0.0, 1.0, 2.0, 3.0, 4.0]);

    pair.app_scene.remove(ids[1]);
    pair.app_scene.update(ids[3], quad(33.0, red()));
    pair.sync(0.016);
    let mirrored: Vec<f32> = pair
        .render
        .scene()
        .ordered()
        .map(|(_, i)| x_of(i))
        .collect();
    assert_eq!(mirrored, vec![0.0, 2.0, 33.0, 4.0]);

    let reordered = [ids[4], ids[0], ids[2], ids[3]];
    pair.app_scene.set_order(&reordered);
    pair.sync(0.032);
    let mirrored: Vec<f32> = pair
        .render
        .scene()
        .ordered()
        .map(|(_, i)| x_of(i))
        .collect();
    assert_eq!(mirrored, vec![4.0, 0.0, 2.0, 33.0]);
}

#[test]
fn quiet_steps_submit_no_frame_and_report_idle() {
    let mut pair = Pair::new();
    pair.app_scene.insert(quad(0.0, red()));
    assert!(pair.sync(0.0).frame.is_some());
    let output = pair.sync(0.016);
    assert_eq!(output.frame, None);
    assert_eq!(output.wake, RenderWake::Idle);
}

#[test]
fn bound_fill_animates_on_the_render_side_while_the_app_scene_is_untouched() {
    let mut pair = Pair::new();
    let id = pair.app_scene.insert(quad(0.0, red()));
    let track = pair.app.spawn(
        None,
        Some(Binding {
            node: id,
            property: Property::Fill,
        }),
        MotionValue::Color(red()),
        MotionKind::Tween {
            duration_secs: 1.0,
            easing: Easing::Linear,
        },
        0.0,
    );
    pair.sync(0.0);
    pair.app
        .set_target(track, MotionValue::Color(blue()), 0.0)
        .unwrap();
    pair.sync(0.0);
    let revision = pair.app_scene.revision();
    for step in 1..=50 {
        let secs = f64::from(step) / 60.0;
        let output = pair.sync(secs);
        assert_eq!(pair.app_scene.revision(), revision);
        assert_eq!(fill_of(pair.app_scene.get(id).unwrap()), red());
        assert!(output.frame.is_some());
        assert_eq!(output.wake, RenderWake::NextFrame);
        let shown = fill_of(pair.render.presented(id).unwrap());
        let expected = red().lerp(blue(), (secs as f32).min(1.0));
        assert!(
            (shown.l - expected.l).abs() < 1e-3,
            "{shown:?} {expected:?}"
        );
    }
    let output = pair.sync(1.1);
    assert_eq!(fill_of(pair.render.presented(id).unwrap()), blue());
    assert!(output
        .events
        .iter()
        .any(|e| matches!(e, RenderEvent::Motion(_))));
    assert!(pair.app.is_settled(track));
    assert_eq!(pair.sync(1.2).wake, RenderWake::Idle);
}

#[test]
fn app_updates_to_an_animating_node_keep_the_presented_override() {
    let mut pair = Pair::new();
    let id = pair.app_scene.insert(quad(0.0, red()));
    let track = pair.app.spawn(
        None,
        Some(Binding {
            node: id,
            property: Property::Edge(aqua_motion::Edge::Left),
        }),
        MotionValue::Scalar(0.0),
        MotionKind::Spring(SpringParams::SNAPPY),
        0.0,
    );
    pair.sync(0.0);
    pair.app
        .set_target(track, MotionValue::Scalar(100.0), 0.0)
        .unwrap();
    pair.sync(0.0);
    pair.sync(0.05);
    pair.app_scene.update(id, quad(0.0, blue()));
    pair.sync(0.06);
    let item = pair.render.presented(id).unwrap();
    assert_eq!(fill_of(item), blue());
    assert!(x_of(item) > 0.0 && x_of(item) < 100.0);
}

#[test]
fn removing_the_track_restores_the_authored_item() {
    let mut pair = Pair::new();
    let id = pair.app_scene.insert(quad(0.0, red()));
    let track = pair.app.spawn(
        None,
        Some(Binding {
            node: id,
            property: Property::Fill,
        }),
        MotionValue::Color(blue()),
        MotionKind::Instant,
        0.0,
    );
    pair.sync(0.0);
    assert_eq!(fill_of(pair.render.presented(id).unwrap()), blue());
    pair.app.remove(track);
    pair.sync(0.016);
    assert_eq!(fill_of(pair.render.presented(id).unwrap()), red());
}

#[test]
fn device_recovery_forces_a_full_resubmission() {
    let mut pair = Pair::new();
    pair.app_scene.insert(quad(0.0, red()));
    pair.sync(0.0);
    let output = pair.render.step(RenderInput {
        commands: Vec::new(),
        feedback: vec![Feedback::DeviceLost, Feedback::DeviceRecovered],
        present_at: at(0.1),
    });
    assert_eq!(
        output.events,
        vec![RenderEvent::DeviceLost, RenderEvent::DeviceRecovered]
    );
    assert!(output.frame.is_some());
}

fn script() -> Vec<RenderInput> {
    let mut scene = Scene::new();
    let mut app = AppMotion::default();
    let a = scene.insert(quad(0.0, red()));
    let b = scene.insert(quad(5.0, blue()));
    let fill = app.spawn(
        None,
        Some(Binding {
            node: a,
            property: Property::Fill,
        }),
        MotionValue::Color(red()),
        MotionKind::Spring(SpringParams::SMOOTH),
        0.0,
    );
    let edge = app.spawn(
        None,
        Some(Binding {
            node: b,
            property: Property::Edge(aqua_motion::Edge::Right),
        }),
        MotionValue::Scalar(15.0),
        MotionKind::Tween {
            duration_secs: 0.3,
            easing: Easing::EaseOut,
        },
        0.0,
    );
    let mut inputs = Vec::new();
    let mut commands = vec![RenderCommand::Scene(emit_scene_diff(&mut scene))];
    commands.extend(app.drain_commands().into_iter().map(RenderCommand::Motion));
    inputs.push(input(commands, 0.0));
    app.set_target(fill, MotionValue::Color(blue()), 0.0)
        .unwrap();
    app.set_target(edge, MotionValue::Scalar(80.0), 0.0)
        .unwrap();
    inputs.push(input(
        app.drain_commands()
            .into_iter()
            .map(RenderCommand::Motion)
            .collect(),
        0.0,
    ));
    for step in 1..=90 {
        let secs = f64::from(step) / 60.0;
        if step == 20 {
            app.set_target(fill, MotionValue::Color(red()), secs)
                .unwrap();
        }
        inputs.push(input(
            app.drain_commands()
                .into_iter()
                .map(RenderCommand::Motion)
                .collect(),
            secs,
        ));
    }
    inputs
}

fn run(inputs: &[RenderInput]) -> (Vec<RenderOutput>, Vec<String>) {
    let mut side = RenderSide::new();
    let mut outputs = Vec::new();
    let mut states = Vec::new();
    for input in inputs {
        outputs.push(side.step(input.clone()));
        states.push(format!(
            "{:?}",
            side.scene()
                .ordered()
                .map(|(_, i)| i.clone())
                .collect::<Vec<_>>()
        ));
    }
    (outputs, states)
}

#[test]
fn replaying_the_same_inputs_reproduces_outputs_and_presented_scenes_exactly() {
    let inputs = script();
    let first = run(&inputs);
    let second = run(&inputs);
    assert_eq!(first, second);
    let animated_frames = first.0.iter().filter(|o| o.frame.is_some()).count();
    assert!(animated_frames > 30);
}

#[test]
fn stepping_in_one_batch_equals_stepping_frame_by_frame_at_the_same_times() {
    let inputs = script();
    let (_, states) = run(&inputs);
    let mut side = RenderSide::new();
    let mut merged = inputs[0].clone();
    merged.commands.extend(inputs[1].commands.clone());
    side.step(merged);
    for input in &inputs[2..] {
        side.step(input.clone());
    }
    let final_state = format!(
        "{:?}",
        side.scene()
            .ordered()
            .map(|(_, i)| i.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(&final_state, states.last().unwrap());
}

#[test]
fn a_viewport_change_alone_requests_a_frame_and_an_unchanged_one_does_not() {
    let mut render = RenderSide::new();
    let viewport = |w: f32| RenderCommand::Viewport {
        size: aqua_core::Size::new(w, 100.0),
        scale: 1.0,
    };
    assert!(render
        .step(input(vec![viewport(200.0)], 0.0))
        .frame
        .is_some());
    assert!(render.step(input(vec![], 0.016)).frame.is_none());
    assert!(render
        .step(input(vec![viewport(200.0)], 0.032))
        .frame
        .is_none());
    assert!(render
        .step(input(vec![viewport(300.0)], 0.048))
        .frame
        .is_some());
}
