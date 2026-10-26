//! Editor + layout coordination: the exact wiring `aqua-ui` will use.
//!
//! A `TextEditor` owns committed text; while composing, `display_text()` is
//! what gets laid out, and caret/selection geometry comes back in display
//! coordinates that map onto the editor state.

use aqua_text::{FontManager, LineMode, TextEditor, TextLayout, TextStyle};

fn setup() -> (FontManager, TextStyle) {
    (FontManager::new(), TextStyle::default())
}

#[test]
fn editor_text_lays_out_with_caret() {
    let (mut fonts, style) = setup();
    let mut ed = TextEditor::with_text("hello world", LineMode::Single);
    ed.move_to_end(false);
    ed.move_word_left(true);
    assert_eq!(ed.selected_text(), Some("world"));

    let mut layout = TextLayout::new();
    layout.set_line_mode(LineMode::Single);
    layout.set_text(ed.text(), &style);
    let laid = layout.layout(&mut fonts);
    assert!(laid.size.0 > 0.0);

    // Caret at end of text matches line end; selection covers "world".
    let sel = ed.selection().range();
    let rects = layout.selection_rects(sel);
    assert!(!rects.is_empty());
    let caret = layout.caret(ed.selection().caret).unwrap();
    assert!(caret.x > 0.0);
}

#[test]
fn composing_text_lays_out_and_exposes_ime_anchor() {
    let (mut fonts, style) = setup();
    let mut ed = TextEditor::with_text("input: ", LineMode::Single);
    ed.move_to_end(false);
    ed.set_composition("にほん", 0..9);
    assert!(ed.is_composing());

    // UI lays out the display text (committed + preedit).
    let mut layout = TextLayout::new();
    layout.set_line_mode(LineMode::Single);
    layout.set_text(&ed.display_text(), &style);
    layout.layout(&mut fonts);

    // IME candidate window anchor = caret rect at the preedit cursor.
    let anchor = layout.caret(ed.ime_caret()).unwrap();
    assert!(anchor.x > 0.0);
    let range = ed.composition_range().unwrap();
    assert!(!range.is_empty());
    let rects = layout.selection_rects(range);
    assert!(!rects.is_empty());

    ed.commit_composition();
    assert_eq!(ed.text(), "input: にほん");
}

#[test]
fn shared_cache_reuses_identical_layouts() {
    use aqua_text::SharedLayoutCache;
    let (mut fonts, style) = setup();
    let mut cache = SharedLayoutCache::new(8);
    let mut a = TextLayout::new();
    a.set_text("cached label", &style);
    let first = a.layout_shared(&mut fonts, &mut cache);
    assert_eq!(cache.misses, 1);

    let mut b = TextLayout::new();
    b.set_text("cached label", &style);
    let second = b.layout_shared(&mut fonts, &mut cache);
    assert_eq!(cache.hits, 1);
    assert!(std::sync::Arc::ptr_eq(&first, &second));

    // Different scale reshapes instead of reusing.
    b.set_scale(2.0);
    let _ = b.layout_shared(&mut fonts, &mut cache);
    assert_eq!(cache.misses, 2);
}
