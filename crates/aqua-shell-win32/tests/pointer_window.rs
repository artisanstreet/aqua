#![cfg(windows)]

use aqua_core::{InputEvent, Point, PointerButton, PointerKind};
use aqua_protocol::{Instant, WindowId};
use aqua_shell_win32::pointer::{
    contact_id, Device, BUTTON_ERASER, BUTTON_PRIMARY, BUTTON_SECONDARY, FLAG_CANCELED,
    FLAG_INCONTACT, FLAG_UP, PEN_BARREL, PEN_ERASER,
};
use aqua_shell_win32::pointer_source::{clear_pointer_source, set_pointer_source};
use aqua_shell_win32::pointer_source::{PointerSource, Reading};
use aqua_shell_win32::{now, quit, run, Handler, ShellEvent, Window, WindowOptions};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;
use windows::Win32::Foundation::{HWND, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::WindowsAndMessaging::{
    SendMessageW, WM_POINTERCAPTURECHANGED, WM_POINTERDOWN, WM_POINTERLEAVE, WM_POINTERUP,
    WM_POINTERUPDATE,
};

const DOWN: u32 = FLAG_INCONTACT | 0x0002 | 0x0001;
const UPDATE: u32 = FLAG_INCONTACT | 0x0002;
const LIFT: u32 = FLAG_UP;

#[derive(Clone, Copy)]
struct Contact {
    device: Device,
    flags: u32,
    pen_flags: u32,
    client: (i32, i32),
}

type Table = Rc<RefCell<HashMap<u32, Contact>>>;

struct Digitizer {
    hwnd: HWND,
    table: Table,
}

impl PointerSource for Digitizer {
    fn read(&mut self, id: u32) -> Option<Reading> {
        let contact = *self.table.borrow().get(&id)?;
        let mut origin = POINT::default();
        let _ = unsafe { ClientToScreen(self.hwnd, &mut origin) };
        Some(Reading {
            device: contact.device,
            flags: contact.flags,
            pen_flags: contact.pen_flags,
            screen: (origin.x + contact.client.0, origin.y + contact.client.1),
        })
    }
}

struct Sink {
    events: Rc<RefCell<Vec<ShellEvent>>>,
    job: Option<Box<dyn FnOnce()>>,
    deadline: Instant,
    quiet_turns: u32,
}

impl Handler for Sink {
    fn window_event(&mut self, _: WindowId, event: ShellEvent) {
        self.events.borrow_mut().push(event);
    }

    fn tick(&mut self, at: Instant) -> Option<Instant> {
        assert!(at < self.deadline, "pointer script timed out");
        match self.job.take() {
            Some(job) => {
                job();
                Some(at + Duration::from_millis(5))
            }
            None => {
                self.quiet_turns += 1;
                match self.quiet_turns > 3 {
                    true => {
                        quit();
                        None
                    }
                    false => Some(at + Duration::from_millis(5)),
                }
            }
        }
    }
}

struct Rig {
    window: Window,
    table: Table,
    events: Rc<RefCell<Vec<ShellEvent>>>,
}

impl Rig {
    fn new() -> Rig {
        let window = Window::create(&WindowOptions {
            title: "aqua-pointer-test".into(),
            logical_size: (320.0, 200.0),
            visible: false,
            ..Default::default()
        })
        .expect("window");
        let table: Table = Default::default();
        set_pointer_source(Digitizer {
            hwnd: HWND(window.raw_hwnd() as *mut _),
            table: table.clone(),
        });
        Rig {
            window,
            table,
            events: Default::default(),
        }
    }

    fn scale(&self) -> f64 {
        self.window.scale()
    }

    fn run(&self, script: impl FnOnce(&Wire) + 'static) -> Vec<InputEvent> {
        let wire = Wire {
            hwnd: HWND(self.window.raw_hwnd() as *mut _),
            table: self.table.clone(),
        };
        run(Sink {
            events: self.events.clone(),
            job: Some(Box::new(move || script(&wire))),
            deadline: now() + Duration::from_secs(20),
            quiet_turns: 0,
        })
        .expect("loop");
        self.events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                ShellEvent::Input(input) => Some(input.clone()),
                _ => None,
            })
            .collect()
    }

    fn finish(self) {
        clear_pointer_source();
        self.window.destroy();
    }
}

struct Wire {
    hwnd: HWND,
    table: Table,
}

impl Wire {
    fn send(&self, message: u32, id: u32, contact: Option<Contact>) {
        if let Some(contact) = contact {
            self.table.borrow_mut().insert(id, contact);
        }
        unsafe { SendMessageW(self.hwnd, message, Some(WPARAM(id as usize)), None) };
    }
}

fn touch(flags: u32, x: i32, y: i32) -> Contact {
    Contact {
        device: Device::Touch,
        flags,
        pen_flags: 0,
        client: (x, y),
    }
}

fn pen(flags: u32, pen_flags: u32, x: i32, y: i32) -> Contact {
    Contact {
        device: Device::Pen,
        flags,
        pen_flags,
        client: (x, y),
    }
}

fn at(rig: &Rig, x: i32, y: i32) -> Point {
    Point::new(
        (f64::from(x) / rig.scale()) as f32,
        (f64::from(y) / rig.scale()) as f32,
    )
}

#[test]
fn a_touch_contact_becomes_down_move_up_in_logical_coordinates() {
    let rig = Rig::new();
    let events = rig.run(|wire| {
        wire.send(WM_POINTERDOWN, 7, Some(touch(DOWN, 100, 50)));
        wire.send(WM_POINTERUPDATE, 7, Some(touch(UPDATE, 140, 90)));
        wire.send(WM_POINTERUP, 7, Some(touch(LIFT, 140, 90)));
    });
    let [InputEvent::PointerDown(down, PointerButton::Primary), InputEvent::PointerMove(moved), InputEvent::PointerUp(up, PointerButton::Primary)] =
        events.as_slice()
    else {
        panic!("{events:?}");
    };
    assert_eq!(down.contact, contact_id(7));
    assert_eq!(down.kind, PointerKind::Touch);
    assert_eq!(down.buttons, BUTTON_PRIMARY);
    assert_eq!(down.position, at(&rig, 100, 50));
    assert_eq!(moved.position, at(&rig, 140, 90));
    assert_eq!(moved.buttons, BUTTON_PRIMARY);
    assert_eq!(up.buttons, 0);
    rig.finish();
}

#[test]
fn simultaneous_touches_keep_independent_contact_ids_and_state() {
    let rig = Rig::new();
    let events = rig.run(|wire| {
        wire.send(WM_POINTERDOWN, 1, Some(touch(DOWN, 10, 10)));
        wire.send(WM_POINTERDOWN, 2, Some(touch(DOWN, 200, 100)));
        wire.send(WM_POINTERUPDATE, 1, Some(touch(UPDATE, 20, 20)));
        wire.send(WM_POINTERUP, 1, Some(touch(LIFT, 20, 20)));
        wire.send(WM_POINTERUPDATE, 2, Some(touch(UPDATE, 210, 110)));
        wire.send(WM_POINTERUP, 2, Some(touch(LIFT, 210, 110)));
    });
    let summary: Vec<(&str, u64)> = events
        .iter()
        .map(|event| match event {
            InputEvent::PointerDown(e, _) => ("down", e.contact),
            InputEvent::PointerMove(e) => ("move", e.contact),
            InputEvent::PointerUp(e, _) => ("up", e.contact),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            ("down", contact_id(1)),
            ("down", contact_id(2)),
            ("move", contact_id(1)),
            ("up", contact_id(1)),
            ("move", contact_id(2)),
            ("up", contact_id(2)),
        ]
    );
    rig.finish();
}

#[test]
fn a_pen_reports_barrel_as_secondary_and_the_eraser_tip_as_extra_five() {
    let rig = Rig::new();
    let events = rig.run(|wire| {
        wire.send(WM_POINTERDOWN, 3, Some(pen(DOWN, 0, 30, 30)));
        wire.send(WM_POINTERUPDATE, 3, Some(pen(UPDATE, PEN_BARREL, 31, 31)));
        wire.send(WM_POINTERUPDATE, 3, Some(pen(UPDATE, 0, 32, 32)));
        wire.send(WM_POINTERUP, 3, Some(pen(LIFT, 0, 32, 32)));
        wire.send(WM_POINTERDOWN, 3, Some(pen(DOWN, PEN_ERASER, 60, 60)));
        wire.send(WM_POINTERUP, 3, Some(pen(LIFT, PEN_ERASER, 60, 60)));
    });
    let pressed: Vec<(PointerButton, u8)> = events
        .iter()
        .filter_map(|event| match event {
            InputEvent::PointerDown(e, button) => Some((*button, e.buttons)),
            _ => None,
        })
        .collect();
    assert_eq!(
        pressed,
        vec![
            (PointerButton::Primary, BUTTON_PRIMARY),
            (PointerButton::Secondary, BUTTON_PRIMARY | BUTTON_SECONDARY),
            (PointerButton::Extra(5), BUTTON_ERASER),
        ]
    );
    assert!(events.iter().all(|event| match event {
        InputEvent::PointerDown(e, _) | InputEvent::PointerUp(e, _) => e.kind == PointerKind::Pen,
        InputEvent::PointerMove(e) => e.kind == PointerKind::Pen,
        _ => false,
    }));
    rig.finish();
}

#[test]
fn losing_capture_or_a_canceled_update_cancels_a_held_contact() {
    let rig = Rig::new();
    let events = rig.run(|wire| {
        wire.send(WM_POINTERDOWN, 4, Some(touch(DOWN, 10, 10)));
        wire.send(WM_POINTERCAPTURECHANGED, 4, None);
        wire.send(WM_POINTERDOWN, 5, Some(touch(DOWN, 10, 10)));
        wire.send(
            WM_POINTERUPDATE,
            5,
            Some(touch(UPDATE | FLAG_CANCELED, 10, 10)),
        );
    });
    let cancels: Vec<u64> = events
        .iter()
        .filter_map(|event| match event {
            InputEvent::PointerCancel { contact } => Some(*contact),
            _ => None,
        })
        .collect();
    assert_eq!(cancels, vec![contact_id(4), contact_id(5)]);
    rig.finish();
}

#[test]
fn leaving_after_lift_reports_nothing_and_leaving_while_held_reports_pointer_left() {
    let rig = Rig::new();
    let events = rig.run(|wire| {
        wire.send(WM_POINTERDOWN, 8, Some(touch(DOWN, 10, 10)));
        wire.send(WM_POINTERUP, 8, Some(touch(LIFT, 10, 10)));
        wire.send(WM_POINTERLEAVE, 8, None);
        wire.send(WM_POINTERDOWN, 9, Some(touch(DOWN, 10, 10)));
        wire.send(WM_POINTERLEAVE, 9, None);
    });
    let lefts = rig
        .events
        .borrow()
        .iter()
        .filter(|event| matches!(event, ShellEvent::PointerLeft))
        .count();
    assert_eq!(lefts, 1, "{events:?}");
    rig.finish();
}

#[test]
fn contacts_the_source_does_not_know_produce_no_input() {
    let rig = Rig::new();
    let events = rig.run(|wire| {
        wire.send(WM_POINTERDOWN, 99, None);
        wire.send(WM_POINTERUPDATE, 99, None);
        wire.send(WM_POINTERUP, 99, None);
    });
    assert!(events.is_empty(), "{events:?}");
    rig.finish();
}
