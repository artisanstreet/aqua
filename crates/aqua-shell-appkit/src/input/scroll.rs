use crate::input::pointer::location;
use crate::looping::emit;
use crate::model::scroll::{delta, Wheel};
use crate::view::AquaView;
use aqua_core::InputEvent;
use aqua_shell::ShellEvent;
use objc2_app_kit::NSEvent;

pub(crate) fn wheel(view: &AquaView, event: &NSEvent) {
    let (dx, dy) = (event.scrollingDeltaX(), event.scrollingDeltaY());
    let wheel = match event.hasPreciseScrollingDeltas() {
        true => Wheel::Precise { dx, dy },
        false => Wheel::Lines { dx, dy },
    };
    if let Some(delta) = delta(wheel) {
        emit(
            view.id(),
            ShellEvent::Input(InputEvent::Scroll {
                at: location(view, event),
                delta,
            }),
        );
    }
}
