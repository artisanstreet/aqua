use aqua_core::{Modifiers, PointerButton, ScrollDelta};

pub const WHEEL_NOTCH: f32 = 120.0;
pub const NOTCH_UNITS: f32 = 16.0;

pub fn modifiers(shift: bool, control: bool, alt: bool, logo: bool) -> Modifiers {
    [
        (shift, Modifiers::SHIFT),
        (control, Modifiers::CONTROL),
        (alt, Modifiers::ALT),
        (logo, Modifiers::LOGO),
    ]
    .into_iter()
    .filter(|(held, _)| *held)
    .fold(Modifiers::NONE, |acc, (_, flag)| acc.union(flag))
}

pub fn is_shortcut(modifiers: Modifiers) -> bool {
    modifiers.contains(Modifiers::LOGO)
        || (modifiers.contains(Modifiers::CONTROL) && !modifiers.contains(Modifiers::ALT))
}

pub fn button(which: PointerButtonSource) -> (PointerButton, u8) {
    match which {
        PointerButtonSource::Left => (PointerButton::Primary, 0x01),
        PointerButtonSource::Right => (PointerButton::Secondary, 0x02),
        PointerButtonSource::Middle => (PointerButton::Middle, 0x04),
        PointerButtonSource::X(1) => (PointerButton::Extra(3), 0x08),
        PointerButtonSource::X(2) => (PointerButton::Extra(4), 0x10),
        PointerButtonSource::X(other) => (PointerButton::Extra(other.min(250) as u8 + 3), 0x20),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButtonSource {
    Left,
    Right,
    Middle,
    X(u16),
}

pub fn scroll(vertical: bool, raw_delta: i16) -> ScrollDelta {
    let units = f32::from(raw_delta) / WHEEL_NOTCH * NOTCH_UNITS;
    match vertical {
        true => ScrollDelta { x: 0.0, y: -units },
        false => ScrollDelta { x: units, y: 0.0 },
    }
}

pub fn control_free(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}
