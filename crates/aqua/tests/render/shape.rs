use aqua::render::shape::{
    Border, Fidelity, Paint, Radii, ShadowSpec, Sides, dashes, emit_box, emit_shadow, pieces,
};
use aqua_core::Oklch;
use aqua_core::geometry::Rect;
use aqua_graph::PaintCommand;

fn bounds() -> Rect {
    Rect {
        x: 10.0,
        y: 20.0,
        w: 100.0,
        h: 60.0,
    }
}

fn red() -> Oklch {
    Oklch::from_srgb8(255, 0, 0, 1.0)
}

fn area(rect: Rect) -> f32 {
    rect.w.max(0.0) * rect.h.max(0.0)
}

#[test]
fn uniform_radii_need_a_single_unclipped_piece() {
    let found = pieces(bounds(), Radii::uniform(8.0), 2.0);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].clip, None);
    assert_eq!(found[0].radius, 8.0);
}

#[test]
fn paired_radii_split_once_and_mixed_radii_split_in_quadrants() {
    let top_bottom = Radii {
        top_left: 8.0,
        top_right: 8.0,
        bottom_right: 0.0,
        bottom_left: 0.0,
    };
    assert_eq!(pieces(bounds(), top_bottom, 2.0).len(), 2);
    let left_right = Radii {
        top_left: 8.0,
        top_right: 0.0,
        bottom_right: 0.0,
        bottom_left: 8.0,
    };
    assert_eq!(pieces(bounds(), left_right, 2.0).len(), 2);
    let mixed = Radii {
        top_left: 8.0,
        top_right: 4.0,
        bottom_right: 2.0,
        bottom_left: 0.0,
    };
    assert_eq!(pieces(bounds(), mixed, 2.0).len(), 4);
}

#[test]
fn quadrant_clips_tile_the_bounds_without_overlap() {
    let mixed = Radii {
        top_left: 8.0,
        top_right: 4.0,
        bottom_right: 2.0,
        bottom_left: 0.0,
    };
    let clips: Vec<Rect> = pieces(bounds(), mixed, 0.0)
        .into_iter()
        .filter_map(|piece| piece.clip)
        .collect();
    let total: f32 = clips.iter().map(|clip| area(*clip)).sum();
    assert!((total - area(bounds())).abs() < 1.0);
    for (index, first) in clips.iter().enumerate() {
        for second in clips.iter().skip(index + 1) {
            assert_eq!(area(first.intersect(*second)), 0.0);
        }
    }
}

#[test]
fn oversized_radii_are_scaled_to_fit_the_box() {
    let fitted = Radii::uniform(100.0).fitted(Rect {
        x: 0.0,
        y: 0.0,
        w: 40.0,
        h: 20.0,
    });
    assert!((fitted.top_left - 10.0).abs() < 1e-4);
}

#[test]
fn dashes_are_evenly_spaced_and_fill_two_thirds_of_each_period() {
    let found = dashes(0.0, 90.0, 10.0);
    assert_eq!(found.len(), 3);
    let period = found[1].0 - found[0].0;
    assert!((period - 30.0).abs() < 1e-4);
    assert!(
        found
            .iter()
            .all(|(_, length)| (*length - 20.0).abs() < 1e-4)
    );
    assert!(dashes(5.0, 5.0, 2.0).is_empty());
}

fn count(commands: &[PaintCommand], test: impl Fn(&PaintCommand) -> bool) -> usize {
    commands.iter().filter(|command| test(command)).count()
}

#[test]
fn uneven_borders_become_one_ring_per_visible_side() {
    let mut out = Vec::new();
    let border = Border {
        widths: Sides {
            top: 2.0,
            right: 0.0,
            bottom: 2.0,
            left: 4.0,
        },
        color: red(),
        dashed: false,
    };
    emit_box(&mut out, bounds(), Radii::uniform(6.0), None, Some(border));
    assert_eq!(count(&out, |c| matches!(c, PaintCommand::Quad(_))), 3);
    assert_eq!(
        count(&out, |c| matches!(c, PaintCommand::PushClip(_))),
        count(&out, |c| matches!(c, PaintCommand::PopClip))
    );
}

#[test]
fn uniform_border_and_fill_share_one_quad() {
    let mut out = Vec::new();
    let border = Border {
        widths: Sides::uniform(2.0),
        color: red(),
        dashed: false,
    };
    emit_box(
        &mut out,
        bounds(),
        Radii::uniform(6.0),
        Some(Paint::Solid(Oklch::WHITE)),
        Some(border),
    );
    assert_eq!(out.len(), 1);
}

#[test]
fn dashed_borders_clip_the_ring_into_corners_and_dashes() {
    let mut out = Vec::new();
    let border = Border {
        widths: Sides::uniform(2.0),
        color: red(),
        dashed: true,
    };
    emit_box(&mut out, bounds(), Radii::uniform(6.0), None, Some(border));
    let rings = count(&out, |c| matches!(c, PaintCommand::Quad(_)));
    assert!(rings > 8, "{rings}");
    assert_eq!(
        count(&out, |c| matches!(c, PaintCommand::PushClip(_))),
        rings
    );
}

fn inset(spread: f32) -> ShadowSpec {
    ShadowSpec {
        dx: 0.0,
        dy: 2.0,
        sigma: 3.0,
        spread,
        color: Oklch::BLACK.with_alpha(0.5),
        inset: true,
    }
}

#[test]
fn inset_shadows_are_four_half_planes_inside_a_clip() {
    let mut out = Vec::new();
    let fidelity = emit_shadow(&mut out, bounds(), Radii::uniform(6.0), inset(0.0));
    assert_eq!(fidelity, Fidelity::Exact);
    assert!(matches!(
        out.first(),
        Some(PaintCommand::PushRoundedClip { .. })
    ));
    assert!(matches!(out.last(), Some(PaintCommand::PopClip)));
    assert_eq!(count(&out, |c| matches!(c, PaintCommand::Shadow(_))), 4);
    let hole = Rect::from_edges(10.0, 22.0, 110.0, 82.0);
    for command in &out {
        if let PaintCommand::Shadow(shadow) = command {
            assert_eq!(area(shadow.bounds.intersect(hole)), 0.0);
        }
    }
}

#[test]
fn inset_shadow_with_a_collapsed_hole_fills_the_whole_box() {
    let mut out = Vec::new();
    emit_shadow(&mut out, bounds(), Radii::uniform(0.0), inset(40.0));
    assert_eq!(count(&out, |c| matches!(c, PaintCommand::Shadow(_))), 0);
    assert_eq!(count(&out, |c| matches!(c, PaintCommand::Quad(_))), 1);
}

#[test]
fn outer_shadows_follow_the_corner_pieces() {
    let mut out = Vec::new();
    let spec = ShadowSpec {
        inset: false,
        ..inset(0.0)
    };
    let mixed = Radii {
        top_left: 8.0,
        top_right: 0.0,
        bottom_right: 0.0,
        bottom_left: 0.0,
    };
    emit_shadow(&mut out, bounds(), mixed, spec);
    assert_eq!(count(&out, |c| matches!(c, PaintCommand::Shadow(_))), 4);
}
