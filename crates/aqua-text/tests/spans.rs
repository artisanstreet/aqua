use aqua_text::{FontManager, FontWeight, SharedLayoutCache, StyledSpan, TextLayout, TextStyle};

#[test]
fn shared_cache_distinguishes_styled_boundaries_and_colors() {
    let mut fonts = FontManager::new();
    let mut cache = SharedLayoutCache::new(8);
    let plain = TextStyle::default();
    let bold = TextStyle {
        weight: FontWeight::BOLD,
        size_px: 28.0,
        ..plain.clone()
    };
    let mut layout = TextLayout::new();
    layout.set_spans(vec![
        StyledSpan::new("a", plain.clone()),
        StyledSpan::new("Wi", bold.clone()),
    ]);
    let first = layout.layout_shared(&mut fonts, &mut cache);
    layout.set_spans(vec![
        StyledSpan::new("aW", plain.clone()),
        StyledSpan::new("i", bold),
    ]);
    let changed_boundary = layout.layout_shared(&mut fonts, &mut cache);
    assert!(!std::sync::Arc::ptr_eq(&first, &changed_boundary));
    assert_ne!(first.size, changed_boundary.size);
    layout.set_spans(vec![
        StyledSpan::new("aWi", plain.clone()).color([255, 0, 0, 255])
    ]);
    let red = layout.layout_shared(&mut fonts, &mut cache);
    layout.set_spans(vec![StyledSpan::new("aWi", plain).color([0, 255, 0, 255])]);
    let green = layout.layout_shared(&mut fonts, &mut cache);
    assert!(!std::sync::Arc::ptr_eq(&red, &green));
    assert_eq!(cache.misses, 4);
}
