use aqua_text::raster::{place_glyph, quantize_x, GlyphImage, Rasterizer};
use aqua_text::{FontManager, TextLayout, TextStyle};

fn glyph_image(
    text: &str,
    size: f32,
    italic: bool,
) -> (GlyphImage, aqua_text::raster::GlyphPlacement) {
    let mut fonts = FontManager::new();
    let mut style = TextStyle {
        size_px: size,
        ..TextStyle::default()
    };
    if italic {
        style.slant = aqua_text::FontSlant::Italic;
    }
    let mut layout = TextLayout::new();
    layout.set_text(text, &style);
    let laid = layout.layout(&mut fonts);
    let glyph = &laid.lines[0].glyphs[0];
    let placement = place_glyph(glyph, 0.0, laid.lines[0].baseline_y, true);
    let image = Rasterizer::new()
        .rasterize(&mut fonts, &placement.key)
        .expect("glyph raster");
    (image, placement)
}

#[test]
fn quarter_pixel_quantization_rounds_to_nearest_step() {
    assert_eq!(quantize_x(3.0), (3, 0));
    assert_eq!(quantize_x(3.26), (3, 1));
    assert_eq!(quantize_x(3.88), (4, 0));
    assert_eq!(quantize_x(-0.3), (-1, 3));
}

#[test]
fn capital_h_has_stem_coverage_and_cap_height_extent() {
    let (image, _) = glyph_image("H", 40.0, false);
    let GlyphImage::Mask(bitmap) = image else {
        panic!("expected mask");
    };
    assert!(bitmap.top > 20 && bitmap.top < 40, "top {}", bitmap.top);
    assert!(bitmap.height as i32 >= bitmap.top - 1);
    let full = bitmap.data.iter().filter(|v| **v == 255).count();
    assert!(full > 50);
    let covered: u32 = bitmap.data.iter().map(|v| u32::from(*v)).sum();
    assert!(covered > 255 * 100);
}

#[test]
fn subpixel_offsets_shift_coverage_without_changing_total_ink() {
    let mut fonts = FontManager::new();
    let mut layout = TextLayout::new();
    layout.set_text(
        "o",
        &TextStyle {
            size_px: 24.0,
            ..TextStyle::default()
        },
    );
    let laid = layout.layout(&mut fonts);
    let glyph = &laid.lines[0].glyphs[0];
    let mut rasterizer = Rasterizer::new();
    let mut totals = Vec::new();
    for step in 0..4u8 {
        let mut placement = place_glyph(glyph, 0.0, 20.0, true);
        placement.key.subpixel = step;
        let image = rasterizer.rasterize(&mut fonts, &placement.key).unwrap();
        totals.push(
            image
                .bitmap()
                .data
                .iter()
                .map(|v| f64::from(*v))
                .sum::<f64>(),
        );
    }
    for total in &totals {
        assert!((total - totals[0]).abs() / totals[0] < 0.02, "{totals:?}");
    }
}

#[test]
fn bold_requests_never_lose_ink_and_fake_bold_widens_stems() {
    let (_, placement) = glyph_image("H", 24.0, false);
    let mut fonts = FontManager::new();
    let regular = placement.key;
    let bold = aqua_text::raster::RasterKey {
        weight: 700,
        ..regular
    };
    let ink = |fonts: &mut FontManager, key: &aqua_text::raster::RasterKey| -> (u64, u32) {
        let image = Rasterizer::new().rasterize(fonts, key).expect("raster");
        let bitmap = image.bitmap();
        (
            bitmap.data.iter().map(|v| u64::from(*v)).sum(),
            bitmap.width,
        )
    };
    let (regular_ink, regular_width) = ink(&mut fonts, &regular);
    let (bold_ink, bold_width) = ink(&mut fonts, &bold);
    assert!(bold_ink >= regular_ink);
    let synthetic = fonts
        .entry_with(bold.font_id, 700, bold.stretch, false)
        .expect("entry")
        .synthetic_bold();
    if synthetic {
        assert!(bold_ink as f64 > regular_ink as f64 * 1.08);
        assert!(bold_width >= regular_width);
    }
}
