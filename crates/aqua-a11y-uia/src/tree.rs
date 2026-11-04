use std::collections::{HashMap, HashSet};

use accesskit::{
    Action, ActionData, Node, NodeId as AkNodeId, Rect as AkRect, Role as AkRole, TextDirection,
    TextPosition, TextSelection, TreeId, TreeInfo, TreeUpdate,
};
use aqua_core::{NodeId, Rect, Role as CoreRole};
use aqua_protocol::{A11yActionData, A11yActionRequest, A11yNode};

use crate::action::translate_action_request;

pub fn pack_node_id(id: NodeId) -> AkNodeId {
    let raw = id.raw();
    AkNodeId(((u64::from(raw.index) << 32) | u64::from(raw.generation)) as accesskit::NodeIdContent)
}

pub fn unpack_node_id(id: AkNodeId) -> NodeId {
    let v = id.0;
    NodeId::from_raw(aqua_core::id::RawId {
        index: (v >> 32) as u32,
        generation: (v & 0xFFFF_FFFF) as u32,
    })
}

pub fn map_role(role: &CoreRole) -> AkRole {
    match role {
        CoreRole::Text => AkRole::Label,
        CoreRole::PasswordField => AkRole::PasswordInput,
        CoreRole::ComboBox => AkRole::ComboBox,
        CoreRole::Option => AkRole::ListBoxOption,
        CoreRole::Separator => AkRole::Splitter,
        CoreRole::Tooltip => AkRole::Tooltip,
        CoreRole::Group => AkRole::Group,
        CoreRole::Button => AkRole::Button,
        CoreRole::Checkbox => AkRole::CheckBox,
        CoreRole::Switch => AkRole::Switch,
        CoreRole::RadioButton => AkRole::RadioButton,
        CoreRole::TextField => AkRole::TextInput,
        CoreRole::Heading(_) => AkRole::Heading,
        CoreRole::List => AkRole::List,
        CoreRole::ListBox => AkRole::ListBox,
        CoreRole::RadioGroup => AkRole::RadioGroup,
        CoreRole::ListItem => AkRole::ListItem,
        CoreRole::Tree => AkRole::Tree,
        CoreRole::TreeItem => AkRole::TreeItem,
        CoreRole::Table => AkRole::Table,
        CoreRole::Row => AkRole::Row,
        CoreRole::ColumnHeader => AkRole::ColumnHeader,
        CoreRole::Cell => AkRole::Cell,
        CoreRole::Slider => AkRole::Slider,
        CoreRole::ProgressIndicator => AkRole::ProgressIndicator,
        CoreRole::TabList => AkRole::TabList,
        CoreRole::Tab => AkRole::Tab,
        CoreRole::Image => AkRole::Image,
        CoreRole::ScrollRegion => AkRole::ScrollView,
        CoreRole::Dialog => AkRole::Dialog,
        CoreRole::Menu => AkRole::Menu,
        CoreRole::MenuItem => AkRole::MenuItem,
    }
}

pub fn build_tree_update(root: NodeId, focus: NodeId, items: &[A11yNode]) -> TreeUpdate {
    A11yTree::default().build(root, focus, items)
}

pub(crate) fn build_base_tree_update(
    root: NodeId,
    focus: NodeId,
    items: &[A11yNode],
) -> TreeUpdate {
    let mut nodes = Vec::with_capacity(items.len());
    let mut focus_id = pack_node_id(root);
    for item in items {
        let id = pack_node_id(item.semantic.node);
        if item.semantic.focused || item.semantic.node == focus {
            focus_id = id;
        }
        let mut node = Node::new(map_role(&item.semantic.role));
        if item.semantic.role == CoreRole::ScrollRegion {
            node.set_clips_children();
        }
        if !item.semantic.disabled {
            match item.semantic.role {
                CoreRole::Button
                | CoreRole::Checkbox
                | CoreRole::Switch
                | CoreRole::RadioButton
                | CoreRole::Tab
                | CoreRole::MenuItem
                | CoreRole::Option
                | CoreRole::TreeItem => {
                    node.add_action(Action::Click);
                    node.add_action(Action::Focus);
                }
                CoreRole::ComboBox => {
                    node.add_action(Action::Click);
                    node.add_action(Action::Focus);
                    node.add_action(Action::Expand);
                    node.add_action(Action::Collapse);
                }
                CoreRole::Slider | CoreRole::Separator if item.semantic.numeric_range.is_some() => {
                    node.add_action(Action::Focus);
                    node.add_action(Action::SetValue);
                    node.add_action(Action::Increment);
                    node.add_action(Action::Decrement);
                }
                CoreRole::TextField | CoreRole::PasswordField => {
                    node.add_action(Action::Focus);
                    node.add_action(Action::SetValue);
                    node.add_action(Action::ReplaceSelectedText);
                }
                _ => {}
            }
        }
        if !item.semantic.label.is_empty() {
            node.set_label(item.semantic.label.clone());
        }
        if let Some(description) = &item.semantic.description {
            node.set_description(description.clone());
        }
        if item.semantic.disabled {
            node.set_disabled();
        }
        if let Some(checked) = item.semantic.checked {
            node.set_toggled(checked.into());
        } else if matches!(item.semantic.role, CoreRole::Switch | CoreRole::RadioButton) {
            node.set_toggled(item.semantic.selected.into());
        } else if item.semantic.selected {
            node.set_selected(true);
        }
        if let Some(level) = item.semantic.level {
            node.set_level(level);
        }
        if let Some(expanded) = item.semantic.expanded {
            node.set_expanded(expanded);
        }
        if let Some(range) = item.semantic.numeric_range.filter(|range| {
            range.min.is_finite()
                && range.max.is_finite()
                && range.min <= range.max
                && range.value.is_finite()
                && range.step.is_finite()
                && range.step > 0.0
        }) {
            node.set_min_numeric_value(range.min);
            node.set_max_numeric_value(range.max);
            node.set_numeric_value(range.value.clamp(range.min, range.max));
            node.set_numeric_value_step(range.step);
        }
        match (&item.semantic.value_text, &item.semantic.role) {
            (Some(value), _) => node.set_value(value.clone()),
            (None, CoreRole::Text) if !item.semantic.label.is_empty() => {
                node.set_value(item.semantic.label.clone())
            }
            _ => {}
        }
        if let Some(collection) = item.semantic.collection {
            if let Some(count) = collection.row_count {
                node.set_row_count(count);
            }
            if let Some(count) = collection.column_count {
                node.set_column_count(count);
            }
            if let Some(index) = collection.row_index {
                node.set_row_index(index);
            }
            if let Some(index) = collection.column_index {
                node.set_column_index(index);
            }
            if let Some(count) = collection.set_size {
                node.set_size_of_set(count);
            }
            if let Some(index) = collection.position_in_set.and_then(|n| n.checked_sub(1)) {
                node.set_position_in_set(index);
            }
        }
        let r = item.bounds.unwrap_or(Rect::ZERO);
        node.set_bounds(AkRect {
            x0: f64::from(r.x),
            y0: f64::from(r.y),
            x1: f64::from(r.right()),
            y1: f64::from(r.bottom()),
        });
        node.set_children(
            item.children
                .iter()
                .map(|c| pack_node_id(*c))
                .collect::<Vec<_>>(),
        );
        nodes.push((id, node));
    }
    TreeUpdate {
        nodes,
        tree: Some(TreeInfo::new(pack_node_id(root))),
        tree_id: TreeId::ROOT,
        focus: focus_id,
    }
}

#[derive(Default)]
pub struct A11yTree {
    next_run: u32,
    ids: HashMap<(NodeId, usize), AkNodeId>,
    runs: HashMap<AkNodeId, RunAddress>,
}

struct RunAddress {
    owner: NodeId,
    byte_offsets: Vec<usize>,
}

impl A11yTree {
    pub fn build(&mut self, root: NodeId, focus: NodeId, items: &[A11yNode]) -> TreeUpdate {
        let mut update = build_base_tree_update(root, focus, items);
        let live: HashSet<_> = items
            .iter()
            .filter_map(|item| {
                item.text
                    .as_ref()
                    .map(|text| (item.semantic.node, text.runs.len()))
            })
            .flat_map(|(owner, count)| (0..count).map(move |index| (owner, index)))
            .collect();
        self.ids.retain(|key, _| live.contains(key));
        self.runs.clear();
        for item in items {
            let Some(text) = &item.text else { continue };
            let mut run_ids = Vec::new();
            for (index, run) in text.runs.iter().enumerate() {
                let id = *self
                    .ids
                    .entry((item.semantic.node, index))
                    .or_insert_with(|| {
                        let serial = self.next_run;
                        self.next_run = self
                            .next_run
                            .checked_add(1)
                            .expect("accessibility text identity exhausted");
                        AkNodeId(
                            ((u64::from(u32::MAX) << 32) | u64::from(serial))
                                as accesskit::NodeIdContent,
                        )
                    });
                let mut bytes = run.byte_start;
                let mut offsets = vec![bytes];
                for length in &run.character_lengths {
                    bytes += usize::from(*length);
                    offsets.push(bytes);
                }
                if bytes - run.byte_start != run.text.len() {
                    continue;
                }
                let mut node = Node::new(AkRole::TextRun);
                node.set_value(run.text.clone());
                node.set_character_lengths(run.character_lengths.clone());
                node.set_word_starts(run.word_starts.clone());
                node.set_text_direction(if run.rtl {
                    TextDirection::RightToLeft
                } else {
                    TextDirection::LeftToRight
                });
                node.set_bounds(accesskit::Rect {
                    x0: f64::from(run.bounds.x),
                    y0: f64::from(run.bounds.y),
                    x1: f64::from(run.bounds.right()),
                    y1: f64::from(run.bounds.bottom()),
                });
                if run.character_positions.len() == run.character_lengths.len()
                    && run.character_widths.len() == run.character_lengths.len()
                {
                    node.set_character_positions(run.character_positions.clone());
                    node.set_character_widths(run.character_widths.clone());
                }
                self.runs.insert(
                    id,
                    RunAddress {
                        owner: item.semantic.node,
                        byte_offsets: offsets,
                    },
                );
                run_ids.push(id);
                update.nodes.push((id, node));
            }
            let selection = text.selection.and_then(|(anchor, focus)| {
                Some(TextSelection {
                    anchor: self.position(&run_ids, anchor)?,
                    focus: self.position(&run_ids, focus)?,
                })
            });
            if let Some((_, parent)) = update
                .nodes
                .iter_mut()
                .find(|(id, _)| *id == pack_node_id(item.semantic.node))
            {
                let mut children = parent.children().to_vec();
                children.extend(run_ids);
                parent.set_children(children);
                if text.selection_enabled && !item.semantic.disabled {
                    parent.add_action(Action::SetTextSelection);
                }
                if let Some(selection) = selection {
                    parent.set_text_selection(selection);
                }
            }
        }
        update
    }

    fn position(&self, ids: &[AkNodeId], byte: usize) -> Option<TextPosition> {
        ids.iter().rev().find_map(|id| {
            let run = self.runs.get(id)?;
            if byte < *run.byte_offsets.first()? {
                return None;
            }
            let index = run
                .byte_offsets
                .partition_point(|offset| *offset <= byte)
                .saturating_sub(1);
            Some(TextPosition {
                node: *id,
                character_index: index.min(run.byte_offsets.len() - 1),
            })
        })
    }

    pub fn translate(&self, request: &accesskit::ActionRequest) -> Option<A11yActionRequest> {
        let mut translated = translate_action_request(request);
        match self.runs.get(&request.target_node) {
            Some(run) => translated.node = run.owner,
            None if request.target_node.0 >> 32 == u64::from(u32::MAX) => return None,
            None => {}
        }
        if let Some(ActionData::SetTextSelection(selection)) = &request.data {
            let anchor = self.runs.get(&selection.anchor.node)?;
            let focus = self.runs.get(&selection.focus.node)?;
            let anchor_byte = *anchor.byte_offsets.get(selection.anchor.character_index)?;
            let focus_byte = *focus.byte_offsets.get(selection.focus.character_index)?;
            translated.data = A11yActionData::TextByteSelection {
                anchor_node: anchor.owner,
                anchor_byte,
                focus_node: focus.owner,
                focus_byte,
            };
        }
        Some(translated)
    }
}
