use aqua_text::document::{
    SelectionDocument, SelectionSource, SelectionSourceId, SourcePosition, SourceSlice,
};

fn source(id: u64, order: &[u64], text: &str, separator: &str) -> SelectionSource {
    SelectionSource {
        id: SelectionSourceId(id),
        order: order.to_vec(),
        text: text.into(),
        separator: separator.into(),
    }
}

fn position(source: u64, offset: usize) -> SourcePosition {
    SourcePosition {
        source: SelectionSourceId(source),
        offset,
    }
}

#[test]
fn copy_uses_source_order_and_includes_unmounted_rows_with_exact_separators() {
    let mut document = SelectionDocument::new();
    document.register(source(30, &[2], "third", "\n\n"));
    document.register(source(10, &[0], "first", ""));
    document.register(source(20, &[1], "offscreen middle", "\n"));
    assert!(document.set_selection(position(10, 2), position(30, 3)));
    assert_eq!(
        document.selected_text().as_deref(),
        Some("rst\noffscreen middle\n\nthi")
    );
    assert!(document.set_selection(position(30, 3), position(10, 2)));
    assert_eq!(
        document.selected_text().as_deref(),
        Some("rst\noffscreen middle\n\nthi")
    );
    let rendered = SourceSlice {
        source: SelectionSourceId(30),
        range: 0..5,
    };
    assert_eq!(rendered.selected_range(&document), Some(0..3));
}

#[test]
fn source_slices_share_selection_without_duplicating_source_copy() {
    let mut document = SelectionDocument::new();
    document.register(source(1, &[0], "alpha beta gamma", ""));
    let first = SourceSlice {
        source: SelectionSourceId(1),
        range: 0..6,
    };
    let second = SourceSlice {
        source: SelectionSourceId(1),
        range: 6..16,
    };
    document.set_selection(first.position(2), second.position(7));
    assert_eq!(document.selected_text().as_deref(), Some("pha beta ga"));
    assert_eq!(first.selected_range(&document), Some(2..6));
    assert_eq!(second.selected_range(&document), Some(0..7));
}

#[test]
fn replacements_retain_stable_endpoints_and_clamp_to_graphemes() {
    let mut document = SelectionDocument::new();
    document.register(source(1, &[0], "abcdef", ""));
    document.register(source(2, &[1], "second", "\n"));
    document.set_selection(position(1, 2), position(2, 6));
    document.register(source(1, &[0], "a👩‍💻", ""));
    assert_eq!(document.selection().unwrap().0.offset, 1);
    document.register(source(2, &[1], "z", "\n"));
    assert_eq!(document.selected_text().as_deref(), Some("👩‍💻\nz"));
    assert!(document.unregister(SelectionSourceId(1)));
    assert_eq!(document.selection(), None);
}

#[test]
fn select_all_includes_empty_source_separators_and_rejects_unknown_endpoints() {
    let mut document = SelectionDocument::new();
    document.register(source(1, &[0], "a", ""));
    document.register(source(2, &[1], "", "\n"));
    document.register(source(3, &[2], "b", "\n"));
    assert!(document.select_all());
    assert_eq!(document.selected_text().as_deref(), Some("a\n\nb"));
    let selection = document.selection();
    assert!(!document.set_selection(position(1, 0), position(99, 0)));
    assert_eq!(document.selection(), selection);
}
