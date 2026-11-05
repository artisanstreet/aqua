use aqua_core::Size;
use aqua_reactive::Runtime;
use aqua_ui::binding::View;
use aqua_ui::element::{Box, Key};
use aqua_ui::hash_key;
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::style::{Axis, Style, px};
use aqua_ui::tree::UiTree;

fn leaf(key: &'static str) -> aqua_ui::element::Element {
    Box::new()
        .style(Style::new().width(px(10.0)).height(px(10.0)))
        .build()
        .with_key(key)
}

fn mounted() -> UiTree {
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let content = Box::new()
        .style(
            Style::new()
                .width(px(100.0))
                .height(px(100.0))
                .axis(Axis::Column),
        )
        .child(leaf("once"))
        .child(leaf("twice"))
        .child(leaf("twice"))
        .child(leaf("twice"))
        .build();
    view.set_content(&mut tree, vec![content]).unwrap();
    tree.compute_layout(Size::new(100.0, 100.0), &NaiveMeasurer)
        .unwrap();
    tree
}

#[test]
fn unique_keys_resolve_to_one_node() {
    let tree = mounted();
    let key = hash_key(Key::from("once"));
    let nodes = tree.nodes_with_key(key);
    assert_eq!(nodes.len(), 1);
    assert_eq!(tree.sole_node_with_key(key), Some(nodes[0]));
    assert_eq!(tree.last_node_with_key(key), Some(nodes[0]));
    assert_eq!(tree.find_by_key(key), Some(nodes[0]));
}

#[test]
fn duplicate_keys_keep_paint_order_and_have_no_sole_node() {
    let tree = mounted();
    let key = hash_key(Key::from("twice"));
    let nodes = tree.nodes_with_key(key);
    assert_eq!(nodes.len(), 3);
    assert_eq!(tree.sole_node_with_key(key), None);
    assert_eq!(tree.find_by_key(key), Some(nodes[0]));
    assert_eq!(tree.last_node_with_key(key), Some(nodes[2]));
}

#[test]
fn missing_keys_resolve_to_nothing() {
    let tree = mounted();
    let key = hash_key(Key::from("absent"));
    assert!(tree.nodes_with_key(key).is_empty());
    assert_eq!(tree.sole_node_with_key(key), None);
    assert_eq!(tree.last_node_with_key(key), None);
}
