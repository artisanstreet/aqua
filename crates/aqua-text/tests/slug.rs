use aqua_text::slug::{PathCommand, SlugGlyph};

fn square(min: f32, max: f32) -> Vec<PathCommand> {
    vec![
        PathCommand::Move([min, min]),
        PathCommand::Line([max, min]),
        PathCommand::Line([max, max]),
        PathCommand::Line([min, max]),
        PathCommand::Close,
    ]
}

fn disc(radius: f32) -> Vec<PathCommand> {
    let k = radius * (std::f32::consts::SQRT_2 - 1.0);
    let r = radius;
    vec![
        PathCommand::Move([r, 0.0]),
        PathCommand::Quad(
            [r, k],
            [
                r * std::f32::consts::FRAC_1_SQRT_2,
                r * std::f32::consts::FRAC_1_SQRT_2,
            ],
        ),
        PathCommand::Quad([k, r], [0.0, r]),
        PathCommand::Quad(
            [-k, r],
            [
                -r * std::f32::consts::FRAC_1_SQRT_2,
                r * std::f32::consts::FRAC_1_SQRT_2,
            ],
        ),
        PathCommand::Quad([-r, k], [-r, 0.0]),
        PathCommand::Quad(
            [-r, -k],
            [
                -r * std::f32::consts::FRAC_1_SQRT_2,
                -r * std::f32::consts::FRAC_1_SQRT_2,
            ],
        ),
        PathCommand::Quad([-k, -r], [0.0, -r]),
        PathCommand::Quad(
            [k, -r],
            [
                r * std::f32::consts::FRAC_1_SQRT_2,
                -r * std::f32::consts::FRAC_1_SQRT_2,
            ],
        ),
        PathCommand::Quad([r, -k], [r, 0.0]),
        PathCommand::Close,
    ]
}

fn total_ink(glyph: &SlugGlyph, pixels_per_em: f32, extent: f32) -> f32 {
    let steps = (extent * 2.0 * pixels_per_em).ceil() as i32;
    let mut sum = 0.0;
    for j in 0..steps {
        for i in 0..steps {
            let x = -extent + (i as f32 + 0.5) / pixels_per_em;
            let y = -extent + (j as f32 + 0.5) / pixels_per_em;
            sum += glyph.coverage(x, y, pixels_per_em);
        }
    }
    sum
}

#[test]
fn square_is_solid_inside_empty_outside_and_half_on_the_edge() {
    let glyph = SlugGlyph::from_path(&square(-0.25, 0.25)).unwrap();
    let ppem = 32.0;
    assert!((glyph.coverage(0.0, 0.0, ppem) - 1.0).abs() < 1e-4);
    assert!(glyph.coverage(0.4, 0.0, ppem) < 1e-4);
    assert!(glyph.coverage(0.0, -0.4, ppem) < 1e-4);
    let edge = glyph.coverage(0.25, 0.0, ppem);
    assert!((edge - 0.5).abs() < 0.05, "edge {edge}");
}

#[test]
fn disc_ink_matches_its_analytic_area_at_every_scale() {
    let glyph = SlugGlyph::from_path(&disc(0.4)).unwrap();
    let area = std::f32::consts::PI * 0.4 * 0.4;
    for ppem in [8.0f32, 16.0, 40.0, 123.0, 400.0] {
        let ink = total_ink(&glyph, ppem, 0.6) / (ppem * ppem);
        assert!(
            (ink - area).abs() / area < 0.03,
            "ppem {ppem}: {ink} vs {area}"
        );
    }
}

#[test]
fn opposite_winding_hole_cuts_out_the_inner_square() {
    let mut commands = square(-0.4, 0.4);
    commands.extend([
        PathCommand::Move([-0.2, -0.2]),
        PathCommand::Line([-0.2, 0.2]),
        PathCommand::Line([0.2, 0.2]),
        PathCommand::Line([0.2, -0.2]),
        PathCommand::Close,
    ]);
    let glyph = SlugGlyph::from_path(&commands).unwrap();
    assert!(glyph.coverage(0.0, 0.0, 64.0) < 1e-3);
    assert!((glyph.coverage(0.3, 0.0, 64.0) - 1.0).abs() < 1e-3);
}

#[test]
fn cubics_split_into_quadratics_that_hug_the_curve() {
    let k = 0.5522847 * 0.4;
    let commands = vec![
        PathCommand::Move([0.4, 0.0]),
        PathCommand::Cubic([0.4, k], [k, 0.4], [0.0, 0.4]),
        PathCommand::Cubic([-k, 0.4], [-0.4, k], [-0.4, 0.0]),
        PathCommand::Cubic([-0.4, -k], [-k, -0.4], [0.0, -0.4]),
        PathCommand::Cubic([k, -0.4], [0.4, -k], [0.4, 0.0]),
        PathCommand::Close,
    ];
    let glyph = SlugGlyph::from_path(&commands).unwrap();
    let area = std::f32::consts::PI * 0.16;
    let ink = total_ink(&glyph, 64.0, 0.6) / (64.0 * 64.0);
    assert!((ink - area).abs() / area < 0.01, "{ink} vs {area}");
}

#[test]
fn bands_partition_curves_by_extent_and_sort_for_early_exit() {
    let glyph = SlugGlyph::from_path(&disc(0.4)).unwrap();
    for bands in [&glyph.horizontal, &glyph.vertical] {
        let covered: usize = bands.ranges.iter().map(|range| range.count as usize).sum();
        assert_eq!(covered, bands.curves.len());
        assert!(covered >= glyph.curves.len());
    }
    let range = glyph.horizontal.ranges[0];
    let members =
        &glyph.horizontal.curves[range.offset as usize..(range.offset + range.count) as usize];
    let maxima: Vec<f32> = members
        .iter()
        .map(|index| {
            let curve = glyph.curves[*index as usize];
            curve.p1[0].max(curve.p2[0]).max(curve.p3[0])
        })
        .collect();
    assert!(maxima.windows(2).all(|pair| pair[0] >= pair[1]));
}

#[test]
fn degenerate_paths_produce_no_glyph() {
    assert!(SlugGlyph::from_path(&[]).is_none());
    assert!(SlugGlyph::from_path(&[PathCommand::Move([0.0, 0.0]), PathCommand::Close]).is_none());
}
