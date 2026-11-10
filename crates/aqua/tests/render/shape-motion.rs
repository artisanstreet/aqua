use aqua::render::lower::Lowering;
use aqua::render::shape_motion::Shapes;
use aqua_core::shape::{ShapeProgram, ShapeTransition};
use aqua_core::{CornerRadii, DisplayItem, Insets, Oklch, Rect, Size};
use aqua_graph::PaintCommand;
use aqua_protocol::Instant;
use aqua_scene::Scene;
use aqua_text::{SharedFonts, SharedRuns};
use std::sync::Arc;
use std::time::Duration;

fn paint(_: Rect, target: Rect, _: &[f32]) -> Vec<DisplayItem> {
    vec![DisplayItem::Quad {
        bounds: target,
        corners: CornerRadii::default(),
        fill: Oklch::WHITE,
        border_widths: Insets::uniform(0.0),
        border_color: Oklch::TRANSPARENT,
        dashed: false,
    }]
}

fn program() -> Arc<ShapeProgram> {
    Arc::new(ShapeProgram {
        name: "test.morph".into(),
        parameters: Arc::from([]),
        paint,
    })
}

fn transition() -> ShapeTransition {
    ShapeTransition {
        duration: Duration::from_secs(1),
        easing: [0.0, 0.0, 1.0, 1.0],
        vertical_reveal: false,
    }
}

fn rect(x: f32) -> Rect {
    Rect {
        x,
        y: 10.0,
        w: 20.0,
        h: 20.0,
    }
}

fn x(commands: &[DisplayItem]) -> f32 {
    match commands[0] {
        DisplayItem::Quad { bounds, .. } => bounds.x,
        _ => panic!("quad"),
    }
}

#[test]
fn morphs_retarget_continuously_and_stop_without_application_frames() {
    let mut shapes = Shapes::default();
    let program = program();
    let bounds = rect(0.0);
    assert!(
        !shapes
            .paint((1, 0), bounds, &program, rect(0.0), transition(), 10.0)
            .1
    );
    assert!(
        shapes
            .paint((1, 0), bounds, &program, rect(100.0), transition(), 11.0)
            .1
    );
    let middle = shapes.paint((1, 0), bounds, &program, rect(100.0), transition(), 11.5);
    assert!((x(&middle.0) - 50.0).abs() < 0.001);
    let retargeted = shapes.paint((1, 0), bounds, &program, rect(0.0), transition(), 11.5);
    assert_eq!(x(&middle.0), x(&retargeted.0));
    assert!(
        (x(&shapes
            .paint((1, 0), bounds, &program, rect(0.0), transition(), 12.0)
            .0)
            - 25.0)
            .abs()
            < 0.001
    );
    let settled = shapes.paint((1, 0), bounds, &program, rect(0.0), transition(), 12.5);
    assert_eq!(x(&settled.0), 0.0);
    assert!(!settled.1);
}

#[test]
fn reduced_motion_resize_and_removal_discard_obsolete_motion() {
    let mut shapes = Shapes::default();
    let program = program();
    shapes.paint((1, 0), rect(0.0), &program, rect(0.0), transition(), 1.0);
    shapes.paint((1, 0), rect(0.0), &program, rect(100.0), transition(), 2.0);
    let immediate = ShapeTransition {
        duration: Duration::ZERO,
        ..transition()
    };
    let reduced = shapes.paint((1, 0), rect(0.0), &program, rect(100.0), immediate, 2.2);
    assert_eq!(x(&reduced.0), 100.0);
    assert!(!reduced.1);
    let resized = shapes.paint((1, 0), rect(1.0), &program, rect(20.0), transition(), 2.3);
    assert_eq!(x(&resized.0), 20.0);
    assert!(!resized.1);
    shapes.begin_frame();
    shapes.end_frame();
    let remounted = shapes.paint((1, 0), rect(1.0), &program, rect(40.0), transition(), 2.4);
    assert_eq!(x(&remounted.0), 40.0);
    assert!(!remounted.1);
}

#[test]
fn retained_scene_morphs_repaint_until_settled_then_removal_stops() {
    let size = Size { w: 200.0, h: 100.0 };
    let shape = |target| DisplayItem::Shape {
        bounds: Rect {
            x: 0.0,
            y: 0.0,
            w: size.w,
            h: size.h,
        },
        damage: rect(0.0),
        program: program(),
        target: rect(target),
        transition: transition(),
    };
    let mut scene = Scene::new();
    let id = scene.insert(shape(0.0));
    let mut lower = Lowering::new(SharedFonts::new(), SharedRuns::new());
    lower.commands(&scene, size, 1.0, Instant::from_secs_f64(1.0));
    scene.update(id, shape(100.0));
    lower.commands(&scene, size, 1.0, Instant::from_secs_f64(2.0));
    let (commands, _, report) = lower.commands(&scene, size, 1.0, Instant::from_secs_f64(2.5));
    let position = commands
        .iter()
        .find_map(|command| match command {
            PaintCommand::Quad(quad) => Some(quad.bounds.x),
            _ => None,
        })
        .unwrap();
    assert!((position - 50.0).abs() < 0.01);
    assert!(report.shape_animation && report.animated());
    assert!(!lower
        .commands(&scene, size, 1.0, Instant::from_secs_f64(3.0))
        .2
        .animated());
    scene.remove(id);
    assert!(!lower
        .commands(&scene, size, 1.0, Instant::from_secs_f64(4.0))
        .2
        .animated());
}

#[test]
fn a_settled_collapsed_shape_reopens_at_the_new_target() {
    let mut shapes = Shapes::default();
    let program = program();
    let collapsed = Rect {
        h: 0.0,
        ..rect(0.0)
    };
    let reveal = ShapeTransition {
        vertical_reveal: true,
        ..transition()
    };
    shapes.paint((1, 0), rect(0.0), &program, collapsed, reveal, 1.0);
    let opened = shapes.paint((1, 0), rect(0.0), &program, rect(100.0), reveal, 2.0);
    let DisplayItem::Quad { bounds, .. } = opened.0[0] else {
        panic!("quad")
    };
    assert_eq!(
        bounds,
        Rect {
            x: 100.0,
            y: 30.0,
            w: 20.0,
            h: 0.0
        }
    );
    let halfway = shapes.paint((1, 0), rect(0.0), &program, rect(100.0), reveal, 2.5);
    let DisplayItem::Quad { bounds, .. } = halfway.0[0] else {
        panic!("quad")
    };
    assert_eq!(bounds.x, 100.0);
    assert!((bounds.h - 10.0).abs() < 0.001);
}
