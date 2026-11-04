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

fn contact_sample(id: u32, device: Device, flags: u32, pen_flags: u32, at: (f32, f32)) -> Sample {
    Sample {
        id,
        device,
        flags,
        pen_flags,
        position: Point::new(at.0, at.1),
    }
}

#[test]
fn simultaneous_touch_contacts_keep_distinct_ids_and_independent_buttons() {
    let down = |id| contact_sample(id, Device::Touch, FLAG_INCONTACT, 0, (id as f32, 0.0));
    let (first, held_a) = translate(0, &down(1));
    let (second, held_b) = translate(0, &down(2));
    let contacts: Vec<u64> = first
        .iter()
        .chain(second.iter())
        .map(|event| match event {
            InputEvent::PointerDown(e, _) => e.contact,
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(contacts, vec![2, 3]);
    assert_eq!((held_a, held_b), (BUTTON_PRIMARY, BUTTON_PRIMARY));
    let (lifted, held_a) = translate(
        held_a,
        &contact_sample(1, Device::Touch, FLAG_UP, 0, (1.0, 0.0)),
    );
    assert_eq!(held_a, 0);
    assert!(matches!(lifted.as_slice(), [InputEvent::PointerUp(e, _)] if e.contact == 2));
    let (moved, held_b) = translate(held_b, &down(2));
    assert_eq!(held_b, BUTTON_PRIMARY);
    assert!(matches!(moved.as_slice(), [InputEvent::PointerMove(e)] if e.contact == 3));
}

#[test]
fn up_flag_wins_over_a_stale_incontact_flag() {
    let both = contact_sample(0, Device::Touch, FLAG_INCONTACT | FLAG_UP, 0, (0.0, 0.0));
    assert_eq!(button_mask(&both), 0);
    let (events, held) = translate(BUTTON_PRIMARY, &both);
    assert_eq!(held, 0);
    assert!(matches!(
        events.as_slice(),
        [InputEvent::PointerUp(_, PointerButton::Primary)]
    ));
}

#[test]
fn eraser_with_barrel_reports_both_buttons_and_swapping_tips_transitions_in_order() {
    let both = contact_sample(
        1,
        Device::Pen,
        FLAG_INCONTACT,
        PEN_ERASER | PEN_BARREL,
        (0.0, 0.0),
    );
    assert_eq!(button_mask(&both), BUTTON_ERASER | BUTTON_SECONDARY);
    let (_, held) = translate(
        0,
        &contact_sample(1, Device::Pen, FLAG_INCONTACT, 0, (0.0, 0.0)),
    );
    let (events, held) = translate(held, &both);
    assert_eq!(held, BUTTON_ERASER | BUTTON_SECONDARY);
    assert!(matches!(
        events.as_slice(),
        [
            InputEvent::PointerUp(_, PointerButton::Primary),
            InputEvent::PointerDown(_, PointerButton::Secondary),
            InputEvent::PointerDown(_, PointerButton::Extra(5)),
        ]
    ));
}

#[test]
fn lifting_with_buttons_held_releases_every_button_in_order() {
    let held = BUTTON_PRIMARY | BUTTON_SECONDARY;
    let (events, held) = translate(
        held,
        &contact_sample(1, Device::Pen, FLAG_UP, PEN_BARREL, (0.0, 0.0)),
    );
    assert_eq!(held, 0);
    assert!(matches!(
        events.as_slice(),
        [
            InputEvent::PointerUp(a, PointerButton::Primary),
            InputEvent::PointerUp(b, PointerButton::Secondary),
        ] if a.buttons == BUTTON_SECONDARY && b.buttons == 0
    ));
}

#[test]
fn cancel_without_contact_still_reports_the_cancelled_contact() {
    let cancel = contact_sample(9, Device::Pen, FLAG_CANCELED, PEN_BARREL, (0.0, 0.0));
    let (events, held) = translate(0, &cancel);
    assert_eq!(held, 0);
    assert_eq!(events, vec![InputEvent::PointerCancel { contact: 10 }]);
}

#[test]
fn pointer_ids_never_collide_with_the_mouse_contact_zero() {
    assert_eq!(contact_id(0), 1);
    assert_eq!(contact_id(u32::MAX), u64::from(u32::MAX) + 1);
}

#[test]
fn positions_are_carried_through_unchanged() {
    let sample = contact_sample(1, Device::Touch, FLAG_INCONTACT, 0, (-4.5, 1e6));
    let (events, _) = translate(BUTTON_PRIMARY, &sample);
    assert!(matches!(
        events.as_slice(),
        [InputEvent::PointerMove(e)] if e.position == Point::new(-4.5, 1e6)
    ));
}
