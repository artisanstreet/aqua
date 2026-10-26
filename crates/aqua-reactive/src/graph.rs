//! Internal dependency topology, ownership records, and scheduling state.
//!
//! Storage is dense: node records live in a `Vec` indexed by a compact slot
//! (`u32`); handles carry their slot alongside their [`NodeId`]. Slots are
//! recycled through a free list, and slot reuse can never alias a stale
//! handle because every access validates `records[idx].id == handle.id`.
//! The [`NodeId`] itself is monotonic and never reused, so diagnostics stay
//! unambiguous.
//!
//! Propagation avoids per-write hashing entirely: ranks are cached and only
//! recomputed after edge mutations; each write stamps the dirty subgraph
//! (version-stamped `Vec`, no `HashSet`) and scans the cached global order
//! (`Vec`, no sort, no Kahn).

use std::collections::{HashMap, VecDeque};

/// Stable identity for a graph node (source, derived value, or effect).
///
/// Ids are monotonically allocated and **never reused**, so a stale handle can
/// never silently alias a newer node, even across slot recycling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub u64);

pub(crate) type ScopeId = u64;
/// Dense slot index into [`Inner::records`]. Carried by handles; validated
/// against the slot's current [`NodeId`] on every access.
pub(crate) type Slot = u32;

#[derive(Clone, Copy)]
pub(crate) enum NodeKind {
    Source,
    Derived,
    Effect { queued: bool },
}

pub(crate) struct NodeRecord {
    pub id: NodeId,
    pub occupied: bool,
    pub scope: ScopeId,
    pub dependents: Vec<Slot>,
    pub dependencies: Vec<Slot>,
    /// Cached longest-path level from a source. Recomputed for the whole
    /// graph only when edges change (`ranks_dirty`); propagation scans the
    /// cached global order instead of running Kahn per write.
    pub rank: u32,
    /// Type-erased callback that marks the node's typed cell disposed.
    pub on_dispose: Option<Box<dyn Fn()>>,
    pub kind: NodeKind,
}

pub(crate) struct ScopeRecord {
    pub parent: Option<ScopeId>,
    pub children: Vec<ScopeId>,
    pub members: Vec<NodeId>,
    pub cleanups: Vec<Box<dyn FnOnce()>>,
    pub disposed: bool,
}

/// Deferred dependency-graph mutation requested by a dynamic (`select`) node.
///
/// Applied outside user-code execution, with cycle checking, so a dynamic
/// re-subscription can never corrupt an in-progress propagation.
#[derive(Debug)]
pub(crate) enum EdgeOp {
    Relink {
        node: NodeId,
        remove: Vec<NodeId>,
        add: Vec<NodeId>,
    },
}

pub(crate) struct Inner {
    pub next_node: u64,
    pub next_scope: u64,
    records: Vec<NodeRecord>,
    free: Vec<Slot>,
    id_to_slot: HashMap<NodeId, Slot>,
    pub scopes: HashMap<ScopeId, ScopeRecord>,
    pub queue: VecDeque<NodeId>,
    pub batch_depth: u32,
    pub batch_marks: Vec<usize>,
    pub flushing: bool,
    pub cycle_rejections: u64,
    pub flush_truncations: u64,
    /// Completions rejected as stale (superseded, cancelled, duplicate, or
    /// disposed). Inspected via `stats().stale_completions`.
    pub stale_completions: u64,
    /// Positive while a derived computation is evaluating. Signal writes are
    /// rejected in that window since derived values must stay pure and
    /// read-only, and the running thunk is temporarily checked out.
    pub derive_depth: u32,
    /// Set by any edge/node structural mutation; cleared by rank recompute.
    pub ranks_dirty: bool,
    /// Live slots sorted by `(rank, slot)`. Rebuilt when `ranks_dirty`.
    order: Vec<Slot>,
    stamps: Vec<u64>,
    stamp_cur: u64,
    /// Reused scratch buffers so propagation avoids per-write allocation.
    stack_scratch: Vec<Slot>,
    order_scratch: Vec<Slot>,
}

impl Inner {
    pub fn new() -> Self {
        Self {
            next_node: 1,
            next_scope: 1,
            records: Vec::new(),
            free: Vec::new(),
            id_to_slot: HashMap::new(),
            scopes: HashMap::new(),
            queue: VecDeque::new(),
            batch_depth: 0,
            batch_marks: Vec::new(),
            flushing: false,
            cycle_rejections: 0,
            flush_truncations: 0,
            stale_completions: 0,
            derive_depth: 0,
            ranks_dirty: true,
            order: Vec::new(),
            stamps: Vec::new(),
            stamp_cur: 1,
            stack_scratch: Vec::new(),
            order_scratch: Vec::new(),
        }
    }

    /// Resolve a [`NodeId`] to its dense slot. `None` when the node was
    /// disposed (entries are removed, not tombstoned).
    pub fn slot_of(&self, id: NodeId) -> Option<Slot> {
        self.id_to_slot.get(&id).copied()
    }

    /// Validate a handle's `(id, slot)` pair against slot recycling.
    pub fn handle_live(&self, id: NodeId, slot: Slot) -> bool {
        self.records
            .get(slot as usize)
            .is_some_and(|r| r.occupied && r.id == id)
    }

    pub fn record(&self, slot: Slot) -> Option<&NodeRecord> {
        self.records.get(slot as usize).filter(|r| r.occupied)
    }

    /// Copy of a slot's kind (`None` when the slot is free or recycled).
    pub fn kind_of(&self, slot: Slot) -> Option<NodeKind> {
        self.record(slot).map(|r| r.kind)
    }

    /// Current [`NodeId`] of a slot (`None` when free).
    pub fn id_of(&self, slot: Slot) -> Option<NodeId> {
        self.record(slot).map(|r| r.id)
    }

    /// Set an effect slot's queued flag.
    pub fn set_queued(&mut self, slot: Slot, queued: bool) {
        if let Some(rec) = self.record_mut(slot) {
            if matches!(rec.kind, NodeKind::Effect { .. }) {
                rec.kind = NodeKind::Effect { queued };
            }
        }
    }

    fn record_mut(&mut self, slot: Slot) -> Option<&mut NodeRecord> {
        self.records.get_mut(slot as usize).filter(|r| r.occupied)
    }

    /// Iterate live records (for stats/inspection).
    pub fn live_records(&self) -> impl Iterator<Item = &NodeRecord> {
        self.records.iter().filter(|r| r.occupied)
    }

    pub fn alloc_node(
        &mut self,
        scope: ScopeId,
        kind: NodeKind,
        on_dispose: Option<Box<dyn Fn()>>,
    ) -> (NodeId, Slot) {
        let id = NodeId(self.next_node);
        self.next_node += 1;
        let slot = match self.free.pop() {
            Some(s) => s,
            None => {
                let s = self.records.len() as Slot;
                self.records.push(NodeRecord {
                    id,
                    occupied: false,
                    scope,
                    dependents: Vec::new(),
                    dependencies: Vec::new(),
                    rank: 0,
                    on_dispose: None,
                    kind: NodeKind::Source,
                });
                self.stamps.push(0);
                s
            }
        };
        let rec = &mut self.records[slot as usize];
        *rec = NodeRecord {
            id,
            occupied: true,
            scope,
            dependents: Vec::new(),
            dependencies: Vec::new(),
            rank: 0,
            on_dispose,
            kind,
        };
        self.id_to_slot.insert(id, slot);
        // An isolated node has rank 0; existing ranks are unaffected.
        (id, slot)
    }

    /// Add a `dep -> dependent` edge. Returns `false` (and changes nothing)
    /// when either endpoint is missing/disposed or the edge would close a
    /// dependency cycle.
    pub fn add_edge_checked(&mut self, dep: NodeId, dependent: NodeId) -> bool {
        if dep == dependent {
            return false;
        }
        let (Some(&a), Some(&b)) = (self.id_to_slot.get(&dep), self.id_to_slot.get(&dependent))
        else {
            return false;
        };
        if !self.handle_live(dep, a) || !self.handle_live(dependent, b) {
            return false;
        }
        // Adding dep -> dependent is cyclic iff dependent already reaches dep.
        if self.reaches(dependent, dep) {
            return false;
        }
        if !self.records[a as usize].dependents.contains(&b) {
            self.records[a as usize].dependents.push(b);
        }
        if !self.records[b as usize].dependencies.contains(&a) {
            self.records[b as usize].dependencies.push(a);
        }
        self.ranks_dirty = true;
        true
    }

    /// True when `to` is reachable from `from` following dependents edges.
    pub fn reaches(&mut self, from: NodeId, to: NodeId) -> bool {
        if from == to {
            return true;
        }
        let (Some(&f), Some(&t)) = (self.id_to_slot.get(&from), self.id_to_slot.get(&to)) else {
            return false;
        };
        self.stamp_cur += 1;
        let mark = self.stamp_cur;
        let Self {
            records,
            stamps,
            stack_scratch: stack,
            ..
        } = self;
        stack.clear();
        stack.push(f);
        while let Some(s) = stack.pop() {
            if s == t {
                return true;
            }
            if stamps[s as usize] == mark {
                continue;
            }
            stamps[s as usize] = mark;
            stack.extend(records[s as usize].dependents.iter().copied());
        }
        false
    }

    pub fn remove_edge(&mut self, dep: NodeId, dependent: NodeId) {
        let a = self.id_to_slot.get(&dep).copied();
        let b = self.id_to_slot.get(&dependent).copied();
        let (Some(a), Some(b)) = (a, b) else { return };
        let before = self.records[a as usize].dependents.len();
        self.records[a as usize].dependents.retain(|d| *d != b);
        let changed_a = self.records[a as usize].dependents.len() != before;
        let before = self.records[b as usize].dependencies.len();
        self.records[b as usize].dependencies.retain(|d| *d != a);
        let changed_b = self.records[b as usize].dependencies.len() != before;
        if changed_a || changed_b {
            self.ranks_dirty = true;
        }
    }

    /// Full longest-path rank recompute over live nodes (Kahn). Runs only
    /// when `ranks_dirty` — i.e. after edge mutations, never per write —
    /// and rebuilds the cached global propagation order.
    fn recompute_ranks(&mut self) {
        let live: Vec<Slot> = self
            .records
            .iter()
            .enumerate()
            .filter(|(_, r)| r.occupied)
            .map(|(i, _)| i as Slot)
            .collect();
        let mut indeg = vec![0usize; self.records.len()];
        for &s in &live {
            let n = self.records[s as usize]
                .dependencies
                .iter()
                .filter(|d| self.records[**d as usize].occupied)
                .count();
            indeg[s as usize] = n;
        }
        for r in self.records.iter_mut().filter(|r| r.occupied) {
            r.rank = 0;
        }
        let mut ready: Vec<Slot> = live
            .iter()
            .copied()
            .filter(|s| indeg[*s as usize] == 0)
            .collect();
        while let Some(s) = ready.pop() {
            let rank = self.records[s as usize].rank;
            let dependents = self.records[s as usize].dependents.clone();
            for d in dependents {
                if !self.records[d as usize].occupied {
                    continue;
                }
                let r = self.records[d as usize].rank.max(rank + 1);
                self.records[d as usize].rank = r;
                indeg[d as usize] -= 1;
                if indeg[d as usize] == 0 {
                    ready.push(d);
                }
            }
        }
        let mut order = live;
        order.sort_by_key(|s| (self.records[*s as usize].rank, *s));
        self.order = order;
        self.ranks_dirty = false;
    }

    /// Fill `out` (cleared first) with the deterministic propagation order
    /// for the live transitive dependents of `start`, as dense slots in rank
    /// order: stamp the dirty subgraph with the version-stamped dense array,
    /// then filter the cached global order. No hashing, no per-write sort,
    /// no per-write Kahn. Pair with [`reclaim_order`](Self::reclaim_order)
    /// to reuse `out`'s capacity across writes.
    pub fn topo_into(&mut self, start: NodeId, out: &mut Vec<Slot>) {
        out.clear();
        if self.ranks_dirty {
            self.recompute_ranks();
        }
        let Some(&root) = self.id_to_slot.get(&start) else {
            return;
        };
        if !self.handle_live(start, root) {
            return;
        }
        self.stamp_cur += 1;
        let mark = self.stamp_cur;
        let Self {
            records,
            stamps,
            stack_scratch: stack,
            order,
            ..
        } = self;
        stack.clear();
        stack.push(root);
        while let Some(s) = stack.pop() {
            if stamps[s as usize] == mark {
                continue;
            }
            if !records[s as usize].occupied {
                continue;
            }
            stamps[s as usize] = mark;
            stack.extend(records[s as usize].dependents.iter().copied());
        }
        out.extend(
            order
                .iter()
                .copied()
                .filter(|s| stamps[*s as usize] == mark),
        );
    }

    /// Park a used order buffer for reuse by the next write.
    pub fn reclaim_order(&mut self, mut order: Vec<Slot>) {
        order.clear();
        self.order_scratch = order;
    }

    /// Take the parked order buffer (capacity reused across writes).
    pub fn take_order_buffer(&mut self) -> Vec<Slot> {
        std::mem::take(&mut self.order_scratch)
    }

    /// Remove a node by id: detach edges, free the slot for recycling, and
    /// report its scope plus disposer for the caller to finalize. Returns
    /// `None` when already gone. Dependents are NOT cascaded here — the
    /// caller drives the cascade so it can drop user closures unborrowed.
    pub fn detach_node(&mut self, id: NodeId) -> Option<Detached> {
        let slot = self.id_to_slot.remove(&id)?;
        let rec = self.records.get_mut(slot as usize)?;
        if !rec.occupied || rec.id != id {
            return None;
        }
        rec.occupied = false;
        let dependents = std::mem::take(&mut rec.dependents);
        let dependencies = std::mem::take(&mut rec.dependencies);
        let on_dispose = rec.on_dispose.take();
        let scope = rec.scope;
        let was_queued_effect = matches!(rec.kind, NodeKind::Effect { queued: true });
        for dep in &dependencies {
            if let Some(r) = self.records.get_mut(*dep as usize).filter(|r| r.occupied) {
                let s = slot;
                r.dependents.retain(|d| *d != s);
            }
        }
        for d in &dependents {
            if let Some(r) = self.records.get_mut(*d as usize).filter(|r| r.occupied) {
                let s = slot;
                r.dependencies.retain(|x| *x != s);
            }
        }
        if was_queued_effect {
            self.queue.retain(|q| *q != id);
        }
        if let Some(scope_rec) = self.scopes.get_mut(&scope) {
            scope_rec.members.retain(|m| *m != id);
        }
        self.free.push(slot);
        // Node removal can lower longest-path ranks of survivors.
        self.ranks_dirty = true;
        Some(Detached {
            dependents: dependents
                .iter()
                .filter_map(|s| {
                    self.records
                        .get(*s as usize)
                        .filter(|r| r.occupied)
                        .map(|r| r.id)
                })
                .collect(),
            on_dispose,
        })
    }

    /// Apply a deferred [`EdgeOp::Relink`]. The whole op is rejected (and
    /// counted) when any added edge would close a cycle; removals are only
    /// performed once all additions are proven acyclic.
    pub fn apply_relink(&mut self, node: NodeId, remove: &[NodeId], add: &[NodeId]) -> bool {
        let live = self
            .slot_of(node)
            .is_some_and(|s| self.handle_live(node, s));
        if !live {
            return false;
        }
        for a in add {
            let ok = self.slot_of(*a).is_some_and(|s| self.handle_live(*a, s));
            if !ok {
                return false;
            }
            if *a != node && self.reaches(node, *a) {
                self.cycle_rejections += 1;
                return false;
            }
        }
        for r in remove {
            self.remove_edge(*r, node);
        }
        let updated = self.add_edges_unchecked(node, add);
        if updated {
            self.ranks_dirty = true;
        }
        true
    }

    fn add_edges_unchecked(&mut self, node: NodeId, add: &[NodeId]) -> bool {
        let mut changed = false;
        for a in add {
            if *a == node {
                continue;
            }
            let (Some(&x), Some(&y)) = (self.id_to_slot.get(a), self.id_to_slot.get(&node)) else {
                continue;
            };
            if !self.records[x as usize].dependents.contains(&y) {
                self.records[x as usize].dependents.push(y);
                changed = true;
            }
            if !self.records[y as usize].dependencies.contains(&x) {
                self.records[y as usize].dependencies.push(x);
                changed = true;
            }
        }
        changed
    }

    /// Drop queued effects past `mark` (used when a batch unwinds), clearing
    /// their queued flags so a later queueing works normally.
    pub fn truncate_queue(&mut self, mark: usize) {
        while self.queue.len() > mark {
            if let Some(id) = self.queue.pop_back() {
                if let Some(s) = self.slot_of(id) {
                    if let Some(rec) = self.record_mut(s) {
                        if rec.id == id && matches!(rec.kind, NodeKind::Effect { .. }) {
                            rec.kind = NodeKind::Effect { queued: false };
                        }
                    }
                }
            }
        }
    }
}

/// The result of detaching one node: dependents to cascade to, and the
/// typed-cell disposer to run outside any borrow.
pub(crate) struct Detached {
    pub dependents: Vec<NodeId>,
    pub on_dispose: Option<Box<dyn Fn()>>,
}
