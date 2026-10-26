//! Platform-neutral input events. The platform adapter translates native
//! events (winit key codes, IME, pointer) into these; UI and apps only ever
//! see this vocabulary. No winit/wgpu types here.

use crate::geometry::Point;

/// Which pointer device produced the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PointerKind {
    /// Mouse.
    Mouse,
    /// Touch contact.
    Touch,
    /// Stylus/pen.
    Pen,
}

/// Mouse buttons plus touch-implied press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PointerButton {
    /// Primary button / single touch contact.
    Primary,
    /// Secondary (right) button.
    Secondary,
    /// Middle button.
    Middle,
    /// Extended button (index 3..).
    Extra(u8),
}

/// Modifier keys held during an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    /// Bit 0 shift, 1 control, 2 alt, 3 logo/super.
    bits: u8,
}

impl Modifiers {
    /// No modifiers held.
    pub const NONE: Modifiers = Modifiers { bits: 0 };
    /// Shift flag.
    pub const SHIFT: Modifiers = Modifiers { bits: 1 };
    /// Control flag.
    pub const CONTROL: Modifiers = Modifiers { bits: 2 };
    /// Alt/option flag.
    pub const ALT: Modifiers = Modifiers { bits: 4 };
    /// Logo/super/command flag.
    pub const LOGO: Modifiers = Modifiers { bits: 8 };

    /// Whether any modifier is held.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.bits == 0
    }

    /// Whether `flag` is held.
    #[must_use]
    pub fn contains(self, flag: Modifiers) -> bool {
        self.bits & flag.bits == flag.bits
    }

    /// Combine flags.
    #[must_use]
    pub fn union(self, other: Modifiers) -> Modifiers {
        Modifiers {
            bits: self.bits | other.bits,
        }
    }
}

/// Physical pointer contact update.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerEvent {
    /// Stable contact identifier (multi-touch); 0 for mouse.
    pub contact: u64,
    /// Device kind.
    pub kind: PointerKind,
    /// Position in window-local logical units.
    pub position: Point,
    /// Buttons held after this event applies.
    pub buttons: u8,
}

/// Scroll input in logical units (positive y = scroll down).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScrollDelta {
    /// Horizontal scroll.
    pub x: f32,
    /// Vertical scroll.
    pub y: f32,
}

/// Logical key identity. Text-producing keys ALSO surface through
/// [`InputEvent::TextInput`]; this enum is for shortcuts, navigation, and
/// non-printable keys — NOT the text entry path. `Char` holds one scalar for
/// identity (e.g. shortcut matching), never a full user input string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// Single-scalar printable identity (already resolved through the
    /// platform layout, e.g. `a`, `ß`). Multi-scalar input (emoji sequences,
    /// combining marks) arrives via `TextInput`, not here.
    Char(char),
    /// Named non-printable key.
    Named(NamedKey),
}

/// Non-printable keys relevant to UI behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedKey {
    /// Enter/return.
    Enter,
    /// Tab.
    Tab,
    /// Backspace.
    Backspace,
    /// Delete (forward).
    Delete,
    /// Escape.
    Escape,
    /// Arrow keys.
    Left,
    /// Arrow keys.
    Right,
    /// Arrow keys.
    Up,
    /// Arrow keys.
    Down,
    /// Home.
    Home,
    /// End.
    End,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Function key (1..=24).
    Function(u8),
    /// Any other key the adapter does not map; carries the platform key name
    /// for diagnostics (never matched for behavior).
    Unidentified,
}

/// Keyboard press/release.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyEvent {
    /// Which key.
    pub key: Key,
    /// Modifiers held.
    pub modifiers: Modifiers,
    /// Whether this is an auto-repeat.
    pub repeat: bool,
}

/// IME composition lifecycle, translated by the platform adapter. Editing
/// operations themselves go through the text model; these events only carry
/// composition state and candidate geometry requests.
#[derive(Debug, Clone, PartialEq)]
pub enum ImeEvent {
    /// Composition started.
    Start,
    /// Composition updated (marked text, not yet committed).
    Update {
        /// Current marked text.
        marked: String,
        /// Candidate cursor/selection as BYTE offsets into `marked`
        /// (from the native preedit event; `None` when the platform does not
        /// report it). `marked` alone cannot place the candidate caret.
        cursor: Option<(usize, usize)>,
    },
    /// Composition committed into the text model.
    Commit {
        /// Committed text.
        text: String,
    },
    /// Composition cancelled.
    Cancel,
}

/// Complete input event vocabulary for UI hit-testing and behaviors.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    /// Pointer moved (hover or drag, distinguished by `buttons`).
    PointerMove(PointerEvent),
    /// Button pressed.
    PointerDown(PointerEvent, PointerButton),
    /// Button released.
    PointerUp(PointerEvent, PointerButton),
    /// Contact cancelled (capture lost, alert overlay, node removed).
    /// Drags MUST terminate safely on this event.
    PointerCancel {
        /// Cancelled contact.
        contact: u64,
    },
    /// Scroll wheel / gesture.
    Scroll {
        /// Position in window-local logical units.
        at: Point,
        /// Scroll amount.
        delta: ScrollDelta,
    },
    /// Key pressed.
    KeyDown(KeyEvent),
    /// Key released.
    KeyUp(KeyEvent),
    /// Committed text from the platform (winit `KeyEvent.text`), routed
    /// exactly once per platform key event and SUPPRESSED while IME is
    /// active (IME text travels via `ImeEvent::Commit` instead). Carries the
    /// full multi-scalar string — `Key::Char` is identity only and cannot
    /// hold emoji sequences or combining marks.
    TextInput {
        /// Committed text.
        text: String,
    },
    /// IME lifecycle.
    Ime(ImeEvent),
    /// Window focus gained/lost (drives focus scopes, caret visibility).
    Focused(bool),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_composition() {
        let m = Modifiers::SHIFT.union(Modifiers::CONTROL);
        assert!(m.contains(Modifiers::SHIFT));
        assert!(m.contains(Modifiers::CONTROL));
        assert!(!m.contains(Modifiers::ALT));
        assert!(!Modifiers::NONE.contains(Modifiers::SHIFT));
    }

    #[test]
    fn cancel_carries_contact_only() {
        let e = InputEvent::PointerCancel { contact: 7 };
        assert!(matches!(e, InputEvent::PointerCancel { contact: 7 }));
    }
}
