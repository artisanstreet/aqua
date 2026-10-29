pub use aqua_core::geometry::Rect;
pub use aqua_core::id::{NodeId, RawId};
pub use aqua_core::scene::{DisplayItem, TextRun};

/// Inflate a rect by `px` on all sides (effect sampling footprint).
pub fn inflate(r: Rect, px: f32) -> Rect {
    Rect {
        x: r.x - px,
        y: r.y - px,
        w: r.w + 2.0 * px,
        h: r.h + 2.0 * px,
    }
}

fn wrap(index: u32, generation: u32) -> NodeId {
    NodeId::from_raw(RawId { index, generation })
}

struct Node {
    generation: u32,
    alive: bool,
    item: DisplayItem,
    /// Last commit revision at which this node changed.
    changed_at: u64,
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("generation", &self.generation)
            .field("alive", &self.alive)
            .field("changed_at", &self.changed_at)
            .finish_non_exhaustive()
    }
}

/// Retained scene with compact storage and incremental commits.
///
/// Change discovery is a real dirty queue, not a scan: `insert` / `update`
/// push the node index onto a pending list (deduped via `changed_at`), and
/// `drain_changes` sorts only the pending list into painter order. Cost is
/// O(changed log changed + changed), independent of total scene size.
#[derive(Debug, Default)]
pub struct Scene {
    nodes: Vec<Node>,
    free: Vec<(u32, u32)>, // (index, generation) reusable slots
    /// Painter order: node indices back-to-front.
    order: Vec<u32>,
    /// Position of each node index in `order`. Rebuilt when `order` changes.
    positions: Vec<u32>,
    order_dirty: bool,
    /// Pending node indices changed since last drain (may contain stale or
    /// duplicate entries; both are filtered at drain time).
    dirty: Vec<u32>,
    /// Removed items since last drain (for damage repaint of old bounds).
    removed: Vec<DisplayItem>,
    revision: u64,
    /// Committed revision the renderer has consumed.
    consumed_through: u64,
    /// Bumped on insert/remove (painter-order structure changes), never on
    /// value updates. Lets the renderer take a fast path for trivial
    /// updates without walking the whole scene.
    struct_version: u64,
    consumed_struct_version: u64,
}

impl Scene {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Insert a node at the back (top) of painter order.
    pub fn insert(&mut self, item: DisplayItem) -> NodeId {
        self.revision += 1;
        let rev = self.revision;
        let (index, generation) = if let Some((index, generation)) = self.free.pop() {
            let node = &mut self.nodes[index as usize];
            node.generation = generation;
            node.alive = true;
            node.item = item;
            node.changed_at = rev;
            self.order.push(index);
            (index, generation)
        } else {
            let index = self.nodes.len() as u32;
            self.nodes.push(Node {
                generation: 0,
                alive: true,
                item,
                changed_at: rev,
            });
            self.order.push(index);
            (index, 0)
        };
        self.order_dirty = true;
        self.struct_version += 1;
        self.mark_dirty(index);
        wrap(index, generation)
    }

    /// Update a node's item. Returns `false` for stale/dead handles.
    /// Identical items do not bump the revision (no-op writes are free).
    pub fn update(&mut self, id: NodeId, item: DisplayItem) -> bool {
        let raw = id.raw();
        let Some(node) = self.nodes.get_mut(raw.index as usize) else {
            return false;
        };
        if !node.alive || node.generation != raw.generation {
            return false;
        }
        if node.item == item {
            return true;
        }
        // Dedupe: already pending stays a single queue entry.
        let already_pending = node.changed_at > self.consumed_through;
        self.revision += 1;
        node.item = item;
        node.changed_at = self.revision;
        if !already_pending {
            self.mark_dirty(raw.index);
        }
        true
    }

    /// Remove a node. Stale handles fail closed.
    pub fn remove(&mut self, id: NodeId) -> bool {
        let raw = id.raw();
        let Some(node) = self.nodes.get_mut(raw.index as usize) else {
            return false;
        };
        if !node.alive || node.generation != raw.generation {
            return false;
        }
        self.revision += 1;
        node.alive = false;
        self.removed.push(std::mem::replace(
            &mut node.item,
            DisplayItem::PopClip, // placeholder; node is dead
        ));
        let next_gen = node.generation.wrapping_add(1);
        node.generation = next_gen;
        self.free.push((raw.index, next_gen));
        self.order.retain(|&i| i != raw.index);
        self.order_dirty = true;
        self.struct_version += 1;
        true
    }

    pub fn get(&self, id: NodeId) -> Option<&DisplayItem> {
        let raw = id.raw();
        self.nodes
            .get(raw.index as usize)
            .filter(|n| n.alive && n.generation == raw.generation)
            .map(|n| &n.item)
    }

    /// Painter-ordered snapshot of live items. The renderer walks this with
    /// the dirty set from [`Scene::drain_changes`] to skip unchanged ranges.
    pub fn ordered(&self) -> impl Iterator<Item = (NodeId, &DisplayItem)> + '_ {
        self.order.iter().map(|&i| {
            let n = &self.nodes[i as usize];
            (wrap(i, n.generation), &n.item)
        })
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    fn mark_dirty(&mut self, index: u32) {
        self.dirty.push(index);
    }

    fn refresh_positions(&mut self) {
        if !self.order_dirty {
            return;
        }
        if self.positions.len() < self.nodes.len() {
            self.positions.resize(self.nodes.len(), u32::MAX);
        }
        for (pos, &idx) in self.order.iter().enumerate() {
            self.positions[idx as usize] = pos as u32;
        }
        self.order_dirty = false;
    }

    /// Drain nodes changed since the last drain, in painter order.
    /// Walks only the dirty queue (plus a position refresh when painter
    /// order changed), never the full node list. A trivial update yields a
    /// one-element set, never a full rebuild.
    pub fn drain_changes(&mut self) -> Vec<(NodeId, DisplayItem)> {
        self.refresh_positions();
        let mut pending: Vec<u32> = std::mem::take(&mut self.dirty);
        pending.sort_by_key(|&i| self.positions.get(i as usize).copied().unwrap_or(u32::MAX));
        pending.dedup();
        let mut out = Vec::with_capacity(pending.len());
        for i in pending {
            let Some(n) = self.nodes.get(i as usize) else {
                continue;
            };
            if !n.alive || n.changed_at <= self.consumed_through {
                continue;
            }
            out.push((wrap(i, n.generation), n.item.clone()));
        }
        self.consumed_through = self.revision;
        out
    }

    /// Drain items removed since the last drain (old bounds for repaint).
    pub fn drain_removed(&mut self) -> Vec<DisplayItem> {
        std::mem::take(&mut self.removed)
    }

    /// True when painter-order structure changed (insert/remove) since the
    /// last call. Consumes the flag. Value updates never set it.
    pub fn take_structure_changed(&mut self) -> bool {
        let changed = self.struct_version != self.consumed_struct_version;
        self.consumed_struct_version = self.struct_version;
        changed
    }

    pub fn pending_change_count(&self) -> usize {
        // Counts distinct pending indices without scanning all nodes.
        let mut ids: Vec<u32> = self.dirty.clone();
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter()
            .filter(|&i| {
                self.nodes
                    .get(i as usize)
                    .is_some_and(|n| n.alive && n.changed_at > self.consumed_through)
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aqua_core::Oklch;

    fn red() -> Oklch {
        Oklch::from_hex("#ff0000").unwrap()
    }

    fn rect_item(x: f32) -> DisplayItem {
        DisplayItem::Rect {
            bounds: Rect {
                x,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            },
            radius: 2.0,
            fill: red(),
            border_width: 0.0,
            border_color: red(),
            shadow: None,
        }
    }

    #[test]
    fn trivial_update_yields_single_change() {
        let mut scene = Scene::new();
        for i in 0..1000 {
            scene.insert(rect_item(i as f32));
        }
        scene.drain_changes();
        assert_eq!(scene.pending_change_count(), 0);
        // Change one node: exactly one pending change, revision bumps once.
        let target = scene.ordered().nth(500).unwrap().0;
        let before = scene.revision();
        assert!(scene.update(target, rect_item(9999.0)));
        assert_eq!(scene.revision(), before + 1);
        assert_eq!(scene.pending_change_count(), 1);
        let changes = scene.drain_changes();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, target);
    }

    #[test]
    fn noop_update_is_free() {
        let mut scene = Scene::new();
        let item = rect_item(1.0);
        let id = scene.insert(item.clone());
        scene.drain_changes();
        let before = scene.revision();
        assert!(scene.update(id, item));
        assert_eq!(scene.revision(), before);
        assert_eq!(scene.pending_change_count(), 0);
    }

    #[test]
    fn stale_handle_fails_closed() {
        let mut scene = Scene::new();
        let id = scene.insert(rect_item(0.0));
        assert!(scene.remove(id));
        // Reuse slot with a new node; old handle must not update it.
        let id2 = scene.insert(rect_item(5.0));
        assert_ne!(id.raw().generation, id2.raw().generation);
        assert!(!scene.update(id, rect_item(7.0)));
        assert!(scene.get(id).is_none());
    }
}
