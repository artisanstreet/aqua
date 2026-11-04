use std::sync::Arc;

use aqua_core::{
    InputEvent, Key, KeyEvent, Modifiers, Point, PointerButton, PointerEvent, PointerKind,
};
use aqua_runtime::{Runtime, RuntimeError};
use aqua_text::{
    document::{
        CopySubstitution, SelectionDocumentId, SelectionSource, SelectionSourceId, SourceSlice,
    },
    SharedFonts, SharedRuns,
};
use aqua_ui::{
    element::ui::Text,
    style::{px, Style},
};

#[test]
fn declared_source_substitutions_copy_inline_slot_literals() {
    let mut app = Runtime::new(SharedFonts::new(), SharedRuns::new());
    app.register_selection_source(SelectionSource {
        id: SelectionSourceId(1),
        order: vec![0],
        text: Arc::from("paint ■ image ◇"),
        separator: Arc::from(""),
        copies: vec![
            CopySubstitution {
                range: 6..9,
                replacement: Arc::from("#ff0000"),
            },
            CopySubstitution {
                range: 16..19,
                replacement: Arc::from("image.png"),
            },
        ],
    })
    .unwrap();
    app.mount(|_| {
        Text::builder()
            .arg("paint ■ image ◇")
            .selection_source(SourceSlice {
                source: SelectionSourceId(1),
                range: 0..19,
            })
            .style(Style::new().width(px(300.0)).height(px(40.0)))
            .build()
    })
    .unwrap();
    app.set_viewport(400.0, 200.0, 1.0);
    app.build_frame().unwrap();
    app.handle_input(&InputEvent::PointerDown(
        PointerEvent {
            contact: 0,
            kind: PointerKind::Mouse,
            position: Point::new(4.0, 8.0),
            buttons: 1,
        },
        PointerButton::Primary,
    ));
    app.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Char('a'),
        modifiers: Modifiers::CONTROL,
        repeat: false,
    }));
    assert_eq!(
        app.selected_text().as_deref(),
        Some("paint #ff0000 image image.png")
    );
}

#[test]
fn dispose_releases_active_and_inactive_registered_sources_and_rejects_remount() {
    let text: Arc<str> = Arc::from("a retained source document");
    let mut app = Runtime::new(SharedFonts::new(), SharedRuns::new());
    for document in [SelectionDocumentId(0), SelectionDocumentId(1)] {
        app.register_selection_source_in(
            document,
            SelectionSource {
                id: SelectionSourceId(0),
                order: vec![0],
                text: text.clone(),
                separator: Arc::from(""),
                copies: Vec::new(),
            },
        )
        .unwrap();
    }
    assert_eq!(Arc::strong_count(&text), 3);
    app.mount(|_| {
        Text::builder()
            .arg("a retained source document")
            .selection_source(SourceSlice {
                source: SelectionSourceId(0),
                range: 0..text.len(),
            })
            .build()
    })
    .unwrap();
    app.dispose();
    assert_eq!(Arc::strong_count(&text), 1);
    assert!(matches!(
        app.mount(|_| Text::builder().arg("new").build()),
        Err(RuntimeError::Disposed)
    ));
    assert!(matches!(app.build_frame(), Err(RuntimeError::Disposed)));
    app.dispose();
    assert_eq!(Arc::strong_count(&text), 1);
}

#[test]
fn prepared_row_registration_is_atomic_across_documents() {
    use aqua_ui::collection::{List, PreparedCollectionRow};
    use std::cell::Cell;
    use std::rc::Rc;

    let valid: Arc<str> = Arc::from("valid body");
    let invalid: Arc<str> = Arc::from("other body");
    let failure = Rc::new(Cell::new(true));
    let mut app = Runtime::new(SharedFonts::new(), SharedRuns::new());
    let first = Arc::clone(&valid);
    let second = Arc::clone(&invalid);
    let should_fail = Rc::clone(&failure);
    app.mount(move |_| {
        let first = Arc::clone(&first);
        let second = Arc::clone(&second);
        let should_fail = Rc::clone(&should_fail);
        List::new()
            .items([0usize])
            .key(|index| *index)
            .try_row(move |_| {
                Ok(PreparedCollectionRow {
                    element: Text::builder().arg("valid body").build(),
                    selection_sources: vec![
                        (
                            SelectionDocumentId(11),
                            SelectionSource {
                                id: SelectionSourceId(1),
                                order: vec![1],
                                text: Arc::clone(&first),
                                separator: Arc::from(""),
                                copies: Vec::new(),
                            },
                        ),
                        (
                            SelectionDocumentId(12),
                            SelectionSource {
                                id: SelectionSourceId(2),
                                order: vec![2],
                                text: Arc::clone(&second),
                                separator: Arc::from(""),
                                copies: if should_fail.get() {
                                    vec![CopySubstitution {
                                        range: 0..999,
                                        replacement: Arc::from("x"),
                                    }]
                                } else {
                                    Vec::new()
                                },
                            },
                        ),
                    ],
                })
            })
            .virtualize()
            .build()
    })
    .unwrap();
    app.set_viewport(200.0, 100.0, 1.0);
    let before = Arc::strong_count(&valid);
    assert!(app.build_frame().is_err());
    assert_eq!(Arc::strong_count(&valid), before);
    failure.set(false);
    app.build_frame().unwrap();
    assert!(Arc::strong_count(&valid) > before);
    assert!(app.unregister_selection_source_in(SelectionDocumentId(11), SelectionSourceId(1)));
    assert!(app.unregister_selection_source_in(SelectionDocumentId(12), SelectionSourceId(2)));
}

#[test]
fn shared_batches_retry_atomically_and_reregister_after_removal() {
    use aqua_ui::collection::{
        BatchedCollectionRow, List, SelectionBatchOwner, SelectionSourceBatch,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    let first_owner = SelectionBatchOwner::new();
    let second_owner = SelectionBatchOwner::new();
    let first_text: Arc<str> = Arc::from("first body");
    let second_text: Arc<str> = Arc::from("second body");
    let source = |id, text: Arc<str>, invalid| SelectionSource {
        id: SelectionSourceId(id),
        order: vec![id],
        text,
        separator: Arc::from(""),
        copies: if invalid {
            vec![CopySubstitution {
                range: 0..999,
                replacement: Arc::from("x"),
            }]
        } else {
            Vec::new()
        },
    };
    let first = Arc::new(SelectionSourceBatch::new(
        SelectionDocumentId(21),
        first_owner,
        1,
        Arc::from([source(1, Arc::clone(&first_text), false)]),
    ));
    let invalid = Arc::new(SelectionSourceBatch::new(
        SelectionDocumentId(22),
        second_owner,
        1,
        Arc::from([source(2, Arc::clone(&second_text), true)]),
    ));
    let batches = Rc::new(RefCell::new(vec![Arc::clone(&first), invalid]));
    let row_batches = Rc::clone(&batches);
    let mut app = Runtime::new(SharedFonts::new(), SharedRuns::new());
    app.mount(move |_| {
        List::new()
            .items(0..1000usize)
            .key(|index| *index)
            .try_batched_row(move |index| {
                Ok(BatchedCollectionRow {
                    element: Text::builder().arg(format!("row-{index}")).build(),
                    batches: row_batches.borrow().clone(),
                })
            })
            .virtualize()
            .build()
    })
    .unwrap();
    app.set_viewport(200.0, 100.0, 1.0);
    let before = Arc::strong_count(&first_text);
    assert!(app.build_frame().is_err());
    assert_eq!(Arc::strong_count(&first_text), before);
    batches.borrow_mut()[1] = Arc::new(SelectionSourceBatch::new(
        SelectionDocumentId(22),
        second_owner,
        2,
        Arc::from([source(2, Arc::clone(&second_text), false)]),
    ));
    app.build_frame().unwrap();
    assert!(Arc::strong_count(&first_text) > before);
    assert!(app.unregister_selection_source_in(SelectionDocumentId(21), SelectionSourceId(1)));
    let root = app.tree().root().unwrap();
    assert!(app.tree_mut().scroll_to(root, Point::new(0.0, 5000.0)));
    app.build_frame().unwrap();
    assert!(app.unregister_selection_source_in(SelectionDocumentId(21), SelectionSourceId(1)));
    app.release_selection_batch_owner(SelectionDocumentId(22), second_owner);
}
