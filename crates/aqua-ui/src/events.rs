use crate::tree::{ActionEvent, ActionKind, NodeKind, UiTree};
use aqua_core::{
    ImeEvent, InputEvent, Key, KeyEvent, Modifiers, NamedKey, NodeId, Point, PointerButton, Rect,
    ScrollDelta,
};

#[must_use]
pub fn hit_test(tree: &UiTree, at: Point) -> Option<NodeId> {
    for &portal in tree.portal_roots_in_paint_order().iter().rev() {
        if hit_test_excluded(tree, portal) {
            continue;
        }
        if let Some(hit) = hit_node(tree, portal, at, None) {
            return Some(hit);
        }
    }
    let root = tree.root()?;
    hit_node(tree, root, at, None)
}

fn hit_test_excluded(tree: &UiTree, mut node: NodeId) -> bool {
    loop {
        let Some(record) = tree.get(node) else {
            return true;
        };
        if record.hit_test_excluded {
            return true;
        }
        match record.parent {
            Some(parent) => node = parent,
            None => return false,
        }
    }
}

fn hit_node(tree: &UiTree, id: NodeId, at: Point, clip: Option<Rect>) -> Option<NodeId> {
    tree.rect(id)?;
    let rec = tree.get(id)?;
    if rec.hit_test_excluded {
        return None;
    }
    let clip = if tree.is_portal(id) { None } else { clip };
    let clips = matches!(&rec.kind, NodeKind::Box(b) if b.paint().clip)
        || matches!(rec.kind, NodeKind::Scroll(_));
    let child_clip = if clips {
        let rect = tree.visual_rect(id)?;
        Some(clip.map_or(rect, |parent| parent.intersect(rect)))
    } else {
        clip
    };
    if child_clip.is_some_and(|clip| !clip.contains(at)) {
        return None;
    }
    for &child in rec.children.iter().rev() {
        if tree.is_portal(child) {
            continue;
        }
        if let Some(hit) = hit_node(tree, child, at, child_clip) {
            return Some(hit);
        }
    }
    let local = crate::input::local_position(tree, id, at)?;
    let layout = tree.rect(id)?;
    (Rect::from_edges(0.0, 0.0, layout.w, layout.h).contains(local)
        && child_clip.is_none_or(|clip| clip.contains(at)))
    .then_some(id)
}

#[must_use]
pub fn focusable_nodes(tree: &UiTree) -> Vec<NodeId> {
    let mut nodes: Vec<_> = tree
        .traverse()
        .into_iter()
        .enumerate()
        .filter_map(|(order, id)| {
            let record = tree.get(id)?;
            (record.focusable && !tree.is_disabled(id) && record.tab_index >= 0).then_some((
                id,
                record.tab_index,
                order,
            ))
        })
        .collect();
    nodes.sort_by_key(|(_, tab_index, order)| {
        (if *tab_index > 0 { 0 } else { 1 }, *tab_index, *order)
    });
    nodes.into_iter().map(|(id, _, _)| id).collect()
}

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

#[must_use]
pub fn scroll_target(tree: &UiTree, at: Point, delta: ScrollDelta) -> Option<NodeId> {
    let mut target = hit_test(tree, at);
    while let Some(id) = target {
        if let Some(offset) = tree.scroll_offset(id) {
            let max = tree.scroll_limit(id);
            let moves = |current: f32, change: f32, limit: f32| {
                change != 0.0 && (current + change).clamp(0.0, limit) != current
            };
            if moves(offset.x, delta.x, max.x) || moves(offset.y, delta.y, max.y) {
                return Some(id);
            }
        }
        if tree.is_portal(id) {
            break;
        }
        target = tree.get(id).and_then(|r| r.parent);
    }
    None
}

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
            if tree.scroll_to(id, Point::new(cur.x + delta.x, cur.y + delta.y)) {
                return true;
            }
            let contains_prompt = tree.get(id).is_some_and(|record| record.children.iter().any(|child| {
                tree.get(*child).is_some_and(|child| matches!(&child.kind, NodeKind::Text(text) if text.viewport_scroll_max.is_some()))
            }));
            if contains_prompt
                && tree
                    .content_size(id)
                    .zip(tree.rect(id))
                    .is_some_and(|(content, bounds)| content.h > bounds.h)
            {
                return true;
            }
        }
        if tree.is_portal(id) {
            break;
        }
        target = tree.get(id).and_then(|r| r.parent);
    }
    false
}

pub fn dispatch_text_input(tree: &mut UiTree, text: &str) -> bool {
    if text_is_composing(tree) {
        return true;
    }
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
                source: None,
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

fn focused_editable(tree: &UiTree) -> Option<NodeId> {
    let id = tree.focused()?;
    let editable = !tree.is_disabled(id)
        && tree
            .get(id)
            .is_some_and(|r| matches!(r.kind, NodeKind::Text(ref t) if t.is_editable()));
    editable.then_some(id)
}

fn utf16_to_byte(text: &str, index: u32) -> Option<usize> {
    let mut units = 0u32;
    for (byte, ch) in text.char_indices() {
        match units.cmp(&index) {
            std::cmp::Ordering::Equal => return Some(byte),
            std::cmp::Ordering::Greater => return None,
            std::cmp::Ordering::Less => units += ch.len_utf16() as u32,
        }
    }
    (units == index).then_some(text.len())
}

fn utf16_to_byte_ceil(text: &str, index: u32) -> usize {
    let mut units = 0u32;
    for (byte, ch) in text.char_indices() {
        if units >= index {
            return byte;
        }
        units += ch.len_utf16() as u32;
    }
    text.len()
}

fn byte_to_utf16(text: &str, byte: usize) -> u32 {
    text[..byte.min(text.len())]
        .chars()
        .map(|c| c.len_utf16() as u32)
        .sum()
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImeSurroundings {
    pub node: NodeId,
    pub window_start: u32,
    pub total_len: u32,
    pub text: String,
    pub selection: (u32, u32),
    pub reversed: bool,
}

const IME_WINDOW_UNITS: u32 = 4096;

pub fn ime_surroundings(tree: &UiTree) -> Option<ImeSurroundings> {
    let node = focused_editable(tree)?;
    let NodeKind::Text(content) = &tree.get(node)?.kind else {
        return None;
    };
    let crate::tree::TextBacking::Editable(editor) = &content.backing else {
        return None;
    };
    let text = editor.text();
    let selection = editor.selection();
    let range = selection.range();
    let total_len = byte_to_utf16(text, text.len());
    let start_unit = byte_to_utf16(text, range.start);
    let end_unit = byte_to_utf16(text, range.end);
    let want_start = start_unit.saturating_sub(IME_WINDOW_UNITS);
    let want_end = end_unit.saturating_add(IME_WINDOW_UNITS).min(total_len);
    let window_start = utf16_to_byte_ceil(text, want_start);
    let window_end = utf16_to_byte_ceil(text, want_end);
    Some(ImeSurroundings {
        node,
        window_start: byte_to_utf16(text, window_start),
        total_len,
        text: text[window_start..window_end].to_owned(),
        selection: (start_unit, end_unit),
        reversed: selection.caret < selection.anchor,
    })
}

fn dispatch_ime_replace(tree: &mut UiTree, start: u32, end: u32, replacement: &str) -> bool {
    let Some(id) = focused_editable(tree) else {
        return false;
    };
    let applied = tree
        .edit_text(id, |tc| match tc.backing {
            crate::tree::TextBacking::Editable(ref mut ed) => {
                let range = utf16_to_byte(ed.text(), start).zip(utf16_to_byte(ed.text(), end));
                match range {
                    Some((from, to)) if from <= to => ed.replace_range(from..to, replacement),
                    _ => false,
                }
            }
            _ => false,
        })
        .unwrap_or(false);
    if applied {
        tree.sync_text_leaf(id);
        let committed = tree.get(id).and_then(|r| match r.kind {
            NodeKind::Text(ref t) => Some(t.text().to_string()),
            _ => None,
        });
        tree.fire_action(ActionEvent {
            source: None,
            node: id,
            kind: ActionKind::TextChanged,
            text: committed,
            focused: None,
        });
    }
    applied
}

fn dispatch_ime_select(tree: &mut UiTree, start: u32, end: u32, reversed: bool) -> bool {
    let Some(id) = focused_editable(tree) else {
        return false;
    };
    let applied = tree
        .edit_text(id, |tc| match tc.backing {
            crate::tree::TextBacking::Editable(ref mut ed) if !ed.is_composing() => {
                let range = utf16_to_byte(ed.text(), start).zip(utf16_to_byte(ed.text(), end));
                match range {
                    Some((from, to)) if from <= to => {
                        let selection = match reversed {
                            true => aqua_text::Selection::new(to, from),
                            false => aqua_text::Selection::new(from, to),
                        };
                        ed.set_selection(selection);
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        })
        .unwrap_or(false);
    applied
}

pub fn dispatch_ime(tree: &mut UiTree, event: &ImeEvent) -> bool {
    let Some(id) = focused_editable(tree) else {
        return false;
    };
    match event {
        ImeEvent::Start => true,
        ImeEvent::Update { marked, cursor } => {
            let range = cursor.unwrap_or((marked.len(), marked.len()));
            let marked = marked.clone();
            tree.edit_text(id, |tc| {
                if let crate::tree::TextBacking::Editable(ref mut ed) = tc.backing {
                    if marked.is_empty() {
                        ed.cancel_composition();
                    } else {
                        ed.break_typing_group();
                        let end = marked.len();
                        let range = range.0.min(end)..range.1.min(end).max(range.0.min(end));
                        ed.set_composition(&marked, range);
                    }
                }
            });
            tree.sync_text_leaf(id);
            true
        }
        ImeEvent::Commit { text } => {
            let text = text.clone();
            let committed_done = tree.edit_text(id, |tc| {
                if let crate::tree::TextBacking::Editable(ref mut ed) = tc.backing {
                    if ed.is_composing() {
                        ed.set_composition(&text, text.len()..text.len());
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
                    source: None,
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

pub fn text_is_composing(tree: &UiTree) -> bool {
    tree.focused().is_some_and(|id| !tree.is_disabled(id) && tree.get(id).is_some_and(|record| {
        matches!(&record.kind, NodeKind::Text(text) if matches!(&text.backing, crate::tree::TextBacking::Editable(editor) if editor.is_composing()))
    }))
}

pub fn dispatch_key(tree: &mut UiTree, event: &KeyEvent) -> bool {
    tree.pending_keyboard_activation = None;
    if !matches!(event.key, Key::Named(NamedKey::Tab))
        && tree
            .focused()
            .and_then(|node| tree.get(node))
            .is_some_and(|record| {
                matches!(
                    &record.kind, NodeKind::Text(text) if text.external_text_input
                )
            })
    {
        return false;
    }
    if text_is_composing(tree) {
        return true;
    }
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
            if tree.is_disabled(id) {
                return false;
            }
            let is_editable = tree
                .get(id)
                .map(|r| matches!(r.kind, NodeKind::Text(ref t) if t.is_editable()))
                .unwrap_or(false);
            if !is_editable {
                if tree
                    .get(id)
                    .is_some_and(|record| record.keyboard_release_activation)
                {
                    let activation =
                        matches!(event.key, Key::Named(NamedKey::Enter) | Key::Char(' '))
                            && event.modifiers.is_empty()
                            && tree.has_action(id, ActionKind::Activate);
                    tree.pending_keyboard_activation =
                        activation.then_some((id, tree.focus_generation));
                    return activation;
                }
                if tree.is_disabled(id) {
                    return false;
                }
                if matches!(event.key, Key::Named(NamedKey::Enter) | Key::Char(' '))
                    && !event.repeat
                {
                    tree.fire_action(ActionEvent {
                        source: Some(crate::tree::ActivationSource::Keyboard(*event)),
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
                            source: None,
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
                key => {
                    let extend = event.modifiers.contains(Modifiers::SHIFT);
                    let control = event.modifiers.contains(Modifiers::CONTROL);
                    let alt = event.modifiers.contains(Modifiers::ALT);
                    if alt || event.modifiers.contains(Modifiers::LOGO) {
                        return false;
                    }
                    let result = tree
                        .edit_editor(id, |ed| {
                            Some(match key {
                                Key::Named(NamedKey::Backspace) => {
                                    if control {
                                        ed.delete_word_backward()
                                    } else {
                                        ed.delete_backward()
                                    }
                                }
                                Key::Named(NamedKey::Delete) => {
                                    if control {
                                        ed.delete_word_forward()
                                    } else {
                                        ed.delete_forward()
                                    }
                                }
                                Key::Named(NamedKey::Left) => {
                                    if control {
                                        ed.move_word_left(extend);
                                    } else {
                                        ed.move_left(extend);
                                    }
                                    false
                                }
                                Key::Named(NamedKey::Right) => {
                                    if control {
                                        ed.move_word_right(extend);
                                    } else {
                                        ed.move_right(extend);
                                    }
                                    false
                                }
                                Key::Named(NamedKey::Up) => {
                                    if control {
                                        ed.move_paragraph(false, extend);
                                    } else {
                                        ed.move_vertical(true, extend);
                                    }
                                    false
                                }
                                Key::Named(NamedKey::Down) => {
                                    if control {
                                        ed.move_paragraph(true, extend);
                                    } else {
                                        ed.move_vertical(false, extend);
                                    }
                                    false
                                }
                                Key::Named(NamedKey::Home) => {
                                    if control {
                                        ed.move_to_start(extend);
                                    } else {
                                        ed.move_line_boundary(false, extend);
                                    }
                                    false
                                }
                                Key::Named(NamedKey::End) => {
                                    if control {
                                        ed.move_to_end(extend);
                                    } else {
                                        ed.move_line_boundary(true, extend);
                                    }
                                    false
                                }
                                Key::Char('a' | 'A') if control => {
                                    ed.select_all();
                                    false
                                }
                                Key::Char('z' | 'Z') if control => {
                                    if extend {
                                        ed.redo()
                                    } else {
                                        ed.undo()
                                    }
                                }
                                Key::Char('y' | 'Y') if control => ed.redo(),
                                _ => return None,
                            })
                        })
                        .flatten();
                    let Some(changed) = result else {
                        return false;
                    };
                    changed
                }
            };
            if changed {
                let committed = tree.get(id).and_then(|r| match r.kind {
                    NodeKind::Text(ref t) => Some(t.text().to_string()),
                    _ => None,
                });
                tree.fire_action(ActionEvent {
                    source: None,
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

pub fn dispatch_input(tree: &mut UiTree, event: &InputEvent) -> bool {
    tree.cancel_blocked_presses(|_, _| true);
    crate::controls::cancel_blocked(tree, |_, _| true);
    let routed = crate::input::route_input(tree, event, |_, _| true);
    let handled = if routed.default_prevented {
        true
    } else {
        dispatch_default(tree, event) || routed.handled
    };
    tree.drain_commands();
    handled
}

pub fn dispatch_default(tree: &mut UiTree, event: &InputEvent) -> bool {
    if crate::controls::dispatch(tree, event) {
        return true;
    }
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
                if tree
                    .get(hit)
                    .is_some_and(|r| r.focusable && !r.semantics.disabled)
                {
                    tree.set_focused(Some(hit));
                    return true;
                }
                return false;
            };
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
            tree.press_sources.insert(pe.contact, (*pe, *btn));
            tree.arm_press(pe.contact, target);
            true
        }
        InputEvent::PointerUp(pe, btn) => {
            if *btn != PointerButton::Primary {
                return false;
            }
            let source = tree.press_sources.remove(&pe.contact);
            let Some(armed) = tree.take_press(pe.contact) else {
                return false;
            };
            let Some(rect) = tree.rect(armed) else {
                return true;
            };
            if tree.is_disabled(armed) {
                return true;
            }
            let source = if pe.kind == aqua_core::PointerKind::Touch {
                tree.interaction_tap_count(pe.contact).map(|tap_count| {
                    crate::tree::ActivationSource::Touch {
                        position: pe.position,
                        tap_count,
                        long_press: false,
                    }
                })
            } else {
                source.map(|(down, button)| crate::tree::ActivationSource::Pointer {
                    down,
                    up: *pe,
                    button,
                    modifiers: tree.input.modifiers(),
                    click_count: tree.interaction_click_count(pe, button),
                })
            };
            if rect.contains(pe.position)
                && (pe.kind != aqua_core::PointerKind::Touch || source.is_some())
            {
                tree.fire_action(ActionEvent {
                    source,
                    node: armed,
                    kind: ActionKind::Activate,
                    text: None,
                    focused: None,
                });
            }
            true
        }
        InputEvent::PointerMove(_) => false,
        InputEvent::PointerCancel { contact } => {
            tree.press_sources.remove(contact);
            tree.take_press(*contact);
            true
        }
        InputEvent::Focused(false) => {
            tree.clear_presses();
            false
        }
        InputEvent::Scroll { at, delta } => dispatch_scroll(tree, *at, *delta),
        InputEvent::KeyDown(k) => dispatch_key(tree, k),
        InputEvent::KeyUp(key) => {
            let pending = tree.pending_keyboard_activation.take();
            let Some((node, generation)) = pending else {
                return false;
            };
            if tree.focused() != Some(node)
                || tree.focus_generation != generation
                || tree.is_disabled(node)
                || !key.modifiers.is_empty()
                || !matches!(key.key, Key::Named(NamedKey::Enter) | Key::Char(' '))
                || !tree.has_action(node, ActionKind::Activate)
            {
                return false;
            }
            tree.fire_action(ActionEvent {
                source: Some(crate::tree::ActivationSource::Keyboard(*key)),
                node,
                kind: ActionKind::Activate,
                text: None,
                focused: None,
            });
            true
        }
        InputEvent::TextInput { text } => dispatch_text_input(tree, text),
        InputEvent::Ime(ime) => dispatch_ime(tree, ime),
        InputEvent::ImeReplace { start, end, text } => {
            dispatch_ime_replace(tree, *start, *end, text)
        }
        InputEvent::ImeSelect {
            start,
            end,
            reversed,
        } => dispatch_ime_select(tree, *start, *end, *reversed),
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
            NodeKind::text(TextContent::label("Add")),
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
        assert!(dispatch_input(&mut tree, &press(1, 10.0, 10.0)));
        assert!(fired.lock().unwrap().is_empty());
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
        assert!(!dispatch_input(
            &mut tree,
            &InputEvent::PointerUp(pe(3, 10.0, 10.0), PointerButton::Primary)
        ));
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
        assert!(!dispatch_input(&mut tree, &press(6, 10.0, 10.0)));
        assert!(fired.lock().unwrap().is_empty());
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
            NodeKind::text(TextContent::editable("", false)),
            LayoutStyle::default(),
        )]);
        let field = tree.get(tree.root().unwrap()).unwrap().children[0];
        assert!(tree.set_focused(Some(field)));
        assert!(!dispatch_input(
            &mut tree,
            &InputEvent::KeyDown(KeyEvent {
                key: Key::Char('x'),
                modifiers: Modifiers::NONE,
                repeat: false,
            })
        ));
        assert!(dispatch_input(
            &mut tree,
            &InputEvent::TextInput { text: "x".into() }
        ));
        assert_eq!(text_of(tree.get(field).unwrap()), "x");
    }

    #[test]
    fn ime_commit_inserts_once_without_text_input() {
        let (mut tree, _root) = laid_out(vec![NodeSpec::new(
            NodeKind::text(TextContent::editable("", false)),
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

    fn focused_field(initial: &str) -> (UiTree, NodeId) {
        let (mut tree, _root) = laid_out(vec![NodeSpec::new(
            NodeKind::text(TextContent::editable(initial, false)),
            LayoutStyle::default(),
        )]);
        let field = tree.get(tree.root().unwrap()).unwrap().children[0];
        assert!(tree.set_focused(Some(field)));
        (tree, field)
    }

    #[test]
    fn ime_replace_uses_utf16_offsets() {
        let (mut tree, field) = focused_field("a😀bc");
        assert!(dispatch_default(
            &mut tree,
            &InputEvent::ImeReplace {
                start: 3,
                end: 4,
                text: "XY".into()
            }
        ));
        assert_eq!(text_of(tree.get(field).unwrap()), "a😀XYc");
    }

    #[test]
    fn ime_replace_inside_surrogate_pair_is_rejected() {
        let (mut tree, field) = focused_field("a😀bc");
        assert!(!dispatch_default(
            &mut tree,
            &InputEvent::ImeReplace {
                start: 2,
                end: 3,
                text: "X".into()
            }
        ));
        assert_eq!(text_of(tree.get(field).unwrap()), "a😀bc");
    }

    #[test]
    fn ime_select_maps_range_and_direction() {
        let (mut tree, _field) = focused_field("a😀bc");
        assert!(dispatch_default(
            &mut tree,
            &InputEvent::ImeSelect {
                start: 1,
                end: 3,
                reversed: true
            }
        ));
        let surroundings = ime_surroundings(&tree).unwrap();
        assert_eq!(surroundings.selection, (1, 3));
        assert!(surroundings.reversed);
        assert_eq!(surroundings.total_len, 5);
        assert_eq!(surroundings.text, "a😀bc");
    }

    #[test]
    fn ime_select_is_ignored_while_composing() {
        let (mut tree, _field) = focused_field("abc");
        assert!(dispatch_ime(
            &mut tree,
            &ImeEvent::Update {
                marked: "に".into(),
                cursor: Some((0, 3)),
            }
        ));
        assert!(!dispatch_default(
            &mut tree,
            &InputEvent::ImeSelect {
                start: 0,
                end: 1,
                reversed: false
            }
        ));
    }

    #[test]
    fn surroundings_window_is_bounded_around_the_selection() {
        let text = "x".repeat(20000);
        let (mut tree, field) = focused_field(&text);
        tree.edit_text(field, |tc| {
            if let crate::tree::TextBacking::Editable(ref mut ed) = tc.backing {
                ed.set_selection(aqua_text::Selection::caret_at(10000));
            }
        });
        let surroundings = ime_surroundings(&tree).unwrap();
        assert_eq!(surroundings.total_len, 20000);
        assert_eq!(surroundings.window_start, 10000 - 4096);
        assert_eq!(surroundings.text.len(), 8192);
        assert_eq!(surroundings.selection, (10000, 10000));
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
            NodeKind::text(TextContent::editable("", false)),
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
                NodeKind::text(TextContent::editable("a", false)),
                LayoutStyle::default(),
            ),
            NodeSpec::new(
                NodeKind::text(TextContent::editable("b", false)),
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
