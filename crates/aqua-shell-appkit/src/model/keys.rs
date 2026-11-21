use aqua_core::{Key, Modifiers, NamedKey};

pub const KEY_SPACE: u16 = 0x31;

pub fn named_key(key_code: u16) -> Option<NamedKey> {
    match key_code {
        0x24 | 0x4C => Some(NamedKey::Enter),
        0x30 => Some(NamedKey::Tab),
        0x33 => Some(NamedKey::Backspace),
        0x75 => Some(NamedKey::Delete),
        0x72 => Some(NamedKey::Insert),
        0x6E => Some(NamedKey::ContextMenu),
        0x35 => Some(NamedKey::Escape),
        0x7B => Some(NamedKey::Left),
        0x7C => Some(NamedKey::Right),
        0x7E => Some(NamedKey::Up),
        0x7D => Some(NamedKey::Down),
        0x73 => Some(NamedKey::Home),
        0x77 => Some(NamedKey::End),
        0x74 => Some(NamedKey::PageUp),
        0x79 => Some(NamedKey::PageDown),
        other => function_key(other).map(NamedKey::Function),
    }
}

fn function_key(key_code: u16) -> Option<u8> {
    const FUNCTION_KEYS: [u16; 20] = [
        0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F, 0x69, 0x6B, 0x71,
        0x6A, 0x40, 0x4F, 0x50, 0x5A,
    ];
    FUNCTION_KEYS
        .iter()
        .position(|code| *code == key_code)
        .map(|index| index as u8 + 1)
}

pub fn resolve_key(key_code: u16, character: Option<char>) -> Key {
    match (named_key(key_code), character) {
        (Some(named), _) => Key::Named(named),
        (None, _) if key_code == KEY_SPACE => Key::Char(' '),
        (None, Some(c)) if !c.is_control() => Key::Char(c),
        (None, _) => Key::Named(NamedKey::Unidentified),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Equivalent {
    App,
    Menu,
}

pub fn key_equivalent(key: Key, modifiers: Modifiers) -> Option<Equivalent> {
    let command = modifiers.contains(Modifiers::LOGO);
    let option = modifiers.contains(Modifiers::ALT);
    let control = modifiers.contains(Modifiers::CONTROL);
    let shift = modifiers.contains(Modifiers::SHIFT);
    match (command, key) {
        (false, _) => None,
        (true, Key::Char('q' | 'h' | 'm')) if !control && !shift => Some(Equivalent::Menu),
        (true, Key::Char('`' | '~')) if !control && !option => Some(Equivalent::Menu),
        (true, _) => Some(Equivalent::App),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Interpretation {
    pub marked_before: bool,
    pub marked_after: bool,
    pub composed: bool,
    pub command: bool,
    pub delivered: bool,
    pub inserted: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    Consumed,
    Key,
    KeyAndText(String),
}

pub fn key_outcome(pass: &Interpretation) -> KeyOutcome {
    match (&pass.inserted, pass.command || pass.delivered) {
        (Some(text), _) => match aqua_shell::input::control_free(text) {
            typed if typed.is_empty() => KeyOutcome::Key,
            typed => KeyOutcome::KeyAndText(typed),
        },
        (None, true) => KeyOutcome::Key,
        (None, false) if pass.composed || pass.marked_before || pass.marked_after => {
            KeyOutcome::Consumed
        }
        (None, false) => KeyOutcome::Key,
    }
}
