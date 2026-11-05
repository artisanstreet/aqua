use aqua::render::lower::Lowering;
use aqua_core::geometry::{CornerRadii, Insets, Rect};
use aqua_core::scene::{DisplayItem, GpuiQuadPulse};
use aqua_core::{Oklch, Size};
use aqua_graph::PaintCommand;
use aqua_protocol::Instant;
use aqua_scene::Scene;
use aqua_text::{SharedFonts, SharedRuns};

const SIZE: Size = Size { w: 200.0, h: 100.0 };

fn pulsing_scene() -> Scene {
    let mut scene = Scene::new();
    [
        DisplayItem::PushGpuiQuadPulse(GpuiQuadPulse {
            start_ms: 123_456,
            period_ms: 1600,
            min_opacity: 0.4,
        }),
        DisplayItem::Quad {
            bounds: Rect {
                x: 10.0,
                y: 10.0,
                w: 50.0,
                h: 50.0,
            },
            corners: CornerRadii::default(),
            fill: Oklch::from_hex("#ffffff").unwrap(),
            border_widths: Insets::uniform(0.0),
            border_color: Oklch::TRANSPARENT,
            dashed: false,
        },
        DisplayItem::PopGpuiQuadPulse,
    ]
    .into_iter()
    .for_each(|item| {
        scene.insert(item);
    });
    scene
}

fn lowering() -> Lowering {
    Lowering::new(SharedFonts::new(), SharedRuns::new())
}

fn alpha_at(lowering: &mut Lowering, scene: &Scene, seconds: f64) -> f32 {
    let (commands, _, _) = lowering.commands(scene, SIZE, 1.0, Instant::from_secs_f64(seconds));
    commands
        .iter()
        .find_map(|command| match command {
            PaintCommand::Quad(quad) => Some(quad.fill.alpha),
            _ => None,
        })
        .expect("a quad")
}

#[test]
fn a_pulse_starts_at_full_opacity_whenever_the_render_thread_first_sees_it() {
    let scene = pulsing_scene();
    [0.0, 17.3, 4000.25].into_iter().for_each(|first| {
        let mut lowering = lowering();
        assert!((alpha_at(&mut lowering, &scene, first) - 1.0).abs() < 1e-6);
        assert!((alpha_at(&mut lowering, &scene, first + 0.8) - 0.4).abs() < 1e-4);
        assert!((alpha_at(&mut lowering, &scene, first + 1.6) - 1.0).abs() < 1e-4);
    });
}

#[test]
fn the_same_times_give_the_same_commands_on_any_wall_clock() {
    let scene = pulsing_scene();
    let times: Vec<f64> = (0..40).map(|step| 5.0 + f64::from(step) * 0.0061).collect();
    let run = || {
        let mut lowering = lowering();
        times
            .iter()
            .map(|seconds| {
                lowering
                    .commands(&scene, SIZE, 1.0, Instant::from_secs_f64(*seconds))
                    .0
            })
            .collect::<Vec<_>>()
    };
    let first = run();
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert_eq!(first, run());
}

#[test]
fn pulsing_is_reported_while_a_pulsed_quad_exists_and_stops_when_it_goes() {
    let scene = pulsing_scene();
    let mut lowering = lowering();
    let (_, _, report) = lowering.commands(&scene, SIZE, 1.0, Instant::from_secs_f64(1.0));
    assert!(report.pulsing);
    let (_, _, report) = lowering.commands(&Scene::new(), SIZE, 1.0, Instant::from_secs_f64(1.1));
    assert!(!report.pulsing);
    assert!(!report.shimmering);
}

#[test]
fn a_pulse_that_left_the_scene_restarts_from_full_opacity_when_it_returns() {
    let scene = pulsing_scene();
    let mut lowering = lowering();
    alpha_at(&mut lowering, &scene, 0.0);
    assert!((alpha_at(&mut lowering, &scene, 0.8) - 0.4).abs() < 1e-4);
    lowering.commands(&Scene::new(), SIZE, 1.0, Instant::from_secs_f64(0.9));
    assert!((alpha_at(&mut lowering, &pulsing_scene(), 1.0) - 1.0).abs() < 1e-6);
}
