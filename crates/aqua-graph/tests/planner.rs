use aqua_core::{Oklch, geometry::Rect};
use aqua_graph::{
    BackdropBlur, Barrier, Extent, PaintCommand, PassKind, PlanError, Quad, ResourceState,
    ShaderId, TextureRef, plan_paint, validate,
};

const EXTENT: Extent = Extent {
    width: 64,
    height: 64,
};

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

fn quad_at(x: f32, y: f32) -> PaintCommand {
    PaintCommand::Quad(Quad {
        bounds: rect(x, y, 10.0, 10.0),
        radius: 0.0,
        border_width: 0.0,
        fill: Oklch::WHITE,
        border_color: Oklch::WHITE,
    })
}

fn quad() -> PaintCommand {
    quad_at(0.0, 0.0)
}

fn clip(x: f32, w: f32) -> PaintCommand {
    PaintCommand::PushClip(rect(x, 0.0, w, 64.0))
}

fn layered() -> Vec<PaintCommand> {
    vec![
        PaintCommand::PushOpacity(0.5),
        quad(),
        quad(),
        PaintCommand::PopOpacity,
    ]
}

fn plan(commands: &[PaintCommand]) -> aqua_graph::FramePlan {
    plan_paint(EXTENT, Oklch::TRANSPARENT, commands).unwrap()
}

#[test]
fn consecutive_quads_share_one_pass() {
    let plan = plan(&[quad(), quad_at(20.0, 0.0), quad_at(40.0, 0.0)]);
    assert_eq!(plan.passes.len(), 1);
    assert_eq!(plan.instance_count(), 3);
    assert!(plan.textures.is_empty());
}

#[test]
fn groups_of_disjoint_children_fold_opacity_into_instances() {
    let plan = plan(&[
        PaintCommand::PushOpacity(0.5),
        quad_at(0.0, 0.0),
        quad_at(20.0, 0.0),
        PaintCommand::PushOpacity(0.5),
        quad_at(40.0, 0.0),
        PaintCommand::PopOpacity,
        PaintCommand::PopOpacity,
    ]);
    assert!(plan.textures.is_empty());
    let PassKind::Instanced { instances, .. } = &plan.passes[0].kind else {
        panic!("instanced pass expected");
    };
    let opacities: Vec<f32> = instances.iter().map(|instance| instance.opacity).collect();
    assert_eq!(opacities, vec![0.5, 0.5, 0.25]);
}

#[test]
fn groups_of_overlapping_children_get_a_layer() {
    let plan = plan(&layered());
    assert_eq!(plan.textures.len(), 1);
    assert_eq!(plan.passes.len(), 2);
    assert!(matches!(
        plan.passes[1].kind,
        PassKind::Fullscreen {
            shader: ShaderId::LayerOver,
            opacity
        } if opacity == 0.5
    ));
}

#[test]
fn sibling_layers_alias_and_nested_layers_do_not() {
    let mut commands = vec![
        PaintCommand::PushOpacity(0.5),
        quad(),
        quad(),
        PaintCommand::PushOpacity(0.5),
        quad(),
        quad(),
        PaintCommand::PopOpacity,
        PaintCommand::PopOpacity,
    ];
    commands.extend(layered());
    let plan = plan(&commands);
    let offsets: Vec<u64> = plan
        .textures
        .iter()
        .map(|texture| texture.heap_offset)
        .collect();
    assert_eq!(plan.textures.len(), 3);
    assert_eq!(offsets.iter().filter(|offset| **offset == 0).count(), 2);
    assert!(offsets.iter().any(|offset| *offset > 0));
    let one_layer = plan.textures[0].bytes().next_multiple_of(64 * 1024);
    assert_eq!(plan.heap_bytes, 2 * one_layer);
}

#[test]
fn aliased_reuse_carries_an_alias_barrier_and_a_clear() {
    let mut commands = layered();
    commands.extend(layered());
    let plan = plan(&commands);
    let reused = plan
        .passes
        .iter()
        .flat_map(|pass| pass.barriers.iter())
        .filter(|barrier| matches!(barrier, Barrier::Alias { .. }))
        .count();
    assert_eq!(reused, 1);
    assert_eq!(plan.textures[0].heap_offset, plan.textures[1].heap_offset);
    assert_eq!(validate(&plan), Ok(()));
}

#[test]
fn surface_ends_presentable_after_render_target_use() {
    let plan = plan(&[quad()]);
    let transitions: Vec<_> = plan
        .passes
        .iter()
        .flat_map(|pass| pass.barriers.iter())
        .collect();
    assert_eq!(
        transitions,
        vec![&Barrier::Transition {
            resource: TextureRef::Surface,
            from: ResourceState::Present,
            to: ResourceState::RenderTarget
        }]
    );
    assert_eq!(
        plan.final_barriers,
        vec![Barrier::Transition {
            resource: TextureRef::Surface,
            from: ResourceState::RenderTarget,
            to: ResourceState::Present
        }]
    );
}

#[test]
fn invisible_work_is_culled() {
    let plan = plan(&[
        PaintCommand::PushOpacity(0.5),
        PaintCommand::PopOpacity,
        clip(0.0, 5.0),
        clip(20.0, 5.0),
        quad(),
        PaintCommand::PopClip,
        PaintCommand::PopClip,
        quad_at(100.0, 100.0),
        quad_at(-50.0, 0.0),
        PaintCommand::PushOpacity(0.0),
        quad(),
        PaintCommand::PopOpacity,
    ]);
    assert!(plan.passes.is_empty());
    assert!(plan.textures.is_empty());
    assert!(plan.final_barriers.is_empty());
}

#[test]
fn clips_intersect_into_instances() {
    let plan = plan(&[
        clip(0.0, 30.0),
        clip(10.0, 30.0),
        quad_at(15.0, 0.0),
        PaintCommand::PopClip,
        quad_at(15.0, 20.0),
        PaintCommand::PopClip,
    ]);
    let PassKind::Instanced { instances, .. } = &plan.passes[0].kind else {
        panic!("instanced pass expected");
    };
    let first = instances[0].clip.unwrap();
    assert_eq!((first.x, first.right()), (10.0, 30.0));
    assert_eq!(instances[1].clip.unwrap().right(), 30.0);
}

#[test]
fn unbalanced_scopes_are_rejected() {
    assert_eq!(
        plan_paint(EXTENT, Oklch::TRANSPARENT, &[PaintCommand::PopOpacity]).unwrap_err(),
        PlanError::UnbalancedPop
    );
    assert_eq!(
        plan_paint(
            EXTENT,
            Oklch::TRANSPARENT,
            &[PaintCommand::PushOpacity(0.5)]
        )
        .unwrap_err(),
        PlanError::UnclosedScope
    );
    assert_eq!(
        plan_paint(
            EXTENT,
            Oklch::TRANSPARENT,
            &[
                PaintCommand::PushClip(rect(0.0, 0.0, 1.0, 1.0)),
                PaintCommand::PopOpacity
            ]
        )
        .unwrap_err(),
        PlanError::UnbalancedPop
    );
}

#[test]
fn validation_catches_broken_plans() {
    let plan = plan(&layered());
    assert_eq!(validate(&plan), Ok(()));

    let mut reordered = plan.clone();
    reordered.passes.reverse();
    assert!(matches!(
        validate(&reordered),
        Err(PlanError::ReadBeforeWrite { .. })
    ));

    let mut surface_read = plan.clone();
    surface_read.passes[0].reads.push(TextureRef::Surface);
    assert!(matches!(
        validate(&surface_read),
        Err(PlanError::SurfaceRead { .. })
    ));

    let mut without_barriers = plan.clone();
    without_barriers.passes[1].barriers.clear();
    assert!(matches!(
        validate(&without_barriers),
        Err(PlanError::MissingBarrier { .. })
    ));

    let mut stuck = plan.clone();
    stuck.final_barriers.clear();
    assert_eq!(validate(&stuck), Err(PlanError::SurfaceNotPresented));

    let mut loads_garbage = plan.clone();
    loads_garbage.passes[0].load = aqua_graph::LoadOp::Load;
    assert!(matches!(
        validate(&loads_garbage),
        Err(PlanError::LoadOfUndefined { .. })
    ));

    let mut small_heap = plan.clone();
    small_heap.heap_bytes = 0;
    assert_eq!(validate(&small_heap), Err(PlanError::HeapTooSmall));
}

#[test]
fn colliding_aliases_are_rejected() {
    let mut commands = vec![
        PaintCommand::PushOpacity(0.5),
        quad(),
        quad(),
        PaintCommand::PushOpacity(0.5),
        quad(),
        quad(),
        PaintCommand::PopOpacity,
        PaintCommand::PopOpacity,
    ];
    commands.push(quad());
    let mut plan = plan(&commands);
    assert_eq!(validate(&plan), Ok(()));
    plan.textures[1].heap_offset = plan.textures[0].heap_offset;
    assert!(matches!(
        validate(&plan),
        Err(PlanError::AliasOverlap { .. })
    ));
}

fn backdrop(radius: f32) -> PaintCommand {
    PaintCommand::BackdropBlur(BackdropBlur {
        bounds: rect(8.0, 8.0, 40.0, 40.0),
        radius,
        corner_radius: 6.0,
    })
}

fn shaders(plan: &aqua_graph::FramePlan, wanted: ShaderId) -> usize {
    plan.passes
        .iter()
        .filter(|pass| matches!(&pass.kind, PassKind::Filter { shader, .. } if *shader == wanted))
        .count()
}

#[test]
fn backdrop_blur_snapshots_the_parent_then_runs_a_pyramid() {
    let plan = plan(&[quad(), backdrop(6.0), quad_at(50.0, 50.0)]);
    assert_eq!(validate(&plan), Ok(()));
    let copy = plan
        .passes
        .iter()
        .position(|pass| matches!(pass.kind, PassKind::Copy { .. }))
        .expect("snapshot copy");
    assert_eq!(plan.passes[copy].reads, vec![TextureRef::Surface]);
    let levels = shaders(&plan, ShaderId::BlurDown);
    assert_eq!(levels, 3);
    assert_eq!(shaders(&plan, ShaderId::BlurUp), levels);
    assert!(matches!(
        plan.passes.iter().rev().nth(1).map(|pass| &pass.kind),
        Some(PassKind::Images { .. })
    ));
    let separate: u64 = plan
        .textures
        .iter()
        .map(|texture| texture.bytes().next_multiple_of(64 * 1024))
        .sum();
    assert!(plan.heap_bytes < separate);
    let sizes: Vec<u32> = plan
        .textures
        .iter()
        .map(|texture| texture.extent.width)
        .collect();
    assert_eq!(sizes[0], 40);
    assert!(
        sizes[1..levels + 1]
            .windows(2)
            .all(|pair| pair[1] < pair[0])
    );
}

#[test]
fn blur_strength_is_continuous_across_level_changes() {
    let strength = |radius: f32| {
        let plan = plan(&[backdrop(radius)]);
        let down = plan
            .passes
            .iter()
            .find_map(|pass| match &pass.kind {
                PassKind::Filter {
                    shader: ShaderId::BlurDown,
                    params,
                } => Some(params.values),
                _ => None,
            })
            .unwrap();
        let levels = shaders(&plan, ShaderId::BlurDown);
        down[0] * (1u32 << levels) as f32
    };
    for radius in [1.5_f32, 3.0, 7.99, 8.01, 20.0, 33.3] {
        assert!((strength(radius) - radius).abs() < 1e-3, "{radius}");
    }
}

#[test]
fn zero_radius_blur_draws_nothing_and_content_blur_passes_through() {
    let plan = plan(&[backdrop(0.0)]);
    assert!(plan.passes.is_empty());
    let plan = plan_through();
    assert_eq!(plan.instance_count(), 1);
    assert!(plan.textures.is_empty());
}

fn plan_through() -> aqua_graph::FramePlan {
    plan(&[
        PaintCommand::PushContentBlur {
            bounds: rect(0.0, 0.0, 30.0, 30.0),
            radius: 0.0,
        },
        quad(),
        PaintCommand::PopContentBlur,
    ])
}

#[test]
fn content_blur_renders_children_to_a_layer_and_blurs_only_the_region() {
    let plan = plan(&[
        PaintCommand::PushContentBlur {
            bounds: rect(4.0, 4.0, 30.0, 30.0),
            radius: 4.0,
        },
        quad(),
        PaintCommand::PopContentBlur,
    ]);
    assert_eq!(validate(&plan), Ok(()));
    assert!(
        plan.passes
            .iter()
            .all(|pass| !matches!(pass.kind, PassKind::Copy { .. }))
    );
    let first_filter = plan
        .passes
        .iter()
        .find_map(|pass| match &pass.kind {
            PassKind::Filter { params, .. } => Some(params.region),
            _ => None,
        })
        .unwrap();
    assert_eq!((first_filter.x, first_filter.w), (4.0, 30.0));
}
