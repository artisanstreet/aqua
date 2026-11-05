use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant as WallInstant};

use aqua_core::InputEvent as CoreInputEvent;
use aqua_core::Modifiers as CoreModifiers;
use aqua_graph::Extent;
use aqua_protocol::chrome::DragRegions;
use aqua_protocol::prefs::{SystemPrefs as OsPreferences, prefers_opaque_frame};
use aqua_protocol::{
    A11yActionRequest, A11yNode, AccessTreeUpdate, DialogOutcome, DialogSpec, GpuEvent, ImeCommand,
    ImeEvent, Instant, MotionCommand, MotionEvent, RenderCommand, RenderEvent, RequestId,
    RuntimeInput, RuntimeOutput, ShellCommand, WindowOp,
};
use aqua_protocol::{
    AquaWindowId, FrameReason, ObservedWindowState, WindowError, WindowEvent, WindowOptions,
    WindowRequest,
};
use aqua_reactive::{FlushReport, Runtime as ReactiveRuntime};
use aqua_runtime::Scheduler;
use aqua_scene::Scene;
use aqua_shell_win32::capability::query_preferences;
use aqua_shell_win32::services::{Clipboard, ClipboardBackend, app_clipboard};

use crate::config::AquaConfig;
use crate::inbox::{AquaUserEvent, Inbox};
use crate::native_window::NativeWindow;
use crate::render::host::Clock;
use crate::render::link::{CompositionSink, ExternalUpdate, InitFactory, RenderLink};
use crate::render::pressure::MemoryPressure;
use crate::render::service::RenderService;
use crate::render::transition::{Invalid, Settings};

pub use aqua_ui::input::ExternalFileDragEvent as ExternalFileDrag;

pub trait AccessSink {
    fn needs_snapshot(&self) -> bool;
    fn connected(&self) -> bool {
        self.needs_snapshot()
    }
    fn connection_epoch(&self) -> u64 {
        0
    }
    fn apply(&mut self, update: &AccessTreeUpdate);
    fn drain_actions(&mut self) -> Vec<A11yActionRequest>;
}

pub trait ImeSink {
    fn command(&mut self, command: ImeCommand);
    fn drain(&mut self, out: &mut Vec<ImeEvent>);
}

impl<B: aqua_protocol::ImeBackend> ImeSink for B {
    fn command(&mut self, command: ImeCommand) {
        if let Err(error) = aqua_protocol::ImeBackend::command(self, command) {
            eprintln!("aqua: ime: {error}");
        }
    }

    fn drain(&mut self, out: &mut Vec<ImeEvent>) {
        aqua_protocol::ImeBackend::drain(self, out);
    }
}

pub struct ExternalFrame<'a> {
    pub window: &'a ObservedWindowState,
    pub now: Instant,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    register: &'a mut dyn FnMut(ExternalUpdate),
}

impl ExternalFrame<'_> {
    pub fn meshes_supported(&self) -> bool {
        true
    }

    pub fn register_image(&mut self, layer: u64, width: u32, height: u32, rgba: Arc<[u8]>) {
        (self.register)(ExternalUpdate::Image {
            layer,
            width,
            height,
            rgba,
        });
    }

    pub fn unregister_external(&mut self, layer: u64) {
        (self.register)(ExternalUpdate::Release { layer });
    }
}

fn invalid_transition(invalid: Invalid) -> WindowError {
    WindowError::Unsupported(match invalid {
        Invalid::Duration => "snapshot transition needs a non-zero duration",
        Invalid::Band => "snapshot transition band must be in (0, 1]",
        Invalid::Tint => "snapshot transition tint must be finite",
    })
}

pub fn run_headless_externals<A: Application>(
    app: &mut A,
    window: &ObservedWindowState,
    now: Instant,
) -> Vec<ExternalUpdate> {
    let scale = window.scale_factor as f32;
    let width = ((window.inner_size.width as f32) * scale).round().max(1.0) as u32;
    let height = ((window.inner_size.height as f32) * scale).round().max(1.0) as u32;
    let mut updates = Vec::new();
    let mut register = |update| updates.push(update);
    let mut frame = ExternalFrame {
        window,
        now,
        width,
        height,
        scale,
        register: &mut register,
    };
    app.externals(&mut frame);
    updates
}

pub(crate) struct WindowRecord {
    pub(crate) observed: ObservedWindowState,
    pub(crate) last_invalidating_input: Option<Instant>,
    pub(crate) vsync: bool,
    pub(crate) last_regions: Option<(DragRegions, f64)>,
    pub(crate) buttons: u8,
    pub(crate) modifiers: CoreModifiers,
    pub(crate) native: Option<Rc<dyn NativeWindow>>,
    pub(crate) link: Option<RenderLink>,
    pub(crate) access: Option<Box<dyn AccessSink>>,
    pub(crate) ime: Option<Box<dyn ImeSink>>,
    pub(crate) presented: u64,
    pub(crate) access_connected: bool,
    pub(crate) access_epoch: u64,
}

impl WindowRecord {
    fn new(observed: ObservedWindowState, vsync: bool) -> Self {
        Self {
            observed,
            last_invalidating_input: None,
            vsync,
            last_regions: None,
            buttons: 0,
            modifiers: CoreModifiers::NONE,
            native: None,
            link: None,
            access: None,
            ime: None,
            presented: 0,
            access_connected: false,
            access_epoch: 0,
        }
    }
}

pub struct AppCtx<'a> {
    requested_frames: std::collections::HashSet<AquaWindowId>,
    now: Instant,
    scheduler: &'a mut Scheduler,
    quit_requested: &'a mut bool,
    inbox: Inbox,
    flushers: &'a mut HashMap<AquaWindowId, Box<dyn FnMut() -> FlushReport>>,
    windows: &'a HashMap<AquaWindowId, WindowRecord>,
    clipboard: &'a mut Box<dyn Clipboard>,
    window_requests: &'a mut VecDeque<WindowRequest>,
    dialogs: &'a mut VecDeque<(AquaWindowId, RequestId, DialogSpec)>,
    next_request: &'a mut u64,
}

impl AppCtx<'_> {
    pub fn window_state(&self, id: AquaWindowId) -> Option<&ObservedWindowState> {
        self.windows.get(&id).map(|record| &record.observed)
    }

    pub fn dialog(&mut self, id: AquaWindowId, spec: DialogSpec) -> RequestId {
        let request = RequestId(*self.next_request);
        *self.next_request += 1;
        self.dialogs.push_back((id, request, spec));
        self.inbox.post(AquaUserEvent::WindowRequests);
        request
    }

    pub fn render_stats(&self, id: AquaWindowId) -> Option<Arc<crate::render::host::RenderStats>> {
        self.windows
            .get(&id)?
            .link
            .as_ref()
            .map(|link| link.stats().clone())
    }

    pub fn request_trim(&self, id: AquaWindowId) {
        aqua_ui::layout::trim_measure_cache();
        if let Some(link) = self.windows.get(&id).and_then(|r| r.link.as_ref()) {
            link.request_trim();
        }
    }

    pub fn inject_device_loss(&self, id: AquaWindowId) {
        if let Some(link) = self.windows.get(&id).and_then(|r| r.link.as_ref()) {
            link.inject_device_loss();
        }
    }

    pub fn display(&self, id: AquaWindowId) -> Option<aqua_protocol::monitor::DisplayInfo> {
        self.windows.get(&id)?.native.as_ref()?.display()
    }

    #[track_caller]
    pub fn bind_mailbox<T>(&self, window: AquaWindowId, mailbox: &crate::mailbox::Mailbox<T>) {
        let inbox = self.inbox.clone();
        let location = std::panic::Location::caller();
        let origin = format!(
            "#{}:{}",
            location.file().rsplit(['/', '\\']).next().unwrap_or(""),
            location.line()
        );
        mailbox.set_waker(Arc::new(move || {
            inbox.post_labeled(AquaUserEvent::Messages(window), "messages", &origin);
        }));
    }

    pub fn request_window(
        &mut self,
        request: WindowRequest,
    ) -> Result<(), aqua_protocol::window::WindowError> {
        match &request {
            WindowRequest::Create(options) => options.validated()?,
            WindowRequest::SetTitle(_, title) if title.contains('\0') => {
                return Err(aqua_protocol::window::WindowError::InvalidTitle);
            }
            WindowRequest::SetInnerSize(_, size)
                if size.is_empty() || !size.width.is_finite() || !size.height.is_finite() =>
            {
                return Err(aqua_protocol::window::WindowError::EmptySize);
            }
            _ => {}
        }
        self.window_requests.push_back(request);
        self.inbox.post(AquaUserEvent::WindowRequests);
        Ok(())
    }

    pub fn now(&self) -> Instant {
        self.now
    }

    #[track_caller]
    pub fn request_frame(&mut self, id: AquaWindowId, reason: FrameReason) {
        if std::env::var_os("AQUA_TRACE_REQ").is_some() {
            eprintln!("req-frame {} {:?}", std::panic::Location::caller(), reason);
        }
        self.requested_frames.insert(id);
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
        let inbox = self.inbox.clone();
        rt.set_wake_callback(move || {
            inbox.post(AquaUserEvent::ReactiveWake(window));
        });
    }

    pub fn quit(&mut self) {
        *self.quit_requested = true;
    }
}

pub trait Application {
    fn on_external_file_drag(
        &mut self,
        _ctx: &mut AppCtx,
        _id: AquaWindowId,
        _event: &ExternalFileDrag,
    ) {
    }

    fn input_invalidation_epoch(&self, _id: AquaWindowId) -> u64 {
        0
    }

    fn color_profile(&self) -> aqua_core::ColorProfile {
        aqua_core::ColorProfile::AquaLinear
    }

    fn color_profile_for(&self, _id: AquaWindowId) -> aqua_core::ColorProfile {
        self.color_profile()
    }

    fn on_click_settings(&mut self, _settings: aqua_core::ClickSettings) {}

    fn on_caption_hover(
        &mut self,
        _ctx: &mut AppCtx,
        _id: AquaWindowId,
        _control: Option<aqua_protocol::CaptionControl>,
    ) {
    }

    fn on_start(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId) {}

    fn on_window_request_error(
        &mut self,
        _request: &WindowRequest,
        _error: &aqua_protocol::window::WindowError,
    ) {
    }

    fn can_close(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId) -> bool {
        true
    }

    fn on_wake(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId) {}

    fn on_frame_completed(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, _sequence: u64) {}

    fn on_dialog(
        &mut self,
        _ctx: &mut AppCtx,
        _id: AquaWindowId,
        _request: RequestId,
        _outcome: DialogOutcome,
    ) {
    }

    fn on_motion_event(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, _event: &MotionEvent) {}

    fn step_input(&mut self, _id: AquaWindowId, _input: RuntimeInput) {}

    fn take_output(&mut self, _id: AquaWindowId) -> RuntimeOutput {
        RuntimeOutput::default()
    }

    fn motion_commands(&mut self, _id: AquaWindowId) -> Vec<MotionCommand> {
        Vec::new()
    }

    fn cursor(&self) -> Option<aqua_core::CursorIcon> {
        None
    }

    fn cursor_for(&self, _id: AquaWindowId) -> Option<aqua_core::CursorIcon> {
        self.cursor()
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

    fn a11y_nodes_for(
        &self,
        _id: AquaWindowId,
    ) -> Option<(aqua_core::NodeId, aqua_core::NodeId, Vec<A11yNode>)> {
        self.a11y_nodes()
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
    fn reactive_runtime_for(&self, _id: AquaWindowId) -> Option<&ReactiveRuntime> {
        self.reactive_runtime()
    }
    fn on_render_error(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, _message: &str) {}
}

#[deprecated(
    since = "0.1.0",
    note = "low-level callback trait renamed to `Application`; root `App` is now the builder"
)]
pub use self::Application as App;

fn options_from_spec(spec: &aqua_protocol::WindowSpec) -> WindowOptions {
    use aqua_protocol::{TitlebarMode, WindowMaterial as SpecMaterial};
    let _ = SpecMaterial::Solid;
    WindowOptions {
        title: spec.title.clone(),
        inner_size: aqua_protocol::LogicalSize::new(f64::from(spec.size.w), f64::from(spec.size.h)),
        min_size: spec
            .min_size
            .map(|size| aqua_protocol::LogicalSize::new(f64::from(size.w), f64::from(size.h))),
        resizable: spec.resizable,
        titlebar: match spec.titlebar {
            TitlebarMode::Native => aqua_protocol::window::Titlebar::Native,
            TitlebarMode::Overlay => aqua_protocol::window::Titlebar::Overlay,
            TitlebarMode::Hidden => aqua_protocol::window::Titlebar::Hidden,
        },
        ..WindowOptions::default()
    }
}

const RESIZE_COST_CAP: Duration = Duration::from_millis(30);
const RESIZE_IDLE_RATIO: f64 = 1.5;

pub fn frame_reason(event: &WindowEvent) -> Option<FrameReason> {
    match event {
        WindowEvent::RedrawRequested(_) => Some(FrameReason::EXPLICIT),
        WindowEvent::Resized(..) | WindowEvent::ScaleChanged(..) => Some(FrameReason::RESIZE),
        WindowEvent::Focused(..) => Some(FrameReason::INPUT),
        WindowEvent::ThemeChanged(..) => Some(FrameReason::MATERIAL),
        WindowEvent::Occluded(_, false) => Some(FrameReason::EXPOSE),
        WindowEvent::Occluded(_, true)
        | WindowEvent::CloseRequested(_)
        | WindowEvent::Destroyed(_) => None,
    }
}

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

struct FrameStats {
    window_start: WallInstant,
    frames: u32,
    cpu_total: Duration,
    cpu_max: Duration,
    submit_total: Duration,
    submit_max: Duration,
    build_total: Duration,
    build_max: Duration,
    per_frame: bool,
}

impl FrameStats {
    fn from_environment() -> Option<Self> {
        std::env::var_os("AQUA_FRAME_STATS")
            .is_some_and(|value| value == "1")
            .then(|| Self {
                window_start: WallInstant::now(),
                frames: 0,
                cpu_total: Duration::ZERO,
                cpu_max: Duration::ZERO,
                submit_total: Duration::ZERO,
                submit_max: Duration::ZERO,
                build_total: Duration::ZERO,
                build_max: Duration::ZERO,
                per_frame: std::env::var_os("AQUA_PERF").is_some_and(|value| value == "1"),
            })
    }

    fn record(&mut self, cpu: Duration, submit: Duration, build: Duration) {
        if self.per_frame {
            let unix_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis());
            eprintln!(
                "aqua-perf-frame: unix_ms={unix_ms} cpu_ms={:.2} build_ms={:.2} submit_ms={:.2}",
                cpu.as_secs_f64() * 1000.0,
                build.as_secs_f64() * 1000.0,
                submit.as_secs_f64() * 1000.0,
            );
        }
        self.build_total += build;
        self.build_max = self.build_max.max(build);
        self.frames += 1;
        self.cpu_total += cpu;
        self.cpu_max = self.cpu_max.max(cpu);
        self.submit_total += submit;
        self.submit_max = self.submit_max.max(submit);
        let elapsed = self.window_start.elapsed();
        if elapsed >= Duration::from_secs(1) {
            let frames = self.frames.max(1);
            eprintln!(
                "aqua-frame-stats: frames={} cpu_avg_ms={:.2} cpu_max_ms={:.2} submit_avg_ms={:.2} submit_max_ms={:.2} build_avg_ms={:.2} build_max_ms={:.2}",
                self.frames,
                self.cpu_total.as_secs_f64() * 1000.0 / f64::from(frames),
                self.cpu_max.as_secs_f64() * 1000.0,
                self.submit_total.as_secs_f64() * 1000.0 / f64::from(frames),
                self.submit_max.as_secs_f64() * 1000.0,
                self.build_total.as_secs_f64() * 1000.0 / f64::from(frames),
                self.build_max.as_secs_f64() * 1000.0,
            );
            self.build_total = Duration::ZERO;
            self.build_max = Duration::ZERO;
            self.window_start = WallInstant::now();
            self.frames = 0;
            self.cpu_total = Duration::ZERO;
            self.cpu_max = Duration::ZERO;
            self.submit_total = Duration::ZERO;
            self.submit_max = Duration::ZERO;
        }
    }
}

type Compositions = Arc<Mutex<Vec<(AquaWindowId, Box<dyn Any + Send>)>>>;

pub struct Aqua {
    pub(crate) config: AquaConfig,
    pub(crate) scheduler: Scheduler,
    pub(crate) windows: HashMap<AquaWindowId, WindowRecord>,
    next_id: u64,
    pub(crate) quit_requested: bool,
    pub(crate) render_errors: u64,
    pub(crate) inbox: Inbox,
    inbox_scratch: Vec<AquaUserEvent>,
    window_scratch: Vec<AquaWindowId>,
    pub(crate) flushers: HashMap<AquaWindowId, Box<dyn FnMut() -> FlushReport>>,
    fonts: aqua_text::SharedFonts,
    runs: aqua_text::SharedRuns,
    pub(crate) prefs: OsPreferences,
    clipboard: Box<dyn Clipboard>,
    pub(crate) window_requests: VecDeque<WindowRequest>,
    pub(crate) dialogs: VecDeque<(AquaWindowId, RequestId, DialogSpec)>,
    next_request: u64,
    clipboard_backend: ClipboardBackend,
    smoke_frames: Option<u64>,
    pub(crate) frames_presented: u64,
    frame_stats: Option<FrameStats>,
    input_latency: Option<crate::latency::InputLatency>,
    pub(crate) smoke_deadline: Option<WallInstant>,
    epoch: WallInstant,
    pub(crate) wakeups: u64,
    native_loop: bool,
    last_host_report: Option<WallInstant>,
    render_service: Option<RenderService>,
    compositions: Compositions,
    pressure: MemoryPressure,
}

impl Aqua {
    pub fn new(config: AquaConfig) -> Result<Self, String> {
        Self::new_with_preferences(config, query_preferences())
    }

    pub(crate) fn new_with_preferences(
        mut config: AquaConfig,
        prefs: OsPreferences,
    ) -> Result<Self, String> {
        crate::profile::init_from_environment();
        config
            .validated()
            .map_err(|e| format!("invalid window options: {e:?}"))?;
        let mut scheduler = Scheduler::new();
        config.apply_to_scheduler(&mut scheduler);
        if aqua_protocol::material::needs_alpha_surface(config.material)
            && !config.window.transparent
        {
            eprintln!("aqua: material requires a transparent window — enabling window.transparent");
            config.window.transparent = true;
        }
        if config.material != aqua_protocol::WindowMaterial::Solid && prefers_opaque_frame(&prefs) {
            eprintln!("aqua: high-contrast/transparency-off — material disabled");
            config.material = aqua_protocol::WindowMaterial::Solid;
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
            next_id: 1,
            quit_requested: false,
            render_errors: 0,
            inbox: Inbox::default(),
            inbox_scratch: Vec::new(),
            window_scratch: Vec::new(),
            flushers: HashMap::new(),
            fonts: aqua_text::SharedFonts::new(),
            runs: aqua_text::SharedRuns::new(),
            prefs,
            clipboard,
            clipboard_backend,
            window_requests: VecDeque::new(),
            dialogs: VecDeque::new(),
            next_request: 1,
            smoke_deadline,
            epoch: WallInstant::now(),
            wakeups: 0,
            smoke_frames,
            frames_presented: 0,
            frame_stats: FrameStats::from_environment(),
            input_latency: crate::latency::InputLatency::from_environment(),
            native_loop: false,
            last_host_report: None,
            render_service: None,
            compositions: Arc::default(),
            pressure: MemoryPressure::new(),
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

    pub fn clock(&self) -> Instant {
        Instant::from_nanos(u64::try_from(self.epoch.elapsed().as_nanos()).unwrap_or(u64::MAX))
    }

    pub fn shared_clock(&self) -> Clock {
        let epoch = self.epoch;
        Arc::new(move || {
            Instant::from_nanos(u64::try_from(epoch.elapsed().as_nanos()).unwrap_or(u64::MAX))
        })
    }

    pub(crate) fn align_clock(&mut self, shell_now: Instant) {
        self.epoch = WallInstant::now()
            .checked_sub(shell_now.since_origin())
            .unwrap_or_else(WallInstant::now);
    }

    pub(crate) fn mark_native_loop(&mut self) {
        self.native_loop = true;
    }

    pub fn wall(&self, at: Instant) -> WallInstant {
        self.epoch + at.since_origin()
    }

    pub fn inbox(&self) -> Inbox {
        self.inbox.clone()
    }

    pub fn config(&self) -> &AquaConfig {
        &self.config
    }

    pub fn has_native_window(&self, id: AquaWindowId) -> bool {
        self.windows
            .get(&id)
            .is_some_and(|record| record.native.is_some())
    }

    fn bind_app_reactive(&mut self, app: &impl Application, id: AquaWindowId) {
        if self.flushers.contains_key(&id) {
            return;
        }
        if let Some(rt) = app.reactive_runtime_for(id) {
            self.with_ctx(|ctx| ctx.bind_reactive(id, rt));
        }
    }

    pub fn flush_output(&mut self, app: &mut impl Application, id: AquaWindowId) {
        let mut rounds = 0;
        let mut output = app.take_output(id);
        loop {
            let feedback = self.apply_output(app, id, output);
            rounds += 1;
            match feedback.is_empty() || rounds > 8 {
                true => break,
                false => {
                    feedback
                        .into_iter()
                        .for_each(|input| app.step_input(id, input));
                    output = app.take_output(id);
                }
            }
        }
    }

    fn apply_output(
        &mut self,
        app: &mut impl Application,
        id: AquaWindowId,
        output: RuntimeOutput,
    ) -> Vec<RuntimeInput> {
        let mut feedback = Vec::new();
        for command in output.shell {
            match command {
                ShellCommand::CreateWindow { spec, .. } => {
                    self.window_requests
                        .push_back(WindowRequest::Create(options_from_spec(&spec)));
                    self.inbox.post(AquaUserEvent::WindowRequests);
                }
                ShellCommand::CloseWindow(_) => {
                    self.window_requests.push_back(WindowRequest::Close(id));
                    self.inbox.post(AquaUserEvent::WindowRequests);
                }
                ShellCommand::SetTitle { title, .. } => {
                    self.window_requests
                        .push_back(WindowRequest::SetTitle(id, title));
                    self.inbox.post(AquaUserEvent::WindowRequests);
                }
                ShellCommand::SetCursor { icon, .. } => {
                    if let Some(native) = self.windows.get(&id).and_then(|r| r.native.as_ref()) {
                        native.set_cursor(icon);
                    }
                }
                ShellCommand::SetDragRegions { regions, .. } => {
                    if let Some(native) = self.windows.get(&id).and_then(|r| r.native.as_ref()) {
                        native.set_drag_regions(&regions);
                    }
                }
                ShellCommand::SetMaterial { .. } | ShellCommand::SetScheme { .. } => {}
                ShellCommand::Operate { op, .. } => {
                    let request = match op {
                        WindowOp::Minimize => WindowRequest::SetMinimized(id, true),
                        WindowOp::Maximize => WindowRequest::SetMaximized(id, true),
                        WindowOp::Restore => WindowRequest::SetMaximized(id, false),
                        WindowOp::Show => WindowRequest::SetVisible(id, true),
                        WindowOp::Hide => WindowRequest::SetVisible(id, false),
                        WindowOp::Focus => WindowRequest::Activate(id),
                    };
                    self.window_requests.push_back(request);
                    self.inbox.post(AquaUserEvent::WindowRequests);
                }
                ShellCommand::RequestFrame { reason, .. } => {
                    self.scheduler.request_frame(id, reason);
                }
                ShellCommand::ClipboardWrite { text } => {
                    if let Err(error) = self.clipboard.set_text(text) {
                        eprintln!("aqua: clipboard write failed: {error}");
                    }
                }
                ShellCommand::ClipboardRead { request } => {
                    feedback.push(RuntimeInput::Shell(crate::paste::clipboard_event(
                        &mut *self.clipboard,
                        request,
                    )));
                }
                ShellCommand::Dialog { request, spec } => {
                    self.dialogs.push_back((id, request, spec));
                    self.inbox.post(AquaUserEvent::WindowRequests);
                }
                ShellCommand::Quit => self.quit_requested = true,
            }
        }
        if let Some(record) = self.windows.get_mut(&id) {
            if let Some(sink) = record.ime.as_mut() {
                output
                    .ime
                    .into_iter()
                    .for_each(|command| sink.command(command));
            }
            if let (Some(sink), Some(update)) = (record.access.as_mut(), output.access.as_ref()) {
                sink.apply(update);
            }
            if let Some(link) = record.link.as_mut() {
                link.push_commands(output.render);
            }
        }
        if let Some(at) = output.wake_at {
            self.scheduler.request_animation_deadline(id, at);
        }
        let _ = app;
        feedback
    }

    pub fn dispatch_start(&mut self, app: &mut impl Application, id: AquaWindowId) {
        if self.windows.contains_key(&id) {
            self.with_ctx(|ctx| app.on_start(ctx, id));
            self.bind_app_reactive(&*app, id);
            self.flush_output(app, id);
        }
    }

    pub fn dispatch_wake(&mut self, app: &mut impl Application, id: AquaWindowId) {
        if self.windows.contains_key(&id) {
            self.with_ctx(|ctx| app.on_wake(ctx, id));
            self.flush_output(app, id);
        }
    }

    pub fn dispatch_dialog(
        &mut self,
        app: &mut impl Application,
        id: AquaWindowId,
        request: RequestId,
        outcome: DialogOutcome,
    ) {
        self.with_ctx(|ctx| app.on_dialog(ctx, id, request, outcome));
        self.flush_output(app, id);
    }

    pub fn dispatch_frame_completed(
        &mut self,
        app: &mut impl Application,
        id: AquaWindowId,
        sequence: u64,
    ) {
        if self.windows.contains_key(&id) {
            self.with_ctx(|ctx| app.on_frame_completed(ctx, id, sequence));
        }
    }

    pub fn dispatch_window_requests(&mut self, app: &mut impl Application) -> Vec<WindowOptions> {
        let requests: Vec<_> = self.window_requests.drain(..).collect();
        let mut creates = Vec::new();
        for request in requests {
            if let WindowRequest::Create(options) = &request {
                if self.native_loop {
                    creates.push(options.clone());
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
                    maximized: false,
                });
                self.dispatch_start(app, id);
                continue;
            }
            if let Err(error) = self.apply_window_request(app, &request) {
                app.on_window_request_error(&request, &error);
            }
        }
        creates
    }

    pub(crate) fn apply_window_request(
        &mut self,
        app: &mut impl Application,
        request: &WindowRequest,
    ) -> Result<(), aqua_protocol::window::WindowError> {
        use aqua_protocol::window::WindowError;
        let id = match request {
            WindowRequest::Create(_) => {
                return Err(WindowError::Unsupported("requires the native loop"));
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
            | WindowRequest::BeginGpuiSnapshotTransition(id, _)
            | WindowRequest::BeginGpuiSourceSnapshotTransition(id, _)
            | WindowRequest::SetTextRasterMode(id, _) => *id,
        };
        let native = self
            .windows
            .get(&id)
            .ok_or(WindowError::UnknownWindow(id))?
            .native
            .clone();
        match request {
            WindowRequest::Close(_) => {
                if self.with_ctx(|ctx| app.can_close(ctx, id)) {
                    self.with_ctx(|ctx| app.on_window_event(ctx, id, &WindowEvent::Destroyed(id)));
                    self.remove_window(id);
                    if let Some(native) = native {
                        native.destroy();
                    }
                }
            }
            WindowRequest::Create(_) => {
                return Err(WindowError::Unsupported("requires the native loop"));
            }
            WindowRequest::SetTextRasterMode(_, mode) => {
                self.send_external(id, ExternalUpdate::TextRaster(*mode))?;
            }
            WindowRequest::BeginSnapshotTransition(_, request) => {
                let settings = Settings::plain(*request).map_err(invalid_transition)?;
                self.send_external(id, ExternalUpdate::Transition(settings))?;
            }
            WindowRequest::BeginGpuiSnapshotTransition(_, request) => {
                let settings = Settings::gpui(*request).map_err(invalid_transition)?;
                self.send_external(id, ExternalUpdate::Transition(settings))?;
            }
            WindowRequest::BeginGpuiSourceSnapshotTransition(_, request) => {
                let settings = Settings::gpui_source(*request).map_err(invalid_transition)?;
                self.send_external(id, ExternalUpdate::Transition(settings))?;
            }
            WindowRequest::SetBadge(_, badge) => {
                let native =
                    native.ok_or(WindowError::Unsupported("window has no native handle"))?;
                native.set_badge(*badge).map_err(|error| match error {
                    aqua_protocol::BadgeError::Unsupported => {
                        WindowError::Unsupported("taskbar badges")
                    }
                    aqua_protocol::BadgeError::Native(message) => WindowError::Native(message),
                })?;
            }
            WindowRequest::SetMaxFrameRate(_, target) => {
                self.scheduler.set_window_fps_target(id, *target);
                self.scheduler.request_frame(id, FrameReason::EXPLICIT);
            }
            WindowRequest::SetVsync(_, enabled) => {
                if let Some(record) = self.windows.get_mut(&id) {
                    record.vsync = *enabled;
                    if let Some(link) = record.link.as_ref() {
                        link.set_immediate(!*enabled);
                    }
                }
                self.scheduler.request_frame(id, FrameReason::EXPLICIT);
            }
            WindowRequest::RequestRedraw(_) => {
                self.scheduler.request_frame(id, FrameReason::EXPLICIT);
            }
            WindowRequest::SetTitle(_, title) => {
                if let Some(native) = native {
                    native.set_title(title);
                }
                if let Some(record) = self.windows.get_mut(&id) {
                    record.observed.title = title.clone();
                }
            }
            WindowRequest::SetInnerSize(_, size) => {
                if let Some(native) = native {
                    native.set_logical_size(size.width, size.height);
                }
            }
            WindowRequest::SetVisible(_, visible) => {
                if let Some(native) = native {
                    native.set_visible(*visible);
                }
                if let Some(record) = self.windows.get_mut(&id) {
                    record.observed.visible = *visible;
                }
            }
            WindowRequest::SetMinimized(_, minimized) => {
                if let Some(native) = native {
                    native.set_minimized(*minimized);
                }
            }
            WindowRequest::SetMaximized(_, maximized) => {
                if let Some(native) = native {
                    native.set_maximized(*maximized);
                }
            }
            WindowRequest::Activate(_) => {
                if let Some(native) = native {
                    native.activate();
                }
            }
            WindowRequest::SetOuterPosition(_, position) => {
                if let Some(native) = native {
                    native.set_outer_position(position.x, position.y);
                }
            }
            WindowRequest::Center(_) => {
                if let Some(native) = native {
                    native.center();
                }
            }
        }
        Ok(())
    }

    pub fn send_external(
        &mut self,
        id: AquaWindowId,
        update: ExternalUpdate,
    ) -> Result<(), WindowError> {
        let link = self
            .windows
            .get(&id)
            .and_then(|record| record.link.as_ref())
            .ok_or(WindowError::Unsupported("window has no render link"))?;
        link.external(update);
        self.scheduler.request_frame(id, FrameReason::EXPLICIT);
        Ok(())
    }

    pub(crate) fn alloc_id(&mut self) -> AquaWindowId {
        let id = AquaWindowId::new(self.next_id).expect("window id overflow");
        self.next_id += 1;
        id
    }

    pub(crate) fn with_ctx<R>(&mut self, f: impl FnOnce(&mut AppCtx) -> R) -> R {
        let now = self.clock();
        let mut ctx = AppCtx {
            requested_frames: std::collections::HashSet::new(),
            now,
            scheduler: &mut self.scheduler,
            quit_requested: &mut self.quit_requested,
            inbox: self.inbox.clone(),
            flushers: &mut self.flushers,
            windows: &self.windows,
            clipboard: &mut self.clipboard,
            window_requests: &mut self.window_requests,
            dialogs: &mut self.dialogs,
            next_request: &mut self.next_request,
        };
        f(&mut ctx)
    }

    pub fn add_headless_window(&mut self, observed: ObservedWindowState) -> AquaWindowId {
        let id = observed.id;
        self.next_id = self.next_id.max(id.raw().saturating_add(1));
        self.windows
            .insert(id, WindowRecord::new(observed, self.config.vsync));
        id
    }

    pub(crate) fn add_native_window(
        &mut self,
        observed: ObservedWindowState,
        native: Rc<dyn NativeWindow>,
    ) -> AquaWindowId {
        let id = self.add_headless_window(observed);
        if let Some(record) = self.windows.get_mut(&id) {
            record.native = Some(native);
        }
        id
    }

    pub fn attach_render(
        &mut self,
        id: AquaWindowId,
        factory: InitFactory,
        interval: Duration,
    ) -> bool {
        let inbox = self.inbox.clone();
        let clock = self.shared_clock();
        let wake: aqua_transport::Notify = {
            let inbox = inbox.clone();
            Arc::new(move || {
                inbox.post(AquaUserEvent::RenderEvents(id));
            })
        };
        let pending = self.compositions.clone();
        let composition: CompositionSink = Arc::new(move |content| {
            pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((id, content));
            inbox.post(AquaUserEvent::RenderEvents(id));
        });
        let immediate = self.windows.get(&id).is_some_and(|record| !record.vsync);
        if !self.windows.contains_key(&id) {
            return false;
        }
        let service = match self.render_service.as_ref() {
            Some(service) => service,
            None => match RenderService::new() {
                Ok(service) => self.render_service.insert(service),
                Err(error) => {
                    eprintln!("aqua: render thread failed to start: {error}");
                    return false;
                }
            },
        };
        let link = RenderLink::spawn(service, factory, composition, clock, interval, wake);
        link.set_immediate(immediate);
        match self.windows.get_mut(&id) {
            Some(record) => {
                record.link = Some(link);
                true
            }
            None => false,
        }
    }

    pub fn attach_access(&mut self, id: AquaWindowId, sink: Box<dyn AccessSink>) {
        if let Some(record) = self.windows.get_mut(&id) {
            record.access = Some(sink);
        }
    }

    pub fn attach_ime(&mut self, id: AquaWindowId, sink: Box<dyn ImeSink>) {
        if let Some(record) = self.windows.get_mut(&id) {
            record.ime = Some(sink);
        }
    }

    pub fn render_stats(&self, id: AquaWindowId) -> Option<Arc<crate::render::host::RenderStats>> {
        self.windows
            .get(&id)?
            .link
            .as_ref()
            .map(|link| link.stats().clone())
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
        if let Some(reason) = frame_reason(event) {
            self.scheduler.request_frame(id, reason);
        }
        let ev = event.clone();
        self.bind_app_reactive(&*app, id);
        self.with_ctx(|ctx| app.on_window_event(ctx, id, &ev));
        self.flush_output(app, id);
        if let WindowEvent::Focused(_, focused) = event {
            self.dispatch_app_input(app, id, &CoreInputEvent::Focused(*focused));
        }
        let close = match event {
            WindowEvent::Destroyed(_) => true,
            WindowEvent::CloseRequested(_) => self.with_ctx(|ctx| app.can_close(ctx, id)),
            _ => false,
        };
        if close {
            if matches!(event, WindowEvent::CloseRequested(_)) {
                self.with_ctx(|ctx| app.on_window_event(ctx, id, &WindowEvent::Destroyed(id)));
            }
            self.remove_window(id);
        }
    }

    pub fn dispatch_input(
        &mut self,
        app: &mut impl Application,
        id: AquaWindowId,
        event: &CoreInputEvent,
    ) -> bool {
        if !self.windows.contains_key(&id) {
            return false;
        }
        self.scheduler.request_frame(id, FrameReason::INPUT);
        self.dispatch_app_input(app, id, event)
    }

    pub(crate) fn dispatch_app_input(
        &mut self,
        app: &mut impl Application,
        id: AquaWindowId,
        event: &CoreInputEvent,
    ) -> bool {
        let _span = aqua_protocol::probe::span(aqua_protocol::probe::Phase::Input);
        let count_activity = !matches!(event, CoreInputEvent::Focused(_));
        crate::profile::note(|| {
            let text = format!("{event:?}");
            let kind = text.split(['(', ' ', '{']).next().unwrap_or("");
            format!("input={kind}")
        });
        let started = WallInstant::now();
        if let Some(probe) = self.input_latency.as_mut() {
            probe.begin_input();
        }
        let result = self.dispatch_attributed_input(app, id, count_activity, |app, ctx| {
            app.on_input(ctx, id, event)
        });
        self.flush_output(app, id);
        if let Some(probe) = self.input_latency.as_mut() {
            probe.end_input(event);
        }
        if self.frame_stats.is_some() {
            let elapsed = started.elapsed();
            if elapsed >= Duration::from_millis(2) {
                eprintln!(
                    "aqua-input-slow: {:.2}ms {:?}",
                    elapsed.as_secs_f64() * 1000.0,
                    std::mem::discriminant(event)
                );
            }
        }
        result
    }

    pub(crate) fn dispatch_attributed_input<A: Application>(
        &mut self,
        app: &mut A,
        id: AquaWindowId,
        count_activity: bool,
        dispatch: impl FnOnce(&mut A, &mut AppCtx<'_>),
    ) -> bool {
        let before = app.input_invalidation_epoch(id);
        let requested = self.with_ctx(|ctx| {
            dispatch(app, ctx);
            ctx.requested_frames.contains(&id)
        });
        let invalidated = requested || app.input_invalidation_epoch(id) != before;
        if invalidated && count_activity {
            let now = self.clock();
            if let Some(window) = self.windows.get_mut(&id) {
                window.last_invalidating_input = Some(now);
            }
        }
        invalidated
    }

    pub fn dispatch_file_drop(
        &mut self,
        app: &mut impl Application,
        id: AquaWindowId,
        paths: &[std::path::PathBuf],
    ) -> bool {
        self.dispatch_file_drop_at(app, id, paths, None)
    }

    pub fn dispatch_file_drop_at(
        &mut self,
        app: &mut impl Application,
        id: AquaWindowId,
        paths: &[std::path::PathBuf],
        at: Option<aqua_core::Point>,
    ) -> bool {
        if !self.windows.contains_key(&id) {
            return false;
        }
        self.scheduler.request_frame(id, FrameReason::INPUT);
        let owned = paths.to_vec();
        let invalidated = self.dispatch_attributed_input(app, id, true, |app, ctx| {
            app.on_files_dropped_at(ctx, id, &owned, at)
        });
        self.flush_output(app, id);
        invalidated
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
                    }
                }
            }
            WindowEvent::Occluded(_, o) => {
                if let Some(rec) = self.windows.get_mut(&id) {
                    rec.observed.occluded = *o;
                }
                if let Some(link) = self.windows.get(&id).and_then(|r| r.link.as_ref()) {
                    link.set_hidden(*o);
                }
                if *o {
                    self.request_trim(id);
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

    pub(crate) fn remove_window(&mut self, id: AquaWindowId) {
        self.windows.remove(&id);
        self.scheduler.remove_window(id);
        self.flushers.remove(&id);
    }

    pub fn set_runs(&mut self, runs: aqua_text::SharedRuns) {
        self.runs = runs;
    }

    pub fn runs(&self) -> &aqua_text::SharedRuns {
        &self.runs
    }

    pub fn set_fonts(&mut self, fonts: aqua_text::SharedFonts) {
        self.fonts = fonts;
    }

    pub fn fonts(&self) -> &aqua_text::SharedFonts {
        &self.fonts
    }

    pub fn presented_frames(&self, id: AquaWindowId) -> Option<u64> {
        self.windows.get(&id).map(|record| record.presented)
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

    pub fn next_wake(&self, now: Instant) -> Option<Instant> {
        self.scheduler.next_wakeup(now)
    }

    pub fn pump_inbox(&mut self, app: &mut impl Application) {
        self.check_memory_pressure();
        let mut events = std::mem::take(&mut self.inbox_scratch);
        self.inbox.drain_into(&mut events);
        for event in events.drain(..) {
            match event {
                AquaUserEvent::Messages(id) => self.dispatch_wake(app, id),
                AquaUserEvent::ReactiveWake(id) => {
                    let _span = aqua_protocol::probe::span(aqua_protocol::probe::Phase::Reactive);
                    let ran = self
                        .flushers
                        .get_mut(&id)
                        .map(|flusher| flusher().ran)
                        .unwrap_or(0);
                    if ran > 0 {
                        self.scheduler.request_frame(id, FrameReason::INPUT);
                    }
                }
                AquaUserEvent::A11yActions(id) => {
                    self.scheduler.request_frame(id, FrameReason::INPUT);
                    self.drain_access_actions(app, id);
                }
                AquaUserEvent::RenderEvents(id) => self.drain_render_events(app, id),
                AquaUserEvent::Tsf(id) => self.drain_ime(app, id),
                AquaUserEvent::WindowRequests => {}
            }
        }
        self.inbox_scratch = events;
        let mut ids = std::mem::take(&mut self.window_scratch);
        ids.clear();
        ids.extend(self.windows.keys().copied());
        for id in ids.iter().copied() {
            self.drain_ime(app, id);
        }
        self.window_scratch = ids;
    }

    pub fn drain_access_actions(&mut self, app: &mut impl Application, id: AquaWindowId) {
        let actions = self
            .windows
            .get_mut(&id)
            .and_then(|record| record.access.as_mut())
            .map(|sink| sink.drain_actions())
            .unwrap_or_default();
        for request in &actions {
            self.with_ctx(|ctx| app.on_a11y_action(ctx, id, request));
        }
        self.flush_output(app, id);
    }

    pub fn drain_ime(&mut self, app: &mut impl Application, id: AquaWindowId) {
        let _span = aqua_protocol::probe::span(aqua_protocol::probe::Phase::Input);
        let mut events = Vec::new();
        if let Some(sink) = self
            .windows
            .get_mut(&id)
            .and_then(|record| record.ime.as_mut())
        {
            sink.drain(&mut events);
        }
        if events.is_empty() {
            return;
        }
        if let Some(probe) = self.input_latency.as_mut() {
            probe.begin_input();
        }
        for event in events {
            app.step_input(id, RuntimeInput::Ime(event));
        }
        self.flush_output(app, id);
        if let Some(probe) = self.input_latency.as_mut() {
            probe.end_text();
        }
    }

    fn attach_compositions(&mut self, id: AquaWindowId) {
        let ready: Vec<Box<dyn Any + Send>> = {
            let mut pending = self
                .compositions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (mine, rest): (Vec<_>, Vec<_>) =
                pending.drain(..).partition(|(window, _)| *window == id);
            *pending = rest;
            mine.into_iter().map(|(_, content)| content).collect()
        };
        let native = self.windows.get(&id).and_then(|r| r.native.clone());
        if let (Some(native), Some(latest)) = (native, ready.last()) {
            match native.attach_composition(latest.as_ref()) {
                true => {
                    if let Some(link) = self.windows.get(&id).and_then(|r| r.link.as_ref()) {
                        link.stats()
                            .attached
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                false => eprintln!("aqua: composition surface could not be attached"),
            }
        }
    }

    pub fn request_trim(&self, id: AquaWindowId) {
        if let Some(link) = self.windows.get(&id).and_then(|r| r.link.as_ref()) {
            link.request_trim();
        }
    }

    pub fn request_trim_all(&self) {
        aqua_ui::layout::trim_measure_cache();
        self.windows
            .values()
            .filter_map(|record| record.link.as_ref())
            .for_each(RenderLink::request_trim);
    }

    fn check_memory_pressure(&mut self) {
        if self.pressure.rising_edge(WallInstant::now()) {
            self.request_trim_all();
        }
    }

    pub fn drain_render_events(&mut self, app: &mut impl Application, id: AquaWindowId) {
        let _span = aqua_protocol::probe::span(aqua_protocol::probe::Phase::Input);
        self.attach_compositions(id);
        let events = self
            .windows
            .get_mut(&id)
            .and_then(|record| record.link.as_mut())
            .map(RenderLink::poll)
            .unwrap_or_default();
        for event in events {
            match event {
                RenderEvent::Presented(frame) => {
                    if let Some(record) = self.windows.get_mut(&id) {
                        record.presented += 1;
                    }
                    self.frames_presented += 1;
                    if let Some(probe) = self.input_latency.as_mut() {
                        if let Some(stats) = self
                            .windows
                            .get(&id)
                            .and_then(|r| r.link.as_ref())
                            .map(|link| link.stats().clone())
                        {
                            probe.presented(&stats);
                        }
                    }
                    if let Some(budget) = self.smoke_frames {
                        if self.frames_presented >= budget {
                            eprintln!(
                                "aqua: smoke budget reached ({} frames) — exiting",
                                self.frames_presented
                            );
                            self.quit_requested = true;
                        }
                    }
                    app.step_input(id, RuntimeInput::Gpu(GpuEvent::Presented(frame)));
                    self.dispatch_frame_completed(app, id, frame);
                }
                RenderEvent::DeviceLost => {
                    self.render_errors += 1;
                    app.step_input(id, RuntimeInput::Gpu(GpuEvent::DeviceLost));
                    self.with_ctx(|ctx| app.on_render_error(ctx, id, "device lost"));
                }
                RenderEvent::DeviceRecovered => {
                    app.step_input(id, RuntimeInput::Gpu(GpuEvent::DeviceRecovered));
                    self.scheduler.request_frame(id, FrameReason::EXPLICIT);
                }
                RenderEvent::Motion(motion) => {
                    app.step_input(id, RuntimeInput::Render(RenderEvent::Motion(motion)));
                    self.with_ctx(|ctx| app.on_motion_event(ctx, id, &motion));
                    self.scheduler.request_frame(id, FrameReason::ANIMATION);
                }
            }
        }
        self.flush_output(app, id);
    }

    pub fn draw_ready(&mut self, app: &mut impl Application, now: Instant) {
        self.draw_windows(app, now, FrameReason::all());
    }

    pub fn draw_input_frames(&mut self, app: &mut impl Application, now: Instant) {
        self.draw_windows(app, now, FrameReason::INPUT);
    }

    pub fn drain_text(&mut self, app: &mut impl Application) {
        let mut ids = std::mem::take(&mut self.window_scratch);
        ids.clear();
        ids.extend(self.windows.keys().copied());
        for id in ids.iter().copied() {
            self.drain_ime(app, id);
        }
        self.window_scratch = ids;
    }

    fn draw_windows(&mut self, app: &mut impl Application, now: Instant, only: FrameReason) {
        let ready: Vec<_> = self
            .scheduler
            .ready_windows(now)
            .into_iter()
            .filter(|(_, reasons)| reasons.intersects(only))
            .collect();
        if ready.is_empty() {
            return;
        }
        let ids: Vec<AquaWindowId> = ready.iter().map(|(id, _)| *id).collect();
        self.scheduler.mark_produced(&ids, now);
        for (id, reasons) in ready {
            if self.frame_stats.is_some() {
                eprintln!("aqua-reason: {reasons:?}");
            }
            crate::profile::begin_frame();
            crate::profile::note(|| format!("rebuild_reason={reasons:?}"));
            let started = WallInstant::now();
            self.produce_frame(app, id, now);
            let cost = started.elapsed();
            crate::profile::end_frame(crate::profile::Role::App, id.raw());
            self.scheduler.mark_presented(id);
            if reasons.contains(FrameReason::RESIZE) {
                let cost = cost.min(RESIZE_COST_CAP);
                self.scheduler
                    .defer_resize_frames(id, now + cost + cost.mul_f64(RESIZE_IDLE_RATIO));
            }
        }
    }

    fn produce_frame(&mut self, app: &mut impl Application, id: AquaWindowId, now: Instant) {
        self.check_memory_pressure();
        let frame_started = WallInstant::now();
        if let Some(probe) = self.input_latency.as_mut() {
            probe.frame_started();
        }
        let Some(record) = self.windows.get(&id) else {
            return;
        };
        let hidden = record.observed.occluded;
        let wants_tree = record
            .access
            .as_ref()
            .is_some_and(|sink| sink.needs_snapshot());
        if record.link.is_none() || (hidden && !wants_tree) {
            return;
        }
        if let Some(flusher) = self.flushers.get_mut(&id) {
            let _span = aqua_protocol::probe::span(aqua_protocol::probe::Phase::Reactive);
            let _report = flusher();
        }
        let Some(observed) = self.windows.get(&id).map(|r| r.observed.clone()) else {
            return;
        };
        let updates = run_headless_externals(app, &observed, now);
        if let Some(link) = self.windows.get(&id).and_then(|r| r.link.as_ref()) {
            updates.into_iter().for_each(|update| link.external(update));
        }
        aqua_core::checkpoint::reach();
        self.sync_access_client(app, id);
        let build_started = WallInstant::now();
        let build_span = aqua_protocol::probe::span(aqua_protocol::probe::Phase::Build);
        let deadline = app.build_scene(&observed, now);
        drop(build_span);
        if let Some(deadline) = deadline.filter(|_| !hidden) {
            self.scheduler.request_animation_deadline(id, deadline);
        }
        self.flush_output(app, id);
        aqua_core::checkpoint::reach();
        let build_time = build_started.elapsed();
        if let Some(probe) = self.input_latency.as_mut() {
            probe.build_finished();
        }
        self.sync_chrome(app, id, &observed);
        aqua_core::checkpoint::reach();
        self.sync_access(app, id);
        aqua_core::checkpoint::reach();
        if hidden {
            return;
        }
        let cursor = app.cursor_for(id);
        let motion = app.motion_commands(id);
        let submit_started = WallInstant::now();
        let scale = observed.scale_factor as f32;
        let Some(record) = self.windows.get_mut(&id) else {
            return;
        };
        if let (Some(native), Some(cursor)) = (record.native.as_ref(), cursor) {
            native.set_cursor(cursor);
        }
        let Some(link) = record.link.as_mut() else {
            return;
        };
        let submit_span = aqua_protocol::probe::span(aqua_protocol::probe::Phase::Submit);
        link.set_viewport(observed.inner_size.as_core_size(), scale);
        if !motion.is_empty() {
            link.push_commands(motion.into_iter().map(RenderCommand::Motion));
        }
        aqua_core::checkpoint::reach();
        if let Some(scene) = app.scene_mut(id) {
            link.push_scene(scene);
        }
        aqua_core::checkpoint::reach();
        if let Some(probe) = self.input_latency.as_mut() {
            probe.scene_pushed(link.scenes_queued());
        }
        let submit_time = submit_started.elapsed();
        drop(submit_span);
        let cpu_time = frame_started.elapsed();
        if let Some(stats) = self.frame_stats.as_mut() {
            stats.record(cpu_time, submit_time, build_time);
        }
    }

    fn sync_access_client(&mut self, app: &mut impl Application, id: AquaWindowId) {
        let Some(record) = self.windows.get_mut(&id) else {
            return;
        };
        let connected = record.access.as_ref().is_some_and(|sink| sink.connected());
        let epoch = record
            .access
            .as_ref()
            .map_or(0, |sink| sink.connection_epoch());
        if connected != record.access_connected || (connected && epoch != record.access_epoch) {
            record.access_connected = connected;
            record.access_epoch = epoch;
            app.step_input(id, RuntimeInput::AccessClient { connected });
            self.flush_output(app, id);
        }
    }

    fn sync_chrome(
        &mut self,
        app: &impl Application,
        id: AquaWindowId,
        observed: &ObservedWindowState,
    ) {
        let next = app.chrome_regions(observed);
        let scale = match observed.scale_factor {
            scale if scale.is_finite() && scale > 0.0 => scale,
            _ => 1.0,
        };
        let Some(record) = self.windows.get_mut(&id) else {
            return;
        };
        if next.is_none() && record.last_regions.is_none() {
            return;
        }
        let Some(plan) = chrome_plan(&record.last_regions, next, scale) else {
            return;
        };
        let cleared;
        let payload = match &plan {
            ChromePlan::Apply(regions) => regions,
            ChromePlan::Clear => {
                cleared = DragRegions::default();
                &cleared
            }
        };
        if let Some(native) = record.native.as_ref() {
            native.set_drag_regions(payload);
        }
        record.last_regions = match plan {
            ChromePlan::Apply(regions) => Some((regions, scale)),
            ChromePlan::Clear => None,
        };
    }

    fn sync_access(&mut self, app: &impl Application, id: AquaWindowId) {
        let needed = self
            .windows
            .get(&id)
            .and_then(|record| record.access.as_ref())
            .is_some_and(|sink| sink.needs_snapshot());
        if !needed {
            return;
        }
        let Some((root, focus, nodes)) = app.a11y_nodes_for(id) else {
            return;
        };
        if let Some(sink) = self
            .windows
            .get_mut(&id)
            .and_then(|record| record.access.as_mut())
        {
            sink.apply(&AccessTreeUpdate { root, focus, nodes });
        }
    }

    pub fn report_host_stats(&mut self) {
        if self.frame_stats.is_none() {
            return;
        }
        let due = self
            .last_host_report
            .is_none_or(|last| last.elapsed() >= Duration::from_secs(1));
        if !due {
            return;
        }
        self.last_host_report = Some(WallInstant::now());
        for (id, record) in &self.windows {
            let Some(link) = record.link.as_ref() else {
                continue;
            };
            use std::sync::atomic::Ordering::Relaxed;
            let stats = link.stats();
            eprintln!(
                "aqua-host-stats: window={} wakeups={} presented={} waits={} lower_ms={:.2} lower_max_ms={:.2} execute_ms={:.2} present_ms={:.2} commands={} glyphs={} skipped={} failures={} recoveries={} backlog={} partial={} full={} damaged_px={} forced={} trims={} sharers={} atlas_mask_kb={} atlas_color_kb={} atlas_pages={}",
                id.raw(),
                self.wakeups,
                stats.frames.load(Relaxed),
                stats.waits.load(Relaxed),
                stats.lower_ns.load(Relaxed) as f64 / 1e6,
                stats.lower_max_ns.load(Relaxed) as f64 / 1e6,
                stats.execute_ns.load(Relaxed) as f64 / 1e6,
                stats.present_ns.load(Relaxed) as f64 / 1e6,
                stats.commands.load(Relaxed),
                stats.glyphs.load(Relaxed),
                stats.skipped_items.load(Relaxed),
                stats.failures.load(Relaxed),
                stats.recoveries.load(Relaxed),
                link.backlog(),
                stats.partial_frames.load(Relaxed),
                stats.full_frames.load(Relaxed),
                stats.damaged_pixels.load(Relaxed),
                stats.forced.load(Relaxed),
                stats.trims.load(Relaxed),
                stats.device_sharers.load(Relaxed),
                stats.atlas_mask_bytes.load(Relaxed) / 1024,
                stats.atlas_color_bytes.load(Relaxed) / 1024,
                stats.atlas_pages.load(Relaxed),
            );
            let shapes = self.fonts().lock().shape_cache_stats();
            eprintln!(
                "aqua-text-stats: window={} atlas_created={} atlas_upload_kb={} glyph_rasterized={} slug_created={} slug_upload_kb={} slug_kb={} shape_hits={} shape_misses={} shape_entries={} shape_kb={}",
                id.raw(),
                stats.atlas_created.load(Relaxed),
                stats.atlas_upload_bytes.load(Relaxed) / 1024,
                stats.glyph_rasterized.load(Relaxed),
                stats.slug_created.load(Relaxed),
                stats.slug_upload_bytes.load(Relaxed) / 1024,
                stats.slug_bytes.load(Relaxed) / 1024,
                shapes.hits,
                shapes.misses,
                shapes.entries,
                shapes.bytes / 1024,
            );
            if crate::mem_probe::memory_report_enabled() {
                eprintln!("aqua-heap: {:?}", crate::mem_probe::process_heap());
                eprintln!("{}", crate::mem_probe::address_space_summary());
                eprintln!("{}", crate::mem_probe::heap_walk_summary());
                #[cfg(feature = "probe-allocator")]
                {
                    eprintln!("aqua-heap-summary: {}", crate::mem_probe::summary());
                    crate::mem_probe::dump_chains();
                }
            }
        }
    }

    pub fn quit_requested(&self) -> bool {
        self.quit_requested
    }

    pub fn run(self, app: impl Application + 'static) -> Result<(), String> {
        if self.config.headless {
            return Err("headless config cannot run the GUI loop; use the headless pump".into());
        }
        crate::native::run(self, app)
    }
}

pub fn physical_extent(window: &ObservedWindowState) -> Extent {
    let scale = window.scale_factor as f32;
    Extent {
        width: ((window.inner_size.width as f32) * scale).round().max(1.0) as u32,
        height: ((window.inner_size.height as f32) * scale).round().max(1.0) as u32,
    }
}

#[cfg(test)]
#[path = "../../aqua-verify/tests/support/desktop.rs"]
mod desktop;

#[cfg(test)]
mod tests {
    use super::*;
    use aqua_core::PointerKind;
    use aqua_protocol::ShellEvent;
    use aqua_protocol::geometry::LogicalSize;

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
            self.scene.insert(aqua_core::DisplayItem::Rect {
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
            maximized: false,
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
    fn a_signal_set_outside_an_input_event_requests_a_frame() {
        use std::cell::Cell;
        use std::rc::Rc;
        struct Reactive {
            probe: Probe,
            runtime: aqua_reactive::Runtime,
        }
        impl Application for Reactive {
            fn on_window_event(&mut self, c: &mut AppCtx, id: AquaWindowId, e: &WindowEvent) {
                self.probe.on_window_event(c, id, e)
            }
            fn on_input(&mut self, c: &mut AppCtx, id: AquaWindowId, e: &CoreInputEvent) {
                self.probe.on_input(c, id, e)
            }
            fn build_scene(&mut self, w: &ObservedWindowState, n: Instant) -> Option<Instant> {
                self.probe.build_scene(w, n)
            }
            fn reactive_runtime_for(&self, _id: AquaWindowId) -> Option<&ReactiveRuntime> {
                Some(&self.runtime)
            }
        }
        let (mut aqua, probe, id) = headless();
        let runtime = aqua_reactive::Runtime::new();
        let mut app = Reactive { probe, runtime };
        aqua.bind_app_reactive(&app, id);
        let scope = app.runtime.create_scope();
        let (value, set_value) = scope.signal(0i32);
        let seen = Rc::new(Cell::new(-1i32));
        let seen_fx = seen.clone();
        let _fx = scope.subscribe(&value, move |v| seen_fx.set(v));
        let now = Instant::from_secs_f64(1.0);
        let ids: Vec<_> = aqua
            .scheduler
            .ready_windows(now)
            .iter()
            .map(|(id, _)| *id)
            .collect();
        aqua.scheduler.mark_produced(&ids, now);
        assert!(!aqua.scheduler.has_pending());
        set_value.set(7);
        aqua.pump_inbox(&mut app);
        assert_eq!(seen.get(), 7);
        assert!(aqua.scheduler.has_pending());
    }

    #[test]
    fn clipboard_round_trips_through_ctx_and_reports_backend() {
        if !super::desktop::require("clipboard_round_trips_through_ctx") {
            return;
        }
        let (mut aqua, _, _) = headless();
        let _ = aqua.clipboard_backend();
        fn contended<T, E: std::fmt::Debug>(mut operation: impl FnMut() -> Result<T, E>) -> T {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                match operation() {
                    Ok(value) => return value,
                    Err(error) if std::time::Instant::now() >= deadline => {
                        panic!("clipboard stayed busy: {error:?}")
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
                }
            }
        }
        aqua.with_ctx(|ctx| {
            contended(|| ctx.clipboard().set_text("copy me".to_string()));
            assert_eq!(
                contended(|| ctx.clipboard().get_text()).as_deref(),
                Some("copy me")
            );
            contended(|| ctx.clipboard().clear());
            assert_eq!(contended(|| ctx.clipboard().get_text()), None);
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
        assert!(!aqua_protocol::prefs::should_reduce_motion(
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
    fn headless_windows_carry_no_native_window() {
        let (mut aqua, mut app, id) = headless();
        assert!(!aqua.has_native_window(id));
        aqua.dispatch_window_event(&mut app, id, &WindowEvent::CloseRequested(id));
        assert!(!aqua.has_native_window(id));
    }

    #[test]
    fn chrome_plan_pushes_only_on_change_and_clears_on_quiet() {
        let rects = || DragRegions {
            draggable: vec![(0.0, 0.0, 200.0, 40.0)],
            no_drag: vec![(160.0, 0.0, 200.0, 40.0)],
            ..Default::default()
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
            ..Default::default()
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

    use std::cell::RefCell;
    use std::sync::Mutex;

    use aqua_core::geometry::Rect as CoreRect;
    use aqua_gpu_ref::RefBackend;
    use aqua_protocol::{GpuBackend, GpuReadback, SurfacePixels, SurfaceTarget};

    use crate::render::host::{FrameTarget, GpuFrames};
    use crate::render::link::RenderContext;
    use crate::render::lower::Lowering;

    struct Capture {
        inner: GpuFrames<RefBackend>,
        pixels: Arc<Mutex<Option<SurfacePixels>>>,
    }

    impl FrameTarget for Capture {
        fn paced(&self) -> bool {
            true
        }

        fn wait_frame(&mut self) {
            self.inner.wait_frame();
        }

        fn present(
            &mut self,
            side: &mut aqua_runtime::RenderSide,
            frame: aqua_protocol::FrameId,
        ) -> Vec<aqua_protocol::Feedback> {
            let feedback = self.inner.present(side, frame);
            let surface = self.inner.surface();
            if let Ok(read) = self.inner.backend_mut().read_surface(surface) {
                *self.pixels.lock().unwrap() = Some(read);
            }
            feedback
        }
    }

    fn reference_factory(
        extent: Extent,
        fonts: aqua_text::SharedFonts,
        runs: aqua_text::SharedRuns,
        pixels: Arc<Mutex<Option<SurfacePixels>>>,
    ) -> InitFactory {
        Box::new(move |context| {
            let RenderContext {
                externals, stats, ..
            } = context;
            Box::new(move || {
                let mut backend = RefBackend::new();
                crate::render::transition::register(backend.kernels_mut());
                let surface = backend
                    .create_surface(SurfaceTarget::Offscreen(extent))
                    .map_err(|e| e.to_string())?;
                let frames = GpuFrames::new(
                    backend,
                    surface,
                    extent,
                    Lowering::new(fonts, runs),
                    externals,
                    stats,
                );
                Ok(Box::new(Capture {
                    inner: frames,
                    pixels,
                }) as Box<dyn FrameTarget>)
            })
        })
    }

    fn wait_for_presented(aqua: &mut Aqua, app: &mut Probe, id: AquaWindowId, frames: u64) {
        let deadline = WallInstant::now() + Duration::from_secs(20);
        while aqua.presented_frames(id).unwrap_or(0) < frames {
            assert!(
                WallInstant::now() < deadline,
                "render thread never presented"
            );
            aqua.drain_render_events(app, id);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn scene_reaches_the_render_thread_and_the_reference_surface() {
        let (mut aqua, mut app, id) = headless();
        let pixels = Arc::new(Mutex::new(None));
        let extent = Extent {
            width: 800,
            height: 600,
        };
        let factory = reference_factory(
            extent,
            aqua.fonts().clone(),
            aqua.runs().clone(),
            pixels.clone(),
        );
        assert!(aqua.attach_render(id, factory, Duration::from_millis(2)));
        aqua.scheduler.request_frame(id, FrameReason::EXPLICIT);
        let now = aqua.clock();
        aqua.draw_ready(&mut app, now);
        wait_for_presented(&mut aqua, &mut app, id, 1);
        let frame = pixels
            .lock()
            .unwrap()
            .clone()
            .expect("a surface was read back");
        assert_eq!(frame.extent, extent);
        let at = |x: usize, y: usize| {
            let offset = (y * extent.width as usize + x) * 4;
            frame.rgba8[offset..offset + 4].to_vec()
        };
        let inside = at(50, 50);
        let outside = at(300, 300);
        assert!(inside[2] > 150 && inside[0] < 80, "blue rect at {inside:?}");
        assert_eq!(outside[3], 0, "empty area is cleared transparent");
        let _ = CoreRect::from_edges(0.0, 0.0, 1.0, 1.0);
    }

    #[derive(Default)]
    struct Calls(RefCell<Vec<String>>);

    struct FakeWindow(Rc<Calls>);

    impl NativeWindow for FakeWindow {
        fn scale(&self) -> f64 {
            1.0
        }
        fn inner_size(&self) -> (u32, u32) {
            (800, 600)
        }
        fn set_title(&self, title: &str) {
            self.0.0.borrow_mut().push(format!("title:{title}"));
        }
        fn set_logical_size(&self, width: f64, height: f64) {
            self.0.0.borrow_mut().push(format!("size:{width}x{height}"));
        }
        fn set_visible(&self, visible: bool) {
            self.0.0.borrow_mut().push(format!("visible:{visible}"));
        }
        fn set_minimized(&self, minimized: bool) {
            self.0.0.borrow_mut().push(format!("minimized:{minimized}"));
        }
        fn set_maximized(&self, maximized: bool) {
            self.0.0.borrow_mut().push(format!("maximized:{maximized}"));
        }
        fn is_maximized(&self) -> bool {
            false
        }
        fn activate(&self) {
            self.0.0.borrow_mut().push("activate".into());
        }
        fn set_outer_position(&self, x: i32, y: i32) {
            self.0.0.borrow_mut().push(format!("position:{x},{y}"));
        }
        fn center(&self) {
            self.0.0.borrow_mut().push("center".into());
        }
        fn set_cursor(&self, cursor: aqua_core::CursorIcon) {
            self.0.0.borrow_mut().push(format!("cursor:{cursor:?}"));
        }
        fn set_drag_regions(&self, _regions: &DragRegions) {
            self.0.0.borrow_mut().push("regions".into());
        }
        fn click_settings(&self) -> aqua_core::ClickSettings {
            aqua_core::ClickSettings::default()
        }
        fn display(&self) -> Option<aqua_protocol::monitor::DisplayInfo> {
            None
        }
        fn set_badge(
            &self,
            badge: Option<aqua_protocol::TaskbarBadge>,
        ) -> Result<(), aqua_protocol::BadgeError> {
            self.0
                .0
                .borrow_mut()
                .push(format!("badge:{}", badge.map_or(0, |b| b.count)));
            Ok(())
        }
        fn destroy(&self) {
            self.0.0.borrow_mut().push("destroy".into());
        }
    }

    struct Scripted {
        outputs: VecDeque<RuntimeOutput>,
        fed: Vec<RuntimeInput>,
    }

    impl Application for Scripted {
        fn on_window_event(&mut self, _c: &mut AppCtx, _: AquaWindowId, _: &WindowEvent) {}
        fn on_input(&mut self, _c: &mut AppCtx, _: AquaWindowId, _: &CoreInputEvent) {}
        fn build_scene(&mut self, _: &ObservedWindowState, _: Instant) -> Option<Instant> {
            None
        }
        fn step_input(&mut self, _id: AquaWindowId, input: RuntimeInput) {
            self.fed.push(input);
        }
        fn take_output(&mut self, _id: AquaWindowId) -> RuntimeOutput {
            self.outputs.pop_front().unwrap_or_default()
        }
    }

    #[test]
    fn machine_output_is_executed_against_the_native_window() {
        use aqua_protocol::ShellCommand;
        let mut aqua = Aqua::new(AquaConfig {
            headless: true,
            ..Default::default()
        })
        .unwrap();
        let id = aqua.alloc_id();
        let calls = Rc::new(Calls::default());
        aqua.add_native_window(
            ObservedWindowState {
                id,
                title: "scripted".into(),
                inner_size: LogicalSize::new(800.0, 600.0),
                scale_factor: 1.0,
                focused: true,
                visible: true,
                occluded: false,
                maximized: false,
            },
            Rc::new(FakeWindow(calls.clone())),
        );
        let window = aqua_protocol::WindowId::new(id.raw()).unwrap();
        let mut app = Scripted {
            outputs: VecDeque::from([RuntimeOutput {
                shell: vec![
                    ShellCommand::SetCursor {
                        window,
                        icon: aqua_core::CursorIcon::Pointer,
                    },
                    ShellCommand::SetTitle {
                        window,
                        title: "renamed".into(),
                    },
                    ShellCommand::ClipboardRead {
                        request: RequestId(9),
                    },
                    ShellCommand::RequestFrame {
                        window,
                        reason: FrameReason::INPUT,
                    },
                ],
                wake_at: Some(Instant::from_nanos(5_000_000)),
                ..RuntimeOutput::default()
            }]),
            fed: Vec::new(),
        };
        aqua.flush_output(&mut app, id);
        assert_eq!(calls.0.borrow().as_slice(), ["cursor:Pointer"]);
        assert!(matches!(
            app.fed.as_slice(),
            [RuntimeInput::Shell(ShellEvent::Clipboard {
                request: RequestId(9),
                ..
            })]
        ));
        assert!(aqua.scheduler.has_pending());
        let creates = aqua.dispatch_window_requests(&mut app);
        assert!(creates.is_empty());
        assert!(calls.0.borrow().iter().any(|call| call == "title:renamed"));
    }
}
