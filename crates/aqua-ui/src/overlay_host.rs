//! Window-level overlay host: the wiring between [`OverlayService`] and the
//! retained [`UiTree`].
//!
//! The service owns placement, stacking, modality, dismissal and focus
//! save/restore; the tree owns nodes, layout, hit-testing and paint. This
//! host is the single bridge — the facade consumes the host, never the
//! service directly, so there is exactly one overlay system:
//!
//! - Popups mount as **absolute-positioned children of the window root**
//!   (the top layer). They escape ancestor clips (a menu owned by a button
//!   inside a scroller renders under the root, not inside the scroller)
//!   with no renderer z-hacks: normal paint order, last child topmost.
//! - **Logical ownership stays in the service** (`OverlayRequest::owner`):
//!   removing an owner closes its popups via [`sync_lifecycle`](OverlayHost::sync_lifecycle).
//!   The same [`UiTree`] arena holds popup nodes, so [`NodeId`]s never
//!   alias across trees.
//! - Anchor rects come from the owner's **committed** rect, adjusted for
//!   ancestor scroll offsets (visible rect). Popup content roots must
//!   declare fixed `Px` width/height — placement needs a size before
//!   mount, and content-sized popups are `[UNIMPLEMENTED]` (see §8 of
//!   `docs/OVERLAYS_AND_MOTION.md`).
//! - Focus crosses the bridge through the service's [`FocusControl`]
//!   adapter over [`UiTree::focused`]/[`UiTree::set_focused`]. Modal Tab
//!   trapping cycles the modal subtree here (the tree owns Tab order for
//!   the main UI via `events::dispatch_key`; the host constrains it under
//!   modality). Removal order is close-first (service restores focus while
//!   nodes are live) then `tree.remove` — never a dangling focus.
//! - Motion travels with the host too: [`OverlayHost`] owns the
//!   [`MotionController`] and exposes scheduler deadlines
//!   ([`motion_deadline`](OverlayHost::motion_deadline)), so popup
//!   animations use the same clock and the same demand-driven wakeups as
//!   everything else. The facade still owns the event loop; this host is
//!   the usage pattern, not a second scheduler.

use crate::events::{self, dispatch_scroll};
use crate::motion::{
    MotionController, MotionHandle, MotionKind, MotionValue, ReducedMotion, Seconds,
};
use crate::overlay::{
    CloseReason, ClosedOverlay, DismissPolicy, FocusControl, KeyOutcome, Modality, OverlayId,
    OverlayKind, OverlayRequest, OverlayService, Placement, PointerOutcome, RenderEntry,
};
use crate::style::{Absolute, Length};
use crate::tree::{NodeKind, NodeSpec, UiTree};
use aqua_core::id::RawId;
use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey, NodeId, Point, Rect, Size};
use std::collections::HashMap;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Request / error
// ---------------------------------------------------------------------------

/// Everything needed to open a tree-backed popup.
#[derive(Debug)]
pub struct PopupRequest {
    /// Logical owner: the visible anchor rect comes from this node's
    /// committed rect. Removing it closes the popup via `sync_lifecycle`.
    pub owner: NodeId,
    /// Nesting parent overlay for cascading dismissal.
    pub parent: Option<OverlayId>,
    /// What the popup is.
    pub kind: OverlayKind,
    /// Input blocking.
    pub modality: Modality,
    /// Ambient-input dismissal.
    pub dismiss: DismissPolicy,
    /// Popup content. The **root must declare `Px` width and height** (the
    /// builder knows intrinsic menu/dialog sizes); the host overwrites its
    /// `absolute` insets with the placed bounds.
    pub content: NodeSpec,
    /// Placement preference + fallback.
    pub placement: Placement,
    /// Live, focusable node to focus on open (`None` = leave focus alone,
    /// unless `focus_root`). Popup-internal nodes exist only after mount,
    /// so prefer `focus_root` for focusing inside the popup.
    pub autofocus: Option<NodeId>,
    /// Focus the popup root itself on open (host marks it focusable first).
    /// Backs modal dialogs and menus without a pre-known item id.
    pub focus_root: bool,
}

/// Error opening or re-anchoring a popup. No partial state: failures leave
/// the tree and the service unchanged (a provisional mount is rolled back).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostError {
    /// The tree has no window root.
    NoRoot,
    /// `owner` names a dead or never-issued node.
    UnknownOwner,
    /// The owner has no committed rect — run `compute_layout` first.
    NoCommittedRect,
    /// Popup content root lacks fixed `Px` width/height.
    ContentSizeNotFixed,
    /// `parent` names an overlay that is not open.
    UnknownParentOverlay,
}

// ---------------------------------------------------------------------------
// Focus adapter
// ---------------------------------------------------------------------------

/// [`FocusControl`] over the shared tree. Stale nodes are the tree's
/// domain: [`UiTree::set_focused`] validates liveness + focusability and
/// [`UiTree::focused`] filters dead nodes, so focus can never dangle
/// through this adapter.
struct TreeFocus<'a> {
    tree: &'a mut UiTree,
}

impl<'a> FocusControl for TreeFocus<'a> {
    fn focused(&self) -> Option<NodeId> {
        self.tree.focused()
    }

    fn set_focused(&mut self, node: Option<NodeId>) {
        let _ = self.tree.set_focused(node);
    }
}

// ---------------------------------------------------------------------------
// Host
// ---------------------------------------------------------------------------

/// Overlay + motion wiring for one window over one [`UiTree`].
///
/// The facade owns the event loop and calls, per frame, in order:
/// `handle_input` (or the granular routers) → `sync_lifecycle` → mutate →
/// `compute_layout` → `reanchor_all` → paint. Motion: `advance_motion` on
/// wake, `motion_deadline[_duration]` to schedule the wake.
pub struct OverlayHost {
    service: OverlayService,
    motion: MotionController,
    /// Overlay handle (raw) → mounted popup root node. Same-tree invariant:
    /// every popup node lives in the single shared [`UiTree`].
    mounts: HashMap<RawId, NodeId>,
    /// Focus saved by the host when a popup moves it. The service only
    /// saves around its own autofocus; host-driven focus (notably
    /// `focus_root`, whose target node does not exist until after mount)
    /// is restored here, deepest-first, after subtree removal.
    saved_focus: HashMap<RawId, Option<NodeId>>,
    viewport: Rect,
}

impl OverlayHost {
    /// Create a host for a viewport (window content bounds, logical units).
    #[must_use]
    pub fn new(viewport: Rect) -> Self {
        Self {
            service: OverlayService::new(viewport),
            motion: MotionController::new(ReducedMotion::Full),
            mounts: HashMap::new(),
            saved_focus: HashMap::new(),
            viewport,
        }
    }

    /// Window content bounds this host places against.
    #[must_use]
    pub fn viewport(&self) -> Rect {
        self.viewport
    }

    /// Read-only service access (stacking, bounds, render plan queries).
    /// Mutation goes through the host so tree mounts never leak.
    #[must_use]
    pub fn service(&self) -> &OverlayService {
        &self.service
    }

    /// Read-only motion access.
    #[must_use]
    pub fn motion(&self) -> &MotionController {
        &self.motion
    }

    /// Motion access for spawning/retargeting popup animations. Prefer
    /// [`animate_popup`](OverlayHost::animate_popup) so animation ownership
    /// follows overlay ownership; closing a popup does NOT auto-dispose
    /// motion (fade-outs may outlive the popup) — call
    /// [`dispose_owner_motion`](OverlayHost::dispose_owner_motion) when done.
    pub fn motion_mut(&mut self) -> &mut MotionController {
        &mut self.motion
    }

    /// Popup root mounted for an open overlay.
    #[must_use]
    pub fn popup_root(&self, overlay: OverlayId) -> Option<NodeId> {
        self.mounts.get(&overlay.raw()).copied()
    }

    /// Whether an overlay is open and mounted.
    #[must_use]
    pub fn is_open(&self, overlay: OverlayId) -> bool {
        self.service.is_open(overlay) && self.mounts.contains_key(&overlay.raw())
    }

    /// Ordered draw plan for the window-level overlay host (delegates to
    /// the service; popup nodes paint through the normal tree scene).
    pub fn render_plan(&self) -> Vec<RenderEntry> {
        self.service.render_plan()
    }

    /// Open a popup: validate, place via the service, mount as an
    /// absolute-positioned last child of the window root, focus.
    ///
    /// Order: provisional mount at the anchor (so popup nodes exist before
    /// focus moves), service open (placement + focus save), final insets.
    /// A failed service open rolls the provisional mount back — no
    /// half-open state.
    pub fn open_popup(
        &mut self,
        tree: &mut UiTree,
        request: PopupRequest,
    ) -> Result<OverlayId, HostError> {
        let root = tree.root().ok_or(HostError::NoRoot)?;
        if tree.get(request.owner).is_none() {
            return Err(HostError::UnknownOwner);
        }
        let anchor = visible_rect(tree, request.owner).ok_or(HostError::NoCommittedRect)?;
        let (w, h) = fixed_size(&request.content).ok_or(HostError::ContentSizeNotFixed)?;
        let size = Size::new(w, h);

        let mut provisional = request.content;
        provisional.style.absolute = Some(Absolute {
            left: Some(anchor.x),
            top: Some(anchor.y),
            right: None,
            bottom: None,
        });
        // NOTE: mounted via `insert`+`attach`, not `insert_spec`: core's
        // `insert_spec` currently panics inside taffy's `remove_child` for
        // the fresh detached child (reported to parent/core — see §9 of
        // `docs/OVERLAYS_AND_MOTION.md`). Fresh popups need no
        // reconciliation, so attach-by-construction is the right primitive
        // here regardless.
        let popup_root = mount_detached(tree, Some(root), provisional);

        let service_request = OverlayRequest {
            owner: request.owner,
            parent: request.parent,
            kind: request.kind,
            modality: request.modality,
            dismiss: request.dismiss,
            anchor,
            size,
            placement: request.placement,
            autofocus: None, // focused below, once the root exists
        };
        let id = match self
            .service
            .open(&mut TreeFocus { tree: &mut *tree }, service_request)
        {
            Ok(id) => id,
            Err(_) => {
                tree.remove(popup_root);
                return Err(HostError::UnknownParentOverlay);
            }
        };

        if request.focus_root {
            tree.set_focusable(popup_root, true);
        }
        // Explicit autofocus wins; otherwise the popup root itself. The
        // service runs with autofocus None (its target must pre-exist);
        // the host saves focus around its own move instead.
        let saved_before = tree.focused();
        let target = match request.autofocus {
            Some(n) => Some(n),
            None if request.focus_root => Some(popup_root),
            None => None,
        };
        if let Some(t) = target {
            let _ = tree.set_focused(Some(t));
            self.saved_focus.insert(id.raw(), saved_before);
        }

        // Final insets from the service placement.
        if let Some(bounds) = self.service.bounds(id) {
            if let Some(rec) = tree.get(popup_root) {
                let mut style = rec.style.clone();
                style.absolute = Some(Absolute {
                    left: Some(bounds.x),
                    top: Some(bounds.y),
                    right: None,
                    bottom: None,
                });
                tree.set_style(popup_root, style);
            }
        }
        self.mounts.insert(id.raw(), popup_root);
        Ok(id)
    }

    /// Close a popup and its nested descendants: service first (restores
    /// focus while nodes are live), then remove mounted subtrees. Never
    /// leaves a dangling focus — [`UiTree`] falls back to the parent and
    /// [`focused`](UiTree::focused) filters dead nodes.
    pub fn close_popup(
        &mut self,
        tree: &mut UiTree,
        overlay: OverlayId,
        reason: CloseReason,
    ) -> Vec<ClosedOverlay> {
        let closed = self
            .service
            .close(&mut TreeFocus { tree: &mut *tree }, overlay, reason);
        self.finish_close(tree, &closed);
        closed
    }

    /// Reconcile service state with the tree. Call after external tree
    /// edits (reconciliation, owner removal) and once per frame:
    ///
    /// - overlays whose **owner is gone** close via owner disposal
    ///   (`OwnerRemoved`) with mount cleanup;
    /// - overlays whose **popup root is gone** (reconciler dropped the
    ///   subtree) close the same way — no headless overlays;
    /// - stale mount entries (service already closed) are dropped.
    pub fn sync_lifecycle(&mut self, tree: &mut UiTree) -> Vec<ClosedOverlay> {
        let mut closed = Vec::new();
        // Owner-gone pass: snapshot first, disposal mutates.
        let mut dead_owners = Vec::new();
        for id in self.service.stack() {
            if let Some(owner) = self.service.owner_of(id) {
                if tree.get(owner).is_none() && !dead_owners.contains(&owner) {
                    dead_owners.push(owner);
                }
            }
        }
        for owner in dead_owners {
            let batch = self
                .service
                .dispose_owner(&mut TreeFocus { tree: &mut *tree }, owner);
            closed.extend(batch);
        }
        self.finish_close(tree, &closed);
        // Popup-root-gone pass.
        let orphaned: Vec<OverlayId> = self
            .mounts
            .iter()
            .filter(|(raw, root)| {
                self.service.is_open(OverlayId::from_raw(**raw)) && tree.get(**root).is_none()
            })
            .map(|(raw, _)| OverlayId::from_raw(*raw))
            .collect();
        for id in orphaned {
            closed.extend(self.close_popup(tree, id, CloseReason::OwnerRemoved));
        }
        // Stale mount entries.
        self.mounts
            .retain(|raw, _| self.service.is_open(OverlayId::from_raw(*raw)));
        closed
    }

    /// Re-anchor one popup from its owner's current visible rect (call
    /// after scroll/reflow moves the anchor). Updates service placement
    /// and mount insets. `None` for unknown overlays or missing rects.
    pub fn reanchor_popup(&mut self, tree: &mut UiTree, overlay: OverlayId) -> Option<Rect> {
        let owner = self.service.owner_of(overlay)?;
        let anchor = visible_rect(tree, owner)?;
        let bounds = self.service.reposition(overlay, anchor)?;
        if let Some(root) = self.mounts.get(&overlay.raw()).copied() {
            if let Some(rec) = tree.get(root) {
                let mut style = rec.style.clone();
                style.absolute = Some(Absolute {
                    left: Some(bounds.x),
                    top: Some(bounds.y),
                    right: None,
                    bottom: None,
                });
                tree.set_style(root, style);
            }
        }
        Some(bounds)
    }

    /// Re-anchor every open popup (call after `compute_layout`/scroll).
    /// Returns overlays whose bounds changed.
    pub fn reanchor_all(&mut self, tree: &mut UiTree) -> Vec<OverlayId> {
        let stack = self.service.stack();
        let mut moved = Vec::new();
        for id in stack {
            let before = self.service.bounds(id);
            if self.reanchor_popup(tree, id).is_some() && self.service.bounds(id) != before {
                moved.push(id);
            }
        }
        moved
    }

    /// Resize the viewport: service re-clamps, mount insets follow.
    /// Returns overlays whose bounds changed.
    pub fn set_viewport(&mut self, tree: &mut UiTree, viewport: Rect) -> Vec<OverlayId> {
        self.viewport = viewport;
        let moved = self.service.set_viewport(viewport);
        for id in &moved {
            if let Some(root) = self.mounts.get(&id.raw()).copied() {
                if let (Some(bounds), Some(rec)) = (self.service.bounds(*id), tree.get(root)) {
                    let mut style = rec.style.clone();
                    style.absolute = Some(Absolute {
                        left: Some(bounds.x),
                        top: Some(bounds.y),
                        right: None,
                        bottom: None,
                    });
                    tree.set_style(root, style);
                }
            }
        }
        moved
    }

    /// Route a press: overlay policy first, tree dispatch for captured or
    /// ignored presses. Dismissals clean up mounts inline.
    pub fn handle_pointer_down(&mut self, tree: &mut UiTree, at: Point) -> PointerOutcome {
        let outcome = self
            .service
            .handle_pointer_down(&mut TreeFocus { tree: &mut *tree }, at);
        match outcome {
            PointerOutcome::Captured { .. } => {
                press_at(tree, at);
                outcome
            }
            PointerOutcome::Dismissed { ref closed } => {
                self.finish_close(tree, closed);
                outcome
            }
            _ => {
                press_at(tree, at);
                outcome
            }
        }
    }

    /// Route scroll: overlays under the point scroll first, modals block
    /// the rest, otherwise the main UI scrolls. A consumed scroll
    /// re-anchors popups (anchors move with content).
    pub fn handle_scroll(
        &mut self,
        tree: &mut UiTree,
        at: Point,
        delta: aqua_core::ScrollDelta,
    ) -> bool {
        let consumed = match self.service.route_scroll(at) {
            crate::overlay::ScrollOutcome::DeliveredToOverlay { .. } => {
                dispatch_scroll(tree, at, delta)
            }
            crate::overlay::ScrollOutcome::BlockedByModal { .. } => true,
            crate::overlay::ScrollOutcome::Ignored => dispatch_scroll(tree, at, delta),
        };
        if consumed {
            self.reanchor_all(tree);
        }
        consumed
    }

    /// Route a key: Tab/Shift-Tab under modality traps focus in the topmost
    /// modal subtree; Escape dismisses via the service (with mount
    /// cleanup); other keys go to the service, falling through to tree
    /// dispatch when the service ignores them.
    pub fn handle_key(&mut self, tree: &mut UiTree, key: &KeyEvent) -> KeyOutcome {
        self.route_key(tree, key).0
    }

    fn route_key(&mut self, tree: &mut UiTree, key: &KeyEvent) -> (KeyOutcome, bool) {
        if key.key == Key::Named(NamedKey::Tab) && !key.repeat {
            if let Some(modal) = self.service.modal_blocker() {
                let reverse = key.modifiers.contains(Modifiers::SHIFT);
                self.trap_tab(tree, modal, reverse);
                return (KeyOutcome::CapturedByModal { overlay: modal }, true);
            }
        }
        let outcome = self
            .service
            .handle_key(&mut TreeFocus { tree: &mut *tree }, key);
        match outcome {
            KeyOutcome::Dismissed { ref closed } => {
                self.finish_close(tree, closed);
                (outcome, true)
            }
            KeyOutcome::CapturedByModal { .. } => {
                events::dispatch_key(tree, key);
                (outcome, true)
            }
            KeyOutcome::Ignored => {
                let consumed = events::dispatch_key(tree, key);
                (outcome, consumed)
            }
        }
    }

    /// Convenience router for full input events. Returns `true` when
    /// consumed (overlay-handled or tree-handled).
    pub fn handle_input(&mut self, tree: &mut UiTree, event: &InputEvent) -> bool {
        match event {
            InputEvent::PointerDown(pe, _) => match self.handle_pointer_down(tree, pe.position) {
                PointerOutcome::Ignored => events::dispatch_input(tree, event),
                _ => true,
            },
            // route_key already performs tree fallback. Dispatching again
            // doubles Backspace, arrow movement, and field submissions.
            InputEvent::KeyDown(key) => self.route_key(tree, key).1,
            InputEvent::Scroll { at, delta } => self.handle_scroll(tree, *at, *delta),
            _ => events::dispatch_input(tree, event),
        }
    }

    /// Which overlay (if any) owns the already-mounted popup `root`
    /// (a direct child of the window root). `None` for main-UI nodes.
    /// Used for focus-based policy (text input under modality) without
    /// re-hit-testing.
    #[must_use]
    pub fn overlay_of_root(&self, root: NodeId) -> Option<OverlayId> {
        self.mounts
            .iter()
            .find(|(_, r)| **r == root)
            .map(|(raw, _)| OverlayId::from_raw(*raw))
    }

    /// Which overlay (if any) owns the node under `at`: hit-test, then walk
    /// to the window root's direct child and match it against mounts.
    /// Lets the facade attribute input to overlays without duplicating hit
    /// logic.
    pub fn overlay_at_point(&self, tree: &UiTree, at: Point) -> Option<OverlayId> {
        let mut node = events::hit_test(tree, at)?;
        loop {
            let parent = tree.get(node)?.parent;
            match parent {
                None => return None,
                Some(p) if Some(p) == tree.root() => break,
                Some(p) => node = p,
            }
        }
        self.mounts
            .iter()
            .find(|(_, root)| **root == node)
            .map(|(raw, _)| OverlayId::from_raw(*raw))
    }

    // -- motion -----------------------------------------------------------

    /// Spawn a popup animation owned by the overlay's owner node. Closing
    /// the popup does NOT dispose it (fade-outs may outlive the popup) —
    /// call [`dispose_owner_motion`](OverlayHost::dispose_owner_motion).
    pub fn animate_popup(
        &mut self,
        initial: MotionValue,
        kind: MotionKind,
        now: Seconds,
        overlay: OverlayId,
    ) -> Option<MotionHandle> {
        let owner = self.service.owner_of(overlay)?;
        Some(self.motion.spawn(Some(owner), initial, kind, now))
    }

    /// Advance all motion; returns newly completed handles.
    pub fn advance_motion(&mut self, now: Seconds) -> Vec<MotionHandle> {
        self.motion.advance(now)
    }

    /// Earliest time motion needs a frame (`None` when settled). Feed to
    /// the platform scheduler — demand-driven, no fixed tick.
    #[must_use]
    pub fn motion_deadline(&self, now: Seconds) -> Option<Seconds> {
        self.motion.next_deadline(now)
    }

    /// [`motion_deadline`](OverlayHost::motion_deadline) as a [`Duration`].
    #[must_use]
    pub fn motion_deadline_duration(&self, now: Duration) -> Option<Duration> {
        self.motion.next_deadline_duration(now)
    }

    /// Drop motion owned by `owner` (silent completions — see motion docs).
    pub fn dispose_owner_motion(&mut self, owner: NodeId) -> Vec<MotionHandle> {
        self.motion.dispose_owner(owner)
    }

    // -- internals --------------------------------------------------------

    /// Remove mounted subtrees for closed overlays, then restore host-saved
    /// focus (deepest-first cascade walk) when nothing live holds it.
    /// Entries whose focus moved on to a live node are dropped silently.
    fn finish_close(&mut self, tree: &mut UiTree, closed: &[ClosedOverlay]) {
        // Removing a focused subtree installs a live parent fallback. That
        // fallback is not a deliberate focus move and must not suppress
        // restoration to the control that opened the popup.
        let mut ancestor = tree.focused();
        let mut removed_focus = false;
        while let Some(node) = ancestor {
            if closed
                .iter()
                .any(|entry| self.mounts.get(&entry.id.raw()) == Some(&node))
            {
                removed_focus = true;
                break;
            }
            ancestor = tree.get(node).and_then(|rec| rec.parent);
        }
        self.remove_mounts(tree, closed);
        if !removed_focus && tree.focused().is_some() {
            for entry in closed {
                self.saved_focus.remove(&entry.id.raw());
            }
            return;
        }
        for entry in closed.iter().rev() {
            if let Some(saved) = self.saved_focus.remove(&entry.id.raw()) {
                if tree.set_focused(saved) {
                    break;
                }
            }
        }
    }

    /// Remove mounted subtrees for closed overlays; drop stale entries.
    fn remove_mounts(&mut self, tree: &mut UiTree, closed: &[ClosedOverlay]) {
        for entry in closed {
            if let Some(root) = self.mounts.remove(&entry.id.raw()) {
                tree.remove(root);
            }
        }
    }

    /// Cycle Tab/Shift-Tab inside the modal popup subtree (paint order,
    /// skipping disabled). Empty scope consumes Tab and keeps focus.
    fn trap_tab(&mut self, tree: &mut UiTree, modal: OverlayId, reverse: bool) {
        let Some(scope) = self.mounts.get(&modal.raw()).copied() else {
            return;
        };
        let mut order = Vec::new();
        let mut stack = vec![scope];
        while let Some(id) = stack.pop() {
            let Some(rec) = tree.get(id) else { continue };
            if rec.focusable && !tree.is_disabled(id) {
                order.push(id);
            }
            stack.extend(rec.children.iter().rev().copied());
        }
        if order.is_empty() {
            return;
        }
        let next = match tree.focused() {
            Some(cur) => match order.iter().position(|n| *n == cur) {
                Some(pos) => {
                    if reverse {
                        order[(pos + order.len() - 1) % order.len()]
                    } else {
                        order[(pos + 1) % order.len()]
                    }
                }
                None => {
                    if reverse {
                        order[order.len() - 1]
                    } else {
                        order[0]
                    }
                }
            },
            None => {
                if reverse {
                    order[order.len() - 1]
                } else {
                    order[0]
                }
            }
        };
        let _ = tree.set_focused(Some(next));
    }
}

/// Forward a press to normal tree dispatch (focus + activate the hit node).
fn press_at(tree: &mut UiTree, at: Point) {
    events::dispatch_input(
        tree,
        &InputEvent::PointerDown(
            aqua_core::PointerEvent {
                contact: 0,
                kind: aqua_core::PointerKind::Mouse,
                position: at,
                buttons: 1,
            },
            aqua_core::PointerButton::Primary,
        ),
    );
}

/// Mount a spec subtree by construction (`insert` + `attach`), preserving
/// keys. Used for fresh popups, which carry host identity (the OverlayId)
/// and never need reconciling insertion.
fn mount_detached(tree: &mut UiTree, parent: Option<NodeId>, spec: NodeSpec) -> NodeId {
    let NodeSpec {
        kind,
        style,
        paint,
        key,
        children,
        // Window-managed collection containers never mount through overlays;
        // the driver owns their children/scroll, so plain construction drops
        // the marker here (overlays agent: keep in sync if that changes).
        ..
    } = spec;
    let is_box = matches!(&kind, crate::tree::NodeKind::Box(_));
    let id = tree.insert(parent, kind, style);
    if !is_box {
        tree.set_paint(id, paint);
    }
    if key.is_some() {
        if let Some(rec) = tree.get_mut(id) {
            rec.key = key;
        }
    }
    for child in children {
        mount_detached(tree, Some(id), child);
    }
    id
}

/// Fixed `Px` width/height of a popup content root.
fn fixed_size(spec: &NodeSpec) -> Option<(f32, f32)> {
    match (spec.style.width, spec.style.height) {
        (Length::Px(w), Length::Px(h)) => Some((w, h)),
        _ => None,
    }
}

/// Committed rect adjusted for ancestor scroll offsets (the visible
/// anchor). `None` when the node is dead or layout never ran.
fn visible_rect(tree: &UiTree, id: NodeId) -> Option<Rect> {
    let mut rect = tree.rect(id)?;
    let mut cursor = id;
    while let Some(parent) = tree.get(cursor).and_then(|r| r.parent) {
        if let Some(rec) = tree.get(parent) {
            if let NodeKind::Scroll(ref s) = rec.kind {
                rect = rect.translate(-s.offset.x, -s.offset.y);
            }
            cursor = parent;
        } else {
            break;
        }
    }
    Some(rect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::NaiveMeasurer;
    use crate::style::{Axis, LayoutStyle};
    use crate::tree::{BoxContent, ScrollContent, TextContent};
    use aqua_core::{Modifiers, ScrollDelta};

    fn viewport() -> Rect {
        Rect::from_edges(0.0, 0.0, 300.0, 300.0)
    }

    fn px_box(w: f32, h: f32) -> NodeSpec {
        let style = LayoutStyle {
            width: Length::Px(w),
            height: Length::Px(h),
            ..Default::default()
        };
        NodeSpec::new(NodeKind::Box(BoxContent::default()), style)
    }

    fn popup_content(w: f32, h: f32, children: Vec<NodeSpec>) -> NodeSpec {
        let mut spec = px_box(w, h);
        for c in children {
            spec = spec.child(c);
        }
        spec
    }

    fn editable_field() -> NodeSpec {
        NodeSpec::new(
            NodeKind::Text(TextContent::editable("", false)),
            LayoutStyle::default(),
        )
    }

    fn menu_request(owner: NodeId, content: NodeSpec) -> PopupRequest {
        PopupRequest {
            owner,
            parent: None,
            kind: OverlayKind::Menu,
            modality: Modality::NonModal,
            dismiss: DismissPolicy::LIGHT,
            content,
            placement: Placement::below(),
            autofocus: None,
            focus_root: false,
        }
    }

    /// Window with a 300x100 scroller; `owner` sits at content y=300.
    /// Returns (tree, root, scroller, owner).
    fn scrolled_tree() -> (UiTree, NodeId, NodeId, NodeId) {
        let content_style = LayoutStyle {
            axis: Axis::Column,
            width: Length::Px(300.0),
            height: Length::Px(600.0),
            ..Default::default()
        };
        let scroller_style = LayoutStyle {
            width: Length::Px(300.0),
            height: Length::Px(100.0),
            ..Default::default()
        };
        let mut tree = UiTree::new();
        let root = tree.mount_spec(
            px_box(300.0, 300.0).child(
                NodeSpec::new(
                    NodeKind::Scroll(ScrollContent {
                        offset: Point::ZERO,
                    }),
                    scroller_style,
                )
                .child(
                    NodeSpec::new(NodeKind::Box(BoxContent::default()), content_style)
                        .child(px_box(300.0, 300.0))
                        .child(px_box(100.0, 30.0)),
                ),
            ),
        );
        let scroller = tree.get(root).unwrap().children[0];
        let content = tree.get(scroller).unwrap().children[0];
        let owner = tree.get(content).unwrap().children[1];
        tree.compute_layout(Size::new(300.0, 300.0), &NaiveMeasurer)
            .unwrap();
        assert!(tree.scroll_to(scroller, Point::new(0.0, 250.0)));
        (tree, root, scroller, owner)
    }

    #[test]
    fn menu_inside_scroller_mounts_at_root_and_escapes_clip() {
        let (mut tree, root, _scroller, owner) = scrolled_tree();
        let mut host = OverlayHost::new(viewport());
        let id = host
            .open_popup(
                &mut tree,
                menu_request(owner, popup_content(120.0, 60.0, vec![])),
            )
            .unwrap();
        tree.compute_layout(Size::new(300.0, 300.0), &NaiveMeasurer)
            .unwrap();

        // Mounted under the window root, NOT inside the scroller.
        let popup_root = host.popup_root(id).unwrap();
        assert_eq!(tree.get(popup_root).unwrap().parent, Some(root));
        // Placed below the visible anchor: owner visible at y 50..80.
        let bounds = host.service().bounds(id).unwrap();
        assert_eq!((bounds.x, bounds.y), (8.0, 84.0));
        // Committed tree rect matches the service placement.
        assert_eq!(tree.rect(popup_root), Some(bounds));
        // The popup overlaps y 100..144, outside the scroller's 0..100 clip
        // box: hit-testing still finds it (clip escaped via the root mount).
        assert_eq!(
            host.overlay_at_point(&tree, Point::new(10.0, 120.0)),
            Some(id)
        );
        assert!(bounds.bottom() > 100.0);
    }

    #[test]
    fn modal_blocks_background_press_but_captures_inside() {
        let mut tree = UiTree::new();
        let root = tree.mount_spec(px_box(300.0, 300.0).child(editable_field()));
        tree.compute_layout(Size::new(300.0, 300.0), &NaiveMeasurer)
            .unwrap();
        let background = tree.get(root).unwrap().children[0];
        let _ = tree.set_focused(Some(background));

        let mut host = OverlayHost::new(viewport());
        let dialog = PopupRequest {
            owner: background,
            parent: None,
            kind: OverlayKind::Dialog,
            modality: Modality::Modal,
            dismiss: DismissPolicy::ESCAPE_ONLY,
            content: popup_content(200.0, 120.0, vec![]),
            placement: Placement::centered_dialog(),
            autofocus: None,
            focus_root: true,
        };
        let modal = host.open_popup(&mut tree, dialog).unwrap();
        tree.compute_layout(Size::new(300.0, 300.0), &NaiveMeasurer)
            .unwrap();

        // Background press is swallowed by the modal barrier.
        assert_eq!(
            host.handle_pointer_down(&mut tree, Point::new(10.0, 280.0)),
            PointerOutcome::BlockedByModal { overlay: modal }
        );
        assert!(host.is_open(modal));
        // Press inside the dialog is captured.
        let bounds = host.service().bounds(modal).unwrap();
        assert_eq!(
            host.handle_pointer_down(&mut tree, Point::new(bounds.x + 4.0, bounds.y + 4.0)),
            PointerOutcome::Captured { overlay: modal }
        );
        // Scroll over the background is blocked; the tree never sees it.
        assert!(host.handle_scroll(
            &mut tree,
            Point::new(10.0, 280.0),
            ScrollDelta { x: 0.0, y: 50.0 }
        ));
    }

    #[test]
    fn modal_tab_trap_cycles_and_close_restores_focus() {
        let mut tree = UiTree::new();
        let root = tree.mount_spec(px_box(300.0, 300.0).child(editable_field()));
        tree.compute_layout(Size::new(300.0, 300.0), &NaiveMeasurer)
            .unwrap();
        let background = tree.get(root).unwrap().children[0];
        assert!(tree.set_focused(Some(background)));

        let mut host = OverlayHost::new(viewport());
        let modal = host
            .open_popup(
                &mut tree,
                PopupRequest {
                    owner: background,
                    parent: None,
                    kind: OverlayKind::Dialog,
                    modality: Modality::Modal,
                    dismiss: DismissPolicy::ESCAPE_ONLY,
                    content: popup_content(200.0, 120.0, vec![editable_field(), editable_field()]),
                    placement: Placement::centered_dialog(),
                    autofocus: None,
                    focus_root: true,
                },
            )
            .unwrap();
        tree.compute_layout(Size::new(300.0, 300.0), &NaiveMeasurer)
            .unwrap();
        let popup_root = host.popup_root(modal).unwrap();
        assert_eq!(tree.focused(), Some(popup_root));

        let tab = KeyEvent {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers::NONE,
            repeat: false,
        };
        let shift_tab = KeyEvent {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers::SHIFT,
            repeat: false,
        };
        let kids = tree.get(popup_root).unwrap().children.clone();
        // Tab walks root -> field0 -> field1 -> root, never the background.
        assert_eq!(
            host.handle_key(&mut tree, &tab),
            KeyOutcome::CapturedByModal { overlay: modal }
        );
        assert_eq!(tree.focused(), Some(kids[0]));
        host.handle_key(&mut tree, &tab);
        assert_eq!(tree.focused(), Some(kids[1]));
        host.handle_key(&mut tree, &tab);
        assert_eq!(tree.focused(), Some(popup_root));
        host.handle_key(&mut tree, &shift_tab);
        assert_eq!(tree.focused(), Some(kids[1]));
        assert_ne!(tree.focused(), Some(background));

        // Escape closes the dialog and restores the background focus.
        let escape = KeyEvent {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::NONE,
            repeat: false,
        };
        match host.handle_key(&mut tree, &escape) {
            KeyOutcome::Dismissed { closed } => assert_eq!(closed.len(), 1),
            other => panic!("expected dismissal, got {other:?}"),
        }
        assert!(!host.is_open(modal));
        assert_eq!(tree.focused(), Some(background));
        assert!(tree.get(popup_root).is_none());
    }

    #[test]
    fn outside_click_dismisses_and_cleans_the_mount() {
        let (mut tree, _root, _scroller, owner) = scrolled_tree();
        let mut host = OverlayHost::new(viewport());
        let id = host
            .open_popup(
                &mut tree,
                menu_request(owner, popup_content(120.0, 60.0, vec![])),
            )
            .unwrap();
        tree.compute_layout(Size::new(300.0, 300.0), &NaiveMeasurer)
            .unwrap();
        let popup_root = host.popup_root(id).unwrap();

        match host.handle_pointer_down(&mut tree, Point::new(290.0, 290.0)) {
            PointerOutcome::Dismissed { closed } => {
                assert_eq!(closed.len(), 1);
                assert_eq!(closed[0].id, id);
            }
            other => panic!("expected dismissal, got {other:?}"),
        }
        assert!(!host.is_open(id));
        assert!(tree.get(popup_root).is_none());
    }

    #[test]
    fn owner_removal_prunes_popup_via_sync() {
        let (mut tree, _root, _scroller, owner) = scrolled_tree();
        let mut host = OverlayHost::new(viewport());
        let id = host
            .open_popup(
                &mut tree,
                menu_request(owner, popup_content(120.0, 60.0, vec![])),
            )
            .unwrap();
        let popup_root = host.popup_root(id).unwrap();

        assert!(tree.remove(owner));
        let closed = host.sync_lifecycle(&mut tree);
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].reason, CloseReason::OwnerRemoved);
        assert!(!host.is_open(id));
        assert!(tree.get(popup_root).is_none());
        // Focus is live-or-cleared, never dangling.
        assert!(tree.focused().is_none_or(|n| tree.get(n).is_some()));
    }

    #[test]
    fn scroll_routes_to_main_ui_and_reanchors() {
        let (mut tree, _root, scroller, _owner) = scrolled_tree();
        let mut host = OverlayHost::new(viewport());
        let before = match tree.get(scroller).unwrap().kind {
            NodeKind::Scroll(ref s) => s.offset,
            _ => unreachable!(),
        };
        assert!(host.handle_scroll(
            &mut tree,
            Point::new(10.0, 10.0),
            ScrollDelta { x: 0.0, y: 20.0 }
        ));
        let after = match tree.get(scroller).unwrap().kind {
            NodeKind::Scroll(ref s) => s.offset,
            _ => unreachable!(),
        };
        assert_eq!(after.y, before.y + 20.0);
    }

    #[test]
    fn motion_deadline_helper_drives_popup_animation() {
        use crate::motion::{Easing, MotionKind, MotionValue};
        use std::cell::RefCell;
        use std::rc::Rc;

        let (mut tree, _root, _scroller, owner) = scrolled_tree();
        let mut host = OverlayHost::new(viewport());
        let id = host
            .open_popup(
                &mut tree,
                menu_request(owner, popup_content(120.0, 60.0, vec![])),
            )
            .unwrap();
        let handle = host
            .animate_popup(
                MotionValue::Scalar(0.0),
                MotionKind::Tween {
                    duration_secs: 0.1,
                    easing: Easing::Linear,
                },
                0.0,
                id,
            )
            .unwrap();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let seen2 = seen.clone();
        host.motion_mut()
            .on_complete(handle, Box::new(move |c| seen2.borrow_mut().push(c)));
        host.motion_mut()
            .set_target(handle, MotionValue::Scalar(1.0), 0.0)
            .unwrap();
        assert_eq!(host.motion_deadline(0.0), Some(0.0));

        // Advance mid-flight, then dispose the owner: the pending
        // completion is dropped silently and never fires.
        host.advance_motion(0.05);
        assert!(host.motion_deadline(0.05).is_some());
        host.dispose_owner_motion(owner);
        assert!(seen.borrow().is_empty());
        assert_eq!(host.motion_deadline(0.05), None);
    }

    #[test]
    fn open_errors_leave_no_partial_state() {
        use aqua_core::id::RawId;

        let mut tree = UiTree::new();
        let root = tree.mount_spec(px_box(300.0, 300.0));
        tree.compute_layout(Size::new(300.0, 300.0), &NaiveMeasurer)
            .unwrap();
        let mut host = OverlayHost::new(viewport());
        let ghost = NodeId::from_raw(RawId {
            index: 99,
            generation: 0,
        });

        // Unknown owner.
        assert_eq!(
            host.open_popup(
                &mut tree,
                menu_request(ghost, popup_content(50.0, 50.0, vec![]))
            ),
            Err(HostError::UnknownOwner)
        );
        // Content-sized root.
        let auto_style = LayoutStyle {
            width: Length::Auto,
            height: Length::Px(50.0),
            ..Default::default()
        };
        let mut bad = menu_request(root, popup_content(50.0, 50.0, vec![]));
        bad.content.style = auto_style;
        assert_eq!(
            host.open_popup(&mut tree, bad),
            Err(HostError::ContentSizeNotFixed)
        );
        // Nothing mounted, nothing open.
        assert_eq!(tree.get(root).unwrap().children.len(), 0);
        assert!(host.service().stack().is_empty());
    }
}
