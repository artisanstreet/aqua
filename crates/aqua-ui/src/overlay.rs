//! Window-level overlay service: menus, tooltips, popovers, modal dialogs.
//!
//! Design contract (from `GUI_FRAMEWORK_DESIGN.md` §§ "Z-order, clipping and
//! overlays"):
//! - Logical ownership and rendered placement are separate. A menu owned by a
//!   button inside a scroller is *owned* by that button's subtree for cleanup
//!   and context, but *rendered* in a window-level host above the main scene.
//!   This service is the lifecycle + placement machinery for that host; it is
//!   NOT a second z-order system and it never mutates ancestor clips. There
//!   are no giant-z or clip-escape hacks here: [`OverlayService::render_plan`]
//!   emits ordered entries the host compositor draws above the main scene.
//! - Placement is anchoring with fallback: preferred side, flip on overflow,
//!   shift along the cross axis, then containment clamp to the viewport.
//! - Dismissal is explicit: nested overlays close with their parent, Escape
//!   closes the topmost Escape-dismissible overlay, outside presses
//!   light-dismiss, modals block input beneath, and disposing an owner node
//!   closes everything it owns. Focus is saved on open and restored on close.
//!
//! Integration (core/UI tree is owned by the core agent and has not landed
//! yet): this service is deliberately tree-agnostic. Callers feed it
//! *committed* anchor rects in window-local logical units (the same space as
//! `aqua_core::scene` paint ops) and a [`FocusControl`] adapter over whatever
//! focus scope the tree owns. No scene-tree types are duplicated here.

use aqua_core::id::RawId;
use aqua_core::{Arena, InputEvent, Key, KeyEvent, NamedKey, NodeId, Point, Rect, Size};

// ---------------------------------------------------------------------------
// Handles
// ---------------------------------------------------------------------------

/// Stable handle to an open overlay. Generational (backed by [`Arena`): a
/// closed overlay's handle never aliases a later overlay.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct OverlayId(RawId);

impl OverlayId {
    /// Rebuild a handle from its raw parts (e.g. across a serialization
    /// boundary). Never resolves unless it was issued by the service.
    #[must_use]
    pub const fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    /// Raw parts of this handle.
    #[must_use]
    pub const fn raw(self) -> RawId {
        self.0
    }
}

// ---------------------------------------------------------------------------
// Placement
// ---------------------------------------------------------------------------

/// Which side of the anchor rect the overlay prefers to sit on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    /// Below the anchor (`y = anchor.bottom + gap`).
    Below,
    /// Above the anchor (`y = anchor.y - height - gap`).
    Above,
    /// After the anchor in reading order (`x = anchor.right + gap`).
    After,
    /// Before the anchor (`x = anchor.x - width - gap`).
    Before,
}

impl Side {
    /// Opposite side, used for overflow fallback.
    #[must_use]
    pub fn opposite(self) -> Side {
        match self {
            Side::Below => Side::Above,
            Side::Above => Side::Below,
            Side::After => Side::Before,
            Side::Before => Side::After,
        }
    }

    /// Whether this side stacks along the vertical axis.
    #[must_use]
    pub fn is_vertical(self) -> bool {
        matches!(self, Side::Below | Side::Above)
    }
}

/// Alignment along the cross axis (horizontal for Above/Below, vertical for
/// Before/After).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    /// Leading edge of the anchor.
    Start,
    /// Centered on the anchor.
    Center,
    /// Trailing edge of the anchor.
    End,
}

/// How an overlay wants to be anchored to its activator.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    /// Preferred side; the opposite side is tried on main-axis overflow when
    /// `flip` is set.
    pub preferred: Side,
    /// Cross-axis alignment.
    pub align: Align,
    /// Gap between the anchor rect and the overlay along the side axis.
    pub gap: f32,
    /// Minimum inset kept from every viewport edge (containment margin).
    pub margin: f32,
    /// Whether main-axis overflow falls back to the opposite side.
    pub flip: bool,
    /// When true the overlay is centered in the viewport instead of being
    /// anchored (dialogs); `preferred`/`align`/`gap` are ignored.
    pub centered: bool,
}

impl Placement {
    /// Menu/popover default: below the anchor, centered, flips above.
    #[must_use]
    pub fn below() -> Self {
        Self {
            preferred: Side::Below,
            align: Align::Center,
            gap: 4.0,
            margin: 8.0,
            flip: true,
            centered: false,
        }
    }

    /// Centered in the viewport (modal dialogs).
    #[must_use]
    pub fn centered_dialog() -> Self {
        Self {
            preferred: Side::Below,
            align: Align::Center,
            gap: 0.0,
            margin: 16.0,
            flip: false,
            centered: true,
        }
    }
}

/// Result of [`place_overlay`]: committed bounds plus which side won.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedOverlay {
    /// Bounds in window-local logical units, contained in the viewport
    /// (modulo oversized overlays, which are pinned top-left-most).
    pub bounds: Rect,
    /// Side actually used (differs from preferred only after fallback).
    pub side: Side,
    /// Whether fallback changed the side.
    pub flipped: bool,
}

/// Pure placement function: anchor an overlay of `size` to `anchor` inside
/// `viewport`.
///
/// 1. Position on the preferred side (or centered, for dialogs).
/// 2. Shift along the cross axis so the overlay stays inside the viewport
///    margin (alignment is best-effort near edges — containment wins).
/// 3. On main-axis overflow with `flip`, retry on the opposite side.
/// 4. Clamp the origin into the viewport margin (containment). An overlay
///    larger than the viewport cannot fit; it is pinned so its top-left
///    stays visible rather than centered off-screen.
///
/// Deterministic and total: degenerate (empty) viewports yield an empty rect
/// at the viewport origin instead of NaN.
pub fn place_overlay(
    viewport: Rect,
    anchor: Rect,
    size: Size,
    placement: &Placement,
) -> PlacedOverlay {
    let margin = placement.margin.max(0.0);
    if viewport.is_empty() || size.w <= 0.0 || size.h <= 0.0 {
        return PlacedOverlay {
            bounds: Rect::from_origin_size(viewport.origin(), Size::ZERO),
            side: placement.preferred,
            flipped: false,
        };
    }
    if placement.centered {
        let x = viewport.x + (viewport.w - size.w) * 0.5;
        let y = viewport.y + (viewport.h - size.h) * 0.5;
        return PlacedOverlay {
            bounds: clamp_origin(
                Rect::from_origin_size(Point::new(x, y), size),
                viewport,
                margin,
            ),
            side: placement.preferred,
            flipped: false,
        };
    }

    let fit_raw = |side: Side| -> Rect {
        let (mut x, mut y) = match side {
            Side::Below => (0.0, anchor.bottom() + placement.gap.max(0.0)),
            Side::Above => (0.0, anchor.y - size.h - placement.gap.max(0.0)),
            Side::After => (anchor.right() + placement.gap.max(0.0), 0.0),
            Side::Before => (anchor.x - size.w - placement.gap.max(0.0), 0.0),
        };
        if side.is_vertical() {
            x = match placement.align {
                Align::Start => anchor.x,
                Align::Center => anchor.x + (anchor.w - size.w) * 0.5,
                Align::End => anchor.right() - size.w,
            };
            x = clamp_cross(x, size.w, viewport.x, viewport.right(), margin);
        } else {
            y = match placement.align {
                Align::Start => anchor.y,
                Align::Center => anchor.y + (anchor.h - size.h) * 0.5,
                Align::End => anchor.bottom() - size.h,
            };
            y = clamp_cross(y, size.h, viewport.y, viewport.bottom(), margin);
        }
        Rect::from_origin_size(Point::new(x, y), size)
    };
    // Containment clamping applies to the WINNER only. Fitting is judged on
    // the raw side position: clamping first would force every side to "fit"
    // and silently defeat the flip fallback.

    let fits_main_axis = |bounds: Rect, side: Side| -> bool {
        if side.is_vertical() {
            bounds.y >= viewport.y + margin && bounds.bottom() <= viewport.bottom() - margin
        } else {
            bounds.x >= viewport.x + margin && bounds.right() <= viewport.right() - margin
        }
    };

    let first_raw = fit_raw(placement.preferred);
    if fits_main_axis(first_raw, placement.preferred) {
        return PlacedOverlay {
            bounds: clamp_origin(first_raw, viewport, margin),
            side: placement.preferred,
            flipped: false,
        };
    }
    if placement.flip {
        let other = placement.preferred.opposite();
        let second_raw = fit_raw(other);
        if fits_main_axis(second_raw, other) {
            return PlacedOverlay {
                bounds: clamp_origin(second_raw, viewport, margin),
                side: other,
                flipped: true,
            };
        }
        // Neither side fits: keep whichever side shows more of the overlay
        // (larger visible area after viewport intersection).
        let visible = |b: Rect| b.intersect(viewport).size().area();
        if visible(second_raw) > visible(first_raw) {
            return PlacedOverlay {
                bounds: clamp_origin(second_raw, viewport, margin),
                side: other,
                flipped: true,
            };
        }
    }
    PlacedOverlay {
        bounds: clamp_origin(first_raw, viewport, margin),
        side: placement.preferred,
        flipped: false,
    }
}

/// Clamp a cross-axis origin so `[pos, pos + len]` stays in
/// `[lo + margin, hi - margin]` when it fits; otherwise pin to the margin edge.
fn clamp_cross(pos: f32, len: f32, lo: f32, hi: f32, margin: f32) -> f32 {
    let min = lo + margin;
    let max = hi - margin - len;
    if max <= min {
        min
    } else {
        pos.clamp(min, max)
    }
}

/// Clamp an overlay origin into the viewport margin box. Oversized overlays
/// pin to the top-left margin corner.
fn clamp_origin(bounds: Rect, viewport: Rect, margin: f32) -> Rect {
    let min_x = viewport.x + margin;
    let min_y = viewport.y + margin;
    let max_x = (viewport.right() - margin - bounds.w).max(min_x);
    let max_y = (viewport.bottom() - margin - bounds.h).max(min_y);
    Rect::from_origin_size(
        Point::new(bounds.x.clamp(min_x, max_x), bounds.y.clamp(min_y, max_y)),
        bounds.size(),
    )
}

// ---------------------------------------------------------------------------
// Policy types
// ---------------------------------------------------------------------------

/// What an overlay is (drives host chrome/semantics; behavior comes from
/// modality + dismissal policy, never from this tag alone).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OverlayKind {
    /// Cascading menu / submenu.
    Menu,
    /// Hover tooltip (usually light-dismiss, non-modal, no focus).
    Tooltip,
    /// Anchored popover / combobox list.
    Popover,
    /// Dialog (usually modal + centered).
    Dialog,
}

/// Whether an overlay blocks input to everything beneath it. Modality is
/// independent of visual stacking: an invisible or fading overlay keeps its
/// input policy until it is actually closed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Modality {
    /// Participates in hit-testing only where its bounds are.
    NonModal,
    /// Swallows all pointer/scroll input outside its bounds and is the only
    /// overlay that can be interacted with while open.
    Modal,
}

/// When an overlay closes itself in response to ambient input.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DismissPolicy {
    /// Escape closes the topmost overlay with this set.
    pub escape: bool,
    /// A press outside the overlay's bounds closes it (and its children).
    pub outside_press: bool,
}

impl DismissPolicy {
    /// Only explicit [`OverlayService::close`] / owner disposal.
    pub const MANUAL: DismissPolicy = DismissPolicy {
        escape: false,
        outside_press: false,
    };
    /// Menus, popovers, tooltips: Escape + outside press.
    pub const LIGHT: DismissPolicy = DismissPolicy {
        escape: true,
        outside_press: true,
    };
    /// Modal dialogs: Escape only (a click on the scrim is reported as
    /// blocked, the app decides whether the scrim itself dismisses).
    pub const ESCAPE_ONLY: DismissPolicy = DismissPolicy {
        escape: true,
        outside_press: false,
    };
}

/// Why an overlay closed. Reported for every overlay removed, including
/// cascaded children.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CloseReason {
    /// Explicit [`OverlayService::close`] call.
    Requested,
    /// Escape key.
    Escape,
    /// Press outside the overlay's bounds.
    OutsidePress,
    /// An ancestor overlay closed.
    AncestorClosed,
    /// The owning node was disposed.
    OwnerRemoved,
}

/// One closed overlay: handle, logical owner and reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClosedOverlay {
    /// Closed overlay.
    pub id: OverlayId,
    /// Logical owner (tree node), for context/cleanup.
    pub owner: NodeId,
    /// Why it closed.
    pub reason: CloseReason,
}

/// Focus adapter. The overlay service saves/restores focus around modal (or
/// autofocus) overlays but does NOT own the focus scope — the tree's focus
/// controller implements this two-method trait.
pub trait FocusControl {
    /// Currently focused node, if any.
    fn focused(&self) -> Option<NodeId>;
    /// Move focus (or clear it with `None`). Stale nodes are the
    /// implementor's responsibility to ignore.
    fn set_focused(&mut self, node: Option<NodeId>);
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

/// Everything needed to open an overlay.
#[derive(Clone, Debug)]
pub struct OverlayRequest {
    /// Logical owner: the tree node whose lifetime/context this overlay
    /// belongs to. Disposing the owner closes the overlay.
    pub owner: NodeId,
    /// Nesting parent for cascading dismissal (submenu of a menu, ...).
    /// `None` for top-level overlays.
    pub parent: Option<OverlayId>,
    /// What this overlay is.
    pub kind: OverlayKind,
    /// Input blocking.
    pub modality: Modality,
    /// Ambient-input dismissal.
    pub dismiss: DismissPolicy,
    /// Committed anchor rect in window-local logical units.
    pub anchor: Rect,
    /// Measured overlay size.
    pub size: Size,
    /// Placement preference + fallback.
    pub placement: Placement,
    /// Node to focus on open (`None` = leave focus alone). Saved focus is
    /// restored on close only when focus is still on this node, so a user
    /// tabbing away is never yanked back.
    pub autofocus: Option<NodeId>,
}

/// Error opening an overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayError {
    /// `parent` names an overlay that is not open (stale or never issued).
    UnknownParent,
}

#[derive(Debug)]
struct OverlayRecord {
    owner: NodeId,
    parent: Option<OverlayId>,
    kind: OverlayKind,
    modality: Modality,
    dismiss: DismissPolicy,
    bounds: Rect,
    size: Size,
    placement: Placement,
    side_used: Side,
    autofocus: Option<NodeId>,
    saved_focus: Option<NodeId>,
}

/// The overlay service: one per window. Owns stacking order, placement,
/// modality, dismissal and focus save/restore. All coordinates are
/// window-local logical units.
#[derive(Debug)]
pub struct OverlayService {
    arena: Arena<OverlayRecord>,
    /// Open overlays bottom → top (stacking order within the overlay host).
    order: Vec<RawId>,
    viewport: Rect,
}

impl Default for OverlayService {
    /// Empty service over an empty viewport; call
    /// [`set_viewport`](OverlayService::set_viewport) before opening.
    fn default() -> Self {
        Self::new(Rect::ZERO)
    }
}

impl OverlayService {
    /// Create a service for a viewport (window content bounds, logical units).
    #[must_use]
    pub fn new(viewport: Rect) -> Self {
        Self {
            arena: Arena::new(),
            order: Vec::new(),
            viewport,
        }
    }

    /// Update the viewport (resize, scale-driven reflow): all open overlays
    /// are re-anchored against their stored anchor/size. Returns the ids
    /// whose bounds changed.
    pub fn set_viewport(&mut self, viewport: Rect) -> Vec<OverlayId> {
        self.viewport = viewport;
        // Anchors are not retained (the tree owns layout); without a fresh
        // anchor we re-clamp current bounds into the new viewport. Full
        // re-placement happens via `reposition` when the tree reports the
        // anchor's new committed rect.
        let mut moved = Vec::new();
        for raw in self.order.clone() {
            if let Some(record) = self.arena.get_mut(raw) {
                let clamped =
                    clamp_origin(record.bounds, viewport, record.placement.margin.max(0.0));
                if clamped != record.bounds {
                    record.bounds = clamped;
                    moved.push(OverlayId(raw));
                }
            }
        }
        moved
    }

    /// Open an overlay: places it, pushes it on top of the stack, saves the
    /// current focus and moves focus to `autofocus` when set.
    pub fn open(
        &mut self,
        focus: &mut dyn FocusControl,
        request: OverlayRequest,
    ) -> Result<OverlayId, OverlayError> {
        if let Some(parent) = request.parent {
            if self.arena.get(parent.0).is_none() {
                return Err(OverlayError::UnknownParent);
            }
        }
        let placed = place_overlay(
            self.viewport,
            request.anchor,
            request.size,
            &request.placement,
        );
        let saved_focus = request.autofocus.map(|_| focus.focused()).unwrap_or(None);
        let record = OverlayRecord {
            owner: request.owner,
            parent: request.parent,
            kind: request.kind,
            modality: request.modality,
            dismiss: request.dismiss,
            bounds: placed.bounds,
            size: request.size,
            placement: request.placement,
            side_used: placed.side,
            autofocus: request.autofocus,
            saved_focus,
        };
        let raw = self.arena.insert(record);
        self.order.push(raw);
        if let Some(target) = request.autofocus {
            focus.set_focused(Some(target));
        }
        Ok(OverlayId(raw))
    }

    /// Close an overlay and all its nested descendants. Idempotent: unknown
    /// handles close nothing and report nothing. Returns one
    /// [`ClosedOverlay`] per removed overlay, target first with `reason`,
    /// descendants with [`CloseReason::AncestorClosed`].
    pub fn close(
        &mut self,
        focus: &mut dyn FocusControl,
        id: OverlayId,
        reason: CloseReason,
    ) -> Vec<ClosedOverlay> {
        let Some(record) = self.arena.get(id.0) else {
            return Vec::new();
        };
        let owner = record.owner;
        let mut victims = vec![id.0];
        victims.extend(self.descendants(id));
        // Capture focus pairs before removal: records are gone afterwards.
        let focus_pairs: Vec<(Option<NodeId>, Option<NodeId>)> = victims
            .iter()
            .map(|raw| {
                self.arena
                    .get(*raw)
                    .map(|record| (record.autofocus, record.saved_focus))
                    .unwrap_or((None, None))
            })
            .collect();
        let mut closed = Vec::with_capacity(victims.len());
        for (index, raw) in victims.iter().enumerate() {
            if let Ok(record) = self.arena.remove(*raw) {
                self.order.retain(|kept| *kept != *raw);
                closed.push(ClosedOverlay {
                    id: OverlayId(*raw),
                    owner: record.owner,
                    reason: if index == 0 {
                        reason
                    } else {
                        CloseReason::AncestorClosed
                    },
                });
            }
        }
        let _ = owner;
        Self::restore_focus_pairs(focus, &focus_pairs);
        closed
    }

    /// Close every overlay owned by `owner` (each cascades to its
    /// descendants). For focus purposes the owner is gone: any overlay whose
    /// autofocus still holds focus falls back to its saved focus.
    pub fn dispose_owner(
        &mut self,
        focus: &mut dyn FocusControl,
        owner: NodeId,
    ) -> Vec<ClosedOverlay> {
        // Snapshot first: `close` mutates the arena/order.
        let owned: Vec<OverlayId> = self
            .order
            .iter()
            .filter_map(|raw| {
                self.arena
                    .get(*raw)
                    .filter(|record| record.owner == owner)
                    .map(|_| OverlayId(*raw))
            })
            .collect();
        let mut closed = Vec::new();
        for id in owned {
            // `id` may already have vanished as another owner's descendant.
            closed.extend(self.close(focus, id, CloseReason::OwnerRemoved));
            // Descendants closed via cascade report AncestorClosed; the one
            // overlay actually owned by `owner` must report OwnerRemoved.
            if let Some(entry) = closed.iter_mut().find(|entry| entry.id == id) {
                entry.reason = CloseReason::OwnerRemoved;
            }
        }
        closed
    }

    /// Re-anchor an open overlay after the tree reports a new committed
    /// anchor rect (scroll, reflow, resize). Returns the new bounds, or
    /// `None` for unknown overlays.
    pub fn reposition(&mut self, id: OverlayId, anchor: Rect) -> Option<Rect> {
        let record = self.arena.get_mut(id.0)?;
        let placed = place_overlay(self.viewport, anchor, record.size, &record.placement);
        record.bounds = placed.bounds;
        record.side_used = placed.side;
        Some(placed.bounds)
    }

    /// Whether this handle names an open overlay.
    #[must_use]
    pub fn is_open(&self, id: OverlayId) -> bool {
        self.arena.contains(id.0)
    }

    /// Number of open overlays.
    #[must_use]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Whether no overlay is open.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Topmost overlay, if any.
    #[must_use]
    pub fn topmost(&self) -> Option<OverlayId> {
        self.order.last().map(|raw| OverlayId(*raw))
    }

    /// Open overlays bottom → top.
    #[must_use]
    pub fn stack(&self) -> Vec<OverlayId> {
        self.order.iter().map(|raw| OverlayId(*raw)).collect()
    }

    /// Committed bounds of an open overlay.
    #[must_use]
    pub fn bounds(&self, id: OverlayId) -> Option<Rect> {
        self.arena.get(id.0).map(|record| record.bounds)
    }

    /// Side actually used after fallback.
    #[must_use]
    pub fn side_used(&self, id: OverlayId) -> Option<Side> {
        self.arena.get(id.0).map(|record| record.side_used)
    }

    /// Logical owner of an open overlay.
    #[must_use]
    pub fn owner_of(&self, id: OverlayId) -> Option<NodeId> {
        self.arena.get(id.0).map(|record| record.owner)
    }

    /// Topmost overlay containing `at`, if any.
    #[must_use]
    pub fn overlay_at(&self, at: Point) -> Option<OverlayId> {
        self.order.iter().rev().find_map(|raw| {
            self.arena
                .get(*raw)
                .filter(|record| record.bounds.contains(at))
                .map(|_| OverlayId(*raw))
        })
    }

    /// Topmost modal overlay, if any.
    #[must_use]
    pub fn modal_blocker(&self) -> Option<OverlayId> {
        self.order.iter().rev().find_map(|raw| {
            self.arena
                .get(*raw)
                .filter(|record| record.modality == Modality::Modal)
                .map(|_| OverlayId(*raw))
        })
    }

    /// Ordered draw plan for the window-level overlay host: bottom → top.
    /// `host_z` is the position inside the overlay host layer (which itself
    /// sits above the main scene) — NOT a global z-value competing with
    /// application content, and attaching here never touches the owning
    /// subtree's clips.
    pub fn render_plan(&self) -> Vec<RenderEntry> {
        self.order
            .iter()
            .enumerate()
            .filter_map(|(z, raw)| {
                self.arena.get(*raw).map(|record| RenderEntry {
                    id: OverlayId(*raw),
                    owner: record.owner,
                    kind: record.kind,
                    bounds: record.bounds,
                    modal: record.modality == Modality::Modal,
                    host_z: z as u32,
                })
            })
            .collect()
    }

    /// Route a press: hit inside the topmost overlay under the point, dismiss
    /// light-dismiss overlays on an outside press, or block on a modal.
    ///
    /// Outside-press dismissal starts at the topmost overlay only: if it
    /// declines outside dismissal, nothing closes (a sticky surface holds
    /// its chain open). If it accepts, the dismissal walks up through
    /// ancestors that also accept outside dismissal and closes the
    /// outermost of that chain — one ambient press dismisses a whole menu
    /// hierarchy, never a sticky palette above or below it.
    pub fn handle_pointer_down(
        &mut self,
        focus: &mut dyn FocusControl,
        at: Point,
    ) -> PointerOutcome {
        if let Some(hit) = self.overlay_at(at) {
            // A modal below the hit overlay still blocks: only the topmost
            // modal and overlays above it are interactive.
            if let Some(blocker) = self.modal_blocker() {
                let blocker_pos = self.order.iter().position(|raw| *raw == blocker.0);
                let hit_pos = self.order.iter().position(|raw| *raw == hit.0);
                if blocker_pos.zip(hit_pos).is_some_and(|(b, h)| h < b) {
                    return PointerOutcome::BlockedByModal { overlay: blocker };
                }
            }
            return PointerOutcome::Captured { overlay: hit };
        }
        if let Some(blocker) = self.modal_blocker() {
            return PointerOutcome::BlockedByModal { overlay: blocker };
        }
        let Some(top) = self.topmost() else {
            return PointerOutcome::Ignored;
        };
        if !self
            .arena
            .get(top.0)
            .is_some_and(|record| record.dismiss.outside_press)
        {
            return PointerOutcome::Ignored;
        }
        // Walk up through ancestors that also accept outside dismissal.
        let mut outermost = top.0;
        while let Some(parent) = self.arena.get(outermost).and_then(|record| record.parent) {
            if !self
                .arena
                .get(parent.0)
                .is_some_and(|record| record.dismiss.outside_press)
            {
                break;
            }
            outermost = parent.0;
        }
        let closed = self.close(focus, OverlayId(outermost), CloseReason::OutsidePress);
        PointerOutcome::Dismissed { closed }
    }

    /// Route scroll: overlays under the point receive it, modals block the
    /// rest, otherwise the main UI scrolls. Scrolling never dismisses.
    #[must_use]
    pub fn route_scroll(&self, at: Point) -> ScrollOutcome {
        if let Some(hit) = self.overlay_at(at) {
            if let Some(blocker) = self.modal_blocker() {
                let blocker_pos = self.order.iter().position(|raw| *raw == blocker.0);
                let hit_pos = self.order.iter().position(|raw| *raw == hit.0);
                if blocker_pos.zip(hit_pos).is_some_and(|(b, h)| h < b) {
                    return ScrollOutcome::BlockedByModal { overlay: blocker };
                }
            }
            return ScrollOutcome::DeliveredToOverlay { overlay: hit };
        }
        if let Some(blocker) = self.modal_blocker() {
            ScrollOutcome::BlockedByModal { overlay: blocker }
        } else {
            ScrollOutcome::Ignored
        }
    }

    /// Route a key event. Escape is directed at the current context — the
    /// topmost overlay only. If it declines Escape dismissal, nothing
    /// closes (Escape never skips the top of the stack to reach overlays
    /// below: closing a submenu leaves its parent menu open, while an
    /// outside press dismisses the whole chain). Any other key is captured
    /// by the topmost modal (the tree's focus scope performs the actual
    /// focus trap — this service reports the blocker so the scope knows
    /// what to trap against).
    pub fn handle_key(&mut self, focus: &mut dyn FocusControl, key: &KeyEvent) -> KeyOutcome {
        if key.key == Key::Named(NamedKey::Escape) {
            // Topmost only: a surface that declines Escape holds the stack.
            let accepts = self
                .topmost()
                .and_then(|top| self.arena.get(top.0))
                .is_some_and(|record| record.dismiss.escape);
            match (accepts, self.topmost()) {
                (true, Some(top)) => {
                    let closed = self.close(focus, top, CloseReason::Escape);
                    KeyOutcome::Dismissed { closed }
                }
                _ => KeyOutcome::Ignored,
            }
        } else if let Some(blocker) = self.modal_blocker() {
            KeyOutcome::CapturedByModal { overlay: blocker }
        } else {
            KeyOutcome::Ignored
        }
    }

    /// Convenience router for [`InputEvent`]: presses and keys go through the
    /// overlay policy, everything else is left to the tree.
    pub fn handle_input(
        &mut self,
        focus: &mut dyn FocusControl,
        event: &InputEvent,
    ) -> InputOutcome {
        match event {
            InputEvent::PointerDown(pointer, _) => {
                match self.handle_pointer_down(focus, pointer.position) {
                    PointerOutcome::Captured { overlay } => InputOutcome::Captured { overlay },
                    PointerOutcome::BlockedByModal { overlay } => {
                        InputOutcome::BlockedByModal { overlay }
                    }
                    PointerOutcome::Dismissed { closed } => InputOutcome::Dismissed { closed },
                    PointerOutcome::Ignored => InputOutcome::Ignored,
                }
            }
            InputEvent::KeyDown(key) => match self.handle_key(focus, key) {
                KeyOutcome::Dismissed { closed } => InputOutcome::Dismissed { closed },
                KeyOutcome::CapturedByModal { overlay } => InputOutcome::BlockedByModal { overlay },
                KeyOutcome::Ignored => InputOutcome::Ignored,
            },
            _ => InputOutcome::Ignored,
        }
    }

    /// All nested descendants of `id` (transitive), in stack order.
    fn descendants(&self, id: OverlayId) -> Vec<RawId> {
        let mut found = Vec::new();
        let mut changed = true;
        while changed {
            changed = false;
            for raw in &self.order {
                if found.contains(raw) || *raw == id.0 {
                    continue;
                }
                let dominated = self.arena.get(*raw).is_some_and(|record| {
                    record
                        .parent
                        .is_some_and(|parent| parent.0 == id.0 || found.contains(&parent.0))
                });
                if dominated {
                    found.push(*raw);
                    changed = true;
                }
            }
        }
        found
    }

    /// Restore saved focus for closed overlays, deepest first: an overlay
    /// restores its saved focus only when focus is still on the node it set,
    /// so focus the user moved elsewhere is never yanked back. Cascaded
    /// chains unwind correctly because each child restores the focus its
    /// parent had set. `pairs` parallels the closed list, target first.
    fn restore_focus_pairs(
        focus: &mut dyn FocusControl,
        pairs: &[(Option<NodeId>, Option<NodeId>)],
    ) {
        for (autofocus, saved) in pairs.iter().rev() {
            if let Some(target) = autofocus {
                if focus.focused() == Some(*target) {
                    focus.set_focused(*saved);
                }
            }
        }
    }
}

/// One entry of the window-level overlay draw plan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderEntry {
    /// Overlay to draw.
    pub id: OverlayId,
    /// Logical owner (for context, NOT for placement in the owner's subtree).
    pub owner: NodeId,
    /// Overlay kind (host chrome/semantics hint).
    pub kind: OverlayKind,
    /// Committed bounds in window-local logical units.
    pub bounds: Rect,
    /// Whether this entry blocks input beneath it.
    pub modal: bool,
    /// Position inside the overlay host layer, bottom → top.
    pub host_z: u32,
}

/// Result of routing a pointer press.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PointerOutcome {
    /// Press landed inside an overlay; the tree delivers it there.
    Captured {
        /// Hit overlay (topmost under the point).
        overlay: OverlayId,
    },
    /// A modal swallowed the press outside every overlay.
    BlockedByModal {
        /// Blocking modal.
        overlay: OverlayId,
    },
    /// An outside press dismissed overlays.
    Dismissed {
        /// Everything removed, target first.
        closed: Vec<ClosedOverlay>,
    },
    /// No overlay involved; deliver to the main UI.
    Ignored,
}

/// Result of routing scroll input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollOutcome {
    /// Scroll over an overlay's bounds.
    DeliveredToOverlay {
        /// Receiving overlay.
        overlay: OverlayId,
    },
    /// A modal swallowed scroll outside every overlay.
    BlockedByModal {
        /// Blocking modal.
        overlay: OverlayId,
    },
    /// Main UI scrolls.
    Ignored,
}

/// Result of routing a key event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyOutcome {
    /// Escape dismissed overlays.
    Dismissed {
        /// Everything removed, target first.
        closed: Vec<ClosedOverlay>,
    },
    /// A modal captured a non-Escape key (tree focus scope traps it).
    CapturedByModal {
        /// Blocking modal.
        overlay: OverlayId,
    },
    /// Main UI handles the key.
    Ignored,
}

/// Result of routing a full [`InputEvent`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputOutcome {
    /// Delivered to an overlay.
    Captured {
        /// Receiving overlay.
        overlay: OverlayId,
    },
    /// Swallowed by a modal.
    BlockedByModal {
        /// Blocking modal.
        overlay: OverlayId,
    },
    /// Dismissal happened.
    Dismissed {
        /// Everything removed, target first.
        closed: Vec<ClosedOverlay>,
    },
    /// Not overlay input.
    Ignored,
}

#[cfg(test)]
mod tests {
    use super::*;
    use aqua_core::event::{Modifiers, PointerButton, PointerEvent, PointerKind};

    fn node(index: u32) -> NodeId {
        NodeId::from_raw(RawId {
            index,
            generation: 0,
        })
    }

    fn viewport() -> Rect {
        Rect::from_edges(0.0, 0.0, 800.0, 600.0)
    }

    fn anchor(x: f32, y: f32) -> Rect {
        Rect::from_edges(x, y, x + 100.0, y + 30.0)
    }

    fn size(w: f32, h: f32) -> Size {
        Size::new(w, h)
    }

    #[derive(Debug, Default)]
    struct TestFocus {
        focused: Option<NodeId>,
        log: Vec<Option<NodeId>>,
    }

    impl FocusControl for TestFocus {
        fn focused(&self) -> Option<NodeId> {
            self.focused
        }

        fn set_focused(&mut self, node: Option<NodeId>) {
            self.focused = node;
            self.log.push(node);
        }
    }

    fn menu_request(owner: u32, anchor_rect: Rect) -> OverlayRequest {
        OverlayRequest {
            owner: node(owner),
            parent: None,
            kind: OverlayKind::Menu,
            modality: Modality::NonModal,
            dismiss: DismissPolicy::LIGHT,
            anchor: anchor_rect,
            size: size(160.0, 200.0),
            placement: Placement::below(),
            autofocus: None,
        }
    }

    fn press_at(x: f32, y: f32) -> InputEvent {
        InputEvent::PointerDown(
            PointerEvent {
                contact: 0,
                kind: PointerKind::Mouse,
                position: Point::new(x, y),
                buttons: 1,
            },
            PointerButton::Primary,
        )
    }

    fn escape() -> InputEvent {
        InputEvent::KeyDown(KeyEvent {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::NONE,
            repeat: false,
        })
    }

    // -- placement --------------------------------------------------------

    #[test]
    fn preferred_side_used_when_it_fits() {
        let placed = place_overlay(
            viewport(),
            anchor(100.0, 100.0),
            size(160.0, 200.0),
            &Placement::below(),
        );
        assert_eq!(placed.side, Side::Below);
        assert!(!placed.flipped);
        assert_eq!(placed.bounds.y, 130.0 + 4.0);
        // Centered on the anchor: anchor center x = 150, overlay w = 160.
        assert_eq!(placed.bounds.x, 70.0);
    }

    #[test]
    fn flips_above_when_no_room_below() {
        let placed = place_overlay(
            viewport(),
            anchor(100.0, 560.0),
            size(160.0, 200.0),
            &Placement::below(),
        );
        assert_eq!(placed.side, Side::Above);
        assert!(placed.flipped);
        assert!(placed.bounds.bottom() <= 592.0);
        assert!(placed.bounds.y >= 8.0);
    }

    #[test]
    fn shifts_to_stay_inside_viewport_edges() {
        // Anchor hard against the right edge: centered placement would
        // overflow, so the overlay shifts left and stays contained.
        let placed = place_overlay(
            viewport(),
            anchor(700.0, 100.0),
            size(160.0, 120.0),
            &Placement::below(),
        );
        assert_eq!(placed.side, Side::Below);
        assert!(placed.bounds.right() <= 792.0, "{:?}", placed.bounds);
        assert!(placed.bounds.x >= 8.0);
    }

    #[test]
    fn oversized_overlay_pins_top_left_visible() {
        let placed = place_overlay(
            viewport(),
            anchor(400.0, 300.0),
            size(2000.0, 2000.0),
            &Placement::below(),
        );
        assert_eq!(placed.bounds.x, 8.0);
        assert_eq!(placed.bounds.y, 8.0);
    }

    #[test]
    fn centered_dialog_ignores_anchor() {
        let placed = place_overlay(
            viewport(),
            anchor(10.0, 10.0),
            size(300.0, 200.0),
            &Placement::centered_dialog(),
        );
        assert_eq!(placed.bounds.x, 250.0);
        assert_eq!(placed.bounds.y, 200.0);
        assert!(!placed.flipped);
    }

    #[test]
    fn degenerate_viewport_yields_empty_rect() {
        let placed = place_overlay(
            Rect::ZERO,
            anchor(10.0, 10.0),
            size(50.0, 50.0),
            &Placement::below(),
        );
        assert!(placed.bounds.is_empty());
    }

    // -- stacking / lifecycle ----------------------------------------------

    #[test]
    fn open_close_tracks_stack_and_plan() {
        let mut focus = TestFocus::default();
        let mut service = OverlayService::new(viewport());
        let a = service
            .open(&mut focus, menu_request(1, anchor(10.0, 10.0)))
            .unwrap();
        let b = service
            .open(&mut focus, menu_request(2, anchor(300.0, 10.0)))
            .unwrap();
        assert_eq!(service.topmost(), Some(b));
        assert_eq!(service.stack(), vec![a, b]);
        let plan = service.render_plan();
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].id, a);
        assert_eq!(plan[1].id, b);
        assert_eq!((plan[0].host_z, plan[1].host_z), (0, 1));
        assert!(service.is_open(a) && service.is_open(b));

        let closed = service.close(&mut focus, a, CloseReason::Requested);
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].reason, CloseReason::Requested);
        assert_eq!(closed[0].owner, node(1));
        assert!(!service.is_open(a));
        assert_eq!(service.topmost(), Some(b));
    }

    #[test]
    fn close_is_idempotent_for_unknown_handles() {
        let mut focus = TestFocus::default();
        let mut service = OverlayService::new(viewport());
        let ghost = OverlayId::from_raw(RawId {
            index: 99,
            generation: 0,
        });
        assert!(service
            .close(&mut focus, ghost, CloseReason::Requested)
            .is_empty());
    }

    #[test]
    fn nested_menus_close_with_parent_but_not_vice_versa() {
        let mut focus = TestFocus::default();
        let mut service = OverlayService::new(viewport());
        let parent = service
            .open(&mut focus, menu_request(1, anchor(10.0, 10.0)))
            .unwrap();
        let mut child_req = menu_request(1, anchor(170.0, 40.0));
        child_req.parent = Some(parent);
        let child = service.open(&mut focus, child_req).unwrap();

        // Closing the child alone keeps the parent.
        let closed = service.close(&mut focus, child, CloseReason::Requested);
        assert_eq!(closed.len(), 1);
        assert!(service.is_open(parent));

        // Reopen the child, then close the parent: both go.
        let mut child_req = menu_request(1, anchor(170.0, 40.0));
        child_req.parent = Some(parent);
        let child = service.open(&mut focus, child_req).unwrap();
        let closed = service.close(&mut focus, parent, CloseReason::Requested);
        assert_eq!(closed.len(), 2);
        assert_eq!(
            closed[0],
            ClosedOverlay {
                id: parent,
                owner: node(1),
                reason: CloseReason::Requested
            }
        );
        assert_eq!(closed[1].reason, CloseReason::AncestorClosed);
        assert!(!service.is_open(child));
    }

    #[test]
    fn open_rejects_stale_parent() {
        let mut focus = TestFocus::default();
        let mut service = OverlayService::new(viewport());
        let mut req = menu_request(1, anchor(10.0, 10.0));
        req.parent = Some(OverlayId::from_raw(RawId {
            index: 7,
            generation: 0,
        }));
        assert_eq!(
            service.open(&mut focus, req),
            Err(OverlayError::UnknownParent)
        );
    }

    // -- dismissal ----------------------------------------------------------

    #[test]
    fn escape_dismisses_topmost_escape_closable_only() {
        let mut focus = TestFocus::default();
        let mut service = OverlayService::new(viewport());
        let bottom = service
            .open(&mut focus, menu_request(1, anchor(10.0, 10.0)))
            .unwrap();
        let mut sticky = menu_request(2, anchor(300.0, 10.0));
        sticky.dismiss = DismissPolicy::MANUAL;
        let top = service.open(&mut focus, sticky).unwrap();

        // Topmost declines Escape: nothing closes.
        let outcome = service.handle_key(
            &mut focus,
            &KeyEvent {
                key: Key::Named(NamedKey::Escape),
                modifiers: Modifiers::NONE,
                repeat: false,
            },
        );
        assert_eq!(outcome, KeyOutcome::Ignored);
        assert!(service.is_open(top) && service.is_open(bottom));

        // Remove the sticky overlay, then Escape closes the menu.
        service.close(&mut focus, top, CloseReason::Requested);
        let outcome = service.handle_key(
            &mut focus,
            &KeyEvent {
                key: Key::Named(NamedKey::Escape),
                modifiers: Modifiers::NONE,
                repeat: false,
            },
        );
        match outcome {
            KeyOutcome::Dismissed { closed } => {
                assert_eq!(closed.len(), 1);
                assert_eq!(closed[0].id, bottom);
                assert_eq!(closed[0].reason, CloseReason::Escape);
            }
            other => panic!("expected dismissal, got {other:?}"),
        }
    }

    #[test]
    fn nested_menu_outside_click_closes_whole_chain() {
        let mut focus = TestFocus::default();
        let mut service = OverlayService::new(viewport());
        let parent = service
            .open(&mut focus, menu_request(1, anchor(10.0, 10.0)))
            .unwrap();
        let mut child_req = menu_request(1, anchor(170.0, 40.0));
        child_req.parent = Some(parent);
        let _child = service.open(&mut focus, child_req).unwrap();

        // Press far from both menus: the whole chain light-dismisses.
        let outcome = service.handle_pointer_down(&mut focus, Point::new(790.0, 590.0));
        match outcome {
            PointerOutcome::Dismissed { closed } => {
                assert_eq!(closed.len(), 2);
                assert_eq!(closed[0].id, parent);
                assert_eq!(closed[0].reason, CloseReason::OutsidePress);
                assert_eq!(closed[1].reason, CloseReason::AncestorClosed);
            }
            other => panic!("expected dismissal, got {other:?}"),
        }
        assert!(service.is_empty());
    }

    #[test]
    fn outside_click_inside_topmost_is_captured_not_dismissed() {
        let mut focus = TestFocus::default();
        let mut service = OverlayService::new(viewport());
        let menu = service
            .open(&mut focus, menu_request(1, anchor(10.0, 10.0)))
            .unwrap();
        let bounds = service.bounds(menu).unwrap();
        let outcome =
            service.handle_pointer_down(&mut focus, Point::new(bounds.x + 5.0, bounds.y + 5.0));
        assert_eq!(outcome, PointerOutcome::Captured { overlay: menu });
        assert!(service.is_open(menu));
    }

    #[test]
    fn manual_overlay_survives_outside_click() {
        let mut focus = TestFocus::default();
        let mut service = OverlayService::new(viewport());
        let mut req = menu_request(1, anchor(10.0, 10.0));
        req.dismiss = DismissPolicy::MANUAL;
        let pinned = service.open(&mut focus, req).unwrap();
        let outcome = service.handle_pointer_down(&mut focus, Point::new(790.0, 590.0));
        assert_eq!(outcome, PointerOutcome::Ignored);
        assert!(service.is_open(pinned));
    }

    #[test]
    fn modal_blocks_press_and_scroll_beneath_but_not_inside() {
        let mut focus = TestFocus::default();
        let mut service = OverlayService::new(viewport());
        let mut dialog = OverlayRequest {
            owner: node(9),
            parent: None,
            kind: OverlayKind::Dialog,
            modality: Modality::Modal,
            dismiss: DismissPolicy::ESCAPE_ONLY,
            anchor: viewport(),
            size: size(300.0, 200.0),
            placement: Placement::centered_dialog(),
            autofocus: Some(node(90)),
        };
        let modal = service.open(&mut focus, dialog.clone()).unwrap();
        dialog.owner = node(10);
        dialog.modality = Modality::NonModal;
        dialog.anchor = anchor(600.0, 500.0);
        dialog.size = size(80.0, 40.0);
        dialog.placement = Placement::below();
        dialog.autofocus = None;
        // A non-modal opened under a modal still stacks above it visually,
        // but the modal blocks interaction with it.
        let under = service.open(&mut focus, dialog).unwrap();

        let blocked = service.handle_pointer_down(&mut focus, Point::new(700.0, 590.0));
        assert_eq!(blocked, PointerOutcome::BlockedByModal { overlay: modal });

        let inside = service.bounds(modal).unwrap();
        let captured =
            service.handle_pointer_down(&mut focus, Point::new(inside.x + 4.0, inside.y + 4.0));
        assert_eq!(captured, PointerOutcome::Captured { overlay: modal });

        assert_eq!(
            service.route_scroll(Point::new(700.0, 590.0)),
            ScrollOutcome::BlockedByModal { overlay: modal }
        );
        assert!(service.is_open(under));
    }

    // -- focus ---------------------------------------------------------------

    #[test]
    fn modal_saves_and_restores_focus() {
        let mut focus = TestFocus {
            focused: Some(node(5)),
            log: Vec::new(),
        };
        let mut service = OverlayService::new(viewport());
        let dialog = OverlayRequest {
            owner: node(9),
            parent: None,
            kind: OverlayKind::Dialog,
            modality: Modality::Modal,
            dismiss: DismissPolicy::ESCAPE_ONLY,
            anchor: viewport(),
            size: size(300.0, 200.0),
            placement: Placement::centered_dialog(),
            autofocus: Some(node(90)),
        };
        let modal = service.open(&mut focus, dialog).unwrap();
        assert_eq!(focus.focused(), Some(node(90)));

        service.close(&mut focus, modal, CloseReason::Requested);
        assert_eq!(focus.focused(), Some(node(5)));
    }

    #[test]
    fn close_does_not_clobber_focus_the_user_moved() {
        let mut focus = TestFocus {
            focused: Some(node(5)),
            log: Vec::new(),
        };
        let mut service = OverlayService::new(viewport());
        let dialog = OverlayRequest {
            owner: node(9),
            parent: None,
            kind: OverlayKind::Dialog,
            modality: Modality::Modal,
            dismiss: DismissPolicy::ESCAPE_ONLY,
            anchor: viewport(),
            size: size(300.0, 200.0),
            placement: Placement::centered_dialog(),
            autofocus: Some(node(90)),
        };
        let modal = service.open(&mut focus, dialog).unwrap();
        // User tabs away before the dialog closes.
        focus.set_focused(Some(node(77)));
        service.close(&mut focus, modal, CloseReason::Requested);
        assert_eq!(focus.focused(), Some(node(77)));
    }

    #[test]
    fn non_autofocus_overlay_leaves_focus_alone() {
        let mut focus = TestFocus {
            focused: Some(node(5)),
            log: Vec::new(),
        };
        let mut service = OverlayService::new(viewport());
        let tip = service
            .open(&mut focus, menu_request(1, anchor(10.0, 10.0)))
            .unwrap();
        assert_eq!(focus.focused(), Some(node(5)));
        service.close(&mut focus, tip, CloseReason::OutsidePress);
        assert_eq!(focus.focused(), Some(node(5)));
        assert!(focus.log.is_empty());
    }

    #[test]
    fn owner_disposal_closes_owned_chain_and_restores_focus() {
        let mut focus = TestFocus {
            focused: Some(node(5)),
            log: Vec::new(),
        };
        let mut service = OverlayService::new(viewport());
        let parent = service
            .open(
                &mut focus,
                OverlayRequest {
                    owner: node(1),
                    parent: None,
                    kind: OverlayKind::Menu,
                    modality: Modality::NonModal,
                    dismiss: DismissPolicy::LIGHT,
                    anchor: anchor(10.0, 10.0),
                    size: size(160.0, 200.0),
                    placement: Placement::below(),
                    autofocus: Some(node(11)),
                },
            )
            .unwrap();
        let mut child_req = menu_request(1, anchor(170.0, 40.0));
        child_req.parent = Some(parent);
        let _child = service.open(&mut focus, child_req).unwrap();
        let _other = service
            .open(&mut focus, menu_request(2, anchor(500.0, 10.0)))
            .unwrap();
        assert_eq!(focus.focused(), Some(node(11)));

        let closed = service.dispose_owner(&mut focus, node(1));
        assert_eq!(closed.len(), 2);
        assert!(closed
            .iter()
            .any(|c| c.id == parent && c.reason == CloseReason::OwnerRemoved));
        // Focus was on the disposed overlay's node: falls back to saved.
        assert_eq!(focus.focused(), Some(node(5)));
        // The other owner's menu survives.
        assert_eq!(service.len(), 1);
    }

    // -- input router ----------------------------------------------------------

    #[test]
    fn input_router_delivers_press_key_and_ignores_moves() {
        let mut focus = TestFocus::default();
        let mut service = OverlayService::new(viewport());
        let menu = service
            .open(&mut focus, menu_request(1, anchor(10.0, 10.0)))
            .unwrap();
        let bounds = service.bounds(menu).unwrap();

        let outcome = service.handle_input(&mut focus, &press_at(bounds.x + 5.0, bounds.y + 5.0));
        assert_eq!(outcome, InputOutcome::Captured { overlay: menu });

        let outcome = service.handle_input(&mut focus, &escape());
        match outcome {
            InputOutcome::Dismissed { closed } => assert_eq!(closed[0].id, menu),
            other => panic!("expected dismissal, got {other:?}"),
        }

        let hover = InputEvent::PointerMove(PointerEvent {
            contact: 0,
            kind: PointerKind::Mouse,
            position: Point::new(400.0, 300.0),
            buttons: 0,
        });
        assert_eq!(
            service.handle_input(&mut focus, &hover),
            InputOutcome::Ignored
        );
    }

    // -- reposition --------------------------------------------------------------

    #[test]
    fn reposition_reanchors_after_scroll() {
        let mut focus = TestFocus::default();
        let mut service = OverlayService::new(viewport());
        let menu = service
            .open(&mut focus, menu_request(1, anchor(100.0, 560.0)))
            .unwrap();
        assert_eq!(service.side_used(menu), Some(Side::Above));
        // The anchor scrolls back into open space: the menu flips below again.
        let bounds = service.reposition(menu, anchor(100.0, 100.0)).unwrap();
        assert_eq!(service.side_used(menu), Some(Side::Below));
        assert_eq!(bounds.y, 134.0);
        assert_eq!(
            service.reposition(
                OverlayId::from_raw(RawId {
                    index: 55,
                    generation: 0
                }),
                anchor(0.0, 0.0),
            ),
            None
        );
    }

    #[test]
    fn viewport_resize_reclamps_open_overlays() {
        let mut focus = TestFocus::default();
        let mut service = OverlayService::new(viewport());
        let menu = service
            .open(&mut focus, menu_request(1, anchor(650.0, 500.0)))
            .unwrap();
        let before = service.bounds(menu).unwrap();
        let moved = service.set_viewport(Rect::from_edges(0.0, 0.0, 400.0, 300.0));
        let after = service.bounds(menu).unwrap();
        assert!(after.right() <= 392.0);
        assert!(after.bottom() <= 292.0);
        assert_eq!(moved, vec![menu]);
        let _ = before;
    }
}
