#![cfg(windows)]

use aqua_text::{
    FontManager, FontWeight, LaidOut, LineMode, StyledSpan, TextLayout, TextStyle, WrapMode,
};

fn layout(fonts: &mut FontManager, spans: Vec<StyledSpan>, width: f32) -> std::sync::Arc<LaidOut> {
    let mut layout = TextLayout::new();
    layout.set_spans(spans);
    layout.set_line_mode(LineMode::Multi);
    layout.set_wrap(WrapMode::Word);
    layout.set_max_width(Some(width));
    layout.layout(fonts)
}

fn plain(text: &str) -> Vec<StyledSpan> {
    vec![StyledSpan::new(text, TextStyle::default())]
}

fn signature(laid: &LaidOut) -> Vec<(u16, u32, u32)> {
    laid.lines
        .iter()
        .flat_map(|line| &line.glyphs)
        .map(|glyph| (glyph.glyph_id, glyph.x.to_bits(), glyph.y.to_bits()))
        .collect()
}

const TEXT: &str = "first paragraph of the block\nsecond paragraph with more words in it\nthird";

#[test]
fn a_new_layout_of_unchanged_text_shapes_nothing() {
    let mut fonts = FontManager::new();
    layout(&mut fonts, plain(TEXT), 300.0);
    let before = fonts.shape_cache_stats();
    layout(&mut fonts, plain(TEXT), 140.0);
    let after = fonts.shape_cache_stats();
    assert_eq!(after.misses, before.misses);
    assert_eq!(after.hits, before.hits + 3);
}

#[test]
fn appending_text_shapes_only_the_changed_paragraph() {
    let mut fonts = FontManager::new();
    layout(&mut fonts, plain(TEXT), 300.0);
    let before = fonts.shape_cache_stats();
    layout(
        &mut fonts,
        plain(&format!("{TEXT} and a streamed token")),
        300.0,
    );
    let after = fonts.shape_cache_stats();
    assert_eq!(after.misses, before.misses + 1);
    assert_eq!(after.hits, before.hits + 2);
}

#[test]
fn a_style_change_is_not_served_from_the_cache() {
    let mut fonts = FontManager::new();
    let regular = layout(&mut fonts, plain(TEXT), 300.0);
    let before = fonts.shape_cache_stats();
    let bold = TextStyle {
        weight: FontWeight::BOLD,
        ..TextStyle::default()
    };
    let heavy = layout(&mut fonts, vec![StyledSpan::new(TEXT, bold)], 300.0);
    assert_eq!(fonts.shape_cache_stats().misses, before.misses + 3);
    assert_ne!(signature(&regular), signature(&heavy));
}

#[test]
fn a_span_boundary_inside_a_paragraph_changes_the_key() {
    let mut fonts = FontManager::new();
    let bold = TextStyle {
        weight: FontWeight::BOLD,
        ..TextStyle::default()
    };
    let split = |at: usize| {
        vec![
            StyledSpan::new(&TEXT[..at], TextStyle::default()),
            StyledSpan::new(&TEXT[at..], bold.clone()),
        ]
    };
    let first = layout(&mut fonts, split(5), 300.0);
    let second = layout(&mut fonts, split(9), 300.0);
    assert_ne!(signature(&first), signature(&second));
}

#[test]
fn cached_shaping_is_identical_to_shaping_from_scratch() {
    let mut warm = FontManager::new();
    layout(&mut warm, plain(TEXT), 300.0);
    let cached = layout(&mut warm, plain(TEXT), 300.0);
    let mut cold = FontManager::new();
    let fresh = layout(&mut cold, plain(TEXT), 300.0);
    assert_eq!(signature(&cached), signature(&fresh));
    assert_eq!(cached.size, fresh.size);
}

#[test]
fn the_cache_stays_within_twice_its_budget() {
    let mut fonts = FontManager::new();
    for n in 0..4000 {
        layout(
            &mut fonts,
            plain(&format!(
                "paragraph number {n} with some padding text to give it body"
            )),
            300.0,
        );
    }
    let stats = fonts.shape_cache_stats();
    assert!(stats.rotations > 0, "{stats:?}");
    assert!(
        stats.bytes <= 2 * aqua_text::SHAPE_CACHE_BUDGET_BYTES,
        "{stats:?}"
    );
}
