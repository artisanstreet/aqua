#![cfg_attr(not(windows), allow(unused))]
use aqua_text::{
    DecorationKind, FontManager, SpanBackground, StyledSpan, TextLayout, TextStyle, WrapMode,
};

#[cfg(windows)]
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

#[cfg(windows)]
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

#[cfg(windows)]
#[test]
fn inline_cell_does_not_compress_glyphs_wider_than_the_cell() {
    let style = TextStyle {
        size_px: 64.0,
        line_height_px: 80.0,
        ..TextStyle::default()
    };
    let mut fonts = FontManager::new();
    for scale in [1.0, 2.0] {
        let mut natural = TextLayout::new();
        natural.set_scale(scale);
        natural.set_spans(vec![StyledSpan::new("@", style.clone())]);
        let natural_width = natural.layout(&mut fonts).size.0;
        assert!(natural_width > 20.0 * scale);
        let mut cell = TextLayout::new();
        cell.set_scale(scale);
        cell.set_spans(vec![StyledSpan::new("@", style.clone())
            .inline_cell(20.0)
            .unwrap()]);
        let actual_width = cell.layout(&mut fonts).size.0;
        assert!((actual_width - natural_width).abs() < 0.01);
    }
}

#[cfg(windows)]
#[test]
fn gpui_backgrounds_follow_glyph_boundaries_and_preserve_float_colors() {
    let mut fonts = FontManager::new();
    let mut layout = TextLayout::new();
    let color = [0.5123457, 0.31, 0.67, 0.42];
    layout.set_spans(vec![
        StyledSpan::plain("plain "),
        StyledSpan::plain("colored ").gpui_background(color),
        StyledSpan::plain("run").gpui_background(color),
        StyledSpan::plain(" tail"),
    ]);
    assert!(layout.gpui_backgrounds().is_err());
    let shaped = layout.layout(&mut fonts);
    let backgrounds = layout.gpui_backgrounds().unwrap();
    assert_eq!(backgrounds.len(), 1);
    let glyphs = &shaped.lines[0].glyphs;
    let first = glyphs
        .iter()
        .find(|glyph| glyph.src_byte_start == 6)
        .unwrap();
    let last = glyphs
        .iter()
        .find(|glyph| glyph.src_byte_start == 17)
        .unwrap();
    assert_eq!(backgrounds[0].bounds.x, first.x);
    assert_eq!(backgrounds[0].bounds.w, last.x - first.x);
    assert_eq!(backgrounds[0].fill, SpanBackground::Solid(color));
}

#[cfg(windows)]
#[test]
fn selection_backgrounds_split_from_adjacent_solid_runs() {
    let mut fonts = FontManager::new();
    let mut layout = TextLayout::new();
    let color = [0.5, 0.3, 0.6, 0.4];
    layout.set_spans(vec![
        StyledSpan::plain("plain "),
        StyledSpan::plain("marked").selection_background(),
        StyledSpan::plain("solid").gpui_background(color),
        StyledSpan::plain(" tail"),
    ]);
    layout.layout(&mut fonts);
    let fills: Vec<_> = layout
        .gpui_backgrounds()
        .unwrap()
        .into_iter()
        .map(|background| background.fill)
        .collect();
    assert_eq!(
        fills,
        vec![SpanBackground::Selection, SpanBackground::Solid(color)]
    );
}
