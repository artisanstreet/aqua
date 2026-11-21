use super::inject::{cg_flags_of, mouse_event, Button, Motion};
use crate::window::Window;
use aqua_core::Modifiers;
use objc2_core_graphics::{CGEvent, CGEventTapLocation};

fn post(event: &CGEvent) {
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(event));
}

pub fn mouse(window: Window, button: Button, motion: Motion, at: (f64, f64), clicks: i64) {
    post(&mouse_event(window, button, motion, at, clicks));
}

pub fn key(code: u16, down: bool, modifiers: Modifiers) {
    let event =
        CGEvent::new_keyboard_event(None, code, down).expect("CoreGraphics builds key events");
    CGEvent::set_flags(Some(&event), cg_flags_of(modifiers));
    post(&event);
}

pub fn mouse_at(button: Button, motion: Motion, (x, y): (f64, f64), clicks: i64) {
    post(&super::inject::mouse_event_at(
        button,
        motion,
        (x, y),
        clicks,
    ));
}
