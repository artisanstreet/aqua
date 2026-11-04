use aqua_text::{CaretAffinity, CaretPosition, FontManager, TextLayout, TextStyle, WrapMode};

#[test]
fn wrapped_lines_retain_buffer_glyphs_and_selection_boundaries() {
    let mut fonts = FontManager::new();
    let mut layout = TextLayout::new();
    layout.set_text("alpha beta gamma", &TextStyle::default());
    layout.set_wrap(WrapMode::Word);
    layout.set_max_width(Some(55.0));
    layout.set_gpui_wrap(true);
    let wrapped = layout.layout(&mut fonts);
    assert!(wrapped.lines.len() > 1);
    let buffer = layout.buffer().unwrap();
    let runs: Vec<_> = buffer.layout_runs().collect();
    assert_eq!(runs.len(), 1);
    for glyph in wrapped.lines.iter().flat_map(|line| &line.glyphs) {
        assert_eq!(
            runs[glyph.buffer_line_index].glyphs[glyph.buffer_glyph_index].glyph_id,
            glyph.glyph_id
        );
    }
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
fn unicode_clusters_and_rtl_keep_original_buffer_mapping() {
    for text in ["café e\u{301} 你好世界 abc", "אבג דהו זחט abc"] {
        let mut fonts = FontManager::new();
        let mut layout = TextLayout::new();
        layout.set_text(text, &TextStyle::default());
        layout.set_wrap(WrapMode::Word);
        layout.set_max_width(Some(48.0));
        layout.set_gpui_wrap(true);
        let wrapped = layout.layout(&mut fonts);
        let runs: Vec<_> = layout.buffer().unwrap().layout_runs().collect();
        for glyph in wrapped.lines.iter().flat_map(|line| &line.glyphs) {
            assert!(text.is_char_boundary(glyph.display_byte_start));
            assert!(text.is_char_boundary(glyph.display_byte_end));
            assert_eq!(
                runs[glyph.buffer_line_index].glyphs[glyph.buffer_glyph_index].glyph_id,
                glyph.glyph_id
            );
        }
    }
}
