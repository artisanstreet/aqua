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

pub mod interaction;

use aqua_text::document::SelectionBinding;
use aqua_text::{StyledSpan, TextStyle};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use aqua_core::{
    media_layer, Arena, CollectionSemantics, DisplayItem, NodeId, Oklch, Point, RawId, Rect, Role,
    SemanticNode, Size, TextRun, Transform2D, ViewportId,
};
use aqua_text::{LineMode, Selection, TextEditor, TextModel};
use taffy::{AvailableSpace, LayoutInput, TaffyTree};

use crate::layout::TextMeasurer;
use crate::style::{LayoutStyle, Paint};

pub type PrepaintCallback = Box<dyn Fn(&UiTree, NodeId)>;

pub type ObserveCallback = Box<dyn Fn(&UiTree, NodeId)>;

/// Keyed-identity marker for reconciliation. Keys hash to `u64` with a fixed
/// seed so equal keys match within and across runs.
pub type KeyHash = u64;

enum KeySlot {
    One(NodeId),
    Many(Vec<NodeId>),
}

impl KeySlot {
    fn push(&mut self, id: NodeId) {
        match self {
            Self::One(first) => *self = Self::Many(vec![*first, id]),
            Self::Many(ids) => ids.push(id),
        }
    }

    fn first(&self) -> NodeId {
        match self {
            Self::One(id) => *id,
            Self::Many(ids) => ids[0],
        }
    }

    fn last(&self) -> NodeId {
        match self {
            Self::One(id) => *id,
            Self::Many(ids) => ids[ids.len() - 1],
        }
    }

    fn to_vec(&self) -> Vec<NodeId> {
        match self {
            Self::One(id) => vec![*id],
            Self::Many(ids) => ids.clone(),
        }
    }
}

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
    Editable(Box<TextEditor>),
}

/// Text node content and editing state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextAppearance {
    pub selection: Oklch,
    pub caret: Oklch,
    pub caret_width: f32,
    pub ime_underline: Oklch,
    pub ime_underline_width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuiTextAppearance {
    pub selection: aqua_core::GpuiHsla,
    pub caret: aqua_core::GpuiHsla,
    pub caret_width: f32,
    pub ime_underline: aqua_core::GpuiHsla,
    pub ime_underline_width: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextInlineGlyph {
    pub byte: usize,
    pub size: f32,
    pub intrinsic: Option<(f32, f32)>,
    pub source: aqua_core::vector::VectorSource,
}

pub(crate) fn valid_gpui_text_opacity(
    text: &str,
    ranges: &[aqua_core::scene::GpuiTextOpacityRange],
) -> bool {
    let mut previous_end = 0;
    ranges.iter().all(|range| {
        let valid = range.bytes.start >= previous_end
            && range.bytes.start < range.bytes.end
            && range.bytes.end <= text.len()
            && text.is_char_boundary(range.bytes.start)
            && text.is_char_boundary(range.bytes.end)
            && range.opacity.is_finite()
            && (0.0..=1.0).contains(&range.opacity);
        previous_end = range.bytes.end;
        valid
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextInlinePaint {
    pub bytes: std::ops::Range<usize>,
    pub content: TextInlinePaintContent,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TextInlinePaintContent {
    Svg {
        source: std::sync::Arc<str>,
        tint: Option<aqua_core::GpuiHsla>,
    },
    FixedSvg {
        source: std::sync::Arc<str>,
        tint: Option<aqua_core::GpuiHsla>,
        size: f32,
        intrinsic: Option<(f32, f32)>,
    },
    Swatch {
        fill: aqua_core::GpuiHsla,
        edge: aqua_core::GpuiHsla,
        centered: bool,
    },
}

pub trait NativeTextModel {
    fn paint_revision(&self) -> Option<u64>;
    fn matches_text(&self, text: &str) -> bool;
    fn selected_bytes(&self) -> Option<std::ops::Range<usize>>;
    fn replace_utf16(&self, text: &str, selected: Option<std::ops::Range<usize>>);
}

pub struct TextContent {
    pub text_style: TextStyle,
    pub spans: Vec<StyledSpan>,
    pub inline_glyphs: Vec<TextInlineGlyph>,
    pub inline_paints: Vec<TextInlinePaint>,
    pub gpui_text_opacity: std::sync::Arc<[aqua_core::scene::GpuiTextOpacityRange]>,
    pub selection_source: Option<SelectionBinding>,
    pub interaction_ranges: Vec<aqua_text::TextInteractionRange>,
    pub selection_auto_scroll: bool,
    pub text_appearance: Option<TextAppearance>,
    pub gpui_text_appearance: Option<GpuiTextAppearance>,
    pub external_text_input: bool,
    pub native_text_model: Option<Rc<dyn NativeTextModel>>,
    pub viewport_scroll_max: Option<(f32, f32)>,
    pub follow_text_caret: bool,
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
    pub overflow: Option<aqua_text::overflow::OverflowMode>,
    pub placeholder_color: Option<Oklch>,
    pub gpui_placeholder_color: Option<aqua_core::GpuiHsla>,
    pub gpui_source_placeholder_color: Option<aqua_core::GpuiTextColor>,
}

impl TextContent {
    pub fn effective_text_style(&self) -> TextStyle {
        let mut style = self.text_style.clone();
        let ratio = self.font_size / style.size_px.max(0.01);
        style.size_px = self.font_size;
        style.line_height_px *= ratio;
        if self.is_placeholder() {
            if let Some(color) = self.gpui_source_placeholder_color {
                style.gpui_source_color = Some([
                    color.hue_degrees,
                    color.saturation,
                    color.lightness,
                    color.alpha,
                ]);
                style.gpui_color = None;
            } else if let Some(color) = self.gpui_placeholder_color {
                style.gpui_source_color = None;
                style.gpui_color = Some([color.h, color.s, color.l, color.a]);
            } else if self.placeholder_color.is_some() {
                style.gpui_color = None;
                style.gpui_source_color = None;
            }
        }
        style
    }

    pub fn shaping_spans(&self) -> &[StyledSpan] {
        let source = self.text();
        let composing =
            matches!(&self.backing, TextBacking::Editable(editor) if editor.is_composing());
        if !self.password
            && (!composing || self.external_text_input)
            && !self.is_placeholder()
            && self.spans.iter().map(|span| span.text.len()).sum::<usize>() == source.len()
            && self
                .spans
                .iter()
                .flat_map(|span| span.text.chars())
                .eq(source.chars())
        {
            &self.spans
        } else {
            &[]
        }
    }

    /// Static label: model-backed, no editing machinery.
    pub fn label(text: impl Into<String>) -> Self {
        Self {
            backing: TextBacking::Static(TextModel::new(text)),
            text_style: TextStyle {
                size_px: 14.0,
                line_height_px: 16.8,
                ..Default::default()
            },
            spans: Vec::new(),
            inline_glyphs: Vec::new(),
            inline_paints: Vec::new(),
            gpui_text_opacity: Default::default(),
            selection_source: None,
            interaction_ranges: Vec::new(),
            selection_auto_scroll: true,
            text_appearance: None,
            gpui_text_appearance: None,
            external_text_input: false,
            native_text_model: None,
            viewport_scroll_max: None,
            follow_text_caret: false,
            font_size: 14.0,
            color: Oklch::WHITE,
            selectable: false,
            selection: None,
            wrap: false,
            password: false,
            placeholder: String::new(),
            overflow: None,
            placeholder_color: None,
            gpui_placeholder_color: None,
            gpui_source_placeholder_color: None,
        }
    }

    /// Editable node: editor-backed.
    pub fn editable(text: impl Into<String>, multiline: bool) -> Self {
        Self {
            backing: TextBacking::Editable(Box::new(TextEditor::with_text(
                &text.into(),
                if multiline {
                    LineMode::Multi
                } else {
                    LineMode::Single
                },
            ))),
            text_style: TextStyle {
                size_px: 14.0,
                line_height_px: 16.8,
                ..Default::default()
            },
            spans: Vec::new(),
            inline_glyphs: Vec::new(),
            inline_paints: Vec::new(),
            gpui_text_opacity: Default::default(),
            selection_source: None,
            interaction_ranges: Vec::new(),
            selection_auto_scroll: true,
            text_appearance: None,
            gpui_text_appearance: None,
            external_text_input: false,
            native_text_model: None,
            viewport_scroll_max: None,
            follow_text_caret: false,
            font_size: 14.0,
            color: Oklch::WHITE,
            selectable: true,
            selection: None,
            wrap: multiline,
            password: false,
            placeholder: String::new(),
            overflow: None,
            placeholder_color: None,
            gpui_placeholder_color: None,
            gpui_source_placeholder_color: None,
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
        if self.text() != text {
            self.spans.clear();
        }
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

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BoxContent {
    paint: Option<Box<Paint>>,
}

impl BoxContent {
    #[must_use]
    pub fn new(paint: Paint) -> Self {
        let mut content = Self::default();
        content.set_paint(paint);
        content
    }

    #[must_use]
    pub fn paint(&self) -> &Paint {
        match &self.paint {
            Some(paint) => paint,
            None => default_paint(),
        }
    }

    pub fn set_paint(&mut self, paint: Paint) {
        self.paint = (paint != Paint::default()).then(|| Box::new(paint));
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaContent {
    pub image: Option<aqua_core::ImageRef>,
    pub opacity: f32,
    pub fit: aqua_core::image::ImageFit,
    pub position: Point,
    pub source: Rect,
}

impl Default for MediaContent {
    fn default() -> Self {
        Self {
            image: None,
            opacity: 1.0,
            fit: aqua_core::image::ImageFit::Fill,
            position: Point::new(0.5, 0.5),
            source: Rect::from_edges(0.0, 0.0, 1.0, 1.0),
        }
    }
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

pub type ContextCanvasPaintCallback = Box<dyn Fn(CanvasPaintContext<'_>) -> Vec<DisplayItem>>;

enum CanvasPainter {
    Legacy(CanvasPaintCallback),
    Context(ContextCanvasPaintCallback),
    ScrollStable(CanvasPaintCallback),
}

struct RegisteredCanvasPainter {
    callback: CanvasPainter,
    token: Rc<()>,
}

struct DeferredCanvasAction {
    node: NodeId,
    painter: u64,
    token: Rc<()>,
    action: Box<dyn FnOnce()>,
}

pub struct CanvasPaintContext<'a> {
    pub bounds: Rect,
    pub clip: Rect,
    node: NodeId,
    painter: u64,
    token: Rc<()>,
    deferred: &'a RefCell<Vec<DeferredCanvasAction>>,
}

impl CanvasPaintContext<'_> {
    pub fn defer(&self, action: impl FnOnce() + 'static) {
        self.deferred.borrow_mut().push(DeferredCanvasAction {
            node: self.node,
            painter: self.painter,
            token: self.token.clone(),
            action: Box::new(action),
        });
    }
}

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
    Text(Box<TextContent>),
    /// Visual media presentation.
    Media(MediaContent),
    /// Scrollable viewport.
    Scroll(ScrollContent),
    /// Application custom drawing via a registered painter.
    Canvas(CanvasContent),
    /// Programmable GPU content under ordinary UI layout/clipping.
    Viewport3D(ViewportContent),
}

impl NodeKind {
    #[must_use]
    pub fn text(content: TextContent) -> Self {
        Self::Text(Box::new(content))
    }
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
    pub control: Option<Box<crate::controls::Control>>,
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
    pub preserve_scroll_offset: bool,
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
            preserve_scroll_offset: false,
            control: None,
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
    Custom(u64),
    /// Activation gesture (click/tap/Enter on an activatable node).
    Activate,
    AccessibilityActivate,
    /// Editable text content changed (carries the new committed text).
    TextChanged,
    /// Keyboard focus entered or left the node.
    FocusChanged,
    /// Enter in a single-line editable field.
    Submit,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ActivationSource {
    Pointer {
        down: aqua_core::PointerEvent,
        up: aqua_core::PointerEvent,
        button: aqua_core::PointerButton,
        modifiers: aqua_core::Modifiers,
        click_count: Option<usize>,
    },
    Keyboard(aqua_core::KeyEvent),
    Touch {
        position: Point,
        tap_count: usize,
        long_press: bool,
    },
}

#[derive(Debug, Clone)]
pub struct ActionEvent {
    pub source: Option<ActivationSource>,
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
    Interaction(Box<interaction::InteractionCommand>),
    EditText {
        node: NodeId,
        revision: Option<u64>,
        edit: crate::input::TextEdit,
    },
    OpenPopup {
        node: NodeId,
        content: Box<crate::element::Element>,
        options: crate::popup::PopupOptions,
    },
    ArmTooltip {
        node: NodeId,
        tooltip: crate::popup::Tooltip,
    },
    HideTooltip {
        node: NodeId,
    },
    ChooseOption {
        node: NodeId,
        index: usize,
    },
    SetControlValue {
        node: NodeId,
        value: f64,
    },
    FollowTextCaret {
        node: NodeId,
    },
    SetTextPresentation {
        node: NodeId,
        spans: Vec<StyledSpan>,
        glyphs: Vec<TextInlineGlyph>,
    },
    SetTextInlinePaint {
        node: NodeId,
        paints: Vec<TextInlinePaint>,
    },
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
    invalidation_counter: Rc<std::cell::Cell<u64>>,
}

impl CommandSink {
    /// Create an empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self {
            queue: Rc::new(RefCell::new(Vec::new())),
            invalidation_counter: Rc::new(std::cell::Cell::new(0)),
        }
    }

    pub fn invalidation_counter(&self) -> Rc<std::cell::Cell<u64>> {
        self.invalidation_counter.clone()
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
    pub collection: Option<Box<CollectionSemantics>>,
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
    pub control: Option<Box<crate::controls::Control>>,
    /// Node kind + properties.
    pub kind: NodeKind,
    /// Typed style.
    pub style: LayoutStyle,
    pub(crate) paint_slot: Option<Box<Paint>>,
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
    pub tab_index: i32,
    pub(crate) keyboard_release_activation: bool,
    pub description: Option<String>,
    pub debug_selector: Option<String>,
    pub(crate) hit_test_excluded: bool,
    /// Stable key for keyed reconciliation (`None` = positional).
    pub key: Option<KeyHash>,
    /// Accessibility metadata (explicit roles; see [`NodeSemantics`]).
    pub semantics: NodeSemantics,
}

fn default_paint() -> &'static Paint {
    thread_local! {
        static DEFAULT: &'static Paint = Box::leak(Box::default());
    }
    DEFAULT.with(|paint| *paint)
}

impl NodeRecord {
    #[must_use]
    pub fn paint(&self) -> &Paint {
        match &self.paint_slot {
            Some(paint) => paint,
            None => default_paint(),
        }
    }
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TreeFootprint {
    pub nodes: usize,
    pub arena_slots: usize,
    pub arena_slot_bytes: usize,
    pub taffy_nodes: usize,
    pub layout_slots: usize,
    pub interaction_entries: usize,
}

/// Per-leaf Taffy context. Text leaves carry measurable content; every other
/// leaf measures from style/availability.
#[derive(Debug, Clone, Default)]
pub struct LeafCtx {
    /// Text measurement input (`None` for non-text leaves).
    pub text: Option<Box<LeafText>>,
}

/// Text measurement input owned by the UI tree.
#[derive(Debug, Clone)]
pub struct LeafText {
    pub text_style: TextStyle,
    pub spans: Vec<StyledSpan>,
    /// Display text to measure.
    pub text: String,
    /// Font size in logical px.
    pub font_size: f32,
    /// Whether to wrap at the available width.
    pub multiline: bool,
    pub memo: Option<MeasureMemo>,
}

#[derive(Debug, Clone, Copy)]
pub struct MeasureMemo {
    epoch: u64,
    max_width_bits: u32,
    size: Size,
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
    /// Nodes the last `compute_layout` looked at, dirty scan plus commit.
    pub nodes_visited: usize,
    /// Scene ops produced by the last [`UiTree::emit_scene`].
    pub scene_ops: usize,
    /// [`UiTree::content_version`] at the last emit.
    pub content_version: u64,
}

/// Retained tree plus layout engine. `aqua-ui` never touches GPU or windows:
///
/// - layout runs headless through Taffy;
/// - scene emission produces renderer-neutral [`DisplayItem`]s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MotionGroups {
    pub opacity: bool,
    pub translate: bool,
}

#[derive(Clone, Copy, Debug)]
struct ScrollSlot {
    view: Rect,
    window: Rect,
    op: u32,
    volatile: bool,
    generation: u64,
}

const SCROLL_OVERSCAN_MIN: f32 = 160.0;
const SCROLL_OVERSCAN_MAX: f32 = 480.0;

fn rect_contains_rect(outer: Rect, inner: Rect) -> bool {
    outer.x <= inner.x
        && outer.y <= inner.y
        && outer.right() >= inner.right()
        && outer.bottom() >= inner.bottom()
}

pub struct UiTree {
    interaction: interaction::InteractionStore,
    gpui_paint: bool,
    portal_roots: std::collections::HashSet<NodeId>,
    portal_order: Vec<NodeId>,
    follow_caret_queue: Vec<NodeId>,
    exiting: std::collections::HashSet<NodeId>,
    access_hidden: std::collections::HashSet<NodeId>,
    motion_groups: HashMap<NodeId, MotionGroups>,
    hoist_guards: Vec<(taffy::NodeId, taffy::NodeId)>,
    motion_declared: bool,
    pub(crate) control_router: crate::controls::ControlRouter,
    pub(crate) input: crate::input::InputRouter,
    nodes: Arena<NodeRecord>,
    root: Option<NodeId>,
    taffy: TaffyTree<LeafCtx>,
    gpui_layout_scale: Option<f32>,
    layout_styles_dirty: bool,
    key_index: RefCell<Option<HashMap<KeyHash, KeySlot>>>,
    structure_epoch: u64,
    /// UI node (raw id) to Taffy node.
    tmap: HashMap<RawId, taffy::NodeId>,
    /// Committed absolute rects, refreshed by [`UiTree::compute_layout`].
    rects: HashMap<RawId, Rect>,
    /// Scroll content extents per scroll node.
    content: HashMap<RawId, Size>,
    /// Currently focused node.
    focused: Option<NodeId>,
    pub(crate) focus_generation: u64,
    pub(crate) pending_keyboard_activation: Option<(NodeId, u64)>,
    /// Pending binding commands, drained before layout. Shared cloneably
    /// with reactive effects via [`UiTree::command_sink`].
    sink: CommandSink,
    /// Registered canvas painters by painter id.
    painters: HashMap<u64, RegisteredCanvasPainter>,
    deferred_canvas_actions: RefCell<Vec<DeferredCanvasAction>>,
    prepaints: HashMap<RawId, PrepaintCallback>,
    observers: HashMap<RawId, ObserveCallback>,
    observer_order: RefCell<Option<(u64, Vec<NodeId>)>>,
    observed_version: std::cell::Cell<Option<u64>>,
    /// Next canvas painter id.
    next_painter: u64,
    /// Action callbacks by (node, action kind).
    actions: HashMap<(RawId, ActionKind), ActionCallback>,
    bound_actions: HashSet<(RawId, ActionKind)>,
    /// Armed press per pointer contact: contact id → activatable node.
    /// Set on primary down, consumed on up/cancel. Cleared when the node is
    /// removed or the window loses focus, so a stale arm can never fire.
    press_captures: HashMap<u64, NodeId>,
    pub(crate) press_sources: HashMap<u64, (aqua_core::PointerEvent, aqua_core::PointerButton)>,
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
    scroll_patches: u64,
    scroll_state: RefCell<HashMap<RawId, ScrollSlot>>,
    scroll_active: RefCell<Vec<RawId>>,
    emit_generation: std::cell::Cell<u64>,
    /// Viewport covered by the last committed rect set.
    last_viewport: Option<Size>,
    /// Statistics for the last layout + emit (dirty-tracking evidence).
    last_stats: FrameStats,
    visited: std::cell::Cell<usize>,
    layout_visited: std::cell::Cell<usize>,
}

impl Default for UiTree {
    fn default() -> Self {
        Self::new()
    }
}

impl UiTree {
    pub(crate) fn measure_intrinsic_width(
        spec: NodeSpec,
        measurer: &dyn TextMeasurer,
        scale: Option<f32>,
    ) -> Result<f32, String> {
        let mut tree = Self::new();
        tree.set_gpui_layout_scale(scale);
        let root = tree.mount_spec(spec);
        let dirty = tree.collect_dirty_layout(root);
        tree.sync_layout_styles(&dirty)?;
        let troot = tree.tmap[&root.raw()];
        let epoch = measurer.memo_epoch();
        tree.taffy
            .compute_layout_with_measure(
                troot,
                taffy::geometry::Size {
                    width: AvailableSpace::MaxContent,
                    height: AvailableSpace::MaxContent,
                },
                |input: LayoutInput, _, ctx, style| {
                    taffy::compute_leaf_layout(
                        input,
                        style,
                        |_, _| 0.0,
                        |known, available| match scale {
                            Some(scale) => {
                                measure_leaf_gpui(known, available, ctx, measurer, scale, epoch)
                            }
                            None => measure_leaf(known, available, ctx, measurer, epoch),
                        },
                    )
                },
            )
            .map_err(|error| format!("taffy layout failed: {error:?}"))?;
        let width = tree
            .taffy
            .layout(troot)
            .map_err(|error| format!("taffy layout failed: {error:?}"))?
            .size
            .width;
        Ok(width / scale.unwrap_or(1.0))
    }

    pub(crate) fn gpui_layout_scale(&self) -> Option<f32> {
        self.gpui_layout_scale
    }

    /// Create an empty tree.
    #[must_use]
    pub fn new() -> Self {
        Self {
            interaction: interaction::InteractionStore::default(),
            gpui_paint: false,
            portal_roots: std::collections::HashSet::new(),
            follow_caret_queue: Vec::new(),
            portal_order: Vec::new(),
            exiting: std::collections::HashSet::new(),
            access_hidden: std::collections::HashSet::new(),
            motion_groups: HashMap::new(),
            hoist_guards: Vec::new(),
            motion_declared: false,
            control_router: crate::controls::ControlRouter::default(),
            input: crate::input::InputRouter::default(),
            nodes: Arena::new(),
            root: None,
            taffy: TaffyTree::new(),
            gpui_layout_scale: None,
            layout_styles_dirty: false,
            key_index: RefCell::new(None),
            structure_epoch: 0,
            tmap: HashMap::new(),
            rects: HashMap::new(),
            content: HashMap::new(),
            focused: None,
            focus_generation: 0,
            pending_keyboard_activation: None,
            sink: CommandSink::new(),
            painters: HashMap::new(),
            deferred_canvas_actions: RefCell::new(Vec::new()),
            prepaints: HashMap::new(),
            observers: HashMap::new(),
            observer_order: RefCell::new(None),
            observed_version: std::cell::Cell::new(None),
            next_painter: 1,
            actions: HashMap::new(),
            bound_actions: HashSet::new(),
            press_captures: HashMap::new(),
            press_sources: HashMap::new(),
            layout_version: 0,
            committed_layout: 0,
            content_version: 0,
            scroll_patches: 0,
            scroll_state: RefCell::new(HashMap::new()),
            scroll_active: RefCell::new(Vec::new()),
            emit_generation: std::cell::Cell::new(0),
            last_viewport: None,
            last_stats: FrameStats::default(),
            visited: std::cell::Cell::new(0),
            layout_visited: std::cell::Cell::new(0),
        }
    }

    pub fn take_follow_caret_nodes(&mut self) -> Vec<NodeId> {
        std::mem::take(&mut self.follow_caret_queue)
    }

    pub fn requeue_follow_caret(&mut self, node: NodeId) {
        self.follow_caret_queue.push(node);
    }

    pub fn take_visited(&self) -> usize {
        self.visited.replace(0)
    }

    pub fn take_layout_visited(&self) -> usize {
        self.layout_visited.replace(0)
    }

    pub fn set_gpui_paint(&mut self, enabled: bool) {
        if self.gpui_paint != enabled {
            self.gpui_paint = enabled;
            self.content_version += 1;
        }
    }

    pub fn set_gpui_layout_scale(&mut self, scale: Option<f32>) -> bool {
        if scale.is_some_and(|value| !value.is_finite() || value <= 0.0) {
            return false;
        }
        if self.gpui_layout_scale == scale {
            return false;
        }
        self.gpui_layout_scale = scale;
        self.layout_styles_dirty = true;
        if scale.is_some() {
            self.taffy.disable_rounding();
        } else {
            self.taffy.enable_rounding();
        }
        for &node in self.tmap.values() {
            let _ = self.taffy.mark_dirty(node);
        }
        self.bump_layout();
        for (_, record) in self.nodes.iter_mut() {
            record.dirty_layout = true;
            record.dirty_paint = true;
            record.paint_rev = self.content_version;
        }
        true
    }

    pub fn invalidate_layout_cache(&mut self) {
        for &node in self.tmap.values() {
            let _ = self.taffy.mark_dirty(node);
        }
        self.rects.clear();
        self.content.clear();
        self.bump_layout();
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

    pub fn invalidation_epoch(&self) -> u64 {
        self.content_version
            .wrapping_add(self.sink.invalidation_counter.get())
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
    pub fn cancel_blocked_presses(&mut self, allowed: impl Fn(&Self, NodeId) -> bool) {
        let blocked: Vec<_> = self
            .press_captures
            .iter()
            .filter(|(_, node)| {
                self.get(**node).is_none() || self.is_disabled(**node) || !allowed(self, **node)
            })
            .map(|(contact, _)| *contact)
            .collect();
        for contact in blocked {
            self.press_sources.remove(&contact);
            self.take_press(contact);
        }
    }

    pub fn set_focused(&mut self, id: Option<NodeId>) -> bool {
        if id.is_some_and(|node| {
            self.get(node).is_none_or(|record| !record.focusable) || self.is_disabled(node)
        }) {
            return false;
        }
        let previous = self.focused;
        if previous == id {
            return true;
        }
        self.focused = id;
        self.focus_generation = self.focus_generation.wrapping_add(1);
        self.pending_keyboard_activation = None;
        self.queue_interaction_focus(previous, id);
        for (node, focused) in [(previous, false), (id, true)] {
            if let Some(node) = node {
                if self
                    .get(node)
                    .is_some_and(|record| record.paint().focus_ring.is_some())
                {
                    self.touch_paint(node);
                }
                self.fire_action(ActionEvent {
                    source: None,
                    node,
                    kind: ActionKind::FocusChanged,
                    text: None,
                    focused: Some(focused),
                });
            }
        }
        true
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

    pub fn set_access_hidden(&mut self, node: NodeId, hidden: bool) {
        let changed = match hidden {
            true => self.access_hidden.insert(node),
            false => self.access_hidden.remove(&node),
        };
        if changed {
            self.content_version += 1;
        }
    }

    #[must_use]
    pub fn is_access_hidden(&self, node: NodeId) -> bool {
        self.exiting.contains(&node) || self.access_hidden.contains(&node)
    }

    fn accessible_nodes(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = self.root.map_or(Vec::new(), |r| vec![r]);
        while let Some(id) = stack.pop() {
            if self.is_access_hidden(id) {
                continue;
            }
            out.push(id);
            if let Some(rec) = self.nodes.get(id.raw()) {
                stack.extend(rec.children.iter().rev().copied());
            }
        }
        self.visited.set(self.visited.get() + out.len());
        out
    }

    pub fn emit_semantics(&self) -> Vec<SemanticNode> {
        let mut out = Vec::new();
        for id in self.accessible_nodes() {
            let Some(rec) = self.nodes.get(id.raw()) else {
                continue;
            };
            let mut label = rec.semantics.label.clone();
            let mut value_text = rec.semantics.value_text.clone();
            if label.is_empty() {
                match &rec.kind {
                    NodeKind::Text(text) if text.is_editable() => label = text.placeholder.clone(),
                    NodeKind::Text(text) if !text.password => label = text.text().to_owned(),
                    _ if matches!(
                        rec.semantics.role,
                        Role::Button | Role::Checkbox | Role::Tab | Role::MenuItem
                    ) =>
                    {
                        let mut pending = rec.children.iter().rev().copied().collect::<Vec<_>>();
                        let mut labels = Vec::new();
                        while let Some(child) = pending.pop() {
                            if let Some(child) = self.get(child) {
                                match &child.kind {
                                    NodeKind::Text(text) if !text.password => {
                                        labels.push(text.text())
                                    }
                                    _ => pending.extend(child.children.iter().rev().copied()),
                                }
                            }
                        }
                        label = labels.join(" ");
                    }
                    _ => {}
                }
            }
            if let NodeKind::Text(text) = &rec.kind {
                if text.is_editable() {
                    value_text = Some(if text.password {
                        "•".repeat(text.text().chars().count())
                    } else {
                        text.text().to_owned()
                    });
                }
            }
            let bounds = self.visual_rect(id);
            let numeric_range = match rec.control.as_deref() {
                Some(crate::controls::Control::Slider(control)) => {
                    Some(aqua_core::NumericRangeSemantics {
                        min: control.range.min,
                        max: control.range.max,
                        step: control.range.step,
                        value: control.value,
                    })
                }
                Some(crate::controls::Control::Resize(control)) => {
                    Some(aqua_core::NumericRangeSemantics {
                        min: control.range.min,
                        max: control.range.max,
                        step: control.range.step,
                        value: control.value,
                    })
                }
                _ => None,
            };
            let expanded =
                self.interaction_semantics(id)
                    .expanded
                    .or_else(|| match rec.control.as_deref() {
                        Some(crate::controls::Control::Select(control)) => Some(control.is_open()),
                        _ => None,
                    });
            out.push(SemanticNode {
                node: id,
                role: rec.semantics.role,
                label,
                description: rec.description.clone(),
                focused: self.focused == Some(id),
                disabled: self.is_disabled(id),
                selected: rec.semantics.selected,
                checked: self.interaction_semantics(id).checked,
                value_text,
                numeric_range,
                expanded,
                level: self.interaction_semantics(id).level,
                children: rec
                    .children
                    .iter()
                    .copied()
                    .filter(|child| !self.is_access_hidden(*child))
                    .collect(),
                bounds,
                collection: rec.semantics.collection.as_deref().copied(),
            });
        }
        out
    }

    /// Set whether a node can take keyboard focus (component behaviors such
    /// as buttons opt in; plain containers stay unfocused). Unknown nodes
    /// fail closed.
    pub fn set_focusable(&mut self, id: NodeId, focusable: bool) -> bool {
        let Some(rec) = self.nodes.get_mut(id.raw()) else {
            return false;
        };
        rec.focusable = focusable;
        let fallback = rec.parent;
        if !focusable && self.focused == Some(id) {
            self.focused = fallback;
            self.focus_generation = self.focus_generation.wrapping_add(1);
            self.pending_keyboard_activation = None;
            self.queue_interaction_focus(Some(id), fallback);
        }
        true
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
        self.bound_actions.remove(&(node.raw(), kind));
        self.actions.insert((node.raw(), kind), cb);
        true
    }

    pub(crate) fn set_bound_action_callback(
        &mut self,
        node: NodeId,
        kind: ActionKind,
        callback: ActionCallback,
    ) {
        if self.set_action_callback(node, kind, callback) {
            self.bound_actions.insert((node.raw(), kind));
        }
    }

    pub(crate) fn retain_bound_actions(&mut self, node: NodeId, kinds: &[ActionKind]) {
        let stale: Vec<_> = self
            .bound_actions
            .iter()
            .copied()
            .filter(|(raw, kind)| *raw == node.raw() && !kinds.contains(kind))
            .collect();
        for key in stale {
            self.bound_actions.remove(&key);
            self.actions.remove(&key);
        }
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
        if let Some(previous) = self.press_captures.insert(contact, node) {
            self.queue_interaction_event(previous, interaction::InteractionEvent::Press(false));
        }
        self.queue_interaction_event(node, interaction::InteractionEvent::Press(true));
    }

    /// Consume and return the armed node for `contact`, if any.
    pub fn take_press(&mut self, contact: u64) -> Option<NodeId> {
        let node = self.press_captures.remove(&contact);
        if let Some(node) = node {
            self.queue_interaction_event(node, interaction::InteractionEvent::Press(false));
        }
        node
    }

    /// Clear all armed presses (window focus loss).
    pub fn clear_presses(&mut self) {
        for node in self.press_captures.values().copied().collect::<Vec<_>>() {
            self.queue_interaction_event(node, interaction::InteractionEvent::Press(false));
        }
        self.press_captures.clear();
        self.press_sources.clear();
        self.pending_keyboard_activation = None;
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

    #[must_use]
    pub fn footprint(&self) -> TreeFootprint {
        TreeFootprint {
            nodes: self.nodes.len(),
            arena_slots: self.nodes.capacity_slots(),
            arena_slot_bytes: std::mem::size_of::<NodeRecord>(),
            taffy_nodes: self.taffy.total_node_count(),
            layout_slots: self.tmap.capacity() + self.rects.capacity() + self.content.capacity(),
            interaction_entries: self.interaction.entry_capacity(),
        }
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
        let mut queue = self.sink.take();
        let mut count = 0;
        while !queue.is_empty() {
            count += queue.len();
            for cmd in queue {
                match cmd {
                    UiCommand::Interaction(command) => self.apply_interaction_command(*command),
                    UiCommand::EditText {
                        node,
                        revision,
                        edit,
                    } => {
                        self.apply_text_edit(node, revision, edit);
                    }
                    UiCommand::OpenPopup {
                        node,
                        content,
                        options,
                    } => self
                        .control_router
                        .request(crate::controls::ControlRequest::OpenPopup {
                            owner: node,
                            content,
                            options,
                        }),
                    UiCommand::ArmTooltip { node, tooltip } => {
                        self.control_router
                            .request(crate::controls::ControlRequest::ArmTooltip {
                                owner: node,
                                tooltip,
                            })
                    }
                    UiCommand::HideTooltip { node } => self
                        .control_router
                        .request(crate::controls::ControlRequest::HideTooltip { owner: node }),
                    UiCommand::ChooseOption { node, index } => {
                        self.choose_control_option(node, index);
                    }
                    UiCommand::SetControlValue { node, value } => {
                        self.set_control_value(node, value);
                    }
                    UiCommand::FollowTextCaret { node } => {
                        if let Some(record) = self.get_mut(node) {
                            if let NodeKind::Text(text) = &mut record.kind {
                                if text.external_text_input {
                                    text.follow_text_caret = true;
                                    self.follow_caret_queue.push(node);
                                }
                            }
                        }
                    }
                    UiCommand::SetTextPresentation {
                        node,
                        spans,
                        glyphs,
                    } => {
                        self.edit_text(node, |text| {
                            if text.external_text_input {
                                text.spans = spans;
                                text.inline_glyphs = glyphs;
                            }
                        });
                    }
                    UiCommand::SetTextInlinePaint { node, paints } => {
                        self.edit_text(node, |text| {
                            if text.external_text_input {
                                text.inline_paints = paints;
                            }
                        });
                    }
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
            queue = self.sink.take();
        }
        count
    }

    /// Mount a descriptor tree as the root, replacing any previous root.
    /// Structural replacement only; per-signal updates use commands and
    /// [`UiTree::sync_children`], never remounts.
    pub fn mount_spec(&mut self, spec: NodeSpec) -> NodeId {
        if let Some(old) = self.root {
            self.remove(old);
        }
        let id = self.build_spec(None, spec);
        self.structure_changed();
        self.root = Some(id);
        self.bump_layout();
        id
    }

    #[must_use]
    pub fn has_motion_specs(&self) -> bool {
        self.motion_declared
    }

    #[must_use]
    pub fn motion_spec(&self, id: NodeId) -> Option<Rc<aqua_style::motion::MotionSpec>> {
        self.get(id).and_then(|rec| match &rec.kind {
            NodeKind::Box(content) => content.paint().motion.clone(),
            _ => rec.paint().motion.clone(),
        })
    }

    pub fn set_hit_test_excluded(&mut self, node: NodeId, excluded: bool) {
        if let Some(record) = self.nodes.get_mut(node.raw()) {
            record.hit_test_excluded = excluded;
        }
    }

    pub fn set_motion_groups(&mut self, groups: HashMap<NodeId, MotionGroups>) -> bool {
        match self.motion_groups == groups {
            true => false,
            false => {
                self.motion_groups = groups;
                self.content_version += 1;
                true
            }
        }
    }

    pub fn is_portal(&self, node: NodeId) -> bool {
        self.portal_roots.contains(&node)
    }

    #[must_use]
    pub fn is_exiting(&self, node: NodeId) -> bool {
        self.exiting.contains(&node)
    }

    #[must_use]
    pub fn exiting_nodes(&self) -> Vec<NodeId> {
        let mut nodes: Vec<NodeId> = self.exiting.iter().copied().collect();
        nodes.sort_by_key(|node| node.raw().index);
        nodes
    }

    fn exit_motion_of(&self, node: NodeId) -> Option<aqua_style::motion::MotionRef> {
        self.paint(node)
            .and_then(|paint| paint.motion)
            .and_then(|spec| spec.presence.exit_motion())
    }

    fn begin_exit(&mut self, node: NodeId) -> bool {
        let Some(record) = self.get(node) else {
            return false;
        };
        let (Some(parent), Some(_)) = (record.parent, record.key) else {
            return false;
        };
        let mut style = record.style.clone();
        if self.exit_motion_of(node).is_none() {
            return false;
        }
        let (Some(rect), Some(parent_rect)) = (self.rect(node), self.rect(parent)) else {
            return false;
        };
        if self
            .focused
            .is_some_and(|focused| self.is_within(focused, node))
        {
            self.set_focused(Some(parent));
        }
        style.absolute = Some(crate::style::Absolute {
            left: Some(rect.x - parent_rect.x),
            top: Some(rect.y - parent_rect.y),
            right: None,
            bottom: None,
        });
        style.width = crate::style::Length::Px(rect.w);
        style.height = crate::style::Length::Px(rect.h);
        style.flex_grow = 0.0;
        style.flex_shrink = 0.0;
        self.set_style(node, style);
        if let Some(record) = self.nodes.get_mut(node.raw()) {
            record.hit_test_excluded = true;
        }
        self.exiting.insert(node);
        true
    }

    fn is_within(&self, mut node: NodeId, ancestor: NodeId) -> bool {
        loop {
            if node == ancestor {
                return true;
            }
            match self.get(node).and_then(|record| record.parent) {
                Some(parent) => node = parent,
                None => return false,
            }
        }
    }

    pub fn finish_exit(&mut self, node: NodeId) -> bool {
        self.exiting.contains(&node) && self.remove(node)
    }

    fn paint_transform(&self, id: NodeId) -> Option<Option<Transform2D>> {
        self.get(id).map(|rec| match &rec.kind {
            NodeKind::Box(b) => b.paint().transform,
            _ => rec.paint().transform,
        })
    }

    pub fn visual_transform(&self, node: NodeId) -> Option<Transform2D> {
        thread_local! {
            static PATH: RefCell<Vec<NodeId>> = const { RefCell::new(Vec::new()) };
        }
        let mut path = PATH.with(|path| std::mem::take(&mut *path.borrow_mut()));
        path.clear();
        let result = self.visual_transform_along(node, &mut path);
        path.clear();
        PATH.with(|slot| *slot.borrow_mut() = path);
        result
    }

    fn visual_transform_along(&self, node: NodeId, path: &mut Vec<NodeId>) -> Option<Transform2D> {
        let mut current = Some(node);
        while let Some(id) = current {
            path.push(id);
            if self.is_portal(id) {
                break;
            }
            current = self.get(id)?.parent;
        }
        let mut offset = Point::ZERO;
        let mut world = Transform2D::IDENTITY;
        let mut origin = Point::ZERO;
        for &id in path.iter().rev() {
            let record = self.get(id)?;
            let rect = self.rect(id)?.translate(offset.x, offset.y);
            origin = rect.origin();
            if let Some(transform) = self.paint_transform(id)? {
                let anchored = Transform2D::translate(-rect.x, -rect.y)
                    .then(transform)
                    .then(Transform2D::translate(rect.x, rect.y));
                world = anchored.then(world);
            }
            if id != node {
                if let NodeKind::Scroll(scroll) = &record.kind {
                    offset.x -= scroll.offset.x;
                    offset.y -= scroll.offset.y;
                }
            }
        }
        Some(Transform2D::translate(origin.x, origin.y).then(world))
    }

    pub fn pre_transform_rect(&self, node: NodeId) -> Option<Rect> {
        let mut rect = self.rect(node)?;
        let mut current = node;
        while !self.is_portal(current) {
            let Some(parent) = self.get(current)?.parent else {
                break;
            };
            if let NodeKind::Scroll(scroll) = &self.get(parent)?.kind {
                rect = rect.translate(-scroll.offset.x, -scroll.offset.y);
            }
            current = parent;
        }
        Some(rect)
    }

    pub fn visual_rect(&self, node: NodeId) -> Option<Rect> {
        let rect = self.rect(node)?;
        let transform = self.visual_transform(node)?;
        Some(Rect::from_edges(0.0, 0.0, rect.w, rect.h).transform(transform))
    }

    pub fn portal_roots_in_paint_order(&self) -> &[NodeId] {
        &self.portal_order
    }

    pub fn mark_portal(&mut self, node: NodeId) -> bool {
        if self.get(node).is_none_or(|record| record.parent.is_none())
            || !self.portal_roots.insert(node)
        {
            return false;
        }
        self.portal_order.push(node);
        true
    }

    pub(crate) fn content_children(&self, node: NodeId) -> Vec<NodeId> {
        self.get(node)
            .map(|record| {
                record
                    .children
                    .iter()
                    .copied()
                    .filter(|child| {
                        !self.portal_roots.contains(child) && !self.exiting.contains(child)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn retained_children(&self, node: NodeId) -> Vec<NodeId> {
        self.get(node)
            .map(|record| {
                record
                    .children
                    .iter()
                    .copied()
                    .filter(|child| !self.portal_roots.contains(child))
                    .collect()
            })
            .unwrap_or_default()
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
        let portals: Vec<_> = self
            .get(parent)
            .map(|record| {
                record
                    .children
                    .iter()
                    .copied()
                    .filter(|child| self.portal_roots.contains(child))
                    .collect()
            })
            .unwrap_or_default();
        let old = self.retained_children(parent);
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
                    if self.exiting.remove(&id) {
                        if let Some(record) = self.nodes.get_mut(id.raw()) {
                            record.hit_test_excluded = false;
                        }
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
        let mut lingering: Vec<NodeId> = Vec::new();
        for (id, used) in old.iter().zip(reused.iter()) {
            if !used {
                if self.exiting.contains(id) || self.begin_exit(*id) {
                    lingering.push(*id);
                } else {
                    self.remove(*id);
                }
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
        let content = built.clone();
        for node in &lingering {
            let before = old.iter().position(|id| id == node).unwrap_or(0);
            let anchor = old[..before]
                .iter()
                .rev()
                .find_map(|id| built.iter().position(|kept| kept == id));
            built.insert(anchor.map_or(0, |at| at + 1), *node);
        }
        built.extend(portals.into_iter().filter(|node| self.get(*node).is_some()));
        if self
            .get(parent)
            .is_some_and(|record| record.children == built)
        {
            return content;
        }
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
        self.structure_changed();
        if let Some(rec) = self.nodes.get_mut(parent.raw()) {
            rec.children = built.clone();
            rec.dirty_layout = true;
        }
        self.mark_layout_dirty(parent);
        content
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
                            cur.text_style = t.text_style;
                            cur.spans = t.spans;
                            cur.inline_glyphs = t.inline_glyphs;
                            cur.inline_paints = t.inline_paints;
                            cur.gpui_text_opacity = t.gpui_text_opacity;
                            cur.selection_source = t.selection_source;
                            cur.interaction_ranges = t.interaction_ranges;
                            cur.selection_auto_scroll = t.selection_auto_scroll;
                            cur.text_appearance = t.text_appearance;
                            cur.gpui_text_appearance = t.gpui_text_appearance;
                            cur.external_text_input = t.external_text_input;
                            cur.native_text_model = t.native_text_model;
                            cur.viewport_scroll_max = t.viewport_scroll_max;
                            cur.font_size = t.font_size;
                            cur.color = t.color;
                            cur.selectable = t.selectable;
                            cur.wrap = t.wrap;
                            cur.password = t.password;
                            cur.placeholder = t.placeholder;
                            cur.overflow = t.overflow;
                            cur.placeholder_color = t.placeholder_color;
                            cur.gpui_placeholder_color = t.gpui_placeholder_color;
                            cur.gpui_source_placeholder_color = t.gpui_source_placeholder_color;
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
                            cur.text_style = t.text_style;
                            cur.spans = t.spans;
                            cur.inline_glyphs = t.inline_glyphs;
                            cur.inline_paints = t.inline_paints;
                            cur.gpui_text_opacity = t.gpui_text_opacity;
                            cur.selection_source = t.selection_source;
                            cur.interaction_ranges = t.interaction_ranges;
                            cur.selection_auto_scroll = t.selection_auto_scroll;
                            cur.text_appearance = t.text_appearance;
                            cur.gpui_text_appearance = t.gpui_text_appearance;
                            cur.external_text_input = t.external_text_input;
                            cur.native_text_model = t.native_text_model;
                            cur.viewport_scroll_max = t.viewport_scroll_max;
                            cur.font_size = t.font_size;
                            cur.color = t.color;
                            cur.selectable = t.selectable;
                            cur.wrap = t.wrap;
                            cur.placeholder = t.placeholder;
                            cur.overflow = t.overflow;
                            cur.placeholder_color = t.placeholder_color;
                            cur.gpui_placeholder_color = t.gpui_placeholder_color;
                            cur.gpui_source_placeholder_color = t.gpui_source_placeholder_color;
                        }
                    }
                    self.sync_text_leaf_if_changed(id);
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
                if !managed && !spec.preserve_scroll_offset {
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
            if rec.key != spec.key {
                rec.key = spec.key;
                self.key_index.get_mut().take();
            }
            self.structure_epoch += 1;
            let mut next = spec.control;
            if let (Some(next), Some(previous)) = (&mut next, &rec.control) {
                next.preserve(previous);
            }
            rec.control = next;
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
        let control = spec.control;
        let key = spec.key;
        let style = spec.style;
        let paint = spec.paint;
        let kind = spec.kind;
        let children = spec.children;
        self.motion_declared |= paint.motion.is_some()
            || matches!(&kind, NodeKind::Box(content) if content.paint().motion.is_some());
        let focusable = matches!(kind, NodeKind::Text(ref t) if t.is_editable())
            || control.as_deref().is_some_and(|control| {
                matches!(
                    control,
                    crate::controls::Control::Slider(_)
                        | crate::controls::Control::Select(_)
                        | crate::controls::Control::Resize(_)
                )
            });
        let ctx = match kind {
            NodeKind::Text(ref t) => LeafCtx {
                text: Some(Box::new(LeafText {
                    text: t.display_text(),
                    font_size: t.font_size,
                    text_style: t.effective_text_style(),
                    spans: t.shaping_spans().to_vec(),
                    multiline: t.multiline(),
                    memo: None,
                })),
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
            control,
            kind,
            style,
            paint_slot: (paint != Paint::default()).then(|| Box::new(paint)),
            children: Vec::new(),
            parent: None,
            dirty_layout: true,
            dirty_paint: true,
            paint_rev: self.content_version + 1,
            focusable,
            tab_index: 0,
            keyboard_release_activation: false,
            description: None,
            debug_selector: None,
            hit_test_excluded: false,
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

    fn guard_hoisted_children(&mut self, parent: taffy::NodeId) {
        let guard = self
            .taffy
            .new_leaf_with_context(taffy::Style::default(), LeafCtx { text: None })
            .expect("taffy leaf insertion is infallible");
        if self.taffy.add_child(parent, guard).is_ok() {
            self.hoist_guards.push((parent, guard));
        } else {
            let _ = self.taffy.remove(guard);
        }
    }

    fn release_hoist_guards(&mut self) {
        for (_, guard) in std::mem::take(&mut self.hoist_guards) {
            let _ = self.taffy.remove(guard);
        }
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
        self.structure_changed();
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
        self.structure_changed();
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
            preserve_scroll_offset: false,
            control: None,
            children: Vec::new(),
        });
        if let Some(p) = parent {
            self.attach(p, id);
        } else if self.root.is_none() {
            self.structure_changed();
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
        self.structure_changed();
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
        let out_of_flow = rec.style.absolute.is_some();
        let previous_focus = self.focused;
        if self.focused == Some(id) {
            self.focused = focus_fallback;
        }
        if let Some(p) = rec.parent {
            if let (Some(&pt), Some(&ct)) = (self.tmap.get(&p.raw()), self.tmap.get(&id.raw())) {
                self.detach_child(pt, ct);
                if out_of_flow && self.taffy.children(pt).is_ok_and(|kids| kids.is_empty()) {
                    self.guard_hoisted_children(pt);
                }
            }
            self.structure_changed();
            if let Some(prec) = self.nodes.get_mut(p.raw()) {
                prec.children.retain(|c| *c != id);
            }
            // Ancestor geometry may depend on this child: propagate.
            self.mark_layout_dirty(p);
        } else if self.root == Some(id) {
            self.structure_changed();
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
            self.portal_roots.remove(&n);
            self.exiting.remove(&n);
            self.access_hidden.remove(&n);
            self.portal_order.retain(|node| *node != n);
            self.remove_interaction(n);
            self.control_router.remove_node(n);
            self.cancel_input_node(n);
            self.rects.remove(&n.raw());
            self.content.remove(&n.raw());
            self.actions.retain(|(raw, _), _| *raw != n.raw());
            self.prepaints.remove(&n.raw());
            self.observers.remove(&n.raw());
            self.bound_actions.retain(|(raw, _)| *raw != n.raw());
            let _ = self.nodes.remove(n.raw());
        }
        // A removed node can never fire: drop arms targeting dead nodes.
        self.press_captures
            .retain(|_, n| self.nodes.contains(n.raw()));
        if previous_focus != self.focused {
            self.focus_generation = self.focus_generation.wrapping_add(1);
            self.pending_keyboard_activation = None;
        }
        self.queue_interaction_focus(previous_focus, self.focused);
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
            NodeKind::Box(b) => b.paint().clone(),
            _ => rec.paint().clone(),
        })
    }

    /// Update appearance without invalidating layout or replacing node identity.
    /// Returns false for a dead node or an unchanged paint value.
    pub fn set_paint(&mut self, id: NodeId, paint: Paint) -> bool {
        self.motion_declared |= paint.motion.is_some();
        let Some(rec) = self.nodes.get_mut(id.raw()) else {
            return false;
        };
        let current = match &rec.kind {
            NodeKind::Box(b) => b.paint(),
            _ => rec.paint(),
        };
        if *current == paint {
            return false;
        }
        let border = |paint: &Paint| {
            paint.gpui_border.map(|border| border.widths).or_else(|| {
                paint.border.map(|border| {
                    paint
                        .border_widths
                        .unwrap_or_else(|| aqua_core::Insets::uniform(border.width))
                })
            })
        };
        let layout_changed = self.gpui_layout_scale.is_some() && border(current) != border(&paint);
        match &mut rec.kind {
            NodeKind::Box(b) => b.set_paint(paint),
            _ => {
                rec.paint_slot = (paint != Paint::default()).then(|| Box::new(paint));
            }
        }
        if layout_changed {
            self.mark_layout_dirty(id);
        }
        self.touch_paint(id);
        true
    }

    /// Register (or replace) the painter for a canvas node. Returns the
    /// painter id. The callback runs at emission with the committed bounds.
    pub fn set_canvas_painter(&mut self, id: NodeId, paint: CanvasPaintCallback) -> Option<u64> {
        self.set_registered_canvas_painter(id, CanvasPainter::Legacy(paint))
    }

    pub fn set_scroll_stable_canvas_painter(
        &mut self,
        id: NodeId,
        paint: CanvasPaintCallback,
    ) -> Option<u64> {
        self.set_registered_canvas_painter(id, CanvasPainter::ScrollStable(paint))
    }

    pub fn set_context_canvas_painter(
        &mut self,
        id: NodeId,
        paint: ContextCanvasPaintCallback,
    ) -> Option<u64> {
        self.set_registered_canvas_painter(id, CanvasPainter::Context(paint))
    }

    fn set_registered_canvas_painter(&mut self, id: NodeId, paint: CanvasPainter) -> Option<u64> {
        let rec = self.nodes.get(id.raw())?;
        let NodeKind::Canvas(c) = rec.kind else {
            return None;
        };
        let pid = c.painter.unwrap_or_else(|| {
            let p = self.next_painter;
            self.next_painter += 1;
            p
        });
        self.painters.insert(
            pid,
            RegisteredCanvasPainter {
                callback: paint,
                token: Rc::new(()),
            },
        );
        if let Some(rec) = self.nodes.get_mut(id.raw()) {
            if let NodeKind::Canvas(ref mut c) = rec.kind {
                c.painter = Some(pid);
            }
            rec.dirty_paint = true;
        }
        self.touch_paint(id);
        Some(pid)
    }

    pub(crate) fn clear_canvas_painter(&mut self, id: NodeId) {
        let painter = match self.nodes.get_mut(id.raw()) {
            Some(record) => match &mut record.kind {
                NodeKind::Canvas(canvas) => canvas.painter.take(),
                _ => None,
            },
            None => None,
        };
        if let Some(painter) = painter {
            self.painters.remove(&painter);
            self.touch_paint(id);
        }
    }

    pub fn set_canvas_prepaint(&mut self, id: NodeId, callback: PrepaintCallback) -> bool {
        if !self
            .get(id)
            .is_some_and(|node| matches!(node.kind, NodeKind::Canvas(_)))
        {
            return false;
        }
        self.prepaints.insert(id.raw(), callback);
        self.touch_paint(id);
        true
    }

    pub fn clear_canvas_prepaint(&mut self, id: NodeId) {
        self.prepaints.remove(&id.raw());
    }

    pub fn set_canvas_observer(&mut self, id: NodeId, callback: ObserveCallback) -> bool {
        if !self
            .get(id)
            .is_some_and(|node| matches!(node.kind, NodeKind::Canvas(_)))
        {
            return false;
        }
        self.observers.insert(id.raw(), callback);
        self.observer_order.get_mut().take();
        self.observed_version.set(None);
        true
    }

    pub fn clear_canvas_observer(&mut self, id: NodeId) {
        if self.observers.remove(&id.raw()).is_some() {
            self.observer_order.get_mut().take();
        }
    }

    pub fn run_observers(&self) {
        if self.observers.is_empty() || self.observed_version.get() == Some(self.content_version) {
            return;
        }
        self.observed_version.set(Some(self.content_version));
        let stale = self
            .observer_order
            .borrow()
            .as_ref()
            .is_none_or(|(epoch, _)| *epoch != self.structure_epoch);
        if stale {
            let ordered: Vec<NodeId> = self
                .traverse()
                .into_iter()
                .filter(|node| self.observers.contains_key(&node.raw()))
                .collect();
            *self.observer_order.borrow_mut() = Some((self.structure_epoch, ordered));
        }
        let ordered = self
            .observer_order
            .borrow()
            .as_ref()
            .map(|(_, nodes)| nodes.clone())
            .unwrap_or_default();
        for node in ordered {
            if let Some(callback) = self.observers.get(&node.raw()) {
                callback(self, node);
            }
        }
    }

    pub fn observe_clip(&self, node: NodeId) -> Rect {
        let viewport =
            Rect::from_origin_size(Point::ZERO, self.last_viewport.unwrap_or(Size::ZERO));
        let mut clip = viewport;
        let mut cursor = Some(node);
        while let Some(id) = cursor {
            let Some(record) = self.get(id) else {
                break;
            };
            let clips = matches!(&record.kind, NodeKind::Box(b) if b.paint().clip)
                || matches!(record.kind, NodeKind::Scroll(_))
                || record.paint().clip;
            if clips {
                if let Some(rect) = self.visual_rect(id) {
                    clip = clip.intersect(rect);
                }
            }
            if self.is_portal(id) {
                break;
            }
            cursor = record.parent;
        }
        clip
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
        if self
            .nodes
            .get(id.raw())
            .is_some_and(|rec| rec.style == style)
        {
            return true;
        }
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
        let (display, font_size, text_style, spans, multiline) = {
            let Some(rec) = self.nodes.get_mut(id.raw()) else {
                return false;
            };
            let NodeKind::Text(ref mut tc) = rec.kind else {
                return false;
            };
            tc.set_text(&text);
            rec.dirty_paint = true;
            (
                tc.display_text(),
                tc.font_size,
                tc.effective_text_style(),
                tc.shaping_spans().to_vec(),
                tc.multiline(),
            )
        };
        if let Some(ctx) = self.taffy.get_node_context_mut(t) {
            ctx.text = Some(Box::new(LeafText {
                text: display,
                font_size,
                text_style,
                spans,
                multiline,
                memo: None,
            }));
        }
        self.mark_layout_dirty(id);
        self.touch_paint(id);
        true
    }

    pub fn set_gpui_text_opacity(
        &mut self,
        id: NodeId,
        ranges: std::sync::Arc<[aqua_core::scene::GpuiTextOpacityRange]>,
    ) -> bool {
        let Some(rec) = self.nodes.get_mut(id.raw()) else {
            return false;
        };
        let NodeKind::Text(text) = &mut rec.kind else {
            return false;
        };
        if !valid_gpui_text_opacity(text.text(), &ranges) {
            return false;
        }
        if text.gpui_text_opacity == ranges {
            return true;
        }
        text.gpui_text_opacity = ranges;
        rec.dirty_paint = true;
        self.touch_paint(id);
        true
    }

    pub fn set_text_selection(&mut self, id: NodeId, selection: Option<Selection>) -> bool {
        let composing = self.get(id).is_some_and(|record| {
            matches!(&record.kind, NodeKind::Text(text) if matches!(&text.backing, TextBacking::Editable(editor) if editor.is_composing()))
        });
        if composing {
            return self
                .edit_text(id, |text| {
                    if let TextBacking::Editable(editor) = &mut text.backing {
                        editor.set_selection(
                            selection
                                .unwrap_or_else(|| Selection::caret_at(editor.selection().caret)),
                        );
                    }
                })
                .is_some();
        }
        let Some(record) = self.nodes.get_mut(id.raw()) else {
            return false;
        };
        let NodeKind::Text(text) = &mut record.kind else {
            return false;
        };
        match &mut text.backing {
            TextBacking::Editable(editor) => {
                editor.set_selection(
                    selection.unwrap_or_else(|| Selection::caret_at(editor.selection().caret)),
                );
            }
            TextBacking::Static(model) => {
                let boundary = |position: usize| {
                    let position = position.min(model.len_bytes());
                    if model.is_grapheme_boundary(position) {
                        position
                    } else {
                        model.prev_grapheme(position)
                    }
                };
                text.selection = selection.map(|selection| {
                    Selection::new(boundary(selection.anchor), boundary(selection.caret))
                });
            }
        }
        true
    }

    pub fn edit_editor<R>(
        &mut self,
        id: NodeId,
        edit: impl FnOnce(&mut TextEditor) -> R,
    ) -> Option<R> {
        let (result, content_changed) = {
            let record = self.nodes.get_mut(id.raw())?;
            let NodeKind::Text(text) = &mut record.kind else {
                return None;
            };
            let TextBacking::Editable(editor) = &mut text.backing else {
                return None;
            };
            let revision = editor.revision();
            let composing = editor.is_composing();
            let result = edit(editor);
            (
                result,
                revision != editor.revision() || composing || editor.is_composing(),
            )
        };
        if content_changed {
            self.sync_text_leaf(id);
            self.touch_paint(id);
        }
        Some(result)
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
            ctx.text = Some(Box::new(LeafText {
                text: tc.display_text(),
                font_size: tc.font_size,
                text_style: tc.effective_text_style(),
                spans: tc.shaping_spans().to_vec(),
                multiline: tc.multiline(),
                memo: None,
            }));
        }
        self.mark_layout_dirty(id);
        true
    }

    pub fn sync_text_leaf_if_changed(&mut self, id: NodeId) -> bool {
        let Some(&t) = self.tmap.get(&id.raw()) else {
            return false;
        };
        let unchanged = match (
            self.nodes.get(id.raw()).map(|rec| &rec.kind),
            self.taffy
                .get_node_context(t)
                .and_then(|ctx| ctx.text.as_ref()),
        ) {
            (Some(NodeKind::Text(tc)), Some(leaf)) => {
                leaf.multiline == tc.multiline()
                    && leaf.font_size == tc.font_size
                    && leaf.text == tc.display_text()
                    && leaf.text_style == tc.effective_text_style()
                    && leaf.spans.as_slice() == tc.shaping_spans()
            }
            _ => false,
        };
        if unchanged {
            return true;
        }
        self.sync_text_leaf(id)
    }

    /// Scroll clamping target: content size if measured, else the viewport
    /// (offset forced to zero).
    fn scroll_max(&self, id: NodeId) -> Point {
        let rect = self.rect(id).map(|r| r.size()).unwrap_or(Size::ZERO);
        let content = self.content_size(id).unwrap_or(Size::ZERO);
        Point::new((content.w - rect.w).max(0.0), (content.h - rect.h).max(0.0))
    }

    #[must_use]
    pub fn scroll_limit(&self, id: NodeId) -> Point {
        self.scroll_max(id)
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
                    if s.offset == clamped {
                        return false;
                    }
                    s.offset = clamped;
                    rec.dirty_paint = true;
                    true
                }
                _ => false,
            },
            None => false,
        };
        if applied {
            match self.scroll_stays_in_window(id, clamped) {
                true => {
                    self.scroll_patches += 1;
                    self.content_version += 1;
                    let rev = self.content_version;
                    if let Some(rec) = self.nodes.get_mut(id.raw()) {
                        rec.paint_rev = rev;
                    }
                }
                false => self.touch_paint(id),
            }
        }
        applied
    }

    fn scroll_stays_in_window(&self, id: NodeId, offset: Point) -> bool {
        let generation = self.emit_generation.get();
        self.scroll_state
            .borrow()
            .get(&id.raw())
            .is_some_and(|slot| {
                slot.generation == generation
                    && !slot.volatile
                    && rect_contains_rect(slot.window, slot.view.translate(offset.x, offset.y))
            })
    }

    #[must_use]
    pub fn scene_version(&self) -> u64 {
        self.content_version - self.scroll_patches
    }

    #[must_use]
    pub fn scroll_revision(&self) -> u64 {
        self.scroll_patches
    }

    #[must_use]
    pub fn scroll_translate_op(&self, node: NodeId) -> Option<u32> {
        let generation = self.emit_generation.get();
        self.scroll_state
            .borrow()
            .get(&node.raw())
            .filter(|slot| slot.generation == generation)
            .map(|slot| slot.op)
    }

    pub fn scroll_translations(&self) -> Vec<(NodeId, DisplayItem)> {
        let generation = self.emit_generation.get();
        self.scroll_state
            .borrow()
            .iter()
            .filter(|(_, slot)| slot.generation == generation)
            .filter_map(|(raw, _)| {
                let node = NodeId::from_raw(*raw);
                let offset = self.scroll_offset(node)?;
                Some((
                    node,
                    DisplayItem::PushTransform(Transform2D::translate(-offset.x, -offset.y)),
                ))
            })
            .collect()
    }

    fn collect_dirty_layout(&self, root: NodeId) -> HashSet<RawId> {
        let mut dirty = HashSet::new();
        let mut visited = 0usize;
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            visited += 1;
            let Some(&t) = self.tmap.get(&id.raw()) else {
                continue;
            };
            if !self.taffy.dirty(t).unwrap_or(true) {
                continue;
            }
            dirty.insert(id.raw());
            if let Some(rec) = self.nodes.get(id.raw()) {
                stack.extend(rec.children.iter().copied());
            }
        }
        self.layout_visited.set(self.layout_visited.get() + visited);
        dirty
    }

    fn sync_layout_styles(&mut self, dirty: &HashSet<RawId>) -> Result<(), String> {
        if self.gpui_layout_scale.is_none() && !self.layout_styles_dirty {
            return Ok(());
        }
        let full = self.layout_styles_dirty;
        for &id in dirty {
            let Some(record) = self.nodes.get(id) else {
                continue;
            };
            if !full && !record.dirty_layout {
                continue;
            }
            let Some(&node) = self.tmap.get(&id) else {
                continue;
            };
            let style = match self.gpui_layout_scale {
                Some(scale) => {
                    let mut style = crate::layout::editor::to_taffy(&record.style, scale);
                    let paint = match &record.kind {
                        NodeKind::Box(content) => content.paint(),
                        _ => record.paint(),
                    };
                    let widths = paint.gpui_border.map(|border| border.widths).or_else(|| {
                        paint.border.map(|border| {
                            paint
                                .border_widths
                                .unwrap_or_else(|| aqua_core::Insets::uniform(border.width))
                        })
                    });
                    if let Some(widths) = widths {
                        style.border = crate::layout::editor::border_to_taffy(widths, scale);
                    }
                    style
                }
                None => crate::style::to_taffy(&record.style),
            };
            if self
                .taffy
                .style(node)
                .map_err(|error| format!("taffy style failed: {error:?}"))?
                != &style
            {
                self.taffy
                    .set_style(node, style)
                    .map_err(|error| format!("taffy set_style failed: {error:?}"))?;
            }
        }
        self.layout_styles_dirty = false;
        Ok(())
    }

    fn commit_layout(
        &mut self,
        root: NodeId,
        dirty: &HashSet<RawId>,
        scale: Option<f32>,
        device_scale: f32,
    ) -> (usize, usize) {
        let mut committed = 0usize;
        let mut visited = 0usize;
        let mut stack = vec![(root, 0.0f32, 0.0f32)];
        while let Some((id, ox, oy)) = stack.pop() {
            visited += 1;
            let Some(&t) = self.tmap.get(&id.raw()) else {
                continue;
            };
            let Ok(layout) = self.taffy.layout(t) else {
                continue;
            };
            let x = ox + layout.location.x;
            let y = oy + layout.location.y;
            let rect = match scale {
                Some(scale) => crate::layout::editor::snap_absolute_bounds(
                    Point::new(x, y),
                    Size::new(layout.size.width, layout.size.height),
                    scale,
                ),
                None => Rect::from_origin_size(
                    Point::new(x, y),
                    Size::new(layout.size.width, layout.size.height),
                ),
            };
            let moved = self.rects.insert(id.raw(), rect) != Some(rect);
            let was_dirty = dirty.contains(&id.raw());
            if !moved && !was_dirty {
                continue;
            }
            committed += 1;
            let Some(rec) = self.nodes.get_mut(id.raw()) else {
                continue;
            };
            rec.dirty_layout = false;
            rec.dirty_paint = false;
            let is_scroll = matches!(rec.kind, NodeKind::Scroll(_));
            let children = rec.children.clone();
            if is_scroll {
                let mut extent = Size::ZERO;
                for c in &children {
                    if let Some(&ct) = self.tmap.get(&c.raw()) {
                        if let Ok(cl) = self.taffy.layout(ct) {
                            extent.w = extent.w.max(cl.location.x + cl.size.width);
                            extent.h = extent.h.max(cl.location.y + cl.size.height);
                        }
                    }
                }
                let extent = Size::new(extent.w / device_scale, extent.h / device_scale);
                self.content.insert(id.raw(), extent);
                if let NodeKind::Scroll(scroll) = &mut rec.kind {
                    scroll.offset.x = scroll.offset.x.clamp(0.0, (extent.w - rect.w).max(0.0));
                    scroll.offset.y = scroll.offset.y.clamp(0.0, (extent.h - rect.h).max(0.0));
                }
            }
            for c in children.into_iter().rev() {
                stack.push((c, x, y));
            }
        }
        (committed, visited)
    }

    /// Compute layout for `viewport` logical size, draining queued binding
    /// commands first. Skips Taffy entirely when the viewport and layout
    /// version are unchanged (paint-only updates cost no layout work).
    /// `measurer` supplies text metrics; production uses
    /// [`ShapedMeasurer`](crate::layout::ShapedMeasurer) over the shared
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
                nodes_visited: 0,
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
        let visited_before = self.layout_visited.get();
        let drained = self.sink.len();
        self.drain_commands();
        let dirty = self.collect_dirty_layout(root);
        self.sync_layout_styles(&dirty)?;
        let measured = std::cell::Cell::new(0usize);
        let scale = self.gpui_layout_scale;
        let device_scale = scale.unwrap_or(1.0);
        let epoch = measurer.memo_epoch();
        self.taffy
            .compute_layout_with_measure(
                troot,
                taffy::geometry::Size {
                    width: AvailableSpace::Definite(viewport.w * device_scale),
                    height: AvailableSpace::Definite(viewport.h * device_scale),
                },
                |input: LayoutInput, _node, ctx, style| {
                    measured.set(measured.get() + 1);
                    aqua_core::checkpoint::reach();
                    taffy::compute_leaf_layout(
                        input,
                        style,
                        |_, _| 0.0,
                        |known_dimensions, available_space| match scale {
                            Some(scale) => measure_leaf_gpui(
                                known_dimensions,
                                available_space,
                                ctx,
                                measurer,
                                scale,
                                epoch,
                            ),
                            None => measure_leaf(
                                known_dimensions,
                                available_space,
                                ctx,
                                measurer,
                                epoch,
                            ),
                        },
                    )
                },
            )
            .map_err(|e| format!("taffy layout failed: {e:?}"))?;
        self.release_hoist_guards();
        let nodes_measured = measured.get();
        let (committed, commit_visited) = self.commit_layout(root, &dirty, scale, device_scale);
        self.layout_visited
            .set(self.layout_visited.get() + commit_visited);
        self.committed_layout = self.layout_version;
        self.last_viewport = Some(viewport);
        self.last_stats = FrameStats {
            layout_skipped: false,
            commands_drained: drained,
            nodes_measured,
            nodes_traversed: committed,
            nodes_visited: self.layout_visited.get() - visited_before,
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
    pub fn run_deferred_canvas_actions(&mut self) -> usize {
        let actions = std::mem::take(&mut *self.deferred_canvas_actions.borrow_mut());
        let mut delivered = 0;
        for item in actions {
            let live = self.get(item.node).is_some_and(|record| {
                matches!(&record.kind, NodeKind::Canvas(canvas) if canvas.painter == Some(item.painter))
            }) && self.painters.get(&item.painter).is_some_and(|painter| Rc::ptr_eq(&painter.token, &item.token));
            if live {
                (item.action)();
                delivered += 1;
            }
        }
        delivered
    }

    pub fn emit_scene(&mut self) -> Vec<(NodeId, DisplayItem)> {
        for node in self.traverse() {
            if let Some(callback) = self.prepaints.get(&node.raw()) {
                callback(self, node);
            }
        }
        let mut out = Vec::new();
        let generation = self.emit_generation.get() + 1;
        self.emit_generation.set(generation);
        if let Some(root) = self.root {
            self.emit_node(
                root,
                Point::ZERO,
                None,
                None,
                Transform2D::IDENTITY,
                &mut out,
            );
        }
        for portal in &self.portal_order {
            self.emit_node(
                *portal,
                Point::ZERO,
                None,
                None,
                Transform2D::IDENTITY,
                &mut out,
            );
        }
        self.scroll_state
            .borrow_mut()
            .retain(|_, slot| slot.generation == generation);
        self.last_stats.scene_ops = out.len();
        self.last_stats.content_version = self.content_version;
        out
    }

    fn emit_node(
        &self,
        id: NodeId,
        scrolled: Point,
        clip: Option<Rect>,
        cull: Option<Rect>,
        inherited_transform: Transform2D,
        out: &mut Vec<(NodeId, DisplayItem)>,
    ) {
        let Some(rec) = self.nodes.get(id.raw()) else {
            return;
        };
        self.visited.set(self.visited.get() + 1);
        let Some(rect) = self.rects.get(&id.raw()).copied() else {
            return;
        };
        let paint = match &rec.kind {
            NodeKind::Box(content) => content.paint().clone(),
            _ => rec.paint().clone(),
        };
        let forced = self.motion_groups.get(&id).copied().unwrap_or_default();
        let group_transform = paint
            .transform
            .or(forced.translate.then_some(Transform2D::IDENTITY));
        let group_opacity = paint.opacity.or(forced.opacity.then_some(1.0));
        let transform = group_transform.map(|transform| {
            Transform2D::translate(-rect.x, -rect.y)
                .then(transform)
                .then(Transform2D::translate(rect.x, rect.y))
        });
        let world = transform.map_or(inherited_transform, |transform| {
            transform.then(inherited_transform)
        });
        let visual_rect = rect.transform(world);
        let footprint = paint
            .shadows
            .iter()
            .fold(rect, |bounds, shadow| bounds.union(shadow.bounds(rect)));
        let footprint = paint.gpui_shadows.iter().fold(footprint, |bounds, shadow| {
            bounds.union(shadow.bounds(rect))
        });
        let footprint = if let Some(shadow) = paint.shadow {
            let pad = 3.0 * shadow.blur.max(0.0);
            footprint.union(Rect::from_edges(
                rect.x + shadow.dx - pad,
                rect.y + shadow.dy - pad,
                rect.right() + shadow.dx + pad,
                rect.bottom() + shadow.dy + pad,
            ))
        } else {
            footprint
        };
        let visual_footprint = footprint.transform(world);
        if cull.is_some_and(|cull| cull.intersect(visual_footprint).is_empty())
            && Some(id) != self.root
            && !paint.shadows.iter().any(|layer| !layer.inset)
            && !paint.gpui_shadows.iter().any(|layer| !layer.inset)
            && paint.shadow.is_none()
            && !matches!(&rec.kind, NodeKind::Canvas(_))
            && (paint.clip || matches!(&rec.kind, NodeKind::Scroll(_)) || rec.children.is_empty())
        {
            return;
        }
        let start = out.len();
        if let Some(transform) = transform {
            out.push((id, DisplayItem::PushTransform(transform)));
        }
        if let Some(pulse) = paint.gpui_quad_pulse {
            out.push((id, DisplayItem::PushGpuiQuadPulse(pulse)));
        }
        if let Some(radius) = paint.content_blur {
            out.push((
                id,
                DisplayItem::PushContentBlur {
                    bounds: rect,
                    radius,
                },
            ));
        }
        if let Some(map) = paint.text_color_map {
            out.push((id, DisplayItem::PushTextColorMap(map)));
        }
        if let Some(map) = paint.gpui_text_color_map {
            out.push((id, DisplayItem::PushGpuiTextColorMap(map)));
        }
        if let Some(shimmer) = paint.gpui_text_shimmer {
            out.push((id, DisplayItem::PushGpuiTextShimmer(shimmer)));
        }
        if let Some(opacity) = group_opacity {
            out.push((id, DisplayItem::PushOpacity(opacity)));
        }
        let corners = paint
            .corners
            .unwrap_or_else(|| aqua_core::geometry::CornerRadii::uniform(paint.radius));
        for layer in paint.shadows.iter().filter(|layer| !layer.inset) {
            out.push((
                id,
                DisplayItem::Shadow {
                    bounds: rect,
                    corners,
                    layer: *layer,
                },
            ));
        }
        for layer in paint.gpui_shadows.iter().filter(|layer| !layer.inset) {
            out.push((
                id,
                DisplayItem::GpuiShadow {
                    bounds: rect,
                    corners,
                    layer: *layer,
                },
            ));
        }
        if let Some(background) = paint.gpui_background {
            out.push((
                id,
                DisplayItem::GpuiBackground {
                    bounds: rect,
                    corners,
                    background,
                },
            ));
        }
        let shaped = paint.gpui_background.is_some()
            || paint.gpui_border.is_some()
            || !paint.gpui_shadows.is_empty()
            || self.gpui_paint
            || corners.top_left != corners.top_right
            || corners.top_left != corners.bottom_right
            || corners.top_left != corners.bottom_left
            || paint.border_widths.is_some()
            || paint.border_dashed;
        if shaped {
            if let Some(shadow) = paint.shadow {
                out.push((
                    id,
                    DisplayItem::Rect {
                        bounds: rect,
                        radius: corners.top_left,
                        fill: Oklch::TRANSPARENT,
                        border_width: 0.0,
                        border_color: Oklch::TRANSPARENT,
                        shadow: Some((shadow.dx, shadow.dy, shadow.blur, shadow.color)),
                    },
                ));
            }
            if let Some(fill) = paint.fill {
                out.push((
                    id,
                    DisplayItem::Quad {
                        bounds: rect,
                        corners,
                        fill,
                        border_widths: aqua_core::Insets::uniform(0.0),
                        border_color: Oklch::TRANSPARENT,
                        dashed: false,
                    },
                ));
            }
        } else {
            if paint.fill.is_some() || paint.border.is_some() || paint.shadow.is_some() {
                out.push((
                    id,
                    DisplayItem::Rect {
                        bounds: rect,
                        radius: paint.corners.map_or(paint.radius, |r| r.top_left),
                        fill: paint.fill.unwrap_or(Oklch::TRANSPARENT),
                        border_width: if paint.shadows.is_empty() {
                            paint.border.map_or(0.0, |x| x.width)
                        } else {
                            0.0
                        },
                        border_color: paint.border.map_or(Oklch::TRANSPARENT, |x| x.color),
                        shadow: paint.shadow.map(|s| (s.dx, s.dy, s.blur, s.color)),
                    },
                ));
            }
        }
        for layer in paint.shadows.iter().filter(|layer| layer.inset) {
            out.push((
                id,
                DisplayItem::Shadow {
                    bounds: rect,
                    corners,
                    layer: *layer,
                },
            ));
        }
        for layer in paint.gpui_shadows.iter().filter(|layer| layer.inset) {
            out.push((
                id,
                DisplayItem::GpuiShadow {
                    bounds: rect,
                    corners,
                    layer: *layer,
                },
            ));
        }
        let clips = paint.clip || matches!(&rec.kind, NodeKind::Scroll(_));
        let child_clip = if clips {
            Some(clip.map_or(visual_rect, |c| c.intersect(visual_rect)))
        } else {
            clip
        };
        let scroll_offset = match &rec.kind {
            NodeKind::Scroll(s) => Some(s.offset),
            _ => None,
        };
        let (child_world, child_cull) = match scroll_offset {
            Some(scroll) => {
                let child_world = Transform2D::translate(-scroll.x, -scroll.y).then(world);
                let visible = rect.translate(scroll.x, scroll.y);
                let overscan = (rect.h * 0.5).clamp(SCROLL_OVERSCAN_MIN, SCROLL_OVERSCAN_MAX);
                let generation = self.emit_generation.get();
                let window = {
                    let mut slots = self.scroll_state.borrow_mut();
                    let window = slots
                        .get(&id.raw())
                        .map(|slot| slot.window)
                        .filter(|window| rect_contains_rect(*window, visible))
                        .unwrap_or_else(|| visible.inflate(overscan));
                    slots.insert(
                        id.raw(),
                        ScrollSlot {
                            view: rect,
                            window,
                            op: 0,
                            volatile: false,
                            generation,
                        },
                    );
                    window
                };
                (child_world, Some(window.transform(child_world)))
            }
            None => (
                world,
                match clips {
                    true => Some(cull.map_or(visual_rect, |c| c.intersect(visual_rect))),
                    false => cull,
                },
            ),
        };
        if clips {
            out.push((
                id,
                if self.gpui_paint {
                    let widths = paint
                        .gpui_border
                        .filter(|border| border.color.a > 0.0)
                        .map(|border| border.widths)
                        .or_else(|| {
                            paint
                                .border
                                .filter(|border| border.color.alpha > 0.0)
                                .map(|border| {
                                    paint
                                        .border_widths
                                        .unwrap_or_else(|| aqua_core::Insets::uniform(border.width))
                                })
                        })
                        .unwrap_or_default();
                    DisplayItem::PushGpuiContentMask {
                        bounds: Rect::from_edges(
                            rect.x + widths.left,
                            rect.y + widths.top,
                            rect.right() - widths.right,
                            rect.bottom() - widths.bottom,
                        ),
                    }
                } else if shaped {
                    DisplayItem::PushRoundedClip {
                        bounds: rect,
                        corners,
                    }
                } else {
                    DisplayItem::PushClip {
                        bounds: rect,
                        radius: corners.top_left,
                    }
                },
            ));
        }
        if let Some(scroll) = scroll_offset {
            if let Some(slot) = self.scroll_state.borrow_mut().get_mut(&id.raw()) {
                slot.op = (out.len() - start) as u32;
            }
            out.push((
                id,
                DisplayItem::PushTransform(Transform2D::translate(-scroll.x, -scroll.y)),
            ));
        }
        match &rec.kind {
            NodeKind::Box(_) | NodeKind::Scroll(_) => {}
            NodeKind::Text(t) => {
                let raw_color = if t.is_placeholder() {
                    t.gpui_placeholder_color.or_else(|| {
                        if t.placeholder_color.is_some()
                            || t.gpui_source_placeholder_color.is_some()
                        {
                            None
                        } else {
                            t.text_style
                                .gpui_color
                                .map(|[h, s, l, a]| aqua_core::GpuiHsla { h, s, l, a })
                        }
                    })
                } else {
                    t.text_style
                        .gpui_color
                        .map(|[h, s, l, a]| aqua_core::GpuiHsla { h, s, l, a })
                };
                let source_color = if t.is_placeholder() {
                    t.gpui_source_placeholder_color.or_else(|| {
                        if t.placeholder_color.is_some() || t.gpui_placeholder_color.is_some() {
                            None
                        } else {
                            t.text_style.gpui_source_color.map(
                                |[hue_degrees, saturation, lightness, alpha]| {
                                    aqua_core::GpuiTextColor {
                                        hue_degrees,
                                        saturation,
                                        lightness,
                                        alpha,
                                    }
                                },
                            )
                        }
                    })
                } else {
                    t.text_style.gpui_source_color.map(
                        |[hue_degrees, saturation, lightness, alpha]| aqua_core::GpuiTextColor {
                            hue_degrees,
                            saturation,
                            lightness,
                            alpha,
                        },
                    )
                };
                if let Some(color) = raw_color {
                    out.push((id, DisplayItem::PushGpuiTextColor(color)));
                }
                if let Some(color) = source_color {
                    out.push((id, DisplayItem::PushGpuiSourceTextColor(color)));
                }
                if !t.is_placeholder() && !t.gpui_text_opacity.is_empty() {
                    out.push((
                        id,
                        DisplayItem::PushGpuiTextOpacity(t.gpui_text_opacity.clone()),
                    ));
                }
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
                if !t.is_placeholder() && !t.gpui_text_opacity.is_empty() {
                    out.push((id, DisplayItem::PopGpuiTextOpacity));
                }
                if source_color.is_some() {
                    out.push((id, DisplayItem::PopGpuiSourceTextColor));
                }
                if raw_color.is_some() {
                    out.push((id, DisplayItem::PopGpuiTextColor));
                }
            }
            NodeKind::Media(m) => {
                if let Some(image) = &m.image {
                    let nonuniform = self.gpui_paint
                        || corners.top_left != corners.top_right
                        || corners.top_left != corners.bottom_right
                        || corners.top_left != corners.bottom_left;
                    if nonuniform {
                        out.push((
                            id,
                            DisplayItem::PushRoundedClip {
                                bounds: rect,
                                corners,
                            },
                        ));
                    }
                    out.push((
                        id,
                        DisplayItem::Image {
                            bounds: rect,
                            layer: media_layer(image.id()),
                            fit: m.fit,
                            position: m.position,
                            source: m.source,
                            opacity: m.opacity,
                            rounded_clip: if nonuniform { 0.0 } else { corners.top_left },
                        },
                    ));
                    if nonuniform {
                        out.push((id, DisplayItem::PopClip));
                    }
                }
            }
            NodeKind::Canvas(c) => {
                if let Some((pid, painter)) = c
                    .painter
                    .and_then(|pid| self.painters.get(&pid).map(|painter| (pid, painter)))
                {
                    let ops = match &painter.callback {
                        CanvasPainter::Legacy(paint) => {
                            paint(rect.translate(-scrolled.x, -scrolled.y))
                        }
                        CanvasPainter::ScrollStable(paint) => paint(rect),
                        CanvasPainter::Context(paint) => {
                            let corners = [
                                rect.origin(),
                                Point::new(rect.right(), rect.y),
                                Point::new(rect.x, rect.bottom()),
                                Point::new(rect.right(), rect.bottom()),
                            ]
                            .map(|point| world.apply(point));
                            let min_x = corners
                                .iter()
                                .map(|point| point.x)
                                .fold(f32::INFINITY, f32::min);
                            let max_x = corners
                                .iter()
                                .map(|point| point.x)
                                .fold(f32::NEG_INFINITY, f32::max);
                            let min_y = corners
                                .iter()
                                .map(|point| point.y)
                                .fold(f32::INFINITY, f32::min);
                            let max_y = corners
                                .iter()
                                .map(|point| point.y)
                                .fold(f32::NEG_INFINITY, f32::max);
                            let bounds = Rect::from_edges(min_x, min_y, max_x, max_y);
                            let viewport = Rect::from_origin_size(
                                Point::ZERO,
                                self.last_viewport.unwrap_or(Size::ZERO),
                            );
                            let clip =
                                child_clip.map_or(viewport, |active| viewport.intersect(active));
                            paint(CanvasPaintContext {
                                bounds,
                                clip,
                                node: id,
                                painter: pid,
                                token: painter.token.clone(),
                                deferred: &self.deferred_canvas_actions,
                            })
                        }
                    };
                    let scroll_canceled = scrolled != Point::ZERO
                        && !matches!(painter.callback, CanvasPainter::ScrollStable(_));
                    if scroll_canceled {
                        for active in self.scroll_active.borrow().iter() {
                            if let Some(slot) = self.scroll_state.borrow_mut().get_mut(active) {
                                slot.volatile = true;
                            }
                        }
                    }
                    let cancel = scroll_canceled && !ops.is_empty();
                    if cancel {
                        out.push((
                            id,
                            DisplayItem::PushTransform(Transform2D::translate(
                                scrolled.x, scrolled.y,
                            )),
                        ));
                    }
                    out.extend(ops.into_iter().map(|op| (id, op)));
                    if cancel {
                        out.push((id, DisplayItem::PopTransform));
                    }
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
        let child_scrolled = match scroll_offset {
            Some(scroll) => Point::new(scrolled.x + scroll.x, scrolled.y + scroll.y),
            None => scrolled,
        };
        if scroll_offset.is_some() {
            self.scroll_active.borrow_mut().push(id.raw());
        }
        for &child in &rec.children {
            if !self.is_portal(child) {
                self.emit_node(
                    child,
                    child_scrolled,
                    child_clip,
                    child_cull,
                    child_world,
                    out,
                );
            }
        }
        if scroll_offset.is_some() {
            self.scroll_active.borrow_mut().pop();
            out.push((id, DisplayItem::PopTransform));
        }
        if clips {
            out.push((id, DisplayItem::PopClip));
        }
        if shaped {
            if let Some(border) = paint.gpui_border {
                out.push((
                    id,
                    DisplayItem::GpuiBorder {
                        bounds: rect,
                        corners,
                        widths: border.widths,
                        color: border.color,
                        dashed: border.dashed,
                    },
                ));
            }
            if let Some(border) = paint.border {
                out.push((
                    id,
                    DisplayItem::Quad {
                        bounds: rect,
                        corners,
                        fill: Oklch::TRANSPARENT,
                        border_widths: paint
                            .border_widths
                            .unwrap_or_else(|| aqua_core::Insets::uniform(border.width)),
                        border_color: border.color,
                        dashed: paint.border_dashed,
                    },
                ));
            }
        } else if !paint.shadows.is_empty() {
            if let Some(border) = paint.border {
                out.push((
                    id,
                    DisplayItem::Rect {
                        bounds: rect,
                        radius: paint.corners.map_or(paint.radius, |r| r.top_left),
                        fill: Oklch::TRANSPARENT,
                        border_width: border.width,
                        border_color: border.color,
                        shadow: None,
                    },
                ));
            }
        }
        if self.focused == Some(id) && !self.is_disabled(id) {
            if let Some(ring) = paint.focus_ring {
                out.push((
                    id,
                    if self.gpui_paint {
                        DisplayItem::Quad {
                            bounds: rect,
                            corners,
                            fill: Oklch::TRANSPARENT,
                            border_widths: aqua_core::Insets::uniform(ring.width),
                            border_color: ring.color,
                            dashed: false,
                        }
                    } else {
                        DisplayItem::Rect {
                            bounds: rect,
                            radius: paint.radius,
                            fill: Oklch::TRANSPARENT,
                            border_width: ring.width,
                            border_color: ring.color,
                            shadow: None,
                        }
                    },
                ));
            }
        }
        if group_opacity.is_some() {
            out.push((id, DisplayItem::PopOpacity));
        }
        if paint.text_color_map.is_some() {
            out.push((id, DisplayItem::PopTextColorMap));
        }
        if paint.gpui_text_shimmer.is_some() {
            out.push((id, DisplayItem::PopGpuiTextColorMap));
        }
        if paint.gpui_text_color_map.is_some() {
            out.push((id, DisplayItem::PopGpuiTextColorMap));
        }
        if paint.content_blur.is_some() {
            out.push((id, DisplayItem::PopContentBlur));
        }
        if paint.gpui_quad_pulse.is_some() {
            out.push((id, DisplayItem::PopGpuiQuadPulse));
        }
        if transform.is_some() {
            out.push((id, DisplayItem::PopTransform));
        }
    }

    fn run_id_for(&self, id: NodeId) -> u64 {
        // Stable per node: index + generation folded into one u64.
        // NOTE: interim scheme until the text RunRegistry mints runs; the
        // revision contract (bump on every content change) already holds.
        ((id.raw().index as u64) << 32) | id.raw().generation as u64
    }

    fn structure_changed(&mut self) {
        self.structure_epoch += 1;
        self.key_index.get_mut().take();
    }

    pub fn structure_epoch(&self) -> u64 {
        self.structure_epoch
    }

    fn with_key_slot<R>(&self, key: KeyHash, read: impl FnOnce(Option<&KeySlot>) -> R) -> R {
        let mut index = self.key_index.borrow_mut();
        let map = index.get_or_insert_with(|| {
            let ids = self.traverse();
            let mut map: HashMap<KeyHash, KeySlot> = HashMap::with_capacity(ids.len() / 4);
            for id in ids {
                if let Some(key) = self.nodes.get(id.raw()).and_then(|record| record.key) {
                    match map.entry(key) {
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(KeySlot::One(id));
                        }
                        std::collections::hash_map::Entry::Occupied(mut entry) => {
                            entry.get_mut().push(id);
                        }
                    }
                }
            }
            map
        });
        read(map.get(&key))
    }

    pub fn nodes_with_key(&self, key: KeyHash) -> Vec<NodeId> {
        self.with_key_slot(key, |slot| slot.map_or_else(Vec::new, KeySlot::to_vec))
    }

    pub fn last_node_with_key(&self, key: KeyHash) -> Option<NodeId> {
        self.with_key_slot(key, |slot| slot.map(KeySlot::last))
    }

    pub fn sole_node_with_key(&self, key: KeyHash) -> Option<NodeId> {
        self.with_key_slot(key, |slot| match slot {
            Some(KeySlot::One(id)) => Some(*id),
            _ => None,
        })
    }

    pub fn find_by_key(&self, key: KeyHash) -> Option<NodeId> {
        self.with_key_slot(key, |slot| slot.map(KeySlot::first))
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
        self.visited.set(self.visited.get() + out.len());
        out
    }
}

fn measure_leaf_gpui(
    known_dimensions: taffy::geometry::Size<Option<f32>>,
    available_space: taffy::geometry::Size<AvailableSpace>,
    ctx: Option<&mut LeafCtx>,
    measurer: &dyn TextMeasurer,
    scale: f32,
    epoch: Option<u64>,
) -> taffy::geometry::Size<f32> {
    let available = |value| match value {
        AvailableSpace::Definite(value) => AvailableSpace::Definite(value / scale),
        other => other,
    };
    let logical = measure_leaf(
        taffy::geometry::Size {
            width: known_dimensions.width.map(|value| value / scale),
            height: known_dimensions.height.map(|value| value / scale),
        },
        taffy::geometry::Size {
            width: available(available_space.width),
            height: available(available_space.height),
        },
        ctx,
        measurer,
        epoch,
    );
    let device = crate::layout::editor::snap_measured_size_to_device_pixels(
        Size::new(logical.width, logical.height),
        scale,
    );
    taffy::geometry::Size {
        width: device.w,
        height: device.h,
    }
}

fn measure_leaf(
    known_dimensions: taffy::geometry::Size<Option<f32>>,
    available_space: taffy::geometry::Size<AvailableSpace>,
    ctx: Option<&mut LeafCtx>,
    measurer: &dyn TextMeasurer,
    epoch: Option<u64>,
) -> taffy::geometry::Size<f32> {
    let mut size = taffy::geometry::Size {
        width: known_dimensions.width.unwrap_or(0.0),
        height: known_dimensions.height.unwrap_or(0.0),
    };
    if let Some(LeafCtx { text: Some(leaf) }) = ctx {
        let LeafText {
            text,
            font_size: _,
            text_style,
            spans,
            multiline,
            memo,
        } = leaf.as_mut();
        let max_width = if *multiline {
            match available_space.width {
                AvailableSpace::Definite(w) => Some(w),
                AvailableSpace::MinContent => Some(0.0),
                AvailableSpace::MaxContent => None,
            }
        } else {
            None
        };
        let max_width_bits = max_width.map_or(u32::MAX, f32::to_bits);
        let remembered = epoch.and_then(|epoch| {
            (*memo).filter(|memo| memo.epoch == epoch && memo.max_width_bits == max_width_bits)
        });
        let measured = match remembered {
            Some(memo) => memo.size,
            None => {
                let measured =
                    measurer.measure_styled(text, text_style, spans, max_width, *multiline);
                if let Some(epoch) = epoch {
                    *memo = Some(MeasureMemo {
                        epoch,
                        max_width_bits,
                        size: measured,
                    });
                }
                measured
            }
        };
        if known_dimensions.width.is_none() {
            size.width = measured.w;
        }
        if known_dimensions.height.is_none() {
            size.height = measured.h;
        }
    }
    size
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::NaiveMeasurer;
    use crate::style::{Axis, LayoutStyle, Length};

    #[test]
    fn every_primitive_paints_without_wrapper_nodes() {
        let kinds = [
            NodeKind::text(TextContent::label("painted")),
            NodeKind::Media(MediaContent::default()),
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
                NodeKind::text(TextContent::editable("draft", false)),
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
        assert!(tree.set_paint(id, paint.clone()));
        assert!(!tree.get(id).unwrap().dirty_layout);
        assert!(tree.content_version() > version);
        let version = tree.content_version();
        assert!(!tree.set_paint(id, paint.clone()));
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
                NodeKind::text(TextContent::label("same")),
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
                NodeKind::text(TextContent::label(label)),
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
            NodeKind::text(TextContent::label("hi")),
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
                NodeKind::text(TextContent::editable("first", false)),
                LayoutStyle::default(),
            )
            .keyed("a"),
            NodeSpec::new(
                NodeKind::text(TextContent::editable("second", false)),
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
                    NodeKind::text(TextContent::editable("", false)),
                    LayoutStyle::default(),
                )
                .keyed("b"),
                NodeSpec::new(
                    NodeKind::text(TextContent::editable("", false)),
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
            NodeKind::text(TextContent::label("x")),
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
            NodeKind::text(TextContent::label("a")),
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
            source: None,
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
                    NodeKind::text(TextContent::label("hi")),
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
                    let mut paint = b.paint().clone();
                    paint.fill = Some(Oklch::from_hex("#123456").unwrap());
                    b.set_paint(paint);
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
                NodeKind::text(TextContent::label("hello")),
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
                collection: Some(Box::new(CollectionSemantics {
                    set_size: Some(100),
                    position_in_set: Some(1),
                    ..Default::default()
                })),
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
                collection: Some(Box::new(CollectionSemantics {
                    set_size: Some(100),
                    position_in_set: Some(42),
                    ..Default::default()
                })),
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
                    NodeKind::text(TextContent::label("row")),
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
            NodeKind::text(TextContent::editable("draft", false)),
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
                NodeKind::text(TextContent::label("nested")),
                LayoutStyle::default(),
            ));
        let ids = tree.sync_children(root, vec![child]);
        assert_eq!(tree.get(ids[0]).unwrap().parent, Some(root));
        let grandchild = tree.get(ids[0]).unwrap().children[0];
        assert_eq!(tree.get(grandchild).unwrap().parent, Some(ids[0]));
    }
}
