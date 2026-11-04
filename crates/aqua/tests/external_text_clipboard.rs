use std::cell::Cell;
use std::rc::Rc;

use aqua::ui_app::dispatch_clipboard;
use aqua_core::{InputEvent, Key, KeyEvent, Modifiers};
use aqua_shell_win32::services::{Clipboard, ClipboardError, MemoryClipboard};
use aqua_text::{Selection, SharedFonts, SharedRuns};
use aqua_ui::element::Text;
use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput, TextEdit};
use aqua_ui::tree::UiCommand;

struct FailingClipboard;

impl Clipboard for FailingClipboard {
    fn get_text(&mut self) -> Result<Option<String>, ClipboardError> {
        Ok(None)
    }
    fn set_text(&mut self, _: String) -> Result<(), ClipboardError> {
        Err(ClipboardError::Os("failed".into()))
    }
    fn clear(&mut self) -> Result<(), ClipboardError> {
        Ok(())
    }
}

#[test]
fn external_cut_occurs_only_after_clipboard_write_succeeds() {
    let cuts = Rc::new(Cell::new(0));
    let observed = cuts.clone();
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        Text::new("hello")
            .editable(true)
            .external_text_input(true)
            .initial_external_selection(Selection::new(0, 5), None)
            .build()
            .focusable(true)
            .on_input(InputKind::ClipboardCut, EventPhase::Capture, move |event| {
                assert!(matches!(&event.input, RoutedInput::ClipboardCut));
                observed.set(observed.get() + 1);
                InputResponse {
                    commands: vec![UiCommand::EditText {
                        node: event.current_target,
                        revision: None,
                        edit: TextEdit::SetExternalProjection {
                            text: String::new(),
                            selection: Selection::caret_at(0),
                            marked: None,
                        },
                    }],
                    ..InputResponse::handled()
                }
            })
    })
    .unwrap();
    app.set_viewport(200.0, 80.0, 1.0);
    app.build_frame().unwrap();
    let node = app.tree().root().unwrap();
    app.tree_mut().set_focused(Some(node));
    let cut = InputEvent::KeyDown(KeyEvent {
        key: Key::Char('x'),
        modifiers: Modifiers::CONTROL,
        repeat: false,
    });
    assert!(dispatch_clipboard(&mut app, &mut FailingClipboard, &cut).is_err());
    assert_eq!(cuts.get(), 0);
    assert_eq!(app.selected_text().as_deref(), Some("hello"));
    let mut clipboard = MemoryClipboard::new();
    dispatch_clipboard(&mut app, &mut clipboard, &cut).unwrap();
    assert_eq!(clipboard.get_text().unwrap().as_deref(), Some("hello"));
    assert_eq!(cuts.get(), 1);
    assert_eq!(app.tree().editor_snapshot(node).unwrap().text, "");
}
