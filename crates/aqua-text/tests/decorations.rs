use aqua_text::{DecorationKind, FontManager, StyledSpan, TextLayout, TextStyle, WrapMode};

#[test]
fn decorations_follow_styled_source_ranges_across_wrapped_lines() {
    let mut fonts = FontManager::new();
    let mut layout = TextLayout::new();
    layout.set_spans(vec![
        StyledSpan::new("plain ", TextStyle::default()),
        StyledSpan::new(
            "underlined text wrapping across rows",
            TextStyle {
                underline: true,
                strikethrough: true,
                ..Default::default()
            },
        )
        .color([200, 50, 20, 255]),
    ]);
    layout.set_wrap(WrapMode::Word);
    layout.set_max_width(Some(90.0));
    let shaped = layout.layout(&mut fonts);
    let decorations = layout.decorations();
    assert!(decorations.len() >= 4);
    for decoration in &decorations {
        let line = shaped
            .lines
            .iter()
            .find(|line| {
                line.y_top <= decoration.bounds.y && decoration.bounds.y <= line.y_top + line.height
            })
            .unwrap();
        assert!(decoration.bounds.w > 0.0);
        assert_eq!(decoration.color_srgb, Some([200, 50, 20, 255]));
        match decoration.kind {
            DecorationKind::Underline => assert!(decoration.bounds.y >= line.baseline_y),
            DecorationKind::Strikethrough => assert!(decoration.bounds.y < line.baseline_y),
        }
    }
    let first = decorations
        .iter()
        .find(|decoration| decoration.kind == DecorationKind::Underline)
        .unwrap();
    assert!(first.bounds.x >= layout.caret("plain ".len()).unwrap().x);
}

#[test]
fn interaction_ids_preserve_exact_source_offsets_through_styling() {
    let mut layout = TextLayout::new();
    layout.set_spans(vec![
        StyledSpan::plain("prefix "),
        StyledSpan::plain("你好").link(12),
        StyledSpan::plain(" and "),
        StyledSpan::plain("reference").link(40),
    ]);
    let ranges = layout.interaction_ranges();
    assert_eq!(ranges[0].range, 7..13);
    assert_eq!(ranges[0].id, 12);
    assert_eq!(ranges[1].range, 18..27);
}

#[test]
fn inline_cells_keep_exact_advance_wrapping_and_source_hit_mapping() {
    use aqua_text::{FontManager, InlineCellError, StyledSpan, TextLayout, TextStyle, WrapMode};
    let style = TextStyle::default();
    let cell = StyledSpan::new("@", style.clone())
        .color([0, 0, 0, 0])
        .inline_cell(20.0)
        .unwrap();
    let mut fonts = FontManager::new();
    let mut layout = TextLayout::new();
    layout.set_spans(vec![cell.clone(), StyledSpan::new("name", style.clone())]);
    layout.set_scale(2.0);
    layout.layout(&mut fonts);
    let rects = layout.selection_rects(0..1);
    assert_eq!(rects.len(), 1);
    assert!((rects[0].w - 40.0).abs() < 0.1);
    assert_eq!(
        layout.word_hit(35.0, rects[0].y + rects[0].h * 0.5),
        Some(0)
    );
    layout.set_wrap(WrapMode::Glyph);
    layout.set_max_width(Some(21.0));
    let wrapped = layout.layout(&mut fonts);
    assert!(wrapped.lines.len() > 1);
    assert!((layout.selection_rects(0..1)[0].w - 40.0).abs() < 0.1);
    layout.set_mask(Some('•'));
    layout.layout(&mut fonts);
    assert!((layout.selection_rects(0..1)[0].w - 40.0).abs() > 1.0);
    assert_eq!(
        StyledSpan::plain("@name").inline_cell(20.0).unwrap_err(),
        InlineCellError::ExpectedOneGrapheme
    );
    assert_eq!(
        StyledSpan::plain("@").inline_cell(f32::NAN).unwrap_err(),
        InlineCellError::InvalidWidth
    );
}
