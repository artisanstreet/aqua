//! Input semantics: hit-testing, focus management, scrolling, and editing
//! dispatch over the retained tree.
//!
//! These are free functions over [`UiTree`], not methods, so the dispatch
//! layer stays separate from storage. All functions are synchronous and
//! headless (pure [`aqua_core::event`] vocabulary in, tree mutations out).
//! Platform adapters translate native events into core events and call these;
//! the facade wires them to windows.
//!
//! No function here retains state between calls and none reenters the tree:
//! callbacks fired during dispatch receive [`ActionEvent`] data only.

use crate::tree::{ActionEvent, ActionKind, NodeKind, UiTree};
use aqua_core::{
    ImeEvent, InputEvent, Key, KeyEvent, Modifiers, NamedKey, NodeId, Point, PointerButton, Rect,
    ScrollDelta,
};

/// Topmost node at window-local logical `at`, respecting paint order
/// (last child = topmost), ancestor clips, and scroll offsets.
/// Returns `None` outside every node.
#[must_use]
pub fn hit_test(tree: &UiTree, at: Point) -> Option<NodeId> {
    let root = tree.root()?;
    hit_node(tree, root, at, None, Point::ZERO)
}

fn hit_node(
    tree: &UiTree,
    id: NodeId,
    at: Point,
    clip: Option<Rect>,
    offset: Point,
) -> Option<NodeId> {
    let rect = tree.rect(id)?.translate(offset.x, offset.y);
    let rec = tree.get(id)?;
    // Scroll children live in content space: shift by -offset for descent.
    let child_offset = match rec.kind {
        NodeKind::Scroll(ref s) => Point::new(offset.x - s.offset.x, offset.y - s.offset.y),
        _ => offset,
    };
    // Clips accumulate down the path (box clip or scroll viewport).
    let mut child_clip = clip;
    if let NodeKind::Box(ref b) = rec.kind {
        if b.paint.clip {
            child_clip = Some(match child_clip {
                Some(c) => c.intersect(rect),
                None => rect,
            });
        }
    }
    if matches!(rec.kind, NodeKind::Scroll(_)) {
        child_clip = Some(match child_clip {
            Some(c) => c.intersect(rect),
            None => rect,
        });
    }
    // Topmost child first.
    for &c in rec.children.iter().rev() {
        if let Some(hit) = hit_node(tree, c, at, child_clip, child_offset) {
            return Some(hit);
        }
    }
    if rect.contains(at) && child_clip.is_none_or(|c| c.contains(at)) {
        Some(id)
    } else {
        None
    }
}

/// Focusable, enabled nodes in traversal (paint) order. Disabled nodes are
/// skipped by Tab navigation.
#[must_use]
pub fn focusable_nodes(tree: &UiTree) -> Vec<NodeId> {
    tree.traverse()
        .into_iter()
        .filter(|id| {
            tree.get(*id)
                .is_some_and(|r| r.focusable && !r.semantics.disabled)
        })
        .collect()
}

/// Move focus to the next focusable node, wrapping around. Returns the newly
/// focused node (`None` when the tree has no focusable nodes).
pub fn focus_next(tree: &mut UiTree) -> Option<NodeId> {
    let order = focusable_nodes(tree);
    if order.is_empty() {
        tree.set_focused(None);
        return None;
    }
    let next = match tree.focused() {
        None => order[0],
        Some(cur) => {
            let pos = order.iter().position(|n| *n == cur).unwrap_or(usize::MAX);
            order[(pos + 1) % order.len()]
        }
    };
    tree.set_focused(Some(next));
    Some(next)
}

/// Move focus to the previous focusable node, wrapping around.
pub fn focus_prev(tree: &mut UiTree) -> Option<NodeId> {
    let order = focusable_nodes(tree);
    if order.is_empty() {
        tree.set_focused(None);
        return None;
    }
    let prev = match tree.focused() {
        None => order[order.len() - 1],
        Some(cur) => {
            let pos = order.iter().position(|n| *n == cur).unwrap_or(0);
            order[(pos + order.len() - 1) % order.len()]
        }
    };
    tree.set_focused(Some(prev));
    Some(prev)
}

/// Route a scroll gesture: hit-test, then scroll the innermost scrollable
/// ancestor. Returns `true` when a scrollable consumed the gesture.
pub fn dispatch_scroll(tree: &mut UiTree, at: Point, delta: ScrollDelta) -> bool {
    let mut target = hit_test(tree, at);
    while let Some(id) = target {
        let is_scroll = matches!(tree.get(id).map(|r| &r.kind), Some(NodeKind::Scroll(_)));
        if is_scroll {
            let cur = match tree.get(id) {
                Some(rec) => match rec.kind {
                    NodeKind::Scroll(ref s) => s.offset,
                    _ => unreachable!(),
                },
                None => return false,
            };
            return tree.scroll_to(id, Point::new(cur.x + delta.x, cur.y + delta.y));
        }
        target = tree.get(id).and_then(|r| r.parent);
    }
    false
}

/// Insert committed text into the focused editable node (platform `TextInput`
/// routing). Normalizes `\r\n`/`\r` to `\n`; the editor enforces single-line
/// policy. Ordinary text travels ONLY through this path — `KeyDown` never
/// inserts (it carries navigation/shortcut identity), and IME text travels
/// via `ImeEvent::Commit`, so there is exactly one insertion per commit.
/// Fires [`ActionKind::TextChanged`]. Returns `true` when handled.
pub fn dispatch_text_input(tree: &mut UiTree, text: &str) -> bool {
    let normalized: String = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    if normalized.is_empty() {
        return false;
    }
    let Some(id) = tree.focused() else {
        return false;
    };
    if tree.is_disabled(id) {
        return false;
    }
    let inserted = tree.edit_text(id, |tc| tc.insert_at_caret(&normalized));
    match inserted {
        Some(true) => {
            let committed = tree.get(id).and_then(|r| match r.kind {
                NodeKind::Text(ref t) => Some(t.text().to_string()),
                _ => None,
            });
            tree.fire_action(ActionEvent {
                node: id,
                kind: ActionKind::TextChanged,
                text: committed,
                focused: None,
            });
            true
        }
        _ => false,
    }
}

/// Route IME lifecycle events into the focused editable node's editor.
/// Preedit state lives in the editor; candidate-window placement reads
/// `layout.caret(editor.ime_caret())` on the text-provider side (platform
/// owner). Returns `true` when handled.
pub fn dispatch_ime(tree: &mut UiTree, event: &ImeEvent) -> bool {
    let Some(id) = tree.focused() else {
        return false;
    };
    let is_editable = tree
        .get(id)
        .map(|r| matches!(r.kind, NodeKind::Text(ref t) if t.is_editable()))
        .unwrap_or(false);
    if !is_editable {
        return false;
    }
    match event {
        ImeEvent::Start => true,
        ImeEvent::Update { marked, cursor } => {
            let range = cursor.unwrap_or((marked.len(), marked.len()));
            let marked = marked.clone();
            tree.edit_text(id, |tc| {
                if let crate::tree::TextBacking::Editable(ref mut ed) = tc.backing {
                    // Clamp the platform byte range into the preedit.
                    let end = marked.len();
                    let range = range.0.min(end)..range.1.min(end).max(range.0.min(end));
                    ed.set_composition(&marked, range);
                }
            });
            tree.sync_text_leaf(id);
            true
        }
        ImeEvent::Commit { text } => {
            // commit_composition() folds the preedit (set by Update) into the
            // model in one undo step: no separate insert, otherwise the text
            // lands twice. Platforms that send Commit without Preedit carry
            // the text here while the editor is not composing: insert it.
            let text = text.clone();
            let committed_done = tree.edit_text(id, |tc| {
                if let crate::tree::TextBacking::Editable(ref mut ed) = tc.backing {
                    if ed.is_composing() {
                        ed.commit_composition();
                    } else if !text.is_empty() {
                        ed.insert(&text);
                    }
                }
            });
            if committed_done.is_some() {
                tree.sync_text_leaf(id);
                let committed = tree.get(id).and_then(|r| match r.kind {
                    NodeKind::Text(ref t) => Some(t.text().to_string()),
                    _ => None,
                });
                tree.fire_action(ActionEvent {
                    node: id,
                    kind: ActionKind::TextChanged,
                    text: committed,
                    focused: None,
                });
                true
            } else {
                false
            }
        }
        ImeEvent::Cancel => {
            tree.edit_text(id, |tc| {
                if let crate::tree::TextBacking::Editable(ref mut ed) = tc.backing {
                    ed.cancel_composition();
                }
            });
            tree.sync_text_leaf(id);
            true
        }
    }
}

/// Route a key event: Tab/Shift-Tab moves focus; editing keys act on the
/// focused editable node; anything else is unhandled (shortcuts belong to
/// the host/facade). Returns `true` when consumed.
pub fn dispatch_key(tree: &mut UiTree, event: &KeyEvent) -> bool {
    if event.repeat && matches!(event.key, Key::Named(NamedKey::Tab)) {
        return false;
    }
    match event.key {
        Key::Named(NamedKey::Tab) => {
            if event.modifiers.contains(Modifiers::SHIFT) {
                focus_prev(tree);
            } else {
                focus_next(tree);
            }
            true
        }
        _ => {
            let Some(id) = tree.focused() else {
                return false;
            };
            let is_editable = tree
                .get(id)
                .map(|r| matches!(r.kind, NodeKind::Text(ref t) if t.is_editable()))
                .unwrap_or(false);
            if !is_editable {
                // Non-editable focus: Enter/Space raise activation for hosts
                // that register `ActionKind::Activate` on the node. Disabled
                // nodes never activate.
                if tree.is_disabled(id) {
                    return false;
                }
                if matches!(event.key, Key::Named(NamedKey::Enter) | Key::Char(' '))
                    && !event.repeat
                {
                    tree.fire_action(ActionEvent {
                        node: id,
                        kind: ActionKind::Activate,
                        text: None,
                        focused: None,
                    });
                    return true;
                }
                return false;
            }
            let changed = match event.key {
                Key::Named(NamedKey::Enter) => {
                    let single = tree
                        .get(id)
                        .is_some_and(|r| matches!(&r.kind, NodeKind::Text(t) if !t.multiline()));
                    let composing = tree.get(id).is_some_and(|r| matches!(&r.kind,
                        NodeKind::Text(t) if matches!(&t.backing, crate::tree::TextBacking::Editable(e) if e.is_composing())));
                    if single && !event.repeat && !composing {
                        tree.fire_action(ActionEvent {
                            node: id,
                            kind: ActionKind::Submit,
                            text: None,
                            focused: None,
                        });
                        return true;
                    }
                    if !single && !composing {
                        tree.edit_text(id, |tc| tc.insert_at_caret("\n"))
                            .unwrap_or(false)
                    } else {
                        return false;
                    }
                }
                Key::Named(NamedKey::Backspace) => tree
                    .edit_text(id, |tc| tc.delete_backward())
                    .unwrap_or(false),
                Key::Named(NamedKey::Delete) => tree
                    .edit_text(id, |tc| tc.delete_forward())
                    .unwrap_or(false),
                Key::Named(NamedKey::Left) => {
                    tree.edit_text(id, |tc| {
                        tc.move_caret(true, event.modifiers.contains(Modifiers::SHIFT))
                    });
                    false
                }
                Key::Named(NamedKey::Right) => {
                    tree.edit_text(id, |tc| {
                        tc.move_caret(false, event.modifiers.contains(Modifiers::SHIFT))
                    });
                    false
                }
                Key::Named(NamedKey::Home) => {
                    tree.edit_text(id, |tc| {
                        if let crate::tree::TextBacking::Editable(ref mut ed) = tc.backing {
                            ed.move_to_start(event.modifiers.contains(Modifiers::SHIFT));
                        }
                    });
                    false
                }
                Key::Named(NamedKey::End) => {
                    tree.edit_text(id, |tc| {
                        if let crate::tree::TextBacking::Editable(ref mut ed) = tc.backing {
                            ed.move_to_end(event.modifiers.contains(Modifiers::SHIFT));
                        }
                    });
                    false
                }
                Key::Char('a') if event.modifiers.contains(Modifiers::CONTROL) => {
                    tree.edit_text(id, |tc| {
                        if let crate::tree::TextBacking::Editable(ref mut ed) = tc.backing {
                            ed.select_all();
                        }
                    });
                    false
                }
                _ => return false,
            };
            tree.sync_text_leaf(id);
            if changed {
                let committed = tree.get(id).and_then(|r| match r.kind {
                    NodeKind::Text(ref t) => Some(t.text().to_string()),
                    _ => None,
                });
                tree.fire_action(ActionEvent {
                    node: id,
                    kind: ActionKind::TextChanged,
                    text: committed,
                    focused: None,
                });
            }
            true
        }
    }
}

/// Nearest ancestor (or self) with an `Activate` action, bubbling label
/// hits up to their owning control. A disabled node or ancestor blocks the
/// whole chain: no activation, no focus, no text.
fn activatable_ancestor(tree: &UiTree, mut id: NodeId) -> Option<NodeId> {
    loop {
        let rec = tree.get(id)?;
        if rec.semantics.disabled {
            return None;
        }
        if tree.has_action(id, ActionKind::Activate) {
            return Some(id);
        }
        id = rec.parent?;
    }
}

/// Route one framework input event. Returns `true` when consumed.
///
/// Press semantics (correct control behavior, not press-to-fire):
/// primary down arms the nearest activatable ancestor with a per-contact
/// capture; primary up inside its bounds fires `Activate` exactly once;
/// release outside, cancellation, focus loss, or node removal clears the
/// arm without firing. Non-primary buttons never arm or fire. A disabled
/// node or ancestor blocks the gesture entirely.
pub fn dispatch_input(tree: &mut UiTree, event: &InputEvent) -> bool {
    match event {
        InputEvent::PointerDown(pe, btn) => {
            if *btn != PointerButton::Primary {
                return false;
            }
            let Some(hit) = hit_test(tree, pe.position) else {
                return false;
            };
            if tree.is_disabled(hit) {
                return false;
            }
            let Some(target) = activatable_ancestor(tree, hit) else {
                // No activatable owner, but still grant focus to a focusable
                // node under the pointer (e.g. an editable field).
                if tree
                    .get(hit)
                    .is_some_and(|r| r.focusable && !r.semantics.disabled)
                {
                    tree.set_focused(Some(hit));
                    return true;
                }
                return false;
            };
            // Prefer focusing the control itself; fall back to the hit node.
            if tree
                .get(target)
                .is_some_and(|r| r.focusable && !r.semantics.disabled)
            {
                tree.set_focused(Some(target));
            } else if tree
                .get(hit)
                .is_some_and(|r| r.focusable && !r.semantics.disabled)
            {
                tree.set_focused(Some(hit));
            }
            tree.arm_press(pe.contact, target);
            true
        }
        InputEvent::PointerUp(pe, btn) => {
            if *btn != PointerButton::Primary {
                return false;
            }
            let Some(armed) = tree.take_press(pe.contact) else {
                return false;
            };
            // Node removed mid-press: swallow, never fire on a dead node.
            let Some(rect) = tree.rect(armed) else {
                return true;
            };
            if tree.is_disabled(armed) {
                return true;
            }
            if rect.contains(pe.position) {
                tree.fire_action(ActionEvent {
                    node: armed,
                    kind: ActionKind::Activate,
                    text: None,
                    focused: None,
                });
            }
            true
        }
        InputEvent::PointerMove(_) => false, // hover unimplemented; drag-out is decided at release
        InputEvent::PointerCancel { contact } => {
            tree.take_press(*contact);
            true
        }
        InputEvent::Focused(false) => {
            tree.clear_presses();
            false
        }
        InputEvent::Scroll { at, delta } => dispatch_scroll(tree, *at, *delta),
        InputEvent::KeyDown(k) => dispatch_key(tree, k),
        InputEvent::TextInput { text } => dispatch_text_input(tree, text),
        InputEvent::Ime(ime) => dispatch_ime(tree, ime),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::NaiveMeasurer;
    use crate::style::{Axis, LayoutStyle, Length};
    use crate::tree::{BoxContent, NodeSpec, ScrollContent, TextContent};
    use aqua_core::Size;

    fn col(children: Vec<NodeSpec>) -> NodeSpec {
        let style = LayoutStyle {
            axis: Axis::Row,
            width: Length::Px(300.0),
            height: Length::Px(300.0),
            ..Default::default()
        };
        let mut root = NodeSpec::new(NodeKind::Box(BoxContent::default()), style);
        root.children = children;
        root
    }

    fn laid_out(children: Vec<NodeSpec>) -> (UiTree, NodeId) {
        let mut tree = UiTree::new();
        let root = tree.mount_spec(col(children));
        tree.compute_layout(Size::new(300.0, 300.0), &NaiveMeasurer)
            .unwrap();
        (tree, root)
    }

    fn pe(contact: u64, x: f32, y: f32) -> aqua_core::PointerEvent {
        aqua_core::PointerEvent {
            contact,
            kind: aqua_core::PointerKind::Mouse,
            position: Point::new(x, y),
            buttons: 1,
        }
    }

    fn text_of(r: &crate::tree::NodeRecord) -> String {
        match r.kind {
            NodeKind::Text(ref t) => t.text().to_string(),
            _ => String::new(),
        }
    }

    fn button_fixture() -> (
        UiTree,
        NodeId,
        NodeId,
        std::sync::Arc<std::sync::Mutex<Vec<NodeId>>>,
    ) {
        use std::sync::{Arc, Mutex};
        let mut tree = UiTree::new();
        let bstyle = LayoutStyle {
            width: Length::Px(100.0),
            height: Length::Px(100.0),
            ..Default::default()
        };
        let root = tree.mount_spec(col(vec![NodeSpec::new(
            NodeKind::Box(BoxContent::default()),
            bstyle,
        )
        .child(NodeSpec::new(
            NodeKind::Text(TextContent::label("Add")),
            LayoutStyle::default(),
        ))]));
        tree.compute_layout(Size::new(300.0, 300.0), &NaiveMeasurer)
            .unwrap();
        let button = tree.get(root).unwrap().children[0];
        tree.set_focusable(button, true);
        let fired = Arc::new(Mutex::new(Vec::new()));
        let fired2 = fired.clone();
        tree.set_action_callback(
            button,
            ActionKind::Activate,
            Box::new(move |e| {
                fired2.lock().unwrap().push(e.node);
            }),
        );
        (tree, root, button, fired)
    }

    fn press(contact: u64, x: f32, y: f32) -> InputEvent {
        InputEvent::PointerDown(pe(contact, x, y), PointerButton::Primary)
    }

    #[test]
    fn down_arms_without_firing_up_inside_fires_once() {
        let (mut tree, _root, button, fired) = button_fixture();
        // Down on the label arms the button but fires nothing yet.
        assert!(dispatch_input(&mut tree, &press(1, 10.0, 10.0)));
        assert!(fired.lock().unwrap().is_empty());
        // Release inside fires exactly once, on the button (bubbled).
        assert!(dispatch_input(
            &mut tree,
            &InputEvent::PointerUp(pe(1, 10.0, 10.0), PointerButton::Primary)
        ));
        assert_eq!(*fired.lock().unwrap(), vec![button]);
    }

    #[test]
    fn press_drag_out_does_not_fire() {
        let (mut tree, _root, _button, fired) = button_fixture();
        assert!(dispatch_input(&mut tree, &press(2, 10.0, 10.0)));
        // Release far outside the button: no activation.
        assert!(dispatch_input(
            &mut tree,
            &InputEvent::PointerUp(pe(2, 250.0, 250.0), PointerButton::Primary)
        ));
        assert!(fired.lock().unwrap().is_empty());
    }

    #[test]
    fn cancel_and_removal_clear_arms() {
        let (mut tree, _root, button, fired) = button_fixture();
        assert!(dispatch_input(&mut tree, &press(3, 10.0, 10.0)));
        assert!(dispatch_input(
            &mut tree,
            &InputEvent::PointerCancel { contact: 3 }
        ));
        // Stale up after cancel: nothing armed, not consumed.
        assert!(!dispatch_input(
            &mut tree,
            &InputEvent::PointerUp(pe(3, 10.0, 10.0), PointerButton::Primary)
        ));
        // Re-arm, then remove the button mid-press: the arm dies with the
        // node, so the stale up is unhandled (nothing armed, never fires).
        assert!(dispatch_input(&mut tree, &press(4, 10.0, 10.0)));
        assert!(tree.remove(button));
        assert!(!dispatch_input(
            &mut tree,
            &InputEvent::PointerUp(pe(4, 10.0, 10.0), PointerButton::Primary)
        ));
        assert!(fired.lock().unwrap().is_empty());
    }

    #[test]
    fn secondary_button_never_arms_or_fires() {
        let (mut tree, _root, _button, fired) = button_fixture();
        let down = pe(5, 10.0, 10.0);
        assert!(!dispatch_input(
            &mut tree,
            &InputEvent::PointerDown(down, PointerButton::Secondary)
        ));
        assert!(!dispatch_input(
            &mut tree,
            &InputEvent::PointerUp(pe(5, 10.0, 10.0), PointerButton::Secondary)
        ));
        assert!(fired.lock().unwrap().is_empty());
    }

    #[test]
    fn disabled_button_blocks_press_and_keyboard() {
        use aqua_core::Role;
        let (mut tree, _root, button, fired) = button_fixture();
        tree.set_semantics(
            button,
            crate::tree::NodeSemantics {
                role: Role::Button,
                label: "Add".to_string(),
                disabled: true,
                selected: false,
                value_text: None,
                collection: None,
            },
        );
        // Press is ignored entirely.
        assert!(!dispatch_input(&mut tree, &press(6, 10.0, 10.0)));
        assert!(fired.lock().unwrap().is_empty());
        // ...and focus is refused, so keyboard activation cannot reach it.
        assert!(!tree.set_focused(Some(button)));
        assert!(!dispatch_input(
            &mut tree,
            &InputEvent::KeyDown(KeyEvent {
                key: Key::Named(NamedKey::Enter),
                modifiers: Modifiers::NONE,
                repeat: false,
            })
        ));
        assert!(fired.lock().unwrap().is_empty());
    }

    #[test]
    fn keyboard_space_enter_activate_focused_button() {
        let (mut tree, _root, button, fired) = button_fixture();
        assert!(tree.set_focused(Some(button)));
        for key in [Key::Named(NamedKey::Enter), Key::Char(' ')] {
            assert!(dispatch_input(
                &mut tree,
                &InputEvent::KeyDown(KeyEvent {
                    key,
                    modifiers: Modifiers::NONE,
                    repeat: false,
                })
            ));
        }
        assert_eq!(*fired.lock().unwrap(), vec![button, button]);
    }

    #[test]
    fn keydown_char_never_inserts_text_input_does_once() {
        let (mut tree, _root) = laid_out(vec![NodeSpec::new(
            NodeKind::Text(TextContent::editable("", false)),
            LayoutStyle::default(),
        )]);
        let field = tree.get(tree.root().unwrap()).unwrap().children[0];
        assert!(tree.set_focused(Some(field)));
        // KeyDown carrying 'x' is identity only: no insertion ...
        assert!(!dispatch_input(
            &mut tree,
            &InputEvent::KeyDown(KeyEvent {
                key: Key::Char('x'),
                modifiers: Modifiers::NONE,
                repeat: false,
            })
        ));
        // ... then the platform TextInput inserts exactly once.
        assert!(dispatch_input(
            &mut tree,
            &InputEvent::TextInput { text: "x".into() }
        ));
        assert_eq!(text_of(tree.get(field).unwrap()), "x");
    }

    #[test]
    fn ime_commit_inserts_once_without_text_input() {
        let (mut tree, _root) = laid_out(vec![NodeSpec::new(
            NodeKind::Text(TextContent::editable("", false)),
            LayoutStyle::default(),
        )]);
        let field = tree.get(tree.root().unwrap()).unwrap().children[0];
        assert!(tree.set_focused(Some(field)));
        assert!(dispatch_ime(
            &mut tree,
            &ImeEvent::Update {
                marked: "に".into(),
                cursor: Some((0, 1)),
            }
        ));
        assert!(dispatch_ime(
            &mut tree,
            &ImeEvent::Commit { text: "に".into() }
        ));
        assert_eq!(text_of(tree.get(field).unwrap()), "に");
    }

    #[test]
    fn hit_test_respects_paint_order_and_clips() {
        let style = LayoutStyle {
            axis: Axis::Row,
            width: Length::Px(100.0),
            height: Length::Px(100.0),
            ..Default::default()
        };
        let (tree, root) = laid_out(vec![
            NodeSpec::new(NodeKind::Box(BoxContent::default()), style.clone()),
            NodeSpec::new(NodeKind::Box(BoxContent::default()), style.clone()),
        ]);
        // Both children are 100x100 in a row: the LAST child (x=100) is
        // topmost where they overlap the 300-wide root; hit the second box.
        let kids = tree.get(root).unwrap().children.clone();
        assert_eq!(hit_test(&tree, Point::new(10.0, 10.0)), Some(kids[0]));
        assert_eq!(hit_test(&tree, Point::new(150.0, 10.0)), Some(kids[1]));
        assert_eq!(hit_test(&tree, Point::new(500.0, 500.0)), None);
    }

    #[test]
    fn scroll_gesture_scrolls_innermost_scroller() {
        let rs = LayoutStyle {
            width: Length::Px(100.0),
            height: Length::Px(100.0),
            ..Default::default()
        };
        let cs = LayoutStyle {
            width: Length::Px(100.0),
            height: Length::Px(400.0),
            ..Default::default()
        };
        let (mut tree, root) = laid_out(vec![NodeSpec::new(
            NodeKind::Scroll(ScrollContent {
                offset: Point::ZERO,
            }),
            rs,
        )
        .child(NodeSpec::new(NodeKind::Box(BoxContent::default()), cs))]);
        let scroller = tree.get(root).unwrap().children[0];
        assert!(dispatch_scroll(
            &mut tree,
            Point::new(10.0, 10.0),
            ScrollDelta { x: 0.0, y: 50.0 }
        ));
        let off = match tree.get(scroller).unwrap().kind {
            NodeKind::Scroll(s) => s.offset,
            _ => unreachable!(),
        };
        assert_eq!(off.y, 50.0);
    }

    #[test]
    fn typing_routes_to_focused_editable_and_fires_action() {
        use std::sync::{Arc, Mutex};
        let (mut tree, root) = laid_out(vec![NodeSpec::new(
            NodeKind::Text(TextContent::editable("", false)),
            LayoutStyle::default(),
        )]);
        let field = tree.get(root).unwrap().children[0];
        assert!(tree.set_focused(Some(field)));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        tree.set_action_callback(
            field,
            ActionKind::TextChanged,
            Box::new(move |e| {
                seen2.lock().unwrap().push(e.text.clone().unwrap());
            }),
        );
        assert!(dispatch_text_input(&mut tree, "héllo 🌍"));
        assert_eq!(
            tree.get(field).map(|r| match r.kind {
                NodeKind::Text(ref t) => t.text().to_string(),
                _ => String::new(),
            }),
            Some("héllo 🌍".to_string())
        );
        assert_eq!(*seen.lock().unwrap(), vec!["héllo 🌍".to_string()]);
    }

    #[test]
    fn tab_cycles_focus() {
        let (mut tree, _root) = laid_out(vec![
            NodeSpec::new(
                NodeKind::Text(TextContent::editable("a", false)),
                LayoutStyle::default(),
            ),
            NodeSpec::new(
                NodeKind::Text(TextContent::editable("b", false)),
                LayoutStyle::default(),
            ),
        ]);
        let first = focus_next(&mut tree).unwrap();
        let second = focus_next(&mut tree).unwrap();
        assert_ne!(first, second);
        assert_eq!(focus_next(&mut tree).unwrap(), first);
    }

    #[test]
    fn cancel_event_is_consumed_safely() {
        let (mut tree, _) = laid_out(vec![]);
        assert!(dispatch_input(
            &mut tree,
            &InputEvent::PointerCancel { contact: 3 }
        ));
    }
}
