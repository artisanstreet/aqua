use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use aqua_core::{ClickSettings, InputEvent, Point, PointerButton, PointerEvent, PointerKind};
use aqua_reactive::Runtime;
use aqua_ui::binding::View;
use aqua_ui::element::Box;
use aqua_ui::input::{EventPhase, InputKind, InputResponse};
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::style::{px, Style};
use aqua_ui::tree::UiTree;

#[test]
fn primary_down_exposes_platform_click_count_and_distance_reset() {
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    tree.set_interaction_click_settings(
        ClickSettings::new(Duration::from_secs(30), Point::new(2.0, 2.0)).unwrap(),
    );
    let counts = Rc::new(RefCell::new(Vec::new()));
    let output = counts.clone();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    view.set_content(
        &mut tree,
        vec![Box::new()
            .style(Style::new().width(px(100.0)).height(px(30.0)))
            .build()
            .on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
                output.borrow_mut().push(event.click_count);
                InputResponse::handled()
            })],
    )
    .unwrap();
    tree.drain_commands();
    tree.compute_layout(aqua_core::Size::new(100.0, 30.0), &NaiveMeasurer)
        .unwrap();
    for x in [1.0, 1.0, 20.0] {
        let pointer = PointerEvent {
            contact: 0,
            kind: PointerKind::Mouse,
            position: Point::new(x, 1.0),
            buttons: 1,
        };
        aqua_ui::events::dispatch_input(
            &mut tree,
            &InputEvent::PointerDown(pointer, PointerButton::Primary),
        );
        aqua_ui::events::dispatch_input(
            &mut tree,
            &InputEvent::PointerUp(
                PointerEvent {
                    buttons: 0,
                    ..pointer
                },
                PointerButton::Primary,
            ),
        );
    }
    assert_eq!(*counts.borrow(), vec![Some(1), Some(2), Some(1)]);
    tree.set_interaction_click_settings(
        ClickSettings::new(Duration::from_nanos(1), Point::new(2.0, 2.0)).unwrap(),
    );
    let pointer = PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: Point::new(1.0, 1.0),
        buttons: 1,
    };
    aqua_ui::events::dispatch_input(
        &mut tree,
        &InputEvent::PointerDown(pointer, PointerButton::Primary),
    );
    aqua_ui::events::dispatch_input(
        &mut tree,
        &InputEvent::PointerDown(pointer, PointerButton::Primary),
    );
    assert_eq!(&counts.borrow()[3..], &[Some(1), Some(1)]);
}
