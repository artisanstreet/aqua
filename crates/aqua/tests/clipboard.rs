use aqua::{
    ui::Text,
    ui_app::{dispatch_clipboard, ClipboardOutcome},
    UiApp,
};
use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey};
use aqua_platform::services::{Clipboard, ClipboardError, MemoryClipboard};
use aqua_reactive::ReadSignal;
use aqua_text::{Selection, SharedFonts, SharedRuns};

fn field() -> (UiApp, ReadSignal<String>) {
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    let (read, write) = app.scope().signal("hello world".to_owned());
    app.mount(|_| {
        Text::builder()
            .bind((read.clone(), write))
            .multiline(true)
            .build()
    })
    .unwrap();
    let node = app.tree().root().unwrap();
    app.tree_mut().set_focused(Some(node));
    app.tree_mut()
        .set_text_selection(node, Some(Selection::new(0, 5)));
    (app, read)
}

fn shortcut(key: Key, modifiers: Modifiers) -> InputEvent {
    InputEvent::KeyDown(KeyEvent {
        key,
        modifiers,
        repeat: false,
    })
}

#[test]
fn clipboard_shortcuts_copy_cut_paste_and_preserve_undo_boundaries() {
    let (mut app, text) = field();
    let mut clipboard = MemoryClipboard::new();
    let copy = shortcut(Key::Named(NamedKey::Insert), Modifiers::CONTROL);
    assert_eq!(
        dispatch_clipboard(&mut app, &mut clipboard, &copy).unwrap(),
        ClipboardOutcome::Handled { redraw: false }
    );
    assert_eq!(clipboard.get_text().unwrap().as_deref(), Some("hello"));
    let cut = shortcut(Key::Named(NamedKey::Delete), Modifiers::SHIFT);
    assert_eq!(
        dispatch_clipboard(&mut app, &mut clipboard, &cut).unwrap(),
        ClipboardOutcome::Handled { redraw: true }
    );
    app.sync().unwrap();
    assert_eq!(text.get(), " world");
    clipboard.set_text("replacement\r\nline".into()).unwrap();
    let paste = shortcut(Key::Named(NamedKey::Insert), Modifiers::SHIFT);
    assert_eq!(
        dispatch_clipboard(&mut app, &mut clipboard, &paste).unwrap(),
        ClipboardOutcome::Handled { redraw: true }
    );
    app.sync().unwrap();
    assert_eq!(text.get(), "replacement\nline world");
    app.handle_input(&shortcut(Key::Char('z'), Modifiers::CONTROL));
    app.sync().unwrap();
    assert_eq!(text.get(), " world");
    app.handle_input(&shortcut(Key::Char('z'), Modifiers::CONTROL));
    app.sync().unwrap();
    assert_eq!(text.get(), "hello world");
}

struct LockedClipboard;

impl Clipboard for LockedClipboard {
    fn get_text(&mut self) -> Result<Option<String>, ClipboardError> {
        Err(ClipboardError::Unavailable("locked"))
    }
    fn set_text(&mut self, _: String) -> Result<(), ClipboardError> {
        Err(ClipboardError::Unavailable("locked"))
    }
    fn clear(&mut self) -> Result<(), ClipboardError> {
        Ok(())
    }
}

#[test]
fn failed_cut_preserves_document_and_selection() {
    let (mut app, text) = field();
    let cut = shortcut(Key::Char('x'), Modifiers::CONTROL);
    assert!(dispatch_clipboard(&mut app, &mut LockedClipboard, &cut).is_err());
    app.sync().unwrap();
    assert_eq!(text.get(), "hello world");
    assert_eq!(app.selected_text().as_deref(), Some("hello"));
}

#[test]
fn altgr_shortcuts_are_left_for_text_input_and_disabled_fields_reject_paste() {
    let (mut app, text) = field();
    let mut clipboard = MemoryClipboard::new();
    clipboard.set_text("replacement".into()).unwrap();
    let altgr = shortcut(Key::Char('v'), Modifiers::CONTROL.union(Modifiers::ALT));
    assert_eq!(
        dispatch_clipboard(&mut app, &mut clipboard, &altgr).unwrap(),
        ClipboardOutcome::Unhandled
    );
    let node = app.tree().root().unwrap();
    app.tree_mut().get_mut(node).unwrap().semantics.disabled = true;
    let paste = shortcut(Key::Char('v'), Modifiers::CONTROL);
    assert_eq!(
        dispatch_clipboard(&mut app, &mut clipboard, &paste).unwrap(),
        ClipboardOutcome::Handled { redraw: false }
    );
    app.sync().unwrap();
    assert_eq!(text.get(), "hello world");
}
