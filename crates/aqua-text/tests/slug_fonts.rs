#![cfg(windows)]

use aqua_text::raster::{place_glyph, GlyphImage, Rasterizer};
use aqua_text::slug::{SlugCache, SlugLookup};
use aqua_text::{FontManager, FontSlant, TextLayout, TextStyle};

fn compare(text: &str, size: f32, slant: FontSlant) -> (f32, f32) {
    let mut fonts = FontManager::new();
    let style = TextStyle {
        size_px: size,
        slant,
        ..TextStyle::default()
    };
    let mut layout = TextLayout::new();
    layout.set_text(text, &style);
    let laid = layout.layout(&mut fonts);
    let glyph = &laid.lines[0].glyphs[0];
    let placement = place_glyph(glyph, 0.0, 0.0, true);
    let GlyphImage::Mask(bitmap) = Rasterizer::new()
        .rasterize(&mut fonts, &placement.key)
        .expect("raster")
    else {
        panic!("mask expected");
    };
    let mut cache = SlugCache::new();
    let SlugLookup::Outline(outline) = cache.lookup(&mut fonts, &placement.key) else {
        panic!("outline expected");
    };
    let ppem = placement.key.size_px();
    let (mut error, mut slug_ink, mut mask_ink) = (0.0f32, 0.0f32, 0.0f32);
    for row in 0..bitmap.height {
        for column in 0..bitmap.width {
            let x = (bitmap.left as f32 + column as f32 + 0.5) / ppem;
            let y = (bitmap.top as f32 - row as f32 - 0.5) / ppem;
            let slug = outline.coverage(x, y, ppem);
            let mask = f32::from(bitmap.data[(row * bitmap.width + column) as usize]) / 255.0;
            error += (slug - mask).abs();
            slug_ink += slug;
            mask_ink += mask;
        }
    }
    (
        error / (bitmap.width * bitmap.height) as f32,
        slug_ink / mask_ink,
    )
}

#[test]
fn slug_coverage_matches_the_exact_area_rasterizer_on_real_glyphs() {
    for text in ["o", "g", "S", "@", "W", "&"] {
        for size in [12.0f32, 17.0, 32.0, 77.0] {
            let (error, ratio) = compare(text, size, FontSlant::Normal);
            assert!(error < 0.045, "{text} at {size}: mean error {error}");
            assert!(
                (ratio - 1.0).abs() < 0.05,
                "{text} at {size}: ink ratio {ratio}"
            );
        }
    }
}

#[test]
fn slug_follows_the_synthetic_italic_skew() {
    for size in [24.0f32, 64.0] {
        let (error, ratio) = compare("H", size, FontSlant::Italic);
        assert!(error < 0.03, "H italic at {size}: mean error {error}");
        assert!((ratio - 1.0).abs() < 0.03, "ratio {ratio}");
    }
}

#[test]
fn cache_reuses_outlines_across_sizes_and_reports_blank_and_color_glyphs() {
    let mut fonts = FontManager::new();
    let mut cache = SlugCache::new();
    let mut lookups = Vec::new();
    for (text, size) in [
        ("o", 12.0f32),
        ("o", 96.0),
        (" ", 16.0),
        ("\u{1f600}", 32.0),
    ] {
        let mut layout = TextLayout::new();
        layout.set_text(
            text,
            &TextStyle {
                size_px: size,
                ..TextStyle::default()
            },
        );
        let laid = layout.layout(&mut fonts);
        let key = place_glyph(&laid.lines[0].glyphs[0], 0.0, 0.0, true).key;
        lookups.push(cache.lookup(&mut fonts, &key));
    }
    assert!(matches!(lookups[0], SlugLookup::Outline(_)));
    assert!(matches!(lookups[2], SlugLookup::Blank));
    assert!(matches!(lookups[3], SlugLookup::Unsupported));
    assert_eq!(cache.len(), 3);
    assert!(cache.heap_bytes() > 0);
}
