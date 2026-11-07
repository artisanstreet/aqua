use super::{DisplayItem, NodeId, NodeKind, Runtime, SceneKey};

#[derive(Clone, Copy)]
pub(super) struct FillSlot {
    generation: u32,
    cached: u32,
    scene: NodeId,
}

impl Runtime {
    pub(super) fn rebuild_fill_slots(&mut self) {
        self.fill_slots.clear();
        for (cached, (key, item)) in self.cached_scene.iter().enumerate() {
            let SceneKey::Tree { node, op: 0 } = *key else {
                continue;
            };
            if !matches!(item, DisplayItem::Rect { .. } | DisplayItem::Quad { .. }) {
                continue;
            }
            if !self.tree.get(node).is_some_and(
                |record| matches!(&record.kind, NodeKind::Box(content) if content.plain_paint()),
            ) {
                continue;
            }
            let Some(scene) = self.scene_map.get(key).copied() else {
                continue;
            };
            let Ok(cached) = u32::try_from(cached) else {
                continue;
            };
            let raw = node.raw();
            let index = raw.index as usize;
            if self.fill_slots.len() <= index {
                self.fill_slots.resize(index + 1, None);
            }
            self.fill_slots[index] = Some(FillSlot {
                generation: raw.generation,
                cached,
                scene,
            });
        }
    }

    pub(super) fn patch_fill_changes(&mut self) -> bool {
        let Some(changes) = self.tree.take_fill_changes() else {
            return false;
        };
        let valid = changes.iter().all(|patch| {
            self.fill_slots
                .get(patch.node.raw().index as usize)
                .and_then(|slot| *slot)
                .is_some_and(|slot| {
                    slot.generation == patch.node.raw().generation
                        && self.scene.get(slot.scene).is_some()
                })
        });
        if !valid {
            self.tree.recycle_fill_changes(changes);
            return false;
        }
        self.frame_work.scene_items_reconciled += changes.len();
        for patch in &changes {
            let slot = self.fill_slots[patch.node.raw().index as usize].unwrap();
            self.scene.update_fill(slot.scene, patch.fill);
            match &mut self.cached_scene[slot.cached as usize].1 {
                DisplayItem::Rect { fill, .. } | DisplayItem::Quad { fill, .. } => {
                    *fill = patch.fill
                }
                _ => unreachable!(),
            }
        }
        self.tree.recycle_fill_changes(changes);
        true
    }
}
