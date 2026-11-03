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

#[test]
fn native_clipboard_defaults_follow_routed_capture_and_do_not_duplicate_delivery() {
    use aqua::ui_app::dispatch_input_with_clipboard;
    use aqua_ui::input::{EventPhase, InputKind, InputResponse};
    use std::{cell::Cell, rc::Rc};
    let (mut app, text) = field();
    let node = app.tree().root().unwrap();
    let calls = Rc::new(Cell::new(0));
    let callback = calls.clone();
    app.tree_mut().listen_input(
        node,
        InputKind::KeyDown,
        EventPhase::Capture,
        Rc::new(move |_| {
            callback.set(callback.get() + 1);
            InputResponse::handled()
        }),
    );
    let paste = shortcut(Key::Char('v'), Modifiers::CONTROL);
    dispatch_input_with_clipboard(&mut app, &mut LockedClipboard, &paste).unwrap();
    assert_eq!(text.get(), "hello world");
    assert_eq!(calls.get(), 1);
    let callback = calls.clone();
    app.tree_mut().listen_input(
        node,
        InputKind::KeyDown,
        EventPhase::Capture,
        Rc::new(move |_| {
            callback.set(callback.get() + 1);
            InputResponse::default()
        }),
    );
    let mut clipboard = MemoryClipboard::new();
    clipboard.set_text("replacement".into()).unwrap();
    dispatch_input_with_clipboard(&mut app, &mut clipboard, &paste).unwrap();
    app.sync().unwrap();
    assert_eq!(text.get(), "replacement world");
    assert_eq!(calls.get(), 2);
    app.handle_input(&shortcut(Key::Char('z'), Modifiers::CONTROL));
    app.sync().unwrap();
    assert_eq!(text.get(), "hello world");
}

#[test]
fn composer_consumes_structured_image_and_file_paste_before_text_fallback() {
    use aqua::ui_app::dispatch_input_with_clipboard;
    use aqua_platform::services::ClipboardImage;
    use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
    use std::{cell::RefCell, rc::Rc};
    let (mut app, text) = field();
    let node = app.tree().root().unwrap();
    let attachments = Rc::new(RefCell::new(Vec::new()));
    let received = attachments.clone();
    app.tree_mut().listen_input(
        node,
        InputKind::ClipboardPaste,
        EventPhase::Bubble,
        Rc::new(move |event| {
            let RoutedInput::ClipboardPaste(content) = &event.input else {
                panic!()
            };
            if let Some(image) = &content.image {
                received.borrow_mut().push(format!(
                    "image:{}x{}:{}",
                    image.width(),
                    image.height(),
                    image.rgba().len()
                ));
            }
            for path in content.paths.iter() {
                received
                    .borrow_mut()
                    .push(path.to_string_lossy().into_owned());
            }
            if content.image.is_some() || !content.paths.is_empty() {
                InputResponse::handled()
            } else {
                InputResponse::default()
            }
        }),
    );
    let mut clipboard = MemoryClipboard::new();
    let paste = shortcut(Key::Char('v'), Modifiers::CONTROL);
    clipboard
        .set_image(ClipboardImage::new(2, 1, vec![255; 8]).unwrap())
        .unwrap();
    dispatch_input_with_clipboard(&mut app, &mut clipboard, &paste).unwrap();
    clipboard
        .set_files(vec!["report.txt".into(), "chart.png".into()])
        .unwrap();
    dispatch_input_with_clipboard(&mut app, &mut clipboard, &paste).unwrap();
    assert_eq!(
        *attachments.borrow(),
        vec!["image:2x1:8", "report.txt", "chart.png"]
    );
    assert_eq!(text.get(), "hello world");
    clipboard.set_text("plain".into()).unwrap();
    dispatch_input_with_clipboard(&mut app, &mut clipboard, &paste).unwrap();
    app.sync().unwrap();
    assert_eq!(text.get(), "plain world");
    app.handle_input(&shortcut(Key::Char('z'), Modifiers::CONTROL));
    app.sync().unwrap();
    assert_eq!(text.get(), "hello world");
}

#[test]
fn clipboard_format_failure_is_reported_before_any_partial_paste() {
    use aqua::ui_app::dispatch_input_with_clipboard;
    use aqua_platform::services::ClipboardImage;
    struct FailedImage;
    impl Clipboard for FailedImage {
        fn get_text(&mut self) -> Result<Option<String>, ClipboardError> {
            Ok(Some("available text".into()))
        }
        fn set_text(&mut self, _: String) -> Result<(), ClipboardError> {
            Ok(())
        }
        fn clear(&mut self) -> Result<(), ClipboardError> {
            Ok(())
        }
        fn get_image(&mut self) -> Result<Option<ClipboardImage>, ClipboardError> {
            Err(ClipboardError::Os("image read failed".into()))
        }
    }
    let (mut app, text) = field();
    let result = dispatch_input_with_clipboard(
        &mut app,
        &mut FailedImage,
        &shortcut(Key::Char('v'), Modifiers::CONTROL),
    );
    assert_eq!(
        result.unwrap_err(),
        ClipboardError::Os("image read failed".into())
    );
    assert_eq!(text.get(), "hello world");
}
