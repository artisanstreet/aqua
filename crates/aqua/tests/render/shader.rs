use aqua::render::lower::Lowering;
use aqua_core::shader::{FragmentShader, ReferenceShader};
use aqua_core::{DisplayItem, Rect, Size, Transform2D};
use aqua_graph::{PaintCommand, PassKind, PlanLimits};
use aqua_protocol::Instant;
use aqua_scene::Scene;
use aqua_text::{SharedFonts, SharedRuns};
use std::sync::Arc;

const SIZE: Size = Size { w: 200.0, h: 100.0 };

fn sample(parameters: &[f32], uv: [f32; 2]) -> [f32; 4] {
    [uv[0], uv[1], parameters[2].sin().abs(), 1.0]
}

fn shader(animated: bool) -> DisplayItem {
    DisplayItem::Shader {
        bounds: Rect { x: 10.0, y: 12.0, w: 80.0, h: 50.0 },
        program: Arc::new(FragmentShader {
            name: "test.coordinates".into(),
            hlsl: "cbuffer Parameters : register(b0) { float4 p; }; struct Input { float4 position : SV_Position; float2 uv : TEXCOORD0; }; float4 shade(Input input) : SV_Target { return float4(input.uv, abs(sin(p.z)), 1.0); }".into(),
            entry: "shade".into(),
            reference: Some(ReferenceShader(sample)),
        }),
        parameters: Arc::from([0.75, 0.0, 0.0, 0.0]),
        animated,
    }
}

fn scene(animated: bool) -> Scene {
    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushTransform(Transform2D::translate(4.0, 7.0)));
    scene.insert(shader(animated));
    scene.insert(DisplayItem::PopTransform);
    scene
}

#[test]
fn retained_shaders_advance_without_scene_updates_and_stop_when_removed() {
    let scene = scene(true);
    let mut lowering = Lowering::new(SharedFonts::new(), SharedRuns::new());
    for seconds in [1.0, 1.5, 2.0, 2.5] {
        let (commands, _, report) =
            lowering.commands(&scene, SIZE, 2.0, Instant::from_secs_f64(seconds));
        let spec = commands
            .iter()
            .find_map(|command| match command {
                PaintCommand::PushCustomEffect { spec, .. } => Some(spec),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            &spec.params[..4],
            &[160.0, 100.0, seconds as f32, 2.0],
            "physical size and render clock reach every retained frame"
        );
        assert_eq!(spec.bounds.x, 28.0, "the parent transform is applied");
        assert_eq!(spec.bounds.y, 38.0, "the parent transform is applied");
        assert_eq!(
            spec.params[4], 0.75,
            "application uniforms follow frame uniforms"
        );
        assert!(
            report.animated(),
            "the render thread schedules another frame"
        );
    }
    let (_, _, report) = lowering.commands(&Scene::new(), SIZE, 2.0, Instant::from_secs_f64(3.0));
    assert!(
        !report.animated(),
        "removing the artwork stops render scheduling"
    );
}

#[test]
fn reduced_motion_freezes_time_and_does_not_schedule_frames() {
    let mut lowering = Lowering::new(SharedFonts::new(), SharedRuns::new());
    let scene = scene(false);
    let (first, _, first_report) =
        lowering.commands(&scene, SIZE, 1.0, Instant::from_secs_f64(1.0));
    let (later, _, later_report) =
        lowering.commands(&scene, SIZE, 1.0, Instant::from_secs_f64(5.0));
    assert_eq!(first, later, "static artwork stays identical over time");
    assert!(
        !first_report.animated() && !later_report.animated(),
        "static artwork settles"
    );
}

#[test]
fn procedural_passes_need_no_input_texture_and_render_in_reference_backend() {
    let mut lowering = Lowering::new(SharedFonts::new(), SharedRuns::new());
    let lowered = lowering
        .lower(&scene(false), SIZE, 1.0, PlanLimits::default())
        .unwrap();
    let pass = lowered
        .plan
        .passes
        .iter()
        .find(|pass| matches!(pass.kind, PassKind::Custom(_)))
        .unwrap();
    assert!(
        pass.reads.is_empty(),
        "procedural content does not allocate an input texture"
    );
    for mut painter in super::support::painters(&SharedFonts::new(), &SharedRuns::new()) {
        let frame = painter.paint(&scene(false), SIZE, 1.0);
        let corner = frame.pixel(15, 20);
        let opposite = frame.pixel(90, 65);
        assert!(
            opposite[0] > corner[0] && opposite[1] > corner[1],
            "shader UVs cover the local viewport: {}",
            painter.label
        );
        assert_eq!(opposite[3], 255, "the shader composites opaquely");
    }
}

#[test]
fn animation_damage_is_confined_to_the_shader_and_stops_for_static_artwork() {
    use aqua::render::damage::{DamagePlanner, Plan};
    let runs = SharedRuns::new();
    for animated in [true, false] {
        let scene = scene(animated);
        let mut planner = DamagePlanner::new();
        planner.plan(&scene, &runs, SIZE, 2.0, true);
        planner.commit();
        let plan = planner.plan(&scene, &runs, SIZE, 2.0, true);
        let Plan::Regions(regions) = plan else {
            panic!("an unchanged scene must not damage the whole window");
        };
        let area: f32 = regions.iter().map(|r| r.w * r.h).sum();
        assert_eq!(
            area > 1.0,
            animated,
            "static artwork leaves only the planner's one-pixel idle damage"
        );
        assert!(
            regions
                .iter()
                .all(|r| r.w < SIZE.w * 2.0 && r.h < SIZE.h * 2.0),
            "damage stays inside the artwork region"
        );
    }
}

#[test]
fn shader_cadence_yields_to_interactive_motion_and_settles_when_removed() {
    use aqua::render::host::Repaint;
    use aqua::render::lower::LowerReport;
    use std::time::Duration;

    let mut lowering = Lowering::new(SharedFonts::new(), SharedRuns::new());
    let (_, _, mut report) =
        lowering.commands(&scene(true), SIZE, 1.0, Instant::from_secs_f64(0.0));
    assert_eq!(
        Repaint::from_report(&report, false, 1.0),
        Repaint::Still(Duration::from_nanos(33_333_333))
    );
    assert_eq!(Repaint::from_report(&report, true, 1.0), Repaint::NextFrame);
    report.pulsing = true;
    assert_eq!(Repaint::from_report(&report, false, 1.0), Repaint::Heavy);
    report.pulsing = false;
    report.shimmering = true;
    assert_eq!(
        Repaint::from_report(&report, false, 0.1),
        Repaint::Animating
    );
    assert_eq!(
        Repaint::from_report(&LowerReport::default(), false, 1.0),
        Repaint::Settled
    );
}

#[test]
fn deterministic_capture_freezes_artwork_while_frame_time_keeps_advancing() {
    let scene = scene(true);
    let mut lowering = Lowering::new(SharedFonts::new(), SharedRuns::new()).with_frozen_shaders(Instant::from_secs_f64(1.25));
    let mut previous = None;
    for seconds in [2.0, 5.0, 20.0] {
        let (commands, _, report) = lowering.commands(&scene, SIZE, 1.0, Instant::from_secs_f64(seconds));
        let time = commands.iter().find_map(|command| match command { PaintCommand::PushCustomEffect { spec, .. } => Some(spec.params[2]), _ => None }).unwrap();
        assert_eq!(time, 1.25);
        assert!(!report.animated());
        if let Some(previous) = &previous { assert_eq!(&commands, previous); }
        previous = Some(commands);
    }
}
