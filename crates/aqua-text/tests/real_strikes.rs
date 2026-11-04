#![cfg(windows)]

use aqua_text::raster::{place_glyph, quantize_size, Bitmap, GlyphImage, RasterKey, Rasterizer};
use aqua_text::{FontFamily, FontManager, TextLayout, TextStyle};

fn font_from_env(variable: &str) -> Option<String> {
    std::env::var(variable)
        .ok()
        .filter(|path| std::path::Path::new(path).is_file())
}

fn key_for(fonts: &mut FontManager, family: &str, text: &str, size: f32) -> RasterKey {
    let style = TextStyle {
        size_px: size,
        families: vec![FontFamily::Named(family.to_string())],
        ..TextStyle::default()
    };
    let mut layout = TextLayout::new();
    layout.set_text(text, &style);
    let laid = layout.layout(fonts);
    let glyph = &laid.lines[0].glyphs[0];
    place_glyph(glyph, 0.0, laid.lines[0].baseline_y, true).key
}

fn ink(bitmap: &Bitmap, stride: usize, channel: usize) -> f64 {
    bitmap
        .data
        .chunks_exact(stride)
        .map(|texel| f64::from(texel[channel]))
        .sum()
}

#[test]
fn cbdt_emoji_scale_from_the_nearest_strike_and_keep_ink_across_subpixel_bins() {
    let Some(path) = font_from_env("AQUA_CBDT_FONT") else {
        eprintln!("AQUA_CBDT_FONT not set; skipping");
        return;
    };
    let mut fonts = FontManager::new_without_system_fonts();
    fonts.load_font_file(&path).expect("load CBDT font");
    let mut rasterizer = Rasterizer::new();
    for size in [16.0f32, 24.0, 32.0, 48.0, 96.0] {
        let base = key_for(&mut fonts, "Noto Color Emoji", "\u{1f600}", size);
        let mut totals = Vec::new();
        for subpixel in 0..4u8 {
            let key = RasterKey {
                subpixel,
                size_bits: quantize_size(size).to_bits(),
                ..base
            };
            let GlyphImage::Color(bitmap) = rasterizer
                .rasterize(&mut fonts, &key)
                .expect("CBDT glyph rasterizes")
            else {
                panic!("expected a color bitmap");
            };
            let expected_width = (size * 136.0 / 109.0).round() as u32;
            assert!(
                bitmap.width >= expected_width && bitmap.width <= expected_width + 1,
                "size {size} subpixel {subpixel}: width {} vs {expected_width}",
                bitmap.width
            );
            assert!(
                bitmap.top > 0 && bitmap.top as f32 <= size * 1.3,
                "top {}",
                bitmap.top
            );
            totals.push(ink(&bitmap, 4, 3));
        }
        let first = totals[0];
        assert!(first > 0.0);
        for total in &totals {
            assert!(
                (total - first).abs() / first < 0.02,
                "size {size}: ink drifts across bins {totals:?}"
            );
        }
    }
}

#[test]
fn cbdt_subpixel_bins_differ_in_pixels_but_not_in_geometry_class() {
    let Some(path) = font_from_env("AQUA_CBDT_FONT") else {
        eprintln!("AQUA_CBDT_FONT not set; skipping");
        return;
    };
    let mut fonts = FontManager::new_without_system_fonts();
    fonts.load_font_file(&path).expect("load CBDT font");
    let base = key_for(&mut fonts, "Noto Color Emoji", "\u{1f600}", 32.0);
    let mut rasterizer = Rasterizer::new();
    let mut raster = |subpixel: u8| {
        let key = RasterKey { subpixel, ..base };
        rasterizer
            .rasterize(&mut fonts, &key)
            .expect("raster")
            .bitmap()
            .clone()
    };
    let (zero, two) = (raster(0), raster(2));
    assert_ne!(zero.data, two.data);
    assert!(two.width >= zero.width);
}

#[test]
fn ebdt_mask_strikes_are_used_at_their_exact_ppem_and_are_one_bit_hinted() {
    let mut fonts = FontManager::new();
    if !fonts.has_family("SimSun") {
        eprintln!("SimSun not installed; skipping");
        return;
    }
    let ppems: Vec<f32> = {
        let id = fonts.face_for_family("SimSun").expect("SimSun face");
        let data = fonts.export_face_data(id).expect("face data");
        let font = skrifa::FontRef::from_index(&data, 0).expect("font");
        use skrifa::MetadataProvider;
        font.bitmap_strikes()
            .iter()
            .map(|strike| strike.ppem())
            .collect()
    };
    assert!(!ppems.is_empty(), "SimSun carries EBDT strikes");
    let mut rasterizer = Rasterizer::new();
    let ppem = ppems[0];
    let exact = key_for(&mut fonts, "SimSun", "\u{4e2d}", ppem);
    let GlyphImage::Mask(bitmap) = rasterizer
        .rasterize(&mut fonts, &exact)
        .expect("strike glyph")
    else {
        panic!("expected a mask");
    };
    assert!(bitmap.data.iter().all(|value| *value == 0 || *value == 255));
    assert!(bitmap.data.contains(&255));
    let off_size = key_for(&mut fonts, "SimSun", "\u{4e2d}", ppem + 7.3);
    let GlyphImage::Mask(outline) = rasterizer
        .rasterize(&mut fonts, &off_size)
        .expect("outline glyph")
    else {
        panic!("expected a mask");
    };
    assert!(outline
        .data
        .iter()
        .any(|value| *value != 0 && *value != 255));
}
