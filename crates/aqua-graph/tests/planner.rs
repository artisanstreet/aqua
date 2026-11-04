use aqua_core::{geometry::Rect, Oklch};
use aqua_graph::{
    depth_for, plan_paint, validate, BackdropBlur, Barrier, Blend, BlurDepth, ColorMatrix, Extent,
    LoadOp, PaintCommand, PassKind, PlanError, Quad, ResourceState, ShaderId, TextureRef,
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
    assert_eq!(plan.textures[0].extent, Extent::new(10, 10));
    let PassKind::Images { instances } = &plan.passes[1].kind else {
        panic!("bounded layer composite expected");
    };
    assert_eq!(instances[0].opacity, 0.5);
    assert_eq!(instances[0].blend, Blend::Over);
    assert_eq!(instances[0].bounds, rect(0.0, 0.0, 10.0, 10.0));
}

#[test]
fn layer_content_is_drawn_in_layer_local_coordinates() {
    let plan = plan(&[
        PaintCommand::PushOpacity(0.5),
        quad_at(20.0, 30.0),
        quad_at(25.0, 33.0),
        PaintCommand::PopOpacity,
    ]);
    assert_eq!(plan.textures[0].extent, Extent::new(15, 13));
    let PassKind::Instanced { instances, .. } = &plan.passes[0].kind else {
        panic!("layer content expected first");
    };
    assert_eq!(instances[0].bounds, rect(0.0, 0.0, 10.0, 10.0));
    assert_eq!(instances[1].bounds, rect(5.0, 3.0, 10.0, 10.0));
    let PassKind::Images { instances } = &plan.passes[1].kind else {
        panic!("composite expected");
    };
    assert_eq!(instances[0].bounds, rect(20.0, 30.0, 15.0, 13.0));
}

#[test]
fn layers_covering_the_target_keep_the_fullscreen_composite() {
    let plan = plan(&[
        PaintCommand::PushOpacity(0.5),
        PaintCommand::Quad(Quad {
            bounds: rect(0.0, 0.0, 64.0, 64.0),
            radius: 0.0,
            border_width: 0.0,
            fill: Oklch::WHITE,
            border_color: Oklch::WHITE,
        }),
        quad(),
        PaintCommand::PopOpacity,
    ]);
    assert!(matches!(
        plan.passes[1].kind,
        PassKind::Fullscreen {
            shader: ShaderId::LayerOver,
            opacity
        } if opacity == 0.5
    ));
}

#[test]
fn rounded_clips_isolate_their_children_and_clip_the_composite() {
    let plan = plan(&[
        PaintCommand::PushRoundedClip {
            rect: rect(4.5, 6.0, 30.0, 20.0),
            radius: 8.0,
        },
        quad_at(0.0, 0.0),
        quad_at(20.0, 10.0),
        PaintCommand::PopClip,
    ]);
    assert_eq!(validate(&plan), Ok(()));
    assert_eq!(plan.textures.len(), 1);
    let layer = plan.textures[0].extent;
    assert!(layer.width < EXTENT.width && layer.height < EXTENT.height);
    let PassKind::Images { instances } = &plan.passes.last().unwrap().kind else {
        panic!("rounded composite expected");
    };
    assert_eq!(instances[0].clip, Some(rect(4.5, 6.0, 30.0, 20.0)));
    assert_eq!(instances[0].clip_radius, 8.0);
    assert_eq!(instances[0].blend, Blend::Over);
}

#[test]
fn rounded_clip_with_no_radius_is_a_plain_clip() {
    let plan = plan(&[
        PaintCommand::PushRoundedClip {
            rect: rect(0.0, 0.0, 30.0, 20.0),
            radius: 0.0,
        },
        quad(),
        PaintCommand::PopClip,
    ]);
    assert!(plan.textures.is_empty());
    let PassKind::Instanced { instances, .. } = &plan.passes[0].kind else {
        panic!("direct draw expected");
    };
    assert_eq!(instances[0].clip, Some(rect(0.0, 0.0, 30.0, 20.0)));
}

#[test]
fn color_matrix_effect_filters_a_bounded_layer() {
    let plan = plan(&[
        PaintCommand::PushColorMatrix {
            bounds: rect(8.0, 8.0, 20.0, 12.0),
            matrix: ColorMatrix::saturation(0.0),
        },
        quad_at(8.0, 8.0),
        PaintCommand::PopColorMatrix,
    ]);
    assert_eq!(validate(&plan), Ok(()));
    let filter = plan
        .passes
        .iter()
        .find_map(|pass| match &pass.kind {
            PassKind::Filter {
                shader: ShaderId::ColorMatrix,
                params,
            } => Some(*params),
            _ => None,
        })
        .expect("color matrix pass");
    assert_eq!(filter.region, rect(0.0, 0.0, 20.0, 12.0));
    assert_eq!(filter.matrix, ColorMatrix::saturation(0.0));
    assert_eq!(plan.textures[0].extent, Extent::new(20, 12));
}

#[test]
fn mismatched_effect_pops_are_rejected() {
    let result = plan_paint(
        EXTENT,
        Oklch::TRANSPARENT,
        &[
            PaintCommand::PushColorMatrix {
                bounds: rect(0.0, 0.0, 8.0, 8.0),
                matrix: ColorMatrix::IDENTITY,
            },
            PaintCommand::PopContentBlur,
        ],
    );
    assert_eq!(result.unwrap_err(), PlanError::UnbalancedPop);
}

#[test]
fn backdrop_blur_replaces_instead_of_blending() {
    let plan = plan(&[quad(), backdrop(10.0)]);
    let PassKind::Images { instances } = &plan.passes.last().unwrap().kind else {
        panic!("backdrop composite expected");
    };
    assert_eq!(instances[0].blend, Blend::Replace);
    assert_eq!(instances[0].radius, 6.0);
}

#[test]
fn plans_that_never_touch_the_surface_still_clear_it() {
    let plan = plan_paint(EXTENT, Oklch::WHITE, &[]).unwrap();
    assert_eq!(plan.passes.len(), 1);
    assert!(plan.passes[0].is_clear());
    assert_eq!(plan.passes[0].load, LoadOp::Clear(Oklch::WHITE));
    assert_eq!(validate(&plan), Ok(()));
}

#[test]
fn drawing_plans_clear_through_the_first_draw_instead_of_an_extra_pass() {
    let plan = plan(&[quad()]);
    assert_eq!(plan.passes.len(), 1);
    assert!(!plan.passes[0].is_clear());
    assert!(matches!(plan.passes[0].load, LoadOp::Clear(_)));
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
    assert_eq!(plan.passes.len(), 1);
    assert!(plan.passes[0].is_clear());
    assert!(plan.textures.is_empty());
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
fn backdrop_blur_reads_the_parent_directly_and_composites_the_half_resolution_level() {
    let plan = plan(&[quad(), backdrop(20.0), quad_at(50.0, 50.0)]);
    assert_eq!(validate(&plan), Ok(()));
    assert!(plan
        .passes
        .iter()
        .all(|pass| !matches!(pass.kind, PassKind::Copy { .. })));
    let levels = shaders(&plan, ShaderId::BlurDown);
    assert_eq!(levels, 4);
    assert_eq!(shaders(&plan, ShaderId::BlurUp), levels - 1);
    let first = plan
        .passes
        .iter()
        .find(|pass| matches!(&pass.kind, PassKind::Filter { shader, .. } if *shader == ShaderId::BlurDown))
        .expect("first down pass");
    assert_eq!(first.reads, vec![TextureRef::Surface]);
    let composite = plan.passes.iter().rev().nth(1).expect("composite pass");
    let PassKind::Images { instances } = &composite.kind else {
        panic!("composite is an images pass");
    };
    assert_eq!(instances[0].bounds.w, 40.0);
    assert_eq!(instances[0].texels.w, 20.0);
    let sizes: Vec<u32> = plan
        .textures
        .iter()
        .map(|texture| texture.extent.width)
        .collect();
    assert_eq!(sizes[0], 20);
    assert_eq!(sizes.len(), levels);
    assert!(sizes.windows(2).all(|pair| pair[1] < pair[0]));
}

#[test]
fn shallow_backdrop_blur_keeps_the_full_resolution_output() {
    let plan = plan(&[quad(), backdrop(5.0), quad_at(50.0, 50.0)]);
    assert_eq!(validate(&plan), Ok(()));
    let levels = shaders(&plan, ShaderId::BlurDown);
    assert_eq!(levels, 2);
    assert_eq!(shaders(&plan, ShaderId::BlurUp), levels);
    let composite = plan.passes.iter().rev().nth(1).expect("composite pass");
    let PassKind::Images { instances } = &composite.kind else {
        panic!("composite is an images pass");
    };
    assert_eq!(instances[0].texels.w, instances[0].bounds.w);
    let separate: u64 = plan
        .textures
        .iter()
        .map(|texture| texture.bytes().next_multiple_of(64 * 1024))
        .sum();
    assert!(plan.heap_bytes < separate);
}

fn offset_of(depth: BlurDepth, radius: f32) -> (u32, f32) {
    match depth {
        BlurDepth::Single { levels } => (levels, radius / (1u32 << levels) as f32),
        BlurDepth::Mixed { lower, .. } => (lower + 1, radius / (1u32 << (lower + 1)) as f32),
    }
}

#[test]
fn single_depths_keep_the_effective_strength_equal_to_the_radius() {
    let size = Extent::new(512, 512);
    for radius in [0.5_f32, 1.5, 3.0, 5.0, 11.0, 20.0, 45.0] {
        let depth = depth_for(radius, size);
        let BlurDepth::Single { levels } = depth else {
            panic!("{radius} should be single depth");
        };
        let (_, offset) = offset_of(depth, radius);
        assert!(
            (offset * (1u32 << levels) as f32 - radius).abs() < 1e-3,
            "{radius}"
        );
    }
}

#[test]
fn blur_depth_crossfades_into_the_next_level_before_each_octave_boundary() {
    let size = Extent::new(512, 512);
    for octave in 1..5 {
        let boundary = (1u32 << (octave + 1)) as f32;
        let below = depth_for(boundary * 0.999, size);
        let above = depth_for(boundary * 1.001, size);
        let BlurDepth::Mixed { lower, weight } = below else {
            panic!("expected a crossfade below {boundary}");
        };
        assert_eq!(lower, octave);
        assert!(weight > 0.99, "{weight}");
        let BlurDepth::Single { levels } = above else {
            panic!("expected a single depth above {boundary}");
        };
        assert_eq!(levels, octave + 1);
        let (_, upper_offset) = offset_of(below, boundary * 0.999);
        let (_, next_offset) = offset_of(above, boundary * 1.001);
        assert!((upper_offset - next_offset).abs() < 0.01);
    }
}

#[test]
fn blur_depth_weight_never_decreases_inside_an_octave() {
    let size = Extent::new(512, 512);
    let mut last = 0.0f32;
    let mut radius = 4.0f32;
    while radius < 7.999 {
        let weight = match depth_for(radius, size) {
            BlurDepth::Single { .. } => 0.0,
            BlurDepth::Mixed { weight, .. } => weight,
        };
        assert!(weight >= last);
        last = weight;
        radius += 0.01;
    }
    assert!(last > 0.9);
}

#[test]
fn mixed_depth_blends_two_pyramids_with_replace_instances() {
    let plan = plan(&[
        quad(),
        PaintCommand::BackdropBlur(BackdropBlur {
            bounds: rect(8.0, 8.0, 40.0, 40.0),
            radius: 7.5,
            corner_radius: 0.0,
        }),
    ]);
    assert_eq!(validate(&plan), Ok(()));
    assert_eq!(shaders(&plan, ShaderId::BlurDown), 5);
    let mix = plan
        .passes
        .iter()
        .find_map(|pass| match &pass.kind {
            PassKind::Images { instances } if instances.len() == 2 => Some(instances.clone()),
            _ => None,
        })
        .expect("mix pass");
    assert!(mix.iter().all(|image| image.blend == Blend::Replace));
    assert_eq!(mix[0].opacity, 1.0);
    assert!(mix[1].opacity > 0.0 && mix[1].opacity < 1.0);
}

#[test]
fn zero_radius_blur_draws_nothing_and_content_blur_passes_through() {
    let plan = plan(&[backdrop(0.0)]);
    assert_eq!(plan.passes.len(), 1);
    assert!(plan.passes[0].is_clear());
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
    assert!(plan
        .passes
        .iter()
        .all(|pass| !matches!(pass.kind, PassKind::Copy { .. })));
    let first_filter = plan
        .passes
        .iter()
        .find_map(|pass| match &pass.kind {
            PassKind::Filter { params, .. } => Some(params.region),
            _ => None,
        })
        .unwrap();
    assert_eq!((first_filter.x, first_filter.w), (0.0, 30.0));
    assert_eq!(plan.textures[0].extent, Extent::new(30, 30));
}
