use aqua_core::{Point, Size};
use aqua_reactive::Runtime;
use aqua_ui::binding::View;
use aqua_ui::element::{Box, Element, Scroll};
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::style::{px, Axis, Style};
use aqua_ui::tree::{NodeKind, UiTree};

fn content(preserve: bool, height: f32) -> Element {
    Scroll::new()
        .preserve_offset(preserve)
        .style(
            Style::new()
                .axis(Axis::Column)
                .width(px(200.0))
                .height(px(100.0)),
        )
        .child(
            Box::new()
                .style(Style::new().height(px(height)).flex_shrink(0.0))
                .build(),
        )
        .build()
        .with_key("scroll")
}

#[test]
fn retained_offsets_survive_root_and_child_refreshes_and_still_clamp() {
    for nested in [false, true] {
        let runtime = Runtime::new();
        let mut tree = UiTree::new();
        let mut view = View::new(runtime.create_scope(), tree.command_sink());
        let element = |height| match nested {
            true => Box::new().child(content(true, height)).build(),
            false => content(true, height),
        };
        view.set_content(&mut tree, vec![element(1000.0)]).unwrap();
        view.compute_layout(&mut tree, Size::new(200.0, 100.0), &NaiveMeasurer)
            .unwrap();
        let scroll = tree
            .traverse()
            .into_iter()
            .find(|node| matches!(tree.get(*node).unwrap().kind, NodeKind::Scroll(_)))
            .unwrap();
        tree.scroll_to(scroll, Point::new(0.0, 350.0));
        for _ in 0..3 {
            view.set_content(&mut tree, vec![element(1000.0)]).unwrap();
            view.compute_layout(&mut tree, Size::new(200.0, 100.0), &NaiveMeasurer)
                .unwrap();
            assert_eq!(tree.scroll_offset(scroll), Some(Point::new(0.0, 350.0)));
        }
        view.set_content(&mut tree, vec![element(140.0)]).unwrap();
        view.compute_layout(&mut tree, Size::new(200.0, 100.0), &NaiveMeasurer)
            .unwrap();
        assert_eq!(tree.scroll_offset(scroll), Some(Point::new(0.0, 40.0)));
    }
}

#[test]
fn explicit_offsets_remain_controlled_on_refresh() {
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    view.set_content(&mut tree, vec![content(false, 1000.0)])
        .unwrap();
    view.compute_layout(&mut tree, Size::new(200.0, 100.0), &NaiveMeasurer)
        .unwrap();
    let scroll = tree.root().unwrap();
    tree.scroll_to(scroll, Point::new(0.0, 350.0));
    view.set_content(&mut tree, vec![content(false, 1000.0)])
        .unwrap();
    view.compute_layout(&mut tree, Size::new(200.0, 100.0), &NaiveMeasurer)
        .unwrap();
    assert_eq!(tree.scroll_offset(scroll), Some(Point::ZERO));
}
