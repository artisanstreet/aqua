use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use aqua_a11y_uia::UiaAdapter;
use aqua_core::{InputEvent, Oklch};
use aqua_graph::Extent;
use aqua_ime_tsf::TsfIme;
use aqua_protocol::window::Titlebar;
use aqua_protocol::{
    AquaWindowId, FrameReason, LogicalSize, ObservedWindowState, ThemePreference, WindowEvent,
    WindowOptions, WindowRequest,
};
use aqua_protocol::{CaptionControl, DialogOutcome, Instant, RequestId, RuntimeInput};
use aqua_shell_win32::dialogs::DialogJob;
use aqua_shell_win32::{
    Chrome, FileDrag, Handler, ShellError, ShellEvent, Window, WindowKind,
    WindowOptions as ShellOptions,
};

use super::gpu;
use super::hook::{TsfHook, UiaHook};
use super::inspector::HostInspector;
use super::wakes::{self, WakeLedger};
use super::window::{UiaSink, Win32Window};
use crate::coalesce::{merge, Coalescible, Held, Merge};
use crate::inbox::AquaUserEvent;
use crate::runtime::{Application, Aqua, ExternalFileDrag};

const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
const FILE_DROP_IDLE: Duration = Duration::from_millis(30);

struct Host<A: Application> {
    aqua: Aqua,
    app: A,
    shell: HashMap<AquaWindowId, Window>,
    dialogs: Vec<(AquaWindowId, RequestId, DialogJob)>,
    inspector: Option<HostInspector>,
    pending: Option<Held>,
    resizes: HashMap<AquaWindowId, (u32, u32)>,
    occlusion: HashMap<AquaWindowId, Occlusion>,
    wakes: Option<WakeLedger>,
    file_drop_pending: Vec<AquaWindowId>,
}

#[derive(Default, Clone, Copy)]
struct Occlusion {
    minimized: bool,
    covered: bool,
}

impl Occlusion {
    fn hidden(self) -> bool {
        self.minimized || self.covered
    }
}

fn render_when_covered() -> bool {
    std::env::var_os("AQUA_RENDER_WHEN_COVERED").is_some_and(|value| value == "1")
}

fn shell_options(options: &WindowOptions, aqua: &Aqua) -> ShellOptions {
    let chrome = match options.effective_titlebar() {
        Titlebar::Native => Chrome::Native,
        Titlebar::Overlay | Titlebar::Hidden => Chrome::Custom,
    };
    let dark = match options.theme {
        ThemePreference::Dark => true,
        ThemePreference::Light => false,
        ThemePreference::Unknown => aqua.preferences().theme == ThemePreference::Dark,
    };
    ShellOptions {
        title: options.title.clone(),
        logical_size: (options.inner_size.width, options.inner_size.height),
        min_logical_size: options.min_size.map(|size| (size.width, size.height)),
        position: None,
        kind: WindowKind::Normal,
        chrome,
        material: aqua.config().effective_material(aqua.preferences()),
        dark,
        topmost: false,
        owner: None,
        resizable: options.resizable,
        minimizable: options.minimizable,
        maximizable: options.maximizable,
        visible: false,
        alpha_surface: options.transparent,
    }
}

fn message_label(event: &ShellEvent) -> &'static str {
    match event {
        ShellEvent::Resized { .. } => "resized",
        ShellEvent::Moved { .. } => "moved",
        ShellEvent::ScaleChanged { .. } => "scale",
        ShellEvent::CloseRequested => "close",
        ShellEvent::Focused(_) => "focus",
        ShellEvent::Minimized(_) => "minimized",
        ShellEvent::Covered(_) => "covered",
        ShellEvent::Maximized(_) => "maximized",
        ShellEvent::Input(_) => "input",
        ShellEvent::FileDrag(_) => "file-drag",
        ShellEvent::PointerEntered | ShellEvent::PointerLeft => "pointer-edge",
        ShellEvent::CaptionHover(_) => "caption-hover",
        ShellEvent::RawMouse { .. } => "raw-mouse",
        ShellEvent::PreferencesChanged => "preferences",
        ShellEvent::Redraw => "redraw",
        ShellEvent::Destroyed => "destroyed",
    }
}

fn logical(window: Window) -> (LogicalSize, f64) {
    let scale = window.scale();
    let (width, height) = window.inner_size();
    (
        LogicalSize::new(f64::from(width) / scale, f64::from(height) / scale),
        scale,
    )
}

fn physical(window: Window) -> Extent {
    let (width, height) = window.inner_size();
    Extent {
        width: width.max(1),
        height: height.max(1),
    }
}

impl<A: Application + 'static> Host<A> {
    fn occlude(&mut self, id: AquaWindowId, change: impl FnOnce(&mut Occlusion)) {
        let state = self.occlusion.entry(id).or_default();
        let before = state.hidden();
        change(state);
        let after = state.hidden();
        if before != after {
            self.window_event(id, WindowEvent::Occluded(id, after));
        }
    }

    fn create_window(&mut self, options: WindowOptions) -> Result<AquaWindowId, String> {
        options.validated().map_err(|e| format!("{e:?}"))?;
        super::startup_mark("window_create_begin");
        let window = Window::create(&shell_options(&options, &self.aqua))
            .map_err(|error: ShellError| error.to_string())?;
        super::startup_mark("window_created");
        let id = window.id();
        let (inner, scale) = logical(window);
        let observed = ObservedWindowState {
            id,
            title: options.title.clone(),
            inner_size: inner,
            scale_factor: scale,
            focused: false,
            visible: true,
            occluded: false,
            maximized: window.is_maximized(),
        };
        let native: Rc<dyn crate::native_window::NativeWindow> = Rc::new(Win32Window { window });
        self.aqua.add_native_window(observed.clone(), native);
        self.shell.insert(id, window);
        self.file_drop_pending.push(id);
        self.rebind_waker();
        let inbox = self.aqua.inbox();
        let wake: aqua_a11y_uia::WakeFn = Arc::new(move || {
            inbox.post(AquaUserEvent::A11yActions(id));
        });
        super::startup_mark("uia_begin");
        let attached = UiaAdapter::attach_routed(id.raw() as isize, wake);
        super::startup_mark("uia_done");
        match attached {
            Ok((adapter, router)) => {
                aqua_shell_win32::add_message_hook(UiaHook::new(id.raw() as isize, router));
                let now = self.aqua.clock();
                if let Some(deadline) = self.app.prepare_accessibility(&observed, now) {
                    self.aqua.scheduler.request_animation_deadline(id, deadline);
                }
                self.aqua.attach_access(
                    id,
                    Box::new(UiaSink {
                        adapter,
                        inspector: self.inspector.as_ref().map(HostInspector::handle),
                    }),
                );
            }
            Err(error) => eprintln!("aqua: uia attach failed: {error:?}"),
        }
        super::startup_mark("ime_begin");
        self.aqua
            .attach_ime(id, Box::new(TsfIme::from_hwnd(id.raw() as isize)));
        super::startup_mark("window_shown");
        window.show();
        super::startup_mark("window_visible");
        let clear = match options.transparent {
            true => Oklch::TRANSPARENT,
            false => Oklch::BLACK,
        };
        let factory = gpu::factory(
            id,
            id.raw() as isize,
            physical(window),
            self.aqua.fonts().clone(),
            self.aqua.runs().clone(),
            clear,
        );
        super::startup_mark("factory_built");
        self.aqua.attach_render(id, factory, FRAME_INTERVAL);
        super::startup_mark("render_attached");
        self.aqua.dispatch_start(&mut self.app, id);
        super::startup_mark("app_started");
        self.aqua.scheduler.request_frame(id, FrameReason::EXPOSE);
        Ok(id)
    }

    fn enable_file_drop_when_idle(&mut self, now: Instant, next: Option<Instant>) {
        let idle = next.is_none_or(|at| at.saturating_duration_since(now) > FILE_DROP_IDLE);
        if self.aqua.frames_presented == 0 || !idle {
            return;
        }
        let pending = std::mem::take(&mut self.file_drop_pending);
        pending
            .into_iter()
            .filter_map(|id| self.shell.get(&id))
            .for_each(|window| window.enable_file_drop());
    }

    fn publish_inspector(&self) {
        let Some(inspector) = &self.inspector else {
            return;
        };
        let stats = self
            .aqua
            .windows
            .values()
            .find_map(|record| record.link.as_ref())
            .map(|link| link.stats());
        if let Some(stats) = stats {
            inspector.publish_stats(stats);
        }
    }

    fn rebind_waker(&mut self) {
        let waker = self.shell.values().next().map(|window| window.waker());
        self.aqua
            .inbox()
            .set_waker(waker.map(|waker| Arc::new(move || waker.wake()) as crate::inbox::Waker));
    }

    fn process_requests(&mut self) {
        let creates = self.aqua.dispatch_window_requests(&mut self.app);
        for options in creates {
            if let Err(message) = self.create_window(options.clone()) {
                self.aqua.render_errors += 1;
                let error = aqua_protocol::window::WindowError::Native(message);
                self.app
                    .on_window_request_error(&WindowRequest::Create(options), &error);
            }
        }
        self.reap_closed();
        while let Some((id, request, spec)) = self.aqua.dialogs.pop_front() {
            let owner = self.shell.get(&id).copied();
            let waker = owner.or_else(|| self.shell.values().next().copied());
            let Some(waker) = waker.map(|window| window.waker()) else {
                continue;
            };
            match aqua_shell_win32::dialogs::open(&spec, owner, waker) {
                Ok(job) => self.dialogs.push((id, request, job)),
                Err(error) => {
                    let outcome = DialogOutcome::Failed(error.to_string());
                    self.aqua
                        .with_ctx(|ctx| self.app.on_dialog(ctx, id, request, outcome));
                }
            }
        }
    }

    fn poll_dialogs(&mut self) {
        let mut finished = Vec::new();
        self.dialogs.retain(|(id, request, job)| match job.poll() {
            Some(result) => {
                finished.push((*id, *request, result));
                false
            }
            None => true,
        });
        for (id, request, result) in finished {
            let outcome = match result {
                Ok(paths) if paths.is_empty() => DialogOutcome::Dismissed,
                Ok(paths) => DialogOutcome::Chosen(
                    paths
                        .into_iter()
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect(),
                ),
                Err(error) => DialogOutcome::Failed(error.to_string()),
            };
            self.aqua
                .with_ctx(|ctx| self.app.on_dialog(ctx, id, request, outcome));
        }
    }

    fn reap_closed(&mut self) {
        let gone: Vec<_> = self
            .shell
            .keys()
            .filter(|id| !self.aqua.windows.contains_key(id))
            .copied()
            .collect();
        for id in gone {
            if let Some(window) = self.shell.remove(&id) {
                window.destroy();
            }
        }
        self.rebind_waker();
        if self.aqua.windows.is_empty() && !self.aqua.config().headless {
            aqua_shell_win32::quit();
        }
    }

    fn window_event(&mut self, id: AquaWindowId, event: WindowEvent) {
        self.aqua.dispatch_window_event(&mut self.app, id, &event);
        self.reap_closed();
    }

    fn file_drag(&mut self, id: AquaWindowId, drag: FileDrag, scale: f64) {
        let _ = scale;
        let event = match &drag {
            FileDrag::Enter { paths, position } => Some(ExternalFileDrag::Enter {
                paths: paths.clone(),
                position: *position,
            }),
            FileDrag::Over { position } => Some(ExternalFileDrag::Move {
                position: *position,
            }),
            FileDrag::Leave => Some(ExternalFileDrag::Exit),
            FileDrag::Drop { position, .. } => Some(ExternalFileDrag::Submit {
                position: *position,
            }),
        };
        if let Some(event) = event {
            self.aqua
                .dispatch_attributed_input(&mut self.app, id, true, |app, ctx| {
                    app.on_external_file_drag(ctx, id, &event)
                });
            self.aqua.flush_output(&mut self.app, id);
        }
        if let FileDrag::Drop { paths, position } = drag {
            self.aqua
                .dispatch_file_drop_at(&mut self.app, id, &paths, Some(position));
        }
    }

    fn caption_hover(&mut self, id: AquaWindowId, control: Option<CaptionControl>) {
        self.app.step_input(
            id,
            RuntimeInput::Shell(aqua_protocol::ShellEvent::CaptionHover {
                window: id,
                control,
            }),
        );
        self.aqua
            .with_ctx(|ctx| self.app.on_caption_hover(ctx, id, control));
        self.aqua.flush_output(&mut self.app, id);
    }

    fn flush_pending(&mut self) {
        if let Some(held) = self.pending.take() {
            self.input(held.window, held.input.into_input());
        }
    }

    fn dispatch_resizes(&mut self, gated_at: Option<Instant>) {
        let ids: Vec<AquaWindowId> = self.resizes.keys().copied().collect();
        for id in ids {
            let gated = gated_at.is_some_and(|now| {
                self.aqua
                    .scheduler
                    .resize_gate(id)
                    .is_some_and(|gate| now < gate)
            });
            if gated {
                continue;
            }
            let Some((width, height)) = self.resizes.remove(&id) else {
                continue;
            };
            let scale = self.shell.get(&id).map_or(1.0, |window| window.scale());
            let size = LogicalSize::new(f64::from(width) / scale, f64::from(height) / scale);
            self.window_event(id, WindowEvent::Resized(id, size));
        }
    }

    fn queue(&mut self, next: Held) {
        match self.pending.take() {
            None => self.pending = Some(next),
            Some(held) => match merge(held, next) {
                Merge::Merged(merged) => self.pending = Some(merged),
                Merge::Split(held, next) => {
                    self.pending = Some(held);
                    self.flush_pending();
                    self.pending = Some(next);
                }
            },
        }
    }

    fn input(&mut self, id: AquaWindowId, input: InputEvent) {
        if matches!(input, InputEvent::PointerDown(..)) {
            if let Some(native) = self
                .aqua
                .windows
                .get(&id)
                .and_then(|record| record.native.clone())
            {
                self.app.on_click_settings(native.click_settings());
            }
        }
        self.aqua.dispatch_input(&mut self.app, id, &input);
    }
}

impl<A: Application + 'static> Handler for Host<A> {
    fn window_event(&mut self, id: aqua_protocol::WindowId, event: ShellEvent) {
        if !self.aqua.windows.contains_key(&id) {
            return;
        }
        let scale = self.shell.get(&id).map_or(1.0, |window| window.scale());
        if let ShellEvent::Input(input) = &event {
            if let Some(input) = Coalescible::classify(input) {
                self.queue(Held { window: id, input });
                return;
            }
        }
        self.flush_pending();
        if let Some(ledger) = &mut self.wakes {
            ledger.message(message_label(&event));
        }
        if !matches!(
            event,
            ShellEvent::Resized { .. }
                | ShellEvent::Maximized(_)
                | ShellEvent::Moved { .. }
                | ShellEvent::Redraw
                | ShellEvent::Covered(_)
                | ShellEvent::Minimized(_)
                | ShellEvent::PointerEntered
                | ShellEvent::PointerLeft
                | ShellEvent::RawMouse { .. }
        ) {
            self.dispatch_resizes(None);
        }
        match event {
            ShellEvent::Resized { width, height } if width > 0 && height > 0 => {
                self.resizes.insert(id, (width, height));
            }
            ShellEvent::Resized { .. } => {}
            ShellEvent::ScaleChanged { scale } => {
                self.window_event(id, WindowEvent::ScaleChanged(id, scale));
            }
            ShellEvent::CloseRequested => self.window_event(id, WindowEvent::CloseRequested(id)),
            ShellEvent::Destroyed => {
                self.occlusion.remove(&id);
                self.resizes.remove(&id);
                self.window_event(id, WindowEvent::Destroyed(id))
            }
            ShellEvent::Focused(focused) => {
                self.window_event(id, WindowEvent::Focused(id, focused))
            }
            ShellEvent::Minimized(minimized) => {
                self.occlude(id, |state| state.minimized = minimized)
            }
            ShellEvent::Covered(covered) => self.occlude(id, |state| {
                state.covered = covered && !render_when_covered()
            }),
            ShellEvent::Maximized(maximized) => {
                if let Some(record) = self.aqua.windows.get_mut(&id) {
                    record.observed.maximized = maximized;
                }
            }
            ShellEvent::Input(input) => self.input(id, input),
            ShellEvent::FileDrag(drag) => self.file_drag(id, drag, scale),
            ShellEvent::PreferencesChanged => {
                let prefs = aqua_shell_win32::prefs::query();
                let scheme = prefs.theme;
                self.aqua.prefs = prefs;
                self.window_event(id, WindowEvent::ThemeChanged(id, scheme));
            }
            ShellEvent::Redraw => self.aqua.scheduler.request_frame(id, FrameReason::EXPLICIT),
            ShellEvent::Moved { .. }
            | ShellEvent::PointerEntered
            | ShellEvent::PointerLeft
            | ShellEvent::RawMouse { .. } => {}
            ShellEvent::CaptionHover(control) => self.caption_hover(id, control),
        }
    }

    fn woken(&mut self) {
        if let Some(ledger) = &mut self.wakes {
            ledger.woken();
        }
        self.flush_pending();
        self.aqua.pump_inbox(&mut self.app);
    }

    fn tick(&mut self, now: Instant) -> Option<Instant> {
        self.aqua.wakeups += 1;
        let presented_before = self.aqua.frames_presented;
        self.flush_pending();
        aqua_core::checkpoint::reach();
        self.dispatch_resizes(Some(now));
        aqua_core::checkpoint::reach();
        self.aqua.drain_text(&mut self.app);
        aqua_core::checkpoint::reach();
        self.aqua.draw_input_frames(&mut self.app, now);
        aqua_core::checkpoint::reach();
        self.aqua.pump_inbox(&mut self.app);
        aqua_core::checkpoint::reach();
        self.process_requests();
        self.poll_dialogs();
        aqua_core::checkpoint::reach();
        self.aqua.draw_ready(&mut self.app, now);
        aqua_core::checkpoint::reach();
        self.aqua.report_host_stats();
        self.publish_inspector();
        if self.aqua.quit_requested() {
            aqua_shell_win32::quit();
            return None;
        }
        if self
            .aqua
            .smoke_deadline
            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
        {
            eprintln!(
                "aqua: smoke report: wakeups={} presented_frames={} render_errors={}",
                self.aqua.wakeups, self.aqua.frames_presented, self.aqua.render_errors
            );
            aqua_shell_win32::quit();
            return None;
        }
        let smoke = self
            .aqua
            .smoke_deadline
            .map(|deadline| now + deadline.saturating_duration_since(std::time::Instant::now()));
        let dialogs = (!self.dialogs.is_empty()).then(|| now + Duration::from_millis(50));
        let resize_gate = self
            .resizes
            .keys()
            .filter_map(|id| self.aqua.scheduler.resize_gate(*id))
            .min();
        let next = [self.aqua.next_wake(now), smoke, dialogs, resize_gate]
            .into_iter()
            .flatten()
            .min();
        self.enable_file_drop_when_idle(now, next);
        let presented = self.aqua.frames_presented != presented_before;
        if let Some(ledger) = &mut self.wakes {
            ledger.tick(now, presented, next);
        }
        next
    }
}

pub fn run<A: Application + 'static>(mut aqua: Aqua, app: A) -> Result<(), String> {
    if aqua_shell_win32::dialogs::run_helper_if_requested() {
        return Ok(());
    }
    gpu::prewarm();
    aqua_core::checkpoint::install(aqua_shell_win32::pump_sent_messages);
    super::opt_out_of_throttling();
    super::raise_thread_priority();
    aqua.mark_native_loop();
    let inbox = aqua.inbox();
    aqua.align_clock(aqua_shell_win32::now());
    let options = aqua.config().window.clone();
    let mut host = Host {
        aqua,
        app,
        shell: HashMap::new(),
        dialogs: Vec::new(),
        inspector: HostInspector::from_env(),
        pending: None,
        resizes: HashMap::new(),
        occlusion: HashMap::new(),
        wakes: wakes::enabled().then(|| WakeLedger::new(inbox)),
        file_drop_pending: Vec::new(),
    };
    super::startup_mark("host_ready");
    aqua_shell_win32::set_message_hook(TsfHook::new());
    super::startup_mark("hook_set");
    if let Err(message) = host.create_window(options.clone()) {
        let error = aqua_protocol::window::WindowError::Native(message.clone());
        host.app
            .on_window_request_error(&WindowRequest::Create(options), &error);
        return Err(format!("startup window failed: {message}"));
    }
    aqua_shell_win32::run(host).map_err(|error| error.to_string())
}
