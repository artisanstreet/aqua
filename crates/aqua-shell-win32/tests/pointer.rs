use aqua_core::{InputEvent, Point, PointerButton, PointerKind};
use aqua_shell_win32::pointer::*;

fn sample(device: Device, flags: u32, pen_flags: u32) -> Sample {
    Sample {
        id: 3,
        device,
        flags,
        pen_flags,
        position: Point::new(10.0, 20.0),
    }
}

fn buttons(event: &InputEvent) -> u8 {
    match event {
        InputEvent::PointerDown(e, _)
        | InputEvent::PointerUp(e, _)
        | InputEvent::PointerMove(e) => e.buttons,
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn touch_contact_lifecycle() {
    let down = sample(Device::Touch, FLAG_INCONTACT | 0x10000, 0);
    let (events, held) = translate(0, &down);
    assert_eq!(held, BUTTON_PRIMARY);
    assert!(matches!(
        events.as_slice(),
        [InputEvent::PointerDown(e, PointerButton::Primary)]
            if e.contact == 4 && e.kind == PointerKind::Touch && e.buttons == BUTTON_PRIMARY
    ));
    let moved = sample(Device::Touch, FLAG_INCONTACT | 0x20000, 0);
    let (events, held) = translate(held, &moved);
    assert!(matches!(events.as_slice(), [InputEvent::PointerMove(e)] if e.buttons == 1));
    let up = sample(Device::Touch, FLAG_UP, 0);
    let (events, held) = translate(held, &up);
    assert_eq!(held, 0);
    assert!(matches!(
        events.as_slice(),
        [InputEvent::PointerUp(e, PointerButton::Primary)] if e.buttons == 0
    ));
}

#[test]
fn cancel_clears_buttons_and_names_the_contact() {
    let cancel = sample(Device::Touch, FLAG_CANCELED | FLAG_INCONTACT, 0);
    let (events, held) = translate(BUTTON_PRIMARY, &cancel);
    assert_eq!(held, 0);
    assert_eq!(events, vec![InputEvent::PointerCancel { contact: 4 }]);
}

#[test]
fn pen_hover_reports_moves_without_buttons() {
    let hover = sample(Device::Pen, 0x2, 0);
    let (events, held) = translate(0, &hover);
    assert_eq!(held, 0);
    assert!(matches!(
        events.as_slice(),
        [InputEvent::PointerMove(e)] if e.kind == PointerKind::Pen && e.buttons == 0
    ));
}

#[test]
fn pen_barrel_press_and_release_mid_contact_emit_button_transitions() {
    let contact = FLAG_INCONTACT | 0x20000;
    let (_, held) = translate(0, &sample(Device::Pen, contact | 0x10000, 0));
    let (events, held) = translate(held, &sample(Device::Pen, contact, PEN_BARREL));
    assert_eq!(held, BUTTON_PRIMARY | BUTTON_SECONDARY);
    assert!(matches!(
        events.as_slice(),
        [InputEvent::PointerDown(_, PointerButton::Secondary)]
    ));
    assert_eq!(buttons(&events[0]), BUTTON_PRIMARY | BUTTON_SECONDARY);
    let (events, held) = translate(held, &sample(Device::Pen, contact, 0));
    assert_eq!(held, BUTTON_PRIMARY);
    assert!(matches!(
        events.as_slice(),
        [InputEvent::PointerUp(_, PointerButton::Secondary)]
    ));
    assert_eq!(buttons(&events[0]), BUTTON_PRIMARY);
}

#[test]
fn eraser_tip_is_its_own_button_and_barrel_is_ignored_on_touch() {
    let eraser = sample(Device::Pen, FLAG_INCONTACT, PEN_ERASER);
    assert_eq!(button_mask(&eraser), BUTTON_ERASER);
    let touch = sample(Device::Touch, FLAG_INCONTACT, PEN_BARREL);
    assert_eq!(button_mask(&touch), BUTTON_PRIMARY);
}

#[test]
fn only_touch_and_pen_pointer_types_are_handled() {
    assert_eq!(Device::from_pointer_type(2), Some(Device::Touch));
    assert_eq!(Device::from_pointer_type(3), Some(Device::Pen));
    assert_eq!(Device::from_pointer_type(4), None);
    assert_eq!(Device::from_pointer_type(5), None);
}
