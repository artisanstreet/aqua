use crate::looping::emit;
use crate::view::AquaView;
use aqua_core::{InputEvent, Point, PointerEvent, PointerKind};
use aqua_shell::input::{button, PointerButtonSource};
use aqua_shell::ShellEvent;
use objc2_app_kit::{NSEvent, NSEventType};

pub(crate) fn location(view: &AquaView, event: &NSEvent) -> Point {
    let in_window = match (
        event.window(objc2_foundation::MainThreadMarker::from(view)),
        view.window(),
    ) {
        (None, Some(window)) => window.convertPointFromScreen(event.locationInWindow()),
        _ => event.locationInWindow(),
    };
    let local = view.convertPoint_fromView(in_window, None);
    Point::new(local.x as f32, local.y as f32)
}

fn source(event: &NSEvent) -> PointerButtonSource {
    match event.r#type() {
        NSEventType::LeftMouseDown | NSEventType::LeftMouseUp | NSEventType::LeftMouseDragged => {
            PointerButtonSource::Left
        }
        NSEventType::RightMouseDown
        | NSEventType::RightMouseUp
        | NSEventType::RightMouseDragged => PointerButtonSource::Right,
        _ => match event.buttonNumber() {
            2 => PointerButtonSource::Middle,
            other => PointerButtonSource::X(u16::try_from(other - 2).unwrap_or(u16::MAX)),
        },
    }
}

fn pointer(view: &AquaView, event: &NSEvent, buttons: u8) -> PointerEvent {
    PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: location(view, event),
        buttons,
    }
}

fn buttons_after(view: &AquaView, change: impl FnOnce(u8) -> u8) -> u8 {
    crate::state::window(view.id(), |window| {
        window.pointer.buttons = change(window.pointer.buttons);
        window.pointer.buttons
    })
    .unwrap_or(0)
}

pub(crate) fn pressed(view: &AquaView, event: &NSEvent) {
    let which = source(event);
    if which == PointerButtonSource::Left && crate::chrome::caption_press(view, event) {
        return;
    }
    let (button, bit) = button(which);
    let buttons = buttons_after(view, |held| held | bit);
    emit(
        view.id(),
        ShellEvent::Input(InputEvent::PointerDown(
            pointer(view, event, buttons),
            button,
        )),
    );
}

pub(crate) fn released(view: &AquaView, event: &NSEvent) {
    let (button, bit) = button(source(event));
    let held = crate::state::window(view.id(), |window| window.pointer.buttons & bit != 0)
        .unwrap_or(false);
    if !held {
        return;
    }
    let buttons = buttons_after(view, |held| held & !bit);
    emit(
        view.id(),
        ShellEvent::Input(InputEvent::PointerUp(pointer(view, event, buttons), button)),
    );
}

pub(crate) fn moved(view: &AquaView, event: &NSEvent) {
    let (raw, buttons) = crate::state::window(view.id(), |window| {
        (window.raw_mouse, window.pointer.buttons)
    })
    .unwrap_or((false, 0));
    if raw {
        emit(
            view.id(),
            ShellEvent::RawMouse {
                dx: event.deltaX().round() as i32,
                dy: event.deltaY().round() as i32,
            },
        );
    }
    emit(
        view.id(),
        ShellEvent::Input(InputEvent::PointerMove(pointer(view, event, buttons))),
    );
    crate::cursor::apply(view.id());
}

pub(crate) fn entered(view: &AquaView, event: &NSEvent) {
    crate::state::window(view.id(), |window| window.pointer.inside = true);
    emit(view.id(), ShellEvent::PointerEntered);
    let buttons = crate::state::window(view.id(), |window| window.pointer.buttons).unwrap_or(0);
    emit(
        view.id(),
        ShellEvent::Input(InputEvent::PointerMove(pointer(view, event, buttons))),
    );
    crate::cursor::apply(view.id());
}

pub(crate) fn exited(view: &AquaView) {
    crate::state::window(view.id(), |window| window.pointer.inside = false);
    emit(view.id(), ShellEvent::PointerLeft);
    crate::cursor::reset();
}
