use aqua_core::{Key, Modifiers, NamedKey};
use aqua_shell_appkit::model::keys::{
    key_equivalent, key_outcome, named_key, resolve_key, Equivalent, Interpretation, KeyOutcome,
};

#[test]
fn function_and_editing_keys_map_like_windows() {
    let expected = [
        (0x7A, NamedKey::Function(1)),
        (0x6F, NamedKey::Function(12)),
        (0x5A, NamedKey::Function(20)),
        (0x33, NamedKey::Backspace),
        (0x75, NamedKey::Delete),
        (0x4C, NamedKey::Enter),
        (0x73, NamedKey::Home),
        (0x79, NamedKey::PageDown),
    ];
    for (code, key) in expected {
        assert_eq!(named_key(code), Some(key), "key code {code:#x}");
    }
    assert_eq!(resolve_key(0x31, None), Key::Char(' '));
    assert_eq!(resolve_key(0x00, Some('a')), Key::Char('a'));
    assert_eq!(
        resolve_key(0x00, Some('\u{1}')),
        Key::Named(NamedKey::Unidentified)
    );
}

#[test]
fn command_shortcuts_go_to_the_app_except_the_ones_the_menu_owns() {
    let command = Modifiers::LOGO;
    let option = Modifiers::LOGO.union(Modifiers::ALT);
    assert_eq!(
        key_equivalent(Key::Char('c'), command),
        Some(Equivalent::App)
    );
    assert_eq!(
        key_equivalent(Key::Char('q'), command),
        Some(Equivalent::Menu)
    );
    assert_eq!(
        key_equivalent(Key::Char('h'), option),
        Some(Equivalent::Menu)
    );
    assert_eq!(
        key_equivalent(Key::Char('m'), command),
        Some(Equivalent::Menu)
    );
    assert_eq!(
        key_equivalent(Key::Char('`'), command),
        Some(Equivalent::Menu)
    );
    assert_eq!(
        key_equivalent(Key::Char('q'), command.union(Modifiers::SHIFT)),
        Some(Equivalent::App)
    );
    assert_eq!(key_equivalent(Key::Char('c'), Modifiers::CONTROL), None);
}

#[test]
fn a_dead_key_types_nothing_until_the_next_key_composes() {
    let accent = Interpretation {
        composed: true,
        marked_after: true,
        ..Interpretation::default()
    };
    assert_eq!(key_outcome(&accent), KeyOutcome::Consumed);
    let letter = Interpretation {
        marked_before: true,
        inserted: Some("é".into()),
        ..Interpretation::default()
    };
    assert_eq!(key_outcome(&letter), KeyOutcome::KeyAndText("é".into()));
}

#[test]
fn commands_and_control_text_reach_the_app_as_keys_only() {
    let enter = Interpretation {
        command: true,
        ..Interpretation::default()
    };
    assert_eq!(key_outcome(&enter), KeyOutcome::Key);
    let control = Interpretation {
        inserted: Some("\u{8}".into()),
        ..Interpretation::default()
    };
    assert_eq!(key_outcome(&control), KeyOutcome::Key);
}

#[test]
fn a_key_whose_text_the_input_method_took_does_not_also_reach_the_app() {
    let commit = Interpretation {
        marked_before: true,
        taken: true,
        ..Interpretation::default()
    };
    assert_eq!(key_outcome(&commit), KeyOutcome::Consumed);
    let passed_through = Interpretation {
        marked_before: true,
        taken: true,
        command: true,
        ..Interpretation::default()
    };
    assert_eq!(key_outcome(&passed_through), KeyOutcome::Key);
    let typed = Interpretation {
        inserted: Some("a".into()),
        ..Interpretation::default()
    };
    assert_eq!(key_outcome(&typed), KeyOutcome::KeyAndText("a".into()));
}
