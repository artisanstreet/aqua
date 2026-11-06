//! View: scope-owned bindings over a mounted element tree (macro-agent owned).
//!
//! A [`View`] owns the [`Scope`] whose effects drive one [`UiTree`]:
//! mounting lowers [`Element`]s to [`NodeSpec`]s, applies each element's
//! bindings exactly once per node id, and routes updates through the shared
//! [`CommandSink`] queue (drained by the tree at the layout boundary —
//! effects never touch the tree, so there is no reentrancy and no `RefCell`
//! around tree state).
//!
//! Per-row scopes (keyed lists): the view holds one scope, but hosts may bind
//! row subtrees under child scopes via [`bind_text`] / [`bind_text_editable`]
//! and dispose the row scope when the keyed reconciler drops the row's
//! [`NodeId`]. Reconciliation reuses node ids, so disposal is keyed to the
//! same ids — see the `row_scope_dispose_kills_row_effect` test.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::rc::Rc;

use aqua_core::{CollectionSemantics, NodeId, Point, RawId, Role, Size};
use aqua_reactive::{ReadSignal, Scope, WriteSignal};

use crate::collection_runtime::{
    spacer_spec, CollectionDescriptor, CollectionLayout, CollectionRevealAlignment, CollectionRole,
    CollectionRowRegistration, CollectionRowSources, CollectionSource, CollectionState,
};
use crate::element::{
    lower_root_in, lower_specs, lower_specs_in, splice_top_level, Element, ElementBinding,
    LowerError, TypoChain,
};
use crate::layout::TextMeasurer;
use crate::style::{Axis, LayoutStyle, Length, Paint};
use crate::tree::{
    ActionEvent, ActionKind, BoxContent, CommandSink, NodeKind, NodeSpec, ScrollContent, UiCommand,
    UiTree,
};
use crate::virtualization::RowRange;

/// Mount/update failures. Fragments vanish at lowering, so structural
/// problems fail here, loudly, instead of mounting a wrong tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountError {
    /// No content to mount.
    EmptyRoot,
    /// A fresh root must be a single node; wrap siblings in a `Box`.
    MultipleRoots {
        /// Lowered top-level spec count.
        count: usize,
    },
    /// Element lowering failed (keyed fragment, sourceless dynamic).
    Lower(LowerError),
}

impl fmt::Display for MountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MountError::EmptyRoot => write!(f, "no elements to mount"),
            MountError::MultipleRoots { count } => write!(
                f,
                "cannot mount {count} top-level nodes as one root; \
                 wrap them in a Box"
            ),
            MountError::Lower(e) => write!(f, "element lowering failed: {e}"),
        }
    }
}

impl std::error::Error for MountError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            MountError::Lower(e) => Some(e),
            _ => None,
        }
    }
}

impl From<LowerError> for MountError {
    fn from(e: LowerError) -> Self {
        MountError::Lower(e)
    }
}

pub struct View {
    scope: Scope,
    sink: CommandSink,
    root: Option<NodeId>,
    bound: HashMap<RawId, BoundState>,
    content: Vec<Element>,
    collections: HashMap<RawId, CollectionMount>,
    layout_viewport: Option<Size>,
}

struct BoundState {
    /// Child scope owning exactly this node's effects. Disposed when the
    /// node is removed or its sources change (stale sources stop); dropping
    /// the scope would leak, so disposal is always explicit.
    scope: Scope,
    /// Reactive source identities the live effects subscribe, in order.
    /// Compared across syncs to decide whether effects stay or re-attach.
    effects: Vec<aqua_reactive::NodeId>,
}

// ---------------------------------------------------------------------------
// Collection driver (View-owned realization over `CollectionState`)
// ---------------------------------------------------------------------------

/// Maximum layout passes per [`View::compute_layout`] call: one initial
/// layout plus bounded re-settles as measured heights converge. Never loops
/// on row counts.
const COLLECTION_SETTLE_PASSES: usize = 3;

/// Retained per-row descriptor: the built [`Element`] (bindings intact
/// until first bound) plus its mounted node, if any.
///
/// Fresh descriptors are bound exactly once, at insertion; relayouts,
/// scrolls, and reorders reuse nodes and scopes untouched. Rebinding a
/// retained element whose bindings were already taken would compute
/// `effect_keys == []` and dispose live signal subscriptions, so the driver
/// only ever binds elements taken from the fresh-build path below — never
/// these retained ones.
struct RetainedEl {
    element: Element,
    node: Option<NodeId>,
    /// Spec margins at last assemble (committed margins minus driver-folded
    /// gap): the exact inputs for stride measurement, with no float
    /// round-trip through committed styles.
    margins: (f32, f32),
}

/// One window-managed collection container.
///
/// The shell (container + empty children) comes from normative lowering; the
/// driver owns everything inside: header/row/spacer realization, bindings,
/// scroll write-back, pinning, measurement, and semantics. Keyed by the
/// container's [`RawId`].
struct CollectionMount {
    /// Container node (List: the Scroll/Box itself; Table: the root Box).
    container: NodeId,
    /// Virtualization state machine (prefix sums, anchor, pin, window).
    state: CollectionState,
    /// Descriptor this state was built/refreshed from (refresh detection).
    last_desc: CollectionDescriptor,
    /// Effective typography chain at the collection position, threaded
    /// through the bind walk exactly like normative lowering (each level
    /// composes its explicit overrides over its parent's chain). Row/header
    /// rebuilds lower under this chain, so inherited text styling survives
    /// scrolls and refreshes like every other subtree.
    typo: TypoChain,
    /// Retained header (tables; `None` for lists and until built).
    header: Option<RetainedHeader>,
    /// Resident rows: index into the current snapshot source.
    rows: HashMap<usize, RetainedEl>,
    /// Column plan the retained elements were built with. Plan adoption
    /// compares `column_widths` only (never `content_width`, which merely
    /// echoes the available width for lists): lists never rebuild on width
    /// change, tables rebuild only when tracks actually resolve differently.
    /// Adoption settles within the mounting frame's bounded passes — never
    /// deferred, so idle apps still paint correct first-frame columns.
    built_plan: CollectionLayout,
    probe_width: f32,
    probe_changed: bool,
    spacers_stale: bool,
    width_node: Option<NodeId>,
    /// Mirrored pin (the state holds the authoritative one; the mirror
    /// detects focus changes without a state getter).
    pin: Option<u64>,
    /// A refreshed descriptor restored the scroll from the reading anchor:
    /// the next realization writes it tree-ward first, then re-reads the
    /// actual clamped offset. Ordinary frames never write — the tree offset
    /// is authoritative input there.
    scroll_restore_pending: bool,
    last_tree_scroll: Option<f32>,
}

/// Retained table header: the bar element plus its mounted nodes
/// (headers usually lower to one bar Box holding the header cells).
struct RetainedHeader {
    element: Element,
    nodes: Vec<NodeId>,
}

/// Child slots of one body realization, in paint order.
enum BodyTag {
    Spacer,
    Width,
    Row(usize),
}

fn same_shape(prev: &CollectionDescriptor, next: &CollectionDescriptor) -> bool {
    prev.shape_epoch.is_some()
        && prev.shape_epoch == next.shape_epoch
        && prev.source.len() == next.source.len()
        && prev.virtualized == next.virtualized
        && prev.estimated_row_height.to_bits() == next.estimated_row_height.to_bits()
        && prev.overscan == next.overscan
        && prev.overscan_pixels == next.overscan_pixels
        && prev.fixed_row_height == next.fixed_row_height
        && prev.measured_uniform_rows == next.measured_uniform_rows
        && prev.width_probe == next.width_probe
        && prev.initial_height.is_some() == next.initial_height.is_some()
        && prev.styles == next.styles
}

fn carry_rows_in_place(
    previous: &dyn CollectionSource,
    next: &dyn CollectionSource,
    rows: HashMap<usize, RetainedEl>,
    alive: impl Fn(&RetainedEl) -> bool,
) -> HashMap<usize, RetainedEl> {
    rows.into_iter()
        .filter(|(index, row)| {
            alive(row)
                && previous.row_revision(*index).is_some()
                && previous.row_revision(*index) == next.row_revision(*index)
                && crate::tree::hash_key(previous.key(*index))
                    == crate::tree::hash_key(next.key(*index))
        })
        .collect()
}

fn carry_rows(
    previous: &dyn CollectionSource,
    next: &dyn CollectionSource,
    rows: HashMap<usize, RetainedEl>,
    alive: impl Fn(&RetainedEl) -> bool,
) -> HashMap<usize, RetainedEl> {
    let mut candidates: HashMap<u64, (u64, RetainedEl)> = HashMap::new();
    for (index, row) in rows {
        if index >= previous.len() || !alive(&row) {
            continue;
        }
        if let Some(revision) = previous.row_revision(index) {
            candidates.insert(crate::tree::hash_key(previous.key(index)), (revision, row));
        }
    }
    if candidates.is_empty() {
        return HashMap::new();
    }
    let mut carried = HashMap::with_capacity(candidates.len());
    for index in 0..next.len() {
        let key = crate::tree::hash_key(next.key(index));
        let matches = candidates
            .get(&key)
            .is_some_and(|(revision, _)| next.row_revision(index) == Some(*revision));
        if matches {
            if let Some((_, row)) = candidates.remove(&key) {
                carried.insert(index, row);
            }
        }
    }
    carried
}

/// Whether `next` describes the same snapshot as `prev`: identical source
/// object plus identical policy and container styles. Same-source renders
/// skip the `O(n)` refresh entirely; anything else refreshes once.
fn same_desc(prev: &CollectionDescriptor, next: &CollectionDescriptor) -> bool {
    Rc::ptr_eq(&prev.source, &next.source)
        && prev.virtualized == next.virtualized
        && prev.estimated_row_height.to_bits() == next.estimated_row_height.to_bits()
        && prev.overscan == next.overscan
        && prev.overscan_pixels == next.overscan_pixels
        && prev.fixed_row_height == next.fixed_row_height
        && prev.measured_uniform_rows == next.measured_uniform_rows
        && prev.width_probe == next.width_probe
        && match (&prev.on_viewport, &next.on_viewport) {
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
        && match (&prev.initial_height, &next.initial_height) {
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
        && prev.styles == next.styles
}

impl View {
    pub fn observe_collection_scrolls(&mut self, tree: &UiTree) {
        for mount in self.collections.values_mut() {
            if !matches!(
                mount.state.follow_state(),
                crate::collection_runtime::FollowState::Tail { .. }
            ) {
                continue;
            }
            let body = match mount.state.descriptor().source.role() {
                CollectionRole::List => Some(mount.container),
                CollectionRole::Table => tree
                    .get(mount.container)
                    .and_then(|record| record.children.get(1))
                    .copied(),
            };
            let Some(offset) = body.and_then(|body| tree.scroll_offset(body)) else {
                continue;
            };
            if mount
                .last_tree_scroll
                .is_some_and(|previous| previous != offset.y)
            {
                mount.state.set_scroll(offset.x, offset.y);
            }
            mount.last_tree_scroll = Some(offset.y);
        }
    }

    pub fn scroll_collection_to(
        &mut self,
        tree: &mut UiTree,
        container: NodeId,
        offset: Point,
    ) -> bool {
        let Some(mount) = self.collections.get_mut(&container.raw()) else {
            return false;
        };
        let body = match mount.state.descriptor().source.role() {
            CollectionRole::List => Some(container),
            CollectionRole::Table => tree
                .get(container)
                .and_then(|record| record.children.get(1))
                .copied(),
        };
        let Some(body) = body else {
            return false;
        };
        if tree.get(body).is_none() || !offset.x.is_finite() || !offset.y.is_finite() {
            return false;
        }
        tree.scroll_to(body, offset);
        let Some(actual) = tree.scroll_offset(body) else {
            return false;
        };
        mount.state.scroll_to(actual.x, actual.y);
        mount.last_tree_scroll = Some(actual.y);
        true
    }

    pub fn collection_follow_state(
        &self,
        container: NodeId,
    ) -> Option<crate::collection_runtime::FollowState> {
        Some(self.collections.get(&container.raw())?.state.follow_state())
    }

    pub fn set_collection_follow_mode(
        &mut self,
        tree: &mut UiTree,
        container: NodeId,
        mode: crate::collection_runtime::FollowMode,
    ) -> bool {
        let Some(mount) = self.collections.get_mut(&container.raw()) else {
            return false;
        };
        if tree.get(container).is_none() {
            return false;
        }
        mount.state.set_follow_mode(mode);
        mount.scroll_restore_pending = false;
        tree.mark_layout_dirty(container);
        true
    }

    pub fn pause_collection_following_tail(
        &mut self,
        tree: &mut UiTree,
        container: NodeId,
    ) -> bool {
        let Some(mount) = self.collections.get_mut(&container.raw()) else {
            return false;
        };
        if tree.get(container).is_none() {
            return false;
        }
        mount.state.pause_following_tail();
        tree.mark_layout_dirty(container);
        true
    }

    pub fn update_collection_item_height(
        &mut self,
        tree: &mut UiTree,
        collection_key: impl Into<crate::element::Key>,
        index: usize,
        height: f32,
    ) -> bool {
        let key = crate::tree::hash_key(collection_key.into());
        let Some(mount) = self.collections.values_mut().find(|mount| {
            tree.get(mount.container)
                .is_some_and(|record| record.key == Some(key))
        }) else {
            return false;
        };
        let changed = mount.state.note_measured(index, height, 0.0, 0.0);
        mount.scroll_restore_pending |= changed;
        if changed {
            mount.spacers_stale = true;
            tree.mark_layout_dirty(mount.container);
        }
        changed
    }

    pub fn reveal_collection_item(
        &mut self,
        tree: &mut UiTree,
        collection_key: impl Into<crate::element::Key>,
        index: usize,
    ) -> bool {
        self.reveal_collection_item_aligned(
            tree,
            collection_key,
            index,
            CollectionRevealAlignment::Nearest,
        )
    }

    pub fn reveal_collection_item_aligned(
        &mut self,
        tree: &mut UiTree,
        collection_key: impl Into<crate::element::Key>,
        index: usize,
        alignment: CollectionRevealAlignment,
    ) -> bool {
        let nodes = self.collection_nodes_for_key(tree, &collection_key.into());
        let [container] = nodes.as_slice() else {
            return false;
        };
        let container = *container;
        self.reveal_collection_index_aligned(tree, container, index, alignment)
    }

    pub fn collection_nodes_for_key(
        &self,
        tree: &UiTree,
        key: &crate::element::Key,
    ) -> Vec<NodeId> {
        let key = crate::tree::hash_key(key.clone());
        self.collections
            .values()
            .filter_map(|mount| {
                tree.get(mount.container)
                    .filter(|record| record.key == Some(key))
                    .map(|_| mount.container)
            })
            .collect()
    }

    pub fn collection_item_target_aligned(
        &self,
        tree: &UiTree,
        collection_key: impl Into<crate::element::Key>,
        index: usize,
        alignment: CollectionRevealAlignment,
    ) -> Option<Point> {
        let nodes = self.collection_nodes_for_key(tree, &collection_key.into());
        let [container] = nodes.as_slice() else {
            return None;
        };
        self.collection_index_target_aligned(tree, *container, index, alignment)
    }

    pub fn collection_index_target_aligned(
        &self,
        tree: &UiTree,
        container: NodeId,
        index: usize,
        alignment: CollectionRevealAlignment,
    ) -> Option<Point> {
        let mount = self.collections.get(&container.raw())?;
        let body = match mount.state.descriptor().source.role() {
            CollectionRole::List => container,
            CollectionRole::Table => *tree.get(container)?.children.get(1)?,
        };
        let rect = tree.rect(body)?;
        if !rect.h.is_finite() || rect.h <= 0.0 {
            return None;
        }
        let content = tree.content_size(body)?;
        let scroll = tree.scroll_offset(body)?;
        let y = mount.state.reveal_target_at(index, alignment, scroll.y)?;
        Some(Point::new(scroll.x, y.min((content.h - rect.h).max(0.0))))
    }

    pub fn reveal_collection_index(
        &mut self,
        tree: &mut UiTree,
        container: NodeId,
        index: usize,
    ) -> bool {
        self.reveal_collection_index_aligned(
            tree,
            container,
            index,
            CollectionRevealAlignment::Nearest,
        )
    }

    pub fn reveal_collection_index_aligned(
        &mut self,
        tree: &mut UiTree,
        container: NodeId,
        index: usize,
        alignment: CollectionRevealAlignment,
    ) -> bool {
        let Some(mount) = self.collections.get_mut(&container.raw()) else {
            return false;
        };
        let body = match mount.state.descriptor().source.role() {
            CollectionRole::List => container,
            CollectionRole::Table => match tree
                .get(container)
                .and_then(|record| record.children.get(1))
                .copied()
            {
                Some(body) => body,
                None => return false,
            },
        };
        if !tree
            .rect(body)
            .is_some_and(|rect| rect.h.is_finite() && rect.h > 0.0)
            || tree.content_size(body).is_none()
            || !mount.state.reveal_index_aligned(index, alignment)
        {
            return false;
        }
        let (x, y) = mount.state.scroll();
        tree.scroll_to(body, Point::new(x, y));
        true
    }

    /// Create a view driven by `scope`, pushing updates into `sink`
    /// (usually `tree.command_sink()`).
    pub fn new(scope: Scope, sink: CommandSink) -> Self {
        scope.set_invalidation_counter(sink.invalidation_counter());
        Self {
            scope,
            sink,
            root: None,
            bound: HashMap::new(),
            content: Vec::new(),
            collections: HashMap::new(),
            layout_viewport: None,
        }
    }

    pub fn bind_existing(
        &mut self,
        tree: &mut UiTree,
        root: NodeId,
        element: Element,
    ) -> Result<(), MountError> {
        if tree.get(root).is_none() {
            return Err(MountError::EmptyRoot);
        }
        let mut content = splice_top_level(vec![element])?;
        if content.len() != 1 {
            return Err(MountError::MultipleRoots {
                count: content.len(),
            });
        }
        self.dispose_bound();
        self.root = Some(root);
        bind_new(
            &self.scope,
            &self.sink,
            &mut self.bound,
            &mut self.collections,
            tree,
            root,
            &mut content[0],
            &TypoChain::default(),
        )?;
        self.content = content;
        Ok(())
    }

    /// Mounted root, if any.
    #[must_use]
    pub fn root(&self) -> Option<NodeId> {
        self.root
    }

    /// The view's scope (create child row scopes from this).
    #[must_use]
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    /// The shared command queue.
    #[must_use]
    pub fn sink(&self) -> &CommandSink {
        &self.sink
    }

    /// Set the whole content: first call mounts (single root required),
    /// later calls refresh the root in place (single element) or keyed-sync
    /// the root's children. Returns the node ids paired with the top-level
    /// elements.
    ///
    /// Top-level fragments splice structurally before anything else, so
    /// node↔element pairing is always positional. Reused node ids keep
    /// their effects while the sources are identical; changed sources
    /// dispose the stale child scope and re-attach without losing the node,
    /// focus, or caret.
    pub fn set_content(
        &mut self,
        tree: &mut UiTree,
        elements: Vec<Element>,
    ) -> Result<Vec<NodeId>, MountError> {
        // Normalize first: splice top-level fragments (keyed multi-child
        // fragments error here, loudly, instead of vanishing a key).
        self.content = splice_top_level(elements)?;
        let specs = lower_all(&self.content)?;
        aqua_core::checkpoint::reach();
        // `synced` tracks whether the tree already reflects the lowered
        // specs, so a refreshed root spec is never synced as a child of
        // itself.
        let mut synced = false;
        match self.root {
            None => {
                if specs.len() != 1 {
                    return Err(if specs.is_empty() {
                        MountError::EmptyRoot
                    } else {
                        MountError::MultipleRoots { count: specs.len() }
                    });
                }
                self.root = Some(tree.mount_spec(into_single(specs)));
            }
            Some(r) if tree.get(r).is_none() => {
                // Root was removed out-of-band: remount from scratch.
                self.dispose_bound();
                if specs.len() != 1 {
                    self.root = None;
                    return Err(if specs.is_empty() {
                        MountError::EmptyRoot
                    } else {
                        MountError::MultipleRoots { count: specs.len() }
                    });
                }
                self.root = Some(tree.mount_spec(into_single(specs)));
            }
            Some(r) => {
                // A single root refreshes in place — props AND children,
                // inside `refresh_mounted`. Anything else syncs the specs
                // as the root's children. Never both: the refreshed root
                // spec must not be synced as a child of itself.
                if self.content.len() == 1 {
                    let same_key = tree.get(r).is_some_and(|record| record.key == specs[0].key);
                    let refreshed = same_key
                        && self.content[0].refresh_mounted(tree, r, &TypoChain::default())?;
                    aqua_core::checkpoint::reach();
                    match refreshed {
                        true => synced = true, // children already synced inside
                        false => {
                            // Kind changed: replace the whole root.
                            self.dispose_bound();
                            tree.remove(r);
                            let specs = lower_all(&self.content)?;
                            let fresh = tree.mount_spec(into_single(specs));
                            self.root = Some(fresh);
                            synced = true; // mounted whole subtree; skip sync
                        }
                    }
                }
                if !synced {
                    let r = self.root.unwrap();
                    let specs = lower_all(&self.content)?;
                    tree.sync_children(r, specs);
                }
            }
        }
        aqua_core::checkpoint::reach();
        let root = self.root.unwrap();
        // Pair nodes with elements positionally (fragments already spliced).
        // Collection elements pair their container node with the window
        // driver (`bind_collection` registers the mount; realization and row
        // binding happen in `compute_layout`). Managed children are never
        // asserted against element children here.
        let Self {
            scope,
            sink,
            bound,
            content,
            collections,
            ..
        } = self;
        if content.len() == 1 {
            bind_new(
                scope,
                sink,
                bound,
                collections,
                tree,
                root,
                &mut content[0],
                &TypoChain::default(),
            )?;
        } else {
            let kids = tree.content_children(root);
            debug_assert_eq!(kids.len(), content.len());
            for (kid, el) in kids.iter().zip(content.iter_mut()) {
                bind_new(
                    scope,
                    sink,
                    bound,
                    collections,
                    tree,
                    *kid,
                    el,
                    &TypoChain::default(),
                )?;
            }
        }
        // Prune bookkeeping AND scopes for removed subtrees (plus dead
        // collection mounts): without this, effects outlive their rows and
        // leak until the whole view disposes.
        aqua_core::checkpoint::reach();
        self.prune_dead(tree);
        Ok(self.paired_ids(tree, root))
    }

    /// Refresh one mounted subtree in place from a freshly built element,
    /// leaving every other node, scope, and collection mount untouched.
    /// Returns `Ok(false)` when the subtree cannot be refreshed in place
    /// (node gone, key or kind changed, unreachable through retained
    /// content such as collection rows): callers fall back to
    /// [`View::set_content`].
    pub fn refresh_subtree(
        &mut self,
        tree: &mut UiTree,
        node: NodeId,
        element: Element,
    ) -> Result<bool, MountError> {
        let Some(root) = self.root else {
            return Ok(false);
        };
        let mut spliced = splice_top_level(vec![element])?;
        let Some(mut element) = spliced.pop().filter(|_| spliced.is_empty()) else {
            return Ok(false);
        };
        let Some(record) = tree.get(node) else {
            return Ok(false);
        };
        if record.key != element.key_hash() {
            return Ok(false);
        }
        let mut path = vec![node];
        while let Some(parent) = tree.get(*path.last().unwrap()).and_then(|r| r.parent) {
            path.push(parent);
        }
        path.reverse();
        if path.first() != Some(&root) || self.content.len() != 1 {
            return Ok(false);
        }
        let before = subtree_ids(tree, node);
        let Self {
            scope,
            sink,
            bound,
            content,
            collections,
            ..
        } = self;
        let mut inherited = TypoChain::default();
        let mut current = &mut content[0];
        for pair in path.windows(2) {
            let kids = tree.content_children(pair[0]);
            let Some(index) = kids.iter().position(|kid| *kid == pair[1]) else {
                return Ok(false);
            };
            inherited = inherited.child(&current.typo_chain());
            let Some(next) = current.flat_children_mut().into_iter().nth(index) else {
                return Ok(false);
            };
            current = next;
        }
        if !element.refresh_mounted(tree, node, &inherited)? {
            return Ok(false);
        }
        bind_new(
            scope,
            sink,
            bound,
            collections,
            tree,
            node,
            &mut element,
            &inherited,
        )?;
        *current = element;
        let after: HashSet<RawId> = subtree_ids(tree, node).into_iter().collect();
        for raw in before.into_iter().filter(|raw| !after.contains(raw)) {
            if let Some(state) = self.bound.remove(&raw) {
                state.scope.dispose();
            }
            self.collections.remove(&raw);
        }
        Ok(true)
    }

    /// Dispose every per-node child scope. Used before remounts and when a
    /// root vanishes: dropping the map would leak the effects (dropping a
    /// `Scope` does not dispose it — reactive ownership is explicit).
    /// Collection mounts are dropped with it (fresh states register on the
    /// remount in the same call).
    fn dispose_bound(&mut self) {
        for (_, state) in self.bound.drain() {
            state.scope.dispose();
        }
        self.collections.clear();
    }

    /// Dispose binding scopes (and drop collection mounts) for subtrees no
    /// longer in the tree. Runs after every structural sync so unmounted
    /// rows never leak subscriptions.
    fn prune_dead(&mut self, tree: &UiTree) {
        let live: HashSet<RawId> = tree.traverse().into_iter().map(|n| n.raw()).collect();
        let dead: Vec<RawId> = self
            .bound
            .keys()
            .filter(|k| !live.contains(k))
            .copied()
            .collect();
        for k in dead {
            if let Some(state) = self.bound.remove(&k) {
                state.scope.dispose();
            }
        }
        let dead_mounts: Vec<RawId> = self
            .collections
            .keys()
            .filter(|k| !live.contains(k))
            .copied()
            .collect();
        for k in dead_mounts {
            self.collections.remove(&k);
        }
    }

    /// Realize collections, run layout, and settle measured heights.
    ///
    /// This is the frozen runtime seam
    /// (`View::compute_layout(&mut UiTree, Size, &dyn TextMeasurer)`): hosts
    /// call it instead of `UiTree::compute_layout` when a view exists, and
    /// fall back to direct tree layout only when no view is mounted.
    /// Collection windows are realized from actual scroll offsets and
    /// committed viewport bounds BEFORE layout; measured row heights feed
    /// back through a bounded number of re-settles after. Clean mounts cost
    /// no factories, no syncs, and no writes, so paint-only frames keep the
    /// tree fast path.
    ///
    /// Every bounded pass re-ingests the committed viewport and re-realizes
    /// regardless of measurement: the first pass runs pre-layout (unknown
    /// rects, minimal anchor window) and later passes expand to the true
    /// window once rects commit — even when measured heights exactly equal
    /// estimates. The loop stops only when measurement, window, plan, and
    /// structure are all quiet, and a layout always follows the last sync.
    pub fn compute_layout(
        &mut self,
        tree: &mut UiTree,
        viewport: Size,
        measurer: &dyn TextMeasurer,
    ) -> Result<(), String> {
        self.compute_layout_with_row_sources(tree, viewport, measurer, &mut |sources| {
            if sources.is_empty() {
                Ok(())
            } else {
                Err("collection row sources require a registry".into())
            }
        })
    }

    pub fn compute_layout_with_row_sources(
        &mut self,
        tree: &mut UiTree,
        viewport: Size,
        measurer: &dyn TextMeasurer,
        register: &mut dyn FnMut(CollectionRowSources) -> Result<(), String>,
    ) -> Result<(), String> {
        self.compute_layout_with_row_registration(tree, viewport, measurer, &mut |registration| {
            if !registration.batches.is_empty() {
                return Err("collection row batches require a batch registry".into());
            }
            register(registration.sources)
        })
    }

    pub fn compute_layout_with_row_registration(
        &mut self,
        tree: &mut UiTree,
        viewport: Size,
        measurer: &dyn TextMeasurer,
        register: &mut dyn FnMut(CollectionRowRegistration) -> Result<(), String>,
    ) -> Result<(), String> {
        let viewport_unchanged = self.layout_viewport == Some(viewport);
        self.layout_viewport = Some(viewport);
        if self.collections.values().any(|mount| {
            mount.state.descriptor().source.width_sensitive()
                && (!viewport_unchanged || mount.state.viewport_width() <= 0.0)
        }) {
            tree.compute_layout(viewport, measurer)?;
            aqua_core::checkpoint::reach();
        }
        for mount in self.collections.values_mut() {
            let width = if let Some(index) = mount.state.descriptor().width_probe {
                let row = mount
                    .state
                    .descriptor()
                    .source
                    .probe_row(index, &mount.built_plan)
                    .map_err(|error| error.to_string())?;
                let mut specs =
                    lower_specs_in(&row, &mount.typo).map_err(|error| error.to_string())?;
                if specs.len() != 1 {
                    return Err("collection width probe must lower to one root".into());
                }
                UiTree::measure_intrinsic_width(
                    specs.remove(0),
                    measurer,
                    tree.gpui_layout_scale(),
                )?
            } else {
                0.0
            };
            if !width.is_finite() || width < 0.0 {
                return Err("collection width probe returned an invalid width".into());
            }
            if mount.probe_width != width {
                mount.probe_width = width;
                mount.probe_changed = true;
            }
        }
        let raws: Vec<RawId> = self.collections.keys().copied().collect();
        let mut structure_changed = false;
        for raw in raws {
            structure_changed |= self.realize_one(tree, viewport, raw, register)?;
        }
        aqua_core::checkpoint::reach();
        if structure_changed {
            self.prune_dead(tree);
        }
        tree.compute_layout(viewport, measurer)?;
        aqua_core::checkpoint::reach();
        for _ in 0..COLLECTION_SETTLE_PASSES {
            let measured = self.measure_all(tree);
            for mount in self.collections.values_mut() {
                mount.state.settle_following();
            }
            self.write_back_all(tree)?;
            let raws: Vec<RawId> = self.collections.keys().copied().collect();
            let mut moved = false;
            for raw in raws {
                moved |= self.realize_one(tree, viewport, raw, register)?;
            }
            if moved {
                self.prune_dead(tree);
            }
            if !measured && !moved {
                break;
            }
            tree.compute_layout(viewport, measurer)?;
            aqua_core::checkpoint::reach();
        }
        for mount in self.collections.values_mut() {
            mount.state.mark_settled();
        }
        Ok(())
    }

    /// Feed committed row geometry back into stride sums. Heights come from
    /// committed rects; margins come from the spec values recorded at
    /// assemble time — never committed margins, which carry the
    /// driver-folded gap and would double-count it through the float
    /// round-trip. Sanitizer mirrors the state seam exactly (non-finite
    /// falls back to zero; negative margins pass through — the source
    /// allows them whenever the total stride stays valid).
    /// Returns whether any stride changed (drives one more bounded settle
    /// pass, never row factories).
    fn measure_all(&mut self, tree: &UiTree) -> bool {
        let mut changed = false;
        for mount in self.collections.values_mut() {
            for (idx, ret) in mount.rows.iter() {
                let Some(node) = ret.node else { continue };
                let Some(rect) = tree.rect(node) else {
                    continue;
                };
                if tree.get(node).is_none() {
                    continue;
                }
                let (mt, mb) = ret.margins;
                let mt = if mt.is_finite() { mt } else { 0.0 };
                let mb = if mb.is_finite() { mb } else { 0.0 };
                if mount.state.note_measured(*idx, rect.h, mt, mb) {
                    changed = true;
                }
            }
        }
        changed
    }

    /// Write state-side scroll adjustments (refresh anchor restores,
    /// measured-height shifts above the viewport) tree-ward. Compared first
    /// so clean passes write nothing. Runs only after measurement — never
    /// at ordinary layout, where the tree offset is authoritative input.
    fn write_back_all(&mut self, tree: &mut UiTree) -> Result<(), String> {
        let raws: Vec<RawId> = self.collections.keys().copied().collect();
        for raw in raws {
            let (container, role) = match self.collections.get(&raw) {
                Some(m) => (m.container, m.state.descriptor().source.role()),
                None => continue,
            };
            if tree.get(container).is_none() {
                continue;
            }
            let body = match role {
                CollectionRole::List => container,
                CollectionRole::Table => match table_shell(tree, container) {
                    Ok((_, b)) => b,
                    Err(e) => return Err(e),
                },
            };
            let (sx, sy) = match self.collections.get(&raw) {
                Some(m) => m.state.scroll(),
                None => continue,
            };
            let (tx, ty) = tree
                .scroll_offset(body)
                .map(|p| (p.x, p.y))
                .unwrap_or((sx, sy));
            if sx != tx || sy != ty {
                tree.scroll_to(body, Point::new(sx, sy));
            }
            if let Some(mount) = self.collections.get_mut(&raw) {
                mount.last_tree_scroll = tree.scroll_offset(body).map(|point| point.y);
            }
        }
        Ok(())
    }

    /// Realize one collection mount for the current frame. Returns whether it
    /// synced (structural change). Reads actual scroll offsets and committed
    /// bounds; writes only on change; calls row/header factories only for
    /// newly resident rows (or after descriptor/plan invalidation).
    #[allow(clippy::too_many_lines)]
    fn realize_one(
        &mut self,
        tree: &mut UiTree,
        viewport: Size,
        raw: RawId,
        register: &mut dyn FnMut(CollectionRowRegistration) -> Result<(), String>,
    ) -> Result<bool, String> {
        // Liveness first: a container removed out-of-band drops its mount
        // (its scopes die in the prune pass).
        let container = match self.collections.get(&raw).map(|m| m.container) {
            Some(c) if tree.get(c).is_some() => c,
            _ => {
                self.collections.remove(&raw);
                return Ok(false);
            }
        };
        let role = self
            .collections
            .get(&raw)
            .map(|m| m.state.descriptor().source.role());
        let Some(role) = role else {
            self.collections.remove(&raw);
            return Ok(false);
        };
        // Resolve the scroll body (and header clip for tables) from the
        // finalized shell structure. Lists manage the container directly;
        // tables manage the header clip plus the body viewport inside it.
        // Table hosts are driver-created once (shell containers lower
        // empty): an empty or wiped container syncs [clip, body] first.
        // The body is a Scroll in both modes (see table_host_specs), so
        // horizontal overflow scrolls with virtualization off too.
        let mut hosts_created = false;
        if role == CollectionRole::Table {
            let kids = tree
                .get(container)
                .map(|r| r.children.clone())
                .unwrap_or_default();
            if kids.len() != 2 {
                tree.sync_children(container, table_host_specs());
                hosts_created = true;
            }
        }
        let (body, clip) = match role {
            CollectionRole::List => (container, None),
            CollectionRole::Table => {
                let (clip, body) = table_shell(tree, container)?;
                (body, Some(clip))
            }
        };
        if tree.get(body).is_none() {
            self.collections.remove(&raw);
            return Ok(false);
        }
        // The internal row stack lays out with Taffy gap zero (row gaps are
        // folded into strides and applied as explicit trailing margins
        // below). Enforce changed-only: authored container gaps return on
        // every refresh render, then settle here exactly once. Table
        // containers keep their own gap (header/clip rhythm is separate).
        ensure_stack_gap_zero(tree, body);
        let width_sensitive = self
            .collections
            .get(&raw)
            .is_some_and(|mount| mount.state.descriptor().source.width_sensitive());
        let committed_width = if width_sensitive {
            let node = self
                .collections
                .get(&raw)
                .and_then(|mount| mount.width_node)
                .filter(|node| tree.get(*node).is_some());
            let Some(node) = node else {
                let height = self
                    .collections
                    .get(&raw)
                    .map_or(0.0, |mount| mount.state.total_height());
                let mut spacer = spacer_spec(height);
                spacer.style.width = Length::Pct(100.0);
                let nodes = tree.sync_children(body, vec![spacer]);
                if let Some(mount) = self.collections.get_mut(&raw) {
                    mount.width_node = nodes.first().copied();
                    mount.rows.clear();
                }
                return Ok(true);
            };
            let Some(rect) = tree.rect(node) else {
                return Ok(false);
            };
            if !rect.w.is_finite() || rect.w <= 0.0 {
                return Ok(false);
            }
            Some(rect.w)
        } else {
            None
        };
        // Pin from keyboard focus: the focused node walks up to a direct
        // body child; its key pins that row in isolation. Focus anywhere
        // else clears the pin.
        let pin = focused_row_key(tree, body);
        // Actual viewport (§6 rule): body committed width if finite and
        // positive, else the UiApp viewport width; body committed height if
        // constrained (below both the window and the total stride estimate),
        // else the UiApp viewport height. Unconstrained Auto bodies expand
        // to content, so the clamps bound the window (a root virtual list
        // with no explicit height at 600x400 mounts ~30 rows, never 100k).
        // Unknown before first layout falls back to UiApp dims, settling to
        // the true window within this call's bounded passes.
        let total = self
            .collections
            .get(&raw)
            .map(|m| m.state.total_height())
            .unwrap_or(0.0);
        let rect = tree.rect(body);
        let vw = committed_width.unwrap_or(match rect {
            Some(r) if r.w.is_finite() && r.w > 0.0 && r.w <= viewport.w => r.w,
            _ => viewport.w,
        });
        let vh = match rect {
            Some(r) if r.h.is_finite() && r.h > 0.0 && r.h < viewport.h && r.h < total => r.h,
            _ => viewport.h,
        };
        let (vw, vh) = (vw.max(0.0), vh.max(0.0));
        // Committed scroll is read fresh inside the mutate block below
        // (after any pending anchor write-back), so the tree stays
        // authoritative at ordinary layout.

        // --- mutate mount (short borrows, then release) ---
        // Scroll discipline: at ordinary layout the tree offset is
        // authoritative — ingest it first and never overwrite user scroll.
        // State-side adjustments (refresh anchor restore, measured-height
        // shifts) write tree-ward ONLY when flagged pending by the refresh
        // or measure passes, then re-read the actual clamped offset.
        // Plan handling: recompute from the committed width once known and
        // adopt on `column_widths` change only (lists merely echo the
        // available width, so width changes never rebuild list rows).
        // Adoption settles inside this frame's bounded passes — never
        // deferred, so the first paint already uses the true width plan.
        enum PlanAction {
            Keep,
            Adopt(CollectionLayout),
        }
        let plan_action = {
            let Some(m) = self.collections.get_mut(&raw) else {
                return Ok(false);
            };
            if m.pin != pin {
                m.pin = pin;
                m.state.set_pinned_key(pin);
            }
            if m.scroll_restore_pending && m.state.is_following_tail() {
                m.scroll_restore_pending = false;
            }
            if m.scroll_restore_pending {
                m.scroll_restore_pending = false;
                let (sx, sy) = m.state.scroll();
                let (tx, ty) = tree
                    .scroll_offset(body)
                    .map(|p| (p.x, p.y))
                    .unwrap_or((sx, sy));
                if sx != tx || sy != ty {
                    tree.scroll_to(body, Point::new(sx, sy));
                }
            }
            if let Some((sx, sy)) = tree.scroll_offset(body).map(|p| (p.x, p.y)) {
                let (cx, cy) = m.state.scroll();
                let tail_mode = matches!(
                    m.state.follow_state(),
                    crate::collection_runtime::FollowState::Tail { .. }
                );
                if (sx != cx || sy != cy)
                    && (!tail_mode || m.last_tree_scroll.is_none_or(|previous| previous != sy))
                {
                    m.state.set_scroll(sx, sy);
                }
                m.last_tree_scroll = Some(sy);
            }
            m.state.set_viewport(vw, vh);
            m.state.settle_following();
            if vw > 0.0 {
                let p = m.state.descriptor().source.layout(vw);
                let source = &m.state.descriptor().source;
                if p.column_widths != m.built_plan.column_widths
                    || (width_sensitive
                        && source.width_class(p.content_width)
                            != source.width_class(m.built_plan.content_width))
                {
                    PlanAction::Adopt(p)
                } else {
                    // Tracks identical: refresh the stored plan (content
                    // width may echo a new available width) without
                    // rebuilding any rows.
                    m.built_plan = p;
                    PlanAction::Keep
                }
            } else {
                PlanAction::Keep
            }
        };
        // Descriptor for factory calls (cloned Rc, no mount borrow held).
        let source = match self.collections.get(&raw) {
            Some(m) => Rc::clone(&m.state.descriptor().source),
            None => return Ok(false),
        };
        if let PlanAction::Adopt(p) = plan_action {
            if let Some(m) = self.collections.get_mut(&raw) {
                m.built_plan = p;
                m.rows.clear();
                m.header = None;
            }
        }
        // Desired runs (pin included in isolation) versus residents.
        let (window, runs): (RowRange, Vec<RowRange>) = {
            let Some(m) = self.collections.get(&raw) else {
                return Ok(false);
            };
            let window = m.state.desired_window();
            let runs = m.state.mounted_runs(window);
            (window, runs)
        };
        let mut desired: HashSet<usize> = HashSet::new();
        for r in &runs {
            desired.extend(r.start..r.end);
        }
        if source.stable_indexed() {
            let mut keys = HashSet::with_capacity(desired.len());
            for &index in &desired {
                if !keys.insert(crate::tree::hash_key(source.key(index))) {
                    return Err(
                        crate::collection_runtime::CollectionError::DuplicateKey.to_string()
                    );
                }
            }
        }
        let resident: HashSet<usize> = match self.collections.get(&raw) {
            Some(m) => m.rows.keys().copied().collect(),
            None => return Ok(false),
        };
        // Recompute the fresh set after the sweep above: evicted and dead
        // residents rebuild through factories exactly once.
        let mut fresh_idx: Vec<usize> = match self.collections.get(&raw) {
            Some(m) => desired
                .iter()
                .copied()
                .filter(|index| {
                    !m.rows
                        .get(index)
                        .is_some_and(|row| row.node.is_some_and(|node| tree.get(node).is_some()))
                })
                .collect(),
            None => return Ok(false),
        };
        fresh_idx.sort_unstable();
        let built_plan = match self.collections.get(&raw) {
            Some(m) => m.built_plan.clone(),
            None => return Ok(false),
        };
        // Fresh descriptors: one factory call per new resident, retained for
        // binding and pure spec rebuilds. Never rebuilt on scroll/relayout.
        let mut fresh_rows: Vec<(usize, Element)> = Vec::with_capacity(fresh_idx.len());
        let mut registration = CollectionRowRegistration::default();
        for idx in fresh_idx {
            aqua_core::checkpoint::reach();
            let (prepared, batches) = source
                .prepare_row_with_batches(idx, &built_plan)
                .map_err(|error| error.to_string())?;
            registration.sources.extend(prepared.selection_sources);
            registration.batches.extend(batches);
            let row_el = prepared.element.with_key(source.key(idx));
            fresh_rows.push((idx, row_el));
        }
        let mut fresh_header: Option<Element> = None;
        if role == CollectionRole::Table {
            let has_header = matches!(self.collections.get(&raw), Some(m) if m.header.is_some());
            if !has_header {
                if let Some(h_el) = source.header(&built_plan) {
                    fresh_header = Some(h_el);
                }
            }
        }
        let mut dropped = false;
        if let Some(m) = self.collections.get_mut(&raw) {
            for g in resident.difference(&desired).copied().collect::<Vec<_>>() {
                m.rows.remove(&g);
                dropped = true;
            }
            // Residents whose nodes died out-of-band remount fresh below
            // (factories + binds) instead of lingering silently stale: a
            // shallow spec over a dead node would mount orphaned content
            // with no subscriptions.
            let dead: Vec<usize> = m
                .rows
                .iter()
                .filter_map(|(idx, ret)| match ret.node {
                    Some(n) if tree.get(n).is_some() => None,
                    _ => Some(*idx),
                })
                .collect();
            for d in dead {
                m.rows.remove(&d);
                dropped = true;
            }
        }
        let probe_changed = self
            .collections
            .get_mut(&raw)
            .is_some_and(|m| std::mem::take(&mut m.probe_changed));
        let spacers_stale = self
            .collections
            .get_mut(&raw)
            .is_some_and(|m| std::mem::take(&mut m.spacers_stale));
        let structural = hosts_created
            || !fresh_rows.is_empty()
            || dropped
            || fresh_header.is_some()
            || probe_changed
            || spacers_stale;
        // Header joint horizontal scroll: the header row lives outside the
        // vertical body viewport (in the clip sibling), so the body's scroll
        // offset is mirrored onto it as a negative leading margin
        // (changed-only; plain Boxes need no offset and stay at zero).
        if role == CollectionRole::Table {
            let body_x = tree.scroll_offset(body).map(|p| p.x).unwrap_or(0.0);
            if let Some(m) = self.collections.get(&raw) {
                if let Some(h) = m.header.as_ref() {
                    for node in h.nodes.iter().copied() {
                        let cur = tree.get(node).map(|r| r.style.margin.left).unwrap_or(0.0);
                        if cur != -body_x {
                            if let Some(rec) = tree.get(node) {
                                let mut style = rec.style.clone();
                                style.margin.left = -body_x;
                                tree.set_style(node, style);
                            }
                        }
                    }
                }
            }
        }
        if !structural {
            return Ok(false);
        }
        // Assemble body children: header (clip side) is synced separately
        // below; here the body runs in order with exact spacer spans.
        let (len, typo) = match self.collections.get(&raw) {
            Some(m) => (m.state.len(), m.typo.clone()),
            None => return Ok(false),
        };
        // Re-lower retained elements purely (no factories) plus fresh builds.
        let mut child_specs = Vec::new();
        let mut tags: Vec<BodyTag> = Vec::new();
        let mut row_margins: HashMap<usize, (f32, f32)> = HashMap::new();
        // Fast lookup for fresh elements by index.
        let mut fresh_by_idx: HashMap<usize, Element> = HashMap::new();
        for (idx, el) in fresh_rows {
            fresh_by_idx.insert(idx, el);
        }
        // NOTE: retained elements are re-lowered through a shared borrow
        // below; fresh elements lower from the local map. Both are pure.
        {
            let Some(m) = self.collections.get(&raw) else {
                return Ok(false);
            };
            let mut prev_end = 0usize;
            // Gap accounting (frozen scheme): the internal stack's Taffy gap
            // is zero, so every logical row adjacency carries its gap
            // explicitly. Mounted rows carry it as trailing bottom margin
            // (see the margin fixup below); spacers absorb the boundary gap
            // of the rows they stand in for — every non-tail span plus one
            // gap, the tail span exact. Totals reconcile to total - gap.
            let gap = m.state.gap();
            for (ri, run) in runs.iter().enumerate() {
                let gap_start = if ri == 0 { 0 } else { prev_end };
                if run.start > gap_start {
                    let h = m.state.span_height(gap_start, run.start) + gap;
                    if h > 0.0 {
                        child_specs.push(spacer_spec(h));
                        tags.push(BodyTag::Spacer);
                    }
                }
                for idx in run.start..run.end {
                    aqua_core::checkpoint::reach();
                    let mut rs = if let Some(el) = fresh_by_idx.get(&idx) {
                        // Fresh descriptors lower fully (bind + mount below).
                        let rs = lower_specs_in(el, &typo).map_err(|e| e.to_string())?;
                        debug_assert_eq!(rs.len(), 1);
                        row_margins.insert(
                            idx,
                            (
                                rs.first().map(|s| s.style.margin.top).unwrap_or(0.0),
                                rs.first().map(|s| s.style.margin.bottom).unwrap_or(0.0),
                            ),
                        );
                        tags.push(BodyTag::Row(idx));
                        rs
                    } else if let Some(ret) = m.rows.get(&idx) {
                        let rs = lower_root_in(&ret.element, &typo).map_err(|e| e.to_string())?;
                        debug_assert_eq!(rs.len(), 1);
                        row_margins.insert(
                            idx,
                            (
                                rs.first().map(|s| s.style.margin.top).unwrap_or(0.0),
                                rs.first().map(|s| s.style.margin.bottom).unwrap_or(0.0),
                            ),
                        );
                        let mut shallow = rs.into_iter().next().unwrap();
                        if let Some(node) = ret.node {
                            if let Some(live) = tree.get(node) {
                                match (&mut shallow.kind, &live.kind) {
                                    (NodeKind::Text(ref mut fresh_tc), NodeKind::Text(live_tc)) => {
                                        if !live_tc.is_editable() {
                                            fresh_tc.set_text(live_tc.text());
                                            fresh_tc.text_style = live_tc.text_style.clone();
                                            fresh_tc.spans = live_tc.spans.clone();
                                            fresh_tc.font_size = live_tc.font_size;
                                            fresh_tc.color = live_tc.color;
                                            fresh_tc.selectable = live_tc.selectable;
                                            fresh_tc.selection = live_tc.selection;
                                            fresh_tc.selection_source =
                                                live_tc.selection_source.clone();
                                            fresh_tc.wrap = live_tc.wrap;
                                            fresh_tc.password = live_tc.password;
                                            fresh_tc.placeholder = live_tc.placeholder.clone();
                                        }
                                    }
                                    (
                                        NodeKind::Canvas(ref mut fresh_c),
                                        NodeKind::Canvas(live_c),
                                    ) => {
                                        fresh_c.painter = live_c.painter;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        shallow.children.clear();
                        shallow.collection_managed = true;
                        tags.push(BodyTag::Row(idx));
                        vec![shallow]
                    } else {
                        continue;
                    };
                    if m.state.descriptor().fixed_row_height {
                        for spec in &mut rs {
                            spec.style.height =
                                Length::Px(m.state.descriptor().estimated_row_height);
                            spec.style.flex_shrink = 0.0;
                        }
                    }
                    child_specs.extend(rs);
                }
                prev_end = run.end;
            }
            if prev_end < len {
                let h = m.state.span_height(prev_end, len);
                if h > 0.0 {
                    child_specs.push(spacer_spec(h));
                    tags.push(BodyTag::Spacer);
                }
            }
            if m.probe_width > 0.0 {
                let mut spacer = spacer_spec(0.0);
                spacer.style.width = Length::Px(m.probe_width);
                spacer.style.align_self = Some(taffy::style::AlignSelf::FLEX_START);
                child_specs.push(spacer);
                tags.push(BodyTag::Spacer);
            }
            if width_sensitive {
                let mut spacer = spacer_spec(0.0);
                spacer.style.width = Length::Pct(100.0);
                child_specs.push(spacer);
                tags.push(BodyTag::Width);
            }
        }
        if !registration.sources.is_empty() || !registration.batches.is_empty() {
            register(registration)?;
        }
        let ids = tree.sync_children(body, child_specs);
        // Gap values for the fixup below (copied out before the split).
        let (gap_v, total_n) = match self.collections.get(&raw) {
            Some(m) => (m.state.gap(), m.state.len()),
            None => return Ok(false),
        };
        // Pair nodes to slots; bind fresh descriptors exactly once.
        let Self {
            scope,
            sink,
            bound,
            collections,
            ..
        } = self;
        let mut cursor = 0usize;
        let mut row_nodes: Vec<(usize, NodeId)> = Vec::new();
        for tag in tags {
            match tag {
                BodyTag::Spacer => {
                    cursor += 1;
                }
                BodyTag::Width => {
                    if let Some(mount) = collections.get_mut(&raw) {
                        mount.width_node = ids.get(cursor).copied();
                    }
                    cursor += 1;
                }
                BodyTag::Row(idx) => {
                    let node = ids[cursor];
                    cursor += 1;
                    row_nodes.push((idx, node));
                    let margins = row_margins.get(&idx).copied().unwrap_or((0.0, 0.0));
                    if let Some(el) = fresh_by_idx.remove(&idx) {
                        let mut el = el;
                        bind_new(scope, sink, bound, collections, tree, node, &mut el, &typo)
                            .map_err(|e| e.to_string())?;
                        if let Some(m) = collections.get_mut(&raw) {
                            m.rows.insert(
                                idx,
                                RetainedEl {
                                    element: el,
                                    node: Some(node),
                                    margins,
                                },
                            );
                        }
                    } else if let Some(m) = collections.get_mut(&raw) {
                        if let Some(ret) = m.rows.get_mut(&idx) {
                            ret.node = Some(node);
                            ret.margins = margins;
                        }
                    }
                }
            }
        }
        // Explicit gap fixup: with a zero stack gap, every logical adjacency
        // carries its gap as trailing row margin (all rows but the logical
        // final one). Changed-only writes; stable afterwards.
        for (idx, node) in &row_nodes {
            if let Some((_, spec_mb)) = row_margins.get(idx) {
                let want = spec_mb + if *idx + 1 < total_n { gap_v } else { 0.0 };
                let cur = tree
                    .get(*node)
                    .map(|r| r.style.margin.bottom)
                    .unwrap_or(f32::NAN);
                if cur != want {
                    if let Some(rec) = tree.get(*node) {
                        let mut style = rec.style.clone();
                        style.margin.bottom = want;
                        tree.set_style(*node, style);
                    }
                }
            }
        }
        // Header lives in the clip sibling (tables): sync + bind + record.
        // The header factory runs at initial/width/source changes only;
        // retained headers are never rebuilt on scroll.
        if role == CollectionRole::Table {
            if let (Some(clip_id), Some(h_el)) = (clip, fresh_header) {
                let hs = lower_specs_in(&h_el, &typo).map_err(|e| e.to_string())?;
                let hids = tree.sync_children(clip_id, hs);
                // Tables lower one bar Box per the module contract; bind it
                // directly so header actions and signals attach. (Multi-node
                // custom headers still mount; only the bar binds today.)
                let mut el = h_el;
                if hids.len() == 1 {
                    bind_new(
                        scope,
                        sink,
                        bound,
                        collections,
                        tree,
                        hids[0],
                        &mut el,
                        &typo,
                    )
                    .map_err(|e| e.to_string())?;
                }
                if let Some(m) = collections.get_mut(&raw) {
                    m.header = Some(RetainedHeader {
                        element: el,
                        nodes: hids,
                    });
                }
            }
        }
        // Semantics + bookkeeping.
        {
            let Some(m) = collections.get(&raw) else {
                return Ok(false);
            };
            let n = m.state.len();
            let col_count = m.built_plan.column_widths.len();
            apply_collection_semantics(tree, m, container, &row_nodes, role, n, col_count);
        }
        if let Some(m) = self.collections.get_mut(&raw) {
            m.state.set_mounted(window);
        }
        Ok(true)
    }
}

/// Build the table internal hosts: the header clip Box plus the body
/// viewport, in order. The body is a `Scroll` in BOTH modes (flex 1,
/// min-height 0 — fill the remaining container height without forcing
/// it): horizontal overflow scrolls even with virtualization off, which is
/// a realization policy only (off mounts every row through the same row
/// factories). Otherwise wide cells in an unvirtualized table would be
/// unreachable. Bodies stack vertically (`Column` — rows keep their own
/// `Row` axis internally) and align children to start on the cross axis:
/// rows keep their natural plan widths so wide content overflows into real
/// horizontal scroll range instead of being cross-stretched to the
/// viewport. The clip fills the container width, carries clip paint, and
/// never shrinks, so the header keeps its natural measured height and
/// clips horizontal overflow.
/// Empty (no factories, no bindings); the driver fills both sides. All
/// driver-managed.
fn table_host_specs() -> Vec<NodeSpec> {
    use taffy::style::{AlignItems, AlignSelf};

    let clip_style = LayoutStyle {
        flex_shrink: 0.0,
        align_self: Some(AlignSelf::STRETCH),
        ..Default::default()
    };
    let clip_paint = Paint {
        clip: true,
        ..Default::default()
    };
    let mut clip = NodeSpec::new(
        NodeKind::Box(BoxContent::new(clip_paint.clone())),
        clip_style,
    );
    clip.paint = clip_paint;
    clip.collection_managed = true;
    let body_style = LayoutStyle {
        axis: Axis::Column,
        align: AlignItems::FLEX_START,
        flex_grow: 1.0,
        min_height: Length::Px(0.0),
        ..Default::default()
    };
    let mut body = NodeSpec::new(
        NodeKind::Scroll(ScrollContent {
            offset: Point::ZERO,
        }),
        body_style,
    );
    body.collection_managed = true;
    vec![clip, body]
}

/// Resolve a table mount's internal hosts from the finalized shell: the
/// container's children are exactly [header clip, body viewport]. Anything
/// else is a loud structural mismatch, never silently misdriven.
fn table_shell(tree: &UiTree, container: NodeId) -> Result<(NodeId, NodeId), String> {
    match tree
        .get(container)
        .map(|r| r.children.clone())
        .unwrap_or_default()
    {
        mut kids if kids.len() == 2 => {
            let body = kids.pop().unwrap();
            let clip = kids.pop().unwrap();
            Ok((clip, body))
        }
        kids => Err(format!(
            "collection: table shell structure mismatch at {container:?}: \
             expected [header clip, body], found {} children",
            kids.len()
        )),
    }
}

/// Key hash of the focused row, if keyboard focus sits inside a body row:
/// walks from the focused node up to a direct body child and reads its
/// reconciliation key (unkeyed header/spacer roots yield no pin).
fn focused_row_key(tree: &UiTree, body: NodeId) -> Option<u64> {
    let mut node = tree.focused()?;
    loop {
        let rec = tree.get(node)?;
        match rec.parent {
            None => return None,
            Some(p) if p == body => return tree.get(node).and_then(|r| r.key),
            Some(p) => node = p,
        }
    }
}

/// Force a row stack's Taffy gap to zero (changed-only). Row gaps live
/// folded in strides plus explicit trailing margins instead, so spacers
/// stay exact. Column gaps are horizontal and untouched, and table
/// containers keep their own gap (header/clip rhythm is separate).
fn ensure_stack_gap_zero(tree: &mut UiTree, stack: NodeId) {
    let dirty = tree
        .get(stack)
        .map(|r| r.style.gap != 0.0 || r.style.row_gap.is_some_and(|g| g != 0.0))
        .unwrap_or(false);
    if dirty {
        if let Some(rec) = tree.get(stack) {
            let mut style = rec.style.clone();
            style.gap = 0.0;
            style.row_gap = None;
            tree.set_style(stack, style);
        }
    }
}

/// Explicit role carried by a retained row/header element, if any
/// (sources assign ListItem/Row/Cell/ColumnHeader through `Element::role`).
fn role_of(el: &Element) -> Option<Role> {
    el.role_opt()
}

/// Assign roles plus logical coordinates through core metadata
/// (read-modify-write, preserving labels and flags). Lists: rows are
/// ListItems with one-based positions and totals. Tables: bar Row, header
/// cells ColumnHeader, body rows Row, body cells Cell, all with zero-based
/// indices plus row/column totals.
#[allow(clippy::too_many_arguments)]
fn apply_collection_semantics(
    tree: &mut UiTree,
    mount: &CollectionMount,
    container: NodeId,
    row_nodes: &[(usize, NodeId)],
    role: CollectionRole,
    total: usize,
    col_count: usize,
) {
    fn set(tree: &mut UiTree, id: NodeId, role: Role, cs: CollectionSemantics) {
        if let Some(rec) = tree.get(id) {
            let mut sem = rec.semantics.clone();
            sem.role = role;
            sem.collection = Some(Box::new(cs));
            tree.set_semantics(id, sem);
        }
    }
    match role {
        CollectionRole::List => {
            set(
                tree,
                container,
                Role::List,
                CollectionSemantics {
                    set_size: Some(total),
                    ..Default::default()
                },
            );
            for (idx, node) in row_nodes {
                let want = mount
                    .rows
                    .get(idx)
                    .and_then(|r| role_of(&r.element))
                    .unwrap_or(Role::ListItem);
                set(
                    tree,
                    *node,
                    want,
                    CollectionSemantics {
                        set_size: Some(total),
                        position_in_set: Some(idx + 1),
                        ..Default::default()
                    },
                );
            }
        }
        CollectionRole::Table => {
            set(
                tree,
                container,
                Role::Table,
                CollectionSemantics {
                    row_count: Some(total),
                    column_count: Some(col_count),
                    ..Default::default()
                },
            );
            if let Some(h) = mount.header.as_ref() {
                for (j, node) in h.nodes.iter().copied().enumerate() {
                    // First node is the bar; any further nodes are treated
                    // as additional header cells (single-bar headers are
                    // the module contract).
                    let (bar_role, cell_role) = if j == 0 {
                        (role_of(&h.element).unwrap_or(Role::Row), Role::ColumnHeader)
                    } else {
                        (Role::ColumnHeader, Role::ColumnHeader)
                    };
                    let _ = cell_role;
                    set(
                        tree,
                        node,
                        bar_role,
                        CollectionSemantics {
                            row_count: Some(total),
                            column_count: Some(col_count),
                            ..Default::default()
                        },
                    );
                    if let Some(kids) = tree.get(node).map(|r| r.children.clone()) {
                        for (k, cell) in kids.iter().copied().enumerate() {
                            set(
                                tree,
                                cell,
                                Role::ColumnHeader,
                                CollectionSemantics {
                                    row_count: Some(total),
                                    column_count: Some(col_count),
                                    column_index: Some(k),
                                    ..Default::default()
                                },
                            );
                        }
                    }
                }
            }
            for (idx, node) in row_nodes {
                let want = mount
                    .rows
                    .get(idx)
                    .and_then(|r| role_of(&r.element))
                    .unwrap_or(Role::Row);
                set(
                    tree,
                    *node,
                    want,
                    CollectionSemantics {
                        row_count: Some(total),
                        column_count: Some(col_count),
                        row_index: Some(*idx),
                        ..Default::default()
                    },
                );
                if let Some(kids) = tree.get(*node).map(|r| r.children.clone()) {
                    for (k, cell) in kids.iter().copied().enumerate() {
                        set(
                            tree,
                            cell,
                            Role::Cell,
                            CollectionSemantics {
                                row_count: Some(total),
                                column_count: Some(col_count),
                                row_index: Some(*idx),
                                column_index: Some(k),
                                ..Default::default()
                            },
                        );
                    }
                }
            }
        }
    }
}

impl View {
    fn paired_ids(&self, tree: &UiTree, root: NodeId) -> Vec<NodeId> {
        if self.content.len() == 1 {
            vec![root]
        } else {
            tree.get(root)
                .map(|r| r.children.clone())
                .unwrap_or_default()
        }
    }

    /// Dispose the view's scope: all bound effects detach, queued effects
    /// purge, cleanups run LIFO, children recurse. The tree keeps its last
    /// committed state (static mount); updates simply stop.
    pub fn dispose(self) {
        self.scope.dispose();
    }
}

fn subtree_ids(tree: &UiTree, node: NodeId) -> Vec<RawId> {
    let mut out = Vec::new();
    let mut stack = vec![node];
    while let Some(id) = stack.pop() {
        out.push(id.raw());
        if let Some(record) = tree.get(id) {
            stack.extend(record.children.iter().copied());
        }
    }
    out
}

/// Lower every top-level element, splicing fragments.
fn lower_all(content: &[Element]) -> Result<Vec<NodeSpec>, MountError> {
    let mut specs = Vec::new();
    for el in content {
        specs.extend(lower_specs(el)?);
    }
    Ok(specs)
}

fn into_single(mut specs: Vec<NodeSpec>) -> NodeSpec {
    debug_assert_eq!(specs.len(), 1);
    specs.pop().unwrap()
}

#[allow(clippy::too_many_arguments)]
fn bind_new(
    scope: &Scope,
    sink: &CommandSink,
    bound: &mut HashMap<RawId, BoundState>,
    collections: &mut HashMap<RawId, CollectionMount>,
    tree: &mut UiTree,
    id: NodeId,
    el: &mut Element,
    inherited: &TypoChain,
) -> Result<(), MountError> {
    let local_typography = el.typo_chain();
    let own = inherited.child(&local_typography);
    if el.is_collection() {
        tree.queue_interaction_binding(id, false);
        sink.interaction(
            crate::tree::interaction::InteractionCommand::TypographyColor {
                node: id,
                own: local_typography.explicit_color(),
                own_gpui: local_typography.explicit_gpui_color(),
                own_source: local_typography.explicit_gpui_source_color(),
            },
        );
        return bind_collection(collections, tree, id, el, &own);
    }
    let bindings = std::mem::take(el.bindings_mut());
    let actions: Vec<_> = bindings
        .iter()
        .filter_map(|binding| match binding {
            ElementBinding::OnAction { kind, .. } => Some(*kind),
            ElementBinding::Popup { .. } => Some(ActionKind::Activate),
            ElementBinding::SubscribeEdit { .. } => Some(ActionKind::TextChanged),
            _ => None,
        })
        .collect();
    tree.retain_bound_actions(id, &actions);
    let keys = effect_keys(&bindings);
    let has_interaction_signal = bindings.iter().any(|binding| {
        matches!(
            binding,
            ElementBinding::InteractionBool { .. } | ElementBinding::InteractionValue(_)
        )
    });
    let same = !has_interaction_signal
        && bound
            .get(&id.raw())
            .map(|s| s.effects == keys)
            .unwrap_or(false);
    match (keys.is_empty(), same) {
        (true, _) => {
            if let Some(state) = bound.remove(&id.raw()) {
                state.scope.dispose();
            }
            apply_bindings(tree, scope, sink, id, bindings);
        }
        (false, true) => {
            let plain: Vec<ElementBinding> =
                bindings.into_iter().filter(|b| !is_effect(b)).collect();
            apply_bindings(tree, scope, sink, id, plain);
        }
        (false, false) => {
            if let Some(state) = bound.remove(&id.raw()) {
                state.scope.dispose();
            }
            let child = scope.child();
            apply_bindings(tree, &child, sink, id, bindings);
            bound.insert(
                id.raw(),
                BoundState {
                    scope: child,
                    effects: keys,
                },
            );
        }
    }
    sink.interaction(
        crate::tree::interaction::InteractionCommand::TypographyColor {
            node: id,
            own: local_typography.explicit_color(),
            own_gpui: local_typography.explicit_gpui_color(),
            own_source: local_typography.explicit_gpui_source_color(),
        },
    );
    if el.role_opt().is_some() || el.collection_semantics_opt().is_some() {
        if let Some(rec) = tree.get(id) {
            let mut sem = rec.semantics.clone();
            if let Some(role) = el.role_opt() {
                sem.role = role;
            }
            if let Some(cs) = el.collection_semantics_opt() {
                sem.collection = Some(Box::new(cs));
            }
            tree.set_semantics(id, sem);
        }
    }
    let kids = tree.content_children(id);
    let mut flat = el.flat_children_mut();
    debug_assert_eq!(
        kids.len(),
        flat.len(),
        "binding walk diverged from lowering: node {id:?} has {} children, element has {}",
        kids.len(),
        flat.len()
    );
    for (kid, child) in kids.iter().zip(flat.iter_mut()) {
        bind_new(scope, sink, bound, collections, tree, *kid, child, &own)?;
    }
    crate::controls::initialize_node(tree, id);
    Ok(())
}

/// Register a collection container with the window driver (or refresh its
/// snapshot when the descriptor changed). Runs at mount/refresh time from
/// the bind walk, for top-level and nested containers alike.
///
/// No row/header factories run here and no bindings are taken: realization
/// and first binding happen in [`View::compute_layout`], which also owns
/// all later window updates. A changed descriptor refreshes the
/// [`CollectionState`] (heights/anchor/pin preserved by key) and drops
/// retained descriptors so the next realization rebuilds; effect scopes
/// recycle by source identity on rebind, and scopes of unmounted rows are
/// disposed by the prune pass. Validation failures surface as mount errors,
/// never silent omissions.
fn bind_collection(
    collections: &mut HashMap<RawId, CollectionMount>,
    tree: &UiTree,
    id: NodeId,
    el: &mut Element,
    inherited: &TypoChain,
) -> Result<(), MountError> {
    let Some(desc) = el.collection_descriptor().cloned() else {
        return Ok(());
    };
    // Effective chain at the collection position, exactly as normative
    // lowering composes it (parent chain plus the collection's own explicit
    // overrides, if any).
    let typo = inherited.child(&el.typo_chain());
    match collections.get_mut(&id.raw()) {
        Some(mount) if same_desc(&mount.last_desc, &desc) && mount.typo == typo => Ok(()),
        Some(mount) if same_shape(&mount.last_desc, &desc) && mount.typo == typo => {
            let previous_rows = std::mem::take(&mut mount.rows);
            mount.rows = carry_rows_in_place(
                mount.last_desc.source.as_ref(),
                desc.source.as_ref(),
                previous_rows,
                |row| row.node.is_some_and(|node| tree.get(node).is_some()),
            );
            mount.state.adopt_source(desc.clone());
            mount.last_desc = desc;
            Ok(())
        }
        Some(mount) => {
            // Ingest the live scroll FIRST: the anchor restore below must
            // capture where the eye actually is, not a stale snapshot.
            // (Lists scroll the container; tables scroll the body child.)
            let body = match mount.state.descriptor().source.role() {
                CollectionRole::List => id,
                CollectionRole::Table => tree
                    .get(id)
                    .map(|r| r.children.clone())
                    .and_then(|kids| kids.get(1).copied())
                    .unwrap_or(id),
            };
            if let Some((sx, sy)) = tree.scroll_offset(body).map(|p| (p.x, p.y)) {
                mount.state.set_scroll(sx, sy);
            }
            let previous_rows = std::mem::take(&mut mount.rows);
            let previous_source = Rc::clone(&mount.last_desc.source);
            mount
                .state
                .refresh(desc.clone())
                .map_err(|e| MountError::Lower(LowerError::Collection(e)))?;
            // Re-resolve the authored gap from the fresh styles (style-only
            // refreshes change strides without touching snapshots).
            mount.state.restyle_gap();
            mount.rows = carry_rows(
                previous_source.as_ref(),
                desc.source.as_ref(),
                previous_rows,
                |row| row.node.is_some_and(|node| tree.get(node).is_some()),
            );
            mount.last_desc = desc;
            mount.header = None;
            if mount.rows.is_empty() {
                mount.width_node = None;
            }
            mount.typo = typo;
            // The refresh restored the scroll from the reading anchor: the
            // next realization writes it tree-ward, then re-reads actual.
            mount.scroll_restore_pending = true;
            Ok(())
        }
        None => {
            let state = CollectionState::new(desc.clone())
                .map_err(|e| MountError::Lower(LowerError::Collection(e)))?;
            let built_plan = state.plan().clone();
            collections.insert(
                id.raw(),
                CollectionMount {
                    container: id,
                    state,
                    last_desc: desc,
                    typo,
                    header: None,
                    rows: HashMap::new(),
                    built_plan,
                    probe_width: 0.0,
                    probe_changed: false,
                    spacers_stale: false,
                    width_node: None,
                    pin: None,
                    scroll_restore_pending: false,
                    last_tree_scroll: None,
                },
            );
            Ok(())
        }
    }
}

/// Whether a binding creates a reactive effect (vs. idempotent registration).
fn is_effect(binding: &ElementBinding) -> bool {
    matches!(
        binding,
        ElementBinding::SubscribeText(_)
            | ElementBinding::SubscribeEdit { .. }
            | ElementBinding::InteractionBool { .. }
            | ElementBinding::InteractionValue(_)
    )
}

/// Reactive source identities subscribed by effect bindings, in order.
/// Compared across syncs to decide whether a reused node's effects stay.
fn effect_keys(bindings: &[ElementBinding]) -> Vec<aqua_reactive::NodeId> {
    let mut keys = Vec::new();
    for binding in bindings {
        match binding {
            ElementBinding::SubscribeText(source) => keys.push(source.node_id()),
            ElementBinding::SubscribeEdit { read, write } => {
                keys.push(read.node_id());
                keys.push(write.node_id());
            }
            ElementBinding::InteractionBool { source, .. } => keys.push(source.node_id()),
            ElementBinding::InteractionValue(source) => keys.push(source.node_id()),
            ElementBinding::Popup { .. }
            | ElementBinding::ContextMenu(_)
            | ElementBinding::Tooltip(_)
            | ElementBinding::Disabled(_)
            | ElementBinding::Focusability(_)
            | ElementBinding::KeyboardReleaseActivation
            | ElementBinding::TabIndex(_)
            | ElementBinding::Label(_)
            | ElementBinding::Description(_)
            | ElementBinding::DebugSelector(_)
            | ElementBinding::ExcludeHitTest
            | ElementBinding::Selected(_)
            | ElementBinding::Checked(_)
            | ElementBinding::Expanded(_)
            | ElementBinding::Level(_)
            | ElementBinding::InteractionStyle { .. }
            | ElementBinding::ValueText(_)
            | ElementBinding::OnInput { .. }
            | ElementBinding::Cursor(_)
            | ElementBinding::Shortcut(_)
            | ElementBinding::OnAction { .. }
            | ElementBinding::Focusable
            | ElementBinding::Painter(_)
            | ElementBinding::ContextPainter(_)
            | ElementBinding::ScrollStablePainter(_)
            | ElementBinding::Observe(_)
            | ElementBinding::Prepaint(_) => {}
        }
    }
    keys
}

/// Apply one element's taken bindings to its mounted node.
fn apply_bindings(
    tree: &mut UiTree,
    scope: &Scope,
    sink: &CommandSink,
    id: NodeId,
    bindings: Vec<ElementBinding>,
) {
    tree.clear_input_bindings(id);
    tree.clear_canvas_prepaint(id);
    tree.clear_canvas_observer(id);
    tree.clear_canvas_painter(id);
    if let Some(record) = tree.get_mut(id) {
        record.description = None;
        record.debug_selector = None;
        record.hit_test_excluded = false;
        record.keyboard_release_activation = false;
    }
    let styled = bindings
        .iter()
        .any(|binding| matches!(binding, ElementBinding::InteractionStyle { .. }));
    tree.queue_interaction_binding(id, styled);
    for binding in bindings {
        match binding {
            ElementBinding::Popup { options, content } => {
                let sink = sink.clone();
                tree.set_bound_action_callback(
                    id,
                    ActionKind::Activate,
                    Box::new(move |_| {
                        sink.push(UiCommand::OpenPopup {
                            node: id,
                            content: Box::new(content()),
                            options: options.clone(),
                        })
                    }),
                );
            }
            ElementBinding::ContextMenu(content) => {
                tree.listen_input_bound(
                    id,
                    crate::input::InputKind::PointerDown,
                    crate::input::EventPhase::Bubble,
                    Rc::new(move |event| {
                        if matches!(
                            event.input,
                            crate::input::RoutedInput::Native(aqua_core::InputEvent::PointerDown(
                                _,
                                aqua_core::PointerButton::Secondary
                            ))
                        ) {
                            crate::input::InputResponse {
                                commands: vec![UiCommand::OpenPopup {
                                    node: id,
                                    content: Box::new(content()),
                                    options: crate::popup::PopupOptions::menu()
                                        .at(event.input.position().unwrap_or(Point::ZERO)),
                                }],
                                ..crate::input::InputResponse::handled()
                            }
                        } else {
                            crate::input::InputResponse::default()
                        }
                    }),
                );
            }
            ElementBinding::Tooltip(tooltip) => {
                tree.listen_input_bound(
                    id,
                    crate::input::InputKind::PointerEnter,
                    crate::input::EventPhase::Bubble,
                    Rc::new(move |_| crate::input::InputResponse {
                        commands: vec![UiCommand::ArmTooltip {
                            node: id,
                            tooltip: tooltip.clone(),
                        }],
                        ..Default::default()
                    }),
                );
                tree.listen_input_bound(
                    id,
                    crate::input::InputKind::PointerLeave,
                    crate::input::EventPhase::Bubble,
                    Rc::new(move |_| crate::input::InputResponse {
                        commands: vec![UiCommand::HideTooltip { node: id }],
                        ..Default::default()
                    }),
                );
            }

            ElementBinding::Disabled(disabled) => {
                if let Some(record) = tree.get_mut(id) {
                    record.semantics.disabled = disabled;
                }
            }
            ElementBinding::Focusability(focusable) => {
                if let Some(record) = tree.get_mut(id) {
                    record.focusable = focusable;
                }
            }
            ElementBinding::KeyboardReleaseActivation => {
                if let Some(record) = tree.get_mut(id) {
                    record.keyboard_release_activation = true;
                }
            }
            ElementBinding::TabIndex(tab_index) => {
                if let Some(record) = tree.get_mut(id) {
                    record.tab_index = tab_index;
                    record.focusable = true;
                }
            }
            ElementBinding::Label(label) => {
                if let Some(record) = tree.get_mut(id) {
                    record.semantics.label = label;
                }
            }

            ElementBinding::Description(description) => {
                if let Some(record) = tree.get_mut(id) {
                    record.description = Some(description);
                }
            }
            ElementBinding::DebugSelector(selector) => {
                if let Some(record) = tree.get_mut(id) {
                    record.debug_selector = Some(selector);
                }
            }
            ElementBinding::ExcludeHitTest => {
                if let Some(record) = tree.get_mut(id) {
                    record.hit_test_excluded = true;
                }
            }
            ElementBinding::InteractionStyle { kind, style } => {
                sink.interaction(crate::tree::interaction::InteractionCommand::Style {
                    node: id,
                    kind,
                    style,
                });
            }
            ElementBinding::Level(value) => {
                sink.interaction(crate::tree::interaction::InteractionCommand::Semantics {
                    node: id,
                    value: crate::tree::interaction::InteractionSemantic::Level(Some(value)),
                })
            }
            ElementBinding::Expanded(value) => {
                sink.interaction(crate::tree::interaction::InteractionCommand::Semantics {
                    node: id,
                    value: crate::tree::interaction::InteractionSemantic::Expanded(Some(value)),
                })
            }
            ElementBinding::Checked(value) => {
                sink.interaction(crate::tree::interaction::InteractionCommand::Semantics {
                    node: id,
                    value: crate::tree::interaction::InteractionSemantic::Checked(Some(value)),
                })
            }
            ElementBinding::InteractionBool { property, source } => {
                crate::element::interaction::bind_interaction_bool(
                    scope, sink, id, property, source,
                )
            }
            ElementBinding::InteractionValue(source) => {
                crate::element::interaction::bind_interaction_value(scope, sink, id, source)
            }
            ElementBinding::Selected(selected) => {
                sink.interaction(crate::tree::interaction::InteractionCommand::Semantics {
                    node: id,
                    value: crate::tree::interaction::InteractionSemantic::Selected(selected),
                })
            }
            ElementBinding::ValueText(value) => {
                sink.interaction(crate::tree::interaction::InteractionCommand::Semantics {
                    node: id,
                    value: crate::tree::interaction::InteractionSemantic::Value(value),
                })
            }

            ElementBinding::OnInput {
                kind,
                phase,
                callback,
            } => {
                tree.listen_input_bound(id, kind, phase, callback);
            }
            ElementBinding::Cursor(cursor) => {
                tree.set_cursor_bound(id, cursor);
            }
            ElementBinding::Shortcut(shortcut) => {
                tree.bind_shortcut_bound(id, shortcut);
            }
            ElementBinding::SubscribeText(source) => {
                let s = sink.clone();
                scope.subscribe(&source, move |text: String| {
                    s.push(UiCommand::SetText { node: id, text });
                });
            }
            ElementBinding::SubscribeEdit { read, write } => {
                let s = sink.clone();
                scope.subscribe(&read, move |text: String| {
                    s.push(UiCommand::SetText { node: id, text });
                });
                tree.set_bound_action_callback(
                    id,
                    ActionKind::TextChanged,
                    std::boxed::Box::new(move |e: ActionEvent| {
                        if let Some(text) = e.text {
                            let _ = write.try_set(text);
                        }
                    }),
                );
            }
            ElementBinding::OnAction { kind, callback } => {
                tree.set_bound_action_callback(id, kind, callback);
            }
            ElementBinding::Focusable => {
                if let Some(rec) = tree.get_mut(id) {
                    rec.focusable = true;
                }
            }
            ElementBinding::Painter(painter) => {
                tree.set_canvas_painter(id, painter);
            }
            ElementBinding::ContextPainter(painter) => {
                tree.set_context_canvas_painter(id, painter);
            }
            ElementBinding::ScrollStablePainter(painter) => {
                tree.set_scroll_stable_canvas_painter(id, painter);
            }
            ElementBinding::Prepaint(callback) => {
                tree.set_canvas_prepaint(id, callback);
            }
            ElementBinding::Observe(callback) => {
                tree.set_canvas_observer(id, callback);
            }
        }
    }
}

/// Bind a read-only text source to a mounted node under `scope`.
/// Returns the effect handle for inspection; the scope owns the effect.
pub fn bind_text(
    scope: &Scope,
    sink: &CommandSink,
    node: NodeId,
    source: &ReadSignal<String>,
) -> aqua_reactive::EffectHandle {
    let s = sink.clone();
    scope.subscribe(source, move |text: String| {
        s.push(UiCommand::SetText { node, text });
    })
}

/// Bind a two-way editable model to a mounted text node under `scope`:
/// `read` drives `SetText` updates, `TextChanged` actions write back.
/// Returns the effect handle for inspection; the scope owns the effect.
pub fn bind_text_editable(
    tree: &mut UiTree,
    scope: &Scope,
    sink: &CommandSink,
    node: NodeId,
    read: &ReadSignal<String>,
    write: &WriteSignal<String>,
) -> aqua_reactive::EffectHandle {
    let handle = bind_text(scope, sink, node, read);
    let w = write.clone();
    tree.set_action_callback(
        node,
        ActionKind::TextChanged,
        std::boxed::Box::new(move |e: ActionEvent| {
            if let Some(text) = e.text {
                let _ = w.try_set(text);
            }
        }),
    );
    handle
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::element::ui;
    use crate::layout::NaiveMeasurer;
    use crate::tree::NodeKind;
    use aqua_core::Size;
    use aqua_reactive::Runtime;

    fn setup() -> (Runtime, UiTree) {
        (Runtime::new(), UiTree::new())
    }

    fn layout(tree: &mut UiTree) {
        tree.compute_layout(Size::new(400.0, 400.0), &NaiveMeasurer)
            .unwrap();
    }

    fn text_of(tree: &UiTree, id: NodeId) -> String {
        match &tree.get(id).map(|r| &r.kind) {
            Some(NodeKind::Text(t)) => t.text().to_string(),
            other => panic!("expected text node, got {other:?}"),
        }
    }

    fn text_color_of(tree: &UiTree, id: NodeId) -> aqua_core::Oklch {
        match &tree.get(id).map(|r| &r.kind) {
            Some(NodeKind::Text(t)) => t.color,
            other => panic!("expected text node, got {other:?}"),
        }
    }

    use crate::style::{px, Style};

    #[test]
    fn mount_box_text_layout_and_scene() {
        let (_rt, mut tree) = setup();
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let mut view = View::new(scope, tree.command_sink());
        let ids = view
            .set_content(
                &mut tree,
                vec![ui::Box::builder()
                    .layout("column")
                    .gap(16)
                    .child(ui::Text::builder().arg("hello").build())
                    .build()],
            )
            .unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(view.root(), Some(ids[0]));
        layout(&mut tree);
        let ops = tree.emit_scene();
        assert!(ops
            .iter()
            .any(|(_, op)| matches!(op, aqua_core::DisplayItem::Text { .. })));
    }

    #[test]
    fn signal_write_updates_mounted_label() {
        let (_rt, mut tree) = setup();
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let (label, set_label) = scope.signal(String::from("one"));
        let mut view = View::new(scope, tree.command_sink());
        let ids = view
            .set_content(
                &mut tree,
                vec![ui::Box::builder()
                    .child(ui::Text::builder().value(label.clone()).build())
                    .build()],
            )
            .unwrap();
        layout(&mut tree);
        let text_id = tree.get(ids[0]).unwrap().children[0];
        assert_eq!(text_of(&tree, text_id), "one");
        set_label.set("two".to_string());
        layout(&mut tree);
        assert_eq!(text_of(&tree, text_id), "two");
    }

    #[test]
    fn no_duplicate_effects_across_syncs() {
        let (_rt, mut tree) = setup();
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let (label, set_label) = scope.signal(String::from("a"));
        let mut view = View::new(scope, tree.command_sink());
        let content = || {
            vec![ui::Box::builder()
                .child(ui::Text::builder().value(label.clone()).build())
                .build()]
        };
        view.set_content(&mut tree, content()).unwrap();
        layout(&mut tree);
        view.set_content(&mut tree, content()).unwrap();
        layout(&mut tree);
        // One write → exactly one queued SetText: effects were not duplicated.
        set_label.set("b".to_string());
        assert_eq!(tree.pending_commands(), 1);
        layout(&mut tree);
    }

    #[test]
    fn button_activation_runs_handler() {
        use std::sync::{Arc, Mutex};
        let (_rt, mut tree) = setup();
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let fired = Arc::new(Mutex::new(0u32));
        let fired2 = fired.clone();
        let mut view = View::new(scope, tree.command_sink());
        let ids = view
            .set_content(
                &mut tree,
                vec![ui::Button::builder()
                    .on_activate(move || {
                        *fired2.lock().unwrap() += 1;
                    })
                    .child(ui::Text::builder().arg("Go").build())
                    .build()],
            )
            .unwrap();
        layout(&mut tree);
        tree.fire_action(crate::tree::ActionEvent {
            source: None,
            node: ids[0],
            kind: crate::tree::ActionKind::Activate,
            text: None,
            focused: None,
        });
        assert_eq!(*fired.lock().unwrap(), 1);
    }

    #[test]
    fn editable_binding_writes_back_through_signal() {
        let (_rt, mut tree) = setup();
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let (text, _set_text) = scope.signal(String::from("draft"));
        let mut view = View::new(scope, tree.command_sink());
        let ids = view
            .set_content(
                &mut tree,
                vec![ui::Text::builder()
                    .bind((text.clone(), _set_text.clone()))
                    .build()],
            )
            .unwrap();
        layout(&mut tree);
        assert_eq!(text_of(&tree, ids[0]), "draft");
        // Host-side edit event (as dispatched by events.rs) writes back.
        tree.fire_action(crate::tree::ActionEvent {
            source: None,
            node: ids[0],
            kind: crate::tree::ActionKind::TextChanged,
            text: Some("draft!".to_string()),
            focused: None,
        });
        assert_eq!(text.get(), "draft!");
    }

    #[test]
    fn placeholder_shows_when_empty_only() {
        let (_rt, mut tree) = setup();
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let mut view = View::new(scope, tree.command_sink());
        let ids = view
            .set_content(
                &mut tree,
                vec![ui::Text::builder()
                    .bind({
                        let s = view.scope();
                        let (r, w) = s.signal(String::new());
                        (r, w)
                    })
                    .placeholder("Search...")
                    .build()],
            )
            .unwrap();
        layout(&mut tree);
        let shown = match &tree.get(ids[0]).unwrap().kind {
            NodeKind::Text(t) => (t.is_placeholder(), t.display_text(), t.text().to_string()),
            other => panic!("expected text, got {other:?}"),
        };
        // Display shows the placeholder; committed text stays empty.
        assert_eq!(
            shown,
            (true, "Search...".to_string(), String::new()),
            "{shown:?}"
        );
        // Typing hides it; clearing restores it.
        assert!(tree.set_text(ids[0], "x"));
        let t = match &tree.get(ids[0]).unwrap().kind {
            NodeKind::Text(t) => (t.is_placeholder(), t.display_text()),
            _ => unreachable!(),
        };
        assert_eq!(t, (false, "x".to_string()));
        assert!(tree.set_text(ids[0], ""));
        assert!(match &tree.get(ids[0]).unwrap().kind {
            NodeKind::Text(t) => t.is_placeholder(),
            _ => false,
        });
    }

    #[test]
    fn keyed_reorder_preserves_nodes_and_retargets_effects() {
        let (_rt, mut tree) = setup();
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let (a, set_a) = scope.signal(String::from("a"));
        let (b, set_b) = scope.signal(String::from("b"));
        let rows = |a: &ReadSignal<String>, b: &ReadSignal<String>| {
            vec![ui::Box::builder()
                .child(
                    ui::Text::builder()
                        .value(a.clone())
                        .build()
                        .with_key("row-a"),
                )
                .child(
                    ui::Text::builder()
                        .value(b.clone())
                        .build()
                        .with_key("row-b"),
                )
                .build()]
        };
        let mut view = View::new(scope, tree.command_sink());
        view.set_content(&mut tree, rows(&a, &b)).unwrap();
        layout(&mut tree);
        let root = view.root().unwrap();
        let before: Vec<NodeId> = tree.get(root).unwrap().children.clone();
        // Reorder by key.
        view.set_content(
            &mut tree,
            vec![ui::Box::builder()
                .child(
                    ui::Text::builder()
                        .value(b.clone())
                        .build()
                        .with_key("row-b"),
                )
                .child(
                    ui::Text::builder()
                        .value(a.clone())
                        .build()
                        .with_key("row-a"),
                )
                .build()],
        )
        .unwrap();
        layout(&mut tree);
        let after: Vec<NodeId> = tree.get(root).unwrap().children.clone();
        assert_eq!(after, vec![before[1], before[0]]);
        // Effects still target the right nodes after reorder.
        set_a.set("A".to_string());
        set_b.set("B".to_string());
        layout(&mut tree);
        assert_eq!(text_of(&tree, after[1]), "A");
        assert_eq!(text_of(&tree, after[0]), "B");
        let _ = set_a;
        let _ = set_b;
    }

    #[test]
    fn row_scope_dispose_kills_row_effect() {
        let (_rt, mut tree) = setup();
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let (label, set_label) = scope.signal(String::from("live"));
        let row_scope = scope.child();
        let mut view = View::new(scope, tree.command_sink());
        let ids = view
            .set_content(
                &mut tree,
                vec![ui::Text::builder().value(label.clone()).build()],
            )
            .unwrap();
        layout(&mut tree);
        assert_eq!(text_of(&tree, ids[0]), "live");
        // Rebind the same node under a row scope, then dispose it.
        bind_text(&row_scope, &tree.command_sink(), ids[0], &label);
        row_scope.dispose();
        set_label.set("dead".to_string());
        rt2.flush();
        // The disposed row effect queued nothing; the view effect still works.
        // (View effect was bound first; both target the same node.)
        layout(&mut tree);
        assert_eq!(text_of(&tree, ids[0]), "dead");
    }

    #[test]
    fn second_set_content_does_not_nest_root() {
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let mut tree = UiTree::new();
        let mut view = View::new(scope, tree.command_sink());
        let content = || {
            vec![ui::Box::builder()
                .child(ui::Text::builder().arg("hi").build())
                .build()]
        };
        let ids = view.set_content(&mut tree, content()).unwrap();
        layout(&mut tree);
        let ids2 = view.set_content(&mut tree, content()).unwrap();
        layout(&mut tree);
        // Same root, one child, no nesting: the refreshed root spec must not
        // become a child of itself.
        assert_eq!(ids, ids2);
        let root = view.root().unwrap();
        assert_eq!(tree.get(root).unwrap().children.len(), 1);
        assert_eq!(tree.len(), 2);
    }

    #[test]
    fn rebind_same_key_switches_source() {
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let (old_src, set_old) = scope.signal(String::from("old"));
        let (new_src, set_new) = scope.signal(String::from("new"));
        let mut tree = UiTree::new();
        let mut view = View::new(scope, tree.command_sink());
        let row = |s: &ReadSignal<String>| {
            vec![ui::Box::builder()
                .child(ui::Text::builder().value(s.clone()).build().with_key("k"))
                .build()]
        };
        let ids = view.set_content(&mut tree, row(&old_src)).unwrap();
        layout(&mut tree);
        let text_id = tree.get(ids[0]).unwrap().children[0];
        assert_eq!(text_of(&tree, text_id), "old");
        // Same key, different source: old must stop, new must drive.
        let ids2 = view.set_content(&mut tree, row(&new_src)).unwrap();
        assert_eq!(ids, ids2, "node identity must survive source switch");
        // Sync carries the new snapshot directly; the rebind's initial
        // effect run also queues one (harmless, deduped at drain).
        assert_eq!(text_of(&tree, text_id), "new");
        layout(&mut tree);
        assert_eq!(tree.pending_commands(), 0);
        // Old source is disposed: its writes must queue nothing.
        set_old.set("old2".to_string());
        rt2.flush();
        assert_eq!(
            tree.pending_commands(),
            0,
            "disposed old source must queue nothing"
        );
        set_new.set("new2".to_string());
        layout(&mut tree);
        assert_eq!(text_of(&tree, text_id), "new2");
    }

    #[test]
    fn refresh_subtree_touches_only_the_keyed_subtree() {
        let rt = Runtime::new();
        let scope = rt.create_scope();
        let (outside, set_outside) = scope.signal(String::from("outside"));
        let mut tree = UiTree::new();
        let mut view = View::new(scope, tree.command_sink());
        let inner = |labels: &[&str]| {
            labels
                .iter()
                .fold(ui::Box::builder(), |builder, label| {
                    builder.child(ui::Text::builder().arg(*label).build())
                })
                .build()
                .with_key("inner")
        };
        let content = |labels: &[&str]| {
            vec![ui::Box::builder()
                .child(
                    ui::Text::builder()
                        .value(outside.clone())
                        .build()
                        .with_key("outside"),
                )
                .child(inner(labels))
                .build()]
        };
        let ids = view.set_content(&mut tree, content(&["a"])).unwrap();
        layout(&mut tree);
        let outside_node = tree.get(ids[0]).unwrap().children[0];
        let inner_node = tree.get(ids[0]).unwrap().children[1];
        assert_eq!(tree.get(inner_node).unwrap().children.len(), 1);
        assert!(view
            .refresh_subtree(&mut tree, inner_node, inner(&["a", "b", "c"]))
            .unwrap());
        layout(&mut tree);
        assert_eq!(tree.get(ids[0]).unwrap().children[0], outside_node);
        assert_eq!(tree.get(ids[0]).unwrap().children[1], inner_node);
        assert_eq!(tree.get(inner_node).unwrap().children.len(), 3);
        set_outside.set("changed".to_string());
        layout(&mut tree);
        assert_eq!(text_of(&tree, outside_node), "changed");
        let unkeyed = ui::Box::builder().build();
        assert!(!view
            .refresh_subtree(&mut tree, inner_node, unkeyed)
            .unwrap());
    }

    #[test]
    fn uncontrolled_editable_survives_keyed_reorder() {
        use crate::tree::TextContent;
        fn committed(tree: &UiTree, id: NodeId) -> (String, Option<usize>) {
            match &tree.get(id).unwrap().kind {
                NodeKind::Text(t) => (t.text().to_string(), t.caret()),
                other => panic!("expected text, got {other:?}"),
            }
        }
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let (s1, _set_s1) = scope.signal(String::from("S1"));
        let (s2, _set_s2) = scope.signal(String::from("S2"));
        let mut tree = UiTree::new();
        let mut view = View::new(scope, tree.command_sink());
        // u: uncontrolled editable (mount-time default ""); l: plain label;
        // c: controlled via signal.
        let rows_u_first = |l: &str, c: &ReadSignal<String>| {
            vec![ui::Box::builder()
                .child(
                    ui::Text::builder()
                        .arg("")
                        .editable(true)
                        .build()
                        .with_key("u"),
                )
                .child(ui::Text::builder().arg(l).build().with_key("l"))
                .child(ui::Text::builder().value(c.clone()).build().with_key("c"))
                .build()]
        };
        let ids = view
            .set_content(&mut tree, rows_u_first("v1", &s1))
            .unwrap();
        layout(&mut tree);
        let root = ids[0];
        let u = tree.get(root).unwrap().children[0];
        // Type into the uncontrolled field: committed "hi", caret 2.
        tree.edit_text(u, |tc: &mut TextContent| {
            tc.insert_at_caret("h");
            tc.insert_at_caret("i");
        });
        assert_eq!(committed(&tree, u), ("hi".to_string(), Some(2)));
        // Rebuild all rows from scratch (stale mount defaults), reorder so
        // the uncontrolled row moves last; label literal changes; controlled
        // switches to a new signal.
        let ids2 = view
            .set_content(
                &mut tree,
                vec![ui::Box::builder()
                    .child(ui::Text::builder().arg("v2").build().with_key("l"))
                    .child(ui::Text::builder().value(s2.clone()).build().with_key("c"))
                    .child(
                        ui::Text::builder()
                            .arg("")
                            .editable(true)
                            .build()
                            .with_key("u"),
                    )
                    .build()],
            )
            .unwrap();
        layout(&mut tree);
        assert_eq!(ids2, vec![root]);
        let after: Vec<NodeId> = tree.get(root).unwrap().children.clone();
        let l = after[0];
        let c = after[1];
        let u2 = after[2];
        // Identity follows keys through the reorder.
        assert_eq!(text_of(&tree, l), "v2");
        assert_eq!(text_of(&tree, c), "S2");
        // Uncontrolled editor keeps its value AND caret despite the stale
        // mount default in the rebuilt element.
        assert_eq!(u2, u);
        assert_eq!(committed(&tree, u2), ("hi".to_string(), Some(2)));
    }

    #[test]
    fn static_to_editable_keeps_id_text_and_caret() {
        use crate::tree::TextContent;
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let mut tree = UiTree::new();
        let mut view = View::new(scope, tree.command_sink());
        let field = |editable: bool| {
            vec![ui::Text::builder()
                .arg("hello")
                .editable(editable)
                .build()
                .with_key("f")]
        };
        let ids = view.set_content(&mut tree, field(false)).unwrap();
        layout(&mut tree);
        // Static label: no editor, no caret.
        assert!(match &tree.get(ids[0]).unwrap().kind {
            NodeKind::Text(t) => !t.is_editable() && t.caret().is_none(),
            _ => false,
        });
        // Give the static label a selection caret, then flip to editable.
        tree.edit_text(ids[0], |tc: &mut TextContent| {
            tc.selection = Some(aqua_text::Selection::caret_at(2));
        });
        let ids2 = view.set_content(&mut tree, field(true)).unwrap();
        layout(&mut tree);
        // Same node id, now editor-backed, text + caret carried over.
        assert_eq!(ids2, ids);
        match &tree.get(ids[0]).unwrap().kind {
            NodeKind::Text(t) => {
                assert!(t.is_editable());
                assert_eq!(t.text(), "hello");
                assert_eq!(t.caret(), Some(2));
            }
            other => panic!("expected text, got {other:?}"),
        }
        // And back to static: caret collapses into the label selection.
        let ids3 = view.set_content(&mut tree, field(false)).unwrap();
        layout(&mut tree);
        assert_eq!(ids3, ids);
        match &tree.get(ids[0]).unwrap().kind {
            NodeKind::Text(t) => {
                assert!(!t.is_editable());
                assert_eq!(t.text(), "hello");
                assert_eq!(t.caret(), Some(2));
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn multiline_flag_flips_line_mode_in_place() {
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let mut tree = UiTree::new();
        let mut view = View::new(scope, tree.command_sink());
        let field = |multiline: bool| {
            vec![ui::Text::builder()
                .arg("a")
                .editable(true)
                .multiline(multiline)
                .build()
                .with_key("f")]
        };
        let ids = view.set_content(&mut tree, field(false)).unwrap();
        layout(&mut tree);
        let mode = |tree: &UiTree| match &tree.get(ids[0]).unwrap().kind {
            NodeKind::Text(t) => (t.is_editable(), t.multiline(), t.text().to_string()),
            other => panic!("expected text, got {other:?}"),
        };
        assert_eq!(mode(&tree), (true, false, "a".to_string()));
        // Flip to multiline: same node, editor switches line mode, text kept.
        let ids2 = view.set_content(&mut tree, field(true)).unwrap();
        layout(&mut tree);
        assert_eq!(ids2, ids);
        assert_eq!(mode(&tree), (true, true, "a".to_string()));
        // And back to single-line.
        let ids3 = view.set_content(&mut tree, field(false)).unwrap();
        layout(&mut tree);
        assert_eq!(ids3, ids);
        assert_eq!(mode(&tree), (true, false, "a".to_string()));
    }

    #[test]
    fn removed_row_disposes_scope_churn_baseline() {
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let (a, _set_a) = scope.signal(String::from("a"));
        let (b, _set_b) = scope.signal(String::from("b"));
        let baseline = rt2.stats();
        assert_eq!(baseline.effects, 0);
        let mut tree = UiTree::new();
        let mut view = View::new(scope, tree.command_sink());
        let rows = |a: &ReadSignal<String>, b: &ReadSignal<String>| {
            vec![ui::Box::builder()
                .child(ui::Text::builder().value(a.clone()).build().with_key("ra"))
                .child(ui::Text::builder().value(b.clone()).build().with_key("rb"))
                .build()]
        };
        view.set_content(&mut tree, rows(&a, &b)).unwrap();
        layout(&mut tree);
        assert_eq!(rt2.stats().effects, baseline.effects + 2);
        // Remove one row: its child scope (and effect) must go away.
        view.set_content(
            &mut tree,
            vec![ui::Box::builder()
                .child(ui::Text::builder().value(a.clone()).build().with_key("ra"))
                .build()],
        )
        .unwrap();
        layout(&mut tree);
        rt2.flush();
        let after = rt2.stats();
        assert_eq!(after.effects, baseline.effects + 1, "{after:?}");
        assert_eq!(after.live_scopes, baseline.live_scopes + 1, "{after:?}");
    }

    #[test]
    fn mount_errors_are_explicit() {
        let (_rt, mut tree) = setup();
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let mut view = View::new(scope, tree.command_sink());
        assert_eq!(
            view.set_content(&mut tree, vec![]),
            Err(MountError::EmptyRoot)
        );
        assert_eq!(
            view.set_content(
                &mut tree,
                vec![
                    ui::Text::builder().arg("a").build(),
                    ui::Text::builder().arg("b").build(),
                ]
            ),
            Err(MountError::MultipleRoots { count: 2 })
        );
        assert!(view.root().is_none());
    }

    #[test]
    fn kind_change_remounts_root() {
        let (_rt, mut tree) = setup();
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let mut view = View::new(scope, tree.command_sink());
        let ids = view
            .set_content(&mut tree, vec![ui::Text::builder().arg("x").build()])
            .unwrap();
        layout(&mut tree);
        let ids2 = view
            .set_content(
                &mut tree,
                vec![ui::Box::builder()
                    .child(ui::Text::builder().arg("x").build())
                    .build()],
            )
            .unwrap();
        assert_ne!(ids, ids2);
        layout(&mut tree);
        assert_eq!(tree.len(), 2);
    }

    #[test]
    fn dynamic_value_updates_like_text_signal() {
        let (_rt, mut tree) = setup();
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let (v, set_v) = scope.signal(String::from("d1"));
        let mut view = View::new(scope, tree.command_sink());
        let ids = view
            .set_content(
                &mut tree,
                vec![ui::Dynamic::builder().value(v.clone()).build()],
            )
            .unwrap();
        layout(&mut tree);
        assert_eq!(text_of(&tree, ids[0]), "d1");
        set_v.set("d2".to_string());
        layout(&mut tree);
        assert_eq!(text_of(&tree, ids[0]), "d2");
    }

    // -- Style integration: mount + refresh + inheritance + precedence --

    #[test]
    fn styles_mount_inherit_and_refresh() {
        let (_rt, mut tree) = setup();
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let mut view = View::new(scope, tree.command_sink());
        let content = |color: aqua_core::Oklch| {
            vec![ui::Box::builder()
                .styles([Style::new().text_color(color)])
                .child(ui::Text::builder().arg("plain").build())
                .child(
                    ui::Text::builder()
                        .arg("pinned")
                        .color(aqua_core::Oklch::WHITE)
                        .build(),
                )
                .build()]
        };
        let ids = view
            .set_content(&mut tree, content(aqua_core::Oklch::BLACK))
            .unwrap();
        layout(&mut tree);
        let root = ids[0];
        let kids = tree.get(root).unwrap().children.clone();
        assert_eq!(text_color_of(&tree, kids[0]), aqua_core::Oklch::BLACK);
        assert_eq!(text_color_of(&tree, kids[1]), aqua_core::Oklch::WHITE);
        // Refresh with a new parent style: inheritance follows, the explicit
        // override stays, and node identities survive (positional reuse).
        let ids2 = view
            .set_content(
                &mut tree,
                content(aqua_core::Oklch::from_hex("#123456").unwrap()),
            )
            .unwrap();
        layout(&mut tree);
        assert_eq!(ids, ids2);
        let kids2 = tree.get(root).unwrap().children.clone();
        assert_eq!(kids, kids2);
        assert_eq!(
            text_color_of(&tree, kids2[0]),
            aqua_core::Oklch::from_hex("#123456").unwrap()
        );
        assert_eq!(text_color_of(&tree, kids2[1]), aqua_core::Oklch::WHITE);
    }

    #[test]
    fn uncontrolled_editable_survives_style_refresh() {
        let (_rt, mut tree) = setup();
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let mut view = View::new(scope, tree.command_sink());
        let content = |color: aqua_core::Oklch| {
            vec![ui::Text::builder()
                .arg("")
                .editable(true)
                .styles([Style::new().text_color(color)])
                .build()]
        };
        let ids = view
            .set_content(&mut tree, content(aqua_core::Oklch::BLACK))
            .unwrap();
        layout(&mut tree);
        tree.edit_text(ids[0], |tc: &mut crate::tree::TextContent| {
            tc.insert_at_caret("h");
            tc.insert_at_caret("i");
        });
        // Refresh with a new style: content AND caret survive, cosmetics apply.
        let ids2 = view
            .set_content(&mut tree, content(aqua_core::Oklch::WHITE))
            .unwrap();
        layout(&mut tree);
        assert_eq!(ids, ids2);
        assert_eq!(text_of(&tree, ids[0]), "hi");
        assert_eq!(text_color_of(&tree, ids[0]), aqua_core::Oklch::WHITE);
        match &tree.get(ids[0]).unwrap().kind {
            NodeKind::Text(t) => assert_eq!(t.caret(), Some(2)),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn signal_drives_after_style_refresh_no_dup_effects() {
        let (_rt, mut tree) = setup();
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let (label, set_label) = scope.signal(String::from("one"));
        let mut view = View::new(scope, tree.command_sink());
        let content = || {
            vec![ui::Box::builder()
                .styles([Style::new().text_color(aqua_core::Oklch::BLACK)])
                .child(ui::Text::builder().value(label.clone()).build())
                .build()]
        };
        view.set_content(&mut tree, content()).unwrap();
        layout(&mut tree);
        view.set_content(&mut tree, content()).unwrap();
        layout(&mut tree);
        set_label.set("two".to_string());
        assert_eq!(tree.pending_commands(), 1);
        layout(&mut tree);
        let root = view.root().unwrap();
        let text_id = tree.get(root).unwrap().children[0];
        assert_eq!(text_of(&tree, text_id), "two");
        assert_eq!(text_color_of(&tree, text_id), aqua_core::Oklch::BLACK);
    }

    #[test]
    fn keyed_reorder_with_styles_keeps_nodes_and_paint() {
        let (_rt, mut tree) = setup();
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let mut view = View::new(scope, tree.command_sink());
        let row = |key: &str, fill: aqua_core::Oklch| {
            ui::Box::builder()
                .styles([Style::new().background(fill)])
                .child(ui::Text::builder().arg(key).build())
                .build()
                .with_key(key)
        };
        view.set_content(
            &mut tree,
            vec![ui::Box::builder()
                .child(row("a", aqua_core::Oklch::BLACK))
                .child(row("b", aqua_core::Oklch::WHITE))
                .build()],
        )
        .unwrap();
        layout(&mut tree);
        let root = view.root().unwrap();
        let before: Vec<NodeId> = tree.get(root).unwrap().children.clone();
        view.set_content(
            &mut tree,
            vec![ui::Box::builder()
                .child(row("b", aqua_core::Oklch::WHITE))
                .child(row("a", aqua_core::Oklch::BLACK))
                .build()],
        )
        .unwrap();
        layout(&mut tree);
        let after: Vec<NodeId> = tree.get(root).unwrap().children.clone();
        assert_eq!(after, vec![before[1], before[0]]);
        // Paint follows the element (key), not the position.
        assert_eq!(
            tree.paint(after[0]),
            Some(crate::style::Paint {
                fill: Some(aqua_core::Oklch::WHITE),
                ..Default::default()
            })
        );
        assert_eq!(
            tree.paint(after[1]),
            Some(crate::style::Paint {
                fill: Some(aqua_core::Oklch::BLACK),
                ..Default::default()
            })
        );
    }

    #[test]
    fn paint_refresh_visible_via_tree_paint() {
        let (_rt, mut tree) = setup();
        let rt2 = Runtime::new();
        let scope = rt2.create_scope();
        let mut view = View::new(scope, tree.command_sink());
        // Shared paint lands on the box AND on the button itself (no wrapper).
        let content = |fill: Option<aqua_core::Oklch>| {
            let mut root_b = ui::Box::builder();
            let mut btn = ui::Button::builder().child(ui::Text::builder().arg("Go").build());
            if let Some(c) = fill {
                let s = Style::new().background(c).radius(px(6.0));
                root_b = root_b.styles([s.clone()]);
                btn = btn.styles([s]);
            }
            vec![root_b.child(btn.build()).build()]
        };
        let ids = view.set_content(&mut tree, content(None)).unwrap();
        layout(&mut tree);
        assert_eq!(tree.paint(ids[0]), Some(crate::style::Paint::default()));
        let ids2 = view
            .set_content(&mut tree, content(Some(aqua_core::Oklch::WHITE)))
            .unwrap();
        layout(&mut tree);
        assert_eq!(ids, ids2);
        let expected = crate::style::Paint {
            fill: Some(aqua_core::Oklch::WHITE),
            radius: 6.0,
            ..Default::default()
        };
        assert_eq!(tree.paint(ids[0]), Some(expected.clone()));
        let btn = tree.get(ids[0]).unwrap().children[0];
        assert_eq!(tree.paint(btn), Some(expected.clone()));
    }
}
