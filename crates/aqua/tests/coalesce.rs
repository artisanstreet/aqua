use aqua::coalesce::{merge, Coalescible, Held, Merge};
use aqua_core::{InputEvent, Point, PointerEvent, PointerKind, ScrollDelta};
use aqua_protocol::AquaWindowId;

fn window(raw: u64) -> AquaWindowId {
    AquaWindowId::new(raw).unwrap()
}

fn scroll(window_raw: u64, x: f32, dx: f32, dy: f32) -> Held {
    Held {
        window: window(window_raw),
        input: Coalescible::Scroll {
            at: Point::new(x, 5.0),
            delta: ScrollDelta { x: dx, y: dy },
        },
    }
}

fn motion(window_raw: u64, contact: u64, buttons: u8, x: f32) -> Held {
    Held {
        window: window(window_raw),
        input: Coalescible::Move(PointerEvent {
            contact,
            kind: PointerKind::Mouse,
            position: Point::new(x, 1.0),
            buttons,
        }),
    }
}

#[test]
fn scrolls_in_one_window_sum_their_deltas_and_keep_the_latest_position() {
    let merged = merge(scroll(1, 10.0, 1.0, -16.0), scroll(1, 20.0, 2.0, -8.0));
    assert_eq!(merged, Merge::Merged(scroll(1, 20.0, 3.0, -24.0)));
}

#[test]
fn opposing_scrolls_cancel_rather_than_split() {
    let Merge::Merged(Held {
        input: Coalescible::Scroll { delta, .. },
        ..
    }) = merge(scroll(1, 0.0, 0.0, 16.0), scroll(1, 0.0, 0.0, -16.0))
    else {
        panic!("expected a merge");
    };
    assert_eq!(delta, ScrollDelta { x: 0.0, y: 0.0 });
}

#[test]
fn scrolls_in_different_windows_are_delivered_in_order() {
    let first = scroll(1, 0.0, 0.0, 16.0);
    let second = scroll(2, 0.0, 0.0, 16.0);
    assert_eq!(merge(first, second), Merge::Split(first, second));
}

#[test]
fn consecutive_moves_keep_only_the_latest_position() {
    assert_eq!(
        merge(motion(1, 0, 0, 3.0), motion(1, 0, 0, 9.0)),
        Merge::Merged(motion(1, 0, 0, 9.0))
    );
}

#[test]
fn moves_are_not_merged_across_button_changes_contacts_or_windows() {
    let held = motion(1, 0, 0, 3.0);
    for next in [motion(1, 0, 1, 4.0), motion(1, 7, 0, 4.0), motion(2, 0, 0, 4.0)] {
        assert_eq!(merge(held, next), Merge::Split(held, next));
    }
}

#[test]
fn a_move_and_a_scroll_never_merge_and_keep_their_order() {
    let moved = motion(1, 0, 0, 3.0);
    let scrolled = scroll(1, 3.0, 0.0, 16.0);
    assert_eq!(merge(moved, scrolled), Merge::Split(moved, scrolled));
    assert_eq!(merge(scrolled, moved), Merge::Split(scrolled, moved));
}

#[test]
fn only_moves_and_scrolls_are_classified_for_coalescing() {
    let held = motion(1, 0, 0, 2.0);
    assert_eq!(
        Coalescible::classify(&held.input.into_input()),
        Some(held.input)
    );
    assert_eq!(
        Coalescible::classify(&InputEvent::PointerCancel { contact: 0 }),
        None
    );
}
