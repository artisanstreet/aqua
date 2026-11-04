use std::cell::RefCell;
use std::rc::Rc;

use aqua_core::{InputEvent, Point, PointerButton, PointerEvent, PointerKind, Size};
use aqua_reactive::Runtime;
use aqua_ui::binding::View;
use aqua_ui::element::{Box, Element};
use aqua_ui::events::{dispatch_input, hit_test};
use aqua_ui::input::{DragPayload, DragRequest, EventPhase, InputKind, InputResponse, RoutedInput};
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::style::{px, LayoutStyle, Length, Style};
use aqua_ui::tree::{BoxContent, NodeKind, UiTree};

fn pointer(x: f32, y: f32, buttons: u8) -> PointerEvent {
    PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: Point::new(x, y),
        buttons,
    }
}

fn content(excluded: bool, output: Rc<RefCell<Vec<bool>>>) -> Element {
    let mut preview = Box::new()
        .style(Style::new().width(px(100.0)).height(px(100.0)))
        .build()
        .with_key("preview");
    if excluded {
        preview = preview.exclude_from_hit_testing();
    }
    let done = output.clone();
    Box::new()
        .child(preview)
        .style(Style::new().width(px(200.0)).height(px(200.0)))
        .build()
        .with_key("root")
        .on_input(InputKind::PointerDown, EventPhase::Bubble, |_| {
            InputResponse {
                drag: DragRequest::Start(DragPayload::new(42_u32)),
                ..InputResponse::capture()
            }
        })
        .on_input(InputKind::DragOver, EventPhase::Bubble, |_| InputResponse {
            accept_drop: true,
            ..InputResponse::default()
        })
        .on_input(InputKind::Drop, EventPhase::Bubble, |_| {
            InputResponse::handled()
        })
        .on_input(InputKind::DragEnd, EventPhase::Bubble, move |event| {
            if let RoutedInput::DragEnd { accepted, .. } = &event.input {
                done.borrow_mut().push(*accepted);
            }
            InputResponse::default()
        })
}

#[test]
fn snapshot_tracks_drag_and_excluded_portal_does_not_block_drop() {
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let ended = Rc::new(RefCell::new(Vec::new()));
    let root = view
        .set_content(&mut tree, vec![content(true, ended.clone())])
        .unwrap()[0];
    let preview = tree.get(root).unwrap().children[0];
    let portal = tree.insert(
        Some(preview),
        NodeKind::Box(BoxContent::default()),
        LayoutStyle {
            width: Length::Px(100.0),
            height: Length::Px(100.0),
            ..Default::default()
        },
    );
    assert!(tree.mark_portal(portal));
    tree.compute_layout(Size::new(200.0, 200.0), &NaiveMeasurer)
        .unwrap();
    assert_eq!(hit_test(&tree, Point::new(10.0, 10.0)), Some(root));
    dispatch_input(
        &mut tree,
        &InputEvent::PointerDown(pointer(10.0, 10.0, 1), PointerButton::Primary),
    );
    let snapshot = tree.drag_snapshot().unwrap();
    assert_eq!(snapshot.source, root);
    assert_eq!(snapshot.payload.get::<u32>(), Some(&42));
    assert_eq!(snapshot.cursor_offset, Point::new(10.0, 10.0));
    assert_eq!(tree.pointer_capture(0), Some(root));
    dispatch_input(&mut tree, &InputEvent::PointerMove(pointer(25.0, 25.0, 1)));
    assert_eq!(
        tree.drag_snapshot().unwrap().pointer.position,
        Point::new(25.0, 25.0)
    );
    dispatch_input(
        &mut tree,
        &InputEvent::PointerUp(pointer(25.0, 25.0, 0), PointerButton::Primary),
    );
    assert!(tree.drag_snapshot().is_none());
    assert_eq!(tree.pointer_capture(0), None);
    assert_eq!(*ended.borrow(), vec![true]);
    view.set_content(&mut tree, vec![content(false, ended.clone())])
        .unwrap();
    tree.compute_layout(Size::new(200.0, 200.0), &NaiveMeasurer)
        .unwrap();
    assert_eq!(tree.get(root).unwrap().children[0], preview);
    assert_eq!(hit_test(&tree, Point::new(10.0, 10.0)), Some(portal));
    view.set_content(&mut tree, vec![content(true, ended.clone())])
        .unwrap();
    tree.compute_layout(Size::new(200.0, 200.0), &NaiveMeasurer)
        .unwrap();
    assert_eq!(hit_test(&tree, Point::new(10.0, 10.0)), Some(root));
    dispatch_input(
        &mut tree,
        &InputEvent::PointerDown(pointer(10.0, 10.0, 1), PointerButton::Primary),
    );
    dispatch_input(&mut tree, &InputEvent::PointerCancel { contact: 0 });
    assert!(tree.drag_snapshot().is_none());
    assert_eq!(*ended.borrow(), vec![true, false]);
    dispatch_input(
        &mut tree,
        &InputEvent::PointerDown(pointer(10.0, 10.0, 1), PointerButton::Primary),
    );
    tree.remove(root);
    assert!(tree.drag_snapshot().is_none());
    assert_eq!(*ended.borrow(), vec![true, false, false]);
}
