use aqua_core::Role;
use aqua_reactive::Runtime;
use aqua_ui::binding::View;
use aqua_ui::element::Box;
use aqua_ui::tree::UiTree;

#[test]
fn keyed_tree_item_refresh_updates_and_removes_level_and_expansion() {
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let item = |level| {
        Box::new()
            .build()
            .with_key("item")
            .role(Role::TreeItem)
            .aria_label("src")
            .aria_level(level)
            .aria_expanded(true)
    };
    let root = |child| {
        Box::new()
            .child(child)
            .build()
            .role(Role::Tree)
            .aria_label("Changed files")
    };
    view.set_content(&mut tree, vec![root(item(2))]).unwrap();
    tree.drain_commands();
    let nodes = tree.emit_semantics();
    let row = nodes
        .iter()
        .find(|node| node.role == Role::TreeItem)
        .unwrap();
    let id = row.node;
    assert_eq!(row.level, Some(2));
    assert_eq!(row.expanded, Some(true));
    view.set_content(&mut tree, vec![root(item(3))]).unwrap();
    tree.drain_commands();
    let nodes = tree.emit_semantics();
    let row = nodes.iter().find(|node| node.node == id).unwrap();
    assert_eq!(row.level, Some(3));
    assert_eq!(row.expanded, Some(true));
    view.set_content(
        &mut tree,
        vec![root(
            Box::new().build().with_key("item").role(Role::TreeItem),
        )],
    )
    .unwrap();
    tree.drain_commands();
    let nodes = tree.emit_semantics();
    let row = nodes.iter().find(|node| node.node == id).unwrap();
    assert_eq!(row.level, None);
    assert_eq!(row.expanded, None);
}
