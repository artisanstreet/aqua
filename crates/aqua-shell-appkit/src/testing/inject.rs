use crate::view::AquaView;
use crate::window::Window;
use aqua_core::Modifiers;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
use objc2_core_foundation::CGPoint;
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventType, CGMouseButton, CGScrollEventUnit,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSString};

pub fn flags_of(modifiers: Modifiers) -> NSEventModifierFlags {
    [
        (Modifiers::SHIFT, NSEventModifierFlags::Shift),
        (Modifiers::CONTROL, NSEventModifierFlags::Control),
        (Modifiers::ALT, NSEventModifierFlags::Option),
        (Modifiers::LOGO, NSEventModifierFlags::Command),
    ]
    .into_iter()
    .filter(|(modifier, _)| modifiers.contains(*modifier))
    .fold(NSEventModifierFlags::empty(), |flags, (_, flag)| {
        flags | flag
    })
}

pub(crate) fn cg_flags_of(modifiers: Modifiers) -> CGEventFlags {
    [
        (Modifiers::SHIFT, CGEventFlags::MaskShift),
        (Modifiers::CONTROL, CGEventFlags::MaskControl),
        (Modifiers::ALT, CGEventFlags::MaskAlternate),
        (Modifiers::LOGO, CGEventFlags::MaskCommand),
    ]
    .into_iter()
    .filter(|(modifier, _)| modifiers.contains(*modifier))
    .fold(CGEventFlags::empty(), |flags, (_, flag)| flags | flag)
}

fn view_of(window: Window) -> Retained<AquaView> {
    crate::state::window(window.id(), |state| state.view.clone())
        .expect("tests inject into a live window")
}

fn send(event: &NSEvent) {
    let mtm = MainThreadMarker::new().expect("tests inject on the main thread");
    crate::app::shared(mtm).sendEvent(event);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keystroke<'a> {
    pub code: u16,
    pub characters: &'a str,
    pub ignoring_modifiers: &'a str,
    pub modifiers: Modifiers,
    pub repeat: bool,
}

fn key_event(window: Window, kind: NSEventType, stroke: Keystroke<'_>) -> Retained<NSEvent> {
    let number = super::window_number(window).expect("tests inject into a live window");
    NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        kind,
        NSPoint::new(0.0, 0.0),
        flags_of(stroke.modifiers),
        0.0,
        number,
        None,
        &NSString::from_str(stroke.characters),
        &NSString::from_str(stroke.ignoring_modifiers),
        stroke.repeat,
        stroke.code,
    )
    .expect("AppKit builds key events")
}

pub fn key_down(window: Window, stroke: Keystroke<'_>) {
    send(&key_event(window, NSEventType::KeyDown, stroke));
}

pub fn key_up(window: Window, stroke: Keystroke<'_>) {
    send(&key_event(window, NSEventType::KeyUp, stroke));
}

pub fn modifiers_changed(window: Window, code: u16, modifiers: Modifiers) {
    let event =
        CGEvent::new_keyboard_event(None, code, true).expect("CoreGraphics builds key events");
    CGEvent::set_type(Some(&event), CGEventType::FlagsChanged);
    CGEvent::set_flags(Some(&event), cg_flags_of(modifiers));
    let event = NSEvent::eventWithCGEvent(&event).expect("AppKit wraps CoreGraphics events");
    let view = view_of(window);
    unsafe { msg_send![&*view, flagsChanged: &*event] }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Other(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Down,
    Up,
    Dragged,
    Moved,
}

fn mouse_type(button: Button, motion: Motion) -> CGEventType {
    match (button, motion) {
        (_, Motion::Moved) => CGEventType::MouseMoved,
        (Button::Left, Motion::Down) => CGEventType::LeftMouseDown,
        (Button::Left, Motion::Up) => CGEventType::LeftMouseUp,
        (Button::Left, Motion::Dragged) => CGEventType::LeftMouseDragged,
        (Button::Right, Motion::Down) => CGEventType::RightMouseDown,
        (Button::Right, Motion::Up) => CGEventType::RightMouseUp,
        (Button::Right, Motion::Dragged) => CGEventType::RightMouseDragged,
        (Button::Other(_), Motion::Down) => CGEventType::OtherMouseDown,
        (Button::Other(_), Motion::Up) => CGEventType::OtherMouseUp,
        (Button::Other(_), Motion::Dragged) => CGEventType::OtherMouseDragged,
    }
}

pub(crate) fn mouse_event(
    window: Window,
    button: Button,
    motion: Motion,
    at: (f64, f64),
    clicks: i64,
) -> Retained<CGEvent> {
    let point = super::screen_point(window, at).expect("tests inject into a live window");
    mouse_event_at(button, motion, point, clicks)
}

pub(crate) fn mouse_event_at(
    button: Button,
    motion: Motion,
    (x, y): (f64, f64),
    clicks: i64,
) -> Retained<CGEvent> {
    let number = match button {
        Button::Left => CGMouseButton::Left,
        Button::Right => CGMouseButton::Right,
        Button::Other(number) => CGMouseButton(number),
    };
    let event =
        CGEvent::new_mouse_event(None, mouse_type(button, motion), CGPoint::new(x, y), number)
            .expect("CoreGraphics builds mouse events");
    CGEvent::set_integer_value_field(Some(&event), CGEventField::MouseEventClickState, clicks);
    event.into()
}

pub fn mouse(window: Window, button: Button, motion: Motion, at: (f64, f64)) {
    let event = mouse_event(window, button, motion, at, 1);
    let event = NSEvent::eventWithCGEvent(&event).expect("AppKit wraps CoreGraphics events");
    let view = view_of(window);
    let event = &*event;
    let view = &*view;
    match (button, motion) {
        (_, Motion::Moved) => unsafe { msg_send![view, mouseMoved: event] },
        (Button::Left, Motion::Down) => unsafe { msg_send![view, mouseDown: event] },
        (Button::Left, Motion::Up) => unsafe { msg_send![view, mouseUp: event] },
        (Button::Left, Motion::Dragged) => unsafe { msg_send![view, mouseDragged: event] },
        (Button::Right, Motion::Down) => unsafe { msg_send![view, rightMouseDown: event] },
        (Button::Right, Motion::Up) => unsafe { msg_send![view, rightMouseUp: event] },
        (Button::Right, Motion::Dragged) => unsafe { msg_send![view, rightMouseDragged: event] },
        (Button::Other(_), Motion::Down) => unsafe { msg_send![view, otherMouseDown: event] },
        (Button::Other(_), Motion::Up) => unsafe { msg_send![view, otherMouseUp: event] },
        (Button::Other(_), Motion::Dragged) => unsafe { msg_send![view, otherMouseDragged: event] },
    }
}

pub fn pointer_entered(window: Window, at: (f64, f64)) {
    let event = mouse_event(window, Button::Left, Motion::Moved, at, 0);
    let event = NSEvent::eventWithCGEvent(&event).expect("AppKit wraps CoreGraphics events");
    let view = view_of(window);
    unsafe { msg_send![&*view, mouseEntered: &*event] }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Wheel {
    Pixels { dx: i32, dy: i32, momentum: bool },
    Lines { dx: i32, dy: i32 },
}

const MOMENTUM_CHANGED: i64 = 2;
const PHASE_CHANGED: i64 = 2;

pub fn scroll(window: Window, at: (f64, f64), wheel: Wheel) {
    let (unit, dx, dy) = match wheel {
        Wheel::Pixels { dx, dy, .. } => (CGScrollEventUnit::Pixel, dx, dy),
        Wheel::Lines { dx, dy } => (CGScrollEventUnit::Line, dx, dy),
    };
    let event = CGEvent::new_scroll_wheel_event2(None, unit, 2, dy, dx, 0)
        .expect("CoreGraphics builds scroll events");
    let (x, y) = super::screen_point(window, at).expect("tests inject into a live window");
    CGEvent::set_location(Some(&event), CGPoint::new(x, y));
    if let Wheel::Pixels { momentum, .. } = wheel {
        match momentum {
            true => CGEvent::set_integer_value_field(
                Some(&event),
                CGEventField::ScrollWheelEventMomentumPhase,
                MOMENTUM_CHANGED,
            ),
            false => CGEvent::set_integer_value_field(
                Some(&event),
                CGEventField::ScrollWheelEventScrollPhase,
                PHASE_CHANGED,
            ),
        }
    }
    let event = NSEvent::eventWithCGEvent(&event).expect("AppKit wraps CoreGraphics events");
    let view = view_of(window);
    unsafe { msg_send![&*view, scrollWheel: &*event] }
}
