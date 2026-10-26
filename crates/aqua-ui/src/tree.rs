//! Retained UI tree: stable node identities, parent/child structure, dirty
//! flags, Taffy layout integration, and scene emission.
//!
//! Update model (binding contract for the reactive owner, the macro-side
//! element builders, and the facade host):
//!
//! - Mount once: [`UiTree::mount_spec`] builds retained nodes from a
//!   [`NodeSpec`] descriptor tree. Afterwards updates are targeted and
//!   in place — handling an event mutates affected nodes and marks them
//!   dirty. A full remount happens only on structural replacement, never
//!   per signal.
//! - The macro-side `Element` type (macro agent's `element.rs`) converts into
//!   [`NodeSpec`] (`kind` + `style` + `key` + `children`; plain data, no
//!   closures). Dynamic behavior binds separately: effects and event actions
//!   reference stable [`NodeId`]s, never rebuilt subtrees.
//! - Reactive bindings enqueue [`UiCommand`]s (shared command queue). The
//!   tree drains the queue at the start of [`UiTree::compute_layout`], so
//!   effects observe a coherent committed layout and never reenter the tree:
//!   there is no `RefCell`, all methods take `&mut self`, painter and action
//!   callbacks receive data only (they cannot reach back into the tree).
//!   Callbacks notify the host (channels/signals); tree mutations flow back
//!   as [`UiCommand`]s at the next boundary.
//! - Keyed reconciliation ([`UiTree::sync_children`]) reuses nodes by stable
//!   key across insert/remove/reorder, so per-row effect scopes (reactive
//!   owner, keyed by [`NodeId`]) and editing state survive structural updates.
//! - Text editing state lives in `aqua-text` types (`TextModel` for static
//!   labels, `TextEditor` for editable nodes): grapheme-safe by construction,
//!   with undo/IME owned by the editor.
//!
//! Interface for element mount (macro agent): [`NodeSpec`], [`UiTree`]
//! create methods ([`UiTree::mount_spec`], [`UiTree::insert_spec`]),
//! update methods ([`UiTree::set_text`], [`UiTree::set_style`],
//! [`UiTree::scroll_to`], [`UiTree::set_focused`], [`UiTree::drain_commands`]
//! via [`UiTree::push_command`]), and event registration
//! ([`UiTree::set_action_callback`], fired by event dispatch in `events.rs`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use aqua_core::{
    media_layer, Arena, CollectionSemantics, DisplayItem, ImageId, NodeId, Oklch, Point, RawId,
    Rect, Role, SemanticNode, Size, TextRun, ViewportId,
};
use aqua_text::{LineMode, Selection, TextEditor, TextModel};
use taffy::{AvailableSpace, LayoutInput, LayoutOutput, TaffyTree};

use crate::layout::TextMeasurer;
use crate::style::{LayoutStyle, Paint};

/// Keyed-identity marker for reconciliation. Keys hash to `u64` with a fixed
/// seed so equal keys match within and across runs.
pub type KeyHash = u64;

/// Hash a user key with a fixed seed (deterministic within and across runs).
#[must_use]
pub fn hash_key(key: impl Hash) -> KeyHash {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
}

/// Text backing store. Static labels hold a [`TextModel`] (no editing
/// machinery allocated); editable nodes hold a [`TextEditor`] (grapheme-safe
/// ops, selection, IME preedit, bounded undo). Exactly one is present.
pub enum TextBacking {
    /// Read-only content (selection state lives in [`TextContent::selection`]).
    Static(TextModel),
    /// Editable content (selection/undo/IME owned by the editor).
    Editable(TextEditor),
}

/// Text node content and editing state.
pub struct TextContent {
    /// Content store.
    pub backing: TextBacking,
    /// Font size in logical px.
    pub font_size: f32,
    /// Text color.
    pub color: Oklch,
    /// Whether a static label can be selected (editing implies selection).
    pub selectable: bool,
    /// Selection for static labels (`None` = caret-less, unselected).
    pub selection: Option<Selection>,
    /// Static-label wrapping (`editable` nodes follow their [`LineMode`]).
    pub wrap: bool,
    /// Password masking (masked bullets for measure/display).
    pub password: bool,
    /// Placeholder hint shown when an editable node is empty. Display-only:
    /// committed text stays empty, caret/selection indexes are unaffected,
    /// and the placeholder never enters input copy/submit paths. Ignored for
    /// static labels. Shown unmasked even in password fields (standard hint
    /// behavior); IME preedit takes precedence while composing.
    pub placeholder: String,
}

impl TextContent {
    /// Static label: model-backed, no editing machinery.
    pub fn label(text: impl Into<String>) -> Self {
        Self {
            backing: TextBacking::Static(TextModel::new(text)),
            font_size: 14.0,
            color: Oklch::WHITE,
            selectable: false,
            selection: None,
            wrap: false,
            password: false,
            placeholder: String::new(),
        }
    }

    /// Editable node: editor-backed.
    pub fn editable(text: impl Into<String>, multiline: bool) -> Self {
        Self {
            backing: TextBacking::Editable(TextEditor::with_text(
                &text.into(),
                if multiline {
                    LineMode::Multi
                } else {
                    LineMode::Single
                },
            )),
            font_size: 14.0,
            color: Oklch::WHITE,
            selectable: true,
            selection: None,
            wrap: multiline,
            password: false,
            placeholder: String::new(),
        }
    }

    /// Committed text (without IME preedit).
    #[must_use]
    pub fn text(&self) -> &str {
        match &self.backing {
            TextBacking::Static(m) => m.text(),
            TextBacking::Editable(e) => e.text(),
        }
    }

    /// Display text: committed text plus active IME preedit for editable
    /// nodes, masked per char when [`TextContent::password`] is set — except
    /// the placeholder (see [`TextContent::is_placeholder`]), which shows
    /// unmasked. This is what measurement and the text provider shape.
    #[must_use]
    pub fn display_text(&self) -> String {
        if self.is_placeholder() {
            return self.placeholder.clone();
        }
        let s = match &self.backing {
            TextBacking::Static(m) => m.text().to_string(),
            TextBacking::Editable(e) => e.display_text(),
        };
        if self.password {
            s.chars().map(|_| '•').collect()
        } else {
            s
        }
    }

    /// Whether the placeholder is currently shown: editable node, empty
    /// committed text, no active IME preedit, non-empty placeholder.
    /// Renderers use this flag for distinct muted styling. The backing model
    /// stays empty, so caret/selection indexes and copy/submit paths are
    /// unaffected — the placeholder exists only in [`TextContent::display_text`].
    #[must_use]
    pub fn is_placeholder(&self) -> bool {
        if !self.is_editable() || self.placeholder.is_empty() {
            return false;
        }
        match &self.backing {
            TextBacking::Static(_) => false,
            TextBacking::Editable(e) => e.text().is_empty() && !e.is_composing(),
        }
    }

    /// Content revision (bumps on every mutation; feeds `TextRun.revision`).
    #[must_use]
    pub fn revision(&self) -> u64 {
        match &self.backing {
            TextBacking::Static(m) => m.revision(),
            TextBacking::Editable(e) => e.revision(),
        }
    }

    /// Whether lines wrap at the available width.
    #[must_use]
    pub fn multiline(&self) -> bool {
        match &self.backing {
            TextBacking::Static(_) => self.wrap,
            TextBacking::Editable(e) => e.line_mode() == LineMode::Multi,
        }
    }

    /// Replace content, preserving editor state (caret clamps, undo records).
    pub fn set_text(&mut self, text: &str) {
        match &mut self.backing {
            TextBacking::Static(m) => m.set_text(text),
            TextBacking::Editable(e) => e.set_text(text),
        }
        if let Some(sel) = &mut self.selection {
            sel.clamp(text.len());
        }
    }

    /// Insert at the caret (editable only). Returns `false` for static text.
    pub fn insert_at_caret(&mut self, s: &str) -> bool {
        match &mut self.backing {
            TextBacking::Static(_) => false,
            TextBacking::Editable(e) => {
                e.insert(s);
                true
            }
        }
    }

    /// Delete the grapheme before the caret (editable only).
    pub fn delete_backward(&mut self) -> bool {
        match &mut self.backing {
            TextBacking::Static(_) => false,
            TextBacking::Editable(e) => e.delete_backward(),
        }
    }

    /// Delete the grapheme after the caret (editable only).
    pub fn delete_forward(&mut self) -> bool {
        match &mut self.backing {
            TextBacking::Static(_) => false,
            TextBacking::Editable(e) => e.delete_forward(),
        }
    }

    /// Move the caret one grapheme, optionally extending the selection.
    pub fn move_caret(&mut self, left: bool, extend: bool) -> bool {
        match &mut self.backing {
            TextBacking::Static(_) => false,
            TextBacking::Editable(e) => {
                if left {
                    e.move_left(extend);
                } else {
                    e.move_right(extend);
                }
                true
            }
        }
    }

    /// Current caret byte offset (editor caret, or static selection caret).
    #[must_use]
    pub fn caret(&self) -> Option<usize> {
        match &self.backing {
            TextBacking::Static(_) => self.selection.map(|s| s.caret),
            TextBacking::Editable(e) => Some(e.selection().caret),
        }
    }

    /// Whether this node hosts editing machinery.
    #[must_use]
    pub fn is_editable(&self) -> bool {
        matches!(self.backing, TextBacking::Editable(_))
    }
}

impl fmt::Debug for TextContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextContent")
            .field("text", &self.text())
            .field("font_size", &self.font_size)
            .field("editable", &self.is_editable())
            .finish()
    }
}

/// Box (container) paint.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BoxContent {
    /// Paint properties.
    pub paint: Paint,
}

/// Media presentation state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MediaContent {
    /// Image asset (`None` = not yet loaded; node keeps layout, paints nothing).
    pub image: Option<ImageId>,
    /// Opacity multiplier.
    pub opacity: f32,
}

/// Scroll viewport state. Content size is derived from laid-out children.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScrollContent {
    /// Current scroll offset in logical units (clamped to content).
    pub offset: Point,
}

/// Canvas custom-drawing binding. The painter callback receives the committed
/// logical bounds and returns scene ops to splice at this position; it cannot
/// reach back into the tree (no reentrancy by signature). The callback lives
/// in a registry ([`UiTree::set_canvas_painter`]) so nodes stay plain data.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CanvasContent {
    /// Registered painter (`None` = reserve layout space, emit nothing).
    pub painter: Option<u64>,
}

/// Custom draw callback: committed logical bounds in, scene ops out.
pub type CanvasPaintCallback = Box<dyn Fn(Rect) -> Vec<DisplayItem>>;

/// Programmable GPU content binding. `layer` names the external surface:
/// image layers key to the [`ImageId`] slot index; viewport/custom layers
/// use the media owner's namespace (see contract §4).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ViewportContent {
    /// Viewport identity.
    pub viewport: Option<ViewportId>,
    /// External layer key (`None` = nothing to composite yet).
    pub layer: Option<u64>,
}

/// The six retained primitives. Rows/columns/grids are `Box` configurations,
/// not extra kinds.
pub enum NodeKind {
    /// Child composition, background, border, clipping.
    Box(BoxContent),
    /// Displayed text with optional selection/editing.
    Text(TextContent),
    /// Visual media presentation.
    Media(MediaContent),
    /// Scrollable viewport.
    Scroll(ScrollContent),
    /// Application custom drawing via a registered painter.
    Canvas(CanvasContent),
    /// Programmable GPU content under ordinary UI layout/clipping.
    Viewport3D(ViewportContent),
}

impl fmt::Debug for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeKind::Box(b) => f.debug_tuple("Box").field(b).finish(),
            NodeKind::Text(t) => f.debug_tuple("Text").field(t).finish(),
            NodeKind::Media(m) => f.debug_tuple("Media").field(m).finish(),
            NodeKind::Scroll(s) => f.debug_tuple("Scroll").field(s).finish(),
            NodeKind::Canvas(c) => f.debug_tuple("Canvas").field(c).finish(),
            NodeKind::Viewport3D(v) => f.debug_tuple("Viewport3D").field(v).finish(),
        }
    }
}

/// Mount/update descriptor consumed by [`UiTree::mount_spec`],
/// [`UiTree::sync_children`] and [`UiTree::insert_spec`]. Plain data:
///
/// - `kind` + `style` + `children` + `key` describe structure and paint;
/// - dynamic behavior binds separately through stable [`NodeId`]s
///   ([`UiCommand`] queue, [`UiTree::set_action_callback`]).
///
/// The macro-side `Element` type converts into this struct (macro agent owns
/// that conversion); the tree never imports macro-owned types.
#[derive(Debug)]
pub struct NodeSpec {
    /// Node properties.
    pub kind: NodeKind,
    /// Typed style.
    pub style: LayoutStyle,
    /// Paint for non-box nodes; boxes retain their legacy `BoxContent::paint`.
    pub paint: Paint,
    /// Stable reconciliation key (`None` = positional).
    pub key: Option<KeyHash>,
    /// Collection window management: when set, the collection runtime driver
    /// owns this container's children and scroll offsets. `update_spec`
    /// applies style/paint only — it never overwrites the live scroll offset
    /// from a stateless spec and never re-syncs children (which would clobber
    /// the realized window with a first-paint guess). Set by collection
    /// lowering for container/header/body nodes; plain nodes leave it false.
    pub collection_managed: bool,
    /// Children in paint order.
    pub children: Vec<NodeSpec>,
}

impl NodeSpec {
    /// Bare descriptor.
    pub fn new(kind: NodeKind, style: LayoutStyle) -> Self {
        Self {
            kind,
            style,
            paint: Paint::default(),
            key: None,
            collection_managed: false,
            children: Vec::new(),
        }
    }

    /// Attach a stable key (fixed-seed hash of any hashable value).
    #[must_use]
    pub fn keyed(mut self, key: impl Hash) -> Self {
        self.key = Some(hash_key(key));
        self
    }

    /// Append a child (paint order).
    #[must_use]
    pub fn child(mut self, c: NodeSpec) -> Self {
        self.children.push(c);
        self
    }
}

/// Action a node can raise (button activation, text commit, ...). Registration
/// lives in the tree; dispatch lives in `events.rs`; handling lives in the
/// host (facade/reactive), which routes back through [`UiCommand`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionKind {
    /// Activation gesture (click/tap/Enter on an activatable node).
    Activate,
    /// Editable text content changed (carries the new committed text).
    TextChanged,
    /// Keyboard focus entered or left the node.
    FocusChanged,
    /// Enter in a single-line editable field.
    Submit,
}

/// Event delivered to an action callback. Data only — no tree access, so
/// callbacks cannot reenter the tree during dispatch.
#[derive(Debug, Clone)]
pub struct ActionEvent {
    /// Source node.
    pub node: NodeId,
    /// What happened.
    pub kind: ActionKind,
    /// New committed text for [`ActionKind::TextChanged`].
    pub text: Option<String>,
    /// Focused state for [`ActionKind::FocusChanged`].
    pub focused: Option<bool>,
}

/// Action callback: event data in, host notification out (channels/signals).
/// Must not block; must not touch the tree (it has no handle to it).
pub type ActionCallback = Box<dyn Fn(ActionEvent)>;

/// Targeted UI update enqueued by bindings/effects and drained before layout.
/// Commands apply in enqueue order; a command naming a removed node is
/// dropped (never panics, never resurrects).
#[derive(Debug)]
pub enum UiCommand {
    /// Replace text content (editor state preserved).
    SetText {
        /// Target node.
        node: NodeId,
        /// New content.
        text: String,
    },
    /// Replace style (layout-invalidating).
    SetStyle {
        /// Target node.
        node: NodeId,
        /// New style.
        style: LayoutStyle,
    },
    /// Scroll to an offset (clamped to content).
    SetScroll {
        /// Target scroll node.
        node: NodeId,
        /// Desired offset.
        offset: Point,
    },
    /// Move keyboard focus (`None` = blur).
    RequestFocus {
        /// Target node.
        node: Option<NodeId>,
    },
    /// Insert a descriptor subtree under `parent` at `index`. Boxed to
    /// keep queued commands small (descriptors own child subtrees).
    Insert {
        /// Parent node.
        parent: NodeId,
        /// Child position.
        index: usize,
        /// Subtree to mount.
        spec: Box<NodeSpec>,
    },
    /// Remove a node and its subtree.
    Remove {
        /// Target node.
        node: NodeId,
    },
    /// Reorder a child within its parent.
    Move {
        /// Parent node.
        parent: NodeId,
        /// Child to move.
        child: NodeId,
        /// New position.
        index: usize,
    },
}

/// Cloneable command queue shared with reactive effects and the host.
/// Effects capture a [`CommandSink`] (cheap clone) and push [`UiCommand`]s
/// from subscription callbacks; the tree drains them at the next layout
/// boundary. The `RefCell` guards only the queue, never the tree: `take()`
/// moves commands out under a brief borrow and runs no user code, so a
/// callback pushing during a drain lands in the fresh queue instead of
/// reentering anything.
#[derive(Debug, Default, Clone)]
pub struct CommandSink {
    queue: Rc<RefCell<Vec<UiCommand>>>,
}

impl CommandSink {
    /// Create an empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self {
            queue: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// Enqueue a command. Never blocks, never reenters.
    pub fn push(&self, cmd: UiCommand) {
        self.queue.borrow_mut().push(cmd);
    }

    /// Move all queued commands out, preserving order.
    #[must_use]
    pub fn take(&self) -> Vec<UiCommand> {
        std::mem::take(&mut *self.queue.borrow_mut())
    }

    /// Pending command count (frame-budget diagnostics).
    #[must_use]
    pub fn len(&self) -> usize {
        self.queue.borrow().len()
    }

    /// Whether no commands are pending.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.borrow().is_empty()
    }
}

/// Accessibility metadata for one node. Roles are assigned explicitly via
/// [`UiTree::set_semantics`]: the default is `Group`, which is correct for
/// plain containers and WRONG for activatable controls — a button left as
/// `Group` is a contract violation (setting a role alone never implements
/// behavior, but a wrong role actively misinforms assistive tech).
#[derive(Debug, Clone, PartialEq)]
pub struct NodeSemantics {
    /// Accessibility role.
    pub role: Role,
    /// Human-readable label (empty label on a `Text` node falls back to its
    /// content at emission).
    pub label: String,
    /// Visible but non-interactive (skipped by focus traversal, announced).
    pub disabled: bool,
    /// Selected state (lists, tabs).
    pub selected: bool,
    /// Value text (sliders, numeric fields, checkboxes).
    pub value_text: Option<String>,
    /// Total collection size and logical positions for virtualized content
    /// (lists report `set_size`/`position_in_set`; tables report row/column
    /// counts and indices). `None` for non-collection nodes.
    pub collection: Option<CollectionSemantics>,
}

impl Default for NodeSemantics {
    fn default() -> Self {
        Self {
            role: Role::Group,
            label: String::new(),
            disabled: false,
            selected: false,
            value_text: None,
            collection: None,
        }
    }
}

/// One retained node.
pub struct NodeRecord {
    /// Node kind + properties.
    pub kind: NodeKind,
    /// Typed style.
    pub style: LayoutStyle,
    /// Paint for non-box nodes, without allocating a wrapper node.
    pub paint: Paint,
    /// Ordered children (paint order = child order; last is topmost).
    pub children: Vec<NodeId>,
    /// Parent (`None` for the root).
    pub parent: Option<NodeId>,
    /// Layout-affecting change pending.
    pub dirty_layout: bool,
    /// Paint-only change pending.
    pub dirty_paint: bool,
    /// Content version at the last paint-affecting mutation of this node.
    /// Hosts use [`UiTree::changed_paint`] to list nodes newer than a version
    /// without scanning the whole tree themselves.
    pub paint_rev: u64,
    /// Whether the node can take keyboard focus.
    pub focusable: bool,
    /// Stable key for keyed reconciliation (`None` = positional).
    pub key: Option<KeyHash>,
    /// Accessibility metadata (explicit roles; see [`NodeSemantics`]).
    pub semantics: NodeSemantics,
}

impl fmt::Debug for NodeRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeRecord")
            .field("kind", &self.kind)
            .field("children", &self.children.len())
            .field("parent", &self.parent)
            .field("dirty_layout", &self.dirty_layout)
            .field("dirty_paint", &self.dirty_paint)
            .field("paint_rev", &self.paint_rev)
            .field("focusable", &self.focusable)
            .field("key", &self.key)
            .field("semantics", &self.semantics)
            .finish()
    }
}

/// Per-leaf Taffy context. Text leaves carry measurable content; every other
/// leaf measures from style/availability.
#[derive(Debug, Clone, Default)]
pub struct LeafCtx {
    /// Text measurement input (`None` for non-text leaves).
    pub text: Option<LeafText>,
}

/// Text measurement input owned by the UI tree.
#[derive(Debug, Clone)]
pub struct LeafText {
    /// Display text to measure.
    pub text: String,
    /// Font size in logical px.
    pub font_size: f32,
    /// Whether to wrap at the available width.
    pub multiline: bool,
}

/// Per-frame dirty statistics. Proves that a local update costs work
/// proportional to its dependency closure, not to total tree size:
/// paint-only updates skip Taffy entirely (`layout_skipped`), and hosts can
/// gate scene re-emission on [`FrameStats::content_version`].
#[derive(Debug, Clone, Default)]
pub struct FrameStats {
    /// Whether Taffy was skipped (viewport and layout version unchanged).
    pub layout_skipped: bool,
    /// Commands drained at the start of the last `compute_layout`.
    pub commands_drained: usize,
    /// Taffy leaf-measure calls during the last `compute_layout`.
    pub nodes_measured: usize,
    /// Rects committed during the last `compute_layout`.
    pub nodes_traversed: usize,
    /// Scene ops produced by the last [`UiTree::emit_scene`].
    pub scene_ops: usize,
    /// [`UiTree::content_version`] at the last emit.
    pub content_version: u64,
}

/// Retained tree plus layout engine. `aqua-ui` never touches GPU or windows:
///
/// - layout runs headless through Taffy;
/// - scene emission produces renderer-neutral [`DisplayItem`]s.
pub struct UiTree {
    nodes: Arena<NodeRecord>,
    root: Option<NodeId>,
    taffy: TaffyTree<LeafCtx>,
    /// UI node (raw id) to Taffy node.
    tmap: HashMap<RawId, taffy::NodeId>,
    /// Committed absolute rects, refreshed by [`UiTree::compute_layout`].
    rects: HashMap<RawId, Rect>,
    /// Scroll content extents per scroll node.
    content: HashMap<RawId, Size>,
    /// Currently focused node.
    focused: Option<NodeId>,
    /// Pending binding commands, drained before layout. Shared cloneably
    /// with reactive effects via [`UiTree::command_sink`].
    sink: CommandSink,
    /// Registered canvas painters by painter id.
    painters: HashMap<u64, CanvasPaintCallback>,
    /// Next canvas painter id.
    next_painter: u64,
    /// Action callbacks by (node, action kind).
    actions: HashMap<(RawId, ActionKind), ActionCallback>,
    /// Armed press per pointer contact: contact id → activatable node.
    /// Set on primary down, consumed on up/cancel. Cleared when the node is
    /// removed or the window loses focus, so a stale arm can never fire.
    press_captures: HashMap<u64, NodeId>,
    /// Bumped by every layout-invalidating mutation. `compute_layout` runs
    /// Taffy only when this differs from `committed_layout` or the viewport
    /// changed; genuine ancestor/sibling invalidation propagates through
    /// `mark_layout_dirty`, so a local paint change never re-runs layout.
    layout_version: u64,
    /// Layout version covered by the last committed rect set.
    committed_layout: u64,
    /// Bumped by every paint- or layout-affecting mutation. Hosts gate
    /// scene re-emission on this (no per-frame whole-tree walks).
    content_version: u64,
    /// Viewport covered by the last committed rect set.
    last_viewport: Option<Size>,
    /// Statistics for the last layout + emit (dirty-tracking evidence).
    last_stats: FrameStats,
}

impl Default for UiTree {
    fn default() -> Self {
        Self::new()
    }
}

impl UiTree {
    /// Create an empty tree.
    #[must_use]
    pub fn new() -> Self {
        Self {
            nodes: Arena::new(),
            root: None,
            taffy: TaffyTree::new(),
            tmap: HashMap::new(),
            rects: HashMap::new(),
            content: HashMap::new(),
            focused: None,
            sink: CommandSink::new(),
            painters: HashMap::new(),
            next_painter: 1,
            actions: HashMap::new(),
            press_captures: HashMap::new(),
            layout_version: 0,
            committed_layout: 0,
            content_version: 0,
            last_viewport: None,
            last_stats: FrameStats::default(),
        }
    }

    /// Layout version: bumped by every layout-invalidating mutation.
    #[must_use]
    pub fn layout_version(&self) -> u64 {
        self.layout_version
    }

    /// Content version: bumped by every paint- or layout-affecting mutation.
    /// Hosts skip scene re-emission while this is unchanged.
    #[must_use]
    pub fn content_version(&self) -> u64 {
        self.content_version
    }

    /// Statistics for the last layout + emit.
    #[must_use]
    pub fn last_stats(&self) -> FrameStats {
        self.last_stats.clone()
    }

    /// Nodes whose `paint_rev` is newer than `since` (changed paint, in no
    /// particular order). A linear arena scan — for host invalidation
    /// listing, not per-op hot paths (the render store elides identical
    /// items per node).
    pub fn changed_paint(&self, since: u64) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|(_, r)| r.paint_rev > since)
            .map(|(id, _)| NodeId::from_raw(id))
            .collect()
    }

    /// Bump both versions (layout-affecting mutation).
    fn bump_layout(&mut self) {
        self.layout_version += 1;
        self.content_version += 1;
    }

    /// Bump content only (paint-affecting mutation) and stamp the node.
    fn touch_paint(&mut self, id: NodeId) {
        self.content_version += 1;
        let rev = self.content_version;
        if let Some(rec) = self.nodes.get_mut(id.raw()) {
            rec.paint_rev = rev;
            rec.dirty_paint = true;
        }
    }

    /// Tree root (`None` before mount).
    #[must_use]
    pub fn root(&self) -> Option<NodeId> {
        self.root
    }

    /// Node record (live handles only).
    #[must_use]
    pub fn get(&self, id: NodeId) -> Option<&NodeRecord> {
        self.nodes.get(id.raw())
    }

    /// Exclusive node access; marks paint dirty. Callers that change layout
    /// inputs must additionally call [`UiTree::mark_layout_dirty`].
    /// Text content edits MUST go through [`UiTree::set_text`] or the
    /// `TextContent` editing methods (keeps the Taffy measure context and
    /// editor in sync), not this method.
    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut NodeRecord> {
        if !self.nodes.contains(id.raw()) {
            return None;
        }
        self.touch_paint(id);
        self.nodes.get_mut(id.raw())
    }

    /// Currently focused node.
    #[must_use]
    pub fn focused(&self) -> Option<NodeId> {
        self.focused.filter(|id| self.nodes.contains(id.raw()))
    }

    /// Move keyboard focus (validates liveness + focusability). Fires
    /// [`ActionKind::FocusChanged`] on both ends when the focus changes.
    pub fn set_focused(&mut self, id: Option<NodeId>) -> bool {
        match id {
            None => {
                let prev = self.focused;
                self.focused = None;
                if let Some(p) = prev {
                    self.fire_action(ActionEvent {
                        node: p,
                        kind: ActionKind::FocusChanged,
                        text: None,
                        focused: Some(false),
                    });
                }
                true
            }
            Some(n) => match self.nodes.get(n.raw()) {
                Some(rec) if rec.focusable && !rec.semantics.disabled => {
                    let prev = self.focused;
                    self.focused = Some(n);
                    if prev != Some(n) {
                        if let Some(p) = prev {
                            self.fire_action(ActionEvent {
                                node: p,
                                kind: ActionKind::FocusChanged,
                                text: None,
                                focused: Some(false),
                            });
                        }
                        self.fire_action(ActionEvent {
                            node: n,
                            kind: ActionKind::FocusChanged,
                            text: None,
                            focused: Some(true),
                        });
                    }
                    true
                }
                _ => false,
            },
        }
    }

    /// Assign accessibility metadata (role, label, disabled, ...). Bumps
    /// the content version so hosts re-emit the accessibility tree.
    /// Interactive nodes MUST get their real role: leaving `Group` on a
    /// button is a contract violation.
    pub fn set_semantics(&mut self, id: NodeId, semantics: NodeSemantics) -> bool {
        if !self.nodes.contains(id.raw()) {
            return false;
        }
        if let Some(rec) = self.nodes.get_mut(id.raw()) {
            rec.semantics = semantics;
        }
        self.touch_paint(id);
        true
    }

    /// Emit the accessibility tree in paint order: one [`SemanticNode`] per
    /// live node with children, committed bounds, role, disabled, focus,
    /// label, and value. Empty labels on `Text` nodes fall back to content.
    /// The platform adapter converts these to native APIs (e.g. accesskit).
    pub fn emit_semantics(&self) -> Vec<SemanticNode> {
        let mut out = Vec::new();
        for id in self.traverse() {
            let Some(rec) = self.nodes.get(id.raw()) else {
                continue;
            };
            let mut label = rec.semantics.label.clone();
            if label.is_empty() {
                if let NodeKind::Text(ref t) = rec.kind {
                    label = t.text().to_string();
                }
            }
            out.push(SemanticNode {
                node: id,
                role: rec.semantics.role,
                label,
                focused: self.focused == Some(id),
                disabled: rec.semantics.disabled,
                selected: rec.semantics.selected,
                value_text: rec.semantics.value_text.clone(),
                children: rec.children.clone(),
                bounds: self.rects.get(&id.raw()).copied(),
                collection: rec.semantics.collection,
            });
        }
        out
    }

    /// Set whether a node can take keyboard focus (component behaviors such
    /// as buttons opt in; plain containers stay unfocused). Unknown nodes
    /// fail closed.
    pub fn set_focusable(&mut self, id: NodeId, focusable: bool) -> bool {
        match self.nodes.get_mut(id.raw()) {
            Some(rec) => {
                rec.focusable = focusable;
                if !focusable && self.focused == Some(id) {
                    self.focused = rec.parent;
                }
                true
            }
            None => false,
        }
    }

    /// Register an action callback for `(node, kind)` (replaces any prior).
    /// Returned ids from mount/sync stay stable across keyed updates, so one
    /// registration survives reorder; removal drops callbacks with the node.
    pub fn set_action_callback(
        &mut self,
        node: NodeId,
        kind: ActionKind,
        cb: ActionCallback,
    ) -> bool {
        if !self.nodes.contains(node.raw()) {
            return false;
        }
        self.actions.insert((node.raw(), kind), cb);
        true
    }

    /// Fire an action: invoke the registered callback with event data only.
    /// Callbacks run during dispatch and cannot reenter the tree; hosts route
    /// follow-up mutations back through [`UiTree::push_command`].
    pub fn fire_action(&self, event: ActionEvent) {
        if let Some(cb) = self.actions.get(&(event.node.raw(), event.kind)) {
            cb(event);
        }
    }

    /// Whether `(node, kind)` has a registered callback.
    #[must_use]
    pub fn has_action(&self, node: NodeId, kind: ActionKind) -> bool {
        self.actions.contains_key(&(node.raw(), kind))
    }

    /// Whether the node or any ancestor is disabled. Disabled subtrees block
    /// activation, focus, and text input.
    #[must_use]
    pub fn is_disabled(&self, mut id: NodeId) -> bool {
        loop {
            match self.nodes.get(id.raw()) {
                None => return false,
                Some(rec) => {
                    if rec.semantics.disabled {
                        return true;
                    }
                    match rec.parent {
                        Some(p) => id = p,
                        None => return false,
                    }
                }
            }
        }
    }

    /// Arm a press capture: `contact` will activate `node` if released
    /// inside its bounds. Replaces any previous arm for the contact.
    pub fn arm_press(&mut self, contact: u64, node: NodeId) {
        self.press_captures.insert(contact, node);
    }

    /// Consume and return the armed node for `contact`, if any.
    pub fn take_press(&mut self, contact: u64) -> Option<NodeId> {
        self.press_captures.remove(&contact)
    }

    /// Clear all armed presses (window focus loss).
    pub fn clear_presses(&mut self) {
        self.press_captures.clear();
    }

    /// Committed absolute rect from the last [`UiTree::compute_layout`].
    #[must_use]
    pub fn rect(&self, id: NodeId) -> Option<Rect> {
        self.rects.get(&id.raw()).copied()
    }

    /// Scroll content size from the last layout.
    #[must_use]
    pub fn content_size(&self, id: NodeId) -> Option<Size> {
        self.content.get(&id.raw()).copied()
    }

    /// Number of live nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the tree is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// Enqueue a binding command; applied at the next layout boundary.
    /// Safe to call during event dispatch: commands queue, they never apply
    /// reentrantly.
    pub fn push_command(&mut self, cmd: UiCommand) {
        self.sink.push(cmd);
    }

    /// Cloneable queue handle for reactive effects and host bindings.
    /// Captured callbacks push [`UiCommand`]s here; the tree drains them via
    /// [`UiTree::drain_commands`] before layout. No tree reference is
    /// captured, so effects cannot reenter the tree during dispatch.
    #[must_use]
    pub fn command_sink(&self) -> CommandSink {
        self.sink.clone()
    }

    /// Pending command count (diagnostics: bindings must not grow this
    /// without bound between frames).
    #[must_use]
    pub fn pending_commands(&self) -> usize {
        self.sink.len()
    }

    /// Apply every queued command in order, returning the count applied.
    /// Commands naming removed nodes are dropped. Runs automatically at the
    /// start of `compute_layout`; call directly in tests to observe effects
    /// without laying out.
    pub fn drain_commands(&mut self) -> usize {
        let queue = self.sink.take();
        let n = queue.len();
        for cmd in queue {
            match cmd {
                UiCommand::SetText { node, text } => {
                    self.set_text(node, text);
                }
                UiCommand::SetStyle { node, style } => {
                    self.set_style(node, style);
                }
                UiCommand::SetScroll { node, offset } => {
                    self.scroll_to(node, offset);
                }
                UiCommand::RequestFocus { node } => {
                    self.set_focused(node);
                }
                UiCommand::Insert {
                    parent,
                    index,
                    spec,
                } => {
                    self.insert_spec(parent, index, *spec);
                }
                UiCommand::Remove { node } => {
                    self.remove(node);
                }
                UiCommand::Move {
                    parent,
                    child,
                    index,
                } => {
                    self.move_child(parent, child, index);
                }
            }
        }
        n
    }

    /// Mount a descriptor tree as the root, replacing any previous root.
    /// Structural replacement only; per-signal updates use commands and
    /// [`UiTree::sync_children`], never remounts.
    pub fn mount_spec(&mut self, spec: NodeSpec) -> NodeId {
        if let Some(old) = self.root {
            self.remove(old);
        }
        let id = self.build_spec(None, spec);
        self.root = Some(id);
        self.bump_layout();
        id
    }

    /// Keyed reconciliation of a parent's children against new descriptors:
    /// nodes with equal keys are reused in place (editing state, focus, and
    /// per-row effect scopes keyed by [`NodeId`] survive), new keys insert,
    /// missing keys remove with disposal, and order follows the descriptors.
    /// Unkeyed (`key == None`) children reconcile positionally. A node whose
    /// kind discriminant changes is replaced, not morphed. Text nodes follow
    /// uncontrolled-editor semantics: a reused editable node keeps its live
    /// model, caret, selection, and undo history (descriptor text is the
    /// mount-time default; explicit changes flow through `SetText`
    /// bindings), while static labels take the descriptor text.
    pub fn sync_children(&mut self, parent: NodeId, children: Vec<NodeSpec>) -> Vec<NodeId> {
        let old: Vec<NodeId> = self
            .get(parent)
            .map(|r| r.children.clone())
            .unwrap_or_default();
        // Index old children by key; unkeyed fall back to positional reuse.
        let mut by_key: HashMap<KeyHash, NodeId> = HashMap::with_capacity(old.len());
        let mut unkeyed: Vec<NodeId> = Vec::new();
        for id in &old {
            match self.get(*id).and_then(|r| r.key) {
                Some(k) => {
                    by_key.insert(k, *id);
                }
                None => unkeyed.push(*id),
            }
        }
        // O(1) old-position lookup: the previous code scanned `old` per reuse
        // (`O(n^2)` for large keyed lists such as non-virtualized collections).
        let old_index: HashMap<RawId, usize> = old
            .iter()
            .enumerate()
            .map(|(i, id)| (id.raw(), i))
            .collect();
        let mut reused = vec![false; old.len()];
        let mut out: Vec<Option<NodeId>> = Vec::with_capacity(children.len());
        let mut unkeyed_cursor = 0;
        let mut pending: Vec<NodeSpec> = Vec::new();
        for spec in children {
            let reuse = match spec.key {
                Some(k) => by_key.remove(&k),
                None => {
                    // Positional: next surviving unkeyed old child.
                    while unkeyed_cursor < unkeyed.len()
                        && !self.nodes.contains(unkeyed[unkeyed_cursor].raw())
                    {
                        unkeyed_cursor += 1;
                    }
                    if unkeyed_cursor < unkeyed.len() {
                        let id = unkeyed[unkeyed_cursor];
                        unkeyed_cursor += 1;
                        Some(id)
                    } else {
                        None
                    }
                }
            };
            match reuse {
                Some(id) if self.kind_matches(id, &spec.kind) => {
                    if let Some(&i) = old_index.get(&id.raw()) {
                        reused[i] = true;
                    }
                    self.update_spec(id, spec);
                    out.push(Some(id));
                }
                _ => {
                    // Fresh or kind-changed: drop the stale node now so its
                    // slot (and any per-row scope keyed to it) disposes
                    // before the replacement mounts.
                    if let Some(stale) = reuse {
                        self.remove(stale);
                        if let Some(&i) = old_index.get(&stale.raw()) {
                            reused[i] = true; // accounted for; do not drop again
                        }
                    }
                    pending.push(spec);
                    out.push(None);
                }
            }
        }
        // Remove leftovers (missing keys / consumed positional slots).
        for (id, used) in old.iter().zip(reused.iter()) {
            if !used {
                self.remove(*id);
            }
        }
        // Build fresh nodes in order via a draining iterator (the old
        // `pending.remove(0)` was `O(n^2)` for large inserts).
        let mut fresh = pending.into_iter();
        let mut built: Vec<NodeId> = Vec::with_capacity(out.len());
        for slot in out {
            match slot {
                Some(id) => built.push(id),
                None => {
                    let spec = fresh.next().expect("pending spec per placeholder");
                    built.push(self.build_detached(spec));
                }
            }
        }
        // Reorder: rebuild the child list and taffy order in one pass.
        if let Some(&pt) = self.tmap.get(&parent.raw()) {
            let attached: Vec<taffy::NodeId> = self
                .get(parent)
                .map(|r| r.children.clone())
                .unwrap_or_default()
                .iter()
                .filter_map(|id| self.tmap.get(&id.raw()).copied())
                .collect();
            for ct in attached {
                self.detach_child(pt, ct);
            }
            for id in &built {
                if let Some(&ct) = self.tmap.get(&id.raw()) {
                    self.taffy.add_child(pt, ct).expect("taffy reorder append");
                }
            }
        }
        for &id in &built {
            if let Some(rec) = self.nodes.get_mut(id.raw()) {
                rec.parent = Some(parent);
            }
        }
        if let Some(rec) = self.nodes.get_mut(parent.raw()) {
            rec.children = built.clone();
            rec.dirty_layout = true;
        }
        self.mark_layout_dirty(parent);
        built
    }

    /// Whether a live node's kind discriminant matches `kind`.
    fn kind_matches(&self, id: NodeId, kind: &NodeKind) -> bool {
        matches!(
            (self.get(id).map(|r| &r.kind), kind),
            (Some(NodeKind::Box(_)), NodeKind::Box(_))
                | (Some(NodeKind::Text(_)), NodeKind::Text(_))
                | (Some(NodeKind::Media(_)), NodeKind::Media(_))
                | (Some(NodeKind::Scroll(_)), NodeKind::Scroll(_))
                | (Some(NodeKind::Canvas(_)), NodeKind::Canvas(_))
                | (Some(NodeKind::Viewport3D(_)), NodeKind::Viewport3D(_))
        )
    }

    /// Update a reused node's props in place (identity preserved).
    fn update_spec(&mut self, id: NodeId, spec: NodeSpec) {
        // Window-managed collection containers (container/header/body): the
        // driver owns children and scroll offsets. Style/paint still flow
        // through; the stateless spec offset (always zero) must never
        // clobber the live scroll, and the spec's first-paint children must
        // never replace the realized window.
        let managed = spec.collection_managed;
        // Style first (marks layout dirty through the ancestor chain).
        self.set_style(id, spec.style);
        self.set_paint(id, spec.paint);
        match spec.kind {
            NodeKind::Text(t) => {
                let live_editable = self.get(id).is_some_and(
                    |r| matches!(r.kind, NodeKind::Text(ref cur) if cur.is_editable()),
                );
                if live_editable && t.is_editable() {
                    // Keyed uncontrolled editor: preserve the live model,
                    // caret, selection, and undo history. Descriptor text is
                    // the mount-time default, not a controlled value —
                    // explicit changes flow through SetText bindings. Apply
                    // cosmetic props only.
                    if let Some(rec) = self.nodes.get_mut(id.raw()) {
                        if let NodeKind::Text(ref mut cur) = rec.kind {
                            cur.font_size = t.font_size;
                            cur.color = t.color;
                            cur.selectable = t.selectable;
                            cur.wrap = t.wrap;
                            cur.password = t.password;
                            cur.placeholder = t.placeholder;
                        }
                        rec.dirty_paint = true;
                    }
                    self.sync_text_leaf(id);
                    self.touch_paint(id);
                } else if live_editable != t.is_editable() {
                    // Static<->Editable mode change: swap backing (fresh state).
                    if let Some(rec) = self.nodes.get_mut(id.raw()) {
                        rec.kind = NodeKind::Text(t);
                        rec.dirty_paint = true;
                    }
                    self.sync_text_leaf(id);
                    self.touch_paint(id);
                } else {
                    self.set_text(id, t.text());
                    // A static label can be restyled without changing its
                    // string. Reconciliation must retain typography too.
                    if let Some(rec) = self.nodes.get_mut(id.raw()) {
                        if let NodeKind::Text(ref mut cur) = rec.kind {
                            cur.font_size = t.font_size;
                            cur.color = t.color;
                            cur.selectable = t.selectable;
                            cur.wrap = t.wrap;
                            cur.placeholder = t.placeholder;
                        }
                    }
                    self.sync_text_leaf(id);
                    self.touch_paint(id);
                }
            }
            NodeKind::Box(b) => {
                if let Some(rec) = self.nodes.get_mut(id.raw()) {
                    if let NodeKind::Box(ref mut cur) = rec.kind {
                        cur.paint = b.paint;
                    }
                    rec.dirty_paint = true;
                }
                self.touch_paint(id);
            }
            NodeKind::Media(m) => {
                if let Some(rec) = self.nodes.get_mut(id.raw()) {
                    rec.kind = NodeKind::Media(m);
                    rec.dirty_paint = true;
                }
                self.touch_paint(id);
            }
            NodeKind::Scroll(s) => {
                if !managed {
                    if let Some(rec) = self.nodes.get_mut(id.raw()) {
                        if let NodeKind::Scroll(ref mut cur) = rec.kind {
                            cur.offset = s.offset;
                        }
                    }
                }
                self.touch_paint(id);
            }
            NodeKind::Canvas(c) => {
                if let Some(rec) = self.nodes.get_mut(id.raw()) {
                    rec.kind = NodeKind::Canvas(c);
                    rec.dirty_paint = true;
                }
                self.touch_paint(id);
            }
            NodeKind::Viewport3D(v) => {
                if let Some(rec) = self.nodes.get_mut(id.raw()) {
                    rec.kind = NodeKind::Viewport3D(v);
                    rec.dirty_paint = true;
                }
                self.touch_paint(id);
            }
        }
        if let Some(rec) = self.nodes.get_mut(id.raw()) {
            rec.key = spec.key;
        }
        if managed {
            // Window-managed: children and scroll stay with the driver.
            return;
        }
        // Recurse into children (keyed at every level).
        self.sync_children(id, spec.children);
    }

    /// Build a descriptor subtree detached (no parent yet); caller attaches.
    fn build_detached(&mut self, spec: NodeSpec) -> NodeId {
        let key = spec.key;
        let style = spec.style;
        let paint = spec.paint;
        let kind = spec.kind;
        let children = spec.children;
        let focusable = matches!(kind, NodeKind::Text(ref t) if t.is_editable());
        let ctx = match kind {
            NodeKind::Text(ref t) => LeafCtx {
                text: Some(LeafText {
                    text: t.display_text(),
                    font_size: t.font_size,
                    multiline: t.multiline(),
                }),
            },
            _ => LeafCtx { text: None },
        };
        let tnode = self
            .taffy
            .new_leaf_with_context(crate::style::to_taffy(&style), ctx)
            .expect("taffy leaf insertion is infallible");
        // Canvas painters referenced by id stay registered across rebuilds;
        // only node records are fresh.
        let id = NodeId::from_raw(self.nodes.insert(NodeRecord {
            kind,
            style,
            paint,
            children: Vec::new(),
            parent: None,
            dirty_layout: true,
            dirty_paint: true,
            paint_rev: self.content_version + 1,
            focusable,
            key,
            semantics: NodeSemantics::default(),
        }));
        self.tmap.insert(id.raw(), tnode);
        for child in children {
            let cid = self.build_detached(child);
            self.attach(id, cid);
        }
        id
    }

    /// Build a descriptor subtree, attaching the root under `parent`.
    fn build_spec(&mut self, parent: Option<NodeId>, spec: NodeSpec) -> NodeId {
        let id = self.build_detached(spec);
        if let Some(p) = parent {
            self.attach(p, id);
        }
        id
    }

    /// Detach a taffy child only when it is actually attached to `parent`.
    /// Taffy's `remove_child` fails on a child/parent link that does not
    /// exist — e.g. a fresh detached node being spliced into an order, or a
    /// node whose taffy parent already changed. Never call `remove_child`
    /// without this guard.
    fn detach_child(&mut self, parent: taffy::NodeId, child: taffy::NodeId) {
        if self.taffy.parent(child) == Some(parent) {
            let _ = self.taffy.remove_child(parent, child);
        }
    }

    /// Insert a descriptor subtree under `parent` at `index`. The fresh
    /// subtree is detached by construction, so reorder never removes a node
    /// that was never attached (see [`UiTree::detach_child`]).
    pub fn insert_spec(&mut self, parent: NodeId, index: usize, spec: NodeSpec) -> Option<NodeId> {
        self.get(parent)?;
        let id = self.build_detached(spec);
        let pt = match self.tmap.get(&parent.raw()) {
            Some(&p) => p,
            None => return Some(id),
        };
        // Splice into retained order and taffy order at the same position.
        let mut kids = self
            .get(parent)
            .map(|r| r.children.clone())
            .unwrap_or_default();
        let at = index.min(kids.len());
        kids.insert(at, id);
        // Detach only attached children; the fresh node has no taffy parent.
        let attached: Vec<taffy::NodeId> = kids
            .iter()
            .filter_map(|k| self.tmap.get(&k.raw()).copied())
            .collect();
        for t in attached {
            self.detach_child(pt, t);
        }
        for k in &kids {
            if let Some(&t) = self.tmap.get(&k.raw()) {
                self.taffy.add_child(pt, t).expect("taffy insert");
            }
        }
        if let Some(rec) = self.nodes.get_mut(parent.raw()) {
            rec.children = kids;
            rec.dirty_layout = true;
        }
        if let Some(rec) = self.nodes.get_mut(id.raw()) {
            rec.parent = Some(parent);
        }
        self.mark_layout_dirty(parent);
        Some(id)
    }

    /// Reorder `child` within `parent` to `index`.
    pub fn move_child(&mut self, parent: NodeId, child: NodeId, index: usize) -> bool {
        let mut kids = match self.get(parent) {
            Some(r) => r.children.clone(),
            None => return false,
        };
        let Some(pos) = kids.iter().position(|c| *c == child) else {
            return false;
        };
        kids.remove(pos);
        kids.insert(index.min(kids.len()), child);
        if let Some(&pt) = self.tmap.get(&parent.raw()) {
            let attached: Vec<taffy::NodeId> = self
                .get(parent)
                .map(|r| r.children.clone())
                .unwrap_or_default()
                .iter()
                .filter_map(|k| self.tmap.get(&k.raw()).copied())
                .collect();
            for t in attached {
                self.detach_child(pt, t);
            }
            for k in &kids {
                if let Some(&t) = self.tmap.get(&k.raw()) {
                    self.taffy.add_child(pt, t).expect("taffy move");
                }
            }
        }
        if let Some(rec) = self.nodes.get_mut(parent.raw()) {
            rec.children = kids;
            rec.dirty_layout = true;
        }
        self.mark_layout_dirty(parent);
        true
    }

    /// Insert a node, optionally under `parent` (appends to paint order).
    /// Prefer [`UiTree::mount_spec`]/[`UiTree::sync_children`] for
    /// descriptor-driven use.
    pub fn insert(&mut self, parent: Option<NodeId>, kind: NodeKind, style: LayoutStyle) -> NodeId {
        let id = self.build_detached(NodeSpec {
            kind,
            style,
            paint: Paint::default(),
            key: None,
            collection_managed: false,
            children: Vec::new(),
        });
        if let Some(p) = parent {
            self.attach(p, id);
        } else if self.root.is_none() {
            self.root = Some(id);
        }
        id
    }

    /// Attach an existing root-less node under `parent`.
    pub fn attach(&mut self, parent: NodeId, child: NodeId) {
        let (pt, ct) = match (self.tmap.get(&parent.raw()), self.tmap.get(&child.raw())) {
            (Some(&p), Some(&c)) => (p, c),
            _ => return,
        };
        self.taffy.add_child(pt, ct).expect("taffy attach");
        if let Some(rec) = self.nodes.get_mut(parent.raw()) {
            rec.children.push(child);
            rec.dirty_layout = true;
        }
        if let Some(rec) = self.nodes.get_mut(child.raw()) {
            rec.parent = Some(parent);
            rec.dirty_layout = true;
        }
        self.mark_layout_dirty(parent);
    }

    /// Remove a node and its subtree. Focus falls back to the parent.
    /// Canvas painters and action callbacks referenced only by removed nodes
    /// are dropped.
    pub fn remove(&mut self, id: NodeId) -> bool {
        let Some(rec) = self.nodes.get(id.raw()) else {
            return false;
        };
        let focus_fallback = rec.parent;
        if self.focused == Some(id) {
            self.focused = focus_fallback;
        }
        if let Some(p) = rec.parent {
            if let (Some(&pt), Some(&ct)) = (self.tmap.get(&p.raw()), self.tmap.get(&id.raw())) {
                self.detach_child(pt, ct);
            }
            if let Some(prec) = self.nodes.get_mut(p.raw()) {
                prec.children.retain(|c| *c != id);
            }
            // Ancestor geometry may depend on this child: propagate.
            self.mark_layout_dirty(p);
        } else if self.root == Some(id) {
            self.root = None;
            self.bump_layout();
        }
        // Drop the taffy subtree and all UI records depth-first.
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            // Focus inside a removed row must never retain a dead descendant.
            if self.focused == Some(n) {
                self.focused = focus_fallback;
            }
            if let Some(rec) = self.nodes.get(n.raw()) {
                stack.extend(rec.children.iter().copied());
            }
            if let Some(&t) = self.tmap.get(&n.raw()) {
                let _ = self.taffy.remove(t);
                self.tmap.remove(&n.raw());
            }
            self.rects.remove(&n.raw());
            self.content.remove(&n.raw());
            self.actions.retain(|(raw, _), _| *raw != n.raw());
            let _ = self.nodes.remove(n.raw());
        }
        // A removed node can never fire: drop arms targeting dead nodes.
        self.press_captures
            .retain(|_, n| self.nodes.contains(n.raw()));
        self.gc_painters();
        true
    }

    /// Drop painters no live canvas node references.
    fn gc_painters(&mut self) {
        if self.painters.is_empty() {
            return;
        }
        let mut used = std::collections::HashSet::new();
        for (_, rec) in self.nodes.iter() {
            if let NodeKind::Canvas(c) = &rec.kind {
                if let Some(p) = c.painter {
                    used.insert(p);
                }
            }
        }
        self.painters.retain(|k, _| used.contains(k));
    }

    /// Effective paint for any node. Boxes preserve the legacy paint storage.
    #[must_use]
    pub fn paint(&self, id: NodeId) -> Option<Paint> {
        self.get(id).map(|rec| match &rec.kind {
            NodeKind::Box(b) => b.paint,
            _ => rec.paint,
        })
    }

    /// Update appearance without invalidating layout or replacing node identity.
    /// Returns false for a dead node or an unchanged paint value.
    pub fn set_paint(&mut self, id: NodeId, paint: Paint) -> bool {
        let Some(rec) = self.nodes.get_mut(id.raw()) else {
            return false;
        };
        let target = match &mut rec.kind {
            NodeKind::Box(b) => &mut b.paint,
            _ => &mut rec.paint,
        };
        if *target == paint {
            return false;
        }
        *target = paint;
        self.touch_paint(id);
        true
    }

    /// Register (or replace) the painter for a canvas node. Returns the
    /// painter id. The callback runs at emission with the committed bounds.
    pub fn set_canvas_painter(&mut self, id: NodeId, paint: CanvasPaintCallback) -> Option<u64> {
        let rec = self.nodes.get(id.raw())?;
        let NodeKind::Canvas(c) = rec.kind else {
            return None;
        };
        let pid = c.painter.unwrap_or_else(|| {
            let p = self.next_painter;
            self.next_painter += 1;
            p
        });
        self.painters.insert(pid, paint);
        if let Some(rec) = self.nodes.get_mut(id.raw()) {
            if let NodeKind::Canvas(ref mut c) = rec.kind {
                c.painter = Some(pid);
            }
            rec.dirty_paint = true;
        }
        self.touch_paint(id);
        Some(pid)
    }

    /// Mark a node (and its ancestors) layout-dirty. Bumps the layout and
    /// content versions, so the next `compute_layout` re-runs Taffy.
    pub fn mark_layout_dirty(&mut self, mut id: NodeId) {
        self.bump_layout();
        loop {
            let Some(rec) = self.nodes.get_mut(id.raw()) else {
                return;
            };
            rec.dirty_layout = true;
            if let Some(&t) = self.tmap.get(&id.raw()) {
                let _ = self.taffy.mark_dirty(t);
            }
            match rec.parent {
                Some(p) => id = p,
                None => return,
            }
        }
    }

    /// Replace a node's style, invalidating layout.
    pub fn set_style(&mut self, id: NodeId, style: LayoutStyle) -> bool {
        let Some(&t) = self.tmap.get(&id.raw()) else {
            return false;
        };
        self.taffy
            .set_style(t, crate::style::to_taffy(&style))
            .expect("taffy set_style");
        if let Some(rec) = self.nodes.get_mut(id.raw()) {
            rec.style = style;
        }
        self.mark_layout_dirty(id);
        true
    }

    /// Replace text content. Editor state (caret, selection, undo) is
    /// preserved and clamped; the Taffy measure context re-syncs from display
    /// text; layout invalidates because metrics may change.
    pub fn set_text(&mut self, id: NodeId, text: impl Into<String>) -> bool {
        let text = text.into();
        let Some(&t) = self.tmap.get(&id.raw()) else {
            return false;
        };
        let should_update = match self.nodes.get(id.raw()) {
            Some(rec) => match &rec.kind {
                NodeKind::Text(tc) => tc.text() != text,
                _ => return false,
            },
            None => return false,
        };
        if !should_update {
            return true;
        }
        let (display, font_size, multiline) = {
            let Some(rec) = self.nodes.get_mut(id.raw()) else {
                return false;
            };
            let NodeKind::Text(ref mut tc) = rec.kind else {
                return false;
            };
            tc.set_text(&text);
            rec.dirty_paint = true;
            (tc.display_text(), tc.font_size, tc.multiline())
        };
        if let Some(ctx) = self.taffy.get_node_context_mut(t) {
            ctx.text = Some(LeafText {
                text: display,
                font_size,
                multiline,
            });
        }
        self.mark_layout_dirty(id);
        self.touch_paint(id);
        true
    }

    /// Run a text edit closure against a text node, then re-sync the Taffy
    /// measure context and invalidate layout. Returns `None` for unknown or
    /// non-text nodes; otherwise the closure's return value. This is the
    /// canonical low-level mutation path for bindings: a reactive effect
    /// subscribing with a target [`NodeId`] calls this (or enqueues
    /// [`UiCommand::SetText`]) instead of rebuilding any subtree.
    pub fn edit_text<R>(&mut self, id: NodeId, f: impl FnOnce(&mut TextContent) -> R) -> Option<R> {
        let out = {
            let rec = self.nodes.get_mut(id.raw())?;
            let NodeKind::Text(ref mut tc) = rec.kind else {
                return None;
            };
            let out = f(tc);
            rec.dirty_paint = true;
            out
        };
        self.sync_text_leaf(id);
        self.touch_paint(id);
        Some(out)
    }

    /// Re-sync a text node's measure context from its display text and mark
    /// layout dirty. No-op for unknown/non-text nodes.
    pub fn sync_text_leaf(&mut self, id: NodeId) -> bool {
        let Some(&t) = self.tmap.get(&id.raw()) else {
            return false;
        };
        let Some(rec) = self.nodes.get(id.raw()) else {
            return false;
        };
        let NodeKind::Text(ref tc) = rec.kind else {
            return false;
        };
        if let Some(ctx) = self.taffy.get_node_context_mut(t) {
            ctx.text = Some(LeafText {
                text: tc.display_text(),
                font_size: tc.font_size,
                multiline: tc.multiline(),
            });
        }
        self.mark_layout_dirty(id);
        true
    }

    /// Scroll clamping target: content size if measured, else the viewport
    /// (offset forced to zero).
    fn scroll_max(&self, id: NodeId) -> Point {
        let rect = self.rect(id).map(|r| r.size()).unwrap_or(Size::ZERO);
        let content = self.content_size(id).unwrap_or(Size::ZERO);
        Point::new((content.w - rect.w).max(0.0), (content.h - rect.h).max(0.0))
    }

    /// Scroll offset of a scroll node (`None` for unknown/non-scroll nodes).
    /// The collection driver reads this as the single source of truth for
    /// the visible window — virtualization follows actual scroll offsets,
    /// never shadow state.
    #[must_use]
    pub fn scroll_offset(&self, id: NodeId) -> Option<Point> {
        match self.nodes.get(id.raw()) {
            Some(rec) => match rec.kind {
                NodeKind::Scroll(ref s) => Some(s.offset),
                _ => None,
            },
            None => None,
        }
    }

    /// Scroll to an offset, clamped to content. Unknown/non-scroll nodes fail.
    pub fn scroll_to(&mut self, id: NodeId, offset: Point) -> bool {
        let max = self.scroll_max(id);
        let clamped = Point::new(offset.x.clamp(0.0, max.x), offset.y.clamp(0.0, max.y));
        let applied = match self.nodes.get_mut(id.raw()) {
            Some(rec) => match rec.kind {
                NodeKind::Scroll(ref mut s) => {
                    s.offset = clamped;
                    rec.dirty_paint = true;
                    true
                }
                _ => false,
            },
            None => false,
        };
        if applied {
            self.touch_paint(id);
        }
        applied
    }

    /// Compute layout for `viewport` logical size, draining queued binding
    /// commands first. Skips Taffy entirely when the viewport and layout
    /// version are unchanged (paint-only updates cost no layout work).
    /// `measurer` supplies text metrics; production uses
    /// [`CosmicMeasurer`](crate::layout::CosmicMeasurer) over the shared
    /// fonts, tests use [`NaiveMeasurer`].
    pub fn compute_layout(
        &mut self,
        viewport: Size,
        measurer: &dyn TextMeasurer,
    ) -> Result<(), String> {
        let drained = self.sink.len();
        self.drain_commands();
        if self.last_viewport == Some(viewport)
            && self.layout_version == self.committed_layout
            && self.root.is_some()
        {
            // Paint-only (or no-op) frame: committed rects stay valid.
            self.last_stats = FrameStats {
                layout_skipped: true,
                commands_drained: drained,
                nodes_measured: 0,
                nodes_traversed: 0,
                scene_ops: self.last_stats.scene_ops,
                content_version: self.content_version,
            };
            return Ok(());
        }
        let Some(root) = self.root else {
            return Ok(());
        };
        let Some(&troot) = self.tmap.get(&root.raw()) else {
            return Err("root has no taffy node".into());
        };
        let drained = self.sink.len();
        self.drain_commands();
        let measured = std::cell::Cell::new(0usize);
        self.taffy
            .compute_layout_with_measure(
                troot,
                taffy::geometry::Size {
                    width: AvailableSpace::Definite(viewport.w),
                    height: AvailableSpace::Definite(viewport.h),
                },
                |input: LayoutInput, _node, ctx, _style| {
                    measured.set(measured.get() + 1);
                    measure_leaf(input, ctx, measurer)
                },
            )
            .map_err(|e| format!("taffy layout failed: {e:?}"))?;
        let nodes_measured = measured.get();
        // Commit absolute rects + scroll content extents.
        self.rects.clear();
        self.content.clear();
        let mut stack = vec![(root, 0.0f32, 0.0f32)];
        while let Some((id, ox, oy)) = stack.pop() {
            let Some(&t) = self.tmap.get(&id.raw()) else {
                continue;
            };
            let Ok(layout) = self.taffy.layout(t) else {
                continue;
            };
            let x = ox + layout.location.x;
            let y = oy + layout.location.y;
            let rect = Rect::from_origin_size(
                Point::new(x, y),
                Size::new(layout.size.width, layout.size.height),
            );
            self.rects.insert(id.raw(), rect);
            let is_scroll = matches!(
                self.nodes.get(id.raw()).map(|r| &r.kind),
                Some(NodeKind::Scroll(_))
            );
            if is_scroll {
                // Content extent = union of laid-out child boxes, so scroll
                // clamping reflects real content, not the viewport.
                let mut extent = Size::ZERO;
                for c in self
                    .nodes
                    .get(id.raw())
                    .map(|r| r.children.clone())
                    .unwrap_or_default()
                {
                    if let Some(&ct) = self.tmap.get(&c.raw()) {
                        if let Ok(cl) = self.taffy.layout(ct) {
                            extent.w = extent.w.max(cl.location.x + cl.size.width);
                            extent.h = extent.h.max(cl.location.y + cl.size.height);
                        }
                    }
                }
                self.content.insert(id.raw(), extent);
            }
            if let Some(rec) = self.nodes.get(id.raw()) {
                for &c in rec.children.iter().rev() {
                    stack.push((c, x, y));
                }
                if let Some(rec) = self.nodes.get_mut(id.raw()) {
                    rec.dirty_layout = false;
                    rec.dirty_paint = false;
                }
            }
        }
        self.committed_layout = self.layout_version;
        self.last_viewport = Some(viewport);
        self.last_stats = FrameStats {
            layout_skipped: false,
            commands_drained: drained,
            nodes_measured,
            nodes_traversed: self.rects.len(),
            scene_ops: self.last_stats.scene_ops,
            content_version: self.content_version,
        };
        Ok(())
    }

    /// Emit painter-ordered scene ops for the committed layout. Records
    /// op counts into [`UiTree::last_stats`]; hosts compare
    /// [`UiTree::content_version`] across frames and skip re-emission (and
    /// the renderer's no-op-write elision skips unchanged nodes) instead of
    /// rebuilding scene state per frame.
    pub fn emit_scene(&mut self) -> Vec<(NodeId, DisplayItem)> {
        let mut out = Vec::new();
        if let Some(root) = self.root {
            self.emit_node(root, Point::ZERO, None, &mut out);
        }
        self.last_stats.scene_ops = out.len();
        self.last_stats.content_version = self.content_version;
        out
    }

    fn emit_node(
        &self,
        id: NodeId,
        offset: Point,
        clip: Option<Rect>,
        out: &mut Vec<(NodeId, DisplayItem)>,
    ) {
        let Some(rec) = self.nodes.get(id.raw()) else {
            return;
        };
        let Some(mut rect) = self.rects.get(&id.raw()).copied() else {
            return;
        };
        rect = rect.translate(offset.x, offset.y);
        // Clipped out entirely: skip the subtree (occlusion, not disposal).
        if let Some(c) = clip {
            if c.intersect(rect).is_empty() {
                return;
            }
        }
        let paint = match &rec.kind {
            NodeKind::Box(b) => b.paint,
            _ => rec.paint,
        };
        // Opacity encloses the node's surface AND all of its content.
        if let Some(opacity) = paint.opacity {
            out.push((id, DisplayItem::PushOpacity(opacity)));
        }
        if paint.fill.is_some() || paint.border.is_some() || paint.shadow.is_some() {
            out.push((
                id,
                DisplayItem::Rect {
                    bounds: rect,
                    radius: paint.corners.map_or(paint.radius, |r| r.top_left),
                    fill: paint.fill.unwrap_or(Oklch::TRANSPARENT),
                    border_width: paint.border.map_or(0.0, |x| x.width),
                    border_color: paint.border.map_or(Oklch::TRANSPARENT, |x| x.color),
                    shadow: paint.shadow.map(|s| (s.dx, s.dy, s.blur, s.color)),
                },
            ));
        }
        let clips = paint.clip || matches!(&rec.kind, NodeKind::Scroll(_));
        let child_clip = if clips {
            Some(clip.map_or(rect, |c| c.intersect(rect)))
        } else {
            clip
        };
        if clips {
            out.push((
                id,
                DisplayItem::PushClip {
                    bounds: rect,
                    radius: paint.corners.map_or(paint.radius, |r| r.top_left),
                },
            ));
        }
        match &rec.kind {
            NodeKind::Box(_) | NodeKind::Scroll(_) => {}
            NodeKind::Text(t) => {
                out.push((
                    id,
                    DisplayItem::Text {
                        at: [
                            rect.x + rec.style.padding.left,
                            rect.y + rec.style.padding.top,
                        ],
                        run: TextRun {
                            id: self.run_id_for(id),
                            revision: t.revision(),
                        },
                        color: t.color,
                        opacity: 1.0,
                    },
                ));
            }
            NodeKind::Media(m) => {
                if let Some(image) = m.image {
                    out.push((
                        id,
                        DisplayItem::External {
                            bounds: rect,
                            layer: media_layer(image),
                            opacity: m.opacity,
                            rounded_clip: 0.0,
                        },
                    ));
                }
            }
            NodeKind::Canvas(c) => {
                if let Some(paint) = c.painter.and_then(|pid| self.painters.get(&pid)) {
                    out.extend(paint(rect).into_iter().map(|op| (id, op)));
                }
            }
            NodeKind::Viewport3D(v) => {
                if let Some(layer) = v.layer {
                    out.push((
                        id,
                        DisplayItem::External {
                            bounds: rect,
                            layer,
                            opacity: 1.0,
                            rounded_clip: 0.0,
                        },
                    ));
                }
            }
        }
        let child_offset = match &rec.kind {
            NodeKind::Scroll(s) => Point::new(offset.x - s.offset.x, offset.y - s.offset.y),
            _ => offset,
        };
        for &child in &rec.children {
            self.emit_node(child, child_offset, child_clip, out);
        }
        if clips {
            out.push((id, DisplayItem::PopClip));
        }
        if paint.opacity.is_some() {
            out.push((id, DisplayItem::PopOpacity));
        }
    }

    fn run_id_for(&self, id: NodeId) -> u64 {
        // Stable per node: index + generation folded into one u64.
        // NOTE: interim scheme until the text RunRegistry mints runs; the
        // revision contract (bump on every content change) already holds.
        ((id.raw().index as u64) << 32) | id.raw().generation as u64
    }

    /// Depth-first traversal in paint order (for focus order, debugging).
    pub fn traverse(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = self.root.map_or(Vec::new(), |r| vec![r]);
        while let Some(id) = stack.pop() {
            out.push(id);
            if let Some(rec) = self.nodes.get(id.raw()) {
                stack.extend(rec.children.iter().rev().copied());
            }
        }
        out
    }
}

/// Leaf measurement: text leaves ask the provider; anything else resolves
/// from known dimensions (style) or collapses to zero (content-less leaf).
fn measure_leaf(
    input: LayoutInput,
    ctx: Option<&mut LeafCtx>,
    measurer: &dyn TextMeasurer,
) -> LayoutOutput {
    let mut size = taffy::geometry::Size {
        width: input.known_dimensions.width.unwrap_or(0.0),
        height: input.known_dimensions.height.unwrap_or(0.0),
    };
    if let Some(LeafCtx {
        text:
            Some(LeafText {
                text,
                font_size,
                multiline,
            }),
    }) = ctx
    {
        // Single-line text never wraps: pass no width so every provider
        // measures one line. Multiline wraps at the definite available width.
        let max_width = if *multiline {
            match input.available_space.width {
                AvailableSpace::Definite(w) => Some(w),
                AvailableSpace::MaxContent | AvailableSpace::MinContent => None,
            }
        } else {
            None
        };
        let measured: Size = measurer.measure_text(text, *font_size, max_width, *multiline);
        if input.known_dimensions.width.is_none() {
            size.width = measured.w;
        }
        if input.known_dimensions.height.is_none() {
            size.height = measured.h;
        }
    }
    LayoutOutput::from_outer_size(size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::NaiveMeasurer;
    use crate::style::{Axis, LayoutStyle, Length};

    #[test]
    fn every_primitive_paints_without_wrapper_nodes() {
        let kinds = [
            NodeKind::Text(TextContent::label("painted")),
            NodeKind::Media(MediaContent {
                image: None,
                opacity: 1.0,
            }),
            NodeKind::Scroll(ScrollContent::default()),
            NodeKind::Canvas(CanvasContent::default()),
            NodeKind::Viewport3D(ViewportContent::default()),
        ];
        for kind in kinds {
            let mut tree = UiTree::new();
            let mut spec = NodeSpec::new(
                kind,
                LayoutStyle {
                    width: Length::Px(100.0),
                    height: Length::Px(40.0),
                    ..Default::default()
                },
            );
            spec.paint = Paint {
                // A border must draw even when there is no background fill.
                border: Some(crate::style::Border {
                    width: 2.0,
                    color: Oklch::WHITE,
                }),
                opacity: Some(0.5),
                clip: true,
                radius: 6.0,
                ..Default::default()
            };
            let id = tree.mount_spec(spec);
            tree.compute_layout(Size::new(100.0, 40.0), &NaiveMeasurer)
                .unwrap();
            let ops: Vec<_> = tree.emit_scene().into_iter().map(|(_, op)| op).collect();
            assert_eq!(tree.traverse(), vec![id]);
            assert!(matches!(ops.first(), Some(DisplayItem::PushOpacity(o)) if *o == 0.5));
            assert!(
                matches!(&ops[1], DisplayItem::Rect { fill, border_width, .. }
                if *fill == Oklch::TRANSPARENT && *border_width == 2.0)
            );
            assert!(matches!(&ops[2], DisplayItem::PushClip { radius, .. } if *radius == 6.0));
            assert!(matches!(&ops[ops.len() - 2], DisplayItem::PopClip));
            assert!(matches!(ops.last(), Some(DisplayItem::PopOpacity)));
        }
    }

    #[test]
    fn keyed_paint_update_preserves_editor_and_avoids_layout_invalidation() {
        let mut tree = UiTree::new();
        let field = || {
            NodeSpec::new(
                NodeKind::Text(TextContent::editable("draft", false)),
                LayoutStyle {
                    width: Length::Px(100.0),
                    height: Length::Px(40.0),
                    ..Default::default()
                },
            )
            .keyed(1u64)
        };
        let root = tree.mount_spec(column(vec![field()]));
        let id = tree.get(root).unwrap().children[0];
        tree.compute_layout(Size::new(200.0, 200.0), &NaiveMeasurer)
            .unwrap();
        let paint = Paint {
            fill: Some(Oklch::WHITE),
            ..Default::default()
        };
        let version = tree.content_version();
        assert!(tree.set_paint(id, paint));
        assert!(!tree.get(id).unwrap().dirty_layout);
        assert!(tree.content_version() > version);
        let version = tree.content_version();
        assert!(!tree.set_paint(id, paint));
        assert_eq!(tree.content_version(), version);
        let mut updated = field();
        updated.paint = Paint {
            fill: Some(Oklch::BLACK),
            ..Default::default()
        };
        tree.sync_children(root, vec![updated]);
        assert_eq!(tree.get(root).unwrap().children, vec![id]);
        assert_eq!(tree.paint(id).unwrap().fill, Some(Oklch::BLACK));
        assert_eq!(tree.get(id).map(text_of).as_deref(), Some("draft"));
    }

    #[test]
    fn keyed_static_label_restyles_without_text_change() {
        let mut tree = UiTree::new();
        let spec = || {
            NodeSpec::new(
                NodeKind::Text(TextContent::label("same")),
                LayoutStyle::default(),
            )
            .keyed(1u64)
        };
        let root = tree.mount_spec(column(vec![spec()]));
        let id = tree.get(root).unwrap().children[0];
        let mut next = spec();
        if let NodeKind::Text(ref mut tc) = next.kind {
            tc.color = Oklch::BLACK;
            tc.font_size = 24.0;
            tc.wrap = true;
        }
        tree.sync_children(root, vec![next]);
        assert_eq!(tree.get(root).unwrap().children, vec![id]);
        let NodeKind::Text(tc) = &tree.get(id).unwrap().kind else {
            panic!("text expected")
        };
        assert_eq!(tc.text(), "same");
        assert_eq!(tc.color, Oklch::BLACK);
        assert_eq!(tc.font_size, 24.0);
        assert!(tc.wrap);
    }

    fn spec_box(key: u64, label: &str) -> NodeSpec {
        NodeSpec::new(NodeKind::Box(BoxContent::default()), LayoutStyle::default())
            .keyed(key)
            .child(NodeSpec::new(
                NodeKind::Text(TextContent::label(label)),
                LayoutStyle::default(),
            ))
    }

    fn column(children: Vec<NodeSpec>) -> NodeSpec {
        let style = LayoutStyle {
            axis: Axis::Column,
            width: Length::Px(200.0),
            height: Length::Px(200.0),
            ..Default::default()
        };
        let mut root = NodeSpec::new(NodeKind::Box(BoxContent::default()), style);
        root.children = children;
        root
    }

    fn box_tree() -> (UiTree, NodeId, NodeId) {
        let mut tree = UiTree::new();
        let root_style = LayoutStyle {
            axis: Axis::Column,
            width: Length::Px(200.0),
            height: Length::Px(200.0),
            ..Default::default()
        };
        let root = tree.insert(None, NodeKind::Box(BoxContent::default()), root_style);
        let child_style = LayoutStyle {
            width: Length::Px(50.0),
            height: Length::Px(30.0),
            ..Default::default()
        };
        let child = tree.insert(
            Some(root),
            NodeKind::Box(BoxContent::default()),
            child_style,
        );
        (tree, root, child)
    }

    #[test]
    fn flex_column_layout_commits_rects() {
        let (mut tree, _root, child) = box_tree();
        tree.compute_layout(Size::new(400.0, 400.0), &NaiveMeasurer)
            .unwrap();
        let r = tree.rect(child).unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 0.0, 50.0, 30.0));
    }

    #[test]
    fn set_text_invalidates_and_remeasures() {
        let mut tree = UiTree::new();
        let root = tree.insert(
            None,
            NodeKind::Box(BoxContent::default()),
            LayoutStyle::default(),
        );
        let text = tree.insert(
            Some(root),
            NodeKind::Text(TextContent::label("hi")),
            LayoutStyle::default(),
        );
        tree.compute_layout(Size::new(400.0, 400.0), &NaiveMeasurer)
            .unwrap();
        let before = tree.rect(text).unwrap();
        assert!(tree.set_text(text, "a much longer line of text"));
        tree.compute_layout(Size::new(400.0, 400.0), &NaiveMeasurer)
            .unwrap();
        let after = tree.rect(text).unwrap();
        assert!(after.w > before.w, "{before:?} -> {after:?}");
    }

    #[test]
    fn editing_uses_editor_grapheme_safe() {
        let mut tc = TextContent::editable("a🇺🇸b", false);
        assert!(tc.is_editable());
        // Move to end, delete one grapheme: the whole flag goes, not half.
        tc.move_caret(false, false);
        tc.move_caret(false, false);
        tc.move_caret(false, false);
        assert!(tc.delete_backward());
        assert_eq!(tc.text(), "a🇺🇸");
        // Static labels allocate no editor and reject edits.
        let mut label = TextContent::label("hi");
        assert!(!label.is_editable());
        assert!(!label.insert_at_caret("x"));
        assert_eq!(label.text(), "hi");
    }

    #[test]
    fn placeholder_shows_only_when_empty_hides_on_type_restores_on_clear() {
        let mut tc = TextContent::editable("", false);
        tc.placeholder = "Search...".to_string();
        assert!(tc.is_placeholder());
        assert_eq!(tc.display_text(), "Search...");
        assert_eq!(tc.text(), "");
        assert_eq!(tc.caret(), Some(0));
        // Typing hides it; the model holds only real input, no fake content.
        tc.insert_at_caret("a");
        assert!(!tc.is_placeholder());
        assert_eq!(tc.display_text(), "a");
        assert_eq!(tc.text(), "a");
        // Clearing restores it.
        tc.delete_backward();
        assert_eq!(tc.text(), "");
        assert!(tc.is_placeholder());
        assert_eq!(tc.display_text(), "Search...");
    }

    #[test]
    fn placeholder_yields_to_ime_preedit_and_ignores_static() {
        let mut tc = TextContent::editable("", false);
        tc.placeholder = "Search...".to_string();
        // Preedit takes precedence while composing; nothing commits.
        if let TextBacking::Editable(ref mut ed) = tc.backing {
            ed.set_composition("に", 0..3);
        }
        assert!(!tc.is_placeholder());
        assert_eq!(tc.display_text(), "に");
        assert_eq!(tc.text(), "");
        if let TextBacking::Editable(ref mut ed) = tc.backing {
            ed.cancel_composition();
        }
        assert!(tc.is_placeholder());
        // Static labels ignore the field entirely.
        let mut label = TextContent::label("");
        label.placeholder = "ignored".to_string();
        assert!(!label.is_placeholder());
        assert_eq!(label.display_text(), "");
    }

    #[test]
    fn placeholder_semantics_value_stays_empty() {
        let mut tree = UiTree::new();
        let mut tc = TextContent::editable("", false);
        tc.placeholder = "Search...".to_string();
        let root = tree.mount_spec(column(vec![NodeSpec::new(
            NodeKind::Text(tc),
            LayoutStyle::default(),
        )]));
        let field = tree.get(root).unwrap().children[0];
        let nodes = tree.emit_semantics();
        let sem = nodes.iter().find(|n| n.node == field).unwrap();
        assert_eq!(sem.label, ""); // committed text, not the hint
        assert_eq!(sem.value_text, None);
        assert!(!sem.focused);
    }

    #[test]
    fn caret_and_revision_tracks_edits() {
        let mut tc = TextContent::editable("hello", false);
        let r0 = tc.revision();
        // Caret starts at 0; move to end, then insert appends.
        tc.move_caret(false, false);
        tc.move_caret(false, false);
        tc.move_caret(false, false);
        tc.move_caret(false, false);
        tc.move_caret(false, false);
        assert!(tc.insert_at_caret("!"));
        assert!(tc.revision() > r0);
        assert_eq!(tc.caret(), Some(6));
        assert_eq!(tc.text(), "hello!");
    }

    #[test]
    fn remove_drops_subtree_and_stale_handles_fail() {
        let (mut tree, root, child) = box_tree();
        assert!(tree.remove(child));
        assert!(tree.get(child).is_none());
        assert_eq!(tree.get(root).unwrap().children.len(), 0);
        assert!(!tree.remove(child));
    }

    #[test]
    fn scroll_emits_clip_ops() {
        let mut tree = UiTree::new();
        let rs = LayoutStyle {
            width: Length::Px(100.0),
            height: Length::Px(100.0),
            ..Default::default()
        };
        let root = tree.insert(
            None,
            NodeKind::Scroll(ScrollContent {
                offset: Point::new(0.0, 0.0),
            }),
            rs,
        );
        let cs = LayoutStyle {
            width: Length::Px(100.0),
            height: Length::Px(400.0),
            ..Default::default()
        };
        tree.insert(Some(root), NodeKind::Box(BoxContent::default()), cs);
        tree.compute_layout(Size::new(400.0, 400.0), &NaiveMeasurer)
            .unwrap();
        let ops: Vec<_> = tree.emit_scene().into_iter().map(|(_, op)| op).collect();
        assert!(matches!(ops[0], DisplayItem::PushClip { .. }));
        assert!(matches!(ops[ops.len() - 1], DisplayItem::PopClip));
    }

    #[test]
    fn scroll_clamps_to_content() {
        let mut tree = UiTree::new();
        let rs = LayoutStyle {
            width: Length::Px(100.0),
            height: Length::Px(100.0),
            ..Default::default()
        };
        let root = tree.insert(
            None,
            NodeKind::Scroll(ScrollContent {
                offset: Point::ZERO,
            }),
            rs,
        );
        let cs = LayoutStyle {
            width: Length::Px(100.0),
            height: Length::Px(400.0),
            ..Default::default()
        };
        tree.insert(Some(root), NodeKind::Box(BoxContent::default()), cs);
        tree.compute_layout(Size::new(400.0, 400.0), &NaiveMeasurer)
            .unwrap();
        assert!(tree.scroll_to(root, Point::new(0.0, 10_000.0)));
        let off = match tree.get(root).unwrap().kind {
            NodeKind::Scroll(s) => s.offset,
            _ => unreachable!(),
        };
        assert_eq!(off.y, 300.0);
    }

    #[test]
    fn keyed_reorder_preserves_identity() {
        let mut tree = UiTree::new();
        let root = tree.mount_spec(column(vec![
            spec_box(1, "row one"),
            spec_box(2, "row two"),
            spec_box(3, "row three"),
        ]));
        tree.compute_layout(Size::new(400.0, 400.0), &NaiveMeasurer)
            .unwrap();
        let before: Vec<NodeId> = tree.get(root).unwrap().children.clone();
        assert_eq!(before.len(), 3);
        // Reverse the rows by key. Nodes must be reused, not rebuilt.
        let after = tree.sync_children(
            root,
            vec![
                spec_box(3, "row three"),
                spec_box(1, "row one"),
                spec_box(2, "row two"),
            ],
        );
        assert_eq!(after, vec![before[2], before[0], before[1]]);
        assert_eq!(tree.len(), 7); // root + 3 rows + 3 texts, nothing leaked
    }

    #[test]
    fn focus_and_caret_survive_keyed_reorder() {
        let mut tree = UiTree::new();
        let root = tree.mount_spec(column(vec![
            NodeSpec::new(
                NodeKind::Text(TextContent::editable("first", false)),
                LayoutStyle::default(),
            )
            .keyed("a"),
            NodeSpec::new(
                NodeKind::Text(TextContent::editable("second", false)),
                LayoutStyle::default(),
            )
            .keyed("b"),
        ]));
        tree.compute_layout(Size::new(400.0, 400.0), &NaiveMeasurer)
            .unwrap();
        let ids: Vec<NodeId> = tree.get(root).unwrap().children.clone();
        // Focus the second row and type into it.
        assert!(tree.set_focused(Some(ids[1])));
        tree.set_text(ids[1], "second!");
        // Reorder: focus and content must follow the key, not the position.
        let after = tree.sync_children(
            root,
            vec![
                NodeSpec::new(
                    NodeKind::Text(TextContent::editable("", false)),
                    LayoutStyle::default(),
                )
                .keyed("b"),
                NodeSpec::new(
                    NodeKind::Text(TextContent::editable("", false)),
                    LayoutStyle::default(),
                )
                .keyed("a"),
            ],
        );
        assert_eq!(after, vec![ids[1], ids[0]]);
        assert_eq!(tree.focused(), Some(ids[1]));
        // Content was updated in place to the synced descriptor's text...
        // (sync carries the new props; identity is what survives).
        assert!(tree.get(ids[1]).is_some());
    }

    #[test]
    fn kind_change_replaces_node() {
        let mut tree = UiTree::new();
        let root = tree.mount_spec(column(vec![NodeSpec::new(
            NodeKind::Text(TextContent::label("x")),
            LayoutStyle::default(),
        )
        .keyed("k")]));
        let before: Vec<NodeId> = tree.get(root).unwrap().children.clone();
        let after = tree.sync_children(
            root,
            vec![
                NodeSpec::new(NodeKind::Box(BoxContent::default()), LayoutStyle::default())
                    .keyed("k"),
            ],
        );
        assert_ne!(after[0], before[0]);
        assert!(tree.get(before[0]).is_none());
    }

    #[test]
    fn commands_drain_before_layout_in_order() {
        let mut tree = UiTree::new();
        let root = tree.insert(
            None,
            NodeKind::Box(BoxContent::default()),
            LayoutStyle::default(),
        );
        let text = tree.insert(
            Some(root),
            NodeKind::Text(TextContent::label("a")),
            LayoutStyle::default(),
        );
        tree.push_command(UiCommand::SetText {
            node: text,
            text: "hello".into(),
        });
        tree.push_command(UiCommand::SetText {
            node: text,
            text: "hello world".into(),
        });
        assert_eq!(tree.pending_commands(), 2);
        tree.compute_layout(Size::new(400.0, 400.0), &NaiveMeasurer)
            .unwrap();
        assert_eq!(tree.pending_commands(), 0);
        assert_eq!(tree.get(text).map(text_of).as_deref(), Some("hello world"));
    }

    #[test]
    fn canvas_painter_splices_ops() {
        let mut tree = UiTree::new();
        let root = tree.insert(
            None,
            NodeKind::Box(BoxContent::default()),
            LayoutStyle::default(),
        );
        let canvas = tree.insert(
            Some(root),
            NodeKind::Canvas(CanvasContent { painter: None }),
            LayoutStyle::default(),
        );
        tree.set_canvas_painter(
            canvas,
            Box::new(|bounds| {
                vec![DisplayItem::BackdropBlur {
                    bounds,
                    radius: 8.0,
                }]
            }),
        );
        tree.compute_layout(Size::new(100.0, 100.0), &NaiveMeasurer)
            .unwrap();
        let ops = tree.emit_scene();
        assert!(ops
            .iter()
            .any(|(_, op)| matches!(op, DisplayItem::BackdropBlur { .. })));
    }

    #[test]
    fn action_callbacks_fire_without_tree_access() {
        use std::sync::{Arc, Mutex};
        let mut tree = UiTree::new();
        let root = tree.insert(
            None,
            NodeKind::Box(BoxContent::default()),
            LayoutStyle::default(),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        tree.set_action_callback(
            root,
            ActionKind::Activate,
            Box::new(move |e| {
                seen2.lock().unwrap().push(e.node);
            }),
        );
        tree.fire_action(ActionEvent {
            node: root,
            kind: ActionKind::Activate,
            text: None,
            focused: None,
        });
        assert_eq!(*seen.lock().unwrap(), vec![root]);
    }

    fn text_of(r: &NodeRecord) -> String {
        match &r.kind {
            NodeKind::Text(t) => t.text().to_string(),
            _ => String::new(),
        }
    }

    fn wide_tree(n: usize) -> (UiTree, NodeId, Vec<NodeId>) {
        let mut tree = UiTree::new();
        let root = tree.mount_spec(column((0..n).map(|i| spec_box(i as u64, "leaf")).collect()));
        tree.compute_layout(Size::new(2000.0, 2000.0), &NaiveMeasurer)
            .unwrap();
        let kids = tree.get(root).unwrap().children.clone();
        (tree, root, kids)
    }

    #[test]
    fn insert_spec_into_empty_and_nonempty_parents() {
        let mut tree = UiTree::new();
        // Empty parent: root with no children.
        let root = tree.mount_spec(column(vec![]));
        tree.compute_layout(Size::new(400.0, 400.0), &NaiveMeasurer)
            .unwrap();
        // Insert a keyed Canvas at index 0 of the empty parent: the fresh
        // detached node must never hit a taffy remove without a parent.
        let canvas = tree
            .insert_spec(
                root,
                0,
                NodeSpec::new(
                    NodeKind::Canvas(CanvasContent { painter: None }),
                    LayoutStyle::default(),
                )
                .keyed("c"),
            )
            .unwrap();
        // Index past the end clamps; keyed Text lands last.
        let text = tree
            .insert_spec(
                root,
                99,
                NodeSpec::new(
                    NodeKind::Text(TextContent::label("hi")),
                    LayoutStyle::default(),
                )
                .keyed("t"),
            )
            .unwrap();
        // Insert at index 0 pushes the others back.
        let first = tree
            .insert_spec(
                root,
                0,
                NodeSpec::new(NodeKind::Box(BoxContent::default()), LayoutStyle::default())
                    .keyed("b"),
            )
            .unwrap();
        let kids = tree.get(root).unwrap().children.clone();
        assert_eq!(kids, vec![first, canvas, text]);
        // Layout runs cleanly; rects commit for every inserted node.
        tree.compute_layout(Size::new(400.0, 400.0), &NaiveMeasurer)
            .unwrap();
        assert!(tree.rect(first).is_some());
        assert!(tree.rect(canvas).is_some());
        assert!(tree.rect(text).is_some());
        assert_eq!(tree.len(), 4);
    }

    #[test]
    fn paint_only_change_skips_layout_at_scale() {
        for n in [10usize, 1000, 10000] {
            let (mut tree, _root, kids) = wide_tree(n);
            assert!(!tree.last_stats().layout_skipped);
            let rect_before = tree.rect(kids[n / 2]).unwrap();
            let v0 = tree.content_version();
            // Paint-only mutation: fill color through a paint path.
            {
                let rec = tree.get_mut(kids[0]).unwrap();
                if let NodeKind::Box(ref mut b) = rec.kind {
                    let mut paint = b.paint;
                    paint.fill = Some(Oklch::from_hex("#123456").unwrap());
                    b.paint = paint;
                }
            }
            assert!(tree.content_version() > v0);
            tree.compute_layout(Size::new(2000.0, 2000.0), &NaiveMeasurer)
                .unwrap();
            let stats = tree.last_stats();
            assert!(stats.layout_skipped, "n={n}");
            assert_eq!(stats.nodes_measured, 0, "n={n}");
            assert_eq!(stats.nodes_traversed, 0, "n={n}");
            // Committed rects are untouched by the skipped layout.
            assert_eq!(tree.rect(kids[n / 2]).unwrap(), rect_before);
            // ...but the paint delta lists exactly the touched node.
            let changed = tree.changed_paint(v0);
            assert_eq!(changed, vec![kids[0]]);
        }
    }

    #[test]
    fn viewport_change_forces_layout() {
        let (mut tree, _root, _kids) = wide_tree(100);
        tree.compute_layout(Size::new(2000.0, 2000.0), &NaiveMeasurer)
            .unwrap();
        assert!(tree.last_stats().layout_skipped);
        tree.compute_layout(Size::new(800.0, 600.0), &NaiveMeasurer)
            .unwrap();
        assert!(!tree.last_stats().layout_skipped);
    }

    #[test]
    fn emit_semantics_carries_roles_bounds_focus() {
        use aqua_core::{Role, SemanticNode};
        let mut tree = UiTree::new();
        let root = tree.mount_spec(column(vec![
            NodeSpec::new(
                NodeKind::Text(TextContent::label("hello")),
                LayoutStyle::default(),
            )
            .keyed("t"),
            NodeSpec::new(NodeKind::Box(BoxContent::default()), LayoutStyle::default()).keyed("b"),
        ]));
        tree.compute_layout(Size::new(300.0, 300.0), &NaiveMeasurer)
            .unwrap();
        let kids = tree.get(root).unwrap().children.clone();
        assert!(tree.set_focusable(kids[1], true));
        tree.set_semantics(
            kids[1],
            NodeSemantics {
                role: Role::Button,
                label: "Add".to_string(),
                disabled: false,
                selected: false,
                value_text: None,
                collection: None,
            },
        );
        assert!(tree.set_focused(Some(kids[1])));
        let nodes: Vec<SemanticNode> = tree.emit_semantics();
        assert_eq!(nodes.len(), 3);
        let button = nodes.iter().find(|n| n.node == kids[1]).unwrap();
        assert_eq!(button.role, Role::Button);
        assert_eq!(button.label, "Add");
        assert!(button.focused);
        assert!(button.bounds.is_some());
        assert_eq!(button.children.len(), 0);
        // Text label falls back to content; root lists both children.
        let text = nodes.iter().find(|n| n.node == kids[0]).unwrap();
        assert_eq!(text.label, "hello");
        let root_node = nodes.iter().find(|n| n.node == root).unwrap();
        assert_eq!(root_node.children, kids);
        assert!(!root_node.focused);
    }

    #[test]
    fn unknown_semantics_update_fails_closed() {
        use aqua_core::CollectionSemantics;
        let mut tree = UiTree::new();
        let bogus = NodeId::from_raw(RawId::INVALID);
        assert!(!tree.set_semantics(
            bogus,
            NodeSemantics {
                role: Role::Button,
                label: String::new(),
                disabled: false,
                selected: false,
                value_text: None,
                collection: Some(CollectionSemantics {
                    set_size: Some(100),
                    position_in_set: Some(1),
                    ..Default::default()
                }),
            }
        ));
    }

    #[test]
    fn collection_semantics_round_trip_through_emit() {
        use aqua_core::CollectionSemantics;
        let mut tree = UiTree::new();
        let root = tree.mount_spec(column(vec![spec_box(1, "row one")]));
        let row = tree.get(root).unwrap().children[0];
        assert!(tree.set_semantics(
            row,
            NodeSemantics {
                role: Role::ListItem,
                label: String::new(),
                disabled: false,
                selected: false,
                value_text: None,
                collection: Some(CollectionSemantics {
                    set_size: Some(100),
                    position_in_set: Some(42),
                    ..Default::default()
                }),
            }
        ));
        // Default is None (non-collection nodes report no coordinates).
        assert!(tree.get(root).unwrap().semantics.collection.is_none());
        let nodes = tree.emit_semantics();
        let sem = nodes.iter().find(|n| n.node == row).unwrap();
        assert_eq!(sem.role, Role::ListItem);
        assert_eq!(
            sem.collection,
            Some(CollectionSemantics {
                set_size: Some(100),
                position_in_set: Some(42),
                ..Default::default()
            })
        );
        let root_sem = nodes.iter().find(|n| n.node == root).unwrap();
        assert_eq!(root_sem.collection, None);
    }

    #[test]
    fn managed_container_skips_children_and_scroll() {
        use crate::style::Length;
        // Driver-owned containers: style/paint flow, children and scroll
        // stay with the driver even when the fresh spec disagrees.
        let mut tree = UiTree::new();
        let root = tree.mount_spec(column(vec![]));
        let body = tree
            .insert_spec(
                root,
                0,
                NodeSpec::new(
                    NodeKind::Scroll(ScrollContent {
                        offset: Point::new(0.0, 120.0),
                    }),
                    LayoutStyle {
                        width: Length::Px(200.0),
                        height: Length::Px(200.0),
                        ..Default::default()
                    },
                )
                .keyed("body"),
            )
            .unwrap();
        let row = tree
            .insert_spec(
                body,
                0,
                NodeSpec::new(
                    NodeKind::Text(TextContent::label("row")),
                    LayoutStyle::default(),
                )
                .keyed("row-7"),
            )
            .unwrap();
        // Fresh managed spec: zero offset, no children, different paint.
        let mut fresh = NodeSpec::new(
            NodeKind::Scroll(ScrollContent {
                offset: Point::ZERO,
            }),
            LayoutStyle {
                width: Length::Px(200.0),
                height: Length::Px(200.0),
                ..Default::default()
            },
        );
        fresh.key = Some(hash_key("body"));
        fresh.paint = Paint {
            fill: Some(Oklch::BLACK),
            ..Default::default()
        };
        fresh.collection_managed = true;
        let ids = tree.sync_children(root, vec![fresh]);
        assert_eq!(ids, vec![body]);
        // Scroll offset preserved (not clobbered to zero).
        assert_eq!(tree.scroll_offset(body), Some(Point::new(0.0, 120.0)));
        // Realized children preserved (not replaced by the empty shell).
        assert_eq!(tree.get(body).unwrap().children, vec![row]);
        // Paint still flowed through.
        assert_eq!(tree.paint(body).unwrap().fill, Some(Oklch::BLACK));
    }

    #[test]
    fn large_keyed_reorder_preserves_identity() {
        // O(n) reconciliation (index map + draining iterator): reversing
        // 2000 keyed rows reuses every node instead of rebuilding.
        let n = 2000usize;
        let make = |rev: bool| {
            let mut idx: Vec<usize> = (0..n).collect();
            if rev {
                idx.reverse();
            }
            column(
                idx.into_iter()
                    .map(|i| spec_box(i as u64, "leaf"))
                    .collect(),
            )
        };
        let mut tree = UiTree::new();
        let root = tree.mount_spec(make(false));
        let before: Vec<NodeId> = tree.get(root).unwrap().children.clone();
        let after = tree.sync_children(root, make(true).children);
        assert_eq!(after.len(), n);
        for (i, id) in after.iter().enumerate() {
            assert_eq!(*id, before[n - 1 - i]);
        }
        assert_eq!(tree.len(), 2 * n + 1);
    }
    #[test]
    fn removing_row_with_focused_descendant_restores_live_parent() {
        let mut tree = UiTree::new();
        let root = tree.insert(
            None,
            NodeKind::Box(BoxContent::default()),
            LayoutStyle::default(),
        );
        let row = tree.insert(
            Some(root),
            NodeKind::Box(BoxContent::default()),
            LayoutStyle::default(),
        );
        let field = tree.insert(
            Some(row),
            NodeKind::Text(TextContent::editable("draft", false)),
            LayoutStyle::default(),
        );
        tree.set_focusable(field, true);
        assert!(tree.set_focused(Some(field)));
        assert!(tree.remove(row));
        assert_eq!(tree.focused(), Some(root));
        assert!(tree.get(field).is_none());
    }
    #[test]
    fn synchronized_fresh_children_keep_parent_chain() {
        let mut tree = UiTree::new();
        let root = tree.insert(
            None,
            NodeKind::Box(BoxContent::default()),
            LayoutStyle::default(),
        );
        let child = NodeSpec::new(NodeKind::Box(BoxContent::default()), LayoutStyle::default())
            .child(NodeSpec::new(
                NodeKind::Text(TextContent::label("nested")),
                LayoutStyle::default(),
            ));
        let ids = tree.sync_children(root, vec![child]);
        assert_eq!(tree.get(ids[0]).unwrap().parent, Some(root));
        let grandchild = tree.get(ids[0]).unwrap().children[0];
        assert_eq!(tree.get(grandchild).unwrap().parent, Some(ids[0]));
    }
}
