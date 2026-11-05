use aqua_core::Oklch;
use aqua_core::geometry::Rect;
use aqua_graph::{
    Extent, Linear2, PaintCommand, PassKind, PlanError, Quad, Shadow, ShadowCast, SlugInstance,
    SlugRun, TextureHandle, TextureRef, plan_paint, validate,
};

const STORE: TextureHandle = TextureHandle(9);
const EXTENT: Extent = Extent::new(120, 80);

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

fn quad(bounds: Rect) -> Quad {
    Quad {
        bounds,
        radius: 0.0,
        border_width: 0.0,
        fill: Oklch::WHITE,
        border_color: Oklch::TRANSPARENT,
    }
}

fn glyph(origin: [f32; 2]) -> SlugInstance {
    SlugInstance {
        origin,
        linear: Linear2::scale(20.0, -20.0),
        glyph: 0,
        em_bounds: [0.0, 0.0, 1.0, 1.0],
        color: Oklch::WHITE,
        clip: None,
        opacity: 1.0,
    }
}

#[test]
fn only_quads_and_images_take_a_linear_transform() {
    let shadow = PaintCommand::Shadow(Shadow {
        bounds: rect(10.0, 10.0, 20.0, 20.0),
        radius: 0.0,
        offset: [0.0, 0.0],
        sigma: 2.0,
        spread: 0.0,
        color: Oklch::BLACK,
        cast: ShadowCast::Whole,
    });
    let commands = [PaintCommand::Linear {
        linear: Linear2::rotation(0.3),
        leaf: Box::new(shadow),
    }];
    assert_eq!(
        plan_paint(EXTENT, Oklch::TRANSPARENT, &commands).unwrap_err(),
        PlanError::NotTransformable
    );
}

#[test]
fn rotated_quads_are_culled_by_their_rotated_footprint() {
    let off_corner = |angle: f32| {
        let commands = [PaintCommand::Linear {
            linear: Linear2::rotation(angle),
            leaf: Box::new(PaintCommand::Quad(quad(rect(125.0, 30.0, 40.0, 4.0)))),
        }];
        plan_paint(EXTENT, Oklch::TRANSPARENT, &commands).unwrap()
    };
    let flat = off_corner(0.0);
    assert_eq!(flat.instance_count(), 0);
    let swung = plan_paint(
        EXTENT,
        Oklch::TRANSPARENT,
        &[PaintCommand::Linear {
            linear: Linear2::rotation(std::f32::consts::FRAC_PI_2),
            leaf: Box::new(PaintCommand::Quad(quad(rect(80.0, 30.0, 80.0, 4.0)))),
        }],
    )
    .unwrap();
    assert_eq!(swung.instance_count(), 1);
    validate(&swung).unwrap();
}

#[test]
fn consecutive_slug_runs_merge_into_one_pass_reading_the_store() {
    let commands = [
        PaintCommand::Slug(SlugRun {
            store: STORE,
            glyphs: vec![glyph([10.0, 40.0]), glyph([40.0, 40.0])],
        }),
        PaintCommand::Slug(SlugRun {
            store: STORE,
            glyphs: vec![glyph([70.0, 40.0])],
        }),
    ];
    let plan = plan_paint(EXTENT, Oklch::TRANSPARENT, &commands).unwrap();
    validate(&plan).unwrap();
    let slugs: Vec<_> = plan
        .passes
        .iter()
        .filter(|pass| matches!(pass.kind, PassKind::Slug { .. }))
        .collect();
    assert_eq!(slugs.len(), 1);
    assert_eq!(slugs[0].reads, vec![TextureRef::Persistent(STORE)]);
    assert_eq!(slugs[0].kind.draw_count(), 3);
}

fn slug_instances(plan: &aqua_graph::FramePlan) -> Vec<SlugInstance> {
    plan.passes
        .iter()
        .flat_map(|pass| match &pass.kind {
            PassKind::Slug { instances, .. } => instances.clone(),
            _ => Vec::new(),
        })
        .collect()
}

#[test]
fn disjoint_children_fold_the_group_opacity_into_the_glyphs() {
    let commands = [
        PaintCommand::PushOpacity(0.5),
        PaintCommand::Quad(quad(rect(2.0, 2.0, 10.0, 10.0))),
        PaintCommand::Slug(SlugRun {
            store: STORE,
            glyphs: vec![glyph([60.0, 60.0])],
        }),
        PaintCommand::PopOpacity,
    ];
    let plan = plan_paint(EXTENT, Oklch::TRANSPARENT, &commands).unwrap();
    validate(&plan).unwrap();
    let instances = slug_instances(&plan);
    assert_eq!(instances.len(), 1);
    assert!((instances[0].opacity - 0.5).abs() < 1.0e-6);
    assert_eq!(instances[0].origin, [60.0, 60.0]);
}

#[test]
fn overlapping_children_draw_into_a_layer_with_local_origins() {
    let commands = [
        PaintCommand::PushOpacity(0.5),
        PaintCommand::Quad(quad(rect(30.0, 20.0, 20.0, 20.0))),
        PaintCommand::Slug(SlugRun {
            store: STORE,
            glyphs: vec![glyph([36.0, 36.0])],
        }),
        PaintCommand::PopOpacity,
    ];
    let plan = plan_paint(EXTENT, Oklch::TRANSPARENT, &commands).unwrap();
    validate(&plan).unwrap();
    let instances = slug_instances(&plan);
    assert_eq!(instances.len(), 1);
    assert_eq!(instances[0].opacity, 1.0);
    assert_eq!(instances[0].origin, [6.0, 21.0]);
}
