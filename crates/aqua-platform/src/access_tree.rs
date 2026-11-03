use std::collections::{HashMap, HashSet};

use accesskit::{
    Action, ActionData, Node, NodeId as AkNodeId, Role, TextDirection, TextPosition, TextSelection,
    TreeUpdate,
};
use aqua_core::NodeId;

use crate::a11y::{
    build_base_tree_update, pack_node_id, translate_action_request, A11yActionData,
    A11yActionRequest, A11yNode,
};

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
                let mut node = Node::new(Role::TextRun);
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
