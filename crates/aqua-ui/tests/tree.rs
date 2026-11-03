use aqua_core::Size;
use aqua_text::Selection;
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::style::LayoutStyle;
use aqua_ui::tree::{NodeKind, NodeSpec, TextBacking, TextContent, UiTree};

#[test]
fn selecting_static_text_clamps_to_graphemes_without_invalidating_content() {
    let mut tree = UiTree::new();
    let node = tree.mount_spec(NodeSpec::new(
        NodeKind::Text(TextContent::label("a👩‍💻b")),
        LayoutStyle::default(),
    ));
    tree.compute_layout(Size::new(300.0, 100.0), &NaiveMeasurer)
        .unwrap();
    let version = tree.content_version();
    assert!(tree.set_text_selection(node, Some(Selection::new(3, usize::MAX))));
    let NodeKind::Text(text) = &tree.get(node).unwrap().kind else {
        panic!()
    };
    assert_eq!(text.selection, Some(Selection::new(1, "a👩‍💻b".len())));
    assert_eq!(tree.content_version(), version);
    tree.compute_layout(Size::new(300.0, 100.0), &NaiveMeasurer)
        .unwrap();
    assert!(tree.last_stats().layout_skipped);
}

#[test]
fn selecting_during_composition_restores_displaced_text_and_invalidates_layout() {
    let mut tree = UiTree::new();
    let node = tree.mount_spec(NodeSpec::new(
        NodeKind::Text(TextContent::editable("draft", false)),
        LayoutStyle::default(),
    ));
    tree.edit_text(node, |text| {
        let TextBacking::Editable(editor) = &mut text.backing else {
            panic!()
        };
        editor.set_selection(Selection::new(0, 3));
        editor.set_composition("候補", 0..3);
    });
    tree.compute_layout(Size::new(300.0, 100.0), &NaiveMeasurer)
        .unwrap();
    let version = tree.content_version();
    assert!(tree.set_text_selection(node, Some(Selection::caret_at(4))));
    let NodeKind::Text(text) = &tree.get(node).unwrap().kind else {
        panic!()
    };
    let TextBacking::Editable(editor) = &text.backing else {
        panic!()
    };
    assert_eq!(editor.text(), "draft");
    assert!(!editor.is_composing());
    assert_eq!(editor.selection(), Selection::caret_at(4));
    assert!(tree.content_version() > version);
    tree.compute_layout(Size::new(300.0, 100.0), &NaiveMeasurer)
        .unwrap();
    assert!(!tree.last_stats().layout_skipped);
}
