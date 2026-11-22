use aqua_core::CursorIcon;
use aqua_protocol::WindowId;
use objc2::rc::Retained;
use objc2::{sel, ClassType};
use objc2_app_kit::{NSCursor, NSCursorFrameResizeDirections, NSCursorFrameResizePosition};
use std::cell::Cell;

thread_local! {
    static HIDDEN: Cell<bool> = const { Cell::new(false) };
}

fn frame_resize(position: NSCursorFrameResizePosition) -> Retained<NSCursor> {
    match NSCursor::class().responds_to(sel!(frameResizeCursorFromPosition:inDirections:)) {
        true => NSCursor::frameResizeCursorFromPosition_inDirections(
            position,
            NSCursorFrameResizeDirections::All,
        ),
        false => NSCursor::arrowCursor(),
    }
}

#[allow(deprecated)]
fn native(icon: CursorIcon) -> Retained<NSCursor> {
    match icon {
        CursorIcon::Default
        | CursorIcon::Wait
        | CursorIcon::Progress
        | CursorIcon::Help
        | CursorIcon::Hidden => NSCursor::arrowCursor(),
        CursorIcon::Text => NSCursor::IBeamCursor(),
        CursorIcon::Pointer => NSCursor::pointingHandCursor(),
        CursorIcon::Crosshair => NSCursor::crosshairCursor(),
        CursorIcon::Move | CursorIcon::Grab => NSCursor::openHandCursor(),
        CursorIcon::Grabbing => NSCursor::closedHandCursor(),
        CursorIcon::NotAllowed => NSCursor::operationNotAllowedCursor(),
        CursorIcon::EwResize | CursorIcon::ColResize => NSCursor::resizeLeftRightCursor(),
        CursorIcon::NsResize | CursorIcon::RowResize => NSCursor::resizeUpDownCursor(),
        CursorIcon::NwseResize => frame_resize(NSCursorFrameResizePosition::TopLeft),
        CursorIcon::NeswResize => frame_resize(NSCursorFrameResizePosition::TopRight),
    }
}

fn show(icon: CursorIcon) {
    let hide = icon == CursorIcon::Hidden;
    match (HIDDEN.replace(hide), hide) {
        (false, true) => NSCursor::hide(),
        (true, false) => NSCursor::unhide(),
        _ => {}
    }
    native(icon).set();
}

pub(crate) fn apply(id: WindowId) {
    let wanted =
        crate::state::window(id, |window| window.pointer.inside.then_some(window.cursor)).flatten();
    if let Some(icon) = wanted {
        show(icon);
    }
}

pub(crate) fn reset() {
    show(CursorIcon::Default);
}

#[cfg(feature = "testing")]
pub(crate) fn current() -> Option<CursorIcon> {
    let shown = NSCursor::currentCursor();
    [
        CursorIcon::Text,
        CursorIcon::Pointer,
        CursorIcon::Crosshair,
        CursorIcon::Grabbing,
        CursorIcon::Grab,
        CursorIcon::NotAllowed,
        CursorIcon::EwResize,
        CursorIcon::NsResize,
        CursorIcon::Default,
    ]
    .into_iter()
    .find(|icon| native(*icon) == shown)
}
