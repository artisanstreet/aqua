//! Native accessibility via AccessKit.
//!
//! Footprint split (bloat audit): the portable `accesskit` schema used here
//! (tree building, role/action mapping) is dependency-light and common, so
//! headless semantic conversion + tests compile everywhere. The OS adapter
//! (`accesskit_winit`: UIA on Windows) lives in [`crate::a11y_winit`] and is
//! `cfg(windows)`-gated — on Linux it would pull ~150 AT-SPI/zbus/async
//! packages for an adapter milestone that hasn't landed.
//!
//! This maps the canonical [`aqua_core::semantic::SemanticNode`] tree to a
//! real platform tree. It does NOT claim the `Role` enum alone is
//! accessibility: bounds, hierarchy, focus, actions, and an attached live
//! window are all required, and each gap is reported explicitly (see
//! [`A11yCapability`] and [`A11yNode::bounds`]).
//!
//! Status: tree building + Windows cross-compile verified. Screen-reader
//! end-to-end run: NOT done (no Windows host / no AT on CI) —
//! [`a11y_capability`] reports `end_to_end_verified: false` until then.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use accesskit::{
    Action, ActionData, Node, NodeId as AkNodeId, Rect as AkRect, Role as AkRole, TreeId, TreeInfo,
    TreeUpdate,
};
use aqua_core::{NodeId, Rect, Role as CoreRole};
pub use aqua_protocol::{A11yAction, A11yActionData, A11yActionRequest, A11yNode};

/// Pack a generational core id into an AccessKit `u64` id.
/// `index << 32 | generation`; core `INVALID` (index `u32::MAX`) can never
/// alias a live node, and stale generations never alias either.
pub fn pack_node_id(id: NodeId) -> AkNodeId {
    let raw = id.raw();
    AkNodeId(((u64::from(raw.index) << 32) | u64::from(raw.generation)) as accesskit::NodeIdContent)
}

/// Inverse of [`pack_node_id`] for inbound action requests.
pub fn unpack_node_id(id: AkNodeId) -> NodeId {
    let v = id.0;
    NodeId::from_raw(aqua_core::id::RawId {
        index: (v >> 32) as u32,
        generation: (v & 0xFFFF_FFFF) as u32,
    })
}

/// Canonical role mapping. Every core role has a real target — never
/// `Unknown` by default. This table is the mapping; the `Role` enum alone
/// (without bounds/hierarchy/focus/actions) is NOT native accessibility.
pub fn map_role(role: &CoreRole) -> AkRole {
    match role {
        CoreRole::Group => AkRole::Group,
        CoreRole::Button => AkRole::Button,
        CoreRole::Checkbox => AkRole::CheckBox,
        CoreRole::TextField => AkRole::TextInput,
        CoreRole::Heading(_) => AkRole::Heading,
        CoreRole::List => AkRole::List,
        CoreRole::ListItem => AkRole::ListItem,
        CoreRole::Table => AkRole::Table,
        CoreRole::Row => AkRole::Row,
        CoreRole::ColumnHeader => AkRole::ColumnHeader,
        CoreRole::Cell => AkRole::Cell,
        CoreRole::Slider => AkRole::Slider,
        CoreRole::TabList => AkRole::TabList,
        CoreRole::Tab => AkRole::Tab,
        CoreRole::Image => AkRole::Image,
        CoreRole::ScrollRegion => AkRole::ScrollView,
        CoreRole::Dialog => AkRole::Dialog,
        CoreRole::Menu => AkRole::Menu,
        CoreRole::MenuItem => AkRole::MenuItem,
    }
}

/// Build a full-tree [`TreeUpdate`] from positioned nodes.
/// `root` must be present in `items`; `focus` falls back to `root` when
/// no item is focused. Pure + headless-testable (no window needed).
pub fn build_tree_update(root: NodeId, focus: NodeId, items: &[A11yNode]) -> TreeUpdate {
    let mut nodes = Vec::with_capacity(items.len());
    let mut focus_id = pack_node_id(root);
    for item in items {
        let id = pack_node_id(item.semantic.node);
        if item.semantic.focused || item.semantic.node == focus {
            focus_id = id;
        }
        let mut node = Node::new(map_role(&item.semantic.role));
        if !item.semantic.label.is_empty() {
            node.set_label(item.semantic.label.clone());
        }
        if item.semantic.disabled {
            node.set_disabled();
        }
        if item.semantic.selected {
            node.set_selected(true);
        }
        if let Some(v) = &item.semantic.value_text {
            node.set_value(v.clone());
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
            // Core list positions are one-based; AccessKit is zero-based.
            if let Some(index) = collection.position_in_set.and_then(|n| n.checked_sub(1)) {
                node.set_position_in_set(index);
            }
        }
        // Zero-rect placeholder when layout hasn't positioned the node.
        // Honest degradation: AT needs bounds; UI must re-push after layout.
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

fn map_action(action: &Action) -> A11yAction {
    match action {
        Action::Click => A11yAction::Press,
        Action::Focus => A11yAction::Focus,
        Action::Expand => A11yAction::Expand,
        Action::Collapse => A11yAction::Collapse,
        Action::Increment => A11yAction::Increment,
        Action::Decrement => A11yAction::Decrement,
        Action::Blur => A11yAction::Other("blur"),
        Action::CustomAction => A11yAction::Other("custom"),
        Action::HideTooltip => A11yAction::Other("hide-tooltip"),
        Action::ShowTooltip => A11yAction::Other("show-tooltip"),
        Action::ReplaceSelectedText => A11yAction::Other("replace-selected-text"),
        Action::ScrollDown
        | Action::ScrollLeft
        | Action::ScrollRight
        | Action::ScrollUp
        | Action::ScrollIntoView
        | Action::ScrollToPoint
        | Action::SetScrollOffset => A11yAction::Other("scroll"),
        Action::SetTextSelection => A11yAction::Other("set-text-selection"),
        Action::SetSequentialFocusNavigationStartingPoint => {
            A11yAction::Other("sequential-focus-start")
        }
        Action::SetValue => A11yAction::Other("set-value"),
        Action::ShowContextMenu => A11yAction::Other("context-menu"),
    }
}

/// Lossless payload mapping. `None` (no `data`) is only valid for actions
/// that take none; a `SetValue` without its value is a caller bug upstream
/// and maps to `None` rather than a fabricated value.
pub fn map_action_data(data: Option<&ActionData>) -> A11yActionData {
    match data {
        None => A11yActionData::None,
        Some(ActionData::Value(v)) => A11yActionData::Value(v.to_string()),
        Some(ActionData::NumericValue(n)) => A11yActionData::Numeric(*n),
        Some(ActionData::SetTextSelection(sel)) => A11yActionData::TextSelection {
            anchor_node: unpack_node_id(sel.anchor.node),
            anchor_index: sel.anchor.character_index,
            focus_node: unpack_node_id(sel.focus.node),
            focus_index: sel.focus.character_index,
        },
        Some(ActionData::ScrollToPoint(p)) => A11yActionData::Point { x: p.x, y: p.y },
        Some(ActionData::SetScrollOffset(p)) => A11yActionData::ScrollOffset { x: p.x, y: p.y },
        Some(ActionData::CustomAction(id)) => A11yActionData::Custom(*id),
        // ScrollUnit/ScrollHint only disambiguate scroll actions; the scroll
        // kind itself is in `Action`, so no data is lost by dropping them.
        Some(ActionData::ScrollUnit(_)) | Some(ActionData::ScrollHint(_)) => A11yActionData::None,
    }
}

/// Build the framework request from a native one. Pure: shared by the live
/// adapter and headless tests.
pub fn translate_action_request(request: &accesskit::ActionRequest) -> A11yActionRequest {
    A11yActionRequest {
        node: unpack_node_id(request.target_node),
        action: map_action(&request.action),
        data: map_action_data(request.data.as_ref()),
    }
}

// ---------------------------------------------------------------------------
// Shared bridge queue: `'static` AccessKit handlers <-> adapter.
// Common (no `accesskit_winit` dep) so the wake contract is unit-tested on
// every target, including headless Linux.
// ---------------------------------------------------------------------------

/// Wake callback: invoked AFTER the queue lock is released, once per
/// inbound bridge event (action, initial-tree request, deactivation).
/// The facade supplies its event-loop proxy / waker here; without it a
/// screen-reader action could sit queued forever while the app idles.
/// Must be non-blocking and must not call back into the queue
/// synchronously (it is invoked without the lock held, so `try_lock`
/// succeeds — the unit test asserts this).
pub type WakeFn = Arc<dyn Fn() + Send + Sync>;

/// Shared queue between the `'static` AccessKit handlers and the adapter.
#[derive(Debug, Default)]
pub struct A11yQueue {
    actions: VecDeque<A11yActionRequest>,
    initial: Option<TreeUpdate>,
    active: bool,
}

/// Handler bundle + queue + waker. The Windows adapter moves the handlers
/// into `accesskit_winit`; tests drive them directly.
pub struct A11yBridge {
    queue: Arc<Mutex<A11yQueue>>,
    waker: WakeFn,
}

impl A11yBridge {
    pub fn new(waker: WakeFn) -> Self {
        Self {
            queue: Arc::new(Mutex::new(A11yQueue::default())),
            waker,
        }
    }

    /// No-op waker for tests that only inspect the queue, and for sessions
    /// with no event loop (headless). Production attachments MUST pass a
    /// real waker (see `a11y_winit::A11yAdapter::attach_with_waker`).
    pub fn without_waker() -> Self {
        Self::new(Arc::new(|| {}))
    }

    fn wake(&self) {
        (self.waker)();
    }

    /// Push an inbound action. Wakes exactly once, after lock release.
    pub fn push_action(&self, request: A11yActionRequest) {
        if let Ok(mut q) = self.queue.lock() {
            q.actions.push_back(request);
        }
        self.wake();
    }

    /// Stage the tree served when AT requests the initial tree. Wakes so a
    /// staged-then-idle app still pushes the snapshot on the next dispatch.
    pub fn stage_initial(&self, update: TreeUpdate) {
        if let Ok(mut q) = self.queue.lock() {
            q.initial = Some(update);
            q.active = true;
        }
        self.wake();
    }

    /// Take the staged initial tree for the activation handler.
    pub fn take_staged(&self) -> Option<TreeUpdate> {
        self.queue.lock().ok()?.initial.clone()
    }

    /// Record AT deactivation. Wakes so the app can release AT-held state
    /// (e.g. stop pushing snapshots) instead of discovering it never.
    pub fn deactivate(&self) {
        if let Ok(mut q) = self.queue.lock() {
            q.active = false;
        }
        self.wake();
    }

    pub fn drain_actions(&self) -> Vec<A11yActionRequest> {
        self.queue
            .lock()
            .map(|mut q| q.actions.drain(..).collect())
            .unwrap_or_default()
    }

    pub fn queue_handle(&self) -> Arc<Mutex<A11yQueue>> {
        self.queue.clone()
    }

    pub fn waker_handle(&self) -> WakeFn {
        self.waker.clone()
    }
}

/// `'static` activation handler for `accesskit_winit`, backed by [`A11yBridge`].
pub struct BridgeActivationHandler {
    bridge: Arc<A11yBridge>,
}

impl BridgeActivationHandler {
    pub fn new(bridge: Arc<A11yBridge>) -> Self {
        Self { bridge }
    }
}

impl accesskit::ActivationHandler for BridgeActivationHandler {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        let tree = self.bridge.take_staged();
        // Activation itself is an event: wake so the app refreshes the
        // snapshot even if it staged the tree long before AT attached.
        (self.bridge.waker_handle())();
        tree
    }
}

/// `'static` action handler for `accesskit_winit`, backed by [`A11yBridge`].
pub struct BridgeActionHandler {
    bridge: Arc<A11yBridge>,
}

impl BridgeActionHandler {
    pub fn new(bridge: Arc<A11yBridge>) -> Self {
        Self { bridge }
    }
}

impl accesskit::ActionHandler for BridgeActionHandler {
    fn do_action(&mut self, request: accesskit::ActionRequest) {
        self.bridge.push_action(translate_action_request(&request));
    }
}

/// `'static` deactivation handler for `accesskit_winit`, backed by [`A11yBridge`].
pub struct BridgeDeactivationHandler {
    bridge: Arc<A11yBridge>,
}

impl BridgeDeactivationHandler {
    pub fn new(bridge: Arc<A11yBridge>) -> Self {
        Self { bridge }
    }
}

impl accesskit::DeactivationHandler for BridgeDeactivationHandler {
    fn deactivate_accessibility(&mut self) {
        self.bridge.deactivate();
    }
}

/// Bridge failures. Never panics on misuse — the AccessKit constructor's
/// visible-window panic is pre-checked into [`A11yError::WindowAlreadyVisible`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum A11yError {
    WindowAlreadyVisible,
    LockPoisoned,
}

/// Capability report. `supported` = the AccessKit backend exists for this
/// platform; `end_to_end_verified` stays false until a screen-reader run
/// passes (tracked, not claimed). The live per-window bridge is
/// [`crate::a11y_winit::A11yAdapter`] (`cfg(windows)`); on other targets
/// the tree/action vocabulary above is still available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A11yCapability {
    pub supported: bool,
    pub end_to_end_verified: bool,
    pub detail: &'static str,
}

pub fn a11y_capability() -> A11yCapability {
    #[cfg(windows)]
    {
        A11yCapability {
            supported: true,
            end_to_end_verified: false,
            detail:
                "UIA via AccessKit; needs active AT + attached window; screen-reader run pending",
        }
    }
    #[cfg(target_os = "macos")]
    {
        A11yCapability {
            supported: true,
            end_to_end_verified: false,
            detail: "NSAccessibility via AccessKit; needs active AT + attached window; screen-reader run pending",
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        A11yCapability {
            supported: false,
            end_to_end_verified: false,
            detail: "AT-SPI adapter milestone not landed (accesskit_winit is cfg(windows)-gated); tree vocabulary builds headless",
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        A11yCapability {
            supported: false,
            end_to_end_verified: false,
            detail: "no AccessKit backend on this target",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aqua_core::SemanticNode;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn node_id(i: u32, g: u32) -> NodeId {
        NodeId::from_raw(aqua_core::id::RawId {
            index: i,
            generation: g,
        })
    }

    #[test]
    fn id_pack_round_trips_generations() {
        let id = node_id(7, 3);
        assert_eq!(unpack_node_id(pack_node_id(id)), id);
        // Stale generations never alias.
        assert_ne!(pack_node_id(node_id(7, 3)), pack_node_id(node_id(7, 4)));
    }

    #[test]
    fn every_core_role_maps_to_real_target() {
        use aqua_core::Role as R;
        let cases = [
            (R::Group, AkRole::Group),
            (R::Button, AkRole::Button),
            (R::Checkbox, AkRole::CheckBox),
            (R::TextField, AkRole::TextInput),
            (R::Heading(2), AkRole::Heading),
            (R::List, AkRole::List),
            (R::ListItem, AkRole::ListItem),
            (R::Table, AkRole::Table),
            (R::Row, AkRole::Row),
            (R::ColumnHeader, AkRole::ColumnHeader),
            (R::Cell, AkRole::Cell),
            (R::Slider, AkRole::Slider),
            (R::TabList, AkRole::TabList),
            (R::Tab, AkRole::Tab),
            (R::Image, AkRole::Image),
            (R::ScrollRegion, AkRole::ScrollView),
            (R::Dialog, AkRole::Dialog),
            (R::Menu, AkRole::Menu),
            (R::MenuItem, AkRole::MenuItem),
        ];
        for (core, expect) in cases {
            assert_eq!(map_role(&core), expect, "role map must never be Unknown");
        }
    }

    #[test]
    fn tree_build_is_pure_headless() {
        let root = node_id(1, 0);
        let btn = node_id(2, 0);
        let mut label = SemanticNode::new(btn, aqua_core::Role::Button);
        label.label = "Add".to_string();
        let items = vec![
            A11yNode {
                semantic: SemanticNode::new(root, aqua_core::Role::Group),
                bounds: Some(Rect::from_edges(0.0, 0.0, 800.0, 600.0)),
                children: vec![btn],
            },
            A11yNode {
                semantic: label,
                bounds: Some(Rect::from_edges(8.0, 8.0, 72.0, 32.0)),
                children: vec![],
            },
        ];
        let update = build_tree_update(root, btn, &items);
        assert_eq!(update.nodes.len(), 2);
        assert_eq!(update.focus, pack_node_id(btn));
        assert!(update.tree.is_some());
    }

    #[test]
    fn virtual_collection_coordinates_keep_logical_positions() {
        let root = node_id(10, 0);
        let row = node_id(11, 0);
        let mut table = SemanticNode::new(root, aqua_core::Role::Table);
        table.collection = Some(aqua_core::CollectionSemantics {
            row_count: Some(100_000),
            column_count: Some(3),
            ..Default::default()
        });
        let mut item = SemanticNode::new(row, aqua_core::Role::Cell);
        item.collection = Some(aqua_core::CollectionSemantics {
            row_index: Some(40_000),
            column_index: Some(2),
            position_in_set: Some(40_001),
            ..Default::default()
        });
        let update = build_tree_update(
            root,
            row,
            &[
                A11yNode {
                    semantic: table,
                    bounds: Some(Rect::ZERO),
                    children: vec![row],
                },
                A11yNode {
                    semantic: item,
                    bounds: Some(Rect::ZERO),
                    children: vec![],
                },
            ],
        );
        assert_eq!(update.nodes.len(), 2);
        assert_eq!(update.nodes[0].1.row_count(), Some(100_000));
        assert_eq!(update.nodes[0].1.column_count(), Some(3));
        assert_eq!(update.nodes[1].1.row_index(), Some(40_000));
        assert_eq!(update.nodes[1].1.column_index(), Some(2));
        assert_eq!(update.nodes[1].1.position_in_set(), Some(40_000));
        assert_eq!(update.nodes[1].1.role(), AkRole::Cell);
    }

    #[test]
    fn capability_never_claims_verification() {
        let c = a11y_capability();
        assert!(!c.end_to_end_verified, "no screen-reader run has happened");
        assert!(!c.detail.is_empty());
    }

    #[test]
    fn action_data_preserved_losslessly() {
        let target = pack_node_id(node_id(5, 0));
        let req = |action: Action, data: Option<ActionData>| accesskit::ActionRequest {
            action,
            target_tree: TreeId::ROOT,
            target_node: target,
            data,
        };
        // SetValue keeps its text.
        let r =
            translate_action_request(&req(Action::Click, Some(ActionData::Value("hello".into()))));
        assert_eq!(r.data, A11yActionData::Value("hello".to_string()));
        // Text selection keeps both endpoints.
        let sel = accesskit::TextSelection {
            anchor: accesskit::TextPosition {
                node: target,
                character_index: 2,
            },
            focus: accesskit::TextPosition {
                node: target,
                character_index: 7,
            },
        };
        let r =
            translate_action_request(&req(Action::Click, Some(ActionData::SetTextSelection(sel))));
        match r.data {
            A11yActionData::TextSelection {
                anchor_index: 2,
                focus_index: 7,
                ..
            } => {}
            other => panic!("selection payload lost: {other:?}"),
        }
        // Scroll targets keep coordinates.
        let r = translate_action_request(&req(
            Action::Click,
            Some(ActionData::ScrollToPoint(accesskit::Point::new(3.0, 4.0))),
        ));
        assert_eq!(r.data, A11yActionData::Point { x: 3.0, y: 4.0 });
        // Dataless stays dataless.
        let r = translate_action_request(&req(Action::Focus, None));
        assert_eq!(r.data, A11yActionData::None);
    }

    /// The async-wake regression test: every bridge event wakes EXACTLY once,
    /// and the waker runs WITHOUT the queue lock held (it `try_lock`s to
    /// prove it), so a facade waker that re-enters the queue cannot deadlock.
    #[test]
    fn bridge_wakes_once_per_event_without_lock_held() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let reentered = Arc::new(AtomicUsize::new(0));
        let queue = Arc::new(Mutex::new(A11yQueue::default()));
        // Waker that proves the lock is free: `try_lock` succeeding inside
        // the waker means the bridge released the lock before waking, so a
        // facade waker that re-enters the queue cannot deadlock.
        let strict_waker: WakeFn = {
            let wakes = wakes.clone();
            let reentered = reentered.clone();
            let queue = queue.clone();
            Arc::new(move || {
                wakes.fetch_add(1, Ordering::SeqCst);
                if queue.try_lock().is_ok() {
                    reentered.fetch_add(1, Ordering::SeqCst);
                }
            })
        };
        let bridge = Arc::new(A11yBridge {
            queue: queue.clone(),
            waker: strict_waker,
        });

        // 1. Action push: exactly one wake, lock free during wake.
        bridge.push_action(A11yActionRequest {
            node: node_id(1, 0),
            action: A11yAction::Press,
            data: A11yActionData::None,
        });
        // 2. Activation staging: exactly one wake.
        let root = node_id(1, 0);
        bridge.stage_initial(build_tree_update(
            root,
            root,
            &[A11yNode {
                semantic: SemanticNode::new(root, aqua_core::Role::Group),
                bounds: None,
                children: vec![],
            }],
        ));
        // 3. Deactivation: exactly one wake.
        bridge.deactivate();

        assert_eq!(wakes.load(Ordering::SeqCst), 3, "one wake per event");
        assert_eq!(
            reentered.load(Ordering::SeqCst),
            3,
            "waker must run without the queue lock held"
        );
        // Queue contents intact: action survived, initial staged.
        assert_eq!(bridge.drain_actions().len(), 1);
        assert!(bridge.take_staged().is_some());
    }

    #[test]
    fn handlers_drive_bridge_without_window() {
        let bridge = Arc::new(A11yBridge::without_waker());
        let target = pack_node_id(node_id(9, 1));
        let mut action_handler = BridgeActionHandler::new(bridge.clone());
        // Async AT click travels WITHOUT any mouse input: the request alone
        // must reach the queue with node + payload intact.
        use accesskit::ActionHandler;
        action_handler.do_action(accesskit::ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: target,
            data: Some(ActionData::Value("go".into())),
        });
        let got = bridge.drain_actions();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].node, node_id(9, 1));
        assert_eq!(got[0].action, A11yAction::Press);
        assert_eq!(got[0].data, A11yActionData::Value("go".to_string()));
    }
}
