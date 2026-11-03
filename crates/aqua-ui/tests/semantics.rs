use aqua_core::Role;
use aqua_ui::{
    style::LayoutStyle,
    tree::{NodeKind, NodeSemantics, NodeSpec, TextBacking, TextContent, UiTree},
};

#[test]
fn placeholder_names_the_field_without_entering_its_editable_value() {
    let mut tree = UiTree::new();
    let mut text = TextContent::editable("", false);
    text.placeholder = "Search".to_owned();
    let node = tree.mount_spec(NodeSpec::new(NodeKind::Text(text), LayoutStyle::default()));
    tree.set_semantics(
        node,
        NodeSemantics {
            role: Role::TextField,
            ..NodeSemantics::default()
        },
    );
    let semantics = || {
        tree.emit_semantics()
            .into_iter()
            .find(|value| value.node == node)
            .unwrap()
    };
    assert_eq!(semantics().value_text.as_deref(), Some(""));
    tree.edit_text(node, |text| text.insert_at_caret("query"));
    let typed = tree
        .emit_semantics()
        .into_iter()
        .find(|value| value.node == node)
        .unwrap();
    assert_eq!(typed.value_text.as_deref(), Some("query"));
    assert_eq!(typed.label, "Search");
    tree.edit_text(node, |text| {
        if let TextBacking::Editable(editor) = &mut text.backing {
            editor.undo();
        }
    });
    let undone = tree
        .emit_semantics()
        .into_iter()
        .find(|value| value.node == node)
        .unwrap();
    assert_eq!(undone.value_text.as_deref(), Some(""));
    assert_eq!(undone.label, "Search");
}
