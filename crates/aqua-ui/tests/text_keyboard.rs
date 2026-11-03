use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey};
use aqua_text::Selection;
use aqua_ui::events::dispatch_input;
use aqua_ui::style::LayoutStyle;
use aqua_ui::tree::{NodeKind, NodeSpec, TextBacking, TextContent, UiTree};

fn press(tree: &mut UiTree, key: Key, modifiers: Modifiers) -> bool {
    dispatch_input(
        tree,
        &InputEvent::KeyDown(KeyEvent {
            key,
            modifiers,
            repeat: false,
        }),
    )
}

fn editor(tree: &UiTree, id: aqua_core::NodeId) -> &aqua_text::TextEditor {
    let NodeKind::Text(content) = &tree.get(id).unwrap().kind else {
        panic!()
    };
    let TextBacking::Editable(editor) = &content.backing else {
        panic!()
    };
    editor
}

#[test]
fn ctrl_paragraph_keys_preserve_column_and_shift_anchor() {
    let mut tree = UiTree::new();
    let id = tree.mount_spec(NodeSpec::new(
        NodeKind::Text(TextContent::editable("abcd\nx\nwxyz", true)),
        LayoutStyle::default(),
    ));
    assert!(tree.set_focused(Some(id)));
    tree.edit_text(id, |content| {
        let TextBacking::Editable(editor) = &mut content.backing else {
            panic!()
        };
        editor.set_caret(3);
    });
    let modifiers = Modifiers::CONTROL.union(Modifiers::SHIFT);
    assert!(press(&mut tree, Key::Named(NamedKey::Down), modifiers));
    assert_eq!(editor(&tree, id).selection(), Selection::new(3, 6));
    assert!(press(&mut tree, Key::Named(NamedKey::Down), modifiers));
    assert_eq!(editor(&tree, id).selection(), Selection::new(3, 10));
    assert!(press(
        &mut tree,
        Key::Named(NamedKey::Up),
        Modifiers::CONTROL
    ));
    assert_eq!(editor(&tree, id).selection(), Selection::caret_at(6));
}

#[test]
fn ctrl_word_deletion_and_undo_redo_route_to_editor() {
    let mut tree = UiTree::new();
    let id = tree.mount_spec(NodeSpec::new(
        NodeKind::Text(TextContent::editable("hello world", true)),
        LayoutStyle::default(),
    ));
    assert!(tree.set_focused(Some(id)));
    assert!(press(
        &mut tree,
        Key::Named(NamedKey::Right),
        Modifiers::CONTROL
    ));
    assert_eq!(editor(&tree, id).selection().caret, 6);
    assert!(press(
        &mut tree,
        Key::Named(NamedKey::Delete),
        Modifiers::CONTROL
    ));
    assert_eq!(editor(&tree, id).text(), "hello ");
    assert!(press(&mut tree, Key::Char('z'), Modifiers::CONTROL));
    assert_eq!(editor(&tree, id).text(), "hello world");
    assert!(press(&mut tree, Key::Char('y'), Modifiers::CONTROL));
    assert_eq!(editor(&tree, id).text(), "hello ");
}
