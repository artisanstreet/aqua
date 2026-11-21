mod keyboard;
mod keys;
mod pointer;
mod repeat;
mod touch;

use crate::state::Shell;
use aqua_protocol::{Instant, WindowId};
use wayland_client::protocol::wl_seat::{self, Capability, WlSeat};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};

pub(crate) use pointer::{cursor_changed, Pointer};

#[derive(Default)]
pub(crate) struct Seat {
    pub pointer: Option<pointer::Pointer>,
    pub keyboard: Option<keyboard::Keyboard>,
    pub touch: Option<touch::Touch>,
    pub serial: Option<u32>,
    pub interacted: Option<WindowId>,
}

impl Seat {
    pub(crate) fn release(&mut self) {
        self.pointer = None;
        self.keyboard = None;
        self.touch = None;
    }

    pub(crate) fn engaged(&self) -> Option<WindowId> {
        self.keyboard
            .as_ref()
            .and_then(keyboard::Keyboard::focus)
            .or(self.interacted)
    }

    pub(crate) fn forget(&mut self, window: WindowId) {
        if self.interacted == Some(window) {
            self.interacted = None;
        }
        if let Some(pointer) = self.pointer.as_mut() {
            pointer.forget(window);
        }
        if let Some(keyboard) = self.keyboard.as_mut() {
            keyboard.forget(window);
        }
        if let Some(touch) = self.touch.as_mut() {
            touch.forget(window);
        }
    }
}

pub(crate) fn fire_repeats(shell: &mut Shell, now: Instant) {
    if let Some(keyboard) = shell.seat.keyboard.as_mut() {
        keyboard.fire_repeats(now);
    }
}

pub(crate) fn repeat_deadline(shell: &mut Shell) -> Option<Instant> {
    shell
        .seat
        .keyboard
        .as_ref()
        .and_then(keyboard::Keyboard::repeat_deadline)
}

impl Dispatch<WlSeat, ()> for Shell {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        else {
            return;
        };
        match (
            capabilities.contains(Capability::Pointer),
            state.seat.pointer.is_some(),
        ) {
            (true, false) => {
                let proxy = seat.get_pointer(qh, ());
                let shape = state
                    .globals
                    .cursor_shape
                    .as_ref()
                    .map(|manager| manager.get_pointer(&proxy, qh, ()));
                state.seat.pointer = Some(Pointer::new(proxy, shape));
            }
            (false, true) => state.seat.pointer = None,
            _ => {}
        }
        match (
            capabilities.contains(Capability::Keyboard),
            state.seat.keyboard.is_some(),
        ) {
            (true, false) => {
                state.seat.keyboard = Some(keyboard::Keyboard::new(seat.get_keyboard(qh, ())))
            }
            (false, true) => state.seat.keyboard = None,
            _ => {}
        }
        match (
            capabilities.contains(Capability::Touch),
            state.seat.touch.is_some(),
        ) {
            (true, false) => state.seat.touch = Some(touch::Touch::new(seat.get_touch(qh, ()))),
            (false, true) => state.seat.touch = None,
            _ => {}
        }
    }
}
