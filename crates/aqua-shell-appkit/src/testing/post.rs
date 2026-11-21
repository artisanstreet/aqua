use super::inject::{cg_flags_of, mouse_event, Button, Motion};
use crate::window::Window;
use aqua_core::Modifiers;
use objc2_core_foundation::CGPoint;
use objc2_core_graphics::{CGEvent, CGEventTapLocation, CGScrollEventUnit};

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

pub const TAB: u16 = 48;
pub const END: u16 = 119;
pub const RETURN: u16 = 36;

pub fn scroll_at((x, y): (f64, f64), lines: i32) {
    let event = CGEvent::new_scroll_wheel_event2(None, CGScrollEventUnit::Line, 1, lines, 0, 0)
        .expect("CoreGraphics builds scroll events");
    CGEvent::set_location(Some(&event), CGPoint::new(x, y));
    post(&event);
}

pub fn tap(code: u16, modifiers: Modifiers) {
    key(code, true, modifiers);
    key(code, false, modifiers);
}

pub fn letter_code(letter: char) -> Option<u16> {
    let code = match letter.to_ascii_lowercase() {
        'a' => 0,
        's' => 1,
        'd' => 2,
        'f' => 3,
        'h' => 4,
        'g' => 5,
        'z' => 6,
        'x' => 7,
        'c' => 8,
        'v' => 9,
        'b' => 11,
        'q' => 12,
        'w' => 13,
        'e' => 14,
        'r' => 15,
        'y' => 16,
        't' => 17,
        'o' => 31,
        'u' => 32,
        'i' => 34,
        'p' => 35,
        'l' => 37,
        'j' => 38,
        'k' => 40,
        'n' => 45,
        'm' => 46,
        ' ' => 49,
        _ => return None,
    };
    Some(code)
}

pub fn type_text(text: &str) {
    text.chars().for_each(|letter| {
        let code = letter_code(letter)
            .unwrap_or_else(|| panic!("{letter:?} has no key in the US layout table"));
        tap(code, Modifiers::NONE);
    });
}
