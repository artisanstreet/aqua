use aqua_core::geometry::Rect;
use aqua_core::Oklch;
use aqua_gpu_ref::conformance::text::{slug_assets, slug_glyph, SLUG_STORE};
use aqua_gpu_ref::raster::{draw_quad, Layer};
use aqua_gpu_ref::sprites::{draw_glyphs, draw_slug, RefTexture};
use aqua_graph::tone::{premultiplied_luminance, ToneCurve};
use aqua_graph::{Extent, GlyphInstance, Linear2, QuadInstance, SlugInstance, TextureFormat};
use aqua_protocol::TextureOp as UploadOp;

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

fn quad(bounds: Rect, linear: Linear2) -> QuadInstance {
    QuadInstance {
        bounds,
        radius: 0.0,
        border_width: 0.0,
        fill: Oklch::from_hex("#ffffff").unwrap(),
        border_color: Oklch::TRANSPARENT,
        clip: None,
        gradient: None,
        opacity: 1.0,
        linear,
    }
}

fn alpha_sum(layer: &Layer) -> f32 {
    layer.pixels.iter().map(|pixel| pixel[3]).sum()
}

fn render_quad(instance: &QuadInstance) -> Layer {
    let mut layer = Layer::transparent(64, 64);
    draw_quad(&mut layer, instance);
    layer
}

#[test]
fn a_quarter_turn_equals_the_swapped_rectangle() {
    let turned = render_quad(&quad(
        rect(10.0, 20.0, 40.0, 10.0),
        Linear2::rotation(std::f32::consts::FRAC_PI_2),
    ));
    let swapped = render_quad(&quad(rect(25.0, 5.0, 10.0, 40.0), Linear2::IDENTITY));
    let worst = turned
        .pixels
        .iter()
        .zip(&swapped.pixels)
        .flat_map(|(a, b)| a.iter().zip(b).map(|(x, y)| (x - y).abs()))
        .fold(0.0f32, f32::max);
    assert!(worst < 2.0e-3, "worst channel difference {worst}");
}

#[test]
fn shear_and_rotation_preserve_area_and_scale_multiplies_it() {
    let base = rect(20.0, 20.0, 20.0, 12.0);
    let area = 20.0 * 12.0;
    for matrix in [
        Linear2::rotation(0.6),
        Linear2::skew(0.5, 0.0),
        Linear2::skew(0.0, -0.4).then(Linear2::rotation(1.1)),
    ] {
        let ink = alpha_sum(&render_quad(&quad(base, matrix)));
        assert!((ink - area).abs() / area < 0.02, "{matrix:?}: {ink}");
    }
    let doubled = alpha_sum(&render_quad(&quad(
        rect(25.0, 25.0, 10.0, 6.0),
        Linear2::scale(2.0, 2.0).then(Linear2::rotation(0.4)),
    )));
    assert!((doubled - area).abs() / area < 0.02, "{doubled}");
}

#[test]
fn rounded_corners_and_borders_follow_the_rotation() {
    let mut instance = quad(rect(14.0, 22.0, 36.0, 20.0), Linear2::rotation(0.8));
    instance.radius = 8.0;
    let rounded = alpha_sum(&render_quad(&instance));
    let corner_loss = (4.0 - std::f32::consts::PI) * 64.0;
    let expected = 36.0 * 20.0 - corner_loss;
    assert!(
        (rounded - expected).abs() / expected < 0.02,
        "{rounded} vs {expected}"
    );
}

fn slug_store() -> RefTexture {
    let mut texture = RefTexture::new(Extent::new(1024, 8), TextureFormat::Rgba32Float).unwrap();
    for upload in &slug_assets().uploads.textures {
        if let (UploadOp::Write { region, data }, true) = (&upload.op, upload.handle == SLUG_STORE)
        {
            texture
                .write(region.x, region.y, region.extent, data)
                .unwrap();
        }
    }
    texture
}

fn render_slug(instance: SlugInstance) -> Layer {
    let mut layer = Layer::transparent(128, 128);
    draw_slug(&mut layer, &slug_store(), &[instance], ToneCurve::EXACT);
    layer
}

#[test]
fn slug_ring_ink_matches_its_analytic_area_at_every_scale_and_placement() {
    let ring = slug_assets().ring;
    let white = Oklch::from_hex("#ffffff").unwrap();
    for (ppem, placement) in [
        (24.0f32, Linear2::IDENTITY),
        (48.0, Linear2::IDENTITY),
        (48.0, Linear2::rotation(0.9)),
        (48.0, Linear2::skew(0.4, 0.0)),
        (30.0, Linear2::skew(0.0, -0.3).then(Linear2::rotation(2.0))),
    ] {
        let layer = render_slug(slug_glyph(ring, [40.0, 80.0], ppem, placement, white));
        let expected = std::f32::consts::PI * (0.5f32 * 0.5 - 0.22 * 0.22) * ppem * ppem;
        let ink = alpha_sum(&layer);
        assert!(
            (ink - expected).abs() / expected < 0.06,
            "ppem {ppem} {placement:?}: {ink} vs {expected}"
        );
    }
}

#[test]
fn slug_counter_is_empty_and_the_stroke_is_solid() {
    let ring = slug_assets().ring;
    let white = Oklch::from_hex("#ffffff").unwrap();
    let layer = render_slug(slug_glyph(
        ring,
        [10.0, 100.0],
        64.0,
        Linear2::IDENTITY,
        white,
    ));
    let centre = layer.at(10 + 32, 100 - 32);
    let stroke = layer.at(10 + 6, 100 - 32);
    assert!(centre[3] < 0.01, "{centre:?}");
    assert!(stroke[3] > 0.99, "{stroke:?}");
}

fn single_glyph(coverage: u8, tone: ToneCurve, color: &str) -> [f32; 4] {
    let mut atlas = RefTexture::new(Extent::new(1, 1), TextureFormat::R8Unorm).unwrap();
    atlas.write(0, 0, Extent::new(1, 1), &[coverage]).unwrap();
    let glyph = GlyphInstance {
        bounds: rect(0.0, 0.0, 1.0, 1.0),
        texels: rect(0.0, 0.0, 1.0, 1.0),
        color: Oklch::from_hex(color).unwrap(),
        clip: None,
        opacity: 1.0,
    };
    let mut layer = Layer::transparent(1, 1);
    draw_glyphs(&mut layer, &atlas, &[glyph], tone);
    layer.at(0, 0)
}

#[test]
fn glyph_alpha_follows_the_plan_tone_curve_in_encoded_space() {
    let native = ToneCurve::native();
    for color in ["#000000", "#ffffff", "#303030", "#dd7030"] {
        let source = Oklch::from_hex(color).unwrap();
        let luminance = premultiplied_luminance(source.to_gpu());
        for coverage in [40u8, 128, 220] {
            let fraction = f32::from(coverage) / 255.0;
            let toned = single_glyph(coverage, native, color)[3];
            let exact = single_glyph(coverage, ToneCurve::EXACT, color)[3];
            assert!(
                (toned - native.alpha(fraction, luminance)).abs() < 1.0e-5,
                "{color} {coverage}: {toned}"
            );
            assert!(
                (exact - fraction).abs() < 1.0e-5,
                "{color} {coverage}: {exact}"
            );
        }
    }
    let dark = single_glyph(128, native, "#000000")[3];
    let light = single_glyph(128, native, "#ffffff")[3];
    assert!(light > dark + 0.02, "{light} vs {dark}");
}
