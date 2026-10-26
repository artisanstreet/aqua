//! Core-neutral input events + pure `winit` translation helpers.
//!
//! The facade calls the `translate_*` functions at the [`crate::host`]
//! boundary; everything here is headless-testable (no windows needed).

use crate::geometry::{LogicalPosition, LogicalSize};
use crate::scheduler::FrameReason;
use crate::window::WindowEvent;

/// Pressed / released, independent of `winit` types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressState {
    Pressed,
    Released,
}

/// Pointer buttons we route. Unknown OS buttons map to `Other(u8)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Middle,
    Right,
    Back,
    Forward,
    Other(u16),
}

/// Framework key codes: a portable subset. Anything else is `Unidentified`.
/// Full IME/compose handling goes through [`AquaImeEvent`], not this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AquaKeyCode {
    Escape,
    Enter,
    Tab,
    Space,
    Backspace,
    Delete,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Home,
    End,
    PageUp,
    PageDown,
    KeyA,
    KeyC,
    KeyV,
    KeyX,
    KeyZ,
    KeyY,
    F(u8),
    Unidentified,
}

/// IME composition, translated from `winit::event::Ime`.
#[derive(Debug, Clone, PartialEq)]
pub enum AquaImeEvent {
    Enabled,
    Disabled,
    Preedit(String, Option<(usize, usize)>),
    Commit(String),
}

/// Pointer / keyboard / IME input in framework coordinates (DIPs).
#[derive(Debug, Clone, PartialEq)]
pub enum AquaInputEvent {
    PointerMoved {
        position: LogicalPosition,
    },
    PointerButton {
        state: PressState,
        button: PointerButton,
        position: LogicalPosition,
    },
    PointerEntered,
    PointerLeft,
    Scroll {
        delta_lines: (f32, f32),
        delta_pixels: (f32, f32),
    },
    Key {
        code: AquaKeyCode,
        state: PressState,
        repeat: bool,
        text: Option<String>,
    },
    Ime(AquaImeEvent),
}

// ---------------------------------------------------------------------------
// Pure winit translations (constructible headless — no display required).
// ---------------------------------------------------------------------------

pub fn translate_element_state(s: winit::event::ElementState) -> PressState {
    match s {
        winit::event::ElementState::Pressed => PressState::Pressed,
        winit::event::ElementState::Released => PressState::Released,
    }
}

pub fn translate_mouse_button(b: winit::event::MouseButton) -> PointerButton {
    use winit::event::MouseButton as W;
    match b {
        W::Left => PointerButton::Left,
        W::Middle => PointerButton::Middle,
        W::Right => PointerButton::Right,
        W::Back => PointerButton::Back,
        W::Forward => PointerButton::Forward,
        W::Other(n) => PointerButton::Other(n),
    }
}

pub fn translate_physical_key(k: winit::keyboard::PhysicalKey) -> AquaKeyCode {
    use winit::keyboard::{KeyCode as K, PhysicalKey as P};
    match k {
        P::Code(code) => match code {
            K::Escape => AquaKeyCode::Escape,
            K::Enter | K::NumpadEnter => AquaKeyCode::Enter,
            K::Tab => AquaKeyCode::Tab,
            K::Space => AquaKeyCode::Space,
            K::Backspace => AquaKeyCode::Backspace,
            K::Delete => AquaKeyCode::Delete,
            K::ArrowLeft => AquaKeyCode::ArrowLeft,
            K::ArrowRight => AquaKeyCode::ArrowRight,
            K::ArrowUp => AquaKeyCode::ArrowUp,
            K::ArrowDown => AquaKeyCode::ArrowDown,
            K::Home => AquaKeyCode::Home,
            K::End => AquaKeyCode::End,
            K::PageUp => AquaKeyCode::PageUp,
            K::PageDown => AquaKeyCode::PageDown,
            K::KeyA => AquaKeyCode::KeyA,
            K::KeyC => AquaKeyCode::KeyC,
            K::KeyV => AquaKeyCode::KeyV,
            K::KeyX => AquaKeyCode::KeyX,
            K::KeyZ => AquaKeyCode::KeyZ,
            K::KeyY => AquaKeyCode::KeyY,
            K::F1 => AquaKeyCode::F(1),
            K::F2 => AquaKeyCode::F(2),
            K::F3 => AquaKeyCode::F(3),
            K::F4 => AquaKeyCode::F(4),
            K::F5 => AquaKeyCode::F(5),
            K::F6 => AquaKeyCode::F(6),
            K::F7 => AquaKeyCode::F(7),
            K::F8 => AquaKeyCode::F(8),
            K::F9 => AquaKeyCode::F(9),
            K::F10 => AquaKeyCode::F(10),
            K::F11 => AquaKeyCode::F(11),
            K::F12 => AquaKeyCode::F(12),
            _ => AquaKeyCode::Unidentified,
        },
        P::Unidentified(_) => AquaKeyCode::Unidentified,
    }
}

pub fn translate_ime(ime: &winit::event::Ime) -> AquaImeEvent {
    match ime {
        winit::event::Ime::Enabled => AquaImeEvent::Enabled,
        winit::event::Ime::Disabled => AquaImeEvent::Disabled,
        winit::event::Ime::Preedit(text, cursor) => AquaImeEvent::Preedit(text.clone(), *cursor),
        winit::event::Ime::Commit(text) => AquaImeEvent::Commit(text.clone()),
    }
}

/// Physical pixels + scale -> logical DIPs (single choke point for the
/// coordinate policy; layout always sees logical units).
pub fn to_logical(x_phys: f64, y_phys: f64, scale: f64) -> LogicalPosition {
    let s = if scale > 0.0 { scale } else { 1.0 };
    LogicalPosition {
        x: x_phys / s,
        y: y_phys / s,
    }
}

pub fn physical_size_to_logical(size: winit::dpi::PhysicalSize<u32>, scale: f64) -> LogicalSize {
    let s = if scale > 0.0 { scale } else { 1.0 };
    LogicalSize::new(f64::from(size.width) / s, f64::from(size.height) / s)
}

/// Which framework [`WindowEvent`]s must schedule work, and why.
/// Pure over our own (already translated) events so it stays stable while
/// `winit` evolves.
pub fn window_event_frame_reason(ev: &WindowEvent) -> Option<FrameReason> {
    match ev {
        WindowEvent::RedrawRequested(_) => Some(FrameReason::EXPLICIT),
        WindowEvent::Resized(_, _) => Some(FrameReason::RESIZE),
        WindowEvent::ScaleChanged(_, _) => Some(FrameReason::RESIZE),
        WindowEvent::Focused(_, _) => Some(FrameReason::INPUT),
        // Theme/material/focus changes can alter the native frame and the
        // fallback contrast posture; re-evaluate without claiming damage.
        WindowEvent::ThemeChanged(_, _) => Some(FrameReason::MATERIAL),
        WindowEvent::Occluded(_, occluded) => {
            if *occluded {
                None
            } else {
                Some(FrameReason::EXPOSE)
            }
        }
        WindowEvent::CloseRequested(_) | WindowEvent::Destroyed(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Canonical translators: winit -> `aqua_core::event` directly.
// New code (facade included) should use these; the provisional enums above
// stay for the existing facade call sites until the facade migrates.
// ---------------------------------------------------------------------------

use crate::window::ThemePreference;

/// `winit` modifier state -> canonical [`aqua_core::Modifiers`].
/// Pure over `Modifiers::state()`; per-side key identity is intentionally
/// dropped (Wayland/X11 hide it; see winit docs).
pub fn translate_modifiers(m: &winit::event::Modifiers) -> aqua_core::Modifiers {
    modifiers_from_state(m.state())
}

/// Flag mapping shared by [`translate_modifiers`]; public so headless tests
/// (which cannot construct `winit::event::Modifiers` with state) can still
/// cover every flag combination.
pub fn modifiers_from_state(s: winit::keyboard::ModifiersState) -> aqua_core::Modifiers {
    let mut out = aqua_core::Modifiers::NONE;
    if s.shift_key() {
        out = out.union(aqua_core::Modifiers::SHIFT);
    }
    if s.control_key() {
        out = out.union(aqua_core::Modifiers::CONTROL);
    }
    if s.alt_key() {
        out = out.union(aqua_core::Modifiers::ALT);
    }
    if s.super_key() {
        out = out.union(aqua_core::Modifiers::LOGO);
    }
    out
}

/// `winit` mouse button -> canonical [`aqua_core::PointerButton`].
/// Matches the facade's historical mapping (Back -> Extra(3), Forward ->
/// Extra(4)) so behavior is unchanged when the facade switches over.
pub fn translate_mouse_button_core(b: winit::event::MouseButton) -> aqua_core::PointerButton {
    use aqua_core::PointerButton as C;
    match translate_mouse_button(b) {
        PointerButton::Left => C::Primary,
        PointerButton::Right => C::Secondary,
        PointerButton::Middle => C::Middle,
        PointerButton::Back => C::Extra(3),
        PointerButton::Forward => C::Extra(4),
        PointerButton::Other(n) => C::Extra(n.min(u16::from(u8::MAX)) as u8),
    }
}

fn aqua_code_to_named(code: AquaKeyCode) -> aqua_core::NamedKey {
    use aqua_core::NamedKey as N;
    match code {
        AquaKeyCode::Escape => N::Escape,
        AquaKeyCode::Enter => N::Enter,
        AquaKeyCode::Tab => N::Tab,
        AquaKeyCode::Backspace => N::Backspace,
        AquaKeyCode::Delete => N::Delete,
        AquaKeyCode::ArrowLeft => N::Left,
        AquaKeyCode::ArrowRight => N::Right,
        AquaKeyCode::ArrowUp => N::Up,
        AquaKeyCode::ArrowDown => N::Down,
        AquaKeyCode::Home => N::Home,
        AquaKeyCode::End => N::End,
        AquaKeyCode::PageUp => N::PageUp,
        AquaKeyCode::PageDown => N::PageDown,
        AquaKeyCode::F(n) => N::Function(n),
        _ => N::Unidentified,
    }
}

/// Full `winit` key occurrence -> canonical [`aqua_core::KeyEvent`] plus the
/// platform-resolved committed text (if any).
///
/// Mapping policy (matches the core contract):
/// - Layout-resolved single non-control scalar -> `Key::Char` (identity for
///   shortcuts; the full string travels separately as committed text).
/// - Otherwise the named physical key; unmapped -> `Unidentified` (kept for
///   diagnostics, never matched for behavior).
/// - Only printable committed text is returned. Windows also supplies
///   control text for Backspace, Enter, Tab, and Escape; those are commands,
///   not document characters. Ctrl/Command shortcuts produce no text (AltGr
///   remains text-producing). The caller also suppresses commits during IME.
pub fn translate_key_event(
    event: &winit::event::KeyEvent,
    modifiers: aqua_core::Modifiers,
) -> (aqua_core::KeyEvent, Option<String>) {
    classify_logical_key(
        event.physical_key,
        &event.logical_key,
        event.text.as_ref().map(|s| s.as_str()),
        modifiers,
        event.repeat,
    )
}

/// Pure kernel of [`translate_key_event`]. Split out because
/// `winit::event::KeyEvent` carries a private platform field and cannot be
/// struct-constructed headless — all mapping logic lives here and is fully
/// unit-tested; the wrapper above is a trivial field projection.
pub fn classify_key(
    physical_key: winit::keyboard::PhysicalKey,
    text: Option<&str>,
    modifiers: aqua_core::Modifiers,
    repeat: bool,
) -> (aqua_core::KeyEvent, Option<String>) {
    let shortcut = modifiers.contains(aqua_core::Modifiers::LOGO)
        || (modifiers.contains(aqua_core::Modifiers::CONTROL)
            && !modifiers.contains(aqua_core::Modifiers::ALT));
    let committed = if shortcut {
        None
    } else {
        text.map(|value| {
            value
                .chars()
                .filter(|c| !c.is_control())
                .collect::<String>()
        })
        .filter(|value| !value.is_empty())
    };
    let key = match text {
        Some(t) => {
            let mut chars = t.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if !c.is_control() => aqua_core::Key::Char(c),
                _ => {
                    aqua_core::Key::Named(aqua_code_to_named(translate_physical_key(physical_key)))
                }
            }
        }
        None => aqua_core::Key::Named(aqua_code_to_named(translate_physical_key(physical_key))),
    };
    (
        aqua_core::KeyEvent {
            key,
            modifiers,
            repeat,
        },
        committed,
    )
}

/// Preserve layout-resolved key identity independently of committed text.
/// In particular Ctrl+A may carry no printable text but is still the A key.
/// This pure projection is also used by native-boundary regression tests.
pub fn classify_logical_key(
    physical_key: winit::keyboard::PhysicalKey,
    logical_key: &winit::keyboard::Key,
    text: Option<&str>,
    modifiers: aqua_core::Modifiers,
    repeat: bool,
) -> (aqua_core::KeyEvent, Option<String>) {
    use aqua_core::{Key as C, NamedKey as K};
    use winit::keyboard::{Key as W, NamedKey as N};
    let (mut key, committed) = classify_key(physical_key, text, modifiers, repeat);
    match logical_key {
        W::Character(value) => {
            let mut chars = value.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                if !c.is_control() {
                    key.key = C::Char(c);
                }
            }
        }
        W::Named(named) => {
            let named = match named {
                N::Backspace => Some(K::Backspace),
                N::Delete => Some(K::Delete),
                N::Enter => Some(K::Enter),
                N::Tab => Some(K::Tab),
                N::Escape => Some(K::Escape),
                N::ArrowLeft => Some(K::Left),
                N::ArrowRight => Some(K::Right),
                N::ArrowUp => Some(K::Up),
                N::ArrowDown => Some(K::Down),
                N::Home => Some(K::Home),
                N::End => Some(K::End),
                N::PageUp => Some(K::PageUp),
                N::PageDown => Some(K::PageDown),
                N::Space => {
                    key.key = C::Char(' ');
                    None
                }
                _ => None,
            };
            if let Some(named) = named {
                key.key = C::Named(named);
            }
        }
        _ => {}
    }
    (key, committed)
}

/// `winit` IME occurrence -> canonical [`aqua_core::InputEvent`].
/// Always `Some` (every native IME occurrence has a core meaning).
pub fn translate_ime_core(ime: &winit::event::Ime) -> aqua_core::InputEvent {
    use aqua_core::{ImeEvent as C, InputEvent as E};
    match translate_ime(ime) {
        AquaImeEvent::Enabled => E::Ime(C::Start),
        AquaImeEvent::Disabled => E::Ime(C::Cancel),
        AquaImeEvent::Preedit(marked, cursor) => E::Ime(C::Update { marked, cursor }),
        AquaImeEvent::Commit(text) => E::Ime(C::Commit { text }),
    }
}

/// Physical pixels + scale -> canonical [`aqua_core::Point`] (DIPs, `f32`).
pub fn to_core_point(x_phys: f64, y_phys: f64, scale: f64) -> aqua_core::Point {
    to_logical(x_phys, y_phys, scale).as_core_point()
}

/// Build a canonical [`aqua_core::PointerEvent`]. The held-button mask is
/// facade-owned state (it tracks press/release); this constructor only
/// places the contact in logical units.
pub fn pointer_contact(
    contact: u64,
    kind: aqua_core::PointerKind,
    x_phys: f64,
    y_phys: f64,
    scale: f64,
    buttons: u8,
) -> aqua_core::PointerEvent {
    aqua_core::PointerEvent {
        contact,
        kind,
        position: to_core_point(x_phys, y_phys, scale),
        buttons,
    }
}

/// `winit` window theme -> portable preference.
pub fn translate_theme(theme: winit::window::Theme) -> ThemePreference {
    match theme {
        winit::window::Theme::Light => ThemePreference::Light,
        winit::window::Theme::Dark => ThemePreference::Dark,
    }
}

/// File-drag occurrences. `winit` reports real OS file drops (OLE on
/// Windows, portal-agnostic on Linux); typed in-app MIME drag-and-drop is a
/// separate, explicitly unsupported facility (see `services::InProcessDnD`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileDrop {
    Hovered(std::path::PathBuf),
    HoverCancelled,
    Dropped(std::path::PathBuf),
}

/// Map the file-drag `winit` variants; `None` for everything else.
pub fn translate_file_event(event: &winit::event::WindowEvent) -> Option<FileDrop> {
    use winit::event::WindowEvent as W;
    match event {
        W::HoveredFile(p) => Some(FileDrop::Hovered(p.clone())),
        W::HoveredFileCancelled => Some(FileDrop::HoverCancelled),
        W::DroppedFile(p) => Some(FileDrop::Dropped(p.clone())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::AquaWindowId;

    fn wid() -> AquaWindowId {
        AquaWindowId::new(9).unwrap()
    }

    #[test]
    fn button_and_state_translate() {
        assert_eq!(
            translate_element_state(winit::event::ElementState::Pressed),
            PressState::Pressed
        );
        assert_eq!(
            translate_mouse_button(winit::event::MouseButton::Left),
            PointerButton::Left
        );
        assert_eq!(
            translate_mouse_button(winit::event::MouseButton::Other(9)),
            PointerButton::Other(9)
        );
    }

    #[test]
    fn key_translation_covers_editing_shortcuts() {
        use winit::keyboard::{KeyCode as K, PhysicalKey as P};
        for (code, expect) in [
            (K::KeyA, AquaKeyCode::KeyA),
            (K::KeyC, AquaKeyCode::KeyC),
            (K::Escape, AquaKeyCode::Escape),
            (K::ArrowLeft, AquaKeyCode::ArrowLeft),
        ] {
            assert_eq!(translate_physical_key(P::Code(code)), expect);
        }
        assert_eq!(translate_physical_key(P::Code(K::F11)), AquaKeyCode::F(11));
        assert_eq!(
            translate_physical_key(P::Code(K::SuperLeft)),
            AquaKeyCode::Unidentified
        );
    }

    #[test]
    fn ime_commit_and_preedit_translate() {
        assert_eq!(
            translate_ime(&winit::event::Ime::Commit("hi".to_string())),
            AquaImeEvent::Commit("hi".to_string())
        );
        assert_eq!(
            translate_ime(&winit::event::Ime::Enabled),
            AquaImeEvent::Enabled
        );
    }

    #[test]
    fn coordinates_use_single_scale_policy() {
        let p = to_logical(300.0, 200.0, 2.0);
        assert_eq!(p, LogicalPosition { x: 150.0, y: 100.0 });
        // Degenerate scale never divides by zero.
        assert_eq!(to_logical(10.0, 10.0, 0.0).x, 10.0);
    }

    #[test]
    fn scheduler_reasons_cover_window_events() {
        assert_eq!(
            window_event_frame_reason(&WindowEvent::Resized(wid(), LogicalSize::new(1.0, 1.0))),
            Some(FrameReason::RESIZE)
        );
        assert_eq!(
            window_event_frame_reason(&WindowEvent::Occluded(wid(), true)),
            None
        );
        assert_eq!(
            window_event_frame_reason(&WindowEvent::Occluded(wid(), false)),
            Some(FrameReason::EXPOSE)
        );
        assert_eq!(
            window_event_frame_reason(&WindowEvent::Destroyed(wid())),
            None
        );
    }

    /// Native-event integration: focus / IME / modifiers through the PUBLIC
    /// canonical translators into core vocabulary.
    #[test]
    fn native_focus_ime_modifiers_integrate_to_core() {
        use aqua_core::{ImeEvent as C, InputEvent as E, Key as K, NamedKey as N};
        // IME commit carries multi-scalar text verbatim (emoji survives).
        let commit = translate_ime_core(&winit::event::Ime::Commit("héllo 🌍".to_string()));
        assert_eq!(
            commit,
            E::Ime(C::Commit {
                text: "héllo 🌍".to_string()
            })
        );
        // Preedit keeps marked text + byte cursor for candidate placement.
        let pre = translate_ime_core(&winit::event::Ime::Preedit("ni".to_string(), Some((0, 2))));
        assert_eq!(
            pre,
            E::Ime(C::Update {
                marked: "ni".to_string(),
                cursor: Some((0, 2))
            })
        );
        // Modifiers: default (no keys) maps to NONE; each flag maps singly
        // through the same pure path the facade uses for ModifiersChanged.
        let mods = translate_modifiers(&winit::event::Modifiers::default());
        assert!(mods.is_empty());
        let mut state = winit::keyboard::ModifiersState::empty();
        state |= winit::keyboard::ModifiersState::SHIFT | winit::keyboard::ModifiersState::SUPER;
        let mods2 = modifiers_from_state(state);
        assert!(mods2.contains(aqua_core::Modifiers::SHIFT));
        assert!(mods2.contains(aqua_core::Modifiers::LOGO));
        assert!(!mods2.contains(aqua_core::Modifiers::CONTROL));
        // Layout-resolved text wins as Char identity; function keys map named.
        let (ke, text) = classify_key(
            winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyA),
            Some("ß"),
            mods,
            true,
        );
        assert_eq!(ke.key, K::Char('ß'));
        assert!(ke.repeat);
        assert_eq!(text.as_deref(), Some("ß"));
        let (ke11, no_text) = classify_key(
            winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::F11),
            None,
            mods,
            false,
        );
        assert_eq!(ke11.key, K::Named(N::Function(11)));
        assert_eq!(no_text, None);
        // Multi-scalar input (combining sequence) is NOT Char identity.
        let (ke_seq, seq_text) = classify_key(
            winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyA),
            Some("é"),
            mods,
            false,
        );
        assert!(matches!(ke_seq.key, K::Named(_)));
        assert_eq!(seq_text.as_deref(), Some("é"));
    }
}
