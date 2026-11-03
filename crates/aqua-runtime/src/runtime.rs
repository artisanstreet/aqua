use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

mod access;
mod interaction;

use interaction::TextInteraction;

use aqua_core::{
    DisplayItem, InputEvent as CoreInputEvent, NodeId, Oklch, Point, Rect, Size, TextRun,
};
use aqua_protocol::{
    A11yAction, A11yActionData, A11yActionRequest, A11yNode, DragRegions, FrameReason, Input,
    Instant, Output,
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
    layout::CosmicMeasurer,
    motion::{ReducedMotion, Seconds},
    overlay_host::OverlayHost,
    tree::{ActionEvent, ActionKind, NodeKind, TextBacking, TextContent, UiTree},
};

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
    Caret { node: NodeId },
    Selection { node: NodeId, index: u32 },
    ImeUnderline { node: NodeId, index: u32 },
    TextClipStart { node: NodeId },
    TextClipEnd { node: NodeId },
    CaretClipStart { node: NodeId },
    CaretClipEnd { node: NodeId },
    CaretOpacityStart { node: NodeId },
    CaretOpacityEnd { node: NodeId },
}

impl SceneKey {
    fn node(self) -> NodeId {
        match self {
            Self::Tree { node, .. }
            | Self::Caret { node }
            | Self::Selection { node, .. }
            | Self::ImeUnderline { node, .. }
            | Self::TextClipStart { node }
            | Self::TextClipEnd { node }
            | Self::CaretClipStart { node }
            | Self::CaretClipEnd { node }
            | Self::CaretOpacityStart { node }
            | Self::CaretOpacityEnd { node } => node,
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
    size_bits: u32,
    wrap: bool,
    line_multi: bool,
    masked: bool,
    scroll_x: f32,
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
}

pub struct Runtime {
    interaction: TextInteraction,
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
    structural: Rc<RefCell<Option<Element>>>,
    disposed: bool,
    clock: Seconds,
    cached_scene: Vec<(SceneKey, DisplayItem)>,
    cached_text_nodes: HashSet<NodeId>,
    dynamic_order: Vec<SceneKey>,
    cached_version: Option<(u64, u32, u32, u32, u64)>,
    frame_work: FrameWork,
}

impl Runtime {
    #[must_use]
    pub fn new(fonts: SharedFonts, runs: SharedRuns) -> Self {
        let runtime = ReactiveRuntime::new();
        let scope = runtime.create_scope();
        Self {
            interaction: TextInteraction::default(),
            tree: UiTree::new(),
            runtime,
            scope,
            view: None,
            host: OverlayHost::new(Rect::ZERO),
            fonts,
            runs,
            viewport: Rect::ZERO,
            scale: 1.0,
            scene_map: HashMap::new(),
            scene_order: Vec::new(),
            text_runs: HashMap::new(),
            scene: Scene::new(),
            chrome_regions: None,
            structural: Rc::new(RefCell::new(None)),
            last_build_error: None,
            disposed: false,
            clock: 0.0,
            cached_scene: Vec::new(),
            cached_text_nodes: HashSet::new(),
            dynamic_order: Vec::new(),
            cached_version: None,
            frame_work: FrameWork::default(),
        }
    }

    pub fn dispose(&mut self) {
        if self.disposed {
            return;
        }
        self.disposed = true;
        self.interaction = TextInteraction::default();
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

    #[must_use]
    pub fn host(&self) -> &OverlayHost {
        &self.host
    }

    pub fn host_mut(&mut self) -> &mut OverlayHost {
        &mut self.host
    }

    pub fn apply_startup_motion_policy(&mut self, reduced: bool) {
        if reduced {
            self.host
                .motion_mut()
                .set_reduced_motion(ReducedMotion::Reduce, 0.0);
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
        self.view = Some(View::new(view_scope, sink));
        let pending = Rc::clone(&self.structural);
        self.scope.effect(deps, move |values| {
            *pending.borrow_mut() = Some(render(values));
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
        Ok(())
    }

    pub fn sync(&mut self) -> Result<(), RuntimeError> {
        if self.disposed {
            return Err(RuntimeError::Disposed);
        }
        self.runtime.flush();
        self.drain_structural()
    }

    pub fn layout(&mut self) -> Result<(), String> {
        let measurer = CosmicMeasurer {
            fonts: self.fonts.clone(),
            scale: self.scale,
        };
        let viewport = Size::new(self.viewport.w, self.viewport.h);
        if let Some(view) = self.view.as_mut() {
            view.compute_layout(&mut self.tree, viewport, &measurer)
        } else {
            self.tree.compute_layout(viewport, &measurer)
        }
    }

    pub fn handle_input(&mut self, event: &CoreInputEvent) -> bool {
        if self.handle_text_interaction(event) {
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
            _ => self.host.handle_input(&mut self.tree, event),
        };
        self.host.sync_lifecycle(&mut self.tree);
        let selected = self.begin_text_interaction(event);
        consumed || selected
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
        let caret_position = text_caret_position(t)?;
        let origin = self.text_origin(focused)?;
        let entry = self.text_runs.get(&focused)?;
        let guard = self.runs.lock();
        let handle = guard.resolve(entry.run)?;
        let layout = handle.layout;
        let caret = layout.caret_position(caret_position)?;
        Some(Rect::from_edges(
            origin.x + caret.x / self.scale,
            origin.y + caret.y_top / self.scale,
            origin.x + (caret.x + 2.0) / self.scale,
            origin.y + (caret.y_top + caret.height) / self.scale,
        ))
    }

    pub fn build_frame(&mut self) -> Result<Option<Seconds>, RuntimeError> {
        let built = self.build_frame_inner();
        self.last_build_error = built.as_ref().err().map(ToString::to_string);
        built
    }

    fn build_frame_inner(&mut self) -> Result<Option<Seconds>, RuntimeError> {
        if self.disposed {
            return Err(RuntimeError::Disposed);
        }
        self.frame_work = FrameWork::default();
        self.runtime.flush();
        self.drain_structural()?;
        self.host.sync_lifecycle(&mut self.tree);
        let fonts_revision = self.fonts.lock().font_revision();
        if self
            .cached_version
            .is_some_and(|version| version.4 != fonts_revision)
        {
            for node in self.tree.traverse() {
                self.tree.sync_text_leaf(node);
            }
        }
        self.layout().map_err(RuntimeError::Build)?;
        if self.advance_text_selection() {
            self.layout().map_err(RuntimeError::Build)?;
        }
        self.host.reanchor_all(&mut self.tree);
        let version = (
            self.tree.content_version(),
            self.scale.to_bits(),
            self.viewport.w.to_bits(),
            self.viewport.h.to_bits(),
            fonts_revision,
        );
        self.sync_selection_bindings();
        let content_changed = self.cached_version != Some(version);
        if content_changed {
            self.sync_text_runs();
        }
        let focused_scroll = |runtime: &Self| {
            runtime
                .tree
                .focused()
                .and_then(|node| runtime.text_runs.get(&node))
                .map(|entry| entry.scroll_x.to_bits())
        };
        let before_scroll = focused_scroll(self);
        self.update_text_scroll();
        let changed = content_changed || before_scroll != focused_scroll(self);
        if changed {
            self.cached_scene = self.emit_keyed();
            self.cached_text_nodes = self
                .cached_scene
                .iter()
                .filter_map(|(key, item)| {
                    matches!(item, DisplayItem::Text { .. }).then_some(key.node())
                })
                .collect();
            self.frame_work.tree_scene_ops_emitted = self.cached_scene.len();
            self.cached_version = Some(version);
        } else {
            self.frame_work.tree_scene_reused = true;
        }
        let dynamic: Vec<_> = self
            .caret_selection_items()
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
        if !changed && self.dynamic_order == dynamic_order {
            self.frame_work.scene_items_reconciled = dynamic.len();
            for (key, item) in dynamic {
                if let Some(id) = self.scene_map.get(&key).copied() {
                    self.scene.update(id, item);
                }
            }
        } else {
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
            self.dynamic_order = dynamic_order;
        }
        let now = self.clock;
        self.host.advance_motion(now);
        Ok(
            match (self.host.motion_deadline(now), self.selection_deadline()) {
                (Some(motion), Some(selection)) => Some(motion.min(selection)),
                (motion, selection) => motion.or(selection),
            },
        )
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
        let mut r = self.tree.rect(node)?;
        let mut parent = self.tree.get(node)?.parent;
        while let Some(id) = parent {
            let record = self.tree.get(id)?;
            if let NodeKind::Scroll(scroll) = &record.kind {
                r = r.translate(-scroll.offset.x, -scroll.offset.y);
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
            let width = self.text_rect(id).map(|r| r.w).unwrap_or(0.0);

            let multiline = t.multiline();
            if self.text_runs.get(&id).is_some_and(|entry| {
                entry.fonts_revision == fonts_revision
                    && entry.paint_revision == rec.paint_rev
                    && entry.size_bits == t.font_size.to_bits()
                    && entry.wrap == (t.wrap || multiline)
                    && entry.line_multi == multiline
                    && entry.masked == (t.password && !t.is_placeholder())
                    && entry.width_bits == width.to_bits()
                    && entry.scale_bits == self.scale.to_bits()
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
                        || e.size_bits != style.size_px.to_bits()
                        || e.wrap != wrap
                        || e.line_multi != multiline
                        || e.masked != masked
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
            let build = || {
                let mut layout = TextLayout::new();
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
                layout.layout(&mut self.fonts.lock());
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
                            size_bits: style.size_px.to_bits(),
                            wrap,
                            line_multi: multiline,
                            masked,
                            scroll_x: 0.0,
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
                entry.size_bits = style.size_px.to_bits();
                entry.wrap = wrap;
                entry.line_multi = multiline;
                entry.masked = masked;
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

    fn emit_keyed(&mut self) -> Vec<(SceneKey, DisplayItem)> {
        let mut out = Vec::new();
        let mut per_node: HashMap<NodeId, u32> = HashMap::new();
        for (node, item) in self.tree.emit_scene() {
            let op = per_node.entry(node).or_insert(0);
            let key = SceneKey::Tree { node, op: *op };
            *op += 1;
            let item = match item {
                DisplayItem::Text {
                    mut at,
                    run,
                    color,
                    opacity,
                } => {
                    let (run_id, rev) = self.ensure_run_for(node);
                    let _ = run;
                    let color =
                        if self.tree.get(node).is_some_and(
                            |r| matches!(&r.kind, NodeKind::Text(t) if t.is_placeholder()),
                        ) {
                            Oklch {
                                alpha: color.alpha * 0.45,
                                ..color
                            }
                        } else {
                            color
                        };
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
            let editable = self
                .tree
                .get(node)
                .is_some_and(|rec| matches!(&rec.kind, NodeKind::Text(t) if t.is_editable()));
            let clip = if editable && matches!(item, DisplayItem::Text { .. }) {
                self.text_rect(node)
            } else {
                None
            };
            if let Some(bounds) = clip {
                out.push((
                    SceneKey::TextClipStart { node },
                    DisplayItem::PushClip {
                        bounds,
                        radius: 0.0,
                    },
                ));
            }
            out.push((key, item));
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
        let (text, style, source_style, spans, wrap, multiline, masked, width, paint_revision) =
            match self.tree.get(node).and_then(|r| match &r.kind {
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
        layout.layout(&mut self.fonts.lock());
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
                size_bits: style.size_px.to_bits(),
                wrap,
                line_multi: multiline,
                masked,
                scroll_x: 0.0,
            },
        );
        (run, r)
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
        let _ = text;
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
            let fill = Oklch::from_hex("#2f6feb")
                .map(|c| Oklch { alpha: 0.35, ..c })
                .unwrap_or(Oklch {
                    l: 0.55,
                    c: 0.12,
                    h: 260.0,
                    alpha: 0.35,
                });
            for (i, r) in layout.selection_rects(a..b).into_iter().enumerate() {
                out.push((
                    SceneKey::Selection {
                        node: focused,
                        index: i as u32,
                    },
                    DisplayItem::Rect {
                        bounds: Rect::from_edges(
                            origin.x + r.x / self.scale,
                            origin.y + r.y / self.scale,
                            origin.x + (r.x + r.w) / self.scale,
                            origin.y + (r.y + r.h) / self.scale,
                        ),
                        radius: 0.0,
                        fill,
                        border_width: 0.0,
                        border_color: Oklch::TRANSPARENT,
                        shadow: None,
                    },
                ));
            }
        }
        if let Some(range) = composition {
            let underline = Oklch::oklch(0.72, 0.14, 95.0, 1.0);
            for (i, r) in layout.selection_rects(range).into_iter().enumerate() {
                out.push((
                    SceneKey::ImeUnderline {
                        node: focused,
                        index: i as u32,
                    },
                    DisplayItem::Rect {
                        bounds: Rect::from_edges(
                            origin.x + r.x / self.scale,
                            origin.y + (r.y + r.h - 2.0) / self.scale,
                            origin.x + (r.x + r.w) / self.scale,
                            origin.y + (r.y + r.h) / self.scale,
                        ),
                        radius: 0.0,
                        fill: underline,
                        border_width: 0.0,
                        border_color: Oklch::TRANSPARENT,
                        shadow: None,
                    },
                ));
            }
        }
        if editable {
            if let Some(position) = caret_byte {
                if let Some(c) = layout.caret_position(position) {
                    out.push((
                        SceneKey::Caret { node: focused },
                        DisplayItem::Rect {
                            bounds: Rect::from_edges(
                                origin.x + c.x / self.scale,
                                origin.y + c.y_top / self.scale,
                                origin.x + (c.x + 1.5) / self.scale,
                                origin.y + (c.y_top + c.height) / self.scale,
                            ),
                            radius: 0.0,
                            fill: Oklch::WHITE,
                            border_width: 0.0,
                            border_color: Oklch::TRANSPARENT,
                            shadow: None,
                        },
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
                        DisplayItem::PushClip {
                            bounds,
                            radius: 0.0,
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
        self.scene_map.retain(|key, id| {
            if live.contains(key) {
                true
            } else {
                scene.remove(*id);
                false
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
    pub fn step(&mut self, input: Input) -> Result<Output, RuntimeError> {
        match input {
            Input::Tick(now) => {
                self.advance_clock(now.as_secs_f64());
                Ok(Output::IDLE)
            }
            Input::Viewport { size, scale } => {
                self.set_viewport(size.w, size.h, scale);
                Ok(Output {
                    redraw: FrameReason::RESIZE,
                    ..Output::IDLE
                })
            }
            Input::Event(event) => {
                let consumed = self.handle_input(&event);
                Ok(Output {
                    redraw: self.input_redraw(consumed),
                    ..Output::IDLE
                })
            }
            Input::Access(request) => {
                self.apply_access(&request);
                Ok(Output {
                    redraw: FrameReason::INPUT,
                    ..Output::IDLE
                })
            }
            Input::Frame(now) => {
                self.advance_clock(now.as_secs_f64());
                let deadline = self.build_frame()?;
                Ok(Output {
                    redraw: FrameReason::empty(),
                    wake_at: deadline.map(Instant::from_secs_f64),
                    ime_area: self.ime_anchor(),
                })
            }
        }
    }

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
                let bounds = semantic.bounds.map(|bounds| Rect {
                    x: bounds.x * self.scale,
                    y: bounds.y * self.scale,
                    w: bounds.w * self.scale,
                    h: bounds.h * self.scale,
                });
                let children = self
                    .tree
                    .get(semantic.node)
                    .map(|r| r.children.clone())
                    .unwrap_or_default();
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

    fn apply_access(&mut self, request: &A11yActionRequest) {
        if self.tree.get(request.node).is_none()
            || self.tree.is_disabled(request.node)
            || !self.text_node_allowed(request.node)
        {
            return;
        }
        match request.action {
            A11yAction::Focus => {
                let _ = self.tree.set_focused(Some(request.node));
            }
            A11yAction::Press if !self.tree.is_disabled(request.node) => {
                let _ = self.tree.set_focused(Some(request.node));
                self.tree.fire_action(ActionEvent {
                    node: request.node,
                    kind: ActionKind::Activate,
                    text: None,
                    focused: None,
                });
            }
            A11yAction::SetValue | A11yAction::ReplaceSelectedText => {
                if let A11yActionData::Value(value) = &request.data {
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
