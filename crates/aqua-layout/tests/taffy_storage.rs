use taffy::geometry::Size;
use taffy::style::{AvailableSpace, Dimension, Display, FlexDirection, Style};
use taffy::TaffyTree;

fn fixed(width: f32, height: f32) -> Style {
    Style {
        size: Size {
            width: Dimension::length(width),
            height: Dimension::length(height),
        },
        ..Style::default()
    }
}

fn lay_out(tree: &mut TaffyTree<()>, root: taffy::NodeId) {
    tree.compute_layout(
        root,
        Size {
            width: AvailableSpace::Definite(1000.0),
            height: AvailableSpace::MaxContent,
        },
    )
    .unwrap();
}

#[test]
fn layout_spans_many_storage_chunks() {
    let mut tree: TaffyTree<()> = TaffyTree::new();
    let children: Vec<_> = (0..200)
        .map(|index| tree.new_leaf(fixed(4.0, 10.0 + index as f32)).unwrap())
        .collect();
    let root = tree
        .new_with_children(
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                ..Style::default()
            },
            &children,
        )
        .unwrap();
    lay_out(&mut tree, root);
    assert_eq!(tree.total_node_count(), 201);
    assert_eq!(tree.layout(children[199]).unwrap().location.x, 4.0 * 199.0);
    assert_eq!(tree.layout(root).unwrap().size.height, 209.0);
    assert_eq!(tree.parent(children[150]), Some(root));
}

#[test]
fn removed_nodes_never_alias_their_replacements() {
    let mut tree: TaffyTree<u32> = TaffyTree::new();
    let first = tree.new_leaf_with_context(fixed(10.0, 10.0), 7).unwrap();
    tree.remove(first).unwrap();
    let second = tree.new_leaf_with_context(fixed(20.0, 20.0), 9).unwrap();
    assert_ne!(first, second);
    assert_eq!(tree.get_node_context(second), Some(&9));
    assert_eq!(tree.get_node_context(first), None);
    assert_eq!(
        tree.style(second).unwrap().size.width,
        Dimension::length(20.0)
    );
    assert_eq!(tree.total_node_count(), 1);
}

#[test]
fn detaching_and_reattaching_children_keeps_layout_consistent() {
    let mut tree: TaffyTree<()> = TaffyTree::new();
    let a = tree.new_leaf(fixed(30.0, 10.0)).unwrap();
    let b = tree.new_leaf(fixed(40.0, 10.0)).unwrap();
    let root = tree
        .new_with_children(
            Style {
                display: Display::Flex,
                ..Style::default()
            },
            &[a, b],
        )
        .unwrap();
    lay_out(&mut tree, root);
    assert_eq!(tree.layout(b).unwrap().location.x, 30.0);
    tree.remove_child(root, a).unwrap();
    assert_eq!(tree.parent(a), None);
    lay_out(&mut tree, root);
    assert_eq!(tree.layout(b).unwrap().location.x, 0.0);
    tree.add_child(root, a).unwrap();
    lay_out(&mut tree, root);
    assert_eq!(tree.layout(a).unwrap().location.x, 40.0);
}
