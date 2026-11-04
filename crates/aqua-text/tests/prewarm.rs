#![cfg(windows)]

use aqua_text::{FontManager, PrewarmedEntry, TextLayout, TextStyle};

fn style() -> TextStyle {
    TextStyle {
        size_px: 15.0,
        ..TextStyle::default()
    }
}

fn build_on_workers(fonts: &mut FontManager) -> Vec<PrewarmedEntry> {
    let jobs = fonts.prewarm_jobs(&[style()]);
    std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .into_iter()
            .map(|job| scope.spawn(move || job.build()))
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap())
            .collect()
    })
}

fn glyph_ids(fonts: &mut FontManager) -> Vec<u16> {
    let mut layout = TextLayout::new();
    layout.set_text("Prewarm 123", &style());
    layout.layout(fonts).lines[0]
        .glyphs
        .iter()
        .map(|glyph| glyph.glyph_id)
        .collect()
}

#[test]
fn faces_built_on_workers_install_once_and_lay_out_identically() {
    let expected = glyph_ids(&mut FontManager::new());
    let mut fonts = FontManager::new();
    let built = build_on_workers(&mut fonts);
    assert!(!built.is_empty());
    let installed = built
        .into_iter()
        .filter(|entry| fonts_install(&mut fonts, entry))
        .count();
    assert!(installed > 0);
    assert!(fonts.prewarm_jobs(&[style()]).is_empty());
    assert_eq!(glyph_ids(&mut fonts), expected);
}

fn fonts_install(fonts: &mut FontManager, entry: &PrewarmedEntry) -> bool {
    fonts.install_prewarmed(entry.clone())
}

#[test]
fn entries_built_before_the_font_set_changed_are_discarded() {
    let mut fonts = FontManager::new();
    let built = build_on_workers(&mut fonts);
    assert!(!built.is_empty());
    fonts.load_font_bytes(Vec::new());
    let installed = built
        .into_iter()
        .filter(|entry| fonts_install(&mut fonts, entry))
        .count();
    assert_eq!(installed, 0);
}
