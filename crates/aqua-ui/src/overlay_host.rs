use crate::events::{self, dispatch_scroll};
use crate::motion::{
    MotionController, MotionHandle, MotionKind, MotionValue, ReducedMotion, Seconds,
};
use crate::overlay::{
    CloseReason, ClosedOverlay, DismissPolicy, FocusControl, KeyOutcome, Modality, OverlayId,
    OverlayKind, OverlayRequest, OverlayService, Placement, PointerOutcome, RenderEntry,
};
use crate::style::{Absolute, Length};
use crate::tree::{NodeSpec, UiTree};
use aqua_core::id::RawId;
use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey, NodeId, Point, Rect, Size};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug)]
pub struct PopupRequest {
    pub owner: NodeId,
    pub parent: Option<OverlayId>,
    pub kind: OverlayKind,
    pub modality: Modality,
    pub dismiss: DismissPolicy,
    pub content: NodeSpec,
    pub placement: Placement,
    pub autofocus: Option<NodeId>,
    pub focus_root: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostError {
    NoRoot,
    UnknownOwner,
    NoCommittedRect,
    ContentSizeNotFixed,
    UnknownParentOverlay,
}

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

pub struct OverlayHost {
    closed: Vec<ClosedOverlay>,
    pointer_anchors: HashMap<RawId, Point>,
    service: OverlayService,
    motion: MotionController,
    mounts: HashMap<RawId, NodeId>,
    saved_focus: HashMap<RawId, Option<NodeId>>,
    viewport: Rect,
}

impl OverlayHost {
    #[must_use]
    pub fn new(viewport: Rect) -> Self {
        Self {
            service: OverlayService::new(viewport),
            motion: MotionController::new(ReducedMotion::Full),
            closed: Vec::new(),
            pointer_anchors: HashMap::new(),
            mounts: HashMap::new(),
            saved_focus: HashMap::new(),
            viewport,
        }
    }

    #[must_use]
    pub fn viewport(&self) -> Rect {
        self.viewport
    }

    #[must_use]
    pub fn service(&self) -> &OverlayService {
        &self.service
    }

    #[must_use]
    pub fn motion(&self) -> &MotionController {
        &self.motion
    }

    pub fn motion_mut(&mut self) -> &mut MotionController {
        &mut self.motion
    }

    pub fn take_closed(&mut self) -> Vec<ClosedOverlay> {
        std::mem::take(&mut self.closed)
    }

    #[must_use]
    pub fn popup_root(&self, overlay: OverlayId) -> Option<NodeId> {
        self.mounts.get(&overlay.raw()).copied()
    }

    #[must_use]
    pub fn is_open(&self, overlay: OverlayId) -> bool {
        self.service.is_open(overlay) && self.mounts.contains_key(&overlay.raw())
    }

    pub fn render_plan(&self) -> Vec<RenderEntry> {
        self.service.render_plan()
    }

    pub fn open_popup(
        &mut self,
        tree: &mut UiTree,
        request: PopupRequest,
    ) -> Result<OverlayId, HostError> {
        self.open_popup_at(tree, request, None)
    }

    pub fn open_popup_at(
        &mut self,
        tree: &mut UiTree,
        request: PopupRequest,
        at: Option<Point>,
    ) -> Result<OverlayId, HostError> {
        let root = tree.root().ok_or(HostError::NoRoot)?;
        if tree.get(request.owner).is_none() {
            return Err(HostError::UnknownOwner);
        }
        let owner_anchor = visible_rect(tree, request.owner).ok_or(HostError::NoCommittedRect)?;
        let anchor = at.map_or(owner_anchor, |point| {
            Rect::from_origin_size(point, Size::ZERO)
        });
        let (w, h) = fixed_size(&request.content).ok_or(HostError::ContentSizeNotFixed)?;
        let size = Size::new(w, h);

        let mut provisional = request.content;
        provisional.style.absolute = Some(Absolute {
            left: Some(anchor.x),
            top: Some(anchor.y),
            right: None,
            bottom: None,
        });
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
        tree.mark_portal(popup_root);
        self.mounts.insert(id.raw(), popup_root);
        if let Some(point) = at {
            self.pointer_anchors.insert(id.raw(), point);
        }
        Ok(id)
    }

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

    pub fn sync_lifecycle(&mut self, tree: &mut UiTree) -> Vec<ClosedOverlay> {
        let mut closed = Vec::new();
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
        self.mounts
            .retain(|raw, _| self.service.is_open(OverlayId::from_raw(*raw)));
        closed
    }

    pub fn reanchor_popup(&mut self, tree: &mut UiTree, overlay: OverlayId) -> Option<Rect> {
        let owner = self.service.owner_of(overlay)?;
        let anchor = self
            .pointer_anchors
            .get(&overlay.raw())
            .map(|point| Rect::from_origin_size(*point, Size::ZERO))
            .or_else(|| visible_rect(tree, owner))?;
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
            PointerOutcome::Ignored => {
                press_at(tree, at);
                outcome
            }
            PointerOutcome::BlockedByModal { .. } => outcome,
        }
    }

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

    pub fn handle_key(&mut self, tree: &mut UiTree, key: &KeyEvent) -> KeyOutcome {
        self.route_key(tree, key).0
    }

    fn route_key(&mut self, tree: &mut UiTree, key: &KeyEvent) -> (KeyOutcome, bool) {
        if events::text_is_composing(tree) && self.focused_in_modal_domain(tree) {
            return (KeyOutcome::Ignored, true);
        }
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
                if self.focused_in_modal_domain(tree) {
                    events::dispatch_default(tree, &InputEvent::KeyDown(*key));
                }
                (outcome, true)
            }
            KeyOutcome::Ignored => {
                let consumed = events::dispatch_default(tree, &InputEvent::KeyDown(*key));
                (outcome, consumed)
            }
        }
    }

    pub fn input_allowed(&self, tree: &UiTree, mut node: NodeId) -> bool {
        let Some(modal) = self.service.modal_blocker() else {
            return true;
        };
        loop {
            let Some(parent) = tree.get(node).and_then(|record| record.parent) else {
                return false;
            };
            if Some(parent) == tree.root() {
                break;
            }
            node = parent;
        }
        let Some(overlay) = self.overlay_of_root(node) else {
            return false;
        };
        let stack = self.service.stack();
        let Some(modal_index) = stack.iter().position(|item| *item == modal) else {
            return false;
        };
        stack
            .iter()
            .position(|item| *item == overlay)
            .is_some_and(|index| index >= modal_index)
    }

    fn focused_in_modal_domain(&self, tree: &UiTree) -> bool {
        let Some(modal) = self.service.modal_blocker() else {
            return true;
        };
        let Some(mut node) = tree.focused() else {
            return false;
        };
        loop {
            let Some(parent) = tree.get(node).and_then(|record| record.parent) else {
                return false;
            };
            if Some(parent) == tree.root() {
                break;
            }
            node = parent;
        }
        let Some(overlay) = self.overlay_of_root(node) else {
            return false;
        };
        let stack = self.service.stack();
        let Some(modal_index) = stack.iter().position(|item| *item == modal) else {
            return false;
        };
        stack
            .iter()
            .position(|item| *item == overlay)
            .is_some_and(|index| index >= modal_index)
    }

    pub fn handle_input(&mut self, tree: &mut UiTree, event: &InputEvent) -> bool {
        if self.prepare_input(tree, event) {
            return true;
        }
        tree.cancel_blocked_presses(|tree, node| self.input_allowed(tree, node));
        crate::controls::cancel_blocked(tree, |tree, node| self.input_allowed(tree, node));
        let routed =
            crate::input::route_input(tree, event, |tree, node| self.input_allowed(tree, node));
        let handled = if routed.default_prevented {
            true
        } else {
            self.handle_prepared_input(tree, event) || routed.handled
        };
        tree.drain_commands();
        handled
    }

    pub fn prepare_input(&mut self, tree: &mut UiTree, event: &InputEvent) -> bool {
        let InputEvent::PointerDown(pointer, _) = event else {
            return false;
        };
        let blocked = match self
            .service
            .handle_pointer_down(&mut TreeFocus { tree: &mut *tree }, pointer.position)
        {
            PointerOutcome::Dismissed { closed } => {
                self.finish_close(tree, &closed);
                true
            }
            PointerOutcome::BlockedByModal { .. } => true,
            PointerOutcome::Ignored | PointerOutcome::Captured { .. } => false,
        };
        if blocked {
            tree.cancel_blocked_presses(|tree, node| self.input_allowed(tree, node));
            crate::controls::cancel_blocked(tree, |tree, node| self.input_allowed(tree, node));
            crate::input::cancel_blocked(tree, |tree, node| self.input_allowed(tree, node));
        }
        blocked
    }

    pub fn handle_input_default(&mut self, tree: &mut UiTree, event: &InputEvent) -> bool {
        self.prepare_input(tree, event) || self.handle_prepared_input(tree, event)
    }

    pub fn handle_prepared_input(&mut self, tree: &mut UiTree, event: &InputEvent) -> bool {
        match event {
            InputEvent::PointerDown(..) => events::dispatch_default(tree, event),
            InputEvent::KeyDown(key) => self.route_key(tree, key).1,
            InputEvent::Scroll { at, delta } => self.handle_scroll(tree, *at, *delta),
            InputEvent::TextInput { .. } | InputEvent::Ime(_)
                if !self.focused_in_modal_domain(tree) =>
            {
                true
            }
            _ => events::dispatch_default(tree, event),
        }
    }

    #[must_use]
    pub fn overlay_of_root(&self, root: NodeId) -> Option<OverlayId> {
        self.mounts
            .iter()
            .find(|(_, r)| **r == root)
            .map(|(raw, _)| OverlayId::from_raw(*raw))
    }

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

    pub fn advance_motion(&mut self, now: Seconds) -> Vec<MotionHandle> {
        self.motion.advance(now)
    }

    #[must_use]
    pub fn motion_deadline(&self, now: Seconds) -> Option<Seconds> {
        self.motion.next_deadline(now)
    }

    #[must_use]
    pub fn motion_deadline_duration(&self, now: Duration) -> Option<Duration> {
        self.motion.next_deadline_duration(now)
    }

    pub fn dispose_owner_motion(&mut self, owner: NodeId) -> Vec<MotionHandle> {
        self.motion.dispose_owner(owner)
    }

    fn finish_close(&mut self, tree: &mut UiTree, closed: &[ClosedOverlay]) {
        self.closed.extend_from_slice(closed);
        for entry in closed {
            self.pointer_anchors.remove(&entry.id.raw());
        }
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

    fn remove_mounts(&mut self, tree: &mut UiTree, closed: &[ClosedOverlay]) {
        for entry in closed {
            if let Some(root) = self.mounts.remove(&entry.id.raw()) {
                tree.remove(root);
            }
        }
    }

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

fn press_at(tree: &mut UiTree, at: Point) {
    events::dispatch_default(
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

fn mount_detached(tree: &mut UiTree, parent: Option<NodeId>, spec: NodeSpec) -> NodeId {
    match parent {
        Some(parent) => tree
            .insert_spec(
                parent,
                tree.get(parent).map_or(0, |record| record.children.len()),
                spec,
            )
            .expect("live overlay parent"),
        None => tree.mount_spec(spec),
    }
}

fn fixed_size(spec: &NodeSpec) -> Option<(f32, f32)> {
    match (spec.style.width, spec.style.height) {
        (Length::Px(w), Length::Px(h)) => Some((w, h)),
        _ => None,
    }
}

fn visible_rect(tree: &UiTree, id: NodeId) -> Option<Rect> {
    tree.visual_rect(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::NaiveMeasurer;
    use crate::style::{Axis, LayoutStyle};
    use crate::tree::{BoxContent, NodeKind, ScrollContent, TextContent};
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

        let popup_root = host.popup_root(id).unwrap();
        assert_eq!(tree.get(popup_root).unwrap().parent, Some(root));
        let bounds = host.service().bounds(id).unwrap();
        assert_eq!((bounds.x, bounds.y), (8.0, 84.0));
        assert_eq!(tree.rect(popup_root), Some(bounds));
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

        assert_eq!(
            host.handle_pointer_down(&mut tree, Point::new(10.0, 280.0)),
            PointerOutcome::BlockedByModal { overlay: modal }
        );
        assert!(host.is_open(modal));
        let bounds = host.service().bounds(modal).unwrap();
        assert_eq!(
            host.handle_pointer_down(&mut tree, Point::new(bounds.x + 4.0, bounds.y + 4.0)),
            PointerOutcome::Captured { overlay: modal }
        );
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

        assert_eq!(
            host.open_popup(
                &mut tree,
                menu_request(ghost, popup_content(50.0, 50.0, vec![]))
            ),
            Err(HostError::UnknownOwner)
        );
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
        assert_eq!(tree.get(root).unwrap().children.len(), 0);
        assert!(host.service().stack().is_empty());
    }
}
