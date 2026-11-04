use aqua_text::style::FontFeature;
use aqua_text::{FontManager, StyledSpan, TextLayout, TextStyle};

#[test]
fn font_features_change_cache_identity() {
    let style = TextStyle {
        font_features: vec![FontFeature {
            tag: *b"tnum",
            value: 1,
        }],
        ..TextStyle::default()
    };
    assert_ne!(style.cache_key(), TextStyle::default().cache_key());
}

#[test]
fn raw_base_color_yields_to_explicit_span_colors() {
    let base = [0.31, 0.7, 0.4, 0.8];
    let override_color = [0.8, 0.2, 0.6, 0.5];
    let style = TextStyle {
        gpui_color: Some(base),
        ..TextStyle::default()
    };
    let mut layout = TextLayout::new();
    layout.set_spans(vec![
        StyledSpan::new("a", style.clone()),
        StyledSpan::new("b", style.clone()).gpui_color(override_color),
        StyledSpan::new("c", style).color([1, 2, 3, 255]),
    ]);
    let shaped = layout.layout(&mut FontManager::new());
    let glyphs = &shaped.lines[0].glyphs;
    assert!(
        glyphs
            .iter()
            .any(|glyph| glyph.src_byte_start == 0 && glyph.color_hsla == Some(base))
    );
    assert!(
        glyphs
            .iter()
            .any(|glyph| glyph.src_byte_start == 1 && glyph.color_hsla == Some(override_color))
    );
    assert!(
        glyphs
            .iter()
            .any(|glyph| glyph.src_byte_start == 2 && glyph.color_hsla.is_none())
    );
}

#[test]
fn gpui_profile_snaps_line_spacing_without_changing_default_layout() {
    let mut fonts = FontManager::new();
    let mut layout = TextLayout::new();
    layout.set_text("first\nsecond", &TextStyle::default());
    layout.set_scale(1.25);
    let regular = layout.layout(&mut fonts);
    assert_eq!(regular.lines.len(), 2);
    assert_eq!(regular.lines[1].y_top - regular.lines[0].y_top, 27.5);
    layout.set_gpui_line_height(true);
    let snapped = layout.layout(&mut fonts);
    assert_eq!(snapped.lines.len(), 2);
    assert_eq!(snapped.lines[0].y_top, regular.lines[0].y_top);
    assert_eq!(snapped.lines[1].y_top - snapped.lines[0].y_top, 27.0);
    layout.set_gpui_line_height(false);
    let restored = layout.layout(&mut fonts);
    assert_eq!(restored.lines[1].y_top - restored.lines[0].y_top, 27.5);
}
