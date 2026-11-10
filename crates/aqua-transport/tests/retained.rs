#[path = "../src/retained.rs"]
mod retained;

use aqua_core::{Oklch, Rect};
use aqua_graph::{
    plan_paint_with, CustomPassSpec, Extent, PaintCommand, PlanLimits, Quad, ToneCurve,
};
use retained::RetainedPlan;

fn block(x: f32) -> PaintCommand {
    PaintCommand::Quad(Quad {
        bounds: Rect {
            x,
            y: 0.0,
            w: 20.0,
            h: 20.0,
        },
        radius: 3.0,
        border_width: 0.0,
        fill: Oklch::new(0.6, 0.1, 40.0),
        border_color: Oklch::TRANSPARENT,
    })
}

fn shader(time: f32) -> Vec<PaintCommand> {
    let bounds = Rect {
        x: 10.0,
        y: 10.0,
        w: 24.0,
        h: 24.0,
    };
    vec![
        PaintCommand::PushCustomEffect {
            bounds,
            spec: CustomPassSpec {
                isolate_backdrop: false,
                reference: None,
                name: "retained.test".into(),
                hlsl: "float4 main() : SV_Target { return 1.0; }".into(),
                entry: "main".into(),
                bounds,
                params: vec![time, 0.0, 0.0, 0.0],
            },
        },
        PaintCommand::PopCustomEffect,
    ]
}

fn store(cache: &mut RetainedPlan, commands: &[PaintCommand]) {
    let plan = plan_paint_with(
        Extent::new(64, 64),
        Oklch::BLACK,
        commands,
        PlanLimits::default(),
    )
    .unwrap();
    cache.store(commands, plan, PlanLimits::default());
}

fn reuse<'a>(
    cache: &'a mut RetainedPlan,
    commands: &[PaintCommand],
) -> Option<&'a aqua_graph::FramePlan> {
    cache.reuse(
        Extent::new(64, 64),
        Oklch::BLACK,
        ToneCurve::default(),
        PlanLimits::default(),
        commands,
    )
}

#[test]
fn repeated_frames_reuse_the_same_plan_allocation_and_geometry_changes_invalidate_it() {
    let commands = [block(0.0), block(30.0)];
    let mut cache = RetainedPlan::default();
    store(&mut cache, &commands);
    let pointer = cache.get().unwrap().passes.as_ptr();
    for _ in 0..100 {
        assert_eq!(
            reuse(&mut cache, &commands).unwrap().passes.as_ptr(),
            pointer
        );
    }
    assert!(reuse(&mut cache, &[block(1.0), block(30.0)]).is_none());
    assert!(reuse(&mut cache, &[block(0.0)]).is_none());
    cache.clear();
    assert!(reuse(&mut cache, &commands).is_none());
}

#[test]
fn changing_only_shader_uniforms_matches_a_fresh_plan_at_each_time() {
    let mut commands = vec![block(0.0)];
    commands.extend(shader(0.0));
    let mut cache = RetainedPlan::default();
    store(&mut cache, &commands);
    let pointer = cache.get().unwrap().passes.as_ptr();
    for time in [0.01, 0.5, 100.0, 1_000_000.0] {
        let mut commands = vec![block(0.0)];
        commands.extend(shader(time));
        let expected = plan_paint_with(
            Extent::new(64, 64),
            Oklch::BLACK,
            &commands,
            PlanLimits::default(),
        )
        .unwrap();
        let actual = reuse(&mut cache, &commands).unwrap();
        assert_eq!(actual, &expected);
        assert_eq!(actual.passes.as_ptr(), pointer);
    }
}

#[test]
fn ambiguous_duplicate_programs_and_changed_shader_contracts_are_replanned() {
    let mut commands = shader(0.0);
    commands.extend(shader(0.0));
    let mut cache = RetainedPlan::default();
    store(&mut cache, &commands);
    assert!(reuse(&mut cache, &commands).is_some());
    commands.splice(0..2, shader(1.0));
    assert!(reuse(&mut cache, &commands).is_none());

    store(&mut cache, &shader(0.0));
    let mut changed = shader(1.0);
    let PaintCommand::PushCustomEffect { spec, .. } = &mut changed[0] else {
        unreachable!()
    };
    spec.hlsl.push_str("\n");
    assert!(reuse(&mut cache, &changed).is_none());
}

#[test]
fn resizing_theme_text_tone_and_backend_limits_invalidate_retained_plans() {
    let commands = [block(0.0)];
    let mut cache = RetainedPlan::default();
    store(&mut cache, &commands);
    let extent = Extent::new(64, 64);
    let limits = PlanLimits::default();
    for (extent, clear, tone, limits) in [
        (
            Extent::new(128, 64),
            Oklch::BLACK,
            ToneCurve::default(),
            limits,
        ),
        (
            extent,
            Oklch::new(0.4, 0.0, 0.0),
            ToneCurve::default(),
            limits,
        ),
        (extent, Oklch::BLACK, ToneCurve::EXACT, limits),
        (
            extent,
            Oklch::BLACK,
            ToneCurve::default(),
            PlanLimits {
                bindless: !limits.bindless,
                ..limits
            },
        ),
    ] {
        assert!(cache
            .reuse(extent, clear, tone, limits, &commands)
            .is_none());
    }
}

#[test]
fn oversized_scenes_do_not_qualify_for_retention() {
    let commands: Vec<_> = (0..4097).map(|_| block(0.0)).collect();
    let mut cache = RetainedPlan::default();
    store(&mut cache, &commands);
    assert!(!cache.bounded());
}
