use aqua_text::{FontManager, TextLayout, TextStyle, WrapMode};

fn shape(text: &str, width: Option<f32>) -> (TextLayout, std::sync::Arc<aqua_text::LaidOut>) {
    let mut fonts = FontManager::new();
    let mut layout = TextLayout::new();
    layout.set_text(text, &TextStyle::default());
    layout.set_wrap(WrapMode::Word);
    layout.set_max_width(width);
    let result = layout.layout(&mut fonts);
    (layout, result)
}

#[test]
fn wrapped_rows_have_distinct_source_ranges_and_vertical_targets() {
    let text = "alpha beta gamma delta epsilon zeta";
    let (layout, result) = shape(text, Some(75.0));
    assert!(result.lines.len() >= 3);
    for pair in result.lines.windows(2) {
        assert!(pair[0].display_range.end <= pair[1].display_range.start);
    }
    let first = &result.lines[0];
    let second = &result.lines[1];
    let target = layout
        .vertical_target(first.display_range.start, false, 0.0)
        .unwrap();
    assert_eq!(target, second.display_range.start);
    assert_eq!(
        layout.line_boundary(target, true),
        Some(second.display_range.end)
    );
    assert!(layout.caret(target).unwrap().y_top >= second.y_top);
}

#[test]
fn paragraph_navigation_skips_wrapped_rows_and_preserves_x() {
    let text = "alpha beta gamma delta epsilon zeta\nnext paragraph";
    let (layout, result) = shape(text, Some(75.0));
    let first = &result.lines[0];
    let next = result
        .lines
        .iter()
        .find(|line| line.paragraph == 1)
        .unwrap();
    assert_eq!(
        layout.paragraph_target(first.display_range.start, false, 0.0),
        Some(next.display_range.start)
    );
}

#[test]
fn rtl_hit_testing_tracks_logical_source_spans() {
    let text = "אבגד";
    let (layout, result) = shape(text, None);
    let line = &result.lines[0];
    let middle = line.y_top + line.height / 2.0;
    assert_eq!(layout.hit(-10.0, middle), Some(text.len()));
    assert_eq!(layout.hit(line.width + 10.0, middle), Some(0));
    let caret = layout.caret("אב".len()).unwrap();
    assert_eq!(layout.hit(caret.x, middle), Some("אב".len()));
}

#[test]
fn selected_combining_cluster_covers_visual_glyph_only() {
    let text = "a e\u{301} z";
    let (layout, _) = shape(text, None);
    let left = layout.caret(2).unwrap();
    let right = layout.caret(5).unwrap();
    let rectangles = layout.selection_rects(2..5);
    assert!(!rectangles.is_empty());
    let width: f32 = rectangles.iter().map(|rect| rect.w).sum();
    assert!((width - (right.x - left.x).abs()).abs() < 0.5);
}

#[test]
fn affinity_distinguishes_both_sides_of_a_soft_wrap() {
    use aqua_text::{CaretAffinity, CaretPosition};
    let text = "abcdefghijklmno";
    let mut fonts = FontManager::new();
    let mut layout = TextLayout::new();
    layout.set_text(text, &TextStyle::default());
    layout.set_wrap(WrapMode::Glyph);
    layout.set_max_width(Some(45.0));
    let result = layout.layout(&mut fonts);
    let pair = result
        .lines
        .windows(2)
        .find(|pair| pair[0].display_range.end == pair[1].display_range.start)
        .unwrap();
    let byte = pair[0].display_range.end;
    let upstream = CaretPosition {
        byte,
        affinity: CaretAffinity::Upstream,
    };
    let downstream = CaretPosition {
        byte,
        affinity: CaretAffinity::Downstream,
    };
    let before = layout.caret_position(upstream).unwrap();
    let after = layout.caret_position(downstream).unwrap();
    assert_eq!(before.y_top, pair[0].y_top);
    assert_eq!(after.y_top, pair[1].y_top);
    assert_eq!(
        layout.hit_position(before.x, before.y_top + before.height / 2.0),
        Some(upstream)
    );
    assert_eq!(
        layout.hit_position(after.x, after.y_top + after.height / 2.0),
        Some(downstream)
    );
    assert_eq!(
        layout.line_boundary_position(upstream, true),
        Some(upstream)
    );
    let right = layout.horizontal_position(upstream, false).unwrap();
    assert!(right.byte > byte);
    assert_eq!(layout.caret_position(right).unwrap().y_top, after.y_top);
    let left = layout.horizontal_position(downstream, true).unwrap();
    assert!(left.byte < byte);
    assert_eq!(layout.caret_position(left).unwrap().y_top, before.y_top);
}

#[test]
fn affinity_resolves_distinct_bidi_junction_edges() {
    use aqua_text::{CaretAffinity, CaretPosition};
    let text = "abc אבגד def";
    let (layout, _) = shape(text, None);
    let byte = "abc ".len();
    let upstream = CaretPosition {
        byte,
        affinity: CaretAffinity::Upstream,
    };
    let downstream = CaretPosition {
        byte,
        affinity: CaretAffinity::Downstream,
    };
    let before = layout.caret_position(upstream).unwrap();
    let after = layout.caret_position(downstream).unwrap();
    assert!((before.x - after.x).abs() > 1.0);
    let left = layout.horizontal_position(downstream, true).unwrap();
    assert!(layout.caret_position(left).unwrap().x < after.x);
    let right = layout.horizontal_position(upstream, false).unwrap();
    assert!(layout.caret_position(right).unwrap().x > before.x);
}

#[test]
fn word_hit_uses_the_glyph_under_pointer_instead_of_nearest_caret() {
    use aqua_text::navigation::{double_click_range, WordBehavior};
    let text = "hello world";
    let (layout, _) = shape(text, None);
    let left = layout.caret(4).unwrap();
    let right = layout.caret(5).unwrap();
    let x = left.x + (right.x - left.x) * 0.8;
    let y = left.y_top + left.height * 0.5;
    assert_eq!(layout.hit(x, y), Some(5));
    assert_eq!(layout.word_hit(x, y), Some(4));
    let range = double_click_range(
        text,
        layout.word_hit(x, y).unwrap(),
        WordBehavior::SkipSpaces,
    );
    assert_eq!(&text[range], "hello ");
}

#[test]
fn word_hit_retains_rtl_and_combining_grapheme_boundaries() {
    for text in ["אבגד next", "ae\u{301} next"] {
        let (layout, result) = shape(text, None);
        let line = &result.lines[0];
        for glyph in &line.glyphs {
            let byte = layout
                .word_hit(glyph.x + glyph.w * 0.8, line.y_top + line.height / 2.0)
                .unwrap();
            assert!(aqua_text::TextModel::new(text).is_grapheme_boundary(byte));
        }
    }
}
