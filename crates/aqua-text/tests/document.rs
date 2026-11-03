use aqua_text::document::{
    SelectionDocument, SelectionSource, SelectionSourceId, SourcePosition, SourceSlice,
};

fn source(id: u64, order: &[u64], text: &str, separator: &str) -> SelectionSource {
    SelectionSource {
        id: SelectionSourceId(id),
        order: order.to_vec(),
        text: text.into(),
        separator: separator.into(),
        copies: Vec::new(),
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
    document
        .register(source(30, &[2], "third", "\n\n"))
        .unwrap();
    document.register(source(10, &[0], "first", "")).unwrap();
    document
        .register(source(20, &[1], "offscreen middle", "\n"))
        .unwrap();
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
    document
        .register(source(1, &[0], "alpha beta gamma", ""))
        .unwrap();
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
    document.register(source(1, &[0], "abcdef", "")).unwrap();
    document.register(source(2, &[1], "second", "\n")).unwrap();
    document.set_selection(position(1, 2), position(2, 6));
    document.register(source(1, &[0], "a👩‍💻", "")).unwrap();
    assert_eq!(document.selection().unwrap().0.offset, 1);
    document.register(source(2, &[1], "z", "\n")).unwrap();
    assert_eq!(document.selected_text().as_deref(), Some("👩‍💻\nz"));
    assert!(document.unregister(SelectionSourceId(1)));
    assert_eq!(document.selection(), None);
}

#[test]
fn select_all_includes_empty_source_separators_and_rejects_unknown_endpoints() {
    let mut document = SelectionDocument::new();
    document.register(source(1, &[0], "a", "")).unwrap();
    document.register(source(2, &[1], "", "\n")).unwrap();
    document.register(source(3, &[2], "b", "\n")).unwrap();
    assert!(document.select_all());
    assert_eq!(document.selected_text().as_deref(), Some("a\n\nb"));
    let selection = document.selection();
    assert!(!document.set_selection(position(1, 0), position(99, 0)));
    assert_eq!(document.selection(), selection);
}

#[test]
fn copy_substitutions_preserve_literals_and_omit_partially_selected_slots() {
    use aqua_text::document::CopySubstitution;
    let mut document = SelectionDocument::new();
    let mut item = source(1, &[0], "before [swatch] after", "");
    item.copies = vec![CopySubstitution {
        range: 7..15,
        replacement: "#ff0000".into(),
    }];
    document.register(item).unwrap();
    document.select_all();
    assert_eq!(
        document.selected_text().as_deref(),
        Some("before #ff0000 after")
    );
    document.set_selection(position(1, 10), position(1, 21));
    assert_eq!(document.selected_text().as_deref(), Some(" after"));
    document.set_selection(position(1, 0), position(1, 11));
    assert_eq!(document.selected_text().as_deref(), Some("before "));
}

#[test]
fn copy_substitutions_are_sorted_and_can_remove_decorative_slots() {
    use aqua_text::document::CopySubstitution;
    let mut document = SelectionDocument::new();
    let mut item = source(1, &[0], "■ image ⓘ", "");
    item.copies = vec![
        CopySubstitution {
            range: "■ image ".len().."■ image ⓘ".len(),
            replacement: "".into(),
        },
        CopySubstitution {
            range: 0.."■".len(),
            replacement: "[preview]".into(),
        },
    ];
    document.register(item).unwrap();
    document.select_all();
    assert_eq!(
        document.selected_text().as_deref(),
        Some("[preview] image ")
    );
}

#[test]
fn malformed_copy_substitutions_are_rejected_without_partial_batch_registration() {
    use aqua_text::document::{CopySubstitution, SelectionSourceError};
    let mut document = SelectionDocument::new();
    document.register(source(1, &[0], "original", "")).unwrap();
    for range in [0..0, 0..99, 2..3] {
        let mut invalid = source(2, &[1], "a👩‍💻b", "\n");
        invalid.copies = vec![CopySubstitution {
            range,
            replacement: "alt".into(),
        }];
        assert!(matches!(
            document.register(invalid),
            Err(SelectionSourceError::InvalidCopyRange { .. })
        ));
        assert_eq!(document.source_ids(), &[SelectionSourceId(1)]);
    }
    let mut invalid = source(2, &[1], "abcdef", "\n");
    invalid.copies = vec![
        CopySubstitution {
            range: 0..3,
            replacement: "a".into(),
        },
        CopySubstitution {
            range: 2..4,
            replacement: "b".into(),
        },
    ];
    assert!(matches!(
        document.register_many([source(3, &[2], "new", "\n"), invalid]),
        Err(SelectionSourceError::OverlappingCopyRanges { .. })
    ));
    assert_eq!(document.source_ids(), &[SelectionSourceId(1)]);
}
