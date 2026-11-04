use std::cell::RefCell;
use std::rc::Rc;

use aqua_core::{
    InputEvent, Key, KeyEvent, Modifiers, NamedKey, Point, PointerButton, PointerEvent,
    PointerKind, Size,
};
use aqua_ui::events::dispatch_input;
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::style::{LayoutStyle, Length};
use aqua_ui::tree::{ActionKind, ActivationSource, BoxContent, NodeKind, NodeSpec, UiTree};

#[test]
fn activation_preserves_pointer_and_keyboard_sources() {
    let mut tree = UiTree::new();
    let node = tree.mount_spec(NodeSpec::new(
        NodeKind::Box(BoxContent::default()),
        LayoutStyle {
            width: Length::Px(100.0),
            height: Length::Px(100.0),
            ..Default::default()
        },
    ));
    tree.set_focusable(node, true);
    tree.compute_layout(Size::new(100.0, 100.0), &NaiveMeasurer)
        .unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = events.clone();
    tree.set_action_callback(
        node,
        ActionKind::Activate,
        Box::new(move |event| observed.borrow_mut().push(event.source)),
    );
    let down = PointerEvent {
        contact: 5,
        kind: PointerKind::Mouse,
        position: Point::new(20.0, 30.0),
        buttons: 1,
    };
    let up = PointerEvent { buttons: 0, ..down };
    dispatch_input(&mut tree, &InputEvent::ModifiersChanged(Modifiers::SHIFT));
    dispatch_input(
        &mut tree,
        &InputEvent::PointerDown(down, PointerButton::Primary),
    );
    dispatch_input(
        &mut tree,
        &InputEvent::PointerUp(up, PointerButton::Primary),
    );
    let key = KeyEvent {
        key: Key::Named(NamedKey::Enter),
        modifiers: Modifiers::NONE,
        repeat: false,
    };
    dispatch_input(&mut tree, &InputEvent::KeyDown(key));
    assert_eq!(
        *events.borrow(),
        vec![
            Some(ActivationSource::Pointer {
                down,
                up,
                button: PointerButton::Primary,
                modifiers: Modifiers::SHIFT,
                click_count: Some(1)
            }),
            Some(ActivationSource::Keyboard(key)),
        ]
    );
}

#[test]
fn touch_taps_count_only_completed_stationary_gestures() {
    let mut tree = UiTree::new();
    let node = tree.mount_spec(NodeSpec::new(
        NodeKind::Box(BoxContent::default()),
        LayoutStyle {
            width: Length::Px(100.0),
            height: Length::Px(100.0),
            ..Default::default()
        },
    ));
    tree.compute_layout(Size::new(100.0, 100.0), &NaiveMeasurer)
        .unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = events.clone();
    tree.set_action_callback(
        node,
        ActionKind::Activate,
        Box::new(move |event| observed.borrow_mut().push(event.source)),
    );
    let down = PointerEvent {
        contact: 7,
        kind: PointerKind::Touch,
        position: Point::new(20.0, 20.0),
        buttons: 1,
    };
    let up = PointerEvent { buttons: 0, ..down };
    for _ in 0..2 {
        dispatch_input(
            &mut tree,
            &InputEvent::PointerDown(down, PointerButton::Primary),
        );
        dispatch_input(
            &mut tree,
            &InputEvent::PointerUp(up, PointerButton::Primary),
        );
    }
    dispatch_input(
        &mut tree,
        &InputEvent::PointerDown(down, PointerButton::Primary),
    );
    dispatch_input(
        &mut tree,
        &InputEvent::PointerMove(PointerEvent {
            position: Point::new(29.0, 20.0),
            ..down
        }),
    );
    dispatch_input(
        &mut tree,
        &InputEvent::PointerUp(up, PointerButton::Primary),
    );
    assert_eq!(
        *events.borrow(),
        vec![
            Some(ActivationSource::Touch {
                position: up.position,
                tap_count: 1,
                long_press: false
            }),
            Some(ActivationSource::Touch {
                position: up.position,
                tap_count: 2,
                long_press: false
            }),
        ]
    );
}

#[test]
fn gpui_keyboard_activation_waits_for_release_and_cancels_after_focus_changes() {
    use aqua_reactive::Runtime;
    use aqua_ui::binding::View;
    use aqua_ui::element::ui::{Box as UiBox, Button};

    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = events.clone();
    let first = Button::new()
        .on_activate_event(move |event| observed.borrow_mut().push(event.source))
        .build()
        .gpui_keyboard_activation();
    let second = Button::new().build().gpui_keyboard_activation();
    let root = view
        .set_content(
            &mut tree,
            vec![UiBox::new().child(first).child(second).build()],
        )
        .unwrap()[0];
    tree.drain_commands();
    let children = tree.get(root).unwrap().children.clone();
    let key = KeyEvent {
        key: Key::Named(NamedKey::Enter),
        modifiers: Modifiers::NONE,
        repeat: false,
    };
    tree.set_focused(Some(children[0]));
    dispatch_input(&mut tree, &InputEvent::KeyDown(key));
    assert!(events.borrow().is_empty());
    dispatch_input(&mut tree, &InputEvent::KeyUp(key));
    assert_eq!(
        *events.borrow(),
        vec![Some(ActivationSource::Keyboard(key))]
    );
    dispatch_input(&mut tree, &InputEvent::KeyDown(key));
    tree.set_focused(Some(children[1]));
    tree.set_focused(Some(children[0]));
    dispatch_input(&mut tree, &InputEvent::KeyUp(key));
    let modified = KeyEvent {
        modifiers: Modifiers::SHIFT,
        ..key
    };
    dispatch_input(&mut tree, &InputEvent::KeyDown(modified));
    dispatch_input(&mut tree, &InputEvent::KeyUp(modified));
    dispatch_input(&mut tree, &InputEvent::KeyDown(key));
    let other = KeyEvent {
        key: Key::Char('x'),
        ..key
    };
    dispatch_input(&mut tree, &InputEvent::KeyDown(other));
    dispatch_input(&mut tree, &InputEvent::KeyUp(key));
    assert_eq!(events.borrow().len(), 1);
}
