#![cfg(windows)]

use aqua_text::raster::{GlyphImage, Rasterizer, place_glyph};
use aqua_text::{FontManager, TextLayout, TextStyle};

#[test]
fn emoji_falls_back_to_a_color_font_and_rasterizes_rgba() {
    let mut fonts = FontManager::new();
    let mut layout = TextLayout::new();
    layout.set_text(
        "a\u{1f600}b",
        &TextStyle {
            size_px: 32.0,
            ..TextStyle::default()
        },
    );
    let laid = layout.layout(&mut fonts);
    let glyphs = &laid.lines[0].glyphs;
    assert_eq!(glyphs.len(), 3);
    assert_ne!(glyphs[0].font_id, glyphs[1].font_id);
    assert_eq!(glyphs[0].font_id, glyphs[2].font_id);
    let placement = place_glyph(&glyphs[1], 0.0, laid.lines[0].baseline_y, true);
    let image = Rasterizer::new()
        .rasterize(&mut fonts, &placement.key)
        .expect("emoji raster");
    let GlyphImage::Color(bitmap) = image else {
        panic!("expected a color bitmap");
    };
    assert!(bitmap.width >= 20 && bitmap.height >= 20);
    let opaque = bitmap
        .data
        .chunks_exact(4)
        .filter(|px| px[3] == 255)
        .count();
    assert!(opaque > 200, "opaque pixels: {opaque}");
    let yellowish = bitmap
        .data
        .chunks_exact(4)
        .filter(|px| px[3] == 255 && px[0] > 200 && px[1] > 140 && px[2] < 120)
        .count();
    assert!(yellowish > 100, "yellowish pixels: {yellowish}");
}
