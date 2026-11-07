use aqua_core::{FillPatch, Oklch, SrgbaFillBatch};
use std::cell::{Cell, RefCell};

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

const CHANGE_LOG_LIMIT: usize = 2048;

#[repr(C)]
struct Node {
    generation: u32,
    alive: bool,
    fill_only: bool,
    changed_at: u64,
    fill_index: usize,
    item: DisplayItem,
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
    geometry_revision: u64,
    fills: Vec<FillPatch>,
    fill_capacity: usize,
    srgba_batches: Vec<SrgbaFillBatch>,
    nodes: Vec<Node>,
    free: Vec<(u32, u32)>, // (index, generation) reusable slots
    /// Painter order: node indices back-to-front.
    order: Vec<u32>,
    /// Position of each node index in `order`. Rebuilt when `order` changes.
    positions: RefCell<Vec<u32>>,
    order_dirty: Cell<bool>,
    log: Vec<(u64, u32)>,
    log_floor: u64,
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

    pub fn geometry_revision(&self) -> u64 {
        self.geometry_revision
    }

    /// Insert a node at the back (top) of painter order.
    pub fn insert(&mut self, item: DisplayItem) -> NodeId {
        self.revision += 1;
        self.geometry_revision = self.revision;
        let rev = self.revision;
        let (index, generation) = if let Some((index, generation)) = self.free.pop() {
            let node = &mut self.nodes[index as usize];
            node.generation = generation;
            node.alive = true;
            node.item = item;
            node.changed_at = rev;
            node.fill_only = false;
            self.order.push(index);
            (index, generation)
        } else {
            let index = self.nodes.len() as u32;
            self.nodes.push(Node {
                fill_only: false,
                fill_index: 0,
                generation: 0,
                alive: true,
                item,
                changed_at: rev,
            });
            self.order.push(index);
            (index, 0)
        };
        self.order_dirty.set(true);
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
        node.fill_only = false;
        self.geometry_revision = self.revision;
        node.changed_at = self.revision;
        self.record_change(raw.index);
        if !already_pending {
            self.mark_dirty(raw.index);
        }
        true
    }

    pub fn update_fill(&mut self, id: NodeId, fill: Oklch) -> bool {
        let raw = id.raw();
        let Some(node) = self
            .nodes
            .get_mut(raw.index as usize)
            .filter(|node| node.alive && node.generation == raw.generation)
        else {
            return false;
        };
        if let DisplayItem::SrgbaRect { bounds, .. } = &node.item {
            let bounds = *bounds;
            return self.update(
                id,
                DisplayItem::Rect {
                    bounds,
                    radius: 0.0,
                    fill,
                    border_width: 0.0,
                    border_color: Oklch::TRANSPARENT,
                    shadow: None,
                },
            );
        }
        let current = match &mut node.item {
            DisplayItem::Rect { fill, .. } | DisplayItem::Quad { fill, .. } => fill,
            _ => return false,
        };
        if *current == fill {
            return true;
        }
        *current = fill;
        let pending = node.changed_at > self.consumed_through;
        node.fill_only = !pending || node.fill_only;
        if node.fill_only {
            if pending {
                self.fills[node.fill_index].fill = fill;
            } else {
                if self.fills.capacity() == 0 {
                    self.fills.reserve_exact(self.fill_capacity);
                }
                node.fill_index = self.fills.len();
                self.fills.push(FillPatch { node: id, fill });
            }
        }
        self.revision += 1;
        node.changed_at = self.revision;
        if !pending {
            self.mark_dirty(raw.index);
        } else {
            self.record_change(raw.index);
        }
        true
    }

    pub fn apply_srgba_batch(&mut self, mut batch: SrgbaFillBatch) -> bool {
        let revision = self.revision + 1;
        let retain_history = batch.len() <= CHANGE_LOG_LIMIT;
        let mut changed = false;
        let mut geometry = false;
        let mut valid = true;
        batch.retain(|patch| {
            let raw = patch.node.raw();
            let Some(node) = self
                .nodes
                .get_mut(raw.index as usize)
                .filter(|node| node.alive && node.generation == raw.generation)
            else {
                valid = false;
                return false;
            };
            let pending = node.changed_at > self.consumed_through;
            let converted = match &mut node.item {
                DisplayItem::SrgbaRect { fill, .. } => {
                    if *fill == patch.fill {
                        return false;
                    }
                    *fill = patch.fill;
                    false
                }
                DisplayItem::Rect {
                    bounds,
                    radius: 0.0,
                    border_width: 0.0,
                    border_color,
                    shadow: None,
                    ..
                } if border_color.alpha == 0.0 => {
                    let bounds = *bounds;
                    node.item = DisplayItem::SrgbaRect {
                        bounds,
                        fill: patch.fill,
                    };
                    true
                }
                _ => {
                    valid = false;
                    return false;
                }
            };
            changed = true;
            geometry |= converted;
            node.fill_only = !converted && (!pending || node.fill_only);
            node.changed_at = revision;
            let journal = node.fill_only;
            if !pending {
                self.dirty.push(raw.index);
            }
            if retain_history {
                self.record_change_at(revision, raw.index);
            }
            journal
        });
        if changed {
            self.revision = revision;
            if !retain_history {
                self.log.clear();
                self.log_floor = revision;
            }
            if geometry {
                self.geometry_revision = revision;
            }
        }
        if !batch.is_empty() {
            self.srgba_batches.push(batch);
        }
        valid
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
        self.geometry_revision = self.revision;
        self.removed.push(std::mem::replace(
            &mut node.item,
            DisplayItem::PopClip, // placeholder; node is dead
        ));
        let next_gen = node.generation.wrapping_add(1);
        node.generation = next_gen;
        self.free.push((raw.index, next_gen));
        self.order.retain(|&i| i != raw.index);
        self.order_dirty.set(true);
        self.struct_version += 1;
        true
    }

    pub fn set_order(&mut self, ids: &[NodeId]) -> bool {
        if ids.len() != self.order.len() {
            return false;
        }
        let mut seen = std::collections::HashSet::with_capacity(ids.len());
        for id in ids {
            if self.get(*id).is_none() || !seen.insert(id.raw().index) {
                return false;
            }
        }
        let order: Vec<u32> = ids.iter().map(|id| id.raw().index).collect();
        if order != self.order {
            self.order = order;
            self.order_dirty.set(true);
            self.struct_version += 1;
            self.revision += 1;
            self.geometry_revision = self.revision;
        }
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

    pub fn item_revision(&self, id: NodeId) -> Option<u64> {
        let raw = id.raw();
        self.nodes
            .get(raw.index as usize)
            .filter(|n| n.alive && n.generation == raw.generation)
            .map(|n| n.changed_at)
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    fn mark_dirty(&mut self, index: u32) {
        self.dirty.push(index);
        self.record_change(index);
    }

    fn record_change(&mut self, index: u32) {
        self.record_change_at(self.revision, index);
    }

    fn record_change_at(&mut self, revision: u64, index: u32) {
        if self.log.last() == Some(&(revision, index)) {
            return;
        }
        if self.log.capacity() == 0 {
            self.log.reserve_exact(CHANGE_LOG_LIMIT * 2 + 1);
        }
        self.log.push((revision, index));
        if self.log.len() > CHANGE_LOG_LIMIT * 2 {
            let dropped = self.log.drain(..CHANGE_LOG_LIMIT).next_back();
            self.log_floor = dropped.map_or(self.log_floor, |(revision, _)| revision);
        }
    }

    fn refresh_positions(&self) {
        if !self.order_dirty.get() {
            return;
        }
        let mut positions = self.positions.borrow_mut();
        if positions.len() < self.nodes.len() {
            positions.resize(self.nodes.len(), u32::MAX);
        }
        for (pos, &idx) in self.order.iter().enumerate() {
            positions[idx as usize] = pos as u32;
        }
        self.order_dirty.set(false);
    }

    pub fn struct_version(&self) -> u64 {
        self.struct_version
    }

    pub fn item_at(&self, position: usize) -> Option<(NodeId, &DisplayItem)> {
        let index = *self.order.get(position)?;
        let node = &self.nodes[index as usize];
        Some((wrap(index, node.generation), &node.item))
    }

    pub fn indices(&self, from: usize, to: usize) -> Option<&[u32]> {
        self.order.get(from..to)
    }

    pub fn position(&self, id: NodeId) -> Option<usize> {
        let raw = id.raw();
        self.nodes
            .get(raw.index as usize)
            .filter(|n| n.alive && n.generation == raw.generation)?;
        self.refresh_positions();
        self.positions
            .borrow()
            .get(raw.index as usize)
            .map(|&position| position as usize)
            .filter(|&position| self.order.get(position) == Some(&raw.index))
    }

    pub fn changed_since(&self, revision: u64, mut visit: impl FnMut(usize)) -> bool {
        if revision < self.log_floor {
            return false;
        }
        self.refresh_positions();
        let positions = self.positions.borrow();
        let start = self.log.partition_point(|&(at, _)| at <= revision);
        for &(_, index) in &self.log[start..] {
            if let Some(&position) = positions.get(index as usize) {
                if self.order.get(position as usize) == Some(&index) {
                    visit(position as usize);
                }
            }
        }
        true
    }

    /// Drain nodes changed since the last drain, in painter order.
    /// Walks only the dirty queue (plus a position refresh when painter
    /// order changed), never the full node list. A trivial update yields a
    /// one-element set, never a full rebuild.
    pub fn drain_changes(&mut self) -> Vec<(NodeId, DisplayItem)> {
        self.srgba_batches.clear();
        self.fills.clear();
        self.refresh_positions();
        let mut pending: Vec<u32> = std::mem::take(&mut self.dirty);
        let positions = self.positions.borrow();
        pending.sort_by_key(|&i| positions.get(i as usize).copied().unwrap_or(u32::MAX));
        drop(positions);
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

    pub fn drain_paint_changes(&mut self) -> (Vec<(NodeId, DisplayItem)>, Vec<FillPatch>) {
        let (mut upserts, fills, batches) = self.drain_updates();
        if batches.is_empty() {
            return (upserts, fills);
        }
        for batch in batches {
            for patch in batch.iter() {
                if let Some(item) = self.get(patch.node) {
                    upserts.push((patch.node, item.clone()));
                }
            }
        }
        upserts.sort_by_key(|(node, _)| self.position(*node).unwrap_or(usize::MAX));
        upserts.dedup_by_key(|(node, _)| *node);
        (upserts, fills)
    }

    pub fn drain_updates(
        &mut self,
    ) -> (
        Vec<(NodeId, DisplayItem)>,
        Vec<FillPatch>,
        Vec<SrgbaFillBatch>,
    ) {
        let mut batches = std::mem::take(&mut self.srgba_batches);
        if self.geometry_revision <= self.consumed_through {
            self.fill_capacity = self.fill_capacity.max(self.fills.len());
            let fills = std::mem::take(&mut self.fills);
            self.dirty.clear();
            self.consumed_through = self.revision;
            return (Vec::new(), fills, batches);
        }
        self.fills.clear();
        for batch in &mut batches {
            batch.retain(|patch| {
                let raw = patch.node.raw();
                self.nodes.get(raw.index as usize).is_some_and(|node| {
                    node.alive
                        && node.generation == raw.generation
                        && node.fill_only
                        && node.changed_at > self.consumed_through
                        && matches!(node.item, DisplayItem::SrgbaRect { .. })
                })
            });
        }
        batches.retain(|batch| !batch.is_empty());
        let mut pending = std::mem::take(&mut self.dirty);
        let fill_capacity = pending
            .first()
            .and_then(|&index| self.nodes.get(index as usize))
            .filter(|node| node.fill_only)
            .map_or(0, |_| pending.len());
        let mut fills = Vec::with_capacity(fill_capacity);
        let mut full = Vec::new();
        for index in pending.drain(..) {
            let Some(node) = self
                .nodes
                .get(index as usize)
                .filter(|node| node.alive && node.changed_at > self.consumed_through)
            else {
                continue;
            };
            match (&node.item, node.fill_only) {
                (DisplayItem::SrgbaRect { .. }, true) => {}
                (DisplayItem::Rect { fill, .. } | DisplayItem::Quad { fill, .. }, true) => fills
                    .push(FillPatch {
                        node: wrap(index, node.generation),
                        fill: *fill,
                    }),
                _ => full.push(index),
            }
        }
        self.dirty = pending;
        self.refresh_positions();
        let positions = self.positions.borrow();
        full.sort_by_key(|&index| positions.get(index as usize).copied().unwrap_or(u32::MAX));
        full.dedup();
        let upserts = full
            .into_iter()
            .map(|index| {
                let node = &self.nodes[index as usize];
                (wrap(index, node.generation), node.item.clone())
            })
            .collect();
        self.consumed_through = self.revision;
        (upserts, fills, batches)
    }

    pub fn acknowledge_changes(&mut self) {
        self.srgba_batches.clear();
        self.fills.clear();
        self.dirty.clear();
        self.consumed_through = self.revision;
    }

    pub fn recycle_fills(&mut self, mut fills: Vec<FillPatch>) {
        fills.clear();
        if self.fills.is_empty() && fills.capacity() > self.fills.capacity() {
            self.fills = fills;
        }
    }

    pub fn drain_fill_changes(&mut self, since: u64) -> Option<Vec<FillPatch>> {
        if !self.srgba_batches.is_empty()
            || self.geometry_revision > since
            || self.consumed_through != since
        {
            return None;
        }
        let (upserts, fills) = self.drain_paint_changes();
        debug_assert!(upserts.is_empty());
        Some(fills)
    }

    pub fn drain_color_changes(
        &mut self,
        since: u64,
    ) -> Option<(Vec<FillPatch>, Vec<SrgbaFillBatch>)> {
        if self.geometry_revision > since || self.consumed_through != since {
            return None;
        }
        let (upserts, fills, batches) = self.drain_updates();
        debug_assert!(upserts.is_empty());
        Some((fills, batches))
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
