use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant as WallInstant};

use aqua_core::InputEvent as CoreInputEvent;
use aqua_core::{Modifiers as CoreModifiers, PointerKind};

#[cfg(windows)]
use aqua_platform::a11y_winit::A11yAdapter;
use aqua_platform::chrome::DragRegions;
use aqua_platform::events::{
    ime_is_composing, pointer_contact, to_core_point, translate_file_event, translate_ime_core,
    translate_key_event, translate_modifiers, translate_mouse_button_core,
    window_event_frame_reason, FileDrop,
};
use aqua_platform::host::{
    control_flow_for_next_wakeup, surface_signal, translate_window_event, window_attributes,
};
use aqua_platform::{
    a11y::{A11yActionRequest, A11yNode},
    prefs::{prefers_opaque_frame, query_preferences, OsPreferences},
    services::{app_clipboard, Clipboard, ClipboardBackend},
    AquaWindowId, FrameReason, ObservedWindowState, WindowEvent, WindowOptions, WindowRequest,
};
use aqua_protocol::Instant;
use aqua_reactive::{FlushReport, Runtime as ReactiveRuntime};
use aqua_render::{Renderer, Scene, SurfaceFacade};
use aqua_runtime::Scheduler;

use crate::config::AquaConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AquaUserEvent {
    ReactiveWake(AquaWindowId),
    Messages(AquaWindowId),
    WindowRequests,
    ReplayBadges,
    A11yActions(AquaWindowId),
    GpuComplete(AquaWindowId, u64),
}

enum ExternalUpdate {
    Register(u64, wgpu::TextureView, u32, u32),
    Remove(u64),
}

pub struct ExternalFrame<'a> {
    pub window: &'a ObservedWindowState,
    pub now: Instant,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub color_format: wgpu::TextureFormat,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    register: &'a mut dyn FnMut(ExternalUpdate),
}

impl ExternalFrame<'_> {
    pub fn unregister_external(&mut self, layer: u64) {
        (self.register)(ExternalUpdate::Remove(layer));
    }

    pub fn register_external(&mut self, layer: u64, view: wgpu::TextureView, w: u32, h: u32) {
        (self.register)(ExternalUpdate::Register(layer, view, w, h))
    }
}

struct WindowRecord {
    observed: ObservedWindowState,
    winit_id: Option<winit::window::WindowId>,
    transparent: bool,
    vsync: bool,
    snapshot_started: Option<Instant>,
    badge: Option<aqua_platform::badge::TaskbarBadge>,
    chrome: Option<aqua_platform::chrome::WindowsChrome>,
    window: Option<Arc<winit::window::Window>>,
    chrome_overlay: bool,
    last_regions: Option<(DragRegions, f64)>,
    gpu: Option<GpuWindow>,
    cursor: Option<(f64, f64)>,
    buttons: u8,
    modifiers: CoreModifiers,
    ime_active: bool,
    #[cfg(windows)]
    a11y: Option<A11yAdapter>,
}

struct GpuWindow {
    surface: SurfaceFacade,
    renderer: Renderer,
    ledger: GpuLedger,
    text_raster_mode: aqua_core::scene::TextRasterMode,
}

#[derive(Debug, Default)]
struct GpuLedger {
    submitted: u64,
    completed: u64,
    presented: u64,
    unreleased: u32,
}

impl GpuLedger {
    fn complete(&mut self, sequence: u64) -> Option<u32> {
        if sequence <= self.completed || sequence > self.submitted {
            return None;
        }
        let released = (sequence - self.completed).min(u64::from(self.unreleased)) as u32;
        self.completed = sequence;
        self.unreleased -= released;
        Some(released)
    }
}

fn complete_gpu_event(ledger: Option<&mut GpuLedger>, sequence: u64) -> Option<u32> {
    ledger.and_then(|ledger| ledger.complete(sequence))
}

#[derive(Clone, Debug)]
pub struct CaptureMetadata {
    pub adapter: wgpu::AdapterInfo,
    pub surface_format: wgpu::TextureFormat,
    pub surface_alpha_mode: wgpu::CompositeAlphaMode,
    pub present_mode: wgpu::PresentMode,
    pub text_raster_mode: aqua_core::scene::TextRasterMode,
    pub client_width: u32,
    pub client_height: u32,
    pub scale_factor: f64,
    pub submitted_sequence: u64,
    pub completed_sequence: u64,
}

struct SharedGpu {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

pub struct AppCtx<'a> {
    now: Instant,
    scheduler: &'a mut Scheduler,
    quit_requested: &'a mut bool,
    proxy: Option<winit::event_loop::EventLoopProxy<AquaUserEvent>>,
    flushers: &'a mut HashMap<AquaWindowId, Box<dyn FnMut() -> FlushReport>>,
    windows: &'a HashMap<AquaWindowId, WindowRecord>,
    shared_gpu: Option<&'a SharedGpu>,
    clipboard: &'a mut Box<dyn Clipboard>,
    window_requests: &'a mut VecDeque<WindowRequest>,
}

impl AppCtx<'_> {
    pub fn capture_metadata(&self, id: AquaWindowId) -> Option<CaptureMetadata> {
        let record = self.windows.get(&id)?;
        let window = record.window.as_ref()?;
        let gpu = record.gpu.as_ref()?;
        let size = window.inner_size();
        Some(CaptureMetadata {
            adapter: self.shared_gpu?.adapter.get_info(),
            surface_format: gpu.surface.format(),
            surface_alpha_mode: gpu.surface.alpha_mode(),
            present_mode: gpu.surface.present_mode(),
            text_raster_mode: gpu.text_raster_mode,
            client_width: size.width,
            client_height: size.height,
            scale_factor: window.scale_factor(),
            submitted_sequence: gpu.ledger.submitted,
            completed_sequence: gpu.ledger.completed,
        })
    }

    pub fn window_state(&self, id: AquaWindowId) -> Option<&ObservedWindowState> {
        self.windows.get(&id).map(|record| &record.observed)
    }

    pub fn dialog(
        &self,
        id: AquaWindowId,
        request: aqua_platform::dialogs::DialogRequest,
    ) -> Result<
        impl std::future::Future<
                Output = Result<Vec<std::path::PathBuf>, aqua_platform::dialogs::DialogError>,
            > + Send
            + 'static,
        aqua_platform::window::WindowError,
    > {
        use aqua_platform::window::WindowError;
        let window = self
            .windows
            .get(&id)
            .ok_or(WindowError::UnknownWindow(id))?
            .window
            .clone()
            .ok_or(WindowError::Unsupported("dialog requires a native window"))?;
        Ok(async move { request.execute_async_for(window.as_ref()).await })
    }

    pub fn display(&self, id: AquaWindowId) -> Option<aqua_platform::monitor::DisplayInfo> {
        aqua_platform::monitor::current(self.windows.get(&id)?.window.as_ref()?)
    }

    pub fn bind_mailbox<T>(&self, window: AquaWindowId, mailbox: &crate::mailbox::Mailbox<T>) {
        let proxy = self.proxy.clone();
        mailbox.set_waker(Arc::new(move || {
            if let Some(proxy) = &proxy {
                let _ = proxy.send_event(AquaUserEvent::Messages(window));
            }
        }));
    }

    pub fn request_window(
        &mut self,
        request: WindowRequest,
    ) -> Result<(), aqua_platform::window::WindowError> {
        match &request {
            WindowRequest::Create(options) => options.validated()?,
            WindowRequest::SetTitle(_, title) if title.contains('\0') => {
                return Err(aqua_platform::window::WindowError::InvalidTitle)
            }
            WindowRequest::SetInnerSize(_, size)
                if size.is_empty() || !size.width.is_finite() || !size.height.is_finite() =>
            {
                return Err(aqua_platform::window::WindowError::EmptySize);
            }
            _ => {}
        }
        self.window_requests.push_back(request);
        if let Some(proxy) = &self.proxy {
            let _ = proxy.send_event(AquaUserEvent::WindowRequests);
        }
        Ok(())
    }

    pub fn now(&self) -> Instant {
        self.now
    }

    pub fn request_frame(&mut self, id: AquaWindowId, reason: FrameReason) {
        self.scheduler.request_frame(id, reason);
    }

    pub fn request_animation_deadline(&mut self, id: AquaWindowId, at: Instant) {
        self.scheduler.request_animation_deadline(id, at);
    }

    pub fn clipboard(&mut self) -> &mut dyn Clipboard {
        &mut **self.clipboard
    }

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

    pub fn quit(&mut self) {
        *self.quit_requested = true;
    }
}

pub trait Application {
    fn on_click_settings(&mut self, _settings: aqua_core::ClickSettings) {}

    fn on_start(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId) {}

    fn on_window_request_error(
        &mut self,
        _request: &WindowRequest,
        _error: &aqua_platform::window::WindowError,
    ) {
    }

    fn can_close(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId) -> bool {
        true
    }

    fn on_wake(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId) {}

    fn on_frame_completed(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, _sequence: u64) {}

    fn cursor(&self) -> aqua_core::CursorIcon {
        aqua_core::CursorIcon::Default
    }

    fn on_window_event(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &WindowEvent);
    fn on_input(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &CoreInputEvent);
    fn on_files_dropped(
        &mut self,
        _ctx: &mut AppCtx,
        _id: AquaWindowId,
        _paths: &[std::path::PathBuf],
    ) {
    }
    fn on_files_dropped_at(
        &mut self,
        ctx: &mut AppCtx,
        id: AquaWindowId,
        paths: &[std::path::PathBuf],
        _at: Option<aqua_core::Point>,
    ) {
        self.on_files_dropped(ctx, id, paths);
    }

    fn on_a11y_action(
        &mut self,
        _ctx: &mut AppCtx,
        _id: AquaWindowId,
        _request: &A11yActionRequest,
    ) {
    }
    fn prepare_accessibility(
        &mut self,
        _window: &ObservedWindowState,
        _now: Instant,
    ) -> Option<Instant> {
        None
    }

    fn a11y_nodes(&self) -> Option<(aqua_core::NodeId, aqua_core::NodeId, Vec<A11yNode>)> {
        None
    }
    fn ime_cursor_area(&self) -> Option<aqua_core::Rect> {
        None
    }
    fn chrome_regions(&self, _window: &ObservedWindowState) -> Option<DragRegions> {
        None
    }
    fn externals(&mut self, _frame: &mut ExternalFrame<'_>) {}
    fn build_scene(&mut self, window: &ObservedWindowState, now: Instant) -> Option<Instant>;
    fn scene_mut(&mut self, _id: AquaWindowId) -> Option<&mut Scene> {
        None
    }
    fn reactive_runtime(&self) -> Option<&ReactiveRuntime> {
        None
    }
    fn on_render_error(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, _message: &str) {}
}

#[deprecated(
    since = "0.1.0",
    note = "low-level callback trait renamed to `Application`; root `App` is now the builder"
)]
pub use self::Application as App;

#[derive(Debug)]
enum ChromePlan {
    Apply(DragRegions),
    Clear,
}

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
    completion_tx: Option<std::sync::mpsc::Sender<(AquaWindowId, u64)>>,
    fonts: aqua_text::SharedFonts,
    runs: aqua_text::SharedRuns,
    prefs: OsPreferences,
    clipboard: Box<dyn Clipboard>,
    window_requests: VecDeque<WindowRequest>,
    clipboard_backend: ClipboardBackend,
    smoke_frames: Option<u64>,
    frames_presented: u64,
    smoke_deadline: Option<WallInstant>,
    epoch: WallInstant,
    wakeups: u64,
}

impl Aqua {
    pub fn new(config: AquaConfig) -> Result<Self, String> {
        Self::new_with_preferences(config, query_preferences())
    }

    pub(crate) fn new_with_preferences(
        mut config: AquaConfig,
        prefs: OsPreferences,
    ) -> Result<Self, String> {
        config
            .validated()
            .map_err(|e| format!("invalid window options: {e:?}"))?;
        let mut scheduler = Scheduler::new();
        config.apply_to_scheduler(&mut scheduler);
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
        let smoke_deadline = std::env::var("AQUA_SMOKE_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&ms| ms > 0)
            .map(|ms| WallInstant::now() + Duration::from_millis(ms));
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
            window_requests: VecDeque::new(),
            smoke_deadline,
            epoch: WallInstant::now(),
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

    pub fn has_chrome(&self, id: AquaWindowId) -> bool {
        self.windows
            .get(&id)
            .is_some_and(|r| r.chrome.is_some() && r.chrome_overlay)
    }

    pub fn clock(&self) -> Instant {
        Instant::from_nanos(u64::try_from(self.epoch.elapsed().as_nanos()).unwrap_or(u64::MAX))
    }

    fn wall(&self, at: Instant) -> WallInstant {
        self.epoch + at.since_origin()
    }

    fn bind_app_reactive(&mut self, app: &impl Application, id: AquaWindowId) {
        if self.flushers.contains_key(&id) {
            return;
        }
        if let Some(rt) = app.reactive_runtime() {
            self.with_ctx(|ctx| ctx.bind_reactive(id, rt));
        }
    }

    pub fn dispatch_start(&mut self, app: &mut impl Application, id: AquaWindowId) {
        if self.windows.contains_key(&id) {
            self.with_ctx(|ctx| app.on_start(ctx, id));
        }
    }

    pub fn dispatch_wake(&mut self, app: &mut impl Application, id: AquaWindowId) {
        if self.windows.contains_key(&id) {
            self.with_ctx(|ctx| app.on_wake(ctx, id));
        }
    }

    pub fn dispatch_window_requests(&mut self, app: &mut impl Application) {
        let requests: Vec<_> = self.window_requests.drain(..).collect();
        for request in requests {
            if let WindowRequest::Create(options) = &request {
                if self.proxy.is_some() {
                    self.window_requests.push_back(request);
                    continue;
                }
                let id = self.alloc_id();
                self.add_headless_window(ObservedWindowState {
                    id,
                    title: options.title.clone(),
                    inner_size: options.inner_size,
                    scale_factor: 1.0,
                    focused: false,
                    visible: false,
                    occluded: false,
                });
                self.dispatch_start(app, id);
                continue;
            }
            if let Err(error) = self.apply_window_request(app, &request) {
                app.on_window_request_error(&request, &error);
            }
        }
    }

    fn apply_window_request(
        &mut self,
        app: &mut impl Application,
        request: &WindowRequest,
    ) -> Result<(), aqua_platform::window::WindowError> {
        use aqua_platform::window::WindowError;
        let id = match request {
            WindowRequest::Create(_) => {
                return Err(WindowError::Unsupported("requires active event loop"))
            }
            WindowRequest::SetTitle(id, _)
            | WindowRequest::SetInnerSize(id, _)
            | WindowRequest::SetVisible(id, _)
            | WindowRequest::RequestRedraw(id)
            | WindowRequest::SetMinimized(id, _)
            | WindowRequest::SetMaximized(id, _)
            | WindowRequest::Close(id)
            | WindowRequest::Activate(id)
            | WindowRequest::SetBadge(id, _)
            | WindowRequest::SetMaxFrameRate(id, _)
            | WindowRequest::SetVsync(id, _)
            | WindowRequest::SetOuterPosition(id, _)
            | WindowRequest::Center(id)
            | WindowRequest::BeginSnapshotTransition(id, _)
            | WindowRequest::SetTextRasterMode(id, _) => *id,
        };
        let window = self
            .windows
            .get(&id)
            .ok_or(WindowError::UnknownWindow(id))?
            .window
            .clone();
        match request {
            WindowRequest::Close(_) => {
                if self.with_ctx(|ctx| app.can_close(ctx, id)) {
                    self.with_ctx(|ctx| app.on_window_event(ctx, id, &WindowEvent::Destroyed(id)));
                    self.remove_window(id);
                }
            }
            WindowRequest::SetTextRasterMode(_, mode) => {
                let record = self.windows.get_mut(&id).expect("validated window");
                let gpu = record.gpu.as_mut().ok_or(WindowError::Unsupported(
                    "text raster mode requires a renderer",
                ))?;
                gpu.renderer
                    .set_text_raster_mode(*mode)
                    .map_err(|error| WindowError::Native(error.to_string()))?;
                gpu.text_raster_mode = *mode;
                gpu.surface.request_present();
                self.scheduler.request_frame(id, FrameReason::EXPLICIT);
            }
            WindowRequest::BeginSnapshotTransition(_, settings) => {
                let now = self.clock();
                let reduced = self.motion_reduced();
                let record = self.windows.get_mut(&id).expect("validated window");
                let gpu = record
                    .gpu
                    .as_mut()
                    .ok_or(WindowError::Unsupported("snapshot requires a renderer"))?;
                if reduced {
                    gpu.renderer.cancel_snapshot_transition();
                    record.snapshot_started = None;
                } else {
                    gpu.renderer
                        .start_snapshot_transition(*settings)
                        .map_err(|error| WindowError::Native(error.to_string()))?;
                    record.snapshot_started = Some(now);
                }
                gpu.surface.request_present();
                self.scheduler.request_frame(id, FrameReason::EXPLICIT);
            }
            WindowRequest::SetMaxFrameRate(_, target) => {
                self.scheduler.set_window_fps_target(id, *target);
                self.scheduler.request_frame(id, FrameReason::EXPLICIT);
            }
            WindowRequest::SetVsync(_, enabled) => {
                if let Some(record) = self.windows.get_mut(&id) {
                    if let (Some(gpu), Some(shared)) =
                        (record.gpu.as_mut(), self.shared_gpu.as_ref())
                    {
                        let mode = if *enabled {
                            wgpu::PresentMode::AutoVsync
                        } else {
                            wgpu::PresentMode::AutoNoVsync
                        };
                        gpu.surface
                            .set_present_mode(&shared.device, &shared.adapter, mode)
                            .map_err(|error| WindowError::Native(error.to_string()))?;
                    }
                    record.vsync = *enabled;
                }
                self.scheduler.request_frame(id, FrameReason::EXPLICIT);
            }
            WindowRequest::SetBadge(_, badge) => {
                self.windows.get_mut(&id).expect("validated window").badge = *badge;
                if let Some(window) = window.as_ref() {
                    aqua_platform::badge::set_badge(window, *badge)
                        .map_err(|error| WindowError::Native(error.to_string()))?;
                }
            }
            WindowRequest::RequestRedraw(_) => {
                self.scheduler.request_frame(id, FrameReason::EXPLICIT);
            }
            _ => {
                if let Some(window) = window.as_ref() {
                    match request {
                        WindowRequest::SetTitle(_, title) => {
                            window.set_title(title);
                            if let Some(record) = self.windows.get_mut(&id) {
                                record.observed.title = window.title();
                            }
                        }
                        WindowRequest::SetInnerSize(_, size) => {
                            let _ = window.request_inner_size(winit::dpi::LogicalSize::new(
                                size.width,
                                size.height,
                            ));
                        }
                        WindowRequest::SetVisible(_, visible) => {
                            window.set_visible(*visible);
                            if let (Some(record), Some(visible)) =
                                (self.windows.get_mut(&id), window.is_visible())
                            {
                                record.observed.visible = visible;
                            }
                        }
                        WindowRequest::SetMinimized(_, minimized) => {
                            window.set_minimized(*minimized)
                        }
                        WindowRequest::SetMaximized(_, maximized) => {
                            window.set_maximized(*maximized)
                        }
                        WindowRequest::Activate(_) => {
                            window.set_minimized(false);
                            window.focus_window();
                        }
                        WindowRequest::SetOuterPosition(_, position) => window.set_outer_position(
                            winit::dpi::PhysicalPosition::new(position.x, position.y),
                        ),
                        WindowRequest::Center(_) => {
                            if let Some(display) = aqua_platform::monitor::current(window) {
                                let size = window.outer_size();
                                let position = display.center_physical(size.width, size.height);
                                window.set_outer_position(winit::dpi::PhysicalPosition::new(
                                    position.x, position.y,
                                ));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }

    fn alloc_id(&mut self) -> AquaWindowId {
        let id = AquaWindowId::new(self.next_id).expect("window id overflow");
        self.next_id += 1;
        id
    }

    fn with_ctx<R>(&mut self, f: impl FnOnce(&mut AppCtx) -> R) -> R {
        let now = self.clock();
        let quit = &mut self.quit_requested;
        let proxy = self.proxy.clone();
        let mut ctx = AppCtx {
            now,
            scheduler: &mut self.scheduler,
            quit_requested: &mut *quit,
            proxy,
            flushers: &mut self.flushers,
            windows: &self.windows,
            shared_gpu: self.shared_gpu.as_ref(),
            clipboard: &mut self.clipboard,
            window_requests: &mut self.window_requests,
        };
        f(&mut ctx)
    }

    pub fn add_headless_window(&mut self, observed: ObservedWindowState) -> AquaWindowId {
        let id = observed.id;
        self.next_id = self.next_id.max(id.raw().saturating_add(1));
        self.windows.insert(
            id,
            WindowRecord {
                observed,
                winit_id: None,
                transparent: false,
                vsync: self.config.vsync,
                snapshot_started: None,
                badge: None,
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

    pub fn dispatch_window_event(
        &mut self,
        app: &mut impl Application,
        id: AquaWindowId,
        event: &WindowEvent,
    ) {
        if !self.windows.contains_key(&id) {
            return;
        }
        self.apply_observed(id, event);
        if let Some(reason) = window_event_frame_reason(event) {
            self.scheduler.request_frame(id, reason);
        }
        let ev = event.clone();
        self.bind_app_reactive(app, id);
        self.with_ctx(|ctx| app.on_window_event(ctx, id, &ev));
        if let WindowEvent::Focused(_, focused) = event {
            self.with_ctx(|ctx| app.on_input(ctx, id, &CoreInputEvent::Focused(*focused)));
        }
        let close = match event {
            WindowEvent::Destroyed(_) => true,
            WindowEvent::CloseRequested(_) => self.with_ctx(|ctx| app.can_close(ctx, id)),
            _ => false,
        };
        if close {
            self.remove_window(id);
        }
    }

    pub fn dispatch_input(
        &mut self,
        app: &mut impl Application,
        id: AquaWindowId,
        event: &CoreInputEvent,
    ) {
        if !self.windows.contains_key(&id) {
            return;
        }
        self.scheduler.request_frame(id, FrameReason::INPUT);
        let ev = event.clone();
        self.with_ctx(|ctx| app.on_input(ctx, id, &ev));
    }

    pub fn dispatch_file_drop(
        &mut self,
        app: &mut impl Application,
        id: AquaWindowId,
        paths: &[std::path::PathBuf],
    ) {
        self.dispatch_file_drop_at(app, id, paths, None);
    }

    pub fn dispatch_file_drop_at(
        &mut self,
        app: &mut impl Application,
        id: AquaWindowId,
        paths: &[std::path::PathBuf],
        at: Option<aqua_core::Point>,
    ) {
        if !self.windows.contains_key(&id) {
            return;
        }
        self.scheduler.request_frame(id, FrameReason::INPUT);
        let owned = paths.to_vec();
        self.with_ctx(|ctx| app.on_files_dropped_at(ctx, id, &owned, at));
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
                    if !f {
                        rec.buttons = 0;
                        rec.modifiers = CoreModifiers::NONE;
                        rec.ime_active = false;
                    }
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
        if let Some(rec) = self.windows.remove(&id) {
            if let Some(wid) = rec.winit_id {
                self.winit_map.remove(&wid);
            }
        }
        self.scheduler.remove_window(id);
        self.flushers.remove(&id);
    }

    pub fn event_proxy(&self) -> Option<winit::event_loop::EventLoopProxy<AquaUserEvent>> {
        self.proxy.clone()
    }

    pub fn set_runs(&mut self, runs: aqua_text::SharedRuns) {
        self.runs = runs;
    }

    pub fn runs(&self) -> &aqua_text::SharedRuns {
        &self.runs
    }

    pub fn text_backend(&self) -> aqua_render::text::TextBackend {
        aqua_render::text::TextBackend::new(self.fonts.clone(), self.runs.clone())
    }

    pub fn set_fonts(&mut self, fonts: aqua_text::SharedFonts) {
        self.fonts = fonts;
    }

    pub fn fonts(&self) -> &aqua_text::SharedFonts {
        &self.fonts
    }

    pub fn gpu_stats(&self, id: AquaWindowId) -> Option<(u64, u64, u64)> {
        self.windows
            .get(&id)
            .and_then(|r| r.gpu.as_ref())
            .map(|g| (g.ledger.submitted, g.ledger.presented, g.ledger.completed))
    }

    pub fn preferences(&self) -> &OsPreferences {
        &self.prefs
    }

    pub(crate) fn motion_reduced(&self) -> bool {
        self.prefs.reduce_motion.is_on() || self.config.reduced_motion
    }

    pub fn clipboard_backend(&self) -> ClipboardBackend {
        self.clipboard_backend
    }

    pub fn run(mut self, app: impl Application + 'static) -> Result<(), String> {
        if self.config.headless {
            return Err("headless config cannot run the GUI loop; use the headless pump".into());
        }
        let mut builder = winit::event_loop::EventLoop::<AquaUserEvent>::with_user_event();
        #[cfg(windows)]
        let taskbar_proxy = std::rc::Rc::new(std::cell::RefCell::new(
            None::<winit::event_loop::EventLoopProxy<AquaUserEvent>>,
        ));
        #[cfg(windows)]
        {
            let proxy = taskbar_proxy.clone();
            aqua_platform::badge::install_taskbar_wake(&mut builder, move || {
                if let Some(proxy) = proxy.borrow().as_ref() {
                    let _ = proxy.send_event(AquaUserEvent::ReplayBadges);
                }
            })
            .map_err(|error| error.to_string())?;
        }
        let event_loop = builder
            .build()
            .map_err(|e| format!("event loop unavailable: {e:?}"))?;
        self.proxy = Some(event_loop.create_proxy());
        #[cfg(windows)]
        {
            *taskbar_proxy.borrow_mut() = self.proxy.clone();
        }
        let mut loop_ = Loop {
            aqua: self,
            app,
            startup_error: None,
        };
        event_loop
            .run_app(&mut loop_)
            .map_err(|e| format!("event loop failed: {e:?}"))?;
        match loop_.startup_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

struct Loop<A: Application> {
    aqua: Aqua,
    app: A,
    startup_error: Option<String>,
}

impl<A: Application> Loop<A> {
    fn process_window_requests(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let requests: Vec<_> = self.aqua.window_requests.drain(..).collect();
        for request in requests {
            let result = match &request {
                WindowRequest::Create(options) => {
                    self.create_window(event_loop, options.clone()).map(|_| ())
                }
                _ => self.aqua.apply_window_request(&mut self.app, &request),
            };
            if let Err(error) = result {
                self.app.on_window_request_error(&request, &error);
            }
        }
        if self.aqua.windows.is_empty() && !self.aqua.config.headless {
            event_loop.exit();
        }
    }
    fn create_window(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        options: WindowOptions,
    ) -> Result<AquaWindowId, aqua_platform::window::WindowError> {
        options.validated()?;
        let mut attrs = window_attributes(&options);
        if self.aqua.config.material != aqua_platform::WindowMaterial::None {
            attrs = attrs.with_transparent(true);
        }
        #[cfg(windows)]
        {
            attrs = attrs.with_visible(false);
        }
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.aqua.render_errors += 1;
                return Err(aqua_platform::window::WindowError::Native(format!(
                    "window creation failed: {e}"
                )));
            }
        };
        let id = self.aqua.alloc_id();
        let chrome = match aqua_platform::chrome::WindowsChrome::install(&window, &options) {
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
            && options.effective_titlebar() == aqua_platform::window::Titlebar::Overlay;
        let scale = window.scale_factor();
        let size = window.inner_size();
        let observed = ObservedWindowState {
            id,
            title: options.title.clone(),
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
                badge: None,
                vsync: self.aqua.config.vsync,
                snapshot_started: None,
                transparent: options.transparent
                    || self.aqua.config.material != aqua_platform::WindowMaterial::None,
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
            let proxy = self.aqua.proxy.clone();
            let waker: aqua_platform::a11y::WakeFn = std::sync::Arc::new(move || {
                if let Some(p) = &proxy {
                    let _ = p.send_event(AquaUserEvent::A11yActions(id));
                }
            });
            match A11yAdapter::attach_with_waker(event_loop, &window, waker) {
                Ok(mut adapter) => {
                    let now = self.aqua.clock();
                    if let Some(observed) = self
                        .aqua
                        .windows
                        .get(&id)
                        .map(|record| record.observed.clone())
                    {
                        if let Some(deadline) = self.app.prepare_accessibility(&observed, now) {
                            self.aqua.scheduler.request_animation_deadline(id, deadline);
                        }
                    }
                    if let Some((root, focus, items)) = self.app.a11y_nodes() {
                        adapter.push_nodes(root, focus, &items);
                    }
                    if let Some(rec) = self.aqua.windows.get_mut(&id) {
                        rec.a11y = Some(adapter);
                    }
                    window.set_visible(true);
                }
                Err(e) => {
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
            self.aqua.remove_window(id);
            return Err(aqua_platform::window::WindowError::Native(format!(
                "GPU setup failed: {e}"
            )));
        }
        self.aqua.with_ctx(|ctx| self.app.on_start(ctx, id));
        self.aqua.scheduler.request_frame(id, FrameReason::EXPOSE);
        Ok(id)
    }

    fn ensure_gpu(
        &mut self,
        _id: AquaWindowId,
        window: &Arc<winit::window::Window>,
    ) -> Result<(), String> {
        if self.aqua.shared_gpu.is_some() {
            return Ok(());
        }
        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_desc.backends = wgpu::Backends::all();
        #[cfg(windows)]
        if self
            .aqua
            .windows
            .get(&_id)
            .is_some_and(|record| record.transparent)
        {
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
        self.ensure_gpu(id, window)?;
        let gpu = self.aqua.shared_gpu.as_ref().expect("gpu ensured");
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Err("zero-sized surface; window not yet laid out".into());
        }
        let mode = if self
            .aqua
            .windows
            .get(&id)
            .is_some_and(|record| record.vsync)
        {
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
        if self
            .aqua
            .windows
            .get(&id)
            .is_some_and(|record| record.transparent)
        {
            eprintln!(
                "aqua: transparent surface: {:?}, alpha={:?}, format={:?}",
                gpu.adapter.get_info().backend,
                facade.alpha_mode(),
                facade.format(),
            );
        }
        let scale = window.scale_factor() as f32;
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
                ledger: GpuLedger::default(),
                text_raster_mode: aqua_core::scene::TextRasterMode::default(),
            });
        }
        Ok(())
    }

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
                    if let (Ok(_), Some(p)) =
                        (device.poll(wgpu::PollType::wait_indefinitely()), &proxy)
                    {
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
            if let Some(flusher) = self.aqua.flushers.get_mut(&id) {
                let _report = flusher();
            }
            let observed = self.aqua.windows.get(&id).map(|r| r.observed.clone());
            let Some(observed) = observed else {
                self.aqua.scheduler.mark_presented(id);
                continue;
            };
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
                let mut updates = Vec::new();
                let mut register = |update| updates.push(update);
                let mut frame = ExternalFrame {
                    window: &observed,
                    now,
                    device: &shared.device,
                    queue: &shared.queue,
                    color_format: format,
                    width,
                    height,
                    scale,
                    register: &mut register,
                };
                self.app.externals(&mut frame);
                for update in updates {
                    match update {
                        ExternalUpdate::Register(layer, view, width, height) => {
                            gpu.renderer.register_external(layer, view, width, height)
                        }
                        ExternalUpdate::Remove(layer) => {
                            gpu.renderer.unregister_external(layer);
                        }
                    }
                }
            }
            if let Some(t) = self.app.build_scene(&observed, now) {
                self.aqua.scheduler.request_animation_deadline(id, t);
            }
            {
                let next = self.app.chrome_regions(&observed);
                let scale = observed.scale_factor;
                let scale = if scale.is_finite() && scale > 0.0 {
                    scale
                } else {
                    1.0
                };
                if let Some(rec) = self.aqua.windows.get_mut(&id) {
                    if let Some(plan) = chrome_plan(&rec.last_regions, next, scale) {
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
            #[cfg(windows)]
            {
                let needed = self
                    .aqua
                    .windows
                    .get(&id)
                    .and_then(|window| window.a11y.as_ref())
                    .is_some_and(|adapter| adapter.needs_snapshot());
                let snapshot = if needed { self.app.a11y_nodes() } else { None };
                if let Some((root, focus, items)) = snapshot {
                    if let Some(rec) = self.aqua.windows.get_mut(&id) {
                        if let Some(adapter) = rec.a11y.as_mut() {
                            adapter.push_nodes(root, focus, &items);
                        }
                    }
                }
            }
            {
                let area = self.app.ime_cursor_area();
                if let Some(rec) = self.aqua.windows.get(&id) {
                    if let Some(window) = rec.window.as_ref() {
                        use aqua_platform::services::{set_ime_cursor_area, ImeCursorArea};
                        let cursor = self.app.cursor();
                        window.set_cursor_visible(cursor != aqua_core::CursorIcon::Hidden);
                        window.set_cursor(aqua_platform::host::to_winit_cursor(cursor));
                        window.set_ime_allowed(rec.observed.focused && area.is_some());
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
            let snapshot_active = rec.snapshot_started.is_some_and(|started| {
                gpu.renderer
                    .set_snapshot_transition_elapsed(now.saturating_duration_since(started))
            });
            if rec.snapshot_started.is_some() && !snapshot_active {
                rec.snapshot_started = None;
                gpu.surface.request_present();
            }
            let snapshot_interval = rec
                .window
                .as_ref()
                .and_then(|window| window.current_monitor())
                .and_then(|monitor| monitor.refresh_rate_millihertz())
                .filter(|rate| *rate > 0)
                .map(|rate| Duration::from_secs_f64(1000.0 / f64::from(rate)))
                .unwrap_or(Duration::from_secs_f64(1.0 / 60.0));
            enum Outcome {
                Submitted { seq: u64 },
                Skipped,
                Failed { message: String, held: u32 },
            }
            let drawn = match self.app.scene_mut(id) {
                Some(scene) => gpu.surface.acquire_and_draw(&mut gpu.renderer, scene),
                None => Ok(false),
            };
            let outcome = match drawn {
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
            if snapshot_active {
                self.aqua
                    .scheduler
                    .request_animation_deadline(id, now + snapshot_interval);
            }
            match outcome {
                Outcome::Submitted { seq } => {
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
                self.draw_ready(self.aqua.clock());
                return;
            }
            if let Some(reason) = window_event_frame_reason(&framework) {
                self.aqua.scheduler.request_frame(id, reason);
            }
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
            self.aqua.bind_app_reactive(&self.app, id);
            self.aqua
                .with_ctx(|ctx| self.app.on_window_event(ctx, id, &fw));
            if let WindowEvent::Focused(_, focused) = fw {
                self.aqua.with_ctx(|ctx| {
                    self.app
                        .on_input(ctx, id, &CoreInputEvent::Focused(focused))
                });
            }
            #[cfg(windows)]
            Self::drain_a11y_actions(&mut self.aqua, &mut self.app, id);
            let close = match fw {
                WindowEvent::Destroyed(_) => true,
                WindowEvent::CloseRequested(_) => {
                    self.aqua.with_ctx(|ctx| self.app.can_close(ctx, id))
                }
                _ => false,
            };
            if close {
                self.aqua.remove_window(id);
                if self.aqua.windows.is_empty() {
                    event_loop.exit();
                }
            }
            return;
        }
        self.route_input(event_loop, id, scale, event);
    }

    fn route_input(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        id: AquaWindowId,
        scale: f64,
        event: winit::event::WindowEvent,
    ) {
        use winit::event::WindowEvent as W;
        if let Some(drop) = translate_file_event(&event) {
            match drop {
                FileDrop::Dropped(path) => {
                    self.aqua.scheduler.request_frame(id, FrameReason::INPUT);
                    let at = self
                        .aqua
                        .windows
                        .get(&id)
                        .and_then(|record| record.window.as_ref())
                        .and_then(|window| aqua_platform::host::cursor_position(window));
                    self.aqua.with_ctx(|ctx| {
                        self.app
                            .on_files_dropped_at(ctx, id, std::slice::from_ref(&path), at)
                    });
                }
                FileDrop::Hovered(_) | FileDrop::HoverCancelled => {}
            }
            return;
        }
        if let W::ModifiersChanged(mods) = &event {
            let modifiers = translate_modifiers(mods);
            if let Some(rec) = self.aqua.windows.get_mut(&id) {
                rec.modifiers = modifiers;
            }
            self.aqua.with_ctx(|ctx| {
                self.app
                    .on_input(ctx, id, &CoreInputEvent::ModifiersChanged(modifiers))
            });
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
                    P::Cancelled => Some(CoreInputEvent::PointerCancel { contact: t.id }),
                    P::Moved => Some(CoreInputEvent::PointerMove(pointer(1))),
                }
            }
            W::Ime(ime) => {
                if let Some(rec) = self.aqua.windows.get_mut(&id) {
                    rec.ime_active = ime_is_composing(&ime);
                }
                Some(translate_ime_core(&ime))
            }
            W::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(rec) = self.aqua.windows.get_mut(&id) {
                    rec.observed.scale_factor = scale_factor;
                }
                self.aqua.scheduler.request_frame(id, FrameReason::RESIZE);
                let fw = WindowEvent::ScaleChanged(id, scale_factor);
                self.aqua.bind_app_reactive(&self.app, id);
                self.aqua
                    .with_ctx(|ctx| self.app.on_window_event(ctx, id, &fw));
                None
            }
            _ => None,
        };
        if let Some(input) = input {
            self.aqua.scheduler.request_frame(id, FrameReason::INPUT);
            if matches!(input, CoreInputEvent::PointerDown(_, _)) {
                if let Some(window) = self
                    .aqua
                    .windows
                    .get(&id)
                    .and_then(|record| record.window.as_ref())
                {
                    self.app
                        .on_click_settings(aqua_platform::host::click_settings(window));
                }
            }
            self.aqua.with_ctx(|ctx| self.app.on_input(ctx, id, &input));
        }
        if let Some(text_event) = extra {
            self.aqua.scheduler.request_frame(id, FrameReason::INPUT);
            self.aqua
                .with_ctx(|ctx| self.app.on_input(ctx, id, &text_event));
        }
    }

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
        if self.aqua.windows.is_empty() && !self.aqua.config.headless {
            let options = self.aqua.config.window.clone();
            if let Err(error) = self.create_window(event_loop, options.clone()) {
                self.app
                    .on_window_request_error(&WindowRequest::Create(options), &error);
                self.startup_error = Some(format!("startup window failed: {error:?}"));
                event_loop.exit();
            }
        }
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
        self.process_window_requests(event_loop);
        let wall_now = WallInstant::now();
        let now = self.aqua.clock();
        self.aqua.wakeups += 1;
        if let Some(deadline) = self.aqua.smoke_deadline {
            if wall_now >= deadline {
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
        let wake = match (
            self.aqua
                .scheduler
                .next_wakeup(now)
                .map(|t| self.aqua.wall(t)),
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
        event_loop: &winit::event_loop::ActiveEventLoop,
        event: AquaUserEvent,
    ) {
        match event {
            AquaUserEvent::ReplayBadges => {
                let requests: Vec<_> = self
                    .aqua
                    .windows
                    .iter()
                    .filter_map(|(id, record)| {
                        record
                            .badge
                            .map(|badge| WindowRequest::SetBadge(*id, Some(badge)))
                    })
                    .collect();
                for request in requests {
                    if let Err(error) = self.aqua.apply_window_request(&mut self.app, &request) {
                        self.app.on_window_request_error(&request, &error);
                    }
                }
            }
            AquaUserEvent::WindowRequests => self.process_window_requests(event_loop),
            AquaUserEvent::Messages(id) => {
                self.aqua.dispatch_wake(&mut self.app, id);
            }
            AquaUserEvent::ReactiveWake(id) => {
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
                self.aqua.scheduler.request_frame(id, FrameReason::INPUT);
                #[cfg(windows)]
                Self::drain_a11y_actions(&mut self.aqua, &mut self.app, id);
                #[cfg(not(windows))]
                let _ = id;
            }
            AquaUserEvent::GpuComplete(id, seq) => {
                let ledger = self
                    .aqua
                    .windows
                    .get_mut(&id)
                    .and_then(|record| record.gpu.as_mut())
                    .map(|gpu| &mut gpu.ledger);
                let release = complete_gpu_event(ledger, seq);
                if let Some(release) = release {
                    for _ in 0..release {
                        self.aqua.scheduler.mark_presented(id);
                    }
                    self.aqua
                        .with_ctx(|ctx| self.app.on_frame_completed(ctx, id, seq));
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
        scene: Scene,
    }

    impl Probe {
        fn new() -> Self {
            Self {
                window_events: Vec::new(),
                inputs: Vec::new(),
                scenes_built: 0,
                errors: Vec::new(),
                cancels: 0,
                scene: Scene::new(),
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
            if matches!(event, CoreInputEvent::PointerCancel { .. }) {
                self.cancels += 1;
            }
            self.inputs.push(event.clone());
            if matches!(event, CoreInputEvent::PointerDown(..)) {
                let now = ctx.now();
                ctx.request_animation_deadline(id, now);
            }
        }

        fn build_scene(&mut self, _window: &ObservedWindowState, _now: Instant) -> Option<Instant> {
            use aqua_core::{Oklch, Rect};
            self.scenes_built += 1;
            self.scene.insert(aqua_render::scene::DisplayItem::Rect {
                bounds: Rect::from_edges(0.0, 0.0, 100.0, 100.0),
                radius: 8.0,
                fill: Oklch::from_hex("#1d4ed8").unwrap(),
                border_width: 0.0,
                border_color: Oklch::from_hex("#000000").unwrap(),
                shadow: None,
            });
            None
        }

        fn scene_mut(&mut self, _id: AquaWindowId) -> Option<&mut Scene> {
            Some(&mut self.scene)
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
    fn gpu_completions_require_submitted_new_sequences_and_ignore_closed_windows() {
        let mut ledger = GpuLedger {
            submitted: 3,
            completed: 0,
            presented: 3,
            unreleased: 3,
        };
        assert_eq!(complete_gpu_event(Some(&mut ledger), 0), None);
        assert_eq!(complete_gpu_event(Some(&mut ledger), 4), None);
        assert_eq!(ledger.completed, 0);
        assert_eq!(complete_gpu_event(Some(&mut ledger), 2), Some(2));
        assert_eq!(ledger.unreleased, 1);
        assert_eq!(complete_gpu_event(Some(&mut ledger), 2), None);
        assert_eq!(complete_gpu_event(Some(&mut ledger), 1), None);
        assert_eq!(complete_gpu_event(None, 3), None);
        assert_eq!(complete_gpu_event(Some(&mut ledger), 3), Some(1));
        assert_eq!(ledger.completed, 3);
        assert_eq!(ledger.unreleased, 0);
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
        let now = aqua.clock();
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
        let (mut aqua, _, id) = headless();
        let rt2 = aqua_reactive::Runtime::new();
        aqua.with_ctx(|ctx| ctx.bind_reactive(id, &rt2));
        assert!(aqua.flushers.contains_key(&id));
        drop(rt2);
    }

    #[test]
    fn clipboard_round_trips_through_ctx_and_reports_backend() {
        let (mut aqua, _, _) = headless();
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
                _window: &ObservedWindowState,
                _now: Instant,
            ) -> Option<Instant> {
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
        assert!(!aqua_platform::prefs::should_reduce_motion(
            aqua.preferences()
        ));
        struct Bare;
        impl Application for Bare {
            fn on_window_event(&mut self, _c: &mut AppCtx, _: AquaWindowId, _: &WindowEvent) {}
            fn on_input(&mut self, _c: &mut AppCtx, _: AquaWindowId, _: &CoreInputEvent) {}
            fn build_scene(&mut self, _: &ObservedWindowState, _: Instant) -> Option<Instant> {
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
        assert!(matches!(
            app.inputs[0],
            CoreInputEvent::PointerCancel { contact: 7 }
        ));
    }

    #[test]
    fn close_removes_window_and_occluded_suppresses_work() {
        let (mut aqua, mut app, id) = headless();
        aqua.dispatch_window_event(&mut app, id, &WindowEvent::Occluded(id, true));
        assert!(!aqua.scheduler.has_pending());
        aqua.dispatch_window_event(&mut app, id, &WindowEvent::CloseRequested(id));
        assert_eq!(aqua.window_count(), 0);
        assert!(!aqua.scheduler.has_pending());
    }

    #[test]
    fn headless_windows_carry_no_chrome_guard() {
        let (mut aqua, mut app, id) = headless();
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
        assert!(chrome_plan(&None, None, 1.0).is_none());
        assert!(chrome_plan(&Some((rects(), 1.0)), Some(rects()), 1.0).is_none());
        assert!(matches!(
            chrome_plan(&None, Some(rects()), 1.0),
            Some(ChromePlan::Apply(_))
        ));
        assert!(matches!(
            chrome_plan(&Some((rects(), 1.0)), None, 1.0),
            Some(ChromePlan::Clear)
        ));
    }

    #[test]
    fn chrome_plan_repushes_equal_regions_at_new_scale() {
        let rects = || DragRegions {
            draggable: vec![(0.0, 0.0, 200.0, 40.0)],
            no_drag: vec![],
        };
        let cached = Some((rects(), 1.0));
        assert!(matches!(
            chrome_plan(&cached, Some(rects()), 2.0),
            Some(ChromePlan::Apply(_))
        ));
        assert!(chrome_plan(&cached, Some(rects()), 1.0).is_none());
        assert!(chrome_plan(&None, None, 2.0).is_none());
    }

    #[test]
    fn application_chrome_regions_defaults_to_none() {
        struct Quiet;
        impl Application for Quiet {
            fn on_window_event(&mut self, _c: &mut AppCtx, _: AquaWindowId, _: &WindowEvent) {}
            fn on_input(&mut self, _c: &mut AppCtx, _: AquaWindowId, _: &CoreInputEvent) {}
            fn build_scene(&mut self, _: &ObservedWindowState, _: Instant) -> Option<Instant> {
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
        let observed = aqua.windows.get(&id).unwrap().observed.clone();
        assert_eq!(app.build_scene(&observed, aqua.clock()), None);
        assert_eq!(app.scenes_built, 1);
        assert_eq!(app.scene_mut(id).map(|scene| scene.len()), Some(1));
    }
}
