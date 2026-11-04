use std::collections::HashMap;
use std::ops::{Deref, DerefMut};

use aqua_core::{InputEvent as CoreInputEvent, Key, Modifiers, NamedKey};
use aqua_protocol::{
    A11yActionRequest, AquaWindowId, Instant, Machine, ObservedWindowState, PixelSize, Recorded,
    RuntimeInput, RuntimeOutput, ShellEvent, WindowEvent, WindowSpec,
};
use aqua_reactive::{DepTuple, Runtime as ReactiveRuntime, Scope};
use aqua_runtime::Runtime;
use aqua_shell_win32::services::{Clipboard, ClipboardError};
use aqua_ui::element::Element;

use crate::runtime::{AppCtx, Application, Aqua};

pub use aqua_runtime::RuntimeError as UiAppError;

pub type WindowFactory = Box<dyn FnMut(AquaWindowId) -> Result<Runtime, UiAppError>>;

struct Slot {
    runtime: Runtime,
    pending: RuntimeOutput,
    log: Option<Vec<Recorded<RuntimeInput, RuntimeOutput>>>,
}

impl Slot {
    fn new(runtime: Runtime) -> Self {
        Self {
            runtime,
            pending: RuntimeOutput::default(),
            log: None,
        }
    }

    fn drive(&mut self, input: RuntimeInput) -> Option<Instant> {
        let recorded = self.log.is_some().then(|| input.clone());
        let output = self.runtime.step(input);
        let wake = output.wake_at;
        if let (Some(log), Some(input)) = (self.log.as_mut(), recorded) {
            log.push(Recorded {
                input,
                output: output.clone(),
            });
        }
        self.pending.merge(output);
        wake
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RootBinding {
    Unbound,
    Bound(AquaWindowId),
    Retired,
}

pub struct UiApp {
    root: Slot,
    binding: RootBinding,
    extra: HashMap<AquaWindowId, Slot>,
    factory: Option<WindowFactory>,
}

impl UiApp {
    pub fn new(fonts: aqua_text::SharedFonts, runs: aqua_text::SharedRuns) -> Self {
        Self::from_runtime(Runtime::new(fonts, runs))
    }

    pub fn from_runtime(inner: Runtime) -> Self {
        Self {
            root: Slot::new(inner),
            binding: RootBinding::Unbound,
            extra: HashMap::new(),
            factory: None,
        }
    }

    pub fn with_window_factory(
        mut self,
        factory: impl FnMut(AquaWindowId) -> Result<Runtime, UiAppError> + 'static,
    ) -> Self {
        self.factory = Some(Box::new(factory));
        self
    }

    pub fn record_session(&mut self, id: AquaWindowId) -> bool {
        self.claim(id)
            .map(|slot| slot.log.get_or_insert_with(Vec::new))
            .is_some()
    }

    pub fn recorded_session(&self, id: AquaWindowId) -> &[Recorded<RuntimeInput, RuntimeOutput>] {
        self.slot(id)
            .and_then(|slot| slot.log.as_deref())
            .unwrap_or_default()
    }

    pub fn into_runtime(self) -> Runtime {
        self.root.runtime
    }

    pub fn window_count(&self) -> usize {
        self.extra.len()
            + match self.binding {
                RootBinding::Bound(_) => 1,
                RootBinding::Unbound | RootBinding::Retired => 0,
            }
    }

    pub fn runtime_for(&self, id: AquaWindowId) -> Option<&Runtime> {
        self.slot(id).map(|slot| &slot.runtime)
    }

    pub fn runtime_for_mut(&mut self, id: AquaWindowId) -> Option<&mut Runtime> {
        self.claim(id).map(|slot| &mut slot.runtime)
    }

    fn slot(&self, id: AquaWindowId) -> Option<&Slot> {
        match self.binding {
            RootBinding::Bound(root) if root == id => Some(&self.root),
            RootBinding::Unbound | RootBinding::Bound(_) | RootBinding::Retired => {
                self.extra.get(&id)
            }
        }
    }

    fn slot_mut(&mut self, id: AquaWindowId) -> Option<&mut Slot> {
        match self.binding {
            RootBinding::Bound(root) if root == id => Some(&mut self.root),
            RootBinding::Unbound | RootBinding::Bound(_) | RootBinding::Retired => {
                self.extra.get_mut(&id)
            }
        }
    }

    fn claim(&mut self, id: AquaWindowId) -> Option<&mut Slot> {
        match self.binding {
            RootBinding::Unbound => {
                self.binding = RootBinding::Bound(id);
                Some(&mut self.root)
            }
            RootBinding::Bound(_) | RootBinding::Retired => self.slot_mut(id),
        }
    }

    fn open(&mut self, id: AquaWindowId) {
        if self.claim(id).is_some() {
            return;
        }
        let built = match self.factory.as_mut() {
            Some(factory) => factory(id),
            None => return,
        };
        match built {
            Ok(runtime) => {
                self.extra.insert(id, Slot::new(runtime));
            }
            Err(error) => eprintln!("aqua: window {id:?} mount failed: {error}"),
        }
    }

    fn retire(&mut self, id: AquaWindowId) {
        match self.binding {
            RootBinding::Bound(root) if root == id => {
                self.root
                    .drive(RuntimeInput::Shell(ShellEvent::Destroyed(id)));
                self.root.pending = RuntimeOutput::default();
                self.binding = RootBinding::Retired;
            }
            RootBinding::Unbound | RootBinding::Bound(_) | RootBinding::Retired => {
                self.extra.remove(&id);
            }
        }
    }

    pub fn absorb(&mut self, output: RuntimeOutput) {
        self.root.pending.merge(output);
    }

    pub fn drive(&mut self, input: RuntimeInput) {
        self.root.drive(input);
    }

    pub fn drive_window(&mut self, id: AquaWindowId, input: RuntimeInput) {
        if let Some(slot) = self.claim(id) {
            slot.drive(input);
        }
    }

    pub fn scene_mut(&mut self) -> &mut aqua_scene::Scene {
        self.root.runtime.scene_mut()
    }

    pub fn a11y_nodes(
        &self,
    ) -> Option<(
        aqua_core::NodeId,
        aqua_core::NodeId,
        Vec<aqua_protocol::A11yNode>,
    )> {
        self.root.runtime.a11y_nodes()
    }

    pub fn chrome_regions(&self) -> Option<&aqua_protocol::DragRegions> {
        self.root.runtime.chrome_regions()
    }

    pub fn cursor(&self) -> aqua_core::CursorIcon {
        self.root.runtime.cursor()
    }

    pub fn is_root_window(&self, id: AquaWindowId) -> bool {
        match self.binding {
            RootBinding::Unbound => true,
            RootBinding::Bound(root) => root == id,
            RootBinding::Retired => false,
        }
    }

    pub fn flush_scene_for(&mut self, id: AquaWindowId) {
        if let Some(slot) = self.slot_mut(id) {
            let diff = aqua_runtime::emit_scene_diff(slot.runtime.scene_mut());
            if !diff.is_empty() {
                slot.pending
                    .render
                    .push(aqua_protocol::RenderCommand::Scene(diff));
            }
        }
    }

    pub fn flush_scene(&mut self) {
        let diff = aqua_runtime::emit_scene_diff(self.root.runtime.scene_mut());
        if !diff.is_empty() {
            self.root
                .pending
                .render
                .push(aqua_protocol::RenderCommand::Scene(diff));
        }
    }

    pub fn take_pending(&mut self) -> RuntimeOutput {
        std::mem::take(&mut self.root.pending)
    }
}

impl Deref for UiApp {
    type Target = Runtime;

    fn deref(&self) -> &Runtime {
        &self.root.runtime
    }
}

impl DerefMut for UiApp {
    fn deref_mut(&mut self) -> &mut Runtime {
        &mut self.root.runtime
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardOutcome {
    Unhandled,
    Handled { redraw: bool },
}

pub fn dispatch_clipboard(
    app: &mut Runtime,
    clipboard: &mut dyn Clipboard,
    event: &CoreInputEvent,
) -> Result<ClipboardOutcome, ClipboardError> {
    let CoreInputEvent::KeyDown(key) = event else {
        return Ok(ClipboardOutcome::Unhandled);
    };
    let control = key.modifiers == Modifiers::CONTROL
        || key.modifiers == Modifiers::CONTROL.union(Modifiers::SHIFT);
    let shift = key.modifiers == Modifiers::SHIFT;
    let copy = control && matches!(key.key, Key::Char('c' | 'C') | Key::Named(NamedKey::Insert));
    let cut = (control && matches!(key.key, Key::Char('x' | 'X')))
        || (shift && key.key == Key::Named(NamedKey::Delete));
    let paste = (control && matches!(key.key, Key::Char('v' | 'V')))
        || (shift && key.key == Key::Named(NamedKey::Insert));
    if copy || cut {
        let mut redraw = false;
        if let Some(text) = app.selected_text() {
            clipboard.set_text(text)?;
            redraw = cut
                && app
                    .cut_external_selection()
                    .unwrap_or_else(|| app.delete_selected_text());
        }
        return Ok(ClipboardOutcome::Handled { redraw });
    }
    if paste {
        let files = match clipboard.get_files() {
            Ok(files) => files.unwrap_or_default(),
            Err(ClipboardError::Unsupported(_)) => Vec::new(),
            Err(error) => return Err(error),
        };
        let image = match clipboard.get_image() {
            Ok(Some(image)) => Some(
                aqua_ui::input::PasteImage::new(
                    image.width(),
                    image.height(),
                    std::sync::Arc::from(image.rgba()),
                )
                .ok_or(ClipboardError::InvalidImage)?,
            ),
            Ok(None) | Err(ClipboardError::Unsupported(_)) => None,
            Err(error) => return Err(error),
        };
        let text = clipboard.get_text()?.map(std::sync::Arc::from);
        let redraw = app.paste_clipboard(aqua_ui::input::ClipboardPaste {
            text,
            image,
            paths: files.into(),
        });
        return Ok(ClipboardOutcome::Handled { redraw });
    }
    Ok(ClipboardOutcome::Unhandled)
}

pub fn dispatch_input_with_clipboard_outcome(
    app: &mut Runtime,
    clipboard: &mut dyn Clipboard,
    event: &CoreInputEvent,
) -> Result<aqua_runtime::InputDispatchOutcome, ClipboardError> {
    let mut error = None;
    let output = app.step_input_with_default_outcome(event, |app| {
        match dispatch_clipboard(app, clipboard, event) {
            Ok(ClipboardOutcome::Unhandled) => None,
            Ok(ClipboardOutcome::Handled { redraw }) => Some(redraw),
            Err(failed) => {
                error = Some(failed);
                Some(false)
            }
        }
    });
    match error {
        Some(error) => Err(error),
        None => Ok(output),
    }
}

fn physical(size: aqua_core::Size, scale: f32) -> PixelSize {
    PixelSize {
        width: (size.w * scale).round().max(1.0) as u32,
        height: (size.h * scale).round().max(1.0) as u32,
    }
}

impl Application for UiApp {
    fn on_start(&mut self, _ctx: &mut AppCtx, id: AquaWindowId) {
        self.open(id);
    }

    fn on_external_file_drag(
        &mut self,
        ctx: &mut AppCtx,
        id: AquaWindowId,
        event: &crate::runtime::ExternalFileDrag,
    ) {
        let handled = self
            .claim(id)
            .is_some_and(|slot| slot.runtime.external_file_drag(event.clone()));
        if handled {
            ctx.request_frame(id, aqua_protocol::FrameReason::INPUT);
        }
    }

    fn input_invalidation_epoch(&self, id: AquaWindowId) -> u64 {
        self.slot(id)
            .map_or(0, |slot| slot.runtime.tree().invalidation_epoch())
    }

    fn color_profile(&self) -> aqua_core::ColorProfile {
        self.root.runtime.color_profile()
    }

    fn color_profile_for(&self, id: AquaWindowId) -> aqua_core::ColorProfile {
        self.slot(id)
            .map_or_else(|| self.color_profile(), |slot| slot.runtime.color_profile())
    }

    fn cursor_for(&self, id: AquaWindowId) -> Option<aqua_core::CursorIcon> {
        self.slot(id).map(|slot| slot.runtime.cursor())
    }

    fn a11y_nodes_for(
        &self,
        id: AquaWindowId,
    ) -> Option<(aqua_core::NodeId, aqua_core::NodeId, Vec<aqua_protocol::A11yNode>)> {
        self.slot(id).and_then(|slot| slot.runtime.a11y_nodes())
    }

    fn on_click_settings(&mut self, settings: aqua_core::ClickSettings) {
        self.root.runtime.set_click_settings(settings);
        self.extra
            .values_mut()
            .for_each(|slot| slot.runtime.set_click_settings(settings));
    }

    fn on_window_event(&mut self, _ctx: &mut AppCtx, id: AquaWindowId, event: &WindowEvent) {
        let resized = |slot: &Slot| match event {
            WindowEvent::Resized(_, size) => Some((size.as_core_size(), slot.runtime.scale())),
            WindowEvent::ScaleChanged(_, scale) => Some((slot.runtime.viewport(), *scale as f32)),
            _ => None,
        };
        match event {
            WindowEvent::Destroyed(_) => self.retire(id),
            _ => {
                if let Some(slot) = self.claim(id) {
                    if let Some((size, scale)) = resized(slot) {
                        slot.drive(RuntimeInput::Shell(ShellEvent::Resized {
                            window: id,
                            size: physical(size, scale),
                            scale,
                        }));
                    }
                }
            }
        }
    }

    fn on_input(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &CoreInputEvent) {
        let now = ctx.now();
        if let Some(slot) = self.claim(id) {
            slot.drive(RuntimeInput::Tick(now));
            slot.drive(RuntimeInput::Shell(ShellEvent::Input {
                window: id,
                event: event.clone(),
            }));
        }
    }

    fn on_files_dropped(
        &mut self,
        ctx: &mut AppCtx,
        id: AquaWindowId,
        paths: &[std::path::PathBuf],
    ) {
        let handled = self
            .claim(id)
            .is_some_and(|slot| slot.runtime.drop_files(paths.to_vec()));
        if handled {
            ctx.request_frame(id, aqua_protocol::FrameReason::INPUT);
        }
    }

    fn on_files_dropped_at(
        &mut self,
        ctx: &mut AppCtx,
        id: AquaWindowId,
        paths: &[std::path::PathBuf],
        at: Option<aqua_core::Point>,
    ) {
        let handled = self
            .claim(id)
            .is_some_and(|slot| slot.runtime.drop_files_at(paths.to_vec(), at));
        if handled {
            ctx.request_frame(id, aqua_protocol::FrameReason::INPUT);
        }
    }

    fn on_a11y_action(&mut self, _ctx: &mut AppCtx, id: AquaWindowId, request: &A11yActionRequest) {
        self.drive_window(id, RuntimeInput::Access(request.clone()));
    }

    fn on_dialog(
        &mut self,
        _ctx: &mut AppCtx,
        id: AquaWindowId,
        request: aqua_protocol::RequestId,
        outcome: aqua_protocol::DialogOutcome,
    ) {
        self.drive_window(
            id,
            RuntimeInput::Shell(ShellEvent::Dialog { request, outcome }),
        );
    }

    fn build_scene(&mut self, window: &ObservedWindowState, now: Instant) -> Option<Instant> {
        let slot = self.claim(window.id)?;
        slot.drive(RuntimeInput::Tick(now));
        let wake = slot.drive(RuntimeInput::Shell(ShellEvent::Frame {
            window: window.id,
            presentation: now,
        }));
        slot.runtime
            .take_diagnostics()
            .iter()
            .for_each(|line| eprintln!("{line}"));
        wake
    }

    fn step_input(&mut self, id: AquaWindowId, input: RuntimeInput) {
        self.drive_window(id, input);
    }

    fn take_output(&mut self, id: AquaWindowId) -> RuntimeOutput {
        self.slot_mut(id)
            .map(|slot| std::mem::take(&mut slot.pending))
            .unwrap_or_default()
    }

    fn reactive_runtime(&self) -> Option<&ReactiveRuntime> {
        Some(self.root.runtime.runtime())
    }

    fn reactive_runtime_for(&self, id: AquaWindowId) -> Option<&ReactiveRuntime> {
        self.slot(id).map(|slot| slot.runtime.runtime())
    }
}

impl Aqua {
    pub fn run_ui_windows(
        self,
        factory: impl FnOnce(&Scope) -> Element,
        windows: impl FnMut(AquaWindowId) -> Result<Runtime, UiAppError> + 'static,
    ) -> Result<(), String> {
        self.run_ui_windows_opening(Vec::new(), factory, windows)
    }

    pub fn run_ui_windows_opening(
        self,
        opening: impl IntoIterator<Item = WindowSpec>,
        factory: impl FnOnce(&Scope) -> Element,
        windows: impl FnMut(AquaWindowId) -> Result<Runtime, UiAppError> + 'static,
    ) -> Result<(), String> {
        let mut app =
            UiApp::new(self.fonts().clone(), self.runs().clone()).with_window_factory(windows);
        app.apply_startup_motion_policy(self.motion_reduced());
        app.set_frame_stats(std::env::var_os("AQUA_FRAME_STATS").is_some_and(|value| value == "1"));
        app.mount(factory)
            .map_err(|e| format!("ui mount failed: {e}"))?;
        opening.into_iter().for_each(|spec| {
            app.open_window(spec);
        });
        self.run(app)
    }

    pub fn run_ui(self, factory: impl FnOnce(&Scope) -> Element) -> Result<(), String> {
        let mut app = UiApp::new(self.fonts().clone(), self.runs().clone());
        app.apply_startup_motion_policy(self.motion_reduced());
        app.set_frame_stats(std::env::var_os("AQUA_FRAME_STATS").is_some_and(|value| value == "1"));
        app.mount(factory)
            .map_err(|e| format!("ui mount failed: {e}"))?;
        self.run(app)
    }

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
