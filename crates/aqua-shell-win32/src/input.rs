use aqua_core::{Key, Modifiers, NamedKey};

pub use aqua_shell::input::{
    button, control_free, is_shortcut, modifiers, scroll, PointerButtonSource, NOTCH_UNITS,
    WHEEL_NOTCH,
};

pub fn named_key(vk: u16) -> Option<NamedKey> {
    match vk {
        0x1B => Some(NamedKey::Escape),
        0x0D => Some(NamedKey::Enter),
        0x09 => Some(NamedKey::Tab),
        0x08 => Some(NamedKey::Backspace),
        0x2E => Some(NamedKey::Delete),
        0x2D => Some(NamedKey::Insert),
        0x5D => Some(NamedKey::ContextMenu),
        0x25 => Some(NamedKey::Left),
        0x27 => Some(NamedKey::Right),
        0x26 => Some(NamedKey::Up),
        0x28 => Some(NamedKey::Down),
        0x24 => Some(NamedKey::Home),
        0x23 => Some(NamedKey::End),
        0x21 => Some(NamedKey::PageUp),
        0x22 => Some(NamedKey::PageDown),
        0x70..=0x87 => Some(NamedKey::Function((vk - 0x6F) as u8)),
        _ => None,
    }
}

pub fn is_modifier_key(vk: u16) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

pub fn resolve_key(vk: u16, character: Option<char>) -> Key {
    match (named_key(vk), character) {
        (Some(named), _) => Key::Named(named),
        (None, Some(c)) if !c.is_control() => Key::Char(c),
        (None, _) if vk == 0x20 => Key::Char(' '),
        (None, _) => Key::Named(NamedKey::Unidentified),
    }
}

pub const WM_CUT: u32 = 0x0300;
pub const WM_COPY: u32 = 0x0301;
pub const WM_PASTE: u32 = 0x0302;

pub fn clipboard_shortcut(message: u32) -> Option<aqua_core::KeyEvent> {
    let letter = match message {
        WM_CUT => 'x',
        WM_COPY => 'c',
        WM_PASTE => 'v',
        _ => return None,
    };
    Some(aqua_core::KeyEvent {
        key: Key::Char(letter),
        modifiers: Modifiers::CONTROL,
        repeat: false,
    })
}

pub fn wheel_delta(wparam: usize) -> i16 {
    ((wparam >> 16) & 0xFFFF) as u16 as i16
}

pub fn point_from_lparam(lparam: isize) -> (i32, i32) {
    (
        i32::from((lparam & 0xFFFF) as u16 as i16),
        i32::from(((lparam >> 16) & 0xFFFF) as u16 as i16),
    )
}

pub fn is_repeat(lparam: isize) -> bool {
    lparam & (1 << 30) != 0
}

#[derive(Debug, Default)]
pub struct Utf16Assembler {
    pending_high: Option<u16>,
}

impl Utf16Assembler {
    pub fn push(&mut self, unit: u16) -> Option<String> {
        match (self.pending_high.take(), unit) {
            (_, 0xD800..=0xDBFF) => {
                self.pending_high = Some(unit);
                None
            }
            (Some(high), 0xDC00..=0xDFFF) => String::from_utf16(&[high, unit])
                .ok()
                .map(|s| control_free(&s)),
            (_, 0xD800..=0xDFFF) => None,
            (_, unit) => String::from_utf16(&[unit]).ok().map(|s| control_free(&s)),
        }
        .filter(|s| !s.is_empty())
    }
}
