use aqua_text::{CaretAffinity, CaretPosition, FontManager, TextLayout, TextStyle, WrapMode};

#[test]
fn wrapped_lines_share_one_paragraph_and_keep_selection_boundaries() {
    let mut fonts = FontManager::new();
    let mut layout = TextLayout::new();
    layout.set_text("alpha beta gamma", &TextStyle::default());
    layout.set_wrap(WrapMode::Word);
    layout.set_max_width(Some(55.0));
    layout.set_gpui_wrap(true);
    let wrapped = layout.layout(&mut fonts);
    assert!(wrapped.lines.len() > 1);
    assert!(wrapped.lines.iter().all(|line| line.paragraph == 0));
    let shown: usize = wrapped.lines.iter().map(|line| line.glyphs.len()).sum();
    assert_eq!(shown, "alpha beta gamma".len());
    let boundary = wrapped.lines[1].display_range.start;
    let up = layout
        .caret_position(CaretPosition {
            byte: boundary,
            affinity: CaretAffinity::Upstream,
        })
        .unwrap();
    let down = layout
        .caret_position(CaretPosition {
            byte: boundary,
            affinity: CaretAffinity::Downstream,
        })
        .unwrap();
    assert!(down.y_top > up.y_top);
    let selection = layout.selection_rects(0.."alpha beta gamma".len());
    assert!(selection.len() >= 2);
}

#[test]
fn unicode_clusters_and_rtl_keep_valid_byte_mapping() {
    for text in ["café e\u{301} 你好世界 abc", "אבג דהו זחט abc"] {
        let mut fonts = FontManager::new();
        let mut layout = TextLayout::new();
        layout.set_text(text, &TextStyle::default());
        layout.set_wrap(WrapMode::Word);
        layout.set_max_width(Some(48.0));
        layout.set_gpui_wrap(true);
        let wrapped = layout.layout(&mut fonts);
        for glyph in wrapped.lines.iter().flat_map(|line| &line.glyphs) {
            assert!(text.is_char_boundary(glyph.display_byte_start));
            assert!(text.is_char_boundary(glyph.display_byte_end));
            assert!(glyph.display_byte_start <= glyph.display_byte_end);
        }
    }
}
