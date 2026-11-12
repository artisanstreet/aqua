use aqua_protocol::probe::{self, Metric, Phase};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

mod access;
mod incremental;
mod interaction;
mod keyed_focus;
mod machine;
mod overlays;
mod paint;
mod render_motion;
mod wheel;
pub use machine::GpuState;
pub use overlays::{PopupError, PopupFocus, PopupOptions};

use crate::emitter::emit_scene_diff;

use interaction::TextInteraction;

use aqua_core::{
    DisplayItem, InputEvent as CoreInputEvent, NodeId, Oklch, Point, Rect, Size, TextRun,
};
use aqua_protocol::{
    A11yAction, A11yActionData, A11yActionRequest, A11yNode, DragRegions, FrameReason,
};
use aqua_reactive::DepTuple;
use aqua_reactive::{Runtime as ReactiveRuntime, Scope};

use aqua_scene::Scene;
use aqua_text::{
    CaretAffinity, CaretPosition, LineMode, Selection, SharedFonts, SharedRuns, StyledSpan,
    TextLayout, TextStyle, WrapMode,
};
use aqua_ui::{
    binding::{MountError, View},
    element::Element,
    events,
    layout::ProfiledMeasurer,
    motion::{ReducedMotion, Seconds},
    overlay_host::OverlayHost,
    tree::{ActionEvent, ActionKind, NodeKind, TextBacking, TextContent, UiTree},
};

#[derive(Debug, Clone, PartialEq)]
pub struct InputDispatchOutcome {
    pub consumed: bool,
    pub redraw: FrameReason,
}

fn text_caret_position(text: &TextContent) -> Option<CaretPosition> {
    match &text.backing {
        TextBacking::Editable(editor) if !editor.is_composing() => Some(editor.caret_position()),
        _ => text.caret().map(|byte| CaretPosition {
            byte,
            affinity: CaretAffinity::Downstream,
        }),
    }
}

#[derive(Debug)]
pub enum RuntimeError {
    NoViewMounted,
    Disposed,
    AlreadyMounted,
    Build(String),
    Mount(MountError),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeError::Disposed => write!(f, "runtime is disposed"),
            RuntimeError::NoViewMounted => write!(f, "no view mounted: call mount() first"),
            RuntimeError::AlreadyMounted => {
                write!(f, "view already mounted: call refresh() or dispose() first")
            }
            RuntimeError::Build(e) => write!(f, "ui build failed: {e}"),
            RuntimeError::Mount(e) => write!(f, "ui mount failed: {e}"),
        }
    }
}

impl std::error::Error for RuntimeError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum SceneKey {
    Tree { node: NodeId, op: u32 },
    TextDecoration { node: NodeId, index: u32 },
    TextBackground { node: NodeId, index: u32, part: u32 },
    TextInlineGlyph { node: NodeId, index: u32 },
    TextInlinePaint { node: NodeId, index: u32, part: u8 },
    Caret { node: NodeId },
    Selection { node: NodeId, index: u32 },
    ImeUnderline { node: NodeId, index: u32 },
    TextClipStart { node: NodeId },
    TextClipEnd { node: NodeId },
    CaretClipStart { node: NodeId },
    CaretClipEnd { node: NodeId },
    CaretOpacityStart { node: NodeId },
    CaretOpacityEnd { node: NodeId },
    ScrollOffset { node: NodeId },
    ScrollContentStart { node: NodeId },
    ScrollContentEnd { node: NodeId },
    PortalStart { node: NodeId, slot: u8 },
    PortalEnd { node: NodeId, slot: u8 },
}

impl SceneKey {
    fn node(self) -> NodeId {
        match self {
            Self::Tree { node, .. }
            | Self::TextDecoration { node, .. }
            | Self::TextBackground { node, .. }
            | Self::TextInlineGlyph { node, .. }
            | Self::TextInlinePaint { node, .. }
            | Self::Caret { node }
            | Self::Selection { node, .. }
            | Self::ImeUnderline { node, .. }
            | Self::TextClipStart { node }
            | Self::TextClipEnd { node }
            | Self::CaretClipStart { node }
            | Self::CaretClipEnd { node }
            | Self::CaretOpacityStart { node }
            | Self::CaretOpacityEnd { node }
            | Self::ScrollOffset { node }
            | Self::ScrollContentStart { node }
            | Self::ScrollContentEnd { node }
            | Self::PortalStart { node, .. }
            | Self::PortalEnd { node, .. } => node,
        }
    }
}

struct TextJob {
    node: NodeId,
    text: String,
    style: TextStyle,
    source_style: TextStyle,
    spans: Vec<StyledSpan>,
    wrap: bool,
    multiline: bool,
    masked: bool,
    overflow: Option<aqua_text::overflow::OverflowMode>,
    width: f32,
    paint_revision: u64,
}

struct TextEntry {
    fonts_revision: u64,
    source_style: TextStyle,
    spans: Vec<StyledSpan>,
    run: aqua_text::RunId,
    registry_rev: u64,
    text: String,
    paint_revision: u64,
    width_bits: u32,
    scale_bits: u32,
    gpui_wrap: bool,
    size_bits: u32,
    wrap: bool,
    line_multi: bool,
    masked: bool,
    overflow: Option<aqua_text::overflow::OverflowMode>,
    scroll_x: f32,
    access_runs: RefCell<Option<access::AccessRuns>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextFrontier {
    pub content_origin: Point,
    pub line_bottom: f32,
    pub block_height: f32,
}

pub struct NativeTextService {
    model: Rc<dyn aqua_ui::tree::NativeTextModel>,
    revision: Option<u64>,
    runs: SharedRuns,
    run: aqua_text::RunId,
    run_revision: u64,
    transform: aqua_core::Transform2D,
    padding: aqua_core::Insets,
    scroll_x: f32,
    scale: f32,
}

impl NativeTextService {
    pub fn selected_bounds(&self) -> Option<Rect> {
        if Some(self.model.paint_revision()?) != self.revision {
            return None;
        }
        let range = self.model.selected_bytes()?;
        let guard = self.runs.lock();
        let handle = guard.resolve(self.run)?;
        if handle.revision != self.run_revision {
            return None;
        }
        let start = handle.layout.caret_position(CaretPosition {
            byte: range.start,
            affinity: CaretAffinity::Downstream,
        })?;
        let end = handle.layout.caret_position(CaretPosition {
            byte: range.end,
            affinity: CaretAffinity::Upstream,
        })?;
        let top = self.transform.apply(Point::new(
            self.padding.left - self.scroll_x + start.x / self.scale,
            self.padding.top + start.y_top / self.scale,
        ));
        let bottom = self.transform.apply(Point::new(
            self.padding.left - self.scroll_x + end.x / self.scale,
            self.padding.top + (start.y_top + start.height) / self.scale,
        ));
        Some(Rect::from_edges(
            top.x,
            top.y,
            bottom.x.max(top.x + 1.0),
            bottom.y,
        ))
    }

    pub fn replace_utf16(&self, text: &str, selected: Option<std::ops::Range<usize>>) {
        self.model.replace_utf16(text, selected);
    }
}

fn shaping_source(t: &TextContent) -> (String, bool) {
    if t.is_placeholder() {
        return (t.placeholder.clone(), false);
    }
    let source = match &t.backing {
        TextBacking::Static(m) => m.text().to_string(),
        TextBacking::Editable(e) => e.display_text(),
    };
    (source, t.password)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameWork {
    pub text_nodes_checked: usize,
    pub text_source_bytes: usize,
    pub text_runs_shaped: usize,
    pub tree_scene_ops_emitted: usize,
    pub scene_items_reconciled: usize,
    pub tree_scene_reused: bool,
    pub tree_nodes_visited: usize,
    pub layouts: usize,
    pub region_layouts: usize,
    pub layout_nodes_measured: usize,
    pub layout_nodes_committed: usize,
    pub layout_nodes_visited: usize,
}

type ViewportCap = (NodeId, f32, f32);

#[derive(Clone, Copy, Debug, Default)]
struct FrameWorkWindow {
    started: Seconds,
    builds: usize,
    total: FrameWork,
    reused: usize,
    max_visited: usize,
}

impl FrameWorkWindow {
    fn record(&mut self, now: Seconds, work: FrameWork) -> Option<String> {
        if self.builds == 0 {
            self.started = now;
        }
        self.builds += 1;
        self.reused += usize::from(work.tree_scene_reused);
        self.max_visited = self.max_visited.max(work.tree_nodes_visited);
        let total = &mut self.total;
        total.text_nodes_checked += work.text_nodes_checked;
        total.text_source_bytes += work.text_source_bytes;
        total.text_runs_shaped += work.text_runs_shaped;
        total.tree_scene_ops_emitted += work.tree_scene_ops_emitted;
        total.scene_items_reconciled += work.scene_items_reconciled;
        total.tree_nodes_visited += work.tree_nodes_visited;
        total.layouts += work.layouts;
        total.region_layouts += work.region_layouts;
        total.layout_nodes_measured += work.layout_nodes_measured;
        total.layout_nodes_committed += work.layout_nodes_committed;
        total.layout_nodes_visited += work.layout_nodes_visited;
        let total = *total;
        (now - self.started >= 1.0).then(|| {
            let line = format!(
                "aqua-work: builds={} scene_reused={} nodes_visited={} max_visited={} layouts={} measured={} committed={} layout_visited={} text_checked={} shaped={} src_bytes={} ops_emitted={} reconciled={}",
                self.builds,
                self.reused,
                total.tree_nodes_visited,
                self.max_visited,
                total.layouts,
                total.layout_nodes_measured,
                total.layout_nodes_committed,
                total.layout_nodes_visited,
                total.text_nodes_checked,
                total.text_runs_shaped,
                total.text_source_bytes,
                total.tree_scene_ops_emitted,
                total.scene_items_reconciled,
            );
            *self = Self::default();
            line
        })
    }
}

pub struct Runtime {
    fill_slots: Vec<Option<paint::FillSlot>>,
    cached_fills_dirty: bool,
    color_profile: aqua_core::ColorProfile,
    interaction: TextInteraction,
    glides: crate::render::scroll::ScrollGlides,
    motion_rt: aqua_ui::motion::runtime::MotionRuntime,
    selection_background: Option<aqua_core::GpuiBackground>,
    motion_seen: Option<(u64, u64, u32, u32)>,
    motion_keys: render_motion::MotionKeys,
    motion_sent: HashMap<aqua_motion::TrackId, Vec<(NodeId, f32)>>,
    tooltip_pending: Option<overlays::PendingTooltip>,
    tooltip_popup: Option<(NodeId, aqua_ui::overlay::OverlayId)>,
    bound_popups: HashMap<aqua_ui::overlay::OverlayId, overlays::BoundPopup>,
    select_popups: HashMap<NodeId, aqua_ui::overlay::OverlayId>,
    tree: UiTree,
    runtime: ReactiveRuntime,
    scope: Scope,
    view: Option<View>,
    host: OverlayHost,
    fonts: SharedFonts,
    runs: SharedRuns,
    viewport: Rect,
    scale: f32,
    scene_map: HashMap<SceneKey, NodeId>,
    scene_order: Vec<SceneKey>,
    text_runs: HashMap<NodeId, TextEntry>,
    scene: Scene,
    chrome_regions: Option<DragRegions>,
    last_build_error: Option<String>,
    footprint_report: Option<Seconds>,
    structural: Rc<RefCell<Option<Element>>>,
    disposed: bool,
    clock: Seconds,
    cached_scene: Vec<(SceneKey, DisplayItem)>,
    fragment_slots: HashMap<NodeId, Vec<usize>>,
    scene_structure: Option<u64>,
    fragment_spans: HashMap<NodeId, std::ops::Range<usize>>,
    patched_topology: bool,
    cached_text_nodes: HashSet<NodeId>,
    viewport_caps: Option<(u64, Vec<ViewportCap>)>,
    native_paint_revisions: HashMap<NodeId, u64>,
    dynamic_order: Vec<SceneKey>,
    cached_version: Option<(u64, u32, u32, u32, u64)>,
    scroll_revision: u64,
    frame_work: FrameWork,
    frame_work_window: Option<FrameWorkWindow>,
    diagnostics: Vec<String>,
    bridge: machine::Bridge,
}

impl Runtime {
    pub fn text_frontier(
        &self,
        node: NodeId,
        byte: usize,
        affinity: CaretAffinity,
    ) -> Option<TextFrontier> {
        if self.disposed || self.runtime.pending() != 0 || !self.tree.command_sink().is_empty() {
            return None;
        }
        let fonts_revision = self.fonts.lock().font_revision();
        let presented = (
            self.scale.to_bits(),
            self.viewport.w.to_bits(),
            self.viewport.h.to_bits(),
            fonts_revision,
        );
        if self
            .cached_version
            .map(|(_, scale, width, height, fonts)| (scale, width, height, fonts))
            != Some(presented)
        {
            return None;
        }
        let record = self.tree.get(node)?;
        let NodeKind::Text(text) = &record.kind else {
            return None;
        };
        let source = text.text();
        if text.is_placeholder() || byte > source.len() || !source.is_char_boundary(byte) {
            return None;
        }
        let entry = self.text_runs.get(&node)?;
        if entry.paint_revision != record.paint_rev
            || entry.scale_bits != self.scale.to_bits()
            || entry.gpui_wrap
                != (self.color_profile == aqua_core::ColorProfile::GpuiWindowsEncoded)
            || entry.fonts_revision != fonts_revision
        {
            return None;
        }
        let runs = self.runs.lock();
        let layout = runs
            .resolve_expected(entry.run, entry.registry_rev)
            .ok()?
            .layout;
        if byte > layout.source_len() {
            return None;
        }
        let caret = layout.caret_position(CaretPosition { byte, affinity })?;
        let height = layout.laid_out()?.size.1 / self.scale;
        let padding = record.style.padding;
        let origin = self
            .tree
            .visual_transform(node)?
            .apply(Point::new(padding.left, padding.top));
        Some(TextFrontier {
            content_origin: origin,
            line_bottom: (caret.y_top + caret.height) / self.scale,
            block_height: height,
        })
    }

    pub fn native_text_service(&self) -> Option<NativeTextService> {
        let node = self.tree.focused()?;
        if !self.cached_text_nodes.contains(&node) {
            return None;
        }
        let record = self.tree.get(node)?;
        let NodeKind::Text(text) = &record.kind else {
            return None;
        };
        let model = text.native_text_model.clone()?;
        model.paint_revision()?;
        let revision = if model.matches_text(text.text()) {
            self.native_paint_revisions.get(&node).copied()
        } else {
            None
        };
        let entry = self.text_runs.get(&node)?;
        Some(NativeTextService {
            model,
            revision,
            runs: self.runs.clone(),
            run: entry.run,
            run_revision: entry.registry_rev,
            transform: self.tree.visual_transform(node)?,
            padding: record.style.padding,
            scroll_x: entry.scroll_x,
            scale: self.scale,
        })
    }
    #[must_use]
    pub fn new(fonts: SharedFonts, runs: SharedRuns) -> Self {
        let runtime = ReactiveRuntime::new();
        let scope = runtime.create_scope();
        Self {
            color_profile: aqua_core::ColorProfile::AquaLinear,
            interaction: TextInteraction::default(),
            glides: crate::render::scroll::ScrollGlides::default(),
            motion_rt: aqua_ui::motion::runtime::MotionRuntime::default(),
            selection_background: None,
            motion_seen: None,
            motion_keys: HashMap::new(),
            motion_sent: HashMap::new(),
            tooltip_pending: None,
            tooltip_popup: None,
            bound_popups: HashMap::new(),
            select_popups: HashMap::new(),
            tree: UiTree::new(),
            runtime,
            scope,
            view: None,
            host: OverlayHost::new(Rect::ZERO),
            fonts,
            runs,
            viewport: Rect::ZERO,
            scale: 1.0,
            fill_slots: Vec::new(),
            cached_fills_dirty: false,
            scene_map: HashMap::new(),
            scene_order: Vec::new(),
            text_runs: HashMap::new(),
            scene: Scene::new(),
            chrome_regions: None,
            structural: Rc::new(RefCell::new(None)),
            last_build_error: None,
            footprint_report: std::env::var_os("AQUA_MEMORY_REPORT")
                .is_some_and(|value| value == "1")
                .then_some(0.0),
            disposed: false,
            clock: 0.0,
            cached_scene: Vec::new(),
            fragment_slots: HashMap::new(),
            scene_structure: None,
            fragment_spans: HashMap::new(),
            patched_topology: false,
            cached_text_nodes: HashSet::new(),
            viewport_caps: None,
            native_paint_revisions: HashMap::new(),
            dynamic_order: Vec::new(),
            cached_version: None,
            scroll_revision: 0,
            frame_work: FrameWork::default(),
            frame_work_window: None,
            diagnostics: Vec::new(),
            bridge: machine::Bridge::new(),
        }
    }

    #[must_use]
    pub fn with_color_profile(mut self, profile: aqua_core::ColorProfile) -> Self {
        if self.color_profile != profile {
            self.color_profile = profile;
            self.tree
                .set_gpui_paint(profile == aqua_core::ColorProfile::GpuiWindowsEncoded);
            let text_nodes: Vec<_> = self
                .tree
                .traverse()
                .into_iter()
                .filter(|node| {
                    self.tree
                        .get(*node)
                        .is_some_and(|record| matches!(record.kind, NodeKind::Text(_)))
                })
                .collect();
            for node in text_nodes {
                self.tree.mark_layout_dirty(node);
            }
        }
        self
    }

    #[must_use]
    pub fn color_profile(&self) -> aqua_core::ColorProfile {
        self.color_profile
    }

    pub fn dispose(&mut self) {
        if self.disposed {
            return;
        }
        self.disposed = true;
        self.tree.dispose_regions();
        self.interaction = TextInteraction::default();
        for (_, popup) in self.bound_popups.drain() {
            popup.view.dispose();
        }
        self.select_popups.clear();
        self.tooltip_pending = None;
        self.tooltip_popup = None;
        if let Some(view) = self.view.take() {
            view.dispose();
        }
        self.scope.clone().dispose();
        let runs: Vec<aqua_text::RunId> = self.text_runs.drain().map(|(_, e)| e.run).collect();
        if !runs.is_empty() {
            let mut registry = self.runs.lock();
            for run in runs {
                registry.remove(run);
            }
        }
    }

    #[must_use]
    pub fn now_secs(&self) -> Seconds {
        self.clock
    }

    pub fn advance_clock(&mut self, now: Seconds) {
        if now.is_finite() && now > self.clock {
            self.clock = now;
            self.tree.advance_control_clock(now);
        }
    }

    #[must_use]
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    #[must_use]
    pub fn runtime(&self) -> &ReactiveRuntime {
        &self.runtime
    }

    #[must_use]
    pub fn tree(&self) -> &UiTree {
        &self.tree
    }

    pub fn tree_mut(&mut self) -> &mut UiTree {
        &mut self.tree
    }

    pub fn scroll_collection_to(
        &mut self,
        key: impl Into<aqua_ui::element::Key>,
        offset: Point,
    ) -> bool {
        let Some((owner, node)) = self.unique_collection_for_key(&key.into()) else {
            return false;
        };
        match owner {
            None => self
                .view
                .as_mut()
                .is_some_and(|view| view.scroll_collection_to(&mut self.tree, node, offset)),
            Some(overlay) => self.bound_popups.get_mut(&overlay).is_some_and(|popup| {
                popup
                    .view
                    .scroll_collection_to(&mut self.tree, node, offset)
            }),
        }
    }

    pub fn collection_follow_state(
        &self,
        key: impl Into<aqua_ui::element::Key>,
    ) -> Option<aqua_ui::collection::FollowState> {
        let (owner, node) = self.unique_collection_for_key(&key.into())?;
        match owner {
            None => self.view.as_ref()?.collection_follow_state(node),
            Some(overlay) => self
                .bound_popups
                .get(&overlay)?
                .view
                .collection_follow_state(node),
        }
    }

    pub fn is_collection_following_tail(
        &self,
        key: impl Into<aqua_ui::element::Key>,
    ) -> Option<bool> {
        self.collection_follow_state(key).map(|state| {
            matches!(
                state,
                aqua_ui::collection::FollowState::Tail { is_following: true }
            )
        })
    }

    pub fn set_collection_follow_mode(
        &mut self,
        key: impl Into<aqua_ui::element::Key>,
        mode: aqua_ui::collection::FollowMode,
    ) -> bool {
        let Some((owner, node)) = self.unique_collection_for_key(&key.into()) else {
            return false;
        };
        match owner {
            None => self
                .view
                .as_mut()
                .is_some_and(|view| view.set_collection_follow_mode(&mut self.tree, node, mode)),
            Some(overlay) => self.bound_popups.get_mut(&overlay).is_some_and(|popup| {
                popup
                    .view
                    .set_collection_follow_mode(&mut self.tree, node, mode)
            }),
        }
    }

    pub fn pause_collection_following_tail(
        &mut self,
        key: impl Into<aqua_ui::element::Key>,
    ) -> bool {
        let Some((owner, node)) = self.unique_collection_for_key(&key.into()) else {
            return false;
        };
        match owner {
            None => self
                .view
                .as_mut()
                .is_some_and(|view| view.pause_collection_following_tail(&mut self.tree, node)),
            Some(overlay) => self.bound_popups.get_mut(&overlay).is_some_and(|popup| {
                popup
                    .view
                    .pause_collection_following_tail(&mut self.tree, node)
            }),
        }
    }

    pub fn update_collection_item_height(
        &mut self,
        collection_key: impl Into<aqua_ui::element::Key>,
        index: usize,
        height: f32,
    ) -> bool {
        self.view.as_mut().is_some_and(|view| {
            view.update_collection_item_height(&mut self.tree, collection_key, index, height)
        })
    }

    pub fn reveal_collection_item(
        &mut self,
        collection_key: impl Into<aqua_ui::element::Key>,
        index: usize,
    ) -> bool {
        self.reveal_collection_item_aligned(
            collection_key,
            index,
            aqua_ui::collection::CollectionRevealAlignment::Nearest,
        )
    }

    pub fn reveal_collection_item_aligned(
        &mut self,
        collection_key: impl Into<aqua_ui::element::Key>,
        index: usize,
        alignment: aqua_ui::collection::CollectionRevealAlignment,
    ) -> bool {
        let Some((owner, node)) = self.unique_collection_for_key(&collection_key.into()) else {
            return false;
        };
        match owner {
            None => self.view.as_mut().is_some_and(|view| {
                view.reveal_collection_index_aligned(&mut self.tree, node, index, alignment)
            }),
            Some(overlay) => self.bound_popups.get_mut(&overlay).is_some_and(|popup| {
                popup
                    .view
                    .reveal_collection_index_aligned(&mut self.tree, node, index, alignment)
            }),
        }
    }

    fn unique_collection_for_key(
        &self,
        key: &aqua_ui::element::Key,
    ) -> Option<(Option<aqua_ui::overlay::OverlayId>, NodeId)> {
        let mut matches = Vec::new();
        if let Some(view) = &self.view {
            matches.extend(
                view.collection_nodes_for_key(&self.tree, key)
                    .into_iter()
                    .map(|node| (None, node)),
            );
        }
        for (overlay, popup) in &self.bound_popups {
            matches.extend(
                popup
                    .view
                    .collection_nodes_for_key(&self.tree, key)
                    .into_iter()
                    .map(|node| (Some(*overlay), node)),
            );
        }
        let [(owner, node)] = matches.as_slice() else {
            return None;
        };
        Some((*owner, *node))
    }

    pub fn collection_item_target_aligned(
        &self,
        collection_key: impl Into<aqua_ui::element::Key>,
        index: usize,
        alignment: aqua_ui::collection::CollectionRevealAlignment,
    ) -> Option<Point> {
        let (owner, node) = self.unique_collection_for_key(&collection_key.into())?;
        match owner {
            None => self
                .view
                .as_ref()?
                .collection_index_target_aligned(&self.tree, node, index, alignment),
            Some(overlay) => self
                .bound_popups
                .get(&overlay)?
                .view
                .collection_index_target_aligned(&self.tree, node, index, alignment),
        }
    }

    pub fn reveal_collection_index(&mut self, container: NodeId, index: usize) -> bool {
        self.view
            .as_mut()
            .is_some_and(|view| view.reveal_collection_index(&mut self.tree, container, index))
    }

    pub fn reveal_collection_index_aligned(
        &mut self,
        container: NodeId,
        index: usize,
        alignment: aqua_ui::collection::CollectionRevealAlignment,
    ) -> bool {
        self.view.as_mut().is_some_and(|view| {
            view.reveal_collection_index_aligned(&mut self.tree, container, index, alignment)
        })
    }

    #[must_use]
    pub fn host(&self) -> &OverlayHost {
        &self.host
    }

    pub fn host_mut(&mut self) -> &mut OverlayHost {
        &mut self.host
    }

    pub fn apply_startup_motion_policy(&mut self, reduced: bool) {
        if reduced {
            self.host.set_reduced_motion(ReducedMotion::Reduce, 0.0);
        }
    }

    #[must_use]
    pub fn viewport(&self) -> Size {
        Size::new(self.viewport.w, self.viewport.h)
    }

    #[must_use]
    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn set_viewport(&mut self, width: f32, height: f32, scale: f32) {
        let rect = Rect::from_edges(0.0, 0.0, width.max(0.0), height.max(0.0));
        if rect != self.viewport {
            self.viewport = rect;
            self.host.set_viewport(&mut self.tree, rect);
        }
        self.scale = scale.max(0.125);
    }

    pub fn mount(&mut self, factory: impl FnOnce(&Scope) -> Element) -> Result<(), RuntimeError> {
        if self.disposed {
            return Err(RuntimeError::Disposed);
        }
        if self.view.is_some() {
            return Err(RuntimeError::AlreadyMounted);
        }
        let element = factory(&self.scope);
        let sink = self.tree.command_sink();
        let mut view = View::new(self.runtime.create_scope(), sink);
        view.set_content(&mut self.tree, vec![element])
            .map_err(RuntimeError::Mount)?;
        self.view = Some(view);
        self.tree.drain_commands();
        Ok(())
    }

    pub fn mount_reactive<D, F>(&mut self, deps: D, mut render: F) -> Result<(), RuntimeError>
    where
        D: DepTuple,
        F: FnMut(D::Values) -> Element + 'static,
    {
        if self.disposed {
            return Err(RuntimeError::Disposed);
        }
        if self.view.is_some() {
            return Err(RuntimeError::AlreadyMounted);
        }
        let sink = self.tree.command_sink();
        let view_scope = self.runtime.create_scope();
        self.view = Some(View::new(view_scope.clone(), sink));
        let pending = Rc::clone(&self.structural);
        let declared = deps.clone();
        view_scope.effect(deps, move |values| {
            *pending.borrow_mut() =
                Some(aqua_reactive::read_check::render_scope(&declared, || {
                    render(values)
                }));
        });
        self.runtime.flush();
        self.drain_structural()
    }

    fn drain_structural(&mut self) -> Result<(), RuntimeError> {
        let Some(element) = self.structural.borrow_mut().take() else {
            return Ok(());
        };
        self.refresh(vec![element])
    }

    pub fn refresh(&mut self, elements: Vec<Element>) -> Result<(), RuntimeError> {
        if self.disposed {
            return Err(RuntimeError::Disposed);
        }
        let Some(view) = self.view.as_mut() else {
            return Err(RuntimeError::NoViewMounted);
        };
        view.set_content(&mut self.tree, elements)
            .map_err(RuntimeError::Mount)?;
        self.tree.drain_commands();
        Ok(())
    }

    pub fn refresh_subtree(
        &mut self,
        key: impl Into<aqua_ui::element::Key>,
        element: Element,
    ) -> Result<bool, RuntimeError> {
        if self.disposed {
            return Err(RuntimeError::Disposed);
        }
        let Some(view) = self.view.as_mut() else {
            return Err(RuntimeError::NoViewMounted);
        };
        let hash = aqua_ui::tree::hash_key(key.into());
        let Some(node) = self.tree.sole_node_with_key(hash) else {
            return Ok(false);
        };
        let refreshed = view
            .refresh_subtree(&mut self.tree, node, element)
            .map_err(RuntimeError::Mount)?;
        self.tree.drain_commands();
        Ok(refreshed)
    }

    pub fn sync(&mut self) -> Result<(), RuntimeError> {
        if self.disposed {
            return Err(RuntimeError::Disposed);
        }
        self.runtime.flush();
        self.drain_structural()?;
        self.tree.drain_commands();
        Ok(())
    }

    pub fn layout(&mut self) -> Result<(), String> {
        let before = self.tree.layout_work();
        let (cap_revision, cap_nodes) = self.tree.viewport_cap_nodes();
        let epoch = self.tree.structure_epoch().wrapping_add(cap_revision);
        if self.viewport_caps.as_ref().map(|(cached, _)| *cached) != Some(epoch) {
            let scan: Vec<_> = cap_nodes
                .into_iter()
                .filter_map(|node| {
                    let record = self.tree.get(node)?;
                    let NodeKind::Text(text) = &record.kind else {
                        return None;
                    };
                    let (share, minimum) = text.viewport_scroll_max?;
                    if !share.is_finite() || !minimum.is_finite() || share < 0.0 || minimum < 0.0 {
                        return None;
                    }
                    let mut parent = record.parent;
                    while let Some(id) = parent {
                        let ancestor = self.tree.get(id)?;
                        if matches!(ancestor.kind, NodeKind::Scroll(_)) {
                            return Some((id, share, minimum));
                        }
                        parent = ancestor.parent;
                    }
                    None
                })
                .collect();
            self.viewport_caps = Some((epoch, scan));
        }
        let viewport_caps: Vec<_> = self
            .viewport_caps
            .as_ref()
            .map(|(_, caps)| caps.as_slice())
            .unwrap_or_default()
            .iter()
            .map(|(id, share, minimum)| (*id, (self.viewport.h * share).max(*minimum)))
            .collect();
        for (node, max_height) in viewport_caps {
            if let Some(record) = self.tree.get(node) {
                let mut style = record.style.clone();
                let next = aqua_ui::style::Length::Px(max_height);
                if style.max_height != next {
                    style.max_height = next;
                    self.tree.set_style(node, style);
                }
            }
        }
        self.refresh_reactive_popups()?;
        let measurer = ProfiledMeasurer {
            fonts: self.fonts.clone(),
            scale: self.scale,
            gpui_line_height: self.color_profile == aqua_core::ColorProfile::GpuiWindowsEncoded,
            gpui_wrap: self.color_profile == aqua_core::ColorProfile::GpuiWindowsEncoded,
        };
        let viewport = Size::new(self.viewport.w, self.viewport.h);
        if let Some(view) = self.view.as_mut() {
            let interaction = &mut self.interaction;
            view.compute_layout_with_row_registration(
                &mut self.tree,
                viewport,
                &measurer,
                &mut |sources| interaction.register_collection_sources(sources),
            )?
        } else {
            self.tree.compute_layout(viewport, &measurer)?
        }
        for popup in self.bound_popups.values_mut() {
            let interaction = &mut self.interaction;
            popup.view.compute_layout_with_row_registration(
                &mut self.tree,
                viewport,
                &measurer,
                &mut |sources| interaction.register_collection_sources(sources),
            )?;
        }
        let after = self.tree.layout_work();
        self.frame_work.layouts += after.passes - before.passes;
        self.frame_work.layout_nodes_measured += after.measured - before.measured;
        self.frame_work.layout_nodes_committed += after.committed - before.committed;
        self.frame_work.region_layouts += after.regions - before.regions;
        Ok(())
    }

    pub fn cursor(&self) -> aqua_core::CursorIcon {
        self.inline_cursor().unwrap_or_else(|| self.tree.cursor())
    }

    #[must_use]
    pub fn last_input_consumed(&self) -> bool {
        self.bridge.input_consumed
    }

    pub fn drop_files(&mut self, paths: Vec<std::path::PathBuf>) -> bool {
        self.drop_files_at(paths, None)
    }

    pub fn drop_files_at(&mut self, paths: Vec<std::path::PathBuf>, at: Option<Point>) -> bool {
        if self.disposed {
            return false;
        }
        let host = &self.host;
        let routed = aqua_ui::input::route_files(&mut self.tree, paths, at, |tree, node| {
            host.input_allowed(tree, node)
        });
        self.tree.drain_commands();
        self.host.sync_lifecycle(&mut self.tree);
        routed.handled || routed.default_prevented
    }

    pub fn external_file_drag(&mut self, event: aqua_ui::input::ExternalFileDragEvent) -> bool {
        if self.disposed {
            return false;
        }
        let before = self.tree.content_version();
        let host = &self.host;
        let result =
            aqua_ui::input::route_external_file_drag(&mut self.tree, event, |tree, node| {
                host.input_allowed(tree, node)
            });
        self.tree.drain_commands();
        self.host.sync_lifecycle(&mut self.tree);
        result.handled || result.default_prevented || self.tree.content_version() != before
    }

    pub fn handle_input(&mut self, event: &CoreInputEvent) -> bool {
        self.handle_input_with_default(event, |_| None)
    }

    pub fn step_input_with_default_outcome(
        &mut self,
        event: &CoreInputEvent,
        default: impl FnOnce(&mut Self) -> Option<bool>,
    ) -> InputDispatchOutcome {
        let mut default_redraw = None;
        let consumed = self.handle_input_with_default(event, |runtime| {
            default(runtime).map(|redraw| {
                default_redraw = Some(redraw);
                true
            })
        });
        InputDispatchOutcome {
            consumed,
            redraw: self.input_redraw(consumed && default_redraw.unwrap_or(true)),
        }
    }

    pub fn handle_input_with_default(
        &mut self,
        event: &CoreInputEvent,
        default: impl FnOnce(&mut Self) -> Option<bool>,
    ) -> bool {
        let consumed = self.handle_input_with_default_inner(event, default);
        if let Some(view) = self.view.as_mut() {
            view.observe_collection_scrolls(&self.tree);
        }
        for popup in self.bound_popups.values_mut() {
            popup.view.observe_collection_scrolls(&self.tree);
        }
        consumed
    }

    fn handle_input_with_default_inner(
        &mut self,
        event: &CoreInputEvent,
        default: impl FnOnce(&mut Self) -> Option<bool>,
    ) -> bool {
        if self.disposed {
            return false;
        }
        if matches!(event, CoreInputEvent::Focused(false)) {
            self.blur_text_interaction();
        }
        if self.host.prepare_input(&mut self.tree, event) {
            self.cancel_text_interaction();
            self.tree.drain_commands();
            self.process_control_requests();
            return true;
        }
        if matches!(
            event,
            CoreInputEvent::PointerDown(..)
                | CoreInputEvent::KeyDown(_)
                | CoreInputEvent::Focused(false)
        ) {
            self.dismiss_tooltip();
        }
        let host = &self.host;
        aqua_ui::controls::cancel_blocked(&mut self.tree, |tree, node| {
            host.input_allowed(tree, node)
        });
        self.tree
            .cancel_blocked_presses(|tree, node| host.input_allowed(tree, node));
        let routed = aqua_ui::input::route_input(&mut self.tree, event, |tree, node| {
            host.input_allowed(tree, node)
        });
        if routed.default_prevented {
            self.cancel_text_interaction();
            self.tree.drain_commands();
            self.host.sync_lifecycle(&mut self.tree);
            self.process_control_requests();
            return true;
        }
        if let Some(consumed) = default(self) {
            self.tree.drain_commands();
            self.host.sync_lifecycle(&mut self.tree);
            self.process_control_requests();
            return consumed || routed.handled;
        }
        if self.handle_inline_input(event) || self.handle_text_interaction(event) {
            self.tree.drain_commands();
            self.process_control_requests();
            return true;
        }
        if self.begin_wheel(event) {
            self.tree.drain_commands();
            return true;
        }
        let consumed = match event {
            CoreInputEvent::TextInput { text } => {
                if self.text_input_allowed() {
                    events::dispatch_text_input(&mut self.tree, text)
                } else {
                    true
                }
            }
            CoreInputEvent::Ime(ime) => {
                if self.text_input_allowed() {
                    events::dispatch_ime(&mut self.tree, ime)
                } else {
                    true
                }
            }
            CoreInputEvent::ImeReplace { .. } | CoreInputEvent::ImeSelect { .. } => {
                if self.text_input_allowed() {
                    events::dispatch_default(&mut self.tree, event)
                } else {
                    true
                }
            }
            _ => self.host.handle_prepared_input(&mut self.tree, event),
        };
        self.host.sync_lifecycle(&mut self.tree);
        let selected = self.begin_text_interaction(event);
        self.tree.drain_commands();
        self.host.sync_lifecycle(&mut self.tree);
        self.process_control_requests();
        consumed || selected || routed.handled
    }

    fn text_input_allowed(&self) -> bool {
        self.tree
            .focused()
            .is_some_and(|node| self.text_node_allowed(node) && !self.tree.is_disabled(node))
    }

    fn text_node_allowed(&self, mut node: NodeId) -> bool {
        let Some(modal) = self.host.service().modal_blocker() else {
            return true;
        };
        loop {
            let parent = self.tree.get(node).and_then(|r| r.parent);
            match parent {
                None => return false,
                Some(p) if Some(p) == self.tree.root() => break,
                Some(p) => node = p,
            }
        }
        let Some(overlay) = self.host.overlay_of_root(node) else {
            return false;
        };
        let stack = self.host.service().stack();
        let modal_pos = stack.iter().position(|o| *o == modal).unwrap_or(0);
        stack
            .iter()
            .position(|o| *o == overlay)
            .is_some_and(|pos| pos >= modal_pos)
    }

    #[must_use]
    pub fn ime_anchor(&self) -> Option<Rect> {
        let focused = self.tree.focused()?;
        let rec = self.tree.get(focused)?;
        let NodeKind::Text(t) = &rec.kind else {
            return None;
        };
        if !t.is_editable() {
            return None;
        }
        let caret_position = if t.external_text_input {
            let TextBacking::Editable(editor) = &t.backing else {
                return None;
            };
            CaretPosition {
                byte: editor.selection().range().start,
                affinity: CaretAffinity::Downstream,
            }
        } else {
            text_caret_position(t)?
        };
        let padding = rec.style.padding;
        let scroll_x = self
            .text_runs
            .get(&focused)
            .map_or(0.0, |entry| entry.scroll_x);
        let transform = self.tree.visual_transform(focused)?;
        let entry = self.text_runs.get(&focused)?;
        let guard = self.runs.lock();
        let handle = guard.resolve(entry.run)?;
        let layout = handle.layout;
        let caret = layout.caret_position(caret_position)?;
        let start = transform.apply(Point::new(
            padding.left - scroll_x + caret.x / self.scale,
            padding.top + caret.y_top / self.scale,
        ));
        let width = if t.external_text_input {
            let TextBacking::Editable(editor) = &t.backing else {
                return None;
            };
            let end = layout.caret_position(CaretPosition {
                byte: editor.selection().range().end,
                affinity: CaretAffinity::Upstream,
            })?;
            (end.x - caret.x).max(self.scale)
        } else {
            2.0
        };
        let end = transform.apply(Point::new(
            padding.left - scroll_x + (caret.x + width) / self.scale,
            padding.top + (caret.y_top + caret.height) / self.scale,
        ));
        Some(Rect::from_edges(start.x, start.y, end.x, end.y))
    }

    pub fn set_frame_stats(&mut self, enabled: bool) {
        self.frame_work_window = enabled.then(FrameWorkWindow::default);
    }

    pub fn take_diagnostics(&mut self) -> Vec<String> {
        std::mem::take(&mut self.diagnostics)
    }

    pub fn build_frame(&mut self) -> Result<Option<Seconds>, RuntimeError> {
        self.tree.take_visited();
        self.tree.take_layout_visited();
        let built = self.build_frame_inner();
        self.frame_work.tree_nodes_visited = self.tree.take_visited();
        self.frame_work.layout_nodes_visited = self.tree.take_layout_visited();
        self.report_work();
        let now = self.clock;
        if self.footprint_report.is_some_and(|next| now >= next) {
            self.footprint_report = Some(now + 1.0);
            self.diagnostics
                .push(format!("aqua-tree-footprint: {:?}", self.tree.footprint()));
        }
        let work = self.frame_work;
        if let Some(line) = self
            .frame_work_window
            .as_mut()
            .and_then(|window| window.record(now, work))
        {
            self.diagnostics.push(line);
        }
        self.last_build_error = built.as_ref().err().map(ToString::to_string);
        built
    }

    fn build_frame_inner(&mut self) -> Result<Option<Seconds>, RuntimeError> {
        if self.disposed {
            return Err(RuntimeError::Disposed);
        }
        self.frame_work = FrameWork::default();
        self.patched_topology = false;
        {
            let _span = probe::span(Phase::Reactive);
            self.runtime.flush();
        }
        self.drain_structural()?;
        self.process_control_requests();
        if self.host.begin_popup_exits(self.clock) {
            self.cached_version = None;
        }
        let fonts_revision = self.fonts.lock().font_revision();
        if self
            .cached_version
            .is_some_and(|version| version.4 != fonts_revision)
        {
            for node in self.tree.traverse() {
                self.tree.sync_text_leaf(node);
            }
        }
        {
            let _span = probe::span(Phase::Layout);
            self.layout().map_err(RuntimeError::Build)?;
        }
        aqua_core::checkpoint::reach();
        if self.advance_text_selection() {
            let _span = probe::span(Phase::Layout);
            self.layout().map_err(RuntimeError::Build)?;
            if let Some(view) = self.view.as_mut() {
                view.observe_collection_scrolls(&self.tree);
            }
        }
        self.follow_external_text_carets();
        self.host.reanchor_all(&mut self.tree);
        self.reconcile_motion();
        let version = (
            self.tree.scene_version(),
            self.scale.to_bits(),
            self.viewport.w.to_bits(),
            self.viewport.h.to_bits(),
            fonts_revision,
        );
        self.sync_selection_bindings();
        self.tree.run_observers();
        let content_changed = self.cached_version != Some(version);
        let same_context = self.cached_version.is_some_and(|previous| {
            (previous.1, previous.2, previous.3, previous.4)
                == (version.1, version.2, version.3, version.4)
        });
        let fill_patched = content_changed && same_context && self.patch_fill_changes();
        if fill_patched {
            self.cached_version = Some(version);
        }
        if content_changed && !fill_patched {
            self.sync_cached_fills();
        }
        let patched = fill_patched || (content_changed && self.patch_dirty_scene(version));
        if content_changed
            && !patched
            && (!self.cached_text_nodes.is_empty()
                || self.scene_structure != Some(self.tree.structure_epoch()))
        {
            let _span = probe::span(Phase::Text);
            self.sync_text_runs();
        }
        aqua_core::checkpoint::reach();
        let focused_scroll = |runtime: &Self| {
            runtime
                .tree
                .focused()
                .and_then(|node| runtime.text_runs.get(&node))
                .map(|entry| entry.scroll_x.to_bits())
        };
        let before_scroll = focused_scroll(self);
        self.update_text_scroll();
        let rebuilt = (content_changed && !patched) || before_scroll != focused_scroll(self);
        if rebuilt {
            let _span = probe::span(Phase::Build);
            let emitted = self.emit_keyed();
            let same_topology = self.scene_structure == Some(self.tree.structure_epoch())
                && emitted.len() == self.cached_scene.len()
                && emitted
                    .iter()
                    .zip(&self.cached_scene)
                    .all(|((key, item), (old, previous))| {
                        key == old
                            && std::mem::discriminant(item) == std::mem::discriminant(previous)
                    });
            self.cached_scene = emitted;
            self.cached_fills_dirty = false;
            if !same_topology {
                self.index_fragments();
                self.cached_text_nodes = self
                    .cached_scene
                    .iter()
                    .filter_map(|(key, item)| {
                        matches!(item, DisplayItem::Text { .. }).then_some(key.node())
                    })
                    .collect();
            }
            self.scene_structure = Some(self.tree.structure_epoch());
            self.scroll_revision = self.tree.scroll_revision();
            aqua_core::checkpoint::reach();
            self.native_paint_revisions = self
                .cached_text_nodes
                .iter()
                .filter_map(|node| {
                    let record = self.tree.get(*node)?;
                    let NodeKind::Text(text) = &record.kind else {
                        return None;
                    };
                    let model = text.native_text_model.as_ref()?;
                    if !model.matches_text(text.text()) {
                        return None;
                    }
                    Some((*node, model.paint_revision()?))
                })
                .collect();
            self.frame_work.tree_scene_ops_emitted = self.cached_scene.len();
            self.cached_version = Some(version);
        } else {
            self.frame_work.tree_scene_reused = !patched;
            self.patch_scroll_offsets();
        }
        aqua_core::checkpoint::reach();
        let selection_items = self.caret_selection_items();
        let mut dynamic = self.text_background_items(&selection_items)?;
        dynamic.extend(selection_items);
        let dynamic: Vec<_> = dynamic
            .into_iter()
            .filter(|(key, _)| {
                self.cached_text_nodes.contains(&key.node())
                    && !matches!(
                        key,
                        SceneKey::CaretOpacityStart { .. } | SceneKey::CaretOpacityEnd { .. }
                    )
            })
            .collect();
        let dynamic_order: Vec<_> = dynamic.iter().map(|(key, _)| *key).collect();
        if !rebuilt && !self.patched_topology && self.dynamic_order == dynamic_order {
            self.frame_work.scene_items_reconciled += dynamic.len();
            for (key, item) in dynamic {
                if let Some(id) = self.scene_map.get(&key).copied() {
                    self.scene.update(id, item);
                }
            }
        } else {
            self.sync_cached_fills();
            let mut grouped: HashMap<NodeId, Vec<(SceneKey, DisplayItem)>> = HashMap::new();
            for (key, item) in dynamic {
                grouped.entry(key.node()).or_default().push((key, item));
            }
            let mut keyed = Vec::with_capacity(self.cached_scene.len() + dynamic_order.len());
            for (key, item) in self.cached_scene.iter().cloned() {
                if matches!(item, DisplayItem::Text { .. }) {
                    if let Some(items) = grouped.remove(&key.node()) {
                        keyed.extend(items);
                    }
                }
                keyed.push((key, item));
            }
            self.frame_work.scene_items_reconciled = keyed.len();
            self.reconcile(keyed);
            self.rebuild_fill_slots();
            self.tree.acknowledge_fills();
            self.dynamic_order = dynamic_order;
        }
        aqua_core::checkpoint::reach();
        let now = self.clock;
        self.host.advance_motion(now);
        self.tree.finish_update_traces();
        self.tree.run_deferred_canvas_actions();
        Ok([
            self.host.motion_deadline(now),
            self.selection_deadline(),
            self.tooltip_deadline(),
        ]
        .into_iter()
        .flatten()
        .min_by(f64::total_cmp))
    }

    fn report_work(&self) {
        if !probe::active() {
            return;
        }
        let work = self.frame_work;
        let count = |metric, value: usize| probe::count(metric, value as u64);
        count(Metric::Layouts, work.layouts);
        count(Metric::NodesMeasured, work.layout_nodes_measured);
        count(Metric::NodesCommitted, work.layout_nodes_committed);
        count(Metric::LayoutNodesVisited, work.layout_nodes_visited);
        count(Metric::NodesVisited, work.tree_nodes_visited);
        count(Metric::TextNodesChecked, work.text_nodes_checked);
        count(Metric::TextRunsShaped, work.text_runs_shaped);
        count(Metric::TextSourceBytes, work.text_source_bytes);
        count(Metric::SceneOpsEmitted, work.tree_scene_ops_emitted);
        count(Metric::SceneReused, usize::from(work.tree_scene_reused));
        count(Metric::ScenePatched, work.scene_items_reconciled);
    }

    #[must_use]
    pub fn build_error(&self) -> Option<&str> {
        self.last_build_error.as_deref()
    }

    #[must_use]
    pub fn frame_stats(&self) -> aqua_ui::tree::FrameStats {
        self.tree.last_stats()
    }

    #[must_use]
    pub fn frame_work(&self) -> FrameWork {
        self.frame_work
    }

    fn text_rect(&self, node: NodeId) -> Option<Rect> {
        let r = self.tree.rect(node)?;
        let mut parent = if self.tree.is_portal(node) {
            None
        } else {
            self.tree.get(node)?.parent
        };
        while let Some(id) = parent {
            let record = self.tree.get(id)?;
            if self.tree.is_portal(id) {
                break;
            }
            parent = record.parent;
        }
        let p = self.tree.get(node)?.style.padding;
        Some(Rect::from_edges(
            r.x + p.left,
            r.y + p.top,
            (r.x + r.w - p.right).max(r.x + p.left),
            (r.y + r.h - p.bottom).max(r.y + p.top),
        ))
    }

    fn text_local_point(&self, node: NodeId, at: Point) -> Option<Point> {
        let point = aqua_ui::input::local_position(&self.tree, node, at)?;
        let padding = self.tree.get(node)?.style.padding;
        let scroll = self
            .text_runs
            .get(&node)
            .map_or(0.0, |entry| entry.scroll_x);
        Some(Point::new(
            (point.x - padding.left + scroll) * self.scale,
            (point.y - padding.top) * self.scale,
        ))
    }

    fn text_world_rect(&self, node: NodeId) -> Option<Rect> {
        let rect = self.tree.rect(node)?;
        let padding = self.tree.get(node)?.style.padding;
        let transform = self.tree.visual_transform(node)?;
        let start = transform.apply(Point::new(padding.left, padding.top));
        let end = transform.apply(Point::new(
            (rect.w - padding.right).max(padding.left),
            (rect.h - padding.bottom).max(padding.top),
        ));
        Some(Rect::from_edges(start.x, start.y, end.x, end.y))
    }

    fn text_origin(&self, node: NodeId) -> Option<Point> {
        let mut origin = self.text_rect(node)?.origin();
        origin.x -= self.text_runs.get(&node).map_or(0.0, |e| e.scroll_x);
        Some(origin)
    }

    fn update_text_scroll(&mut self) {
        let Some(node) = self.tree.focused() else {
            return;
        };
        let Some(rec) = self.tree.get(node) else {
            return;
        };
        let NodeKind::Text(t) = &rec.kind else { return };
        if !t.is_editable() || t.multiline() {
            return;
        }
        let Some(position) = text_caret_position(t) else {
            return;
        };
        let Some(rect) = self.text_rect(node) else {
            return;
        };
        let Some(entry) = self.text_runs.get_mut(&node) else {
            return;
        };
        let guard = self.runs.lock();
        let Some(run) = guard.resolve(entry.run) else {
            return;
        };
        let Some(caret) = run.layout.caret_position(position) else {
            return;
        };
        let x = caret.x / self.scale;
        let width = (rect.w - 3.0).max(1.0);
        if x < entry.scroll_x {
            entry.scroll_x = x;
        }
        if x > entry.scroll_x + width {
            entry.scroll_x = x - width;
        }
        entry.scroll_x = entry.scroll_x.max(0.0);
    }

    fn sync_text_runs(&mut self) {
        self.sync_text_runs_for(None);
    }

    fn sync_text_runs_for(&mut self, target: Option<NodeId>) {
        let fonts_revision = self.fonts.lock().font_revision();
        let mut live = HashSet::new();
        let mut jobs: Vec<TextJob> = Vec::new();
        let nodes = target.map_or_else(|| self.tree.traverse(), |node| vec![node]);
        for id in nodes {
            let Some(rec) = self.tree.get(id) else {
                continue;
            };
            let NodeKind::Text(t) = &rec.kind else {
                continue;
            };
            live.insert(id.raw());
            self.frame_work.text_nodes_checked += 1;
            self.tree
                .trace_work(id, aqua_ui::program::WorkStage::TextInspected);
            let width = self.text_rect(id).map(|r| r.w).unwrap_or(0.0);

            let multiline = t.multiline();
            if self.text_runs.get(&id).is_some_and(|entry| {
                entry.fonts_revision == fonts_revision
                    && entry.paint_revision == rec.paint_rev
                    && entry.size_bits == t.font_size.to_bits()
                    && entry.wrap == (t.wrap || multiline)
                    && entry.line_multi == multiline
                    && entry.masked == (t.password && !t.is_placeholder())
                    && entry.overflow == t.overflow
                    && entry.width_bits == width.to_bits()
                    && entry.scale_bits == self.scale.to_bits()
                    && entry.gpui_wrap
                        == (self.color_profile == aqua_core::ColorProfile::GpuiWindowsEncoded)
            }) {
                continue;
            }
            let (text, masked) = shaping_source(t);
            self.frame_work.text_source_bytes += text.len();
            jobs.push(TextJob {
                node: id,
                text,
                style: t.effective_text_style(),
                source_style: t.text_style.clone(),
                spans: t.shaping_spans().to_vec(),
                wrap: t.wrap || multiline,
                multiline,
                masked,
                overflow: t.overflow,
                width,
                paint_revision: rec.paint_rev,
            });
        }
        for TextJob {
            node: id,
            text,
            style,
            source_style,
            spans,
            wrap,
            multiline,
            masked,
            overflow,
            width,
            paint_revision,
        } in jobs
        {
            let needs = match self.text_runs.get(&id) {
                Some(e) => {
                    e.fonts_revision != fonts_revision
                        || e.source_style != source_style
                        || e.spans != spans
                        || e.text != text
                        || e.width_bits != width.to_bits()
                        || e.scale_bits != self.scale.to_bits()
                        || e.gpui_wrap
                            != (self.color_profile == aqua_core::ColorProfile::GpuiWindowsEncoded)
                        || e.size_bits != style.size_px.to_bits()
                        || e.wrap != wrap
                        || e.line_multi != multiline
                        || e.masked != masked
                        || e.overflow != overflow
                }
                None => true,
            };
            if !needs {
                if let Some(entry) = self.text_runs.get_mut(&id) {
                    entry.paint_revision = paint_revision;
                }
                continue;
            }
            self.frame_work.text_runs_shaped += 1;
            self.tree
                .trace_work(id, aqua_ui::program::WorkStage::TextShaped);
            let build = || {
                let mut layout = TextLayout::new();
                layout.set_gpui_line_height(
                    self.color_profile == aqua_core::ColorProfile::GpuiWindowsEncoded,
                );
                layout.set_gpui_wrap(
                    self.color_profile == aqua_core::ColorProfile::GpuiWindowsEncoded,
                );
                match spans.is_empty() {
                    true => layout.set_text(&text, &style),
                    false => layout.set_spans(spans.clone()),
                }
                layout.set_line_mode(if multiline {
                    LineMode::Multi
                } else {
                    LineMode::Single
                });
                layout.set_wrap(if wrap { WrapMode::Word } else { WrapMode::None });
                layout.set_max_width(if wrap { Some(width.max(0.0)) } else { None });
                layout.set_scale(self.scale);
                layout.set_mask(masked.then_some('\u{2022}'));
                if let Some(mode) = overflow.filter(|_| !multiline && !masked) {
                    layout.layout_single_line_overflow(
                        &mut self.fonts.lock(),
                        width.max(0.0),
                        mode,
                    );
                } else {
                    layout.layout(&mut self.fonts.lock());
                }
                layout
            };
            let updated = self
                .text_runs
                .get(&id)
                .map(|e| (e.run, self.runs.lock().update(e.run, build())));
            let registry_rev = match updated {
                Some((_, Some(r))) => r,
                Some((stale, None)) => {
                    let (run, r) = self.runs.lock().insert(build());
                    if let Some(entry) = self.text_runs.get_mut(&id) {
                        debug_assert_eq!(entry.run, stale);
                        entry.run = run;
                    }
                    r
                }
                None => {
                    let (run, r) = self.runs.lock().insert(build());
                    self.text_runs.insert(
                        id,
                        TextEntry {
                            fonts_revision,
                            run,
                            registry_rev: r,
                            text: text.clone(),
                            source_style: source_style.clone(),
                            spans: spans.clone(),
                            paint_revision,
                            width_bits: width.to_bits(),
                            scale_bits: self.scale.to_bits(),
                            gpui_wrap: self.color_profile
                                == aqua_core::ColorProfile::GpuiWindowsEncoded,
                            size_bits: style.size_px.to_bits(),
                            wrap,
                            line_multi: multiline,
                            masked,
                            overflow,
                            scroll_x: 0.0,
                            access_runs: RefCell::new(None),
                        },
                    );
                    r
                }
            };
            if let Some(entry) = self.text_runs.get_mut(&id) {
                entry.fonts_revision = fonts_revision;
                entry.registry_rev = registry_rev;
                entry.text = text;
                entry.source_style = source_style;
                entry.spans = spans;
                entry.paint_revision = paint_revision;
                entry.width_bits = width.to_bits();
                entry.scale_bits = self.scale.to_bits();
                entry.gpui_wrap = self.color_profile == aqua_core::ColorProfile::GpuiWindowsEncoded;
                entry.size_bits = style.size_px.to_bits();
                entry.wrap = wrap;
                entry.line_multi = multiline;
                entry.masked = masked;
                entry.overflow = overflow;
            }
        }
        if target.is_some() {
            return;
        }
        let dead: Vec<NodeId> = self
            .text_runs
            .keys()
            .copied()
            .filter(|id| !live.contains(&id.raw()))
            .collect();
        for id in dead {
            if let Some(entry) = self.text_runs.remove(&id) {
                self.runs.lock().remove(entry.run);
            }
        }
    }

    fn patch_scroll_offsets(&mut self) {
        if self.scroll_revision == self.tree.scroll_revision() {
            return;
        }
        self.scroll_revision = self.tree.scroll_revision();
        for (node, item) in self.tree.scroll_translations() {
            let key = SceneKey::ScrollOffset { node };
            let Some(slot) = self.cached_scene.iter_mut().find(|(k, _)| *k == key) else {
                continue;
            };
            slot.1 = item.clone();
            if let Some(id) = self.scene_map.get(&key).copied() {
                self.scene.update(id, item);
            }
            self.frame_work.scene_items_reconciled += 1;
        }
    }

    fn emit_keyed(&mut self) -> Vec<(SceneKey, DisplayItem)> {
        let items = self.emit_tree_keyed();
        self.wrap_motion_groups(items)
    }

    fn emit_tree_keyed(&mut self) -> Vec<(SceneKey, DisplayItem)> {
        let items = self.tree.emit_scene();
        for (node, _) in &items {
            self.tree
                .trace_work(*node, aqua_ui::program::WorkStage::SceneRebuilt);
        }
        self.motion_keys.clear();
        self.key_tree_items(items)
    }

    fn key_tree_items(
        &mut self,
        items: Vec<(NodeId, DisplayItem)>,
    ) -> Vec<(SceneKey, DisplayItem)> {
        let mut out = Vec::new();
        let mut per_node: HashMap<NodeId, u32> = HashMap::new();
        for (node, item) in items {
            let op = per_node.entry(node).or_insert(0);
            let offset_slot = matches!(item, DisplayItem::PushTransform(_))
                && self.tree.scroll_translate_op(node) == Some(*op);
            let key = match offset_slot {
                true => SceneKey::ScrollOffset { node },
                false => {
                    let key = SceneKey::Tree { node, op: *op };
                    *op += 1;
                    key
                }
            };
            let item = match item {
                DisplayItem::Text {
                    mut at,
                    run,
                    color,
                    opacity,
                } => {
                    let (run_id, rev) = self.ensure_run_for(node);
                    let _ = run;
                    let color = self
                        .tree
                        .get(node)
                        .and_then(|record| match &record.kind {
                            NodeKind::Text(text) if text.is_placeholder() => {
                                Some(text.placeholder_color.unwrap_or(Oklch {
                                    alpha: color.alpha * 0.45,
                                    ..color
                                }))
                            }
                            _ => None,
                        })
                        .unwrap_or(color);
                    if let Some(entry) = self.text_runs.get(&node) {
                        at[0] -= entry.scroll_x;
                    }
                    DisplayItem::Text {
                        at,
                        run: TextRun {
                            id: run_id.0,
                            revision: rev,
                        },
                        color,
                        opacity,
                    }
                }
                other => other,
            };
            if !offset_slot {
                render_motion::note_motion_item(&mut self.motion_keys, node, key, &item);
            }
            let clip_text = self
                .tree
                .get(node)
                .is_some_and(|rec| matches!(&rec.kind, NodeKind::Text(t) if t.is_editable() || t.overflow.is_some()));
            let clip = if clip_text && matches!(item, DisplayItem::Text { .. }) {
                self.text_rect(node)
            } else {
                None
            };
            if let Some(bounds) = clip {
                out.push((
                    SceneKey::TextClipStart { node },
                    if self.color_profile == aqua_core::ColorProfile::GpuiWindowsEncoded {
                        DisplayItem::PushGpuiContentMask { bounds }
                    } else {
                        DisplayItem::PushClip {
                            bounds,
                            radius: 0.0,
                        }
                    },
                ));
            }
            let decorations = match &item {
                DisplayItem::Text {
                    at, color, opacity, ..
                } => self.text_runs.get(&node).and_then(|entry| {
                    let guard = self.runs.lock();
                    let layout = guard.resolve(entry.run)?.layout;
                    Some(
                        layout
                            .decorations()
                            .into_iter()
                            .enumerate()
                            .map(|(index, decoration)| {
                                let bounds = decoration.bounds;
                                let mut fill =
                                    decoration.color_srgb.map_or(*color, |[r, g, b, a]| {
                                        Oklch::from_srgb8(r, g, b, a as f32 / 255.0)
                                    });
                                fill.alpha *= opacity;
                                (
                                    SceneKey::TextDecoration {
                                        node,
                                        index: index as u32,
                                    },
                                    DisplayItem::Rect {
                                        bounds: Rect::from_edges(
                                            at[0] + bounds.x / self.scale,
                                            at[1] + bounds.y / self.scale,
                                            at[0] + (bounds.x + bounds.w) / self.scale,
                                            at[1] + (bounds.y + bounds.h) / self.scale,
                                        ),
                                        radius: 0.0,
                                        fill,
                                        border_width: 0.0,
                                        border_color: Oklch::TRANSPARENT,
                                        shadow: None,
                                    },
                                )
                            })
                            .collect::<Vec<_>>(),
                    )
                }),
                _ => None,
            };
            let glyphs = match &item {
                DisplayItem::Text { at, opacity, .. } => self.tree.get(node).and_then(|record| {
                    let NodeKind::Text(text) = &record.kind else {
                        return None;
                    };
                    if text.is_placeholder() || text.password || text.shaping_spans().is_empty() {
                        return None;
                    }
                    let entry = self.text_runs.get(&node)?;
                    let guard = self.runs.lock();
                    let layout = guard.resolve(entry.run)?.layout;
                    Some(
                        text.inline_glyphs
                            .iter()
                            .enumerate()
                            .filter_map(|(index, glyph)| {
                                let caret = layout.caret_position(CaretPosition {
                                    byte: glyph.byte,
                                    affinity: CaretAffinity::Downstream,
                                })?;
                                let left = at[0] + caret.x / self.scale;
                                let top = at[1]
                                    + caret.y_top / self.scale
                                    + (caret.height / self.scale - glyph.size) / 2.0;
                                let (width, height) = match glyph.intrinsic {
                                    Some((width, height))
                                        if width > 0.0 && height > 0.0 && width > height =>
                                    {
                                        (glyph.size, glyph.size * height / width)
                                    }
                                    Some((width, height)) if width > 0.0 && height > 0.0 => {
                                        (glyph.size * width / height, glyph.size)
                                    }
                                    _ => (glyph.size, glyph.size),
                                };
                                let bounds = Rect::from_edges(
                                    left + (glyph.size - width) / 2.0,
                                    top + (glyph.size - height) / 2.0,
                                    left + (glyph.size + width) / 2.0,
                                    top + (glyph.size + height) / 2.0,
                                );
                                Some((
                                    SceneKey::TextInlineGlyph {
                                        node,
                                        index: index as u32,
                                    },
                                    DisplayItem::Vector {
                                        bounds,
                                        source: glyph.source.clone(),
                                        opacity: *opacity,
                                    },
                                ))
                            })
                            .collect::<Vec<_>>(),
                    )
                }),
                _ => None,
            };
            let inline_paints = match &item {
                DisplayItem::Text { at, opacity, .. } => self.tree.get(node).and_then(|record| {
                    let NodeKind::Text(text) = &record.kind else {
                        return None;
                    };
                    if text.is_placeholder() || text.password {
                        return None;
                    }
                    let entry = self.text_runs.get(&node)?;
                    let guard = self.runs.lock();
                    let layout = guard.resolve(entry.run)?.layout;
                    let mut items = Vec::new();
                    for (index, paint) in text.inline_paints.iter().enumerate() {
                        let Some(start) = layout.caret_position(CaretPosition {
                            byte: paint.bytes.start,
                            affinity: CaretAffinity::Downstream,
                        }) else {
                            continue;
                        };
                        let Some(end) = layout.caret_position(CaretPosition {
                            byte: paint.bytes.end,
                            affinity: CaretAffinity::Upstream,
                        }) else {
                            continue;
                        };
                        let side = (end.x - start.x) / self.scale;
                        if side <= 0.0 {
                            continue;
                        }
                        let line_height = start.height / self.scale;
                        let left = at[0] + start.x / self.scale;
                        let line_top = at[1] + start.y_top / self.scale;
                        let key = |part| SceneKey::TextInlinePaint {
                            node,
                            index: index as u32,
                            part,
                        };
                        match &paint.content {
                            aqua_ui::tree::TextInlinePaintContent::Svg { source, tint } => {
                                let y = line_top + (line_height - side) / 2.0;
                                items.push((
                                    key(0),
                                    DisplayItem::GpuiSvg {
                                        bounds: Rect::from_edges(left, y, left + side, y + side),
                                        source: source.clone(),
                                        tint: *tint,
                                        opacity: *opacity,
                                    },
                                ));
                            }
                            aqua_ui::tree::TextInlinePaintContent::FixedSvg {
                                source,
                                tint,
                                size,
                                intrinsic,
                            } => {
                                let (width, height) = match intrinsic {
                                    Some((width, height))
                                        if *width > 0.0 && *height > 0.0 && width > height =>
                                    {
                                        (*size, *size * height / width)
                                    }
                                    Some((width, height)) if *width > 0.0 && *height > 0.0 => {
                                        (*size * width / height, *size)
                                    }
                                    _ => (*size, *size),
                                };
                                let x = left + (*size - width) / 2.0;
                                let y =
                                    line_top + (line_height - *size) / 2.0 + (*size - height) / 2.0;
                                items.push((
                                    key(0),
                                    DisplayItem::GpuiSvg {
                                        bounds: Rect::from_edges(x, y, x + width, y + height),
                                        source: source.clone(),
                                        tint: *tint,
                                        opacity: *opacity,
                                    },
                                ));
                            }
                            aqua_ui::tree::TextInlinePaintContent::Swatch {
                                fill,
                                edge,
                                centered,
                            } => {
                                let size = (side * 0.78).min(line_height * 0.62);
                                let x = left + if *centered { (side - size) / 2.0 } else { 0.0 };
                                let y = line_top + (line_height - size) / 2.0;
                                let bounds = Rect::from_edges(x, y, x + size, y + size);
                                let corners = aqua_core::CornerRadii::uniform(size * 0.22);
                                let mut fill = *fill;
                                let mut edge = *edge;
                                fill.a *= *opacity;
                                edge.a *= *opacity;
                                items.push((
                                    key(0),
                                    DisplayItem::GpuiBackground {
                                        bounds,
                                        corners,
                                        background: aqua_core::GpuiBackground::Solid(fill),
                                    },
                                ));
                                items.push((
                                    key(1),
                                    DisplayItem::GpuiBorder {
                                        bounds,
                                        corners,
                                        widths: aqua_core::Insets::uniform(1.0),
                                        color: edge,
                                        dashed: false,
                                    },
                                ));
                            }
                        }
                    }
                    Some(items)
                }),
                _ => None,
            };
            out.push((key, item));
            if let Some(decorations) = decorations {
                out.extend(decorations);
            }
            if let Some(glyphs) = glyphs {
                out.extend(glyphs);
            }
            if let Some(paints) = inline_paints {
                out.extend(paints);
            }
            if clip.is_some() {
                out.push((SceneKey::TextClipEnd { node }, DisplayItem::PopClip));
            }
        }
        out
    }

    fn ensure_run_for(&mut self, node: NodeId) -> (aqua_text::RunId, u64) {
        if let Some(e) = self.text_runs.get(&node) {
            return (e.run, e.registry_rev);
        }
        let (
            text,
            style,
            source_style,
            spans,
            wrap,
            multiline,
            masked,
            overflow,
            width,
            paint_revision,
        ) = match self.tree.get(node).and_then(|r| match &r.kind {
            NodeKind::Text(t) => {
                let style = t.effective_text_style();
                let (display, masked) = shaping_source(t);
                Some((
                    display,
                    style,
                    t.text_style.clone(),
                    t.shaping_spans().to_vec(),
                    t.wrap || t.multiline(),
                    t.multiline(),
                    masked,
                    t.overflow,
                    self.text_rect(node).map(|r| r.w).unwrap_or(0.0),
                    r.paint_rev,
                ))
            }
            _ => None,
        }) {
            Some(v) => v,
            None => return (aqua_text::RunId(0), 0),
        };
        let mut layout = TextLayout::new();
        layout.set_gpui_line_height(
            self.color_profile == aqua_core::ColorProfile::GpuiWindowsEncoded,
        );
        layout.set_gpui_wrap(self.color_profile == aqua_core::ColorProfile::GpuiWindowsEncoded);
        match spans.is_empty() {
            true => layout.set_text(&text, &style),
            false => layout.set_spans(spans.clone()),
        }
        layout.set_line_mode(if multiline {
            LineMode::Multi
        } else {
            LineMode::Single
        });
        layout.set_wrap(if wrap { WrapMode::Word } else { WrapMode::None });
        layout.set_max_width(if wrap { Some(width.max(0.0)) } else { None });
        layout.set_scale(self.scale);
        layout.set_mask(masked.then_some('\u{2022}'));
        if let Some(mode) = overflow.filter(|_| !multiline && !masked) {
            layout.layout_single_line_overflow(&mut self.fonts.lock(), width.max(0.0), mode);
        } else {
            layout.layout(&mut self.fonts.lock());
        }
        let (run, r) = self.runs.lock().insert(layout);
        self.text_runs.insert(
            node,
            TextEntry {
                fonts_revision: self.fonts.lock().font_revision(),
                run,
                registry_rev: r,
                text,
                source_style,
                spans,
                paint_revision,
                width_bits: width.to_bits(),
                scale_bits: self.scale.to_bits(),
                gpui_wrap: self.color_profile == aqua_core::ColorProfile::GpuiWindowsEncoded,
                size_bits: style.size_px.to_bits(),
                wrap,
                line_multi: multiline,
                masked,
                overflow,
                scroll_x: 0.0,
                access_runs: RefCell::new(None),
            },
        );
        (run, r)
    }

    fn text_background_items(
        &self,
        selection_items: &[(SceneKey, DisplayItem)],
    ) -> Result<Vec<(SceneKey, DisplayItem)>, RuntimeError> {
        let mut output = Vec::new();
        let runs = self.runs.lock();
        for &node in &self.cached_text_nodes {
            let Some(entry) = self.text_runs.get(&node) else {
                continue;
            };
            if !entry
                .spans
                .iter()
                .any(|span| span.background_hsla.is_some())
            {
                continue;
            }
            let Some(origin) = self.text_origin(node) else {
                continue;
            };
            let Some(handle) = runs.resolve(entry.run) else {
                continue;
            };
            let backgrounds = handle
                .layout
                .gpui_backgrounds()
                .map_err(|error| RuntimeError::Build(error.into()))?;
            if !backgrounds.is_empty()
                && self.color_profile != aqua_core::ColorProfile::GpuiWindowsEncoded
            {
                return Err(RuntimeError::Build(
                    "GPUI text backgrounds require the encoded color profile".into(),
                ));
            }
            let selections: Vec<Rect> = selection_items
                .iter()
                .filter_map(|(key, item)| match key {
                    SceneKey::Selection { node: selected, .. } if *selected == node => {
                        item.bounds()
                    }
                    _ => None,
                })
                .collect();
            for (index, background) in backgrounds.into_iter().enumerate() {
                let r = background.bounds;
                let bounds = Rect::from_edges(
                    origin.x + r.x / self.scale,
                    origin.y + r.y / self.scale,
                    origin.x + (r.x + r.w) / self.scale,
                    origin.y + (r.y + r.h) / self.scale,
                );
                let mut remaining = vec![bounds];
                for selection in &selections {
                    let mut next = Vec::new();
                    for part in remaining {
                        let overlap = part.intersect(*selection);
                        if overlap.is_empty() {
                            next.push(part);
                        } else {
                            if overlap.x > part.x {
                                next.push(Rect::from_edges(
                                    part.x,
                                    part.y,
                                    overlap.x,
                                    part.bottom(),
                                ));
                            }
                            if overlap.right() < part.right() {
                                next.push(Rect::from_edges(
                                    overlap.right(),
                                    part.y,
                                    part.right(),
                                    part.bottom(),
                                ));
                            }
                            if overlap.y > part.y {
                                next.push(Rect::from_edges(
                                    overlap.x,
                                    part.y,
                                    overlap.right(),
                                    overlap.y,
                                ));
                            }
                            if overlap.bottom() < part.bottom() {
                                next.push(Rect::from_edges(
                                    overlap.x,
                                    overlap.bottom(),
                                    overlap.right(),
                                    part.bottom(),
                                ));
                            }
                        }
                    }
                    remaining = next;
                }
                let [h, s, l, a] = background.color_hsla;
                for (part, bounds) in remaining.into_iter().enumerate() {
                    output.push((
                        SceneKey::TextBackground {
                            node,
                            index: index as u32,
                            part: part as u32,
                        },
                        DisplayItem::GpuiBackground {
                            bounds,
                            corners: aqua_core::CornerRadii::uniform(0.0),
                            background: aqua_core::GpuiBackground::Solid(aqua_core::GpuiHsla {
                                h,
                                s,
                                l,
                                a,
                            }),
                        },
                    ));
                }
            }
        }
        Ok(output)
    }

    pub fn selection_rects(&self) -> Vec<Rect> {
        self.caret_selection_items()
            .into_iter()
            .filter_map(|(key, item)| match (key, item) {
                (
                    SceneKey::Selection { node, .. },
                    DisplayItem::GpuiBackground { bounds, .. } | DisplayItem::Rect { bounds, .. },
                ) => {
                    let laid = self.tree.rect(node)?;
                    let shown = self.tree.visual_rect(node)?;
                    let (dx, dy) = (shown.x - laid.x, shown.y - laid.y);
                    Some(Rect::from_edges(
                        bounds.x + dx,
                        bounds.y + dy,
                        bounds.x + bounds.w + dx,
                        bounds.y + bounds.h + dy,
                    ))
                }
                _ => None,
            })
            .collect()
    }

    fn caret_selection_items(&self) -> Vec<(SceneKey, DisplayItem)> {
        let mut nodes = self.interaction.selected_nodes.clone();
        if let Some(node) = self.tree.focused() {
            if !nodes.contains(&node) {
                nodes.push(node);
            }
        }
        nodes
            .into_iter()
            .flat_map(|node| self.text_selection_items(node))
            .collect()
    }

    fn text_selection_items(&self, focused: NodeId) -> Vec<(SceneKey, DisplayItem)> {
        if !self.text_node_allowed(focused) {
            return Vec::new();
        }
        let mut opacity = 1.0;
        let mut ancestor = Some(focused);
        while let Some(node) = ancestor {
            opacity *= self
                .tree
                .paint(node)
                .and_then(|p| p.opacity)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
            ancestor = self.tree.get(node).and_then(|r| r.parent);
        }
        if opacity == 0.0 {
            return Vec::new();
        }
        let (text, origin, editable, selection, caret_byte, composition) =
            match self.tree.get(focused).and_then(|r| match &r.kind {
                NodeKind::Text(t) => Some(t),
                _ => None,
            }) {
                Some(t) => {
                    let Some(rect) = self.tree.rect(focused) else {
                        return Vec::new();
                    };
                    let Some(origin) = self.text_origin(focused) else {
                        return Vec::new();
                    };
                    let _ = rect;
                    let selection = match &t.backing {
                        TextBacking::Static(_) => t.selection,
                        TextBacking::Editable(e) => {
                            let s = e.selection();
                            (s.anchor != s.caret).then_some(s)
                        }
                    };
                    let composition = match &t.backing {
                        TextBacking::Editable(e) => e.composition_range(),
                        TextBacking::Static(_) => None,
                    };
                    (
                        t,
                        origin,
                        t.is_editable(),
                        selection,
                        text_caret_position(t),
                        composition,
                    )
                }
                None => return Vec::new(),
            };
        let appearance = text.text_appearance;
        let gpui_appearance = text.gpui_text_appearance;
        let paint = |bounds, fill, source| match source {
            Some(color) => DisplayItem::GpuiBackground {
                bounds,
                corners: aqua_core::CornerRadii::uniform(0.0),
                background: aqua_core::GpuiBackground::Solid(color),
            },
            None => DisplayItem::Rect {
                bounds,
                radius: 0.0,
                fill,
                border_width: 0.0,
                border_color: Oklch::TRANSPARENT,
                shadow: None,
            },
        };
        let Some(entry) = self.text_runs.get(&focused) else {
            return Vec::new();
        };
        let guard = self.runs.lock();
        let Some(handle) = guard.resolve(entry.run) else {
            return Vec::new();
        };
        let layout = handle.layout;
        let mut out = Vec::new();
        if let Some(sel @ Selection { .. }) = selection {
            let (a, b) = (sel.anchor.min(sel.caret), sel.anchor.max(sel.caret));
            let fill = appearance
                .map(|appearance| appearance.selection)
                .unwrap_or_else(|| {
                    Oklch::from_hex("#2f6feb")
                        .map(|c| Oklch { alpha: 0.35, ..c })
                        .unwrap_or(Oklch {
                            l: 0.55,
                            c: 0.12,
                            h: 260.0,
                            alpha: 0.35,
                        })
                });
            let background = gpui_appearance
                .map(|appearance| appearance.selection)
                .or(self.selection_background);
            for (i, r) in layout.selection_rects(a..b).into_iter().enumerate() {
                let bounds = Rect::from_edges(
                    origin.x + r.x / self.scale,
                    origin.y + r.y / self.scale,
                    origin.x + (r.x + r.w) / self.scale,
                    origin.y + (r.y + r.h) / self.scale,
                );
                out.push((
                    SceneKey::Selection {
                        node: focused,
                        index: i as u32,
                    },
                    match background {
                        Some(background) => DisplayItem::GpuiBackground {
                            bounds,
                            corners: aqua_core::CornerRadii::uniform(0.0),
                            background,
                        },
                        None => paint(bounds, fill, None),
                    },
                ));
            }
        }
        if let Some(range) = composition {
            let underline = appearance.map_or(Oklch::oklch(0.72, 0.14, 95.0, 1.0), |appearance| {
                appearance.ime_underline
            });
            let width = gpui_appearance.map_or_else(
                || {
                    appearance.map_or(2.0, |appearance| {
                        appearance.ime_underline_width * self.scale
                    })
                },
                |appearance| appearance.ime_underline_width * self.scale,
            );
            for (i, r) in layout.selection_rects(range).into_iter().enumerate() {
                out.push((
                    SceneKey::ImeUnderline {
                        node: focused,
                        index: i as u32,
                    },
                    paint(
                        Rect::from_edges(
                            origin.x + r.x / self.scale,
                            origin.y + (r.y + r.h - width) / self.scale,
                            origin.x + (r.x + r.w) / self.scale,
                            origin.y + (r.y + r.h) / self.scale,
                        ),
                        underline,
                        gpui_appearance.map(|appearance| appearance.ime_underline),
                    ),
                ));
            }
        }
        if editable {
            if let Some(position) = caret_byte {
                if let Some(c) = layout.caret_position(position) {
                    out.push((
                        SceneKey::Caret { node: focused },
                        paint(
                            Rect::from_edges(
                                origin.x + c.x / self.scale,
                                origin.y + c.y_top / self.scale,
                                origin.x
                                    + (c.x
                                        + gpui_appearance.map_or_else(
                                            || {
                                                appearance.map_or(1.5, |appearance| {
                                                    appearance.caret_width * self.scale
                                                })
                                            },
                                            |appearance| appearance.caret_width * self.scale,
                                        ))
                                        / self.scale,
                                origin.y + (c.y_top + c.height) / self.scale,
                            ),
                            appearance.map_or(Oklch::WHITE, |appearance| appearance.caret),
                            gpui_appearance.map(|appearance| appearance.caret),
                        ),
                    ));
                }
            }
        }
        if !out.is_empty() {
            if let Some(bounds) = self.text_rect(focused) {
                out.insert(
                    0,
                    (
                        SceneKey::CaretClipStart { node: focused },
                        if gpui_appearance.is_some() {
                            DisplayItem::PushGpuiContentMask { bounds }
                        } else {
                            DisplayItem::PushClip {
                                bounds,
                                radius: 0.0,
                            }
                        },
                    ),
                );
                out.push((
                    SceneKey::CaretClipEnd { node: focused },
                    DisplayItem::PopClip,
                ));
            }
        }
        if !out.is_empty() && opacity != 1.0 {
            out.insert(
                0,
                (
                    SceneKey::CaretOpacityStart { node: focused },
                    DisplayItem::PushOpacity(opacity),
                ),
            );
            out.push((
                SceneKey::CaretOpacityEnd { node: focused },
                DisplayItem::PopOpacity,
            ));
        }
        out
    }

    fn reconcile(&mut self, items: Vec<(SceneKey, DisplayItem)>) {
        let scene = &mut self.scene;
        let order: Vec<SceneKey> = items.iter().map(|(k, _)| *k).collect();
        if order == self.scene_order
            && order.len() == self.scene_map.len()
            && order.iter().all(|k| self.scene_map.contains_key(k))
        {
            for (key, item) in items {
                if let Some(id) = self.scene_map.get(&key).copied() {
                    scene.update(id, item);
                }
            }
            return;
        }
        let live: HashSet<_> = order.iter().copied().collect();
        self.scene_order
            .iter()
            .filter(|key| !live.contains(*key))
            .for_each(|key| {
                if let Some(id) = self.scene_map.remove(key) {
                    scene.remove(id);
                }
            });
        let mut ids = Vec::with_capacity(items.len());
        for (key, item) in items {
            let id = match self.scene_map.get(&key).copied() {
                Some(id) => {
                    scene.update(id, item);
                    id
                }
                None => {
                    let id = scene.insert(item);
                    self.scene_map.insert(key, id);
                    id
                }
            };
            ids.push(id);
        }
        assert!(scene.set_order(&ids));
        self.scene_order = order;
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.dispose();
    }
}

impl Runtime {
    fn input_redraw(&self, consumed: bool) -> FrameReason {
        let input = match consumed {
            true => FrameReason::INPUT,
            false => FrameReason::empty(),
        };
        let motion = match self.host.motion_deadline(self.clock) {
            Some(_) => FrameReason::ANIMATION,
            None => FrameReason::empty(),
        };
        input | motion
    }

    #[must_use]
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }

    #[must_use]
    pub fn chrome_regions(&self) -> Option<&DragRegions> {
        self.chrome_regions.as_ref()
    }

    #[must_use]
    pub fn a11y_nodes(&self) -> Option<(NodeId, NodeId, Vec<A11yNode>)> {
        let root = self.tree.root()?;
        let focused = self.tree.focused().unwrap_or(root);
        let items = self
            .tree
            .emit_semantics()
            .into_iter()
            .map(|semantic| {
                let bounds = self.tree.visual_rect(semantic.node).map(|bounds| Rect {
                    x: bounds.x * self.scale,
                    y: bounds.y * self.scale,
                    w: bounds.w * self.scale,
                    h: bounds.h * self.scale,
                });
                let children = semantic.children.clone();
                A11yNode {
                    text: self.text_accessibility(semantic.node),
                    semantic,
                    bounds,
                    children,
                }
            })
            .collect();
        Some((root, focused, items))
    }

    fn press(&mut self, node: NodeId) {
        let _ = self.tree.set_focused(Some(node));
        let kind = if self
            .tree
            .has_action(node, ActionKind::AccessibilityActivate)
        {
            ActionKind::AccessibilityActivate
        } else {
            ActionKind::Activate
        };
        self.tree.fire_action(ActionEvent {
            source: None,
            node,
            kind,
            text: None,
            focused: None,
        });
    }

    fn apply_access(&mut self, request: &A11yActionRequest) {
        if self.tree.get(request.node).is_none()
            || self.tree.is_disabled(request.node)
            || !self.text_node_allowed(request.node)
        {
            return;
        }
        if let Some(control) = self.tree.control(request.node) {
            let numeric = match control {
                aqua_ui::controls::Control::Slider(control) => Some((control.value, control.range)),
                aqua_ui::controls::Control::Resize(control) => Some((control.value, control.range)),
                _ => None,
            };
            if let Some((value, range)) = numeric {
                let next = match (&request.action, &request.data) {
                    (A11yAction::SetValue, A11yActionData::Numeric(value)) => Some(*value),
                    (A11yAction::SetValue, A11yActionData::Value(value)) => value.parse().ok(),
                    (A11yAction::Increment, _) => Some(value + range.step),
                    (A11yAction::Decrement, _) => Some(value - range.step),
                    _ => None,
                };
                if let Some(next) = next.filter(|value| value.is_finite()) {
                    self.tree.set_control_value(request.node, next);
                    return;
                }
            }
        }
        match request.action {
            A11yAction::Expand | A11yAction::Collapse => {
                let expand = request.action == A11yAction::Expand;
                if self.tree.control(request.node).is_some() {
                    self.tree.set_control_expanded(request.node, expand);
                    self.process_control_requests();
                } else if self.tree.interaction_semantics(request.node).expanded == Some(!expand)
                    && !self.tree.is_disabled(request.node)
                {
                    self.press(request.node);
                }
            }
            A11yAction::Focus => {
                let _ = self.tree.set_focused(Some(request.node));
            }
            A11yAction::Press if !self.tree.is_disabled(request.node) => self.press(request.node),
            A11yAction::SetValue | A11yAction::ReplaceSelectedText => {
                if let A11yActionData::Value(value) = &request.data {
                    if request.action == A11yAction::SetValue
                        && self.tree.control(request.node).is_some()
                    {
                        if let Ok(number) = value.parse::<f64>() {
                            self.tree.set_control_value(request.node, number);
                        }
                        return;
                    }
                    let replace_all = request.action == A11yAction::SetValue;
                    let normalized: String = value
                        .replace("\r\n", "\n")
                        .replace('\r', "\n")
                        .chars()
                        .filter(|character| {
                            !character.is_control() || matches!(character, '\n' | '\t')
                        })
                        .collect();
                    let changed = self
                        .tree
                        .edit_text(request.node, |text| {
                            if let TextBacking::Editable(editor) = &mut text.backing {
                                if replace_all {
                                    editor.select_all();
                                }
                                editor.insert_paste(&normalized);
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false);
                    if changed {
                        let text =
                            self.tree
                                .get(request.node)
                                .and_then(|record| match &record.kind {
                                    NodeKind::Text(text) => Some(text.text().to_owned()),
                                    _ => None,
                                });
                        self.tree.fire_action(ActionEvent {
                            source: None,
                            node: request.node,
                            kind: ActionKind::TextChanged,
                            text,
                            focused: None,
                        });
                    }
                }
            }
            _ => {}
        }
        if let A11yActionData::TextSelection {
            focus_node,
            focus_index,
            anchor_index,
            ..
        } = &request.data
        {
            self.apply_access_selection(request.node, *focus_node, *anchor_index, *focus_index);
        }
        if let A11yActionData::TextByteSelection {
            anchor_node,
            anchor_byte,
            focus_node,
            focus_byte,
        } = request.data
        {
            if request.node == anchor_node {
                self.apply_access_byte_selection(anchor_node, focus_node, anchor_byte, focus_byte);
            }
        }
    }

    fn apply_access_selection(
        &mut self,
        node: NodeId,
        focus_node: NodeId,
        anchor_index: usize,
        focus_index: usize,
    ) {
        if focus_node != node {
            return;
        }
        let Some(NodeKind::Text(text)) = self.tree.get(node).map(|r| &r.kind) else {
            return;
        };
        if !text.is_editable() && !text.selectable {
            return;
        }
        let display = text.display_text();
        let to_byte = |char_index: usize| {
            display
                .char_indices()
                .nth(char_index)
                .map_or(display.len(), |(b, _)| b)
        };
        let selection = Selection::new(to_byte(anchor_index), to_byte(focus_index));
        self.selection_from_access(node, selection);
    }

    pub fn set_chrome_regions(&mut self, regions: Option<DragRegions>) {
        self.chrome_regions = regions;
    }

    #[must_use]
    pub fn text_run_count(&self) -> usize {
        self.text_runs.len()
    }
}

#[cfg(test)]
mod style_decoration_tests {
    use super::*;
    use aqua_ui::element::ui::{Box as UiBox, Text as UiText};
    use aqua_ui::style::{px, Style};

    #[test]
    fn caret_respects_own_and_ancestor_style_opacity() {
        let mut app = Runtime::new(SharedFonts::new(), SharedRuns::new());
        app.mount(|_| {
            UiBox::new()
                .style(Style::new().opacity(0.5))
                .child(
                    UiText::builder()
                        .arg("draft")
                        .editable(true)
                        .style(Style::new().width(px(120.0)).height(px(40.0)).opacity(0.5))
                        .build(),
                )
                .build()
        })
        .unwrap();
        app.set_viewport(300.0, 200.0, 1.0);
        app.build_frame().unwrap();
        let root = app.tree.root().unwrap();
        let field = app.tree.get(root).unwrap().children[0];
        app.tree.set_focused(Some(field));
        let items = app.caret_selection_items();
        assert!(matches!(items.first(), Some((_, DisplayItem::PushOpacity(o))) if *o == 0.25));
        assert!(matches!(items.last(), Some((_, DisplayItem::PopOpacity))));
        let mut paint = app.tree.paint(root).unwrap();
        paint.opacity = Some(0.0);
        app.tree.set_paint(root, paint);
        assert!(app.caret_selection_items().is_empty());
    }
}
