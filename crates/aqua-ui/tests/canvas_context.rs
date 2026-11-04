use std::cell::{Cell, RefCell};
use std::rc::Rc;

use aqua_core::{Point, Rect, Size};
use aqua_reactive::Runtime;
use aqua_ui::binding::View;
use aqua_ui::element::{Box, Canvas, Element, Scroll};
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::style::{px, Axis, Style};
use aqua_ui::tree::UiTree;

fn canvas(observed: Rc<RefCell<Vec<(Rect, Rect)>>>, delivered: Rc<Cell<usize>>) -> Element {
    Canvas::new()
        .style(
            Style::new()
                .width(px(60.0))
                .height(px(40.0))
                .flex_shrink(0.0),
        )
        .on_paint_context(move |context| {
            observed.borrow_mut().push((context.bounds, context.clip));
            if !context.bounds.intersect(context.clip).is_empty() {
                let delivered = delivered.clone();
                context.defer(move || delivered.set(delivered.get() + 1));
            }
            Vec::new()
        })
        .build()
        .with_key("canvas")
}

#[test]
fn clipped_canvas_visibility_tracks_scroll_and_defers_delivery() {
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let observed = Rc::new(RefCell::new(Vec::new()));
    let delivered = Rc::new(Cell::new(0));
    let content = Scroll::new()
        .style(
            Style::new()
                .width(px(60.0))
                .height(px(60.0))
                .axis(Axis::Column),
        )
        .child(
            Box::new()
                .style(
                    Style::new()
                        .width(px(60.0))
                        .height(px(100.0))
                        .flex_shrink(0.0),
                )
                .build(),
        )
        .child(canvas(observed.clone(), delivered.clone()))
        .build();
    let root = view.set_content(&mut tree, vec![content]).unwrap()[0];
    tree.compute_layout(Size::new(100.0, 100.0), &NaiveMeasurer)
        .unwrap();
    tree.emit_scene();
    assert_eq!(delivered.get(), 0);
    assert_eq!(tree.run_deferred_canvas_actions(), 0);
    let (bounds, clip) = observed.borrow().last().copied().unwrap();
    assert!(bounds.intersect(clip).is_empty());
    assert!(tree.scroll_to(root, Point::new(0.0, 80.0)));
    tree.emit_scene();
    assert_eq!(delivered.get(), 0);
    assert_eq!(tree.run_deferred_canvas_actions(), 1);
    assert_eq!(delivered.get(), 1);
    let (bounds, clip) = observed.borrow().last().copied().unwrap();
    assert!(!bounds.intersect(clip).is_empty());
    assert_eq!(clip, Rect::from_edges(0.0, 0.0, 60.0, 60.0));
}

#[test]
fn deferred_actions_are_fenced_by_canvas_binding_and_node_lifetime() {
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let observed = Rc::new(RefCell::new(Vec::new()));
    let delivered = Rc::new(Cell::new(0));
    let id = view
        .set_content(&mut tree, vec![canvas(observed.clone(), delivered.clone())])
        .unwrap()[0];
    tree.compute_layout(Size::new(100.0, 100.0), &NaiveMeasurer)
        .unwrap();
    tree.emit_scene();
    view.set_content(&mut tree, vec![canvas(observed.clone(), delivered.clone())])
        .unwrap();
    assert_eq!(tree.run_deferred_canvas_actions(), 0);
    tree.compute_layout(Size::new(100.0, 100.0), &NaiveMeasurer)
        .unwrap();
    tree.emit_scene();
    tree.remove(id);
    assert_eq!(tree.run_deferred_canvas_actions(), 0);
    assert_eq!(delivered.get(), 0);
}

#[test]
fn portal_canvas_uses_window_clip_instead_of_ancestor_clip() {
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let observed = Rc::new(RefCell::new(Vec::new()));
    let delivered = Rc::new(Cell::new(0));
    let root = view
        .set_content(
            &mut tree,
            vec![Box::new()
                .clip(true)
                .style(Style::new().width(px(20.0)).height(px(20.0)))
                .child(canvas(observed.clone(), delivered))
                .build()],
        )
        .unwrap()[0];
    let child = tree.get(root).unwrap().children[0];
    tree.mark_portal(child);
    tree.compute_layout(Size::new(80.0, 80.0), &NaiveMeasurer)
        .unwrap();
    tree.emit_scene();
    let (_, clip) = observed.borrow().last().copied().unwrap();
    assert_eq!(clip, Rect::from_edges(0.0, 0.0, 80.0, 80.0));
}
