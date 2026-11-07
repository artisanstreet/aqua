use aqua_core::Oklch;
use aqua_reactive::Runtime;
use aqua_ui::paint::{FillError, FillGroup};
use aqua_ui::style::LayoutStyle;
use aqua_ui::{BoxContent, NodeKind, NodeSpec, UiTree};

fn mounted(tree: &mut UiTree) -> Vec<aqua_core::NodeId> {
    let mut root = NodeSpec::new(NodeKind::Box(BoxContent::default()), LayoutStyle::default());
    root.children = (0..3)
        .map(|_| NodeSpec::new(NodeKind::Box(BoxContent::default()), LayoutStyle::default()))
        .collect();
    let root = tree.mount_spec(root);
    tree.get(root).unwrap().children.clone()
}

#[test]
fn invalid_batch_never_partially_changes_retained_nodes() {
    let mut tree = UiTree::new();
    let nodes = mounted(&mut tree);
    let group = FillGroup::new(&tree, nodes.clone()).unwrap();
    group.apply(&mut tree, &[Oklch::BLACK; 3]).unwrap();
    let version = tree.content_version();
    let invalid = Oklch {
        l: f32::NAN,
        ..Oklch::WHITE
    };
    assert_eq!(
        group.apply(&mut tree, &[Oklch::WHITE, Oklch::WHITE, invalid]),
        Err(FillError::InvalidColor { index: 2 })
    );
    assert_eq!(tree.content_version(), version);
    assert!(nodes
        .iter()
        .all(|node| tree.paint(*node).unwrap().fill == Some(Oklch::BLACK)));
    assert_eq!(
        group.apply(&mut tree, &[Oklch::WHITE]),
        Err(FillError::Count {
            expected: 3,
            actual: 1
        })
    );
    tree.remove(nodes[2]);
    assert_eq!(
        group.apply(&mut tree, &[Oklch::WHITE; 3]),
        Err(FillError::StaleNode(nodes[2]))
    );
    assert_eq!(tree.paint(nodes[0]).unwrap().fill, Some(Oklch::BLACK));
}

#[test]
fn groups_reject_duplicates_and_other_trees_with_equal_raw_handles() {
    let mut first = UiTree::new();
    let nodes = mounted(&mut first);
    assert!(matches!(
        FillGroup::new(&first, vec![nodes[0], nodes[0]]),
        Err(FillError::DuplicateNode(_))
    ));
    let group = FillGroup::new(&first, nodes.clone()).unwrap();
    let mut second = UiTree::new();
    assert_eq!(mounted(&mut second), nodes);
    assert_eq!(
        group.apply(&mut second, &[Oklch::WHITE; 3]),
        Err(FillError::ForeignTree)
    );
    assert!(second.paint(nodes[0]).unwrap().fill.is_none());
}

#[test]
fn reactive_batches_coalesce_inputs_and_report_errors_without_replacing_nodes() {
    let runtime = Runtime::new();
    let scope = runtime.create_scope();
    let (read, write) = scope.signal(0u32);
    let mut tree = UiTree::new();
    let nodes = mounted(&mut tree);
    let group = FillGroup::new(&tree, nodes.clone()).unwrap();
    let effect = group.bind(&scope, &tree.command_sink(), (read,), |(value,), colors| {
        colors.extend((0..3).map(|index| Oklch::from_srgb8(value as u8, index, 30, 1.0)));
    });
    runtime.flush();
    assert_eq!(tree.drain_commands(), 1);
    for value in [1, 2, 3] {
        write.set(value);
        runtime.flush();
        assert_eq!(tree.drain_commands(), 1);
        for (index, node) in nodes.iter().enumerate() {
            assert_eq!(
                tree.paint(*node).unwrap().fill,
                Some(Oklch::from_srgb8(value as u8, index as u8, 30, 1.0))
            );
        }
    }
    assert_eq!(group.take_error(), None);
    tree.remove(nodes[1]);
    write.set(4);
    runtime.flush();
    tree.drain_commands();
    assert_eq!(group.take_error(), Some(FillError::StaleNode(nodes[1])));
    effect.dispose();
    write.set(5);
    runtime.flush();
    assert_eq!(tree.pending_commands(), 0);
}

#[test]
fn a_batch_stamps_every_changed_node_and_noop_batches_preserve_the_revision() {
    let mut tree = UiTree::new();
    let nodes = mounted(&mut tree);
    let group = FillGroup::new(&tree, nodes.clone()).unwrap();
    group.apply(&mut tree, &[Oklch::BLACK; 3]).unwrap();
    tree.acknowledge_fills();
    let before = tree.content_version();
    group.apply(&mut tree, &[Oklch::WHITE; 3]).unwrap();
    let revision = tree.content_version();
    assert!(revision > before);
    assert_eq!(tree.changed_paint(before), nodes);
    assert!(nodes
        .iter()
        .all(|node| tree.get(*node).unwrap().paint_rev == revision));
    group.apply(&mut tree, &[Oklch::WHITE; 3]).unwrap();
    assert_eq!(tree.content_version(), revision);
    let fills = tree.take_fill_changes().unwrap();
    assert_eq!(fills.len(), nodes.len());
    assert!(fills
        .iter()
        .zip(&nodes)
        .all(|(patch, node)| patch.node == *node && patch.fill == Oklch::WHITE));
}
