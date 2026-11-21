use crate::input::{layout, text};
use crate::looping::emit;
use crate::model::keys::{
    key_equivalent as equivalent, key_outcome, resolve_key, Equivalent, Interpretation, KeyOutcome,
};
use crate::view::AquaView;
use aqua_core::{InputEvent, KeyEvent, Modifiers};
use aqua_shell::ShellEvent;
use objc2::DefinedClass;
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
use objc2_foundation::{MainThreadMarker, NSArray};

pub(crate) fn modifiers_of(flags: NSEventModifierFlags) -> Modifiers {
    aqua_shell::input::modifiers(
        flags.contains(NSEventModifierFlags::Shift),
        flags.contains(NSEventModifierFlags::Control),
        flags.contains(NSEventModifierFlags::Option),
        flags.contains(NSEventModifierFlags::Command),
    )
}

fn ignoring_modifiers(event: &NSEvent) -> Option<char> {
    event
        .charactersIgnoringModifiers()
        .and_then(|characters| characters.to_string().chars().next())
}

fn key_event(event: &NSEvent, repeat: bool) -> KeyEvent {
    let modifiers = modifiers_of(event.modifierFlags());
    let code = event.keyCode();
    let character = match modifiers.contains(Modifiers::LOGO) {
        true => layout::ascii_character(code, modifiers.contains(Modifiers::SHIFT))
            .or_else(|| ignoring_modifiers(event)),
        false => ignoring_modifiers(event),
    };
    KeyEvent {
        key: resolve_key(code, character),
        modifiers,
        repeat,
    }
}

fn press(view: &AquaView, key: KeyEvent) {
    emit(view.id(), ShellEvent::Input(InputEvent::KeyDown(key)));
}

pub(crate) fn key_equivalent(view: &AquaView, event: &NSEvent) -> bool {
    let mtm = MainThreadMarker::from(view);
    let ours = event.r#type() == NSEventType::KeyDown
        && event
            .window(mtm)
            .zip(view.window())
            .is_some_and(|(from, mine)| from == mine);
    if !ours {
        return false;
    }
    let key = key_event(event, event.isARepeat());
    match equivalent(key.key, key.modifiers) {
        Some(Equivalent::App) => {
            press(view, key);
            true
        }
        Some(Equivalent::Menu) | None => false,
    }
}

pub(crate) fn key_down(view: &AquaView, event: &NSEvent) {
    let key = key_event(event, event.isARepeat());
    if key.modifiers.contains(Modifiers::LOGO) {
        press(view, key);
        return;
    }
    text::key_pressed(view);
    let marked_before = text::has_marked(view);
    let previous = view.ivars().pass.replace(Some(Interpretation {
        marked_before,
        ..Interpretation::default()
    }));
    assert!(
        previous.is_none(),
        "invariant: key interpretation never nests"
    );
    view.interpretKeyEvents(&NSArray::from_slice(&[event]));
    let mut pass = view
        .ivars()
        .pass
        .take()
        .expect("invariant: the key interpretation survives interpretKeyEvents");
    pass.marked_after = text::has_marked(view);
    match key_outcome(&pass) {
        KeyOutcome::Consumed => view.ivars().consumed.set(Some(event.keyCode())),
        KeyOutcome::Key => press(view, key),
        KeyOutcome::KeyAndText(typed) => {
            press(view, key);
            emit(
                view.id(),
                ShellEvent::Input(InputEvent::TextInput { text: typed }),
            );
        }
    }
}

pub(crate) fn key_up(view: &AquaView, event: &NSEvent) {
    let code = event.keyCode();
    if view.ivars().consumed.get() == Some(code) {
        view.ivars().consumed.set(None);
        return;
    }
    emit(
        view.id(),
        ShellEvent::Input(InputEvent::KeyUp(key_event(event, false))),
    );
}

pub(crate) fn flags_changed(view: &AquaView, event: &NSEvent) {
    emit(
        view.id(),
        ShellEvent::Input(InputEvent::ModifiersChanged(modifiers_of(
            event.modifierFlags(),
        ))),
    );
}
