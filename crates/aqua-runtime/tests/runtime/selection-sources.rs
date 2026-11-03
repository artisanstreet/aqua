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
