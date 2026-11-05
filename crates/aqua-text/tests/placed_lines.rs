#![cfg(windows)]

use aqua_text::{
    FontManager, LaidOut, LineMode, StyledSpan, TextAlign, TextLayout, TextStyle, WrapMode,
};

const STREAM: &str = "First paragraph that is long enough to wrap across several lines at this width.\n\
Second paragraph with different words and a tail that keeps growing token by token while streaming.\n\
Third paragraph.";

fn configured(align: TextAlign, width: Option<f32>) -> TextLayout {
    let mut layout = TextLayout::new();
    layout.set_line_mode(LineMode::Multi);
    layout.set_wrap(WrapMode::Word);
    layout.set_align(align);
    layout.set_max_width(width);
    layout
}

fn spans_for(text: &str, style: &TextStyle) -> Vec<StyledSpan> {
    let split = text.len().min(40);
    let split = (0..=split)
        .rev()
        .find(|index| text.is_char_boundary(*index))
        .unwrap_or(0);
    vec![
        StyledSpan::new(&text[..split], style.clone()).color([200, 40, 40, 255]),
        StyledSpan::new(&text[split..], style.clone()).color([40, 40, 200, 255]),
    ]
}

fn signature(laid: &LaidOut) -> String {
    format!(
        "{:?}|{:?}|{:?}",
        laid.size, laid.first_baseline_y, laid.lines
    )
}

fn tokens(text: &str) -> Vec<usize> {
    text.char_indices()
        .map(|(index, ch)| index + ch.len_utf8())
        .step_by(7)
        .collect()
}

#[test]
fn streamed_growth_matches_a_fresh_layout_at_every_step() {
    let style = TextStyle::default();
    for (align, width) in [
        (TextAlign::Start, Some(220.0)),
        (TextAlign::Center, Some(220.0)),
        (TextAlign::Justify, Some(220.0)),
        (TextAlign::Right, None),
    ] {
        let mut fonts = FontManager::new();
        let mut streaming = configured(align, width);
        for end in tokens(STREAM) {
            let text = &STREAM[..end];
            streaming.set_spans(spans_for(text, &style));
            let incremental = streaming.layout(&mut fonts);
            let mut fresh = configured(align, width);
            fresh.set_spans(spans_for(text, &style));
            let expected = fresh.layout(&mut FontManager::new());
            assert_eq!(
                signature(&incremental),
                signature(&expected),
                "{align:?} {width:?} at {end}"
            );
        }
    }
}

#[test]
fn width_changes_replace_cached_lines() {
    let style = TextStyle::default();
    let mut fonts = FontManager::new();
    let mut layout = configured(TextAlign::Start, Some(400.0));
    layout.set_spans(spans_for(STREAM, &style));
    let wide = layout.layout(&mut fonts);
    layout.set_max_width(Some(150.0));
    let narrow = layout.layout(&mut fonts);
    assert!(narrow.lines.len() > wide.lines.len());
    layout.set_max_width(Some(400.0));
    let again = layout.layout(&mut fonts);
    assert_eq!(signature(&wide), signature(&again));
}

#[test]
fn recoloring_an_earlier_span_changes_only_that_paragraph() {
    let style = TextStyle::default();
    let mut fonts = FontManager::new();
    let mut layout = configured(TextAlign::Start, Some(220.0));
    layout.set_spans(vec![
        StyledSpan::new(STREAM, style.clone()).color([1, 2, 3, 255]),
    ]);
    let before = layout.layout(&mut fonts);
    layout.set_spans(vec![
        StyledSpan::new(STREAM, style.clone()).color([9, 9, 9, 255]),
    ]);
    let after = layout.layout(&mut fonts);
    assert!(
        before
            .lines
            .iter()
            .flat_map(|line| &line.glyphs)
            .all(|glyph| glyph.color_srgb == Some([1, 2, 3, 255]))
    );
    assert!(
        after
            .lines
            .iter()
            .flat_map(|line| &line.glyphs)
            .all(|glyph| glyph.color_srgb == Some([9, 9, 9, 255]))
    );
}
