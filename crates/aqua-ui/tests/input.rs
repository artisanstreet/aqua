use std::cell::{Cell, RefCell};
use std::rc::Rc;

use aqua_core::{
    CursorIcon, InputEvent, Key, KeyEvent, Modifiers, NodeId, Point, PointerButton, PointerEvent,
    PointerKind, Size,
};
use aqua_ui::events::dispatch_input;
use aqua_ui::input::{EventPhase, FocusRequest, InputKind, InputResponse, Shortcut};
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::style::{LayoutStyle, Length};
use aqua_ui::tree::{ActionKind, BoxContent, NodeKind, NodeSpec, UiTree};

fn fixture() -> (UiTree, NodeId, NodeId) {
    let mut tree = UiTree::new();
    let root = tree.mount_spec(
        NodeSpec::new(
            NodeKind::Box(BoxContent::default()),
            LayoutStyle {
                width: Length::Px(300.0),
                height: Length::Px(300.0),
                ..Default::default()
            },
        )
        .child(NodeSpec::new(
            NodeKind::Box(BoxContent::default()),
            LayoutStyle {
                width: Length::Px(100.0),
                height: Length::Px(100.0),
                ..Default::default()
            },
        )),
    );
    let child = tree.get(root).unwrap().children[0];
    tree.compute_layout(Size::new(300.0, 300.0), &NaiveMeasurer)
        .unwrap();
    (tree, root, child)
}

fn pointer(x: f32, y: f32, buttons: u8) -> PointerEvent {
    PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: Point::new(x, y),
        buttons,
    }
}

#[test]
fn input_visits_capture_then_bubble_and_prevents_default_activation() {
    let (mut tree, root, child) = fixture();
    let seen = Rc::new(RefCell::new(Vec::new()));
    for (node, phase) in [
        (root, EventPhase::Capture),
        (child, EventPhase::Capture),
        (child, EventPhase::Bubble),
        (root, EventPhase::Bubble),
    ] {
        let seen = seen.clone();
        tree.listen_input(
            node,
            InputKind::PointerDown,
            phase,
            Rc::new(move |event| {
                seen.borrow_mut().push((event.current_target, event.phase));
                assert_eq!(event.target, child);
                InputResponse {
                    prevent_default: true,
                    ..Default::default()
                }
            }),
        );
    }
    let activated = Rc::new(Cell::new(0));
    let output = activated.clone();
    tree.set_action_callback(
        child,
        ActionKind::Activate,
        Box::new(move |_| output.set(output.get() + 1)),
    );
    dispatch_input(
        &mut tree,
        &InputEvent::PointerDown(pointer(20.0, 20.0, 1), PointerButton::Primary),
    );
    dispatch_input(
        &mut tree,
        &InputEvent::PointerUp(pointer(20.0, 20.0, 0), PointerButton::Primary),
    );
    assert_eq!(
        *seen.borrow(),
        vec![
            (root, EventPhase::Capture),
            (child, EventPhase::Capture),
            (child, EventPhase::Bubble),
            (root, EventPhase::Bubble)
        ]
    );
    assert_eq!(activated.get(), 0);
}

#[test]
fn stopping_propagation_does_not_silently_prevent_default() {
    let (mut tree, root, child) = fixture();
    tree.listen_input(
        root,
        InputKind::PointerDown,
        EventPhase::Capture,
        Rc::new(|_| InputResponse {
            stop_propagation: true,
            ..Default::default()
        }),
    );
    let bubbled = Rc::new(Cell::new(false));
    let output = bubbled.clone();
    tree.listen_input(
        child,
        InputKind::PointerDown,
        EventPhase::Bubble,
        Rc::new(move |_| {
            output.set(true);
            InputResponse::default()
        }),
    );
    let activated = Rc::new(Cell::new(false));
    let output = activated.clone();
    tree.set_action_callback(
        child,
        ActionKind::Activate,
        Box::new(move |_| output.set(true)),
    );
    dispatch_input(
        &mut tree,
        &InputEvent::PointerDown(pointer(20.0, 20.0, 1), PointerButton::Primary),
    );
    dispatch_input(
        &mut tree,
        &InputEvent::PointerUp(pointer(20.0, 20.0, 0), PointerButton::Primary),
    );
    assert!(!bubbled.get());
    assert!(activated.get());
}

#[test]
fn capture_routes_outside_bounds_and_cancels_on_focus_loss_or_removal() {
    let (mut tree, _, child) = fixture();
    tree.set_focusable(child, true);
    tree.listen_input(
        child,
        InputKind::PointerDown,
        EventPhase::Bubble,
        Rc::new(|_| InputResponse {
            focus: FocusRequest::Focus,
            ..InputResponse::capture()
        }),
    );
    let moves = Rc::new(RefCell::new(Vec::new()));
    let output = moves.clone();
    tree.listen_input(
        child,
        InputKind::PointerMove,
        EventPhase::Bubble,
        Rc::new(move |event| {
            output.borrow_mut().push(event.local_position.unwrap());
            InputResponse::handled()
        }),
    );
    let cancels = Rc::new(Cell::new(0));
    let output = cancels.clone();
    tree.listen_input(
        child,
        InputKind::PointerCancel,
        EventPhase::Bubble,
        Rc::new(move |_| {
            output.set(output.get() + 1);
            InputResponse::default()
        }),
    );
    dispatch_input(
        &mut tree,
        &InputEvent::PointerDown(pointer(20.0, 20.0, 1), PointerButton::Primary),
    );
    assert_eq!(tree.pointer_capture(0), Some(child));
    assert_eq!(tree.focused(), Some(child));
    dispatch_input(
        &mut tree,
        &InputEvent::PointerMove(pointer(400.0, 350.0, 1)),
    );
    assert_eq!(*moves.borrow(), vec![Point::new(400.0, 350.0)]);
    dispatch_input(&mut tree, &InputEvent::Focused(false));
    assert_eq!(cancels.get(), 1);
    assert_eq!(tree.pointer_capture(0), None);
    dispatch_input(
        &mut tree,
        &InputEvent::PointerDown(pointer(20.0, 20.0, 1), PointerButton::Primary),
    );
    tree.remove(child);
    assert_eq!(cancels.get(), 2);
    assert_eq!(tree.pointer_capture(0), None);
}

#[test]
fn hover_is_once_per_node_and_cursor_inherits_from_hovered_path() {
    let (mut tree, root, child) = fixture();
    let enters = Rc::new(RefCell::new(Vec::new()));
    let leaves = Rc::new(RefCell::new(Vec::new()));
    for node in [root, child] {
        let output = enters.clone();
        tree.listen_input(
            node,
            InputKind::PointerEnter,
            EventPhase::Bubble,
            Rc::new(move |event| {
                output.borrow_mut().push(event.current_target);
                InputResponse::default()
            }),
        );
        let output = leaves.clone();
        tree.listen_input(
            node,
            InputKind::PointerLeave,
            EventPhase::Bubble,
            Rc::new(move |event| {
                output.borrow_mut().push(event.current_target);
                InputResponse::default()
            }),
        );
    }
    tree.set_cursor(root, CursorIcon::Crosshair);
    dispatch_input(&mut tree, &InputEvent::PointerMove(pointer(20.0, 20.0, 0)));
    assert_eq!(*enters.borrow(), vec![root, child]);
    assert_eq!(tree.cursor(), CursorIcon::Crosshair);
    tree.set_cursor(child, CursorIcon::Grab);
    assert_eq!(tree.cursor(), CursorIcon::Grab);
    dispatch_input(
        &mut tree,
        &InputEvent::PointerMove(pointer(200.0, 200.0, 0)),
    );
    assert_eq!(*leaves.borrow(), vec![child]);
    assert_eq!(tree.cursor(), CursorIcon::Crosshair);
    dispatch_input(&mut tree, &InputEvent::Focused(false));
    assert_eq!(*leaves.borrow(), vec![child, root]);
    assert_eq!(tree.cursor(), CursorIcon::Default);
}

#[test]
fn shortcuts_resolve_nearest_context_and_exact_modifiers() {
    let (mut tree, root, child) = fixture();
    tree.set_focusable(child, true);
    tree.set_focused(Some(child));
    let calls = Rc::new(RefCell::new(Vec::new()));
    for (node, action) in [(root, 1), (child, 2)] {
        let output = calls.clone();
        tree.set_action_callback(
            node,
            ActionKind::Custom(action),
            Box::new(move |_| output.borrow_mut().push(action)),
        );
        tree.bind_shortcut(
            node,
            Shortcut {
                key: Key::Char('k'),
                modifiers: Modifiers::CONTROL,
                action: ActionKind::Custom(action),
                repeat: false,
            },
        );
    }
    for (modifiers, repeat) in [
        (Modifiers::CONTROL, false),
        (Modifiers::CONTROL.union(Modifiers::ALT), false),
        (Modifiers::CONTROL, true),
    ] {
        dispatch_input(
            &mut tree,
            &InputEvent::KeyDown(KeyEvent {
                key: Key::Char('k'),
                modifiers,
                repeat,
            }),
        );
    }
    assert_eq!(*calls.borrow(), vec![2]);
}

#[test]
fn typed_drag_payload_is_accepted_only_by_compatible_drop_targets() {
    use aqua_ui::input::{DragPayload, DragRequest, RoutedInput};
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct ThreadId(u64);
    let (mut tree, root, source) = fixture();
    tree.listen_input(
        source,
        InputKind::PointerDown,
        EventPhase::Bubble,
        Rc::new(|_| InputResponse {
            drag: DragRequest::Start(DragPayload::new(ThreadId(9))),
            ..InputResponse::handled()
        }),
    );
    tree.listen_input(
        root,
        InputKind::DragOver,
        EventPhase::Bubble,
        Rc::new(|event| {
            let RoutedInput::DragOver { payload, .. } = &event.input else {
                panic!()
            };
            InputResponse {
                accept_drop: payload.get::<ThreadId>().is_some(),
                ..Default::default()
            }
        }),
    );
    let dropped = Rc::new(RefCell::new(None));
    let output = dropped.clone();
    tree.listen_input(
        root,
        InputKind::Drop,
        EventPhase::Bubble,
        Rc::new(move |event| {
            let RoutedInput::Drop { payload, .. } = &event.input else {
                panic!()
            };
            *output.borrow_mut() = payload.get::<ThreadId>().cloned();
            InputResponse::handled()
        }),
    );
    let ended = Rc::new(Cell::new(None));
    let output = ended.clone();
    tree.listen_input(
        source,
        InputKind::DragEnd,
        EventPhase::Bubble,
        Rc::new(move |event| {
            let RoutedInput::DragEnd { accepted, .. } = &event.input else {
                panic!()
            };
            output.set(Some(*accepted));
            InputResponse::default()
        }),
    );
    dispatch_input(
        &mut tree,
        &InputEvent::PointerDown(pointer(20.0, 20.0, 1), PointerButton::Primary),
    );
    dispatch_input(
        &mut tree,
        &InputEvent::PointerMove(pointer(200.0, 200.0, 1)),
    );
    dispatch_input(
        &mut tree,
        &InputEvent::PointerUp(pointer(200.0, 200.0, 0), PointerButton::Primary),
    );
    assert_eq!(*dropped.borrow(), Some(ThreadId(9)));
    assert_eq!(ended.get(), Some(true));
    assert_eq!(tree.pointer_capture(0), None);
}

#[test]
fn external_files_keep_grouped_paths_and_cancel_without_focus_fallback() {
    use aqua_ui::input::{route_external_file_drag, ExternalFileDragEvent, RoutedInput};
    use std::path::PathBuf;

    let (mut tree, _, child) = fixture();
    let events = Rc::new(RefCell::new(Vec::new()));
    for kind in [
        InputKind::DragEnter,
        InputKind::DragOver,
        InputKind::DragLeave,
        InputKind::FilesDropped,
    ] {
        let events = events.clone();
        tree.listen_input(
            child,
            kind,
            EventPhase::Bubble,
            Rc::new(move |event| {
                match &event.input {
                    RoutedInput::DragEnter { .. } => {
                        events.borrow_mut().push(("enter", Vec::new()))
                    }
                    RoutedInput::DragOver { .. } => events.borrow_mut().push(("over", Vec::new())),
                    RoutedInput::DragLeave { .. } => {
                        events.borrow_mut().push(("leave", Vec::new()))
                    }
                    RoutedInput::FilesDropped { paths, .. } => {
                        events.borrow_mut().push(("drop", paths.to_vec()))
                    }
                    _ => unreachable!(),
                }
                InputResponse::default()
            }),
        );
    }
    let paths = vec![PathBuf::from("one.png"), PathBuf::from("two.txt")];
    let route = |tree: &mut UiTree, event| route_external_file_drag(tree, event, |_, _| true);
    route(
        &mut tree,
        ExternalFileDragEvent::Enter {
            paths: paths.clone(),
            position: Point::new(20.0, 20.0),
        },
    );
    route(
        &mut tree,
        ExternalFileDragEvent::Move {
            position: Point::new(350.0, 350.0),
        },
    );
    route(
        &mut tree,
        ExternalFileDragEvent::Submit {
            position: Point::new(350.0, 350.0),
        },
    );
    assert_eq!(
        &*events.borrow(),
        &[("enter", vec![]), ("over", vec![]), ("leave", vec![])]
    );
    events.borrow_mut().clear();
    route(
        &mut tree,
        ExternalFileDragEvent::Enter {
            paths: paths.clone(),
            position: Point::new(20.0, 20.0),
        },
    );
    route(
        &mut tree,
        ExternalFileDragEvent::Submit {
            position: Point::new(20.0, 20.0),
        },
    );
    assert_eq!(
        &*events.borrow(),
        &[
            ("enter", vec![]),
            ("over", vec![]),
            ("drop", paths),
            ("leave", vec![])
        ]
    );
    events.borrow_mut().clear();
    route(
        &mut tree,
        ExternalFileDragEvent::Enter {
            paths: vec![PathBuf::from("x")],
            position: Point::new(20.0, 20.0),
        },
    );
    route(&mut tree, ExternalFileDragEvent::Exit);
    route(
        &mut tree,
        ExternalFileDragEvent::Submit {
            position: Point::new(20.0, 20.0),
        },
    );
    assert_eq!(
        &*events.borrow(),
        &[("enter", vec![]), ("over", vec![]), ("leave", vec![])]
    );
}

#[test]
fn declarative_listener_refresh_keeps_direct_registrations_and_cancels_capture() {
    let (mut tree, _, child) = fixture();
    let cancelled = Rc::new(Cell::new(0));
    let output = cancelled.clone();
    tree.listen_input_bound(
        child,
        InputKind::PointerDown,
        EventPhase::Bubble,
        Rc::new(|_| InputResponse::capture()),
    );
    tree.listen_input(
        child,
        InputKind::PointerCancel,
        EventPhase::Bubble,
        Rc::new(move |_| {
            output.set(output.get() + 1);
            InputResponse::default()
        }),
    );
    dispatch_input(
        &mut tree,
        &InputEvent::PointerDown(pointer(20.0, 20.0, 1), PointerButton::Primary),
    );
    tree.clear_input_bindings(child);
    assert_eq!(cancelled.get(), 1);
    assert_eq!(tree.pointer_capture(0), None);
    dispatch_input(
        &mut tree,
        &InputEvent::PointerDown(pointer(20.0, 20.0, 1), PointerButton::Primary),
    );
    assert_eq!(tree.pointer_capture(0), None);
}

#[test]
fn typed_drag_cancels_when_source_is_disabled_or_removed() {
    use aqua_ui::input::{DragPayload, DragRequest, RoutedInput};
    let (mut tree, _, source) = fixture();
    tree.listen_input(
        source,
        InputKind::PointerDown,
        EventPhase::Bubble,
        Rc::new(|_| InputResponse {
            drag: DragRequest::Start(DragPayload::new(123_u64)),
            ..InputResponse::capture()
        }),
    );
    let ended = Rc::new(RefCell::new(Vec::new()));
    let output = ended.clone();
    tree.listen_input(
        source,
        InputKind::DragEnd,
        EventPhase::Bubble,
        Rc::new(move |event| {
            let RoutedInput::DragEnd { accepted, .. } = &event.input else {
                panic!()
            };
            output.borrow_mut().push(*accepted);
            InputResponse::default()
        }),
    );
    dispatch_input(
        &mut tree,
        &InputEvent::PointerDown(pointer(20.0, 20.0, 1), PointerButton::Primary),
    );
    assert!(tree.input_is_captured());
    tree.set_semantics(
        source,
        aqua_ui::tree::NodeSemantics {
            disabled: true,
            ..Default::default()
        },
    );
    dispatch_input(
        &mut tree,
        &InputEvent::PointerMove(pointer(200.0, 200.0, 1)),
    );
    assert_eq!(*ended.borrow(), vec![false]);
    assert!(!tree.input_is_captured());
    tree.set_semantics(source, Default::default());
    dispatch_input(
        &mut tree,
        &InputEvent::PointerDown(pointer(20.0, 20.0, 1), PointerButton::Primary),
    );
    tree.remove(source);
    assert_eq!(*ended.borrow(), vec![false, false]);
    assert!(!tree.input_is_captured());
}

#[test]
fn transformed_targets_report_local_coordinates_and_portals_escape_clipping() {
    use aqua_core::Transform2D;
    let (mut tree, root, child) = fixture();
    let mut paint = tree.paint(root).unwrap();
    paint.clip = true;
    paint.transform = Some(Transform2D::scale(2.0, 2.0).then(Transform2D::translate(50.0, 20.0)));
    tree.set_paint(root, paint);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let output = seen.clone();
    tree.listen_input(
        child,
        InputKind::PointerDown,
        EventPhase::Bubble,
        Rc::new(move |event| {
            output.borrow_mut().push(event.local_position.unwrap());
            InputResponse::handled()
        }),
    );
    dispatch_input(
        &mut tree,
        &InputEvent::PointerDown(pointer(90.0, 80.0, 1), PointerButton::Primary),
    );
    assert_eq!(*seen.borrow(), vec![Point::new(20.0, 30.0)]);
    tree.mark_portal(child);
    dispatch_input(
        &mut tree,
        &InputEvent::PointerDown(pointer(10.0, 10.0, 1), PointerButton::Primary),
    );
    assert_eq!(
        *seen.borrow(),
        vec![Point::new(20.0, 30.0), Point::new(10.0, 10.0)]
    );
}

#[test]
fn transactional_text_commands_respect_disabled_composing_and_routed_domains() {
    use aqua_text::Selection;
    use aqua_ui::input::{route_input, TextEdit};
    use aqua_ui::tree::{TextContent, UiCommand};
    let mut tree = UiTree::new();
    let root = tree.mount_spec(
        NodeSpec::new(NodeKind::Box(BoxContent::default()), LayoutStyle::default())
            .child(NodeSpec::new(
                NodeKind::Text(TextContent::editable("background", false)),
                LayoutStyle::default(),
            ))
            .child(NodeSpec::new(
                NodeKind::Text(TextContent::editable("modal", false)),
                LayoutStyle::default(),
            )),
    );
    let background = tree.get(root).unwrap().children[0];
    let modal = tree.get(root).unwrap().children[1];
    tree.set_focused(Some(modal));
    tree.listen_input(
        modal,
        InputKind::KeyDown,
        EventPhase::Bubble,
        Rc::new(move |_| InputResponse {
            commands: vec![UiCommand::EditText {
                node: background,
                revision: None,
                edit: TextEdit::Insert("blocked".into()),
            }],
            ..InputResponse::handled()
        }),
    );
    route_input(
        &mut tree,
        &InputEvent::KeyDown(KeyEvent {
            key: Key::Char('a'),
            modifiers: Modifiers::NONE,
            repeat: false,
        }),
        |_, node| node == modal,
    );
    tree.drain_commands();
    assert_eq!(tree.editor_snapshot(background).unwrap().text, "background");
    tree.set_semantics(
        modal,
        aqua_ui::tree::NodeSemantics {
            disabled: true,
            ..Default::default()
        },
    );
    assert!(!tree.apply_text_edit(modal, None, TextEdit::Insert("blocked".into())));
    tree.set_semantics(modal, Default::default());
    tree.edit_editor(modal, |editor| {
        editor.set_selection(Selection::new(0, 5));
        editor.set_composition("文", 3..3);
    });
    assert!(!tree.apply_text_edit(modal, None, TextEdit::Undo));
    assert!(!tree.apply_text_edit(modal, None, TextEdit::SetSelection(Selection::caret_at(0))));
    tree.edit_editor(modal, |editor| {
        editor.cancel_composition();
    });
    assert_eq!(tree.editor_snapshot(modal).unwrap().text, "modal");
}

#[test]
fn nested_scroll_chains_at_edges_and_portals_keep_wheel_inside_their_scope() {
    use aqua_core::ScrollDelta;
    use aqua_ui::events::dispatch_scroll;
    use aqua_ui::style::Axis;
    use aqua_ui::tree::ScrollContent;
    let style = |height| LayoutStyle {
        width: Length::Px(100.0),
        height: Length::Px(height),
        axis: Axis::Column,
        flex_shrink: 0.0,
        ..Default::default()
    };
    let mut tree = UiTree::new();
    let root = tree.mount_spec(
        NodeSpec::new(NodeKind::Scroll(ScrollContent::default()), style(100.0))
            .child(
                NodeSpec::new(NodeKind::Scroll(ScrollContent::default()), style(50.0)).child(
                    NodeSpec::new(NodeKind::Box(BoxContent::default()), style(150.0)),
                ),
            )
            .child(NodeSpec::new(
                NodeKind::Box(BoxContent::default()),
                style(250.0),
            )),
    );
    let inner = tree.get(root).unwrap().children[0];
    tree.compute_layout(Size::new(100.0, 100.0), &NaiveMeasurer)
        .unwrap();
    let wheel = ScrollDelta { x: 0.0, y: 200.0 };
    let at = Point::new(10.0, 10.0);
    assert!(dispatch_scroll(&mut tree, at, wheel));
    assert_eq!(tree.scroll_offset(inner).unwrap().y, 100.0);
    assert_eq!(tree.scroll_offset(root).unwrap().y, 0.0);
    assert!(dispatch_scroll(
        &mut tree,
        at,
        ScrollDelta { x: 0.0, y: 20.0 }
    ));
    assert_eq!(tree.scroll_offset(root).unwrap().y, 20.0);
    tree.scroll_to(root, Point::ZERO);
    tree.mark_portal(inner);
    assert!(!dispatch_scroll(&mut tree, at, wheel));
    assert_eq!(tree.scroll_offset(root).unwrap().y, 0.0);
}
