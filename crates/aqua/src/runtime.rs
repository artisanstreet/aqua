//! Application runtime: event loop orchestration on peer contracts.
//!
//! The runtime owns NO UI tree, text model, overlay, motion, or scene
//! vocabulary — those belong to `aqua-ui`, `aqua-text`, `aqua-render`, and
//! `aqua-core`. It owns exactly: window lifetimes, event routing, the
//! platform [`Scheduler`](aqua_platform::Scheduler), GPU surface setup, and
//! the ready → produce → present cycle.
//!
//! Event flow per winit occurrence:
//! `host::translate_window_event` → framework [`WindowEvent`]; the `None`
//! arm (input/IME/scale detail) routes through `events::translate_*` and is
//! converted to canonical [`aqua_core::event::InputEvent`] — including real
//! capture-loss cancel. [`window_event_frame_reason`] decides scheduling.
//! Nothing here invents event semantics.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aqua_core::InputEvent as CoreInputEvent;
use aqua_core::{Modifiers as CoreModifiers, PointerKind};
#[cfg(windows)]
use aqua_platform::a11y::build_tree_update;
#[cfg(windows)]
use aqua_platform::a11y_winit::A11yAdapter;
use aqua_platform::chrome::DragRegions;
use aqua_platform::events::{
    pointer_contact, to_core_point, translate_file_event, translate_ime_core, translate_key_event,
    translate_modifiers, translate_mouse_button_core, window_event_frame_reason, FileDrop,
};
use aqua_platform::host::{
    control_flow_for_next_wakeup, surface_signal, translate_window_event, window_attributes,
};
use aqua_platform::{
    a11y::{A11yActionRequest, A11yNode},
    prefs::{prefers_opaque_frame, query_preferences, OsPreferences},
    services::{app_clipboard, Clipboard, ClipboardBackend},
    AquaWindowId, FrameReason, ObservedWindowState, Scheduler, WindowEvent,
};
use aqua_reactive::{FlushReport, Runtime as ReactiveRuntime};
use aqua_render::{Renderer, Scene, SurfaceFacade};

use crate::config::AquaConfig;

/// User wake event for cross-thread reactive notification.
///
/// Source writes from worker threads must wake the UI executor through the
/// event-loop proxy (`EventLoopProxy::send_event`) — never by polling.
/// The payload names the window whose bound runtime queued effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AquaUserEvent {
    ReactiveWake(AquaWindowId),
    /// Assistive-technology actions arrived while idle (bridge waker):
    /// drain them into UI behaviors now.
    A11yActions(AquaWindowId),
    /// GPU completion for one window's submission sequence: all submissions
    /// numbered `<= seq` have completed. Releases scheduler in-flight slots.
    /// Stale ids (closed window) are ignored; device loss force-releases via
    /// the error path instead of waiting on callbacks that may never fire.
    GpuComplete(AquaWindowId, u64),
}

/// Per-frame context for [`Application::externals`]: the Viewport3D / custom
/// renderer extension boundary. The app borrows the shared wgpu
/// device/queue (cheap `Arc` clones inside) to run offscreen media and
/// viewport passes, then feeds sampled views back through
/// [`ExternalFrame::register_external`] every frame shown. No private
/// framework fork, no second queue, no blocking reads in production paths.
pub struct ExternalFrame<'a> {
    /// Shared logical device (cloned handle, not a new device).
    pub device: &'a wgpu::Device,
    /// Shared queue (never create a second one).
    pub queue: &'a wgpu::Queue,
    /// Renderer working color format for offscreen targets.
    pub color_format: wgpu::TextureFormat,
    /// Physical pixels of the window surface.
    pub width: u32,
    /// Physical pixels of the window surface.
    pub height: u32,
    /// UI scale (logical px * scale = physical px).
    pub scale: f32,
    register: &'a mut dyn FnMut(u64, wgpu::TextureView, u32, u32),
}

impl ExternalFrame<'_> {
    /// Feed one sampled view for this frame (re-register every frame shown;
    /// the renderer only samples). Views must stay alive through `draw`.
    pub fn register_external(&mut self, layer: u64, view: wgpu::TextureView, w: u32, h: u32) {
        (self.register)(layer, view, w, h)
    }
}

/// Per-window runtime record.
struct WindowRecord {
    observed: ObservedWindowState,
    winit_id: Option<winit::window::WindowId>,
    /// Custom-titlebar guard (`aqua-platform::chrome::WindowsChrome`):
    /// installed after window creation, before the window is shown, and
    /// retained here until destruction. Declared BEFORE `window` so it
    /// drops first — the guard must release/restore native chrome state
    /// while the HWND is still alive. `None` for headless windows and
    /// whenever install falls back to the native titlebar.
    chrome: Option<aqua_platform::chrome::WindowsChrome>,
    window: Option<Arc<winit::window::Window>>,
    /// Whether the retained guard backs an active overlay (`effective_titlebar
    /// == Overlay` at install). Native/Hidden installs retain a guard but
    /// draw no custom chrome — [`Aqua::has_chrome`] reports this flag, never
    /// bare retention. Plain data — no drop-ordering role.
    chrome_overlay: bool,
    /// Last explicit drag regions pushed to the chrome guard plus the scale
    /// they were converted with (`None` = the install-time defaults are in
    /// effect). The guard stores PHYSICAL rects, so equal logical regions
    /// at a new scale must be re-pushed. Compared per frame so clean frames
    /// push nothing; a hook flipping `Some` → `None` pushes empty rects to
    /// clear stale holes. Plain data — no drop-ordering role.
    last_regions: Option<(DragRegions, f64)>,
    gpu: Option<GpuWindow>,
    cursor: Option<(f64, f64)>,
    /// Buttons currently held (bit 0 primary, 1 secondary, 2 middle, 3+ extra).
    buttons: u8,
    /// Last modifier state (`ModifiersChanged`); routed into key events.
    modifiers: CoreModifiers,
    /// IME composition active: committed key text is suppressed while set
    /// (IME text travels via `Commit` instead).
    ime_active: bool,
    /// Live accessibility bridge (`cfg(windows)` only).
    #[cfg(windows)]
    a11y: Option<A11yAdapter>,
}

struct GpuWindow {
    surface: SurfaceFacade,
    renderer: Renderer,
    scene: Scene,
    ledger: GpuLedger,
}

/// Actual GPU submission accounting per window. `mark_presented` (the
/// scheduler's in-flight release) fires on *completion* reported by the
/// dedicated worker thread — never at submit: `frame.present()` only queues
/// work, so releasing at submit would let a 900 Hz Immediate loop outrun
/// the GPU without bound.
#[derive(Debug, Default)]
struct GpuLedger {
    /// Submission sequence number (1-based count of submitted frames).
    submitted: u64,
    /// Highest completed sequence (from the completion worker).
    completed: u64,
    /// Present requests submitted to the surface.
    presented: u64,
    /// Scheduler slots held awaiting completion.
    unreleased: u32,
}

struct SharedGpu {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

/// Handle handed to [`Application`] callbacks: frame demand, reactive binding,
/// clipboard, and ordered shutdown.
pub struct AppCtx<'a> {
    scheduler: &'a mut Scheduler,
    quit_requested: &'a mut bool,
    proxy: Option<winit::event_loop::EventLoopProxy<AquaUserEvent>>,
    flushers: &'a mut HashMap<AquaWindowId, Box<dyn FnMut() -> FlushReport>>,
    clipboard: &'a mut Box<dyn Clipboard>,
}

impl AppCtx<'_> {
    pub fn request_frame(&mut self, id: AquaWindowId, reason: FrameReason) {
        self.scheduler.request_frame(id, reason);
    }

    pub fn request_animation_deadline(&mut self, id: AquaWindowId, at: Instant) {
        self.scheduler.request_animation_deadline(id, at);
    }

    /// Application clipboard: native backend when available, explicit
    /// in-memory fallback otherwise. All operations are fallible and
    /// reported — never a silent no-op.
    pub fn clipboard(&mut self) -> &mut dyn Clipboard {
        &mut **self.clipboard
    }

    /// Bind an app-owned [`ReactiveRuntime`] to a window with a host-driven
    /// update boundary: switches the runtime to `auto_flush(false)`,
    /// installs a wake callback that pings the UI thread through the
    /// event-loop proxy, and registers the runtime for flushing at the draw
    /// boundary. The app must keep owning the `Runtime` (handles are `Weak`;
    /// dropping it diagnoses `RuntimeDropped`, never UB). Headless use:
    /// bind, then `flush()` manually after state updates — no proxy needed.
    pub fn bind_reactive(&mut self, window: AquaWindowId, rt: &ReactiveRuntime) {
        rt.set_auto_flush(false);
        let flusher = rt.clone();
        self.flushers
            .insert(window, Box::new(move || flusher.flush()));
        let proxy = self.proxy.clone();
        rt.set_wake_callback(move || {
            if let Some(p) = &proxy {
                let _ = p.send_event(AquaUserEvent::ReactiveWake(window));
            }
        });
    }

    /// Ordered shutdown: finish the current dispatch, then exit the loop.
    pub fn quit(&mut self) {
        *self.quit_requested = true;
    }
}

/// Low-level application endpoint (one callback object per event loop).
/// The runtime delivers canonical [`aqua_core::event`] vocabulary: window
/// events from the platform translation, input already converted to
/// [`CoreInputEvent`] (including real capture-loss cancel, committed key
/// text exactly once, and IME suppression — see `route_input`). Scene
/// content is committed into the render-owned [`Scene`] (retained by the
/// runtime per window — `update` with identical items is a free no-op, so
/// rebuilding descriptors per frame is safe).
///
/// Most apps should use the root [`crate::App`] builder instead: it mounts a
/// `ui!` [`Element`](aqua_ui::element::Element) (or an explicit
/// [`crate::ReactiveView`]) through [`crate::UiApp`] and drives this trait
/// for you. Implement `Application` directly only for custom low-level
/// endpoints. Existing [`Aqua::run`] / [`Aqua::run_ui`] /
/// [`Aqua::run_ui_reactive`] entry points are unchanged.
pub trait Application {
    fn on_window_event(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &WindowEvent);
    fn on_input(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &CoreInputEvent);
    /// OS file drop onto a window (OLE / portal paths). Default ignores.
    fn on_files_dropped(
        &mut self,
        _ctx: &mut AppCtx,
        _id: AquaWindowId,
        _paths: &[std::path::PathBuf],
    ) {
    }
    /// Assistive-technology action request (Press/Focus/...) for a semantic
    /// node. Default ignores; UI owners route to focus/activation behaviors.
    fn on_a11y_action(
        &mut self,
        _ctx: &mut AppCtx,
        _id: AquaWindowId,
        _request: &A11yActionRequest,
    ) {
    }
    /// Semantic snapshot for the accessibility tree: root, focused node, and
    /// positioned nodes. `None` (default) publishes nothing.
    fn a11y_nodes(&self) -> Option<(aqua_core::NodeId, aqua_core::NodeId, Vec<A11yNode>)> {
        None
    }
    /// IME candidate-window anchor in window-local logical units, or `None`
    /// when no editor has focus. The runtime forwards it to the OS via
    /// `set_ime_cursor_area` after every scene build. UiApp owners: implement
    /// this from your shaped caret (`ime_anchor`); the hook name
    /// `ime_cursor_area` is the agreed facade contract.
    fn ime_cursor_area(&self) -> Option<aqua_core::Rect> {
        None
    }
    /// Custom-titlebar drag regions for `window`: extra draggable rects plus
    /// `no_drag` holes for interactive header widgets, in client-local
    /// logical px (the runtime converts with the current scale). `None`
    /// (default) keeps the install-time regions — and, after previously
    /// returning `Some`, clears explicit rects back to the default drag
    /// band rather than leaving stale holes. Applied after every
    /// scene/layout commit, only while a live chrome guard is retained.
    fn chrome_regions(&self, _window: &ObservedWindowState) -> Option<DragRegions> {
        None
    }
    /// GPU extension boundary (Viewport3D, media, custom passes): run
    /// offscreen passes against the shared device/queue and register sampled
    /// views for this frame. Default ignores. Called before `build_scene` so
    /// committed `External` items reference freshly registered layers.
    fn externals(&mut self, _frame: &mut ExternalFrame<'_>) {}
    /// Rebuild window content into the retained [`Scene`]. May return the
    /// next animation deadline: the runtime registers it with the platform
    /// scheduler so motion progresses without input. Return `None` when
    /// settled (idle windows schedule nothing). Deadlines must be in the
    /// future; a perpetually-past deadline would keep the loop awake.
    fn build_scene(
        &mut self,
        scene: &mut Scene,
        window: &ObservedWindowState,
    ) -> Option<std::time::Instant>;
    fn on_render_error(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, _message: &str) {}
}

/// Legacy alias for the renamed low-level callback trait.
///
/// `Application` is canonical. This alias exists only so `runtime::App`
/// paths keep resolving during migration; it must NOT be re-exported at the
/// crate root, where `App` is the new builder (see [`crate::App`]).
#[deprecated(
    since = "0.1.0",
    note = "low-level callback trait renamed to `Application`; root `App` is now the builder"
)]
pub use self::Application as App;

/// Per-frame chrome-region decision for one window.
#[derive(Debug)]
enum ChromePlan {
    /// Push these explicit rects to the guard.
    Apply(DragRegions),
    /// The hook went quiet after explicit rects were pushed: push empty
    /// rects so stale holes don't linger. The install-time drag band is
    /// separate state and persists.
    Clear,
}

/// Decide whether explicit drag rects need pushing: only when the hook's
/// value differs from what was last pushed, or when the scale changed (the
/// guard stores PHYSICAL rects, so equal logical regions at a new scale
/// must be re-pushed). `None` after `Some` yields [`ChromePlan::Clear`]
/// (never a stale no-op); equal values at equal scale yield no work, so
/// clean frames allocate and apply nothing. Non-finite scales normalize
/// to 1.0, matching the guard's own clamp.
fn chrome_plan(
    cached: &Option<(DragRegions, f64)>,
    next: Option<DragRegions>,
    scale: f64,
) -> Option<ChromePlan> {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    match (cached, &next) {
        (Some((committed, committed_scale)), Some(n))
            if committed == n && *committed_scale == scale =>
        {
            None
        }
        (None, None) => None,
        _ => match next {
            Some(regions) => Some(ChromePlan::Apply(regions)),
            None => Some(ChromePlan::Clear),
        },
    }
}

/// The Aqua application runtime.
pub struct Aqua {
    config: AquaConfig,
    scheduler: Scheduler,
    windows: HashMap<AquaWindowId, WindowRecord>,
    winit_map: HashMap<winit::window::WindowId, AquaWindowId>,
    next_id: u64,
    shared_gpu: Option<SharedGpu>,
    quit_requested: bool,
    render_errors: u64,
    proxy: Option<winit::event_loop::EventLoopProxy<AquaUserEvent>>,
    flushers: HashMap<AquaWindowId, Box<dyn FnMut() -> FlushReport>>,
    /// Completion driver mailbox. The worker thread blocks in
    /// `device.poll(Wait)` per submission (off the UI thread) and wakes the
    /// loop with `GpuComplete`; it parks on `recv` when nothing is in
    /// flight. Created lazily on the first submitted frame — one thread per
    /// device, never per frame.
    completion_tx: Option<std::sync::mpsc::Sender<(AquaWindowId, u64)>>,
    /// The one shared font set for shaping (apps) and glyph rasterization
    /// (renderer, via [`TextBackend`](aqua_render::text::TextBackend)).
    /// Headless-safe: no GPU involved.
    fonts: aqua_text::SharedFonts,
    /// The one shared run registry: apps mint `TextRun`s here, the renderer
    /// resolves them here. UI writes the registry BEFORE the scene commits,
    /// so a scene revision never outruns its layout.
    runs: aqua_text::SharedRuns,
    /// OS preferences snapshot (prefs drive material fallback + motion
    /// policy). Theme updates arrive via `WindowEvent::ThemeChanged`.
    prefs: OsPreferences,
    /// Application clipboard: native backend when available, explicit
    /// in-memory fallback otherwise (reported, never silent).
    clipboard: Box<dyn Clipboard>,
    clipboard_backend: ClipboardBackend,
    /// Presented-frame budget from `AQUA_SMOKE_FRAMES` (validation only):
    /// the loop exits cleanly after this many presented frames so CI/WSLg
    /// smoke runs terminate without user input. For mostly-static apps use
    /// `AQUA_SMOKE_MS` instead: frame budgets never finish without active
    /// animation.
    smoke_frames: Option<u64>,
    frames_presented: u64,
    /// Wall-clock smoke budget from `AQUA_SMOKE_MS` (validation only):
    /// sleep normally until the deadline, then quit with a production
    /// report. Validates idle behavior (e.g. fps 900 must not loop).
    smoke_deadline: Option<Instant>,
    /// Event-loop iterations, reported by time-bounded smoke runs.
    wakeups: u64,
}

impl Aqua {
    pub fn new(config: AquaConfig) -> Result<Self, String> {
        Self::new_with_preferences(config, query_preferences())
    }

    /// Crate-private construction over one caller-supplied preferences
    /// snapshot. The root [`crate::App`] builder queries preferences once,
    /// resolves its authoring [`crate::Config`] against that same snapshot,
    /// and constructs here — so startup never snapshots the OS twice.
    /// Legacy [`Aqua::new`] keeps querying itself for direct low-level use.
    pub(crate) fn new_with_preferences(
        mut config: AquaConfig,
        prefs: OsPreferences,
    ) -> Result<Self, String> {
        config
            .validated()
            .map_err(|e| format!("invalid window options: {e:?}"))?;
        let mut scheduler = Scheduler::new();
        config.apply_to_scheduler(&mut scheduler);
        // `prefs` is the single startup snapshot (see `new_with_preferences`).
        // Native materials need transparency everywhere they show; without
        // it the backdrop is silently hidden behind opaque content.
        if aqua_platform::material::material_needs_transparent_window(config.material)
            && !config.window.transparent
        {
            eprintln!("aqua: material requires a transparent window — enabling window.transparent");
            config.window.transparent = true;
        }
        if config.material != aqua_platform::WindowMaterial::None && prefers_opaque_frame(&prefs) {
            eprintln!("aqua: high-contrast/transparency-off — material disabled");
            config.material = aqua_platform::WindowMaterial::None;
        }
        if config.reduced_motion {
            eprintln!("aqua: reduced-motion motion policy requested");
        }
        let (clipboard, clipboard_backend) = app_clipboard();
        eprintln!("aqua: clipboard backend: {clipboard_backend:?}");
        let smoke_frames = std::env::var("AQUA_SMOKE_FRAMES")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&n| n > 0);
        if let Some(n) = smoke_frames {
            eprintln!("aqua: smoke mode — exiting after {n} presented frames");
        }
        // Time-bounded smoke: a static app may only ever draw one frame, so
        // a frame budget alone never finishes without active animation.
        let smoke_deadline = std::env::var("AQUA_SMOKE_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&ms| ms > 0)
            .map(|ms| Instant::now() + Duration::from_millis(ms));
        if let Some(deadline) = smoke_deadline {
            eprintln!("aqua: time-bounded smoke — exiting at {deadline:?}");
        }
        Ok(Self {
            config,
            scheduler,
            windows: HashMap::new(),
            winit_map: HashMap::new(),
            next_id: 1,
            shared_gpu: None,
            quit_requested: false,
            render_errors: 0,
            proxy: None,
            flushers: HashMap::new(),
            completion_tx: None,
            fonts: aqua_text::SharedFonts::new(),
            runs: aqua_text::SharedRuns::new(),
            prefs,
            clipboard,
            clipboard_backend,
            smoke_deadline,
            wakeups: 0,
            smoke_frames,
            frames_presented: 0,
        })
    }

    pub fn scheduler(&self) -> &Scheduler {
        &self.scheduler
    }

    pub fn render_error_count(&self) -> u64 {
        self.render_errors
    }

    pub fn window_count(&self) -> usize {
        self.windows.len()
    }

    /// Whether an ACTIVE overlay guard is retained for `id`: a guard is
    /// present *and* the window was installed as `Overlay`. Native/Hidden
    /// installs retain a (detached) guard but draw no custom chrome, so
    /// bare retention would be a false claim — this reports `false` for
    /// them, for install fallback, and for headless windows.
    pub fn has_chrome(&self, id: AquaWindowId) -> bool {
        self.windows
            .get(&id)
            .is_some_and(|r| r.chrome.is_some() && r.chrome_overlay)
    }

    fn alloc_id(&mut self) -> AquaWindowId {
        let id = AquaWindowId::new(self.next_id).expect("window id overflow");
        self.next_id += 1;
        id
    }

    fn with_ctx<R>(&mut self, f: impl FnOnce(&mut AppCtx) -> R) -> R {
        let quit = &mut self.quit_requested;
        let proxy = self.proxy.clone();
        let mut ctx = AppCtx {
            scheduler: &mut self.scheduler,
            quit_requested: &mut *quit,
            proxy,
            flushers: &mut self.flushers,
            clipboard: &mut self.clipboard,
        };
        f(&mut ctx)
    }

    /// Headless window registration (tests): no winit, no GPU.
    pub fn add_headless_window(&mut self, observed: ObservedWindowState) -> AquaWindowId {
        let id = observed.id;
        self.windows.insert(
            id,
            WindowRecord {
                observed,
                winit_id: None,
                chrome: None,
                chrome_overlay: false,
                last_regions: None,
                window: None,
                gpu: None,
                cursor: None,
                buttons: 0,
                modifiers: CoreModifiers::NONE,
                ime_active: false,
                #[cfg(windows)]
                a11y: None,
            },
        );
        id
    }

    /// Headless dispatch of a framework window event (tests + pumps).
    pub fn dispatch_window_event(
        &mut self,
        app: &mut impl Application,
        id: AquaWindowId,
        event: &WindowEvent,
    ) {
        self.apply_observed(id, event);
        if let Some(reason) = window_event_frame_reason(event) {
            self.scheduler.request_frame(id, reason);
        }
        let ev = event.clone();
        self.with_ctx(|ctx| app.on_window_event(ctx, id, &ev));
        if matches!(
            event,
            WindowEvent::CloseRequested(_) | WindowEvent::Destroyed(_)
        ) {
            self.remove_window(id);
        }
    }

    /// Headless dispatch of a canonical input event (tests + pumps).
    pub fn dispatch_input(
        &mut self,
        app: &mut impl Application,
        id: AquaWindowId,
        event: &CoreInputEvent,
    ) {
        self.scheduler.request_frame(id, FrameReason::INPUT);
        let ev = event.clone();
        self.with_ctx(|ctx| app.on_input(ctx, id, &ev));
    }

    /// Headless file-drop delivery (tests + pumps).
    pub fn dispatch_file_drop(
        &mut self,
        app: &mut impl Application,
        id: AquaWindowId,
        paths: &[std::path::PathBuf],
    ) {
        self.scheduler.request_frame(id, FrameReason::INPUT);
        let owned = paths.to_vec();
        self.with_ctx(|ctx| app.on_files_dropped(ctx, id, &owned));
    }

    fn apply_observed(&mut self, id: AquaWindowId, event: &WindowEvent) {
        match event {
            WindowEvent::Resized(_, size) => {
                if let Some(rec) = self.windows.get_mut(&id) {
                    rec.observed.inner_size = *size;
                }
            }
            WindowEvent::ScaleChanged(_, s) => {
                if let Some(rec) = self.windows.get_mut(&id) {
                    rec.observed.scale_factor = *s;
                }
            }
            WindowEvent::Focused(_, f) => {
                if let Some(rec) = self.windows.get_mut(&id) {
                    rec.observed.focused = *f;
                }
            }
            WindowEvent::Occluded(_, o) => {
                if let Some(rec) = self.windows.get_mut(&id) {
                    rec.observed.occluded = *o;
                }
            }
            WindowEvent::ThemeChanged(_, theme) => {
                self.prefs.theme = *theme;
            }
            WindowEvent::CloseRequested(_)
            | WindowEvent::Destroyed(_)
            | WindowEvent::RedrawRequested(_) => {}
        }
    }

    fn remove_window(&mut self, id: AquaWindowId) {
        // The record (custom-titlebar guard included) drops here: guard
        // first, then the winit window — see `WindowRecord` field order.
        if let Some(rec) = self.windows.remove(&id) {
            if let Some(wid) = rec.winit_id {
                self.winit_map.remove(&wid);
            }
        }
        self.scheduler.remove_window(id);
        self.flushers.remove(&id);
    }

    /// Cloneable wake handle for worker threads: `send_event` wakes the UI
    /// executor without polling. Returns `None` before `run()` starts the
    /// loop (headless use needs no proxy — flush manually instead).
    pub fn event_proxy(&self) -> Option<winit::event_loop::EventLoopProxy<AquaUserEvent>> {
        self.proxy.clone()
    }

    /// Replace the shared run registry (e.g. hermetic tests).
    pub fn set_runs(&mut self, runs: aqua_text::SharedRuns) {
        self.runs = runs;
    }

    /// The shared registry apps mint `TextRun`s through.
    pub fn runs(&self) -> &aqua_text::SharedRuns {
        &self.runs
    }

    /// Backend handle for renderer construction: one font database and one
    /// run registry for the whole process. Never a second `FontSystem`.
    pub fn text_backend(&self) -> aqua_render::text::TextBackend {
        aqua_render::text::TextBackend::new(self.fonts.clone(), self.runs.clone())
    }

    /// Replace the shared font set (e.g. hermetic test fonts, bundled app
    /// fonts). The same handle serves shaping in apps and glyph
    /// rasterization in the renderer.
    pub fn set_fonts(&mut self, fonts: aqua_text::SharedFonts) {
        self.fonts = fonts;
    }

    /// The shared fonts for app-side shaping (`TextLayout::layout`).
    pub fn fonts(&self) -> &aqua_text::SharedFonts {
        &self.fonts
    }

    /// Per-window GPU accounting: (submitted, present-requested, completed).
    /// `None` for headless windows. Submitted counts encoder submissions;
    /// completed counts GPU-finished work (never the submit call itself).
    pub fn gpu_stats(&self, id: AquaWindowId) -> Option<(u64, u64, u64)> {
        self.windows
            .get(&id)
            .and_then(|r| r.gpu.as_ref())
            .map(|g| (g.ledger.submitted, g.ledger.presented, g.ledger.completed))
    }

    /// OS preferences snapshot (material fallback + motion policy source).
    pub fn preferences(&self) -> &OsPreferences {
        &self.prefs
    }

    /// Effective startup motion request: OS reduced-motion preference or an
    /// explicit config request. The single accessor low-level entry points
    /// use to derive the policy.
    pub(crate) fn motion_reduced(&self) -> bool {
        self.prefs.reduce_motion.is_on() || self.config.reduced_motion
    }

    /// Which clipboard backend is active (native vs explicit fallback).
    pub fn clipboard_backend(&self) -> ClipboardBackend {
        self.clipboard_backend
    }

    /// Run the GUI event loop. Returns on ordered shutdown.
    /// GPU absence is a loud error, never a silent fallback (software
    /// fallback policy is an explicit future decision per `docs/PLATFORM.md`).
    pub fn run(mut self, app: impl Application + 'static) -> Result<(), String> {
        if self.config.headless {
            return Err("headless config cannot run the GUI loop; use the headless pump".into());
        }
        let event_loop = winit::event_loop::EventLoop::<AquaUserEvent>::with_user_event()
            .build()
            .map_err(|e| format!("event loop unavailable: {e:?}"))?;
        self.proxy = Some(event_loop.create_proxy());
        let mut loop_ = Loop { aqua: self, app };
        event_loop
            .run_app(&mut loop_)
            .map_err(|e| format!("event loop failed: {e:?}"))
    }
}

struct Loop<A: Application> {
    aqua: Aqua,
    app: A,
}

impl<A: Application> Loop<A> {
    fn ensure_gpu(&mut self, window: &Arc<winit::window::Window>) -> Result<(), String> {
        if self.aqua.shared_gpu.is_some() {
            return Ok(());
        }
        // Proposed backend policy (design §GPU strategy): DX12 on Windows,
        // Metal on macOS, Vulkan on Linux — selected by wgpu's default
        // backend ordering through `request_adapter`, not by forcing one.
        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_desc.backends = wgpu::Backends::all();
        #[cfg(windows)]
        if self.aqua.config.window.transparent {
            // Win32 Vulkan surfaces and HWND DXGI swapchains are opaque.
            // Let wgpu own a DirectComposition visual and its lifetime so
            // desktop materials show through the premultiplied swapchain.
            instance_desc.backends = wgpu::Backends::DX12;
            instance_desc.backend_options.dx12.presentation_system =
                wgpu::Dx12SwapchainKind::DxgiFromVisual;
        }
        let instance = wgpu::Instance::new(instance_desc);
        #[allow(clippy::arc_with_non_send_sync)]
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("surface creation failed: {e:?}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|e| format!("no usable GPU adapter (software fallback unresolved): {e:?}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("aqua"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| format!("GPU device request failed: {e:?}"))?;
        self.aqua.shared_gpu = Some(SharedGpu {
            instance,
            adapter,
            device,
            queue,
        });
        Ok(())
    }

    fn attach_gpu_window(
        &mut self,
        id: AquaWindowId,
        window: &Arc<winit::window::Window>,
    ) -> Result<(), String> {
        self.ensure_gpu(window)?;
        let gpu = self.aqua.shared_gpu.as_ref().expect("gpu ensured");
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Err("zero-sized surface; window not yet laid out".into());
        }
        let mode = if self.aqua.config.vsync {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        #[allow(clippy::arc_with_non_send_sync)]
        let surface = gpu
            .instance
            .create_surface(window.clone())
            .map_err(|e| format!("surface creation failed: {e:?}"))?;
        let facade = SurfaceFacade::with_adapter(
            &gpu.device,
            &gpu.adapter,
            surface,
            size.width,
            size.height,
            None,
            Some(mode),
        )
        .map_err(|e| format!("surface configuration failed: {e:?}"))?;
        if self.aqua.config.window.transparent {
            eprintln!(
                "aqua: transparent surface: {:?}, alpha={:?}, format={:?}",
                gpu.adapter.get_info().backend,
                facade.alpha_mode(),
                facade.format(),
            );
        }
        let scale = window.scale_factor() as f32;
        // Shared text backend: one font database + one run registry for the
        // process, so measurement and pixels cannot disagree.
        let backend =
            aqua_render::text::TextBackend::new(self.aqua.fonts.clone(), self.aqua.runs.clone());
        let mut renderer = Renderer::new_with_text(
            gpu.device.clone(),
            gpu.queue.clone(),
            facade.format(),
            backend,
        );
        renderer
            .set_size(size.width, size.height, scale)
            .map_err(|e| format!("renderer sizing failed: {e:?}"))?;
        if let Some(rec) = self.aqua.windows.get_mut(&id) {
            rec.gpu = Some(GpuWindow {
                surface: facade,
                renderer,
                scene: Scene::new(),
                ledger: GpuLedger::default(),
            });
        }
        Ok(())
    }

    /// Ensure the GPU completion worker exists, spawning it once. The worker
    /// owns a device clone and the loop proxy: per submission it blocks in
    /// `device.poll(Wait)` (everything submitted so far on this queue is
    /// ordered, so the waited submission is complete on return) and then
    /// wakes the loop. Parking on `recv` means zero idle ticks; blocking
    /// off-UI means the in-flight cap stays meaningful even when no draws
    /// are happening to piggyback a poll on.
    fn ensure_completion_worker(&mut self) {
        if self.aqua.completion_tx.is_some() {
            return;
        }
        let Some(gpu) = self.aqua.shared_gpu.as_ref() else {
            return;
        };
        let device = gpu.device.clone();
        let proxy = self.aqua.proxy.clone();
        let (tx, rx) = std::sync::mpsc::channel::<(AquaWindowId, u64)>();
        std::thread::Builder::new()
            .name("aqua-gpu-complete".into())
            .spawn(move || {
                for (id, seq) in rx {
                    // Blocking wait for everything submitted so far; the
                    // awaited submission is therefore complete on return.
                    // Under sustained saturation this never returns, which
                    // is exactly the backpressure signal — draws have
                    // already stopped at the cap, and the wake fires the
                    // moment the queue drains.
                    let _ = device.poll(wgpu::PollType::wait_indefinitely());
                    if let Some(p) = &proxy {
                        let _ = p.send_event(AquaUserEvent::GpuComplete(id, seq));
                    }
                }
            })
            .expect("aqua: completion worker spawn failed");
        self.aqua.completion_tx = Some(tx);
    }

    fn draw_ready(&mut self, now: Instant) {
        let ready = self.aqua.scheduler.ready_windows(now);
        if ready.is_empty() {
            return;
        }
        let ids: Vec<AquaWindowId> = ready.into_iter().map(|(id, _)| id).collect();
        self.aqua.scheduler.mark_produced(&ids, now);
        for id in ids {
            // Occluded windows do no work (surface signal): release the slot.
            let occluded = self
                .aqua
                .windows
                .get(&id)
                .is_some_and(|r| r.observed.occluded);
            if occluded {
                self.aqua.scheduler.mark_presented(id);
                continue;
            }
            let has_gpu = self.aqua.windows.get(&id).is_some_and(|r| r.gpu.is_some());
            if !has_gpu {
                self.aqua.scheduler.mark_presented(id);
                continue;
            }
            // Host-driven update boundary: flush bound reactive effects
            // before the app rebuilds the scene, so the frame observes one
            // coherent state. `flush()` on an empty queue is a no-op.
            if let Some(flusher) = self.aqua.flushers.get_mut(&id) {
                let _report = flusher();
            }
            // Build scene through the app, then present.
            let observed = self.aqua.windows.get(&id).map(|r| r.observed.clone());
            let Some(observed) = observed else {
                self.aqua.scheduler.mark_presented(id);
                continue;
            };
            // GPU extension boundary: offscreen media/viewport passes run
            // first; `build_scene` then commits `External` items referencing
            // freshly registered layers.
            {
                let rec = self.aqua.windows.get_mut(&id).expect("window present");
                let gpu = rec.gpu.as_mut().expect("gpu checked");
                let format = gpu.renderer.format();
                let scale = observed.scale_factor as f32;
                let width = ((observed.inner_size.width as f32) * scale)
                    .round()
                    .max(1.0) as u32;
                let height = ((observed.inner_size.height as f32) * scale)
                    .round()
                    .max(1.0) as u32;
                let shared = self.aqua.shared_gpu.as_ref().expect("gpu exists");
                let mut register = |layer: u64, view: wgpu::TextureView, w: u32, h: u32| {
                    gpu.renderer.register_external(layer, view, w, h);
                };
                let mut frame = ExternalFrame {
                    device: &shared.device,
                    queue: &shared.queue,
                    color_format: format,
                    width,
                    height,
                    scale,
                    register: &mut register,
                };
                self.app.externals(&mut frame);
            }
            {
                let rec = self.aqua.windows.get_mut(&id).expect("window present");
                let gpu = rec.gpu.as_mut().expect("gpu checked");
                let deadline = self.app.build_scene(&mut gpu.scene, &observed);
                if let Some(t) = deadline {
                    self.aqua.scheduler.request_animation_deadline(id, t);
                }
            }
            // Custom-titlebar drag regions: push the hook's explicit rects
            // after the scene/layout commit, converted with the current
            // scale — only while a live guard is retained, and only when
            // the value or the scale changed (clean frames do nothing). A
            // quiet hook after explicit rects pushes empty rects to clear
            // stale holes; the install-time drag band persists regardless.
            {
                let next = self.app.chrome_regions(&observed);
                // Normalized like the guard's own clamp, so the cache key
                // always matches what was actually pushed.
                let scale = observed.scale_factor;
                let scale = if scale.is_finite() && scale > 0.0 {
                    scale
                } else {
                    1.0
                };
                if let Some(rec) = self.aqua.windows.get_mut(&id) {
                    if let Some(plan) = chrome_plan(&rec.last_regions, next, scale) {
                        // Zero-alloc empty: `Vec::new()` never heap-allocates.
                        let cleared;
                        let payload = match &plan {
                            ChromePlan::Apply(regions) => regions,
                            ChromePlan::Clear => {
                                cleared = DragRegions::default();
                                &cleared
                            }
                        };
                        if let Some(guard) = rec.chrome.as_ref() {
                            if let Err(e) =
                                guard.set_drag_rects(&payload.draggable, &payload.no_drag, scale)
                            {
                                eprintln!("aqua: chrome regions rejected ({e:?})");
                            }
                        }
                        rec.last_regions = match plan {
                            ChromePlan::Apply(regions) => Some((regions, scale)),
                            ChromePlan::Clear => None,
                        };
                    }
                }
            }
            // Publish the semantic snapshot when a bridge is attached and
            // the app provides one (bounds come from committed layout).
            #[cfg(windows)]
            {
                let snapshot = self.app.a11y_nodes();
                if let Some((root, focus, items)) = snapshot {
                    if let Some(rec) = self.aqua.windows.get_mut(&id) {
                        if let Some(adapter) = rec.a11y.as_mut() {
                            adapter.push_snapshot(build_tree_update(root, focus, &items));
                        }
                    }
                }
            }
            // IME candidate window: position from the app's shaped caret
            // every frame; explicitly disable when no editor has focus so a
            // stale candidate window cannot linger after focus leaves.
            {
                let area = self.app.ime_cursor_area();
                if let Some(rec) = self.aqua.windows.get(&id) {
                    if let Some(window) = rec.window.as_ref() {
                        use aqua_platform::services::{set_ime_cursor_area, ImeCursorArea};
                        window.set_ime_allowed(area.is_some());
                        if let Some(r) = area {
                            let logical = aqua_platform::geometry::LogicalPosition {
                                x: r.x as f64,
                                y: r.y as f64,
                            };
                            let size = aqua_platform::geometry::LogicalSize::new(
                                r.w.max(1.0) as f64,
                                r.h.max(1.0) as f64,
                            );
                            set_ime_cursor_area(
                                window.as_ref(),
                                ImeCursorArea {
                                    origin: logical,
                                    size,
                                },
                            );
                        }
                    }
                }
            }
            let rec = self.aqua.windows.get_mut(&id).expect("window present");
            let gpu = rec.gpu.as_mut().expect("gpu checked");
            enum Outcome {
                /// Submitted + present-requested; the completion worker
                /// drives the slot release.
                Submitted { seq: u64 },
                /// Clean frame skipped: nothing submitted.
                Skipped,
                /// Surface/device failure with message + held slot count.
                Failed { message: String, held: u32 },
            }
            let outcome = match gpu
                .surface
                .acquire_and_draw(&mut gpu.renderer, &mut gpu.scene)
            {
                Ok(true) => {
                    gpu.ledger.submitted += 1;
                    gpu.ledger.presented += 1;
                    gpu.ledger.unreleased += 1;
                    Outcome::Submitted {
                        seq: gpu.ledger.submitted,
                    }
                }
                Ok(false) => Outcome::Skipped,
                Err(e) => {
                    let held = gpu.ledger.unreleased;
                    gpu.ledger.unreleased = 0;
                    Outcome::Failed {
                        message: format!("{e:?}"),
                        held,
                    }
                }
            };
            // Window borrow ends here; scheduler/context calls below.
            match outcome {
                Outcome::Submitted { seq } => {
                    // Hold the scheduler slot until GPU *completion*, so the
                    // in-flight cap bounds actual GPU work even in Immediate
                    // mode at 900 Hz. The dedicated worker thread blocks in
                    // `poll(Wait)` for this submission (off-UI) and wakes
                    // the loop — this path never blocks and never polls.
                    self.ensure_completion_worker();
                    if let Some(tx) = self.aqua.completion_tx.clone() {
                        let _ = tx.send((id, seq));
                    }
                    self.aqua.frames_presented += 1;
                    if let Some(budget) = self.aqua.smoke_frames {
                        if self.aqua.frames_presented >= budget {
                            eprintln!(
                                "aqua: smoke budget reached ({} frames) — exiting",
                                self.aqua.frames_presented
                            );
                            self.aqua.quit_requested = true;
                        }
                    }
                }
                Outcome::Skipped => {
                    self.aqua.scheduler.mark_presented(id);
                }
                Outcome::Failed { message, held } => {
                    self.aqua.render_errors += 1;
                    // Device loss / surface failure: completions for
                    // in-flight submissions may never arrive. Force-release
                    // every held slot so the window cannot wedge in
                    // backpressure; the next demand retries fresh.
                    for _ in 0..held {
                        self.aqua.scheduler.mark_presented(id);
                    }
                    self.aqua.scheduler.mark_presented(id);
                    self.aqua
                        .with_ctx(|ctx| self.app.on_render_error(ctx, id, &message));
                }
            }
        }
    }

    fn route_winit_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        let Some(&id) = self.aqua.winit_map.get(&window_id) else {
            return;
        };
        // AccessKit plumbing first: the adapter must see every native event
        // before app handling (contract in `a11y_winit`).
        #[cfg(windows)]
        {
            let window = self.aqua.windows.get(&id).and_then(|r| r.window.clone());
            if let (Some(w), Some(rec)) = (window, self.aqua.windows.get_mut(&id)) {
                if let Some(adapter) = rec.a11y.as_mut() {
                    adapter.process_window_event(&w, &event);
                }
            }
        }
        let scale = self
            .aqua
            .windows
            .get(&id)
            .and_then(|r| r.window.clone())
            .map(|w| w.scale_factor())
            .unwrap_or(1.0);
        if let Some(framework) = translate_window_event(&event, id, scale) {
            if matches!(framework, WindowEvent::RedrawRequested(_)) {
                self.aqua.scheduler.request_frame(id, FrameReason::EXPLICIT);
                self.draw_ready(Instant::now());
                return;
            }
            if let Some(reason) = window_event_frame_reason(&framework) {
                self.aqua.scheduler.request_frame(id, reason);
            }
            // One place for DPI/material/focus follow-ups (`surface_signal`).
            let signal = surface_signal(&framework);
            if signal.resize_gpu {
                if let WindowEvent::Resized(_, size) = &framework {
                    let phys = size.to_physical(scale);
                    if let Some(rec) = self.aqua.windows.get_mut(&id) {
                        rec.observed.inner_size = *size;
                        if let Some(gpuw) = rec.gpu.as_mut() {
                            let g = self.aqua.shared_gpu.as_ref().expect("gpu exists");
                            let _ = gpuw.surface.resize(
                                &g.device,
                                &mut gpuw.renderer,
                                phys.width,
                                phys.height,
                                scale as f32,
                            );
                        }
                    }
                }
            }
            if signal.material_recheck {
                Self::reapply_material(&mut self.aqua, id);
            }
            let fw = framework.clone();
            self.aqua.apply_observed(id, &fw);
            self.aqua
                .with_ctx(|ctx| self.app.on_window_event(ctx, id, &fw));
            // Drain AT action requests into UI behaviors after app handling.
            #[cfg(windows)]
            Self::drain_a11y_actions(&mut self.aqua, &mut self.app, id);
            if matches!(
                fw,
                WindowEvent::CloseRequested(_) | WindowEvent::Destroyed(_)
            ) {
                self.aqua.remove_window(id);
                if self.aqua.windows.is_empty() {
                    event_loop.exit();
                }
            }
            return;
        }
        self.route_input(event_loop, id, scale, event);
    }

    /// Canonical translators (`events::translate_*_core`) at the boundary.
    /// Modifiers are tracked per window; committed key text is delivered
    /// exactly once as `TextInput` and SUPPRESSED while IME is active.
    fn route_input(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        id: AquaWindowId,
        scale: f64,
        event: winit::event::WindowEvent,
    ) {
        use winit::event::WindowEvent as W;
        // File drops route to their own app hook (no core InputEvent
        // variant exists, deliberately — drops are commands, not gestures).
        if let Some(drop) = translate_file_event(&event) {
            match drop {
                FileDrop::Dropped(path) => {
                    self.aqua.scheduler.request_frame(id, FrameReason::INPUT);
                    self.aqua.with_ctx(|ctx| {
                        self.app
                            .on_files_dropped(ctx, id, std::slice::from_ref(&path))
                    });
                }
                // Hover enter/leave: no visual commitment in the facade.
                FileDrop::Hovered(_) | FileDrop::HoverCancelled => {}
            }
            return;
        }
        // Modifiers are state, not events: store for key translation.
        if let W::ModifiersChanged(mods) = &event {
            if let Some(rec) = self.aqua.windows.get_mut(&id) {
                rec.modifiers = translate_modifiers(mods);
            }
            return;
        }
        let mut extra: Option<CoreInputEvent> = None;
        let input: Option<CoreInputEvent> = match event {
            W::KeyboardInput { event, .. } => {
                let pressed = event.state == winit::event::ElementState::Pressed;
                let modifiers = self
                    .aqua
                    .windows
                    .get(&id)
                    .map(|r| r.modifiers)
                    .unwrap_or(CoreModifiers::NONE);
                let (key_event, text) = translate_key_event(&event, modifiers);
                // Committed text exactly once per press, suppressed during IME.
                if pressed {
                    let ime = self.aqua.windows.get(&id).is_some_and(|r| r.ime_active);
                    if !ime {
                        if let Some(t) = text.filter(|t| !t.is_empty()) {
                            extra = Some(CoreInputEvent::TextInput { text: t });
                        }
                    }
                }
                Some(if pressed {
                    CoreInputEvent::KeyDown(key_event)
                } else {
                    CoreInputEvent::KeyUp(key_event)
                })
            }
            W::CursorMoved { position, .. } => {
                let buttons = self.aqua.windows.get(&id).map(|r| r.buttons).unwrap_or(0);
                let pointer = pointer_contact(
                    0,
                    PointerKind::Mouse,
                    position.x,
                    position.y,
                    scale,
                    buttons,
                );
                if let Some(rec) = self.aqua.windows.get_mut(&id) {
                    rec.cursor = Some((pointer.position.x as f64, pointer.position.y as f64));
                }
                Some(CoreInputEvent::PointerMove(pointer))
            }
            W::MouseInput { state, button, .. } => {
                let core_button = translate_mouse_button_core(button);
                let bit = match button {
                    winit::event::MouseButton::Left => 0x01,
                    winit::event::MouseButton::Right => 0x02,
                    winit::event::MouseButton::Middle => 0x04,
                    _ => 0x08,
                };
                let pressed = state == winit::event::ElementState::Pressed;
                let pointer = match self.aqua.windows.get_mut(&id) {
                    Some(rec) => {
                        if pressed {
                            rec.buttons |= bit;
                        } else {
                            rec.buttons &= !bit;
                        }
                        let (x, y) = rec.cursor.unwrap_or((0.0, 0.0));
                        pointer_contact(
                            0,
                            PointerKind::Mouse,
                            x * scale,
                            y * scale,
                            scale,
                            rec.buttons,
                        )
                    }
                    None => pointer_contact(0, PointerKind::Mouse, 0.0, 0.0, 1.0, 0),
                };
                Some(if pressed {
                    CoreInputEvent::PointerDown(pointer, core_button)
                } else {
                    CoreInputEvent::PointerUp(pointer, core_button)
                })
            }
            W::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => (x * 16.0, y * 16.0),
                    winit::event::MouseScrollDelta::PixelDelta(p) => {
                        (p.x as f32 / scale as f32, p.y as f32 / scale as f32)
                    }
                };
                let at = self
                    .aqua
                    .windows
                    .get(&id)
                    .and_then(|r| r.cursor)
                    .map(|(x, y)| aqua_core::Point::new(x as f32, y as f32))
                    .unwrap_or(aqua_core::Point::new(0.0, 0.0));
                Some(CoreInputEvent::Scroll {
                    at,
                    delta: aqua_core::ScrollDelta { x: dx, y: dy },
                })
            }
            W::Touch(t) => {
                let position = to_core_point(t.location.x, t.location.y, scale);
                if let Some(rec) = self.aqua.windows.get_mut(&id) {
                    rec.cursor = Some((position.x as f64, position.y as f64));
                }
                use winit::event::TouchPhase as P;
                let pointer = |buttons| {
                    pointer_contact(
                        t.id,
                        PointerKind::Touch,
                        t.location.x,
                        t.location.y,
                        scale,
                        buttons,
                    )
                };
                match t.phase {
                    P::Started => Some(CoreInputEvent::PointerDown(
                        pointer(1),
                        translate_mouse_button_core(winit::event::MouseButton::Left),
                    )),
                    P::Ended => Some(CoreInputEvent::PointerUp(
                        pointer(0),
                        translate_mouse_button_core(winit::event::MouseButton::Left),
                    )),
                    // Capture loss / interruption: real cancel. The UI must
                    // terminate the drag; a synthesized release could
                    // mis-activate the press target.
                    P::Cancelled => Some(CoreInputEvent::PointerCancel { contact: t.id }),
                    P::Moved => Some(CoreInputEvent::PointerMove(pointer(1))),
                }
            }
            W::Ime(ime) => {
                let core = translate_ime_core(&ime);
                // Track composition for key-text suppression.
                if let Some(rec) = self.aqua.windows.get_mut(&id) {
                    match &core {
                        CoreInputEvent::Ime(aqua_core::ImeEvent::Start)
                        | CoreInputEvent::Ime(aqua_core::ImeEvent::Update { .. }) => {
                            rec.ime_active = true;
                        }
                        _ => rec.ime_active = false,
                    }
                }
                Some(core)
            }
            W::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(rec) = self.aqua.windows.get_mut(&id) {
                    rec.observed.scale_factor = scale_factor;
                }
                self.aqua.scheduler.request_frame(id, FrameReason::RESIZE);
                let fw = WindowEvent::ScaleChanged(id, scale_factor);
                self.aqua
                    .with_ctx(|ctx| self.app.on_window_event(ctx, id, &fw));
                None
            }
            _ => None,
        };
        if let Some(input) = input {
            self.aqua.scheduler.request_frame(id, FrameReason::INPUT);
            self.aqua.with_ctx(|ctx| self.app.on_input(ctx, id, &input));
        }
        // Committed text follows the key event as its own delivery.
        if let Some(text_event) = extra {
            self.aqua.scheduler.request_frame(id, FrameReason::INPUT);
            self.aqua
                .with_ctx(|ctx| self.app.on_input(ctx, id, &text_event));
        }
    }

    /// Drain queued AT action requests into UI behaviors. The full request
    /// (node, action, payload [`A11yActionData`](aqua_platform::a11y::A11yActionData))
    /// is forwarded untouched — the facade never drops the payload.
    /// Free function so both the event path and the waker path share it.
    #[cfg(windows)]
    fn drain_a11y_actions(aqua: &mut Aqua, app: &mut A, id: AquaWindowId) {
        let actions = aqua
            .windows
            .get_mut(&id)
            .and_then(|r| r.a11y.as_mut())
            .map(|a| a.drain_actions())
            .unwrap_or_default();
        for request in &actions {
            let req = request.clone();
            aqua.with_ctx(|ctx| app.on_a11y_action(ctx, id, &req));
        }
    }

    /// Re-apply the configured native material (focus/theme/prefs changes
    /// alter the fallback posture). Honest capability reporting, never
    /// silent success; non-Windows always falls back with a reason.
    /// Associated function taking `&mut Aqua` so all call sites share it.
    fn reapply_material(aqua: &mut Aqua, id: AquaWindowId) {
        let material = aqua.config.effective_material(&aqua.prefs);
        let capability = aqua_platform::material::material_capability(material);
        if !capability.supported {
            eprintln!("aqua: material {:?}: {}", material, capability.reason);
            return;
        }
        let window = aqua.windows.get(&id).and_then(|r| r.window.clone());
        let Some(window) = window else { return };
        #[cfg(windows)]
        match aqua_platform::adapters::apply_material(&window, material) {
            Ok(applied) => eprintln!("aqua: material {:?}: {}", material, applied.reason),
            Err(e) => {
                aqua.render_errors += 1;
                eprintln!("aqua: material apply failed: {e:?}");
            }
        }
        #[cfg(not(windows))]
        {
            let applied = aqua_platform::adapters::apply_material(material);
            eprintln!("aqua: material {:?}: {}", material, applied.reason);
            let _ = window;
        }
    }
}

impl<A: Application> winit::application::ApplicationHandler<AquaUserEvent> for Loop<A> {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        if !self.aqua.windows.is_empty() || self.aqua.config.headless {
            return;
        }
        let mut attrs = window_attributes(&self.aqua.config.window);
        // Material shows only through transparency (platform contract).
        if self.aqua.config.material != aqua_platform::WindowMaterial::None {
            attrs = attrs.with_transparent(true);
        }
        // AccessKit attaches only to initially-invisible windows; the
        // adapter constructor rejects visible ones. Default off here —
        // enabled per-window below once attachment succeeds.
        #[cfg(windows)]
        {
            attrs = attrs.with_visible(false);
        }
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.aqua.render_errors += 1;
                eprintln!("aqua: window creation failed: {e:?}");
                event_loop.exit();
                return;
            }
        };
        let id = self.aqua.alloc_id();
        // Custom titlebar chrome (platform `chrome::WindowsChrome` contract):
        // install BEFORE reading size/scale — the install re-resolves the
        // frame (`SWP_FRAMECHANGED` drops the native caption), so any size
        // observed earlier would be stale for scene/layout/IME. Failure is
        // loud and falls back to the honest native titlebar — never fake
        // buttons. The guard moves into the per-window record below and
        // drops with it (guard first — see `WindowRecord` field order).
        let chrome = match aqua_platform::chrome::WindowsChrome::install(
            &window,
            &self.aqua.config.window,
        ) {
            Ok(guard) => {
                eprintln!("aqua: window chrome installed: {:?}", guard.diagnostics());
                Some(guard)
            }
            Err(e) => {
                eprintln!(
                    "aqua: window chrome install failed ({e:?}) — continuing with native titlebar"
                );
                None
            }
        };
        let chrome_overlay = chrome.is_some()
            && self.aqua.config.window.effective_titlebar()
                == aqua_platform::window::Titlebar::Overlay;
        // The adapter applies the requested client dimensions while
        // recalculating the custom frame. Read its actual size here;
        // winit's ordinary size request would add the native caption
        // margins back because the overlay retains WS_CAPTION.
        let scale = window.scale_factor();
        let size = window.inner_size();
        let observed = ObservedWindowState {
            id,
            title: self.aqua.config.window.title.clone(),
            inner_size: aqua_platform::geometry::LogicalSize::new(
                size.width as f64 / scale,
                size.height as f64 / scale,
            ),
            scale_factor: scale,
            focused: false,
            visible: true,
            occluded: false,
        };
        self.aqua.winit_map.insert(window.id(), id);
        self.aqua.windows.insert(
            id,
            WindowRecord {
                observed,
                winit_id: Some(window.id()),
                chrome,
                chrome_overlay,
                last_regions: None,
                window: Some(window.clone()),
                gpu: None,
                cursor: None,
                buttons: 0,
                modifiers: CoreModifiers::NONE,
                ime_active: false,
                #[cfg(windows)]
                a11y: None,
            },
        );
        #[cfg(windows)]
        {
            // Bridge waker (required): AT actions arriving while idle wake
            // the loop through the proxy; invoked after the bridge lock is
            // released, so draining here cannot deadlock.
            let proxy = self.aqua.proxy.clone();
            let waker: aqua_platform::a11y::WakeFn = std::sync::Arc::new(move || {
                if let Some(p) = &proxy {
                    let _ = p.send_event(AquaUserEvent::A11yActions(id));
                }
            });
            match A11yAdapter::attach_with_waker(event_loop, &window, waker) {
                Ok(adapter) => {
                    if let Some(rec) = self.aqua.windows.get_mut(&id) {
                        rec.a11y = Some(adapter);
                    }
                    window.set_visible(true);
                }
                Err(e) => {
                    // No bridge (already-visible race, AT plumbing down):
                    // show the window and run without accessibility.
                    eprintln!("aqua: a11y attach failed ({e:?}) — continuing without bridge");
                    window.set_visible(true);
                }
            }
            Self::reapply_material(&mut self.aqua, id);
        }
        #[cfg(not(windows))]
        {
            Self::reapply_material(&mut self.aqua, id);
        }
        if let Err(e) = self.attach_gpu_window(id, &window) {
            self.aqua.render_errors += 1;
            eprintln!("aqua: GPU setup failed: {e}");
        }
        self.aqua.scheduler.request_frame(id, FrameReason::EXPOSE);
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        self.route_winit_event(event_loop, window_id, event);
    }

    fn about_to_wait(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let now = Instant::now();
        self.aqua.wakeups += 1;
        // Time-bounded smoke: report actual production, then quit. Wakeup vs
        // produced counts show whether an idle app slept (target 900 must
        // not loop).
        if let Some(deadline) = self.aqua.smoke_deadline {
            if now >= deadline {
                let stats = self.aqua.scheduler.stats();
                eprintln!(
                    "aqua: smoke report: wakeups={} requested={} produced={} presented_frames={} coalesced={} deferred_backpressure={} render_errors={}",
                    self.aqua.wakeups,
                    stats.requested,
                    stats.produced,
                    self.aqua.frames_presented,
                    stats.coalesced,
                    stats.deferred_backpressure,
                    self.aqua.render_errors,
                );
                event_loop.exit();
                return;
            }
        }
        self.draw_ready(now);
        // Fuse the scheduler deadline with the smoke deadline: a static app
        // with no pending scheduler work must still wake for the smoke exit
        // instead of sleeping past it.
        let wake = match (
            self.aqua.scheduler.next_wakeup(now),
            self.aqua.smoke_deadline,
        ) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, b) => b,
        };
        event_loop.set_control_flow(control_flow_for_next_wakeup(wake));
        if self.aqua.quit_requested {
            event_loop.exit();
        }
    }

    fn user_event(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        event: AquaUserEvent,
    ) {
        match event {
            AquaUserEvent::ReactiveWake(id) => {
                // Worker-thread write queued effects: flush on the UI thread
                // and demand a frame only when effects actually ran.
                let ran = self
                    .aqua
                    .flushers
                    .get_mut(&id)
                    .map(|flusher| flusher().ran)
                    .unwrap_or(0);
                if ran > 0 {
                    self.aqua.scheduler.request_frame(id, FrameReason::INPUT);
                }
            }
            AquaUserEvent::A11yActions(id) => {
                // Bridge waker fired: actions arrived while idle.
                #[cfg(windows)]
                Self::drain_a11y_actions(&mut self.aqua, &mut self.app, id);
                #[cfg(not(windows))]
                let _ = id;
            }
            AquaUserEvent::GpuComplete(id, seq) => {
                // Completion callback: every submission numbered <= seq is
                // done. Release exactly the newly completed scheduler slots;
                // unknown ids (closed window) are ignored.
                let release = match self.aqua.windows.get_mut(&id) {
                    Some(rec) => match rec.gpu.as_mut() {
                        Some(gpu) if seq > gpu.ledger.completed => {
                            let newly = (seq - gpu.ledger.completed)
                                .min(gpu.ledger.unreleased as u64)
                                as u32;
                            gpu.ledger.completed = seq;
                            gpu.ledger.unreleased -= newly;
                            newly
                        }
                        _ => 0,
                    },
                    None => 0,
                };
                for _ in 0..release {
                    self.aqua.scheduler.mark_presented(id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aqua_platform::geometry::LogicalSize;

    struct Probe {
        window_events: Vec<WindowEvent>,
        inputs: Vec<CoreInputEvent>,
        scenes_built: u64,
        errors: Vec<String>,
        cancels: u64,
    }

    impl Probe {
        fn new() -> Self {
            Self {
                window_events: Vec::new(),
                inputs: Vec::new(),
                scenes_built: 0,
                errors: Vec::new(),
                cancels: 0,
            }
        }
    }

    impl Application for Probe {
        fn on_window_event(&mut self, ctx: &mut AppCtx, _id: AquaWindowId, event: &WindowEvent) {
            if matches!(event, WindowEvent::CloseRequested(_)) {
                ctx.quit();
            }
            self.window_events.push(event.clone());
        }

        fn on_input(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &CoreInputEvent) {
            // Capture loss must arrive as cancel — never as a release that
            // could mis-activate the press target.
            if matches!(event, CoreInputEvent::PointerCancel { .. }) {
                self.cancels += 1;
            }
            self.inputs.push(event.clone());
            // Demand-driven: input that changes state requests animation work.
            if matches!(event, CoreInputEvent::PointerDown(..)) {
                ctx.request_animation_deadline(id, Instant::now());
            }
        }

        fn build_scene(
            &mut self,
            scene: &mut Scene,
            _window: &ObservedWindowState,
        ) -> Option<std::time::Instant> {
            use aqua_core::{Oklch, Rect};
            self.scenes_built += 1;
            scene.insert(aqua_render::scene::DisplayItem::Rect {
                bounds: Rect::from_edges(0.0, 0.0, 100.0, 100.0),
                radius: 8.0,
                fill: Oklch::from_hex("#1d4ed8").unwrap(),
                border_width: 0.0,
                border_color: Oklch::from_hex("#000000").unwrap(),
                shadow: None,
            });
            None
        }

        fn on_render_error(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, message: &str) {
            self.errors.push(message.to_string());
        }
    }

    fn headless() -> (Aqua, Probe, AquaWindowId) {
        let mut aqua = Aqua::new(AquaConfig {
            headless: true,
            ..Default::default()
        })
        .unwrap();
        let id = aqua.alloc_id();
        aqua.add_headless_window(ObservedWindowState {
            id,
            title: "probe".into(),
            inner_size: LogicalSize::new(800.0, 600.0),
            scale_factor: 1.0,
            focused: true,
            visible: true,
            occluded: false,
        });
        (aqua, Probe::new(), id)
    }

    #[test]
    fn window_events_update_observed_state_and_schedule() {
        let (mut aqua, mut app, id) = headless();
        aqua.dispatch_window_event(
            &mut app,
            id,
            &WindowEvent::Resized(id, LogicalSize::new(1024.0, 768.0)),
        );
        assert_eq!(app.window_events.len(), 1);
        assert!(aqua.scheduler.has_pending());
        let rec = aqua.windows.get(&id).unwrap();
        assert_eq!(rec.observed.inner_size, LogicalSize::new(1024.0, 768.0));
        // Ready → produced → presented releases the slot, no GPU needed.
        let now = Instant::now();
        let ready = aqua.scheduler.ready_windows(now);
        assert_eq!(ready.len(), 1);
        aqua.scheduler.mark_produced(&[id], now);
        aqua.scheduler.mark_presented(id);
        assert!(!aqua.scheduler.has_pending());
    }

    #[test]
    fn input_requests_frame_and_deadline() {
        use aqua_core::{PointerButton as CoreButton, PointerEvent as CorePointer};
        let (mut aqua, mut app, id) = headless();
        aqua.dispatch_input(
            &mut app,
            id,
            &CoreInputEvent::PointerDown(
                CorePointer {
                    contact: 0,
                    kind: PointerKind::Mouse,
                    position: aqua_core::Point::new(10.0, 20.0),
                    buttons: 1,
                },
                CoreButton::Primary,
            ),
        );
        assert_eq!(app.inputs.len(), 1);
        assert_eq!(app.cancels, 0);
        assert!(aqua.scheduler.has_pending());
    }

    #[test]
    fn host_driven_reactive_boundary_manual_flush_headless() {
        use std::cell::Cell;
        use std::rc::Rc;
        // Host-driven pattern: auto-flush off, wake hook installed, explicit
        // flush at the update boundary. Headless pumps flush manually.
        let rt = aqua_reactive::Runtime::new();
        rt.set_auto_flush(false);
        let scope = rt.create_scope();
        let (value, set_value) = scope.signal(0i32);
        let seen = Rc::new(Cell::new(-1i32));
        let seen_fx = seen.clone();
        let _fx = scope.subscribe(&value, move |v| seen_fx.set(v));
        set_value.set(1);
        set_value.set(2);
        assert!(
            rt.pending() > 0,
            "writes queue effects instead of running them"
        );
        let report = rt.flush();
        assert_eq!(report.ran, 1, "coalesced writes run the effect once");
        assert_eq!(seen.get(), 2, "effect observed the latest value");
        // bind_reactive headless: no proxy, wake callback installed but inert.
        let (mut aqua, _, id) = headless();
        let rt2 = aqua_reactive::Runtime::new();
        aqua.with_ctx(|ctx| ctx.bind_reactive(id, &rt2));
        assert!(aqua.flushers.contains_key(&id));
        drop(rt2); // app must keep owning the Runtime; handles diagnose use-after-drop
    }

    #[test]
    fn clipboard_round_trips_through_ctx_and_reports_backend() {
        let (mut aqua, _, _) = headless();
        // Backend is explicit (system where available, memory fallback headless).
        let _ = aqua.clipboard_backend();
        aqua.with_ctx(|ctx| {
            ctx.clipboard().set_text("copy me".to_string()).unwrap();
            assert_eq!(
                ctx.clipboard().get_text().unwrap().as_deref(),
                Some("copy me")
            );
            ctx.clipboard().clear().unwrap();
            assert_eq!(ctx.clipboard().get_text().unwrap(), None);
        });
    }

    #[test]
    fn file_drops_reach_their_own_hook() {
        use std::cell::RefCell;
        use std::rc::Rc;
        struct DropProbe {
            drops: Rc<RefCell<Vec<std::path::PathBuf>>>,
        }
        impl Application for DropProbe {
            fn on_window_event(
                &mut self,
                _ctx: &mut AppCtx,
                _id: AquaWindowId,
                _event: &WindowEvent,
            ) {
            }
            fn on_input(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, _event: &CoreInputEvent) {}
            fn build_scene(
                &mut self,
                _scene: &mut Scene,
                _window: &ObservedWindowState,
            ) -> Option<std::time::Instant> {
                None
            }
            fn on_files_dropped(
                &mut self,
                _ctx: &mut AppCtx,
                _id: AquaWindowId,
                paths: &[std::path::PathBuf],
            ) {
                self.drops.borrow_mut().extend(paths.iter().cloned());
            }
        }
        let (mut aqua, _, id) = headless();
        let drops = Rc::new(RefCell::new(Vec::new()));
        let mut app = DropProbe {
            drops: drops.clone(),
        };
        aqua.dispatch_file_drop(&mut app, id, &[std::path::PathBuf::from("/tmp/a.png")]);
        assert_eq!(drops.borrow().len(), 1);
        assert!(aqua.scheduler.has_pending());
    }

    #[test]
    fn prefs_and_a11y_defaults_are_honest() {
        let (aqua, _, _) = headless();
        // Unknown prefs behave as off (never guessed on).
        assert!(!aqua_platform::prefs::should_reduce_motion(
            aqua.preferences()
        ));
        // No snapshot published by default; no actions observed.
        struct Bare;
        impl Application for Bare {
            fn on_window_event(&mut self, _c: &mut AppCtx, _: AquaWindowId, _: &WindowEvent) {}
            fn on_input(&mut self, _c: &mut AppCtx, _: AquaWindowId, _: &CoreInputEvent) {}
            fn build_scene(
                &mut self,
                _: &mut Scene,
                _: &ObservedWindowState,
            ) -> Option<std::time::Instant> {
                None
            }
        }
        assert!(Bare.a11y_nodes().is_none());
    }

    #[test]
    fn pointer_cancel_arrives_as_cancel_never_release() {
        let (mut aqua, mut app, id) = headless();
        aqua.dispatch_input(&mut app, id, &CoreInputEvent::PointerCancel { contact: 7 });
        assert_eq!(app.cancels, 1);
        // Cancel carries only the contact: no position, no button that a
        // gesture could mistake for an activation.
        assert!(matches!(
            app.inputs[0],
            CoreInputEvent::PointerCancel { contact: 7 }
        ));
    }

    #[test]
    fn close_removes_window_and_occluded_suppresses_work() {
        let (mut aqua, mut app, id) = headless();
        aqua.dispatch_window_event(&mut app, id, &WindowEvent::Occluded(id, true));
        assert!(!aqua.scheduler.has_pending()); // occluded: no work scheduled
        aqua.dispatch_window_event(&mut app, id, &WindowEvent::CloseRequested(id));
        assert_eq!(aqua.window_count(), 0);
        assert!(!aqua.scheduler.has_pending());
    }

    #[test]
    fn headless_windows_carry_no_chrome_guard() {
        let (mut aqua, mut app, id) = headless();
        // No winit window exists headless: no guard installed, no overlay
        // claimed. The native fallback is the only honest state here.
        assert!(!aqua.has_chrome(id));
        aqua.dispatch_window_event(&mut app, id, &WindowEvent::CloseRequested(id));
        assert!(!aqua.has_chrome(id));
    }

    #[test]
    fn chrome_plan_pushes_only_on_change_and_clears_on_quiet() {
        let rects = || DragRegions {
            draggable: vec![(0.0, 0.0, 200.0, 40.0)],
            no_drag: vec![(160.0, 0.0, 200.0, 40.0)],
        };
        // Clean frames push nothing (no per-frame allocation/apply).
        assert!(chrome_plan(&None, None, 1.0).is_none());
        assert!(chrome_plan(&Some((rects(), 1.0)), Some(rects()), 1.0).is_none());
        // New explicit regions push.
        assert!(matches!(
            chrome_plan(&None, Some(rects()), 1.0),
            Some(ChromePlan::Apply(_))
        ));
        // Quiet hook after explicit rects clears stale holes (band persists).
        assert!(matches!(
            chrome_plan(&Some((rects(), 1.0)), None, 1.0),
            Some(ChromePlan::Clear)
        ));
    }

    #[test]
    fn chrome_plan_repushes_equal_regions_at_new_scale() {
        // The guard stores PHYSICAL rects: identical logical regions after
        // a ScaleChanged must be re-pushed with the new scale.
        let rects = || DragRegions {
            draggable: vec![(0.0, 0.0, 200.0, 40.0)],
            no_drag: vec![],
        };
        let cached = Some((rects(), 1.0));
        assert!(matches!(
            chrome_plan(&cached, Some(rects()), 2.0),
            Some(ChromePlan::Apply(_))
        ));
        // Same scale stays silent.
        assert!(chrome_plan(&cached, Some(rects()), 1.0).is_none());
        // No explicit regions anywhere: nothing physical is stale.
        assert!(chrome_plan(&None, None, 2.0).is_none());
    }

    #[test]
    fn application_chrome_regions_defaults_to_none() {
        struct Quiet;
        impl Application for Quiet {
            fn on_window_event(&mut self, _c: &mut AppCtx, _: AquaWindowId, _: &WindowEvent) {}
            fn on_input(&mut self, _c: &mut AppCtx, _: AquaWindowId, _: &CoreInputEvent) {}
            fn build_scene(&mut self, _: &mut Scene, _: &ObservedWindowState) -> Option<Instant> {
                None
            }
        }
        let (aqua, _, id) = headless();
        let observed = aqua.windows.get(&id).unwrap().observed.clone();
        assert!(Quiet.chrome_regions(&observed).is_none());
    }

    #[test]
    fn invalid_config_rejected_at_construction() {
        let mut bad = AquaConfig::default();
        bad.window.inner_size = LogicalSize::new(0.0, 0.0);
        assert!(Aqua::new(bad).is_err());
    }

    #[test]
    fn headless_input_sequence_drives_app_without_window_or_gpu() {
        use aqua_core::{
            Key as CoreKey, KeyEvent as CoreKeyEvent, Modifiers as CoreModifiers,
            PointerButton as CoreButton, PointerEvent as CorePointer,
        };
        let (mut aqua, mut app, id) = headless();
        let at = aqua_core::Point::new(50.0, 120.0);
        let down = CorePointer {
            contact: 0,
            kind: PointerKind::Mouse,
            position: at,
            buttons: 1,
        };
        let up = CorePointer {
            contact: 0,
            kind: PointerKind::Mouse,
            position: at,
            buttons: 0,
        };
        // Pointer move, press, release, cancel, then a text-producing key.
        for event in [
            CoreInputEvent::PointerMove(CorePointer {
                contact: 0,
                kind: PointerKind::Mouse,
                position: at,
                buttons: 0,
            }),
            CoreInputEvent::PointerDown(down, CoreButton::Primary),
            CoreInputEvent::PointerUp(up, CoreButton::Primary),
            CoreInputEvent::PointerCancel { contact: 0 },
            CoreInputEvent::KeyDown(CoreKeyEvent {
                key: CoreKey::Char('a'),
                modifiers: CoreModifiers::NONE,
                repeat: false,
            }),
        ] {
            aqua.dispatch_input(&mut app, id, &event);
        }
        assert_eq!(app.inputs.len(), 5);
        assert_eq!(app.cancels, 1);
        assert!(aqua.scheduler.has_pending());
        // Scene builds headless through the app (no GPU presentation).
        let observed = aqua.windows.get(&id).unwrap().observed.clone();
        let mut scene = Scene::new();
        assert_eq!(app.build_scene(&mut scene, &observed), None);
        assert_eq!(app.scenes_built, 1);
        assert_eq!(scene.len(), 1);
    }
}
