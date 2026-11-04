use aqua_text::{FontManager, TextLayout, TextStyle};

fn layout_glyph_fonts(fonts: &mut FontManager, text: &str) -> Vec<String> {
    let mut layout = TextLayout::new();
    layout.set_text(
        text,
        &TextStyle {
            size_px: 16.0,
            ..TextStyle::default()
        },
    );
    let laid = layout.layout(fonts);
    laid.lines[0]
        .glyphs
        .iter()
        .map(|glyph| format!("{:?}:{}", glyph.font_id, glyph.glyph_id))
        .collect()
}

#[test]
fn indexed_fallback_resolves_like_the_direct_scan_and_persists() {
    let text = "a\u{4f60}\u{597d}\u{5d0}\u{20ac}";
    let mut direct = FontManager::new();
    let expected = layout_glyph_fonts(&mut direct, text);

    let mut indexed = FontManager::new();
    indexed.use_fallback_index(None);
    let first = layout_glyph_fonts(&mut indexed, text);
    assert_eq!(first, expected);
    let snapshot = indexed.fallback_index_snapshot();
    assert_eq!(snapshot.is_some(), indexed.fallback_index_len() > 0);

    let mut warmed = FontManager::new();
    warmed.use_fallback_index(snapshot.as_deref());
    assert_eq!(warmed.fallback_index_len(), indexed.fallback_index_len());
    assert_eq!(layout_glyph_fonts(&mut warmed, text), expected);
    assert!(warmed.fallback_index_snapshot().is_none());
}

#[test]
fn a_corrupt_persisted_index_is_ignored() {
    let mut fonts = FontManager::new();
    fonts.use_fallback_index(Some(b"garbage"));
    assert_eq!(fonts.fallback_index_len(), 0);
}
