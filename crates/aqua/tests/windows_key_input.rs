#![cfg(windows)]
use aqua::{ui::Text, UiApp};
use aqua_core::{InputEvent, KeyEvent, Modifiers};
use aqua_reactive::ReadSignal;
use aqua_shell_win32::input::{is_shortcut, named_key, resolve_key, Utf16Assembler};
use aqua_text::{SharedFonts, SharedRuns};
use std::{cell::Cell, rc::Rc};

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

const BACKSPACE: u16 = 0x08;
const TAB: u16 = 0x09;
const ENTER: u16 = 0x0D;
const ESCAPE: u16 = 0x1B;

fn press(
    app: &mut UiApp,
    vk: u16,
    character: Option<char>,
    payload: Option<&str>,
    modifiers: Modifiers,
) -> (KeyEvent, Option<String>) {
    let character = named_key(vk).map_or(character, |_| None);
    let key = KeyEvent {
        key: resolve_key(vk, character),
        modifiers,
        repeat: false,
    };
    let mut assembler = Utf16Assembler::default();
    let committed = payload
        .into_iter()
        .flat_map(str::encode_utf16)
        .filter_map(|unit| assembler.push(unit))
        .reduce(|mut all, next| {
            all.push_str(&next);
            all
        })
        .filter(|_| !is_shortcut(modifiers));
    app.handle_input(&InputEvent::KeyDown(key));
    if let Some(text) = &committed {
        app.handle_input(&InputEvent::TextInput { text: text.clone() });
    }
    app.sync().unwrap();
    (key, committed)
}

#[test]
fn windows_backspace_control_text_never_reinserts_a_box() {
    let (mut app, text) = field(false, Rc::new(Cell::new(0)));
    for expected in ["ab", "a", "", ""] {
        let (_, committed) = press(&mut app, BACKSPACE, None, Some("\u{8}"), Modifiers::NONE);
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
    for (vk, payload) in [(ENTER, "\r"), (TAB, "\t"), (ESCAPE, "\u{1b}")] {
        assert_eq!(
            press(&mut app, vk, None, Some(payload), Modifiers::NONE).1,
            None
        );
        assert_eq!(text.get(), "abc");
    }
    assert_eq!(submitted.get(), 1);
    let (mut multi, text) = field(true, Rc::new(Cell::new(0)));
    press(&mut multi, ENTER, None, Some("\r"), Modifiers::NONE);
    assert_eq!(text.get(), "abc\n");
    press(&mut multi, BACKSPACE, None, Some("\u{8}"), Modifiers::NONE);
    assert_eq!(text.get(), "abc");
}

#[test]
fn shortcuts_do_not_insert_text_and_altgr_unicode_are_preserved() {
    let (mut app, text) = field(false, Rc::new(Cell::new(0)));
    // Logical identity survives even when Windows reports a control payload.
    let (key, committed) = press(&mut app, 0x41, Some('a'), Some("\u{1}"), Modifiers::CONTROL);
    assert_eq!(key.key, aqua_core::Key::Char('a'));
    assert_eq!(committed, None);
    assert_eq!(
        text.get(),
        "abc",
        "Ctrl+A selects without replacing the document"
    );
    press(&mut app, BACKSPACE, None, Some("\u{8}"), Modifiers::NONE);
    assert_eq!(
        text.get(),
        "",
        "Ctrl+A followed by Backspace clears the selection"
    );
    press(
        &mut app,
        0x51,
        Some('@'),
        Some("@"),
        Modifiers::CONTROL.union(Modifiers::ALT),
    );
    let unicode = "e\u{301} 👨‍👩‍👧 □";
    press(&mut app, 0x45, Some('e'), Some(unicode), Modifiers::NONE);
    assert_eq!(text.get(), format!("@{unicode}"));
    press(&mut app, 0x43, Some('c'), Some("c"), Modifiers::CONTROL);
    assert_eq!(
        text.get(),
        format!("@{unicode}"),
        "shortcut text is suppressed even if printable"
    );
    press(&mut app, BACKSPACE, None, Some("\u{8}"), Modifiers::NONE);
    assert_eq!(
        text.get(),
        "@e\u{301} 👨‍👩‍👧 ",
        "a literal square remains valid user text and is deletable"
    );
}
