use crate::looping::emit;
use crate::state::Shell;
use aqua_core::{InputEvent, Point, PointerButton, PointerEvent, PointerKind};
use aqua_protocol::WindowId;
use aqua_shell::ShellEvent;
use std::collections::HashMap;
use wayland_client::protocol::wl_touch::{self, WlTouch};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

#[derive(Debug, Clone, Copy, PartialEq)]
struct Contact {
    window: WindowId,
    at: (f64, f64),
}

pub(crate) struct Touch {
    proxy: WlTouch,
    contacts: HashMap<i32, Contact>,
}

fn contact_id(id: i32) -> u64 {
    u64::from(id.unsigned_abs()) + 1
}

fn event(id: i32, contact: Contact, buttons: u8) -> PointerEvent {
    PointerEvent {
        contact: contact_id(id),
        kind: PointerKind::Touch,
        position: Point::new(contact.at.0 as f32, contact.at.1 as f32),
        buttons,
    }
}

impl Touch {
    pub(crate) fn new(proxy: WlTouch) -> Self {
        Self {
            proxy,
            contacts: HashMap::new(),
        }
    }

    pub(crate) fn forget(&mut self, window: WindowId) {
        self.contacts.retain(|_, contact| contact.window != window);
    }

    fn down(&mut self, window: WindowId, id: i32, at: (f64, f64)) {
        let contact = Contact { window, at };
        self.contacts.insert(id, contact);
        emit(
            window,
            ShellEvent::Input(InputEvent::PointerDown(
                event(id, contact, 1),
                PointerButton::Primary,
            )),
        );
    }

    fn up(&mut self, id: i32) {
        if let Some(contact) = self.contacts.remove(&id) {
            emit(
                contact.window,
                ShellEvent::Input(InputEvent::PointerUp(
                    event(id, contact, 0),
                    PointerButton::Primary,
                )),
            );
        }
    }

    fn moved(&mut self, id: i32, at: (f64, f64)) {
        if let Some(contact) = self.contacts.get_mut(&id) {
            contact.at = at;
            let contact = *contact;
            emit(
                contact.window,
                ShellEvent::Input(InputEvent::PointerMove(event(id, contact, 1))),
            );
        }
    }

    fn cancel(&mut self) {
        for (id, contact) in self.contacts.drain() {
            emit(
                contact.window,
                ShellEvent::Input(InputEvent::PointerCancel {
                    contact: contact_id(id),
                }),
            );
        }
    }
}

impl Drop for Touch {
    fn drop(&mut self) {
        if self.proxy.version() >= 3 {
            self.proxy.release();
        }
    }
}

impl Dispatch<WlTouch, ()> for Shell {
    fn event(
        state: &mut Self,
        _: &WlTouch,
        event: wl_touch::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let seat = &mut state.seat;
        let Some(touch) = seat.touch.as_mut() else {
            return;
        };
        match event {
            wl_touch::Event::Down {
                serial,
                surface,
                id,
                x,
                y,
                ..
            } => {
                seat.serial = Some(serial);
                if let Some(window) = surface.data::<WindowId>().copied() {
                    seat.interacted = Some(window);
                    touch.down(window, id, (x, y));
                }
            }
            wl_touch::Event::Up { id, .. } => touch.up(id),
            wl_touch::Event::Motion { id, x, y, .. } => touch.moved(id, (x, y)),
            wl_touch::Event::Cancel => touch.cancel(),
            _ => {}
        }
    }
}
