//! Verification: real `UiTree` keyed reconciliation, focus/edit retention,
//! and control-grade pointer/key dispatch.
//!
//! Black-box tests against the actual public `aqua-ui` tree + events APIs
//! (`UiTree::mount_spec` / `sync_children`, `dispatch_input` /
//! `dispatch_text_input` / `dispatch_key`). Companion to `keyed_rows.rs`
//! (reactive-identity mechanism); these prove the retained tree preserves
//! node identity, editing state, and focus through keyed updates, and that
//! activation follows arm-on-down / fire-on-up-inside semantics.

use std::sync::{Arc, Mutex};

use aqua_core::{
    InputEvent, Key, KeyEvent, Modifiers, NamedKey, Point, PointerButton, PointerEvent,
    PointerKind, Size,
};
use aqua_ui::events::{dispatch_input, dispatch_key, dispatch_text_input};
use aqua_ui::style::{LayoutStyle, Length};
use aqua_ui::tree::{
    ActionEvent, ActionKind, BoxContent, NodeKind, NodeSemantics, NodeSpec, TextBacking,
    TextContent,
};
use aqua_ui::{NaiveMeasurer, UiTree};

fn text_row(key: &str, text: &str) -> NodeSpec {
    NodeSpec::new(
        NodeKind::Text(TextContent::editable(text, false)),
        LayoutStyle::default(),
    )
    .keyed(key)
}

fn mount_rows(keys: &[&str]) -> (UiTree, aqua_core::NodeId, Vec<aqua_core::NodeId>) {
    let mut tree = UiTree::new();
    let root = tree.mount_spec(NodeSpec::new(
        NodeKind::Box(BoxContent::default()),
        LayoutStyle::default(),
    ));
    let ids = tree.sync_children(
        root,
        keys.iter()
            .map(|k| text_row(k, &format!("row-{k}")))
            .collect(),
    );
    tree.compute_layout(Size::new(300.0, 300.0), &NaiveMeasurer)
        .unwrap();
    (tree, root, ids)
}

fn row_text(tree: &UiTree, id: aqua_core::NodeId) -> String {
    match &tree.get(id).unwrap().kind {
        NodeKind::Text(t) => t.text().to_string(),
        _ => panic!("not a text node"),
    }
}

// -- keyed reconciliation -------------------------------------------------------

#[test]
fn keyed_reorder_reuses_nodes_in_place() {
    let (mut tree, root, before) = mount_rows(&["a", "b", "c"]);
    let after = tree.sync_children(
        root,
        vec![text_row("c", "x"), text_row("a", "x"), text_row("b", "x")],
    );
    assert_eq!(after.len(), 3);
    // Same node set, new order: c, a, b.
    assert_eq!(after, vec![before[2], before[0], before[1]]);
    assert_eq!(tree.get(root).unwrap().children, after);
}

#[test]
fn keyed_remove_and_insert_with_disposal() {
    let (mut tree, root, before) = mount_rows(&["a", "b", "c"]);
    let after = tree.sync_children(root, vec![text_row("a", "x"), text_row("d", "new")]);
    assert_eq!(after.len(), 2);
    assert_eq!(after[0], before[0], "survivor keeps identity");
    assert!(tree.get(before[1]).is_none(), "removed key disposed");
    assert!(tree.get(before[2]).is_none(), "removed key disposed");
    assert_ne!(after[1], before[1]);
    assert_ne!(after[1], before[2]);
    assert_eq!(row_text(&tree, after[1]), "new");
}

#[test]
fn editing_state_and_focus_survive_reorder() {
    let (mut tree, root, before) = mount_rows(&["a", "b", "c"]);
    // Type into row b and focus it (fresh editors start with caret at 0,
    // so move to end first — the tree preserves caret, not just text).
    tree.set_focused(Some(before[1]));
    tree.edit_text(before[1], |tc| {
        if let TextBacking::Editable(ed) = &mut tc.backing {
            ed.move_to_end(false);
        }
        tc.insert_at_caret(" edited");
    });
    assert_eq!(row_text(&tree, before[1]), "row-b edited");
    // Reorder around it.
    let after = tree.sync_children(
        root,
        vec![text_row("c", "x"), text_row("b", "x"), text_row("a", "x")],
    );
    assert_eq!(after[1], before[1], "row b keeps its NodeId");
    assert_eq!(row_text(&tree, after[1]), "row-b edited", "edit state kept");
    assert_eq!(tree.focused(), Some(before[1]), "focus follows the node");
}

#[test]
fn kind_change_replaces_instead_of_morphing() {
    let (mut tree, root, before) = mount_rows(&["a"]);
    // Same key, different kind discriminant: must replace, not morph.
    let after = tree.sync_children(
        root,
        vec![
            NodeSpec::new(NodeKind::Box(BoxContent::default()), LayoutStyle::default()).keyed("a"),
        ],
    );
    assert_eq!(after.len(), 1);
    assert_ne!(after[0], before[0], "kind change replaces the node");
    assert!(tree.get(before[0]).is_none());
}

// -- control dispatch: arm on down, fire on up inside -----------------------------

fn pe(contact: u64, x: f32, y: f32) -> PointerEvent {
    PointerEvent {
        contact,
        kind: PointerKind::Mouse,
        position: Point::new(x, y),
        buttons: 1,
    }
}

fn press(contact: u64, x: f32, y: f32) -> InputEvent {
    InputEvent::PointerDown(pe(contact, x, y), PointerButton::Primary)
}

fn release(contact: u64, x: f32, y: f32) -> InputEvent {
    InputEvent::PointerUp(pe(contact, x, y), PointerButton::Primary)
}

fn button_fixture() -> (
    UiTree,
    aqua_core::NodeId,
    Arc<Mutex<Vec<aqua_core::NodeId>>>,
) {
    let mut tree = UiTree::new();
    let bstyle = LayoutStyle {
        width: Length::Px(100.0),
        height: Length::Px(100.0),
        ..Default::default()
    };
    let root = tree.mount_spec(NodeSpec::new(
        NodeKind::Box(BoxContent::default()),
        bstyle.clone(),
    ));
    let button = tree
        .sync_children(
            root,
            vec![
                NodeSpec::new(NodeKind::Box(BoxContent::default()), bstyle).child(NodeSpec::new(
                    NodeKind::Text(TextContent::label("Add")),
                    LayoutStyle::default(),
                )),
            ],
        )
        .pop()
        .unwrap();
    tree.compute_layout(Size::new(300.0, 300.0), &NaiveMeasurer)
        .unwrap();
    tree.set_focusable(button, true);
    let fired = Arc::new(Mutex::new(Vec::new()));
    let fired2 = fired.clone();
    tree.set_action_callback(
        button,
        ActionKind::Activate,
        Box::new(move |e: ActionEvent| {
            fired2.lock().unwrap().push(e.node);
        }),
    );
    (tree, button, fired)
}

#[test]
fn label_down_arms_without_firing_up_inside_fires_once() {
    let (mut tree, button, fired) = button_fixture();
    // Down on the label (10,10): arms the button, fires nothing.
    assert!(dispatch_input(&mut tree, &press(1, 10.0, 10.0)));
    assert!(fired.lock().unwrap().is_empty());
    // Release inside: exactly one activation, on the button (bubbled).
    assert!(dispatch_input(&mut tree, &release(1, 10.0, 10.0)));
    assert_eq!(*fired.lock().unwrap(), vec![button]);
    // A second release without an arm fires nothing.
    assert!(!dispatch_input(&mut tree, &release(1, 10.0, 10.0)));
    assert_eq!(fired.lock().unwrap().len(), 1);
}

#[test]
fn secondary_button_never_arms_or_fires() {
    let (mut tree, _button, fired) = button_fixture();
    let down = InputEvent::PointerDown(pe(2, 10.0, 10.0), PointerButton::Secondary);
    assert!(!dispatch_input(&mut tree, &down));
    let up = InputEvent::PointerUp(pe(2, 10.0, 10.0), PointerButton::Secondary);
    assert!(!dispatch_input(&mut tree, &up));
    assert!(fired.lock().unwrap().is_empty());
}

#[test]
fn cancel_and_release_outside_do_not_fire() {
    let (mut tree, _button, fired) = button_fixture();
    // Arm, then cancel: no activation.
    assert!(dispatch_input(&mut tree, &press(3, 10.0, 10.0)));
    assert!(dispatch_input(
        &mut tree,
        &InputEvent::PointerCancel { contact: 3 }
    ));
    assert!(fired.lock().unwrap().is_empty());
    // Arm, then release far outside the button: drag-out, no activation.
    assert!(dispatch_input(&mut tree, &press(4, 10.0, 10.0)));
    assert!(dispatch_input(&mut tree, &release(4, 250.0, 250.0)));
    assert!(fired.lock().unwrap().is_empty());
}

#[test]
fn disabled_button_blocks_gesture_entirely() {
    let (mut tree, button, fired) = button_fixture();
    tree.set_semantics(
        button,
        NodeSemantics {
            disabled: true,
            ..Default::default()
        },
    );
    assert!(!dispatch_input(&mut tree, &press(5, 10.0, 10.0)));
    assert!(!dispatch_input(&mut tree, &release(5, 10.0, 10.0)));
    assert!(fired.lock().unwrap().is_empty());
}

#[test]
fn removed_mid_press_swallows_release() {
    let (mut tree, button, fired) = button_fixture();
    assert!(dispatch_input(&mut tree, &press(6, 10.0, 10.0)));
    assert!(tree.remove(button));
    // Removal clears the arm: release consumes nothing and never fires.
    assert!(!dispatch_input(&mut tree, &release(6, 10.0, 10.0)));
    assert!(fired.lock().unwrap().is_empty());
}

// -- text input: multi-codepoint once, shortcuts don't insert ---------------------

#[test]
fn text_input_inserts_multicodepoint_once_and_fires_changed() {
    let (mut tree, _root, ids) = mount_rows(&["a"]);
    tree.set_focused(Some(ids[0]));
    let changed = Arc::new(Mutex::new(Vec::new()));
    let changed2 = changed.clone();
    tree.set_action_callback(
        ids[0],
        ActionKind::TextChanged,
        Box::new(move |e: ActionEvent| {
            changed2.lock().unwrap().push(e.text.clone().unwrap());
        }),
    );
    // Multi-codepoint single input event: one insertion, one notification.
    // (Caret starts at 0 in a fresh editor; move to end to append.)
    tree.edit_text(ids[0], |tc| {
        if let TextBacking::Editable(ed) = &mut tc.backing {
            ed.move_to_end(false);
        }
    });
    assert!(dispatch_text_input(&mut tree, " héllo 👨‍👩‍👧"));
    assert_eq!(row_text(&tree, ids[0]), "row-a héllo 👨‍👩‍👧");
    assert_eq!(changed.lock().unwrap().len(), 1);
}

#[test]
fn keydown_char_shortcut_does_not_insert() {
    let (mut tree, _root, ids) = mount_rows(&["a"]);
    tree.set_focused(Some(ids[0]));
    let before = row_text(&tree, ids[0]);
    // A bare character KeyDown is NOT text input (that arrives as TextInput);
    // shortcuts must not corrupt the buffer.
    let ev = KeyEvent {
        key: Key::Char('x'),
        modifiers: Modifiers::CONTROL,
        repeat: false,
    };
    assert!(!dispatch_key(&mut tree, &ev));
    assert_eq!(row_text(&tree, ids[0]), before);
}

#[test]
fn tab_traps_focus_forward_and_back() {
    let (mut tree, _root, ids) = mount_rows(&["a", "b"]);
    tree.set_focusable(ids[0], true);
    tree.set_focusable(ids[1], true);
    tree.set_focused(Some(ids[0]));
    let tab = KeyEvent {
        key: Key::Named(NamedKey::Tab),
        modifiers: Modifiers::NONE,
        repeat: false,
    };
    assert!(dispatch_key(&mut tree, &tab));
    assert_eq!(tree.focused(), Some(ids[1]));
    let shift_tab = KeyEvent {
        key: Key::Named(NamedKey::Tab),
        modifiers: Modifiers::SHIFT,
        repeat: false,
    };
    assert!(dispatch_key(&mut tree, &shift_tab));
    assert_eq!(tree.focused(), Some(ids[0]));
}
