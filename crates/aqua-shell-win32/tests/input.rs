use aqua_core::{Key, Modifiers, NamedKey, PointerButton};
use aqua_shell_win32::input::*;

#[test]
fn wheel_notches_become_signed_logical_units() {
    assert_eq!(scroll(true, 120).y, -16.0);
    assert_eq!(scroll(true, -240).y, 32.0);
    assert_eq!(scroll(false, 120).x, 16.0);
    assert_eq!(scroll(true, 60).y, -8.0);
    assert_eq!(wheel_delta((0xFF88usize) << 16), -120);
}

#[test]
fn lparam_points_are_signed_per_word() {
    assert_eq!(point_from_lparam(0x0010_0020), (0x20, 0x10));
    assert_eq!(
        point_from_lparam(((-5i16 as u16 as isize) << 16) | (-7i16 as u16 as isize)),
        (-7, -5)
    );
}

#[test]
fn named_keys_take_priority_over_characters() {
    assert_eq!(resolve_key(0x25, Some('%')), Key::Named(NamedKey::Left));
    assert_eq!(resolve_key(0x70, None), Key::Named(NamedKey::Function(1)));
    assert_eq!(resolve_key(0x87, None), Key::Named(NamedKey::Function(24)));
    assert_eq!(resolve_key(0x41, Some('a')), Key::Char('a'));
    assert_eq!(resolve_key(0x20, None), Key::Char(' '));
    assert_eq!(resolve_key(0xE7, None), Key::Named(NamedKey::Unidentified));
    assert_eq!(
        resolve_key(0x41, Some('\u{1}')),
        Key::Named(NamedKey::Unidentified)
    );
}

#[test]
fn modifier_chords_classify_as_shortcuts_except_altgr() {
    let ctrl = modifiers(false, true, false, false);
    assert!(is_shortcut(ctrl));
    assert!(!is_shortcut(modifiers(true, false, false, false)));
    assert!(!is_shortcut(modifiers(false, true, true, false)));
    assert!(is_shortcut(modifiers(false, false, false, true)));
    assert_eq!(
        modifiers(true, true, true, true).contains(Modifiers::LOGO),
        true
    );
    assert_eq!(modifiers(false, false, false, false), Modifiers::NONE);
}

#[test]
fn extra_buttons_map_to_distinct_bits() {
    assert_eq!(
        button(PointerButtonSource::Left),
        (PointerButton::Primary, 1)
    );
    assert_eq!(
        button(PointerButtonSource::Right),
        (PointerButton::Secondary, 2)
    );
    assert_eq!(
        button(PointerButtonSource::Middle),
        (PointerButton::Middle, 4)
    );
    assert_eq!(
        button(PointerButtonSource::X(1)),
        (PointerButton::Extra(3), 8)
    );
    assert_eq!(
        button(PointerButtonSource::X(2)),
        (PointerButton::Extra(4), 16)
    );
}

#[test]
fn utf16_assembler_joins_surrogates_and_drops_controls() {
    let mut assembler = Utf16Assembler::default();
    assert_eq!(assembler.push(0xD83D), None);
    assert_eq!(assembler.push(0xDE00), Some("\u{1F600}".into()));
    assert_eq!(assembler.push(0x0008), None);
    assert_eq!(assembler.push(0x000D), None);
    assert_eq!(assembler.push(u16::from(b'x')), Some("x".into()));
    assert_eq!(assembler.push(0xDC00), None);
    assert_eq!(assembler.push(0xD83D), None);
    assert_eq!(assembler.push(u16::from(b'y')), Some("y".into()));
}

#[test]
fn modifier_keys_are_recognized_by_virtual_key() {
    assert!(is_modifier_key(0x10));
    assert!(is_modifier_key(0xA3));
    assert!(is_modifier_key(0x5B));
    assert!(!is_modifier_key(0x41));
}
