//! Replay Windows key payloads through the real platform translator and UI host.
use aqua::{ui::Text, UiApp};
use aqua_core::{InputEvent, KeyEvent, Modifiers};
use aqua_platform::events::classify_logical_key;
use aqua_reactive::ReadSignal;
use aqua_text::{SharedFonts, SharedRuns};
use std::{cell::Cell, rc::Rc};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};

fn field(multiline: bool, submitted: Rc<Cell<usize>>) -> (UiApp, ReadSignal<String>) {
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    let mut source = None;
    app.mount(|scope| {
        let (read, write) = scope.signal("abc".to_owned());
        source = Some(read.clone());
        Text::builder()
            .bind((read, write))
            .multiline(multiline)
            .on_submit(move || submitted.set(submitted.get() + 1))
            .build()
    })
    .unwrap();
    let id = app.tree().root().unwrap();
    app.tree_mut().set_focused(Some(id));
    app.tree_mut().edit_text(id, |t| {
        if let aqua_ui::tree::TextBacking::Editable(editor) = &mut t.backing {
            editor.move_to_end(false);
        }
    });
    (app, source.unwrap())
}

fn press(
    app: &mut UiApp,
    physical: KeyCode,
    logical: Key,
    text: Option<&str>,
    modifiers: Modifiers,
) -> (KeyEvent, Option<String>) {
    let translated = classify_logical_key(
        PhysicalKey::Code(physical),
        &logical,
        text,
        modifiers,
        false,
    );
    app.handle_input(&InputEvent::KeyDown(translated.0));
    if let Some(text) = &translated.1 {
        app.handle_input(&InputEvent::TextInput { text: text.clone() });
    }
    app.sync().unwrap();
    translated
}

#[test]
fn windows_backspace_control_text_never_reinserts_a_box() {
    let (mut app, text) = field(false, Rc::new(Cell::new(0)));
    for expected in ["ab", "a", "", ""] {
        let (_, committed) = press(
            &mut app,
            KeyCode::Backspace,
            Key::Named(NamedKey::Backspace),
            Some("\u{8}"),
            Modifiers::NONE,
        );
        assert_eq!(committed, None);
        assert_eq!(
            text.get(),
            expected,
            "one native Backspace deletes once and never inserts U+0008"
        );
    }
    // A second adapter cannot smuggle these controls into the editor either.
    assert!(!app.handle_input(&InputEvent::TextInput {
        text: "\u{8}\0\u{7f}".into()
    }));
    assert_eq!(text.get(), "");
}

#[test]
fn control_keys_are_commands_and_multiline_enter_still_inserts_once() {
    let submitted = Rc::new(Cell::new(0));
    let (mut app, text) = field(false, submitted.clone());
    for (physical, logical, payload) in [
        (KeyCode::Enter, NamedKey::Enter, "\r"),
        (KeyCode::Tab, NamedKey::Tab, "\t"),
        (KeyCode::Escape, NamedKey::Escape, "\u{1b}"),
    ] {
        assert_eq!(
            press(
                &mut app,
                physical,
                Key::Named(logical),
                Some(payload),
                Modifiers::NONE
            )
            .1,
            None
        );
        assert_eq!(text.get(), "abc");
    }
    assert_eq!(submitted.get(), 1);
    let (mut multi, text) = field(true, Rc::new(Cell::new(0)));
    press(
        &mut multi,
        KeyCode::Enter,
        Key::Named(NamedKey::Enter),
        Some("\r"),
        Modifiers::NONE,
    );
    assert_eq!(text.get(), "abc\n");
    press(
        &mut multi,
        KeyCode::Backspace,
        Key::Named(NamedKey::Backspace),
        Some("\u{8}"),
        Modifiers::NONE,
    );
    assert_eq!(text.get(), "abc");
}

#[test]
fn shortcuts_do_not_insert_text_and_altgr_unicode_are_preserved() {
    let (mut app, text) = field(false, Rc::new(Cell::new(0)));
    // Logical identity survives even when Windows reports a control payload.
    let (key, committed) = press(
        &mut app,
        KeyCode::KeyA,
        Key::Character("a".into()),
        Some("\u{1}"),
        Modifiers::CONTROL,
    );
    assert_eq!(key.key, aqua_core::Key::Char('a'));
    assert_eq!(committed, None);
    assert_eq!(
        text.get(),
        "abc",
        "Ctrl+A selects without replacing the document"
    );
    press(
        &mut app,
        KeyCode::Backspace,
        Key::Named(NamedKey::Backspace),
        Some("\u{8}"),
        Modifiers::NONE,
    );
    assert_eq!(
        text.get(),
        "",
        "Ctrl+A followed by Backspace clears the selection"
    );
    press(
        &mut app,
        KeyCode::KeyQ,
        Key::Character("@".into()),
        Some("@"),
        Modifiers::CONTROL.union(Modifiers::ALT),
    );
    let unicode = "e\u{301} 👨‍👩‍👧 □";
    press(
        &mut app,
        KeyCode::KeyE,
        Key::Character("e".into()),
        Some(unicode),
        Modifiers::NONE,
    );
    assert_eq!(text.get(), format!("@{unicode}"));
    press(
        &mut app,
        KeyCode::KeyC,
        Key::Character("c".into()),
        Some("c"),
        Modifiers::CONTROL,
    );
    assert_eq!(
        text.get(),
        format!("@{unicode}"),
        "shortcut text is suppressed even if printable"
    );
    press(
        &mut app,
        KeyCode::Backspace,
        Key::Named(NamedKey::Backspace),
        Some("\u{8}"),
        Modifiers::NONE,
    );
    assert_eq!(
        text.get(),
        "@e\u{301} 👨‍👩‍👧 ",
        "a literal square remains valid user text and is deletable"
    );
}
