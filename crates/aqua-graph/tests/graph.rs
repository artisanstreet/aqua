use aqua_core::{Oklch, geometry::Rect};
use aqua_graph::{
    Extent, FilterParams, GlyphInstance, Pass, PassKind, PlanError, PlanLimits, QuadInstance,
    RenderGraph, ShaderId, TextureFormat, TextureHandle, TextureRef, validate,
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
            params: FilterParams {
                region: EXTENT.bounds(),
                values: [0.0; 4],
            },
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
