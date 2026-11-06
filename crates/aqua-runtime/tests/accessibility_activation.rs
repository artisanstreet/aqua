use std::cell::Cell;
use std::rc::Rc;

use aqua_core::{
    InputEvent, Key, KeyEvent, Modifiers, NamedKey, NodeId, Point, PointerButton, PointerEvent,
    PointerKind, Role,
};
use aqua_protocol::{A11yAction, A11yActionData, A11yActionRequest};

mod support;
use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box, Button};
use aqua_ui::style::{px, Style};

fn press(runtime: &mut Runtime, node: NodeId) {
    support::access(
        runtime,
        A11yActionRequest {
            node,
            action: A11yAction::Press,
            data: A11yActionData::None,
        },
    );
}

#[test]
fn accessibility_activation_is_independent_and_follows_keyed_lifecycle() {
    let calls = Rc::new(Cell::new(0));
    let first = calls.clone();
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            Box::new()
                .style(Style::new().width(px(100.0)).height(px(30.0)))
                .build()
                .with_key("row")
                .role(Role::TreeItem)
                .focusable(true)
                .on_accessibility_activate(move || first.set(first.get() + 1))
        })
        .unwrap();
    runtime.set_viewport(200.0, 100.0, 1.0);
    runtime.build_frame().unwrap();
    let node = runtime.tree().root().unwrap();
    press(&mut runtime, node);
    assert_eq!(calls.get(), 1);
    let key = KeyEvent {
        key: Key::Named(NamedKey::Enter),
        modifiers: Modifiers::NONE,
        repeat: false,
    };
    runtime.handle_input(&InputEvent::KeyDown(key));
    runtime.handle_input(&InputEvent::KeyUp(key));
    let pointer = PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: Point::new(5.0, 5.0),
        buttons: 1,
    };
    runtime.handle_input(&InputEvent::PointerDown(pointer, PointerButton::Primary));
    runtime.handle_input(&InputEvent::PointerUp(
        PointerEvent {
            buttons: 0,
            ..pointer
        },
        PointerButton::Primary,
    ));
    assert_eq!(calls.get(), 1);
    let next = calls.clone();
    runtime
        .refresh(vec![Box::new()
            .build()
            .with_key("row")
            .role(Role::TreeItem)
            .focusable(true)
            .on_accessibility_activate(move || next.set(next.get() + 10))])
        .unwrap();
    assert_eq!(runtime.tree().root(), Some(node));
    press(&mut runtime, node);
    assert_eq!(calls.get(), 11);
    runtime
        .refresh(vec![Box::new()
            .build()
            .with_key("row")
            .role(Role::TreeItem)
            .focusable(true)])
        .unwrap();
    press(&mut runtime, node);
    assert_eq!(calls.get(), 11);
    runtime
        .refresh(vec![Box::new().build().with_key("other")])
        .unwrap();
    press(&mut runtime, node);
    assert_eq!(calls.get(), 11);
}

#[test]
fn accessibility_press_falls_back_to_existing_activation() {
    let calls = Rc::new(Cell::new(0));
    let output = calls.clone();
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            Button::new()
                .on_activate(move || output.set(output.get() + 1))
                .build()
        })
        .unwrap();
    let node = runtime.tree().root().unwrap();
    press(&mut runtime, node);
    assert_eq!(calls.get(), 1);
    let generic = calls.clone();
    let specific = calls.clone();
    runtime
        .refresh(vec![Button::new()
            .on_activate(move || generic.set(generic.get() + 1))
            .build()
            .on_accessibility_activate(move || {
                specific.set(specific.get() + 10)
            })])
        .unwrap();
    press(&mut runtime, node);
    assert_eq!(calls.get(), 11);
}

#[test]
fn expanding_a_disclosure_button_presses_it_only_when_the_state_changes() {
    let calls = Rc::new(Cell::new(0));
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    let disclosure = |expanded: bool| {
        let output = calls.clone();
        Button::new()
            .on_activate(move || output.set(output.get() + 1))
            .build()
            .with_key("menu")
            .aria_expanded(expanded)
    };
    runtime.mount(|_| disclosure(false)).unwrap();
    let node = runtime.tree().root().unwrap();
    let request = |action| A11yActionRequest {
        node,
        action,
        data: A11yActionData::None,
    };
    support::access(&mut runtime, request(A11yAction::Collapse));
    assert_eq!(calls.get(), 0);
    support::access(&mut runtime, request(A11yAction::Expand));
    assert_eq!(calls.get(), 1);
    runtime.refresh(vec![disclosure(true)]).unwrap();
    support::access(&mut runtime, request(A11yAction::Expand));
    assert_eq!(calls.get(), 1);
    support::access(&mut runtime, request(A11yAction::Collapse));
    assert_eq!(calls.get(), 2);
}
