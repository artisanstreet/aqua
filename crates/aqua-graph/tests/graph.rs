use aqua_core::{geometry::Rect, Oklch};
use aqua_graph::{
    validate, Extent, FilterParams, GlyphInstance, Pass, PassKind, PlanError, PlanLimits,
    QuadInstance, RenderGraph, ShaderId, TextureFormat, TextureHandle, TextureRef,
};

const EXTENT: Extent = Extent {
    width: 128,
    height: 128,
};

fn quad_instance(x: f32) -> QuadInstance {
    QuadInstance {
        bounds: Rect {
            x,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        },
        radius: 0.0,
        border_width: 0.0,
        fill: Oklch::WHITE,
        border_color: Oklch::WHITE,
        clip: None,
        gradient: None,
        opacity: 1.0,
        linear: aqua_graph::Linear2::IDENTITY,
    }
}

fn draw(x: f32, target: TextureRef) -> Pass {
    Pass::new(
        PassKind::Instanced {
            shader: ShaderId::Quad,
            instances: vec![quad_instance(x)],
        },
        Vec::new(),
        vec![target],
    )
}

fn composite(source: TextureRef) -> Pass {
    Pass::new(
        PassKind::Fullscreen {
            shader: ShaderId::LayerOver,
            opacity: 1.0,
        },
        vec![source],
        vec![TextureRef::Surface],
    )
}

fn graph() -> RenderGraph {
    RenderGraph::new(EXTENT, Oklch::BLACK)
}

#[test]
fn independent_draws_hoist_into_the_earlier_batch() {
    let mut graph = graph();
    let a = graph.create(EXTENT, TextureFormat::Rgba16Float);
    let b = graph.create(EXTENT, TextureFormat::Rgba16Float);
    graph.push(draw(0.0, a));
    graph.push(draw(20.0, b));
    graph.push(draw(40.0, a));
    graph.push(composite(a));
    graph.push(composite(b));
    let (plan, stats) = graph.plan_with_stats(PlanLimits::default()).unwrap();
    assert_eq!(stats.merged_passes, 1);
    let first = &plan.passes[0];
    assert_eq!(first.kind.draw_count(), 2);
    assert_eq!(first.writes, vec![a]);
}

#[test]
fn dependent_draws_do_not_cross_a_reader() {
    let mut graph = graph();
    let a = graph.create(EXTENT, TextureFormat::Rgba16Float);
    graph.push(draw(0.0, a));
    graph.push(composite(a));
    graph.push(draw(40.0, a));
    graph.push(composite(a));
    let (plan, stats) = graph.plan_with_stats(PlanLimits::default()).unwrap();
    assert_eq!(stats.merged_passes, 0);
    assert_eq!(plan.passes.len(), 4);
}

#[test]
fn passes_feeding_nothing_are_culled() {
    let mut graph = graph();
    let unused = graph.create(EXTENT, TextureFormat::Rgba16Float);
    graph.push(draw(0.0, unused));
    graph.push(draw(0.0, TextureRef::Surface));
    let (plan, stats) = graph.plan_with_stats(PlanLimits::default()).unwrap();
    assert_eq!(stats.culled_passes, 1);
    assert!(plan.textures.is_empty());
    assert_eq!(plan.passes.len(), 1);
}

#[test]
fn transients_with_disjoint_lifetimes_share_heap_and_overlapping_ones_do_not() {
    let mut graph = graph();
    let a = graph.create(EXTENT, TextureFormat::Rgba16Float);
    let b = graph.create(EXTENT, TextureFormat::Rgba16Float);
    let c = graph.create(EXTENT, TextureFormat::Rgba16Float);
    graph.push(draw(0.0, a));
    graph.push(draw(0.0, b));
    graph.push(composite(a));
    graph.push(composite(b));
    graph.push(draw(0.0, c));
    graph.push(composite(c));
    let (plan, stats) = graph.plan_with_stats(PlanLimits::default()).unwrap();
    let offset = |texture: TextureRef| match texture {
        TextureRef::Transient(id) => plan.texture(id).unwrap().heap_offset,
        _ => unreachable!(),
    };
    assert_ne!(offset(a), offset(b));
    assert!(offset(c) == offset(a) || offset(c) == offset(b));
    assert_eq!(
        stats.aliased_bytes_saved,
        plan.textures[0].bytes().next_multiple_of(65536)
    );
}

#[test]
fn oversized_transients_are_rejected_against_the_limits() {
    let mut graph = graph();
    let huge = graph.create(Extent::new(40000, 10), TextureFormat::R8Unorm);
    graph.push(draw(0.0, huge));
    graph.push(composite(huge));
    assert!(matches!(
        graph.plan(PlanLimits::default()),
        Err(PlanError::TextureTooLarge { .. })
    ));
}

#[test]
fn glyph_passes_bind_the_atlas_and_survive_validation() {
    let mut graph = graph();
    let atlas = TextureHandle(7);
    graph.push(Pass::new(
        PassKind::Glyphs {
            atlas,
            instances: vec![GlyphInstance {
                bounds: Rect {
                    x: 1.0,
                    y: 1.0,
                    w: 8.0,
                    h: 8.0,
                },
                texels: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 8.0,
                    h: 8.0,
                },
                color: Oklch::WHITE,
                clip: None,
                opacity: 1.0,
                backdrop: None,
            }],
        },
        vec![TextureRef::Persistent(atlas)],
        vec![TextureRef::Surface],
    ));
    let plan = graph.plan(PlanLimits::default()).unwrap();
    assert_eq!(validate(&plan), Ok(()));

    let mut mismatched = plan.clone();
    mismatched.passes[0].reads.clear();
    assert!(matches!(
        validate(&mismatched),
        Err(PlanError::BadBindings { .. })
    ));
    let mut persistent_write = plan.clone();
    persistent_write.passes[0].writes = vec![TextureRef::Persistent(atlas)];
    assert!(matches!(
        validate(&persistent_write),
        Err(PlanError::PersistentWrite { .. })
    ));
}

#[test]
fn filters_reading_and_writing_the_same_texture_are_rejected() {
    let mut graph = graph();
    let a = graph.create(EXTENT, TextureFormat::Rgba16Float);
    graph.push(draw(0.0, a));
    graph.push(Pass::new(
        PassKind::Filter {
            shader: ShaderId::ColorMatrix,
            params: FilterParams::color_matrix(EXTENT.bounds(), aqua_graph::ColorMatrix::IDENTITY),
        },
        vec![a],
        vec![a],
    ));
    graph.push(composite(a));
    assert!(matches!(
        graph.plan(PlanLimits::default()),
        Err(PlanError::ReadWriteHazard { .. })
    ));
}

fn custom_spec(name: &str, param: f32) -> aqua_graph::CustomPassSpec {
    aqua_graph::CustomPassSpec {
        name: name.into(),
        hlsl: String::new(),
        entry: "main".into(),
        bounds: EXTENT.bounds(),
        params: vec![param],
    }
}

fn effect_graph(kind: PassKind) -> RenderGraph {
    let mut graph = graph();
    let a = graph.create(EXTENT, TextureFormat::Rgba16Float);
    let b = graph.create(EXTENT, TextureFormat::Rgba16Float);
    graph.push(draw(0.0, a));
    graph.push(Pass::new(kind, vec![a], vec![b]));
    graph.push(composite(b));
    graph
}

#[test]
fn compute_and_custom_passes_plan_with_unordered_access_and_declared_inputs() {
    let compute = PassKind::Compute {
        shader: ShaderId::Custom(aqua_graph::CustomShaderId(1)),
        groups: [2, 128, 1],
        params: [3.0, 0.0, 0.0, 0.0],
    };
    let plan = effect_graph(compute).plan(PlanLimits::default()).unwrap();
    assert_eq!(validate(&plan), Ok(()));
    assert!(plan
        .passes
        .iter()
        .flat_map(|pass| pass.barriers.iter())
        .any(|barrier| matches!(
            barrier,
            aqua_graph::Barrier::Transition {
                to: aqua_graph::ResourceState::UnorderedAccess,
                ..
            }
        )));
    let plan = effect_graph(PassKind::Custom(custom_spec("effect", 1.0)))
        .plan(PlanLimits::default())
        .unwrap();
    assert_eq!(validate(&plan), Ok(()));
}

#[test]
fn malformed_compute_and_custom_passes_are_rejected() {
    let empty_groups = PassKind::Compute {
        shader: ShaderId::Custom(aqua_graph::CustomShaderId(1)),
        groups: [0, 1, 1],
        params: [0.0; 4],
    };
    assert!(matches!(
        effect_graph(empty_groups).plan(PlanLimits::default()),
        Err(PlanError::NonFiniteValue { .. })
    ));
    let builtin_shader = PassKind::Compute {
        shader: ShaderId::Quad,
        groups: [1, 1, 1],
        params: [0.0; 4],
    };
    assert!(matches!(
        effect_graph(builtin_shader).plan(PlanLimits::default()),
        Err(PlanError::ShaderMismatch { .. })
    ));
    assert!(matches!(
        effect_graph(PassKind::Custom(custom_spec("", 1.0))).plan(PlanLimits::default()),
        Err(PlanError::BadBindings { .. })
    ));
    assert!(matches!(
        effect_graph(PassKind::Custom(custom_spec("effect", f32::NAN))).plan(PlanLimits::default()),
        Err(PlanError::NonFiniteValue { .. })
    ));
}

#[test]
fn color_matrix_filters_reject_non_finite_matrices() {
    let mut matrix = aqua_graph::ColorMatrix::IDENTITY;
    matrix.0[7] = f32::INFINITY;
    let kind = PassKind::Filter {
        shader: ShaderId::ColorMatrix,
        params: FilterParams::color_matrix(EXTENT.bounds(), matrix),
    };
    assert!(matches!(
        effect_graph(kind).plan(PlanLimits::default()),
        Err(PlanError::NonFiniteValue { .. })
    ));
}
