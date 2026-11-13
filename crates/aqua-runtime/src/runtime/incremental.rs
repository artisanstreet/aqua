use super::*;

impl Runtime {
    pub(super) fn index_fragments(&mut self) {
        self.fragment_slots.clear();
        self.fragment_spans.clear();
        for (index, (key, _)) in self.cached_scene.iter().enumerate() {
            self.fragment_slots
                .entry(key.node())
                .or_default()
                .push(index);
        }
        for (node, slots) in &self.fragment_slots {
            let start = *slots.first().unwrap();
            let end = *slots.last().unwrap() + 1;
            let leaf = self
                .tree
                .get(*node)
                .is_some_and(|record| record.children.is_empty());
            let closed = matches!(
                self.cached_scene[end - 1].1,
                DisplayItem::PopOpacity
                    | DisplayItem::PopTransform
                    | DisplayItem::PopClip
                    | DisplayItem::PopContentBlur
            );
            if leaf || closed {
                self.fragment_spans.insert(*node, start..end);
            }
        }
    }

    pub(super) fn patch_dirty_scene(&mut self, version: (u64, u32, u32, u32, u64)) -> bool {
        let Some(previous) = self.cached_version else {
            return false;
        };
        if previous.1 != version.1
            || previous.2 != version.2
            || previous.3 != version.3
            || previous.4 != version.4
            || self.scroll_revision != self.tree.scroll_revision()
        {
            return false;
        }
        let Some(fragments) = self.tree.emit_dirty_fragments() else {
            return false;
        };
        for node in &fragments.nodes {
            if self
                .tree
                .get(*node)
                .is_some_and(|record| matches!(record.kind, NodeKind::Text(_)))
            {
                self.sync_text_runs_for(Some(*node));
            }
        }
        for node in &fragments.nodes {
            render_motion::forget_motion_node(&mut self.motion_keys, *node);
        }
        let expected = fragments
            .nodes
            .iter()
            .map(|node| self.fragment_slots.get(node).map_or(0, Vec::len))
            .sum::<usize>();
        let structure_changed = self.scene_structure != Some(self.tree.structure_epoch());
        let root_nodes = (structure_changed || fragments.items.len() != expected).then(|| {
            fragments
                .roots
                .iter()
                .map(|(_, range)| {
                    fragments.items[range.clone()]
                        .iter()
                        .map(|(node, _)| *node)
                        .collect::<HashSet<_>>()
                })
                .collect::<Vec<_>>()
        });
        let items = self.key_tree_items(fragments.items);
        if structure_changed
            || items.len() != expected
            || items
                .iter()
                .any(|(key, _)| !self.scene_map.contains_key(key))
        {
            let Some(root_nodes) = root_nodes else {
                return false;
            };
            let mut replacements = Vec::new();
            for ((root, _), nodes) in fragments.roots.iter().zip(&root_nodes) {
                let Some(range) = self.fragment_spans.get(root).cloned() else {
                    return false;
                };
                let replacement: Vec<_> = items
                    .iter()
                    .filter(|(key, _)| nodes.contains(&key.node()))
                    .cloned()
                    .collect();
                replacements.push((range, replacement));
            }
            replacements.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
            for pair in replacements.windows(2) {
                if pair[1].0.end > pair[0].0.start {
                    return false;
                }
            }
            for (range, replacement) in replacements {
                let removed: HashSet<_> = self.cached_scene[range.clone()]
                    .iter()
                    .map(|(key, _)| key.node())
                    .filter(|node| self.tree.get(*node).is_none())
                    .collect();
                for node in removed {
                    if let Some(entry) = self.text_runs.remove(&node) {
                        self.runs.lock().remove(entry.run);
                    }
                    self.native_paint_revisions.remove(&node);
                    render_motion::forget_motion_node(&mut self.motion_keys, node);
                }
                self.cached_scene.splice(range, replacement);
            }
            let unwrapped = std::mem::take(&mut self.cached_scene)
                .into_iter()
                .filter(|(key, _)| !key.wraps_motion())
                .collect();
            self.cached_scene = self.wrap_motion_groups(unwrapped);
            self.index_fragments();
            self.cached_text_nodes = self
                .cached_scene
                .iter()
                .filter_map(|(key, item)| {
                    matches!(item, DisplayItem::Text { .. }).then_some(key.node())
                })
                .collect();
            self.scene_structure = Some(self.tree.structure_epoch());
            self.cached_version = Some(version);
            self.frame_work.tree_scene_ops_emitted += items.len();
            self.patched_topology = true;
            for (key, _) in &items {
                self.tree
                    .trace_work(key.node(), aqua_ui::program::WorkStage::ScenePatched);
            }
            return true;
        }
        for (key, item) in &items {
            let Some(slot) = self.fragment_slots.get(&key.node()).and_then(|slots| {
                slots
                    .iter()
                    .copied()
                    .find(|index| self.cached_scene[*index].0 == *key)
            }) else {
                return false;
            };
            self.cached_scene[slot].1 = item.clone();
            self.scene.update(self.scene_map[key], item.clone());
            self.tree
                .trace_work(key.node(), aqua_ui::program::WorkStage::ScenePatched);
        }
        self.frame_work.tree_scene_ops_emitted += items.len();
        self.frame_work.scene_items_reconciled += items.len();
        self.cached_version = Some(version);
        true
    }
}
