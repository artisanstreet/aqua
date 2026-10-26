//! High-level single-window UI host: the first product path for `ui!` apps.
//!
//! [`UiApp`] owns one [`UiTree`], its reactive [`Runtime`](aqua_reactive::Runtime)
//! and root [`Scope`], the [`View`] mount, shared fonts + run registry,
//! the [`OverlayHost`], and the retained [`Scene`] reconciliation — and
//! implements [`Application`](crate::runtime::Application), so a `ui!`
//! factory becomes a runnable program via [`Aqua::run_ui`]. One window per `UiApp` (the runtime stays the
//! multi-window low-level owner); no competing text, overlay, or graph
//! logic lives here — everything delegates to the peer owners' contracts.
//!
//! Frame flow inside [`Application::build_scene`](crate::runtime::Application::build_scene):
//! flush effects → [`OverlayHost::sync_lifecycle`] → `compute_layout` →
//! re-anchor popups → sync text runs (mint/update [`RunRegistry`], GC
//! removed runs) → [`UiTree::emit_scene`] with interim runs replaced by
//! real ([`RunId`], revision) pairs → caret / selection / IME underline
//! items from real [`TextLayout`] geometry → reconcile into the retained
//! [`Scene`] by stable `(NodeId, op index)` keys → advance motion and
//! return the next animation deadline.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::runtime::ExternalFrame;
use aqua_core::{
    DisplayItem, InputEvent as CoreInputEvent, NodeId, Oklch, Point, Rect, Size, TextRun,
};
use aqua_platform::{a11y::A11yNode, AquaWindowId, FrameReason, ObservedWindowState};
use aqua_reactive::DepTuple;
use aqua_reactive::{Runtime as ReactiveRuntime, Scope};

/// Media/viewport provider for the GPU extension boundary (see
/// [`UiApp::set_externals`]).
pub type ExternalsProvider = Box<dyn FnMut(&mut ExternalFrame)>;
use aqua_render::Scene;
use aqua_text::{LineMode, Selection, SharedFonts, SharedRuns, TextLayout, TextStyle, WrapMode};
use aqua_ui::{
    binding::{MountError, View},
    element::Element,
    events,
    layout::CosmicMeasurer,
    motion::{ReducedMotion, Seconds},
    overlay_host::OverlayHost,
    tree::{ActionEvent, ActionKind, NodeKind, TextBacking, TextContent, UiTree},
};

use crate::runtime::{AppCtx, Application, Aqua};
use aqua_platform::chrome::DragRegions;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Failure to mount or refresh the UI content. Structural problems are
/// loud programming errors, never silent no-ops.
#[derive(Debug)]
pub enum UiAppError {
    /// `refresh` before any `mount`.
    NoViewMounted,
    /// `mount` when a view is already mounted (leaking the old scope
    /// would be silent disposal debt — refresh or dispose first).
    AlreadyMounted,
    /// Layout failure during [`build_frame`](UiApp::build_frame).
    Build(String),
    /// Element lowering / mount failure from the view.
    Mount(MountError),
}

impl std::fmt::Display for UiAppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UiAppError::NoViewMounted => write!(f, "no view mounted: call mount() first"),
            UiAppError::AlreadyMounted => {
                write!(f, "view already mounted: call refresh() or dispose() first")
            }
            UiAppError::Build(e) => write!(f, "ui build failed: {e}"),
            UiAppError::Mount(e) => write!(f, "ui mount failed: {e}"),
        }
    }
}

impl std::error::Error for UiAppError {}

// ---------------------------------------------------------------------------
// Scene keys
// ---------------------------------------------------------------------------

/// Stable identity for one retained scene op. Tree ops key by
/// `(node, per-node op index)` so reorderings reconcile by key, not by
/// position; caret/selection/IME items have their own keyspace and can
/// never collide with tree ops.
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

// ---------------------------------------------------------------------------
// Text runs
// ---------------------------------------------------------------------------

/// One text node queued for (re)shaping.
struct TextJob {
    node: NodeId,
    text: String,
    style: TextStyle,
    wrap: bool,
    multiline: bool,
    masked: bool,
    width: f32,
    hash: u64,
}

/// Retained shaping for one live text node.
struct TextEntry {
    run: aqua_text::RunId,
    /// Registry revision last committed to the scene.
    registry_rev: u64,
    /// Hash of the display text last shaped (committed text + IME
    /// preedit + placeholder + masking — everything `display_text()`
    /// can change, not just the committed revision).
    text_hash: u64,
    /// Layout inputs the shaping used (rebuild on change).
    width_bits: u32,
    scale_bits: u32,
    size_bits: u32,
    wrap: bool,
    line_multi: bool,
    masked: bool,
    scroll_x: f32,
}

/// Unmasked shaping source + mask mode for a text node.
///
/// Core `TextContent::display_text()` already char-masks passwords AND
/// returns the placeholder hint: shaping that string AND calling
/// `set_mask` would double-mask, destroying the original UTF-8/grapheme
/// indices (caret/selection break, visibly for emoji passwords) and
/// wrongly masking the placeholder. Instead: shape the unmasked source
/// (editor display text with preedit, or the static model text; the
/// placeholder hint when showing) and let `TextLayout::set_mask` keep the
/// source↔display coordinate mapping intact.
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

/// Hash display text for shaping cache keys.
fn hash_text(text: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

// ---------------------------------------------------------------------------
// UiApp
// ---------------------------------------------------------------------------

/// Single-window UI application host. Owns the tree, the reactive runtime,
/// the view mount, shared fonts/runs, the overlay host (which owns motion),
/// and the scene reconciliation map.
pub struct UiApp {
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
    bound_windows: HashSet<AquaWindowId>,
    /// Media/viewport provider for the GPU extension boundary (set via
    /// [`set_externals`](UiApp::set_externals), called from
    /// [`Application::externals`](Application::externals)). Lets `ui!` `Media` nodes show
    /// actual registered assets through the one host — no second host.
    externals: Option<ExternalsProvider>,
    /// Fixed custom-titlebar drag regions served by the
    /// [`Application::chrome_regions`] hook (set via
    /// [`set_chrome_regions`](UiApp::set_chrome_regions)). `None` (default)
    /// keeps the install-time regions.
    chrome_regions: Option<DragRegions>,
    /// Last build failure recorded by the [`Application`] impl (`None` when the
    /// last frame built cleanly). Headless pumps should assert this stays
    /// `None` — a scene that silently stops updating is a hung UI.
    last_build_error: Option<String>,
    /// Explicit-dependency structural queue: the `mount_reactive` effect
    /// renders fresh elements here; the frame boundary consumes them
    /// (`drain_structural`). Effects never touch the tree — the queue is
    /// the one structural seam, and targeted prop updates stay in
    /// bindings.
    structural: Rc<RefCell<Option<Element>>>,
    /// Teardown guard: [`dispose`](UiApp::dispose) and [`Drop`] share it,
    /// so cleanup runs exactly once. Note the reactive [`Scope`]s are
    /// disposed explicitly here — dropping the [`Runtime`](aqua_reactive::Runtime)
    /// alone intentionally does NOT run user cleanups.
    disposed: bool,
    t0: Instant,
}

impl UiApp {
    /// Create an unmounted host sharing the given fonts and run registry
    /// (normally `Aqua::fonts()` / `Aqua::runs()`, so shaping and pixels
    /// share one font system and one registry).
    #[must_use]
    pub fn new(fonts: SharedFonts, runs: SharedRuns) -> Self {
        let runtime = ReactiveRuntime::new();
        let scope = runtime.create_scope();
        Self {
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
            bound_windows: HashSet::new(),
            externals: None,
            chrome_regions: None,
            structural: Rc::new(RefCell::new(None)),
            last_build_error: None,
            disposed: false,
            t0: Instant::now(),
        }
    }

    /// Tear down in dependency order, exactly once (repeat calls are
    /// no-ops, and [`Drop`] funnels here):
    ///
    /// 1. bound view effects + per-node child scopes (`View::dispose`,
    ///    which owns the dedicated view scope);
    /// 2. the root app scope (user cleanups run LIFO exactly once);
    /// 3. all retained text runs leave the shared registry.
    ///
    /// The retained [`Scene`] is caller-owned (the runtime drops per-window
    /// scenes on window removal); scene keys are forgotten here. Because
    /// [`Runtime`](aqua_reactive::Runtime) drop intentionally does NOT run
    /// user cleanups, every exit path must go through this (or `Drop`).
    pub fn dispose(&mut self) {
        if self.disposed {
            return;
        }
        self.disposed = true;
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

    /// Elapsed seconds since creation: the motion clock. Caller-supplied
    /// (an `Instant` read here, never inside motion), so tests can drive
    /// the same path with explicit timestamps.
    #[must_use]
    pub fn now_secs(&self) -> Seconds {
        self.t0.elapsed().as_secs_f64()
    }

    /// Root reactive scope: create app signals from it, then pass them
    /// into the mount factory.
    #[must_use]
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    /// The owned reactive runtime (flush boundary, effects).
    #[must_use]
    pub fn runtime(&self) -> &ReactiveRuntime {
        &self.runtime
    }

    /// Retained UI tree.
    #[must_use]
    pub fn tree(&self) -> &UiTree {
        &self.tree
    }

    /// Retained UI tree, mutably (tests, popup builders).
    pub fn tree_mut(&mut self) -> &mut UiTree {
        &mut self.tree
    }

    /// Overlay + motion host.
    #[must_use]
    pub fn host(&self) -> &OverlayHost {
        &self.host
    }

    /// Overlay + motion host, mutably (popups, animations).
    pub fn host_mut(&mut self) -> &mut OverlayHost {
        &mut self.host
    }

    /// Apply the startup motion policy: when `reduced` is true, decorative
    /// motion snaps instead of animating. Callers pass the effective
    /// request — OS preference OR authoring/low-level config — so an
    /// explicit `reduced_motion` request is never silently dropped.
    /// Shared by [`Aqua::run_ui`], [`Aqua::run_ui_reactive`], and the root
    /// [`crate::App`] builder.
    pub(crate) fn apply_startup_motion_policy(&mut self, reduced: bool) {
        if reduced {
            self.host
                .motion_mut()
                .set_reduced_motion(ReducedMotion::Reduce, 0.0);
        }
    }

    /// Set the window content bounds + scale (headless pumps, tests, and
    /// the `Application` impl all funnel through here).
    pub fn set_viewport(&mut self, width: f32, height: f32, scale: f32) {
        let rect = Rect::from_edges(0.0, 0.0, width.max(0.0), height.max(0.0));
        if rect != self.viewport {
            self.viewport = rect;
            self.host.set_viewport(&mut self.tree, rect);
        }
        self.scale = scale.max(0.125);
    }

    /// Mount the UI exactly once: lower the factory's [`Element`], reconcile
    /// it into the tree, and bind effects. Later structural updates go
    /// through [`refresh`](UiApp::refresh), which preserves keyed nodes.
    pub fn mount(&mut self, factory: impl FnOnce(&Scope) -> Element) -> Result<(), UiAppError> {
        if self.view.is_some() {
            return Err(UiAppError::AlreadyMounted);
        }
        let element = factory(&self.scope);
        let sink = self.tree.command_sink();
        // Dedicated view scope (sibling of the root app scope): view
        // teardown disposes view effects without touching app signals,
        // and refresh keeps reusing it. Never the root scope itself —
        // disposing that would kill signals out from under the app.
        let mut view = View::new(self.runtime.create_scope(), sink);
        view.set_content(&mut self.tree, vec![element])
            .map_err(UiAppError::Mount)?;
        self.view = Some(view);
        Ok(())
    }

    /// Structural mount with explicit dependencies: `render` rebuilds the
    /// whole element whenever `deps` change, and the frame boundary
    /// consumes it through [`refresh`](UiApp::refresh) — keyed
    /// reconciliation preserves nodes, focus, and carets across rebuilds.
    /// Targeted prop updates (bound text, toggles) keep flowing through
    /// bindings without a rebuild; this seam is only for structural
    /// change (add/remove rows, branches).
    ///
    /// The effect owns no tree handle: it renders into a queue the frame
    /// boundary drains. Failures surface from [`sync`](UiApp::sync) and
    /// [`build_frame`](UiApp::build_frame), never silently.
    pub fn mount_reactive<D, F>(&mut self, deps: D, mut render: F) -> Result<(), UiAppError>
    where
        D: DepTuple,
        F: FnMut(D::Values) -> Element + 'static,
    {
        if self.view.is_some() {
            return Err(UiAppError::AlreadyMounted);
        }
        let sink = self.tree.command_sink();
        let view_scope = self.runtime.create_scope();
        self.view = Some(View::new(view_scope, sink));
        let pending = Rc::clone(&self.structural);
        self.scope.effect(deps, move |values| {
            *pending.borrow_mut() = Some(render(values));
        });
        // Run the initial render synchronously so content exists before
        // the first frame (effects also re-run on every dep change).
        self.runtime.flush();
        self.drain_structural()
    }

    /// Consume a queued structural element into the view (keyed refresh).
    /// No-op when the queue is empty.
    fn drain_structural(&mut self) -> Result<(), UiAppError> {
        let Some(element) = self.structural.borrow_mut().take() else {
            return Ok(());
        };
        self.refresh(vec![element])
    }

    /// Refresh content (structural updates: add/remove rows, conditional
    /// branches). Keyed reconciliation preserves nodes, focus, and carets.
    pub fn refresh(&mut self, elements: Vec<Element>) -> Result<(), UiAppError> {
        let Some(view) = self.view.as_mut() else {
            return Err(UiAppError::NoViewMounted);
        };
        view.set_content(&mut self.tree, elements)
            .map_err(UiAppError::Mount)?;
        Ok(())
    }

    /// Host-driven update boundary: flush queued effects (bindings push
    /// [`UiCommand`]s into the shared sink here), then consume any queued
    /// structural render. Headless pumps and tests drive frames through
    /// this plus [`layout`](UiApp::layout).
    pub fn sync(&mut self) -> Result<(), UiAppError> {
        self.runtime.flush();
        self.drain_structural()
    }

    /// Run layout with the production cosmic-text measurer over the shared
    /// fonts. Paint-only frames skip Taffy internally (see `FrameStats`).
    ///
    /// Delegates to the mounted [`View`] through the runtime seam
    /// `View::compute_layout(&mut UiTree, Size, &dyn TextMeasurer)`; when no
    /// view is mounted, the ordinary tree layout runs directly.
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

    /// Route one canonical input event: text input edits the focused
    /// editable directly (unless a modal captures it — see
    /// [`text_input_allowed`](UiApp::text_input_allowed)); everything else
    /// goes through the overlay host (overlay policy → tree dispatch),
    /// then lifecycle sync. Returns whether the event was consumed.
    pub fn handle_input(&mut self, event: &CoreInputEvent) -> bool {
        let consumed = match event {
            CoreInputEvent::TextInput { text } => {
                if self.text_input_allowed() {
                    events::dispatch_text_input(&mut self.tree, text)
                } else {
                    // Modal capture: the keystroke belongs to no editable.
                    // Consumed (a modal owns all input while open), not
                    // applied anywhere.
                    true
                }
            }
            _ => self.host.handle_input(&mut self.tree, event),
        };
        self.host.sync_lifecycle(&mut self.tree);
        if let CoreInputEvent::PointerDown(pe, aqua_core::PointerButton::Primary) = event {
            self.place_pointer_caret(pe.position);
        }
        consumed
    }

    /// Whether committed text may reach the focused editable: under
    /// modality, only focus inside the topmost modal's subtree — or inside
    /// a popup stacked above it (interactive per pointer policy) — may
    /// receive text. Mirrors the pointer-blocking rule for keystrokes.
    fn text_input_allowed(&self) -> bool {
        let Some(modal) = self.host.service().modal_blocker() else {
            return true;
        };
        let Some(focused) = self.tree.focused() else {
            return false;
        };
        // Walk to the window root's direct child.
        let mut node = focused;
        loop {
            let parent = self.tree.get(node).and_then(|r| r.parent);
            match parent {
                None => return false,
                Some(p) if Some(p) == self.tree.root() => break,
                Some(p) => node = p,
            }
        }
        let Some(overlay) = self.host.overlay_of_root(node) else {
            // Focus is in main UI beneath a modal: blocked.
            return false;
        };
        // Allowed at the modal level and above (stack is bottom → top).
        let stack = self.host.service().stack();
        let modal_pos = stack.iter().position(|o| *o == modal).unwrap_or(0);
        stack
            .iter()
            .position(|o| *o == overlay)
            .is_some_and(|pos| pos >= modal_pos)
    }

    /// Caret rectangle for the platform IME cursor area, in window-local
    /// logical units. The facade forwards this to
    /// `set_ime_cursor_area`; `None` when no editable has a caret.
    ///
    /// Shared geometry for the platform IME hook and the painted caret.
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
        let caret_byte = t.caret()?;
        let origin = self.text_origin(focused)?;
        let entry = self.text_runs.get(&focused)?;
        let guard = self.runs.lock();
        let handle = guard.resolve(entry.run)?;
        let layout = handle.layout;
        let caret = layout.caret(caret_byte)?;
        Some(Rect::from_edges(
            origin.x + caret.x / self.scale,
            origin.y + caret.y_top / self.scale,
            origin.x + (caret.x + 2.0) / self.scale,
            origin.y + (caret.y_top + caret.height) / self.scale,
        ))
    }

    /// Build (or rebuild) window content into the retained [`Scene`]:
    /// flush → lifecycle sync → layout → re-anchor → text runs → emit →
    /// caret/selection/IME → reconcile → motion advance. Returns the next
    /// animation deadline (`None` when idle), or the layout failure.
    /// Failures are returned (not just logged) so headless pumps and the
    /// facade can react; the [`Application`] impl records them for
    /// [`build_error`](UiApp::build_error).
    pub fn build_scene_into(
        &mut self,
        scene: &mut Scene,
        window: &ObservedWindowState,
    ) -> Result<Option<Instant>, String> {
        self.set_viewport(
            window.inner_size.width as f32,
            window.inner_size.height as f32,
            window.scale_factor as f32,
        );
        self.build_frame(scene).map_err(|e| {
            let msg = e.to_string();
            self.last_build_error = Some(msg.clone());
            eprintln!("aqua: build failed: {msg}");
            msg
        })
    }

    /// One headless frame against the already-set viewport: flush →
    /// structural drain → lifecycle sync → layout → re-anchor → text runs
    /// → emit → caret/selection/IME → reconcile → motion advance. Returns
    /// the next animation deadline (`None` when idle), or the first
    /// failure. The [`Application`] impl funnels here after applying the observed
    /// window size.
    pub fn build_frame(&mut self, scene: &mut Scene) -> Result<Option<Instant>, UiAppError> {
        self.runtime.flush();
        self.drain_structural()?;
        self.host.sync_lifecycle(&mut self.tree);
        self.layout().map_err(UiAppError::Build)?;
        self.host.reanchor_all(&mut self.tree);
        self.sync_text_runs();
        self.update_text_scroll();
        let mut keyed = self.emit_keyed();
        keyed.extend(self.caret_selection_items());
        self.reconcile(scene, keyed);
        let now_secs = self.now_secs();
        self.host.advance_motion(now_secs);
        // Relative deadline: `motion_deadline_duration` is already
        // now-relative, so no elapsed-doubling (`Instant::now + d`).
        Ok(self
            .host
            .motion_deadline_duration(Duration::from_secs_f64(now_secs))
            .map(|d| Instant::now() + d))
    }

    /// Last build failure recorded by the [`Application`] impl (`None` when the
    /// last frame built cleanly). Headless pumps should assert this stays
    /// `None` — a scene that silently stops updating is a hung UI.
    #[must_use]
    pub fn build_error(&self) -> Option<&str> {
        self.last_build_error.as_deref()
    }

    /// Dirty statistics from the last layout (paint-only frames skip
    /// Taffy; hosts gate scene re-emission on the content version).
    #[must_use]
    pub fn frame_stats(&self) -> aqua_ui::tree::FrameStats {
        self.tree.last_stats()
    }

    // -- internals --------------------------------------------------------

    /// Content geometry is logical; shaping geometry is physical at the current scale.
    fn text_rect(&self, node: NodeId) -> Option<Rect> {
        let r = self.tree.rect(node)?;
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
        let Some(byte) = t.caret() else { return };
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
        let Some(caret) = run.layout.caret(byte) else {
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
        let mut live = HashSet::new();
        let mut jobs: Vec<TextJob> = Vec::new();
        for id in self.tree.traverse() {
            let Some(rec) = self.tree.get(id) else {
                continue;
            };
            let NodeKind::Text(t) = &rec.kind else {
                continue;
            };
            live.insert(id.raw());
            let width = self.text_rect(id).map(|r| r.w).unwrap_or(0.0);
            let style = TextStyle {
                size_px: t.font_size,
                line_height_px: t.font_size * 1.2,
                ..Default::default()
            };
            let multiline = t.multiline();
            // Unmasked shaping source (committed + preedit, or the
            // placeholder hint when showing); masking stays a layout
            // mode so source coordinates survive for caret/selection.
            let (text, masked) = shaping_source(t);
            let text_hash = hash_text(&text);
            jobs.push(TextJob {
                node: id,
                text,
                style,
                wrap: t.wrap || multiline,
                multiline,
                masked,
                width,
                hash: text_hash,
            });
        }
        for TextJob {
            node: id,
            text,
            style,
            wrap,
            multiline,
            masked,
            width,
            hash: text_hash,
        } in jobs
        {
            let needs = match self.text_runs.get(&id) {
                Some(e) => {
                    e.text_hash != text_hash
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
                continue;
            }
            // Built twice only on the rare re-mint path (TextLayout is not
            // Clone); the retained result is reused across frames.
            let build = || {
                let mut layout = TextLayout::new();
                layout.set_text(&text, &style);
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
            // Scope the update result before matching: the match
            // scrutinee would otherwise hold the registry mutex guard
            // across the re-mint lock in the None arm and deadlock.
            let updated = self
                .text_runs
                .get(&id)
                .map(|e| (e.run, self.runs.lock().update(e.run, build())));
            let registry_rev = match updated {
                Some((_, Some(r))) => r,
                Some((stale, None)) => {
                    // Registry lost the run (should not happen while the
                    // UI owns removals); re-mint so text stays visible.
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
                            run,
                            registry_rev: r,
                            text_hash,
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
                entry.registry_rev = registry_rev;
                entry.text_hash = text_hash;
                entry.width_bits = width.to_bits();
                entry.scale_bits = self.scale.to_bits();
                entry.size_bits = style.size_px.to_bits();
                entry.wrap = wrap;
                entry.line_multi = multiline;
                entry.masked = masked;
            }
        }
        // GC: runs of unmounted or non-text nodes leave the registry.
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

    /// Emit the tree scene, swapping interim run ids for real
    /// ([`RunId`], revision) pairs minted above. Nodes lacking a run are
    /// shaped on demand so the scene never references a dead run.
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
            // Media layers arrive canonical from the tree: `emit_scene`
            // calls `aqua_core::scene::media_layer(image)` (tag +
            // generation + index), matching the renderer asset cache.
            // No UiApp remap (an earlier cut mapped raw slot indices here;
            // core has since deduplicated the helper — see contract).
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

    /// Real ([`RunId`], revision) for a text node, shaping on demand.
    fn ensure_run_for(&mut self, node: NodeId) -> (aqua_text::RunId, u64) {
        if let Some(e) = self.text_runs.get(&node) {
            return (e.run, e.registry_rev);
        }
        // On-demand path (emit raced a brand-new node): shape the
        // display text now (committed + preedit + placeholder).
        let (text, style, wrap, multiline, masked, width, text_hash) =
            match self.tree.get(node).and_then(|r| match &r.kind {
                NodeKind::Text(t) => {
                    let style = TextStyle {
                        size_px: t.font_size,
                        line_height_px: t.font_size * 1.2,
                        ..Default::default()
                    };
                    let (display, masked) = shaping_source(t);
                    let hash = hash_text(&display);
                    Some((
                        display,
                        style,
                        t.wrap || t.multiline(),
                        t.multiline(),
                        masked,
                        self.text_rect(node).map(|r| r.w).unwrap_or(0.0),
                        hash,
                    ))
                }
                _ => None,
            }) {
                Some(v) => v,
                None => return (aqua_text::RunId(0), 0),
            };
        let mut layout = TextLayout::new();
        layout.set_text(&text, &style);
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
                run,
                registry_rev: r,
                text_hash,
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

    /// Caret bar, selection highlights, and IME composition underline for
    /// the focused text node, from real [`TextLayout`] geometry resolved
    /// through the run registry.
    fn caret_selection_items(&self) -> Vec<(SceneKey, DisplayItem)> {
        let Some(focused) = self.tree.focused() else {
            return Vec::new();
        };
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
                        t.caret(),
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
        // Selection highlights first (paint under the caret).
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
        // IME composition underline.
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
        // Caret bar (editable focus only).
        if editable {
            if let Some(byte) = caret_byte {
                if let Some(c) = layout.caret(byte) {
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

    /// Reconcile keyed items into the retained [`Scene`]: identical key
    /// order updates in place (identical items are renderer no-ops);
    /// any structural change rebuilds in order. Returns nothing; the
    /// scene owns presentation, this map owns identity.
    fn reconcile(&mut self, scene: &mut Scene, items: Vec<(SceneKey, DisplayItem)>) {
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
        for (_, id) in self.scene_map.drain() {
            scene.remove(id);
        }
        self.scene_map.clear();
        for (key, item) in items {
            let id = scene.insert(item);
            self.scene_map.insert(key, id);
        }
        self.scene_order = order;
    }
}

impl Drop for UiApp {
    fn drop(&mut self) {
        self.dispose();
    }
}

impl UiApp {
    /// Register the media/viewport provider for the GPU extension
    /// boundary. Called from the [`Application::externals`] override before every
    /// `build_scene`, so `ui!` `Media` nodes show actual registered assets
    /// through the one host — no second host, no forked renderer path.
    /// `None` clears it.
    pub fn set_externals(&mut self, provider: Option<ExternalsProvider>) {
        self.externals = provider;
    }

    /// Set fixed custom-titlebar drag regions served by the
    /// [`Application::chrome_regions`] hook: extra draggable rects plus
    /// `no_drag` holes for interactive header widgets, in client-local
    /// logical px. `None` (default) keeps the install-time regions; setting
    /// `None` after `Some` clears explicit rects back to the default drag
    /// band. Advanced API — the root builder may add `Context` convenience
    /// later; reactivity-driven regions belong in a custom `Application`.
    pub fn set_chrome_regions(&mut self, regions: Option<DragRegions>) {
        self.chrome_regions = regions;
    }

    /// Number of live text runs retained in the shared registry.
    #[must_use]
    pub fn text_run_count(&self) -> usize {
        self.text_runs.len()
    }

    fn place_pointer_caret(&mut self, at: Point) {
        use aqua_ui::events::hit_test;
        let Some(focused) = self.tree.focused() else {
            return;
        };
        if hit_test(&self.tree, at) != Some(focused) {
            return;
        }
        let origin = match self.text_origin(focused) {
            Some(r) => r,
            None => return,
        };
        let entry = match self.text_runs.get(&focused) {
            Some(e) => e,
            None => return,
        };
        let guard = self.runs.lock();
        let layout = match guard.resolve(entry.run) {
            Some(h) => h.layout,
            None => return,
        };
        // Hit testing uses the physical shaping coordinates; pointer
        // positions and layout rectangles are logical window coordinates.
        let byte = layout.hit(
            (at.x - origin.x) * self.scale,
            (at.y - origin.y) * self.scale,
        );
        drop(guard);
        if let Some(byte) = byte {
            self.tree.edit_text(focused, |tc| {
                if let TextBacking::Editable(ref mut ed) = tc.backing {
                    ed.set_caret(byte);
                }
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Application impl
// ---------------------------------------------------------------------------

impl Application for UiApp {
    fn on_window_event(
        &mut self,
        ctx: &mut AppCtx,
        id: AquaWindowId,
        event: &aqua_platform::WindowEvent,
    ) {
        // Host-driven reactive binding: first sight of a window binds this
        // app's runtime with a wake proxy; dropping the runtime degrades to
        // diagnostics, never UB (see AppCtx::bind_reactive).
        if !self.bound_windows.contains(&id) {
            ctx.bind_reactive(id, &self.runtime);
            self.bound_windows.insert(id);
        }
        match event {
            aqua_platform::WindowEvent::Resized(_, size) => {
                self.set_viewport(size.width as f32, size.height as f32, self.scale);
            }
            aqua_platform::WindowEvent::ScaleChanged(_, s) => {
                let (w, h) = (self.viewport.w, self.viewport.h);
                self.set_viewport(w, h, *s as f32);
            }
            _ => {}
        }
    }

    fn on_input(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &CoreInputEvent) {
        if self.handle_input(event) {
            ctx.request_frame(id, FrameReason::INPUT);
        }
        // Motion started by the input above needs a wake even when the
        // tree is otherwise quiet.
        if self.host.motion_deadline(self.now_secs()).is_some() {
            ctx.request_frame(id, FrameReason::ANIMATION);
        }
        // Pointer caret placement follows focus changes from the press
        // (shaped geometry, grapheme-snapped by the editor).
    }

    /// Move the caret of the focused editable to a pointer position, in
    /// layout space, snapped to grapheme boundaries by the editor. Only
    /// when the press actually hit the focused node (background presses
    /// must not yank the caret). Drag-select, double-click word select,
    /// and touch handles are future work (documented gap).
    /// Assistive-technology action routing: Focus moves real tree focus
    /// (validated: liveness + focusability); Press focuses then fires the
    /// real `Activate` binding, so an AT click runs the same handler as a
    /// pointer click — never a dropped or duplicate path. Disabled nodes
    /// ignore Press (visible but non-interactive). Anything else is
    /// unhandled (no framework behavior exists for it yet).
    fn on_a11y_action(
        &mut self,
        _ctx: &mut AppCtx,
        _id: AquaWindowId,
        request: &aqua_platform::a11y::A11yActionRequest,
    ) {
        use aqua_platform::a11y::A11yAction;
        if self.tree.get(request.node).is_none() {
            return;
        }
        match request.action {
            A11yAction::Focus => {
                let _ = self.tree.set_focused(Some(request.node));
            }
            A11yAction::Press => {
                if self.tree.is_disabled(request.node) {
                    return;
                }
                let _ = self.tree.set_focused(Some(request.node));
                self.tree.fire_action(ActionEvent {
                    node: request.node,
                    kind: ActionKind::Activate,
                    text: None,
                    focused: None,
                });
            }
            _ => {}
        }
        // AT text selection (character indices in the request payload):
        // convert to byte indices against the live display text and apply
        // through the editor (grapheme snap + clamp + preedit cancel).
        // Pointer caret placement lives in `place_pointer_caret`; drag
        // paths share `set_selection` once gesture disambiguation lands.
        if let aqua_platform::a11y::A11yActionData::TextSelection {
            focus_node,
            focus_index,
            anchor_node,
            anchor_index,
        } = &request.data
        {
            if *focus_node != request.node {
                return;
            }
            let (display, editable) = match self.tree.get(request.node) {
                Some(rec) => match &rec.kind {
                    NodeKind::Text(t) => (t.display_text(), t.is_editable()),
                    _ => return,
                },
                None => return,
            };
            if !editable {
                return;
            }
            let to_byte = |char_index: usize| {
                display
                    .char_indices()
                    .nth(char_index)
                    .map(|(b, _)| b)
                    .unwrap_or(display.len())
            };
            let selection =
                aqua_text::Selection::new(to_byte(*anchor_index), to_byte(*focus_index));
            let _ = self.tree.set_focused(Some(request.node));
            self.tree.edit_text(request.node, |tc| {
                if let TextBacking::Editable(ref mut ed) = tc.backing {
                    ed.set_selection(selection);
                }
            });
            let _ = anchor_node;
        }
    }

    fn externals(&mut self, frame: &mut crate::runtime::ExternalFrame<'_>) {
        if let Some(provider) = self.externals.as_mut() {
            provider(frame);
        }
    }

    /// IME candidate-window anchor from the shaped caret geometry. The
    /// runtime forwards this to the OS via `set_ime_cursor_area` after
    /// every scene build (facade contract `ime_cursor_area`).
    fn ime_cursor_area(&self) -> Option<aqua_core::Rect> {
        self.ime_anchor()
    }

    fn chrome_regions(&self, _window: &ObservedWindowState) -> Option<DragRegions> {
        self.chrome_regions.clone()
    }

    fn a11y_nodes(&self) -> Option<(NodeId, NodeId, Vec<A11yNode>)> {
        let root = self.tree.root()?;
        let focused = self.tree.focused().unwrap_or(root);
        let items = self
            .tree
            .emit_semantics()
            .into_iter()
            .map(|semantic| {
                let bounds = self.tree.rect(semantic.node);
                let children = self
                    .tree
                    .get(semantic.node)
                    .map(|r| r.children.clone())
                    .unwrap_or_default();
                A11yNode {
                    semantic,
                    bounds,
                    children,
                }
            })
            .collect();
        Some((root, focused, items))
    }

    fn build_scene(&mut self, scene: &mut Scene, window: &ObservedWindowState) -> Option<Instant> {
        match self.build_scene_into(scene, window) {
            Ok(deadline) => {
                self.last_build_error = None;
                deadline
            }
            Err(e) => {
                self.last_build_error = Some(e);
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Aqua entry point
// ---------------------------------------------------------------------------

impl Aqua {
    /// Run a `ui!` application on one window: mount the factory's element,
    /// then enter the GUI event loop with the [`UiApp`] endpoint. Uses the
    /// shared [`Aqua::fonts`] / [`Aqua::runs`] so shaping and pixels agree.
    pub fn run_ui(self, factory: impl FnOnce(&Scope) -> Element) -> Result<(), String> {
        let mut app = UiApp::new(self.fonts().clone(), self.runs().clone());
        app.apply_startup_motion_policy(self.motion_reduced());
        app.mount(factory)
            .map_err(|e| format!("ui mount failed: {e}"))?;
        self.run(app)
    }

    /// Run a reactive `ui!` application on one window: `setup` builds
    /// scope-owned signals and returns the explicit dependency tuple plus
    /// the structural render closure; the host rebuilds (keyed) on every
    /// dep change while bindings handle targeted updates. Same shared
    /// fonts/runs contract as [`run_ui`](Aqua::run_ui).
    pub fn run_ui_reactive<D, F>(self, setup: impl FnOnce(&Scope) -> (D, F)) -> Result<(), String>
    where
        D: DepTuple,
        F: FnMut(D::Values) -> Element + 'static,
    {
        let mut app = UiApp::new(self.fonts().clone(), self.runs().clone());
        app.apply_startup_motion_policy(self.motion_reduced());
        let (deps, render) = setup(app.scope());
        app.mount_reactive(deps, render)
            .map_err(|e| format!("ui mount failed: {e}"))?;
        self.run(app)
    }
}

#[cfg(test)]
mod style_decoration_tests {
    use super::*;
    use aqua_ui::element::ui::{Box as UiBox, Text as UiText};
    use aqua_ui::style::{px, Style};

    #[test]
    fn caret_respects_own_and_ancestor_style_opacity() {
        let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
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
        app.build_frame(&mut Scene::new()).unwrap();
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

#[cfg(test)]
mod chrome_regions_tests {
    use super::*;
    use aqua_platform::geometry::LogicalSize;

    fn observed() -> ObservedWindowState {
        ObservedWindowState::new(
            AquaWindowId::new(1).expect("test window id"),
            "chrome-test".to_string(),
            LogicalSize::new(800.0, 600.0),
        )
    }

    #[test]
    fn fixed_chrome_regions_round_trip_through_hook() {
        let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
        // Default: no explicit regions (install-time regions stand).
        assert!(app.chrome_regions(&observed()).is_none());
        let regions = DragRegions {
            draggable: vec![(0.0, 0.0, 200.0, 40.0)],
            no_drag: vec![(160.0, 4.0, 196.0, 36.0)],
        };
        app.set_chrome_regions(Some(regions.clone()));
        assert_eq!(app.chrome_regions(&observed()), Some(regions));
        // Clearing serves None again (runtime pushes empty rects).
        app.set_chrome_regions(None);
        assert!(app.chrome_regions(&observed()).is_none());
    }
}
