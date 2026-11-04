use std::ops::{Deref, DerefMut};

use aqua_core::{InputEvent as CoreInputEvent, Key, Modifiers, NamedKey};
use aqua_platform::{
    services::{Clipboard, ClipboardError},
    AquaWindowId, ObservedWindowState, WindowEvent,
};
use aqua_protocol::{
    A11yActionRequest, Instant, Machine, PixelSize, RuntimeInput, RuntimeOutput, ShellEvent,
};
use aqua_reactive::{DepTuple, Runtime as ReactiveRuntime, Scope};
use aqua_runtime::Runtime;
use aqua_ui::element::Element;

use crate::runtime::{AppCtx, Application, Aqua};

pub use aqua_runtime::RuntimeError as UiAppError;

pub struct UiApp {
    inner: Runtime,
    pending: RuntimeOutput,
}

impl UiApp {
    pub fn new(fonts: aqua_text::SharedFonts, runs: aqua_text::SharedRuns) -> Self {
        Self::from_runtime(Runtime::new(fonts, runs))
    }

    pub fn from_runtime(inner: Runtime) -> Self {
        Self {
            inner,
            pending: RuntimeOutput::default(),
        }
    }

    pub fn into_runtime(self) -> Runtime {
        self.inner
    }

    pub fn absorb(&mut self, output: RuntimeOutput) {
        self.pending.merge(output);
    }

    pub fn drive(&mut self, input: RuntimeInput) {
        let output = self.inner.step(input);
        self.pending.merge(output);
    }

    pub fn scene_mut(&mut self) -> &mut aqua_scene::Scene {
        self.inner.scene_mut()
    }

    pub fn a11y_nodes(
        &self,
    ) -> Option<(
        aqua_core::NodeId,
        aqua_core::NodeId,
        Vec<aqua_protocol::A11yNode>,
    )> {
        self.inner.a11y_nodes()
    }

    pub fn chrome_regions(&self) -> Option<&aqua_protocol::DragRegions> {
        self.inner.chrome_regions()
    }

    pub fn cursor(&self) -> aqua_core::CursorIcon {
        self.inner.cursor()
    }

    pub fn flush_scene(&mut self) {
        let diff = aqua_runtime::emit_scene_diff(self.inner.scene_mut());
        if !diff.is_empty() {
            self.pending
                .render
                .push(aqua_protocol::RenderCommand::Scene(diff));
        }
    }

    pub fn take_pending(&mut self) -> RuntimeOutput {
        std::mem::take(&mut self.pending)
    }
}

impl Deref for UiApp {
    type Target = Runtime;

    fn deref(&self) -> &Runtime {
        &self.inner
    }
}

impl DerefMut for UiApp {
    fn deref_mut(&mut self) -> &mut Runtime {
        &mut self.inner
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
    fn on_external_file_drag(
        &mut self,
        ctx: &mut AppCtx,
        id: AquaWindowId,
        event: &crate::runtime::ExternalFileDrag,
    ) {
        if self.inner.external_file_drag(event.clone()) {
            ctx.request_frame(id, aqua_protocol::FrameReason::INPUT);
        }
    }

    fn input_invalidation_epoch(&self, _id: AquaWindowId) -> u64 {
        self.tree().invalidation_epoch()
    }

    fn color_profile(&self) -> aqua_core::ColorProfile {
        self.inner.color_profile()
    }

    fn on_click_settings(&mut self, settings: aqua_core::ClickSettings) {
        self.set_click_settings(settings);
    }

    fn on_window_event(&mut self, _ctx: &mut AppCtx, id: AquaWindowId, event: &WindowEvent) {
        let resized = match event {
            WindowEvent::Resized(_, size) => Some((size.as_core_size(), self.scale())),
            WindowEvent::ScaleChanged(_, scale) => Some((self.viewport(), *scale as f32)),
            _ => None,
        };
        if let Some((size, scale)) = resized {
            self.drive(RuntimeInput::Shell(ShellEvent::Resized {
                window: id,
                size: physical(size, scale),
                scale,
            }));
        }
    }

    fn on_input(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &CoreInputEvent) {
        self.drive(RuntimeInput::Tick(ctx.now()));
        self.drive(RuntimeInput::Shell(ShellEvent::Input {
            window: id,
            event: event.clone(),
        }));
    }

    fn on_files_dropped(
        &mut self,
        ctx: &mut AppCtx,
        id: AquaWindowId,
        paths: &[std::path::PathBuf],
    ) {
        if self.drop_files(paths.to_vec()) {
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
        if self.drop_files_at(paths.to_vec(), at) {
            ctx.request_frame(id, aqua_protocol::FrameReason::INPUT);
        }
    }

    fn on_a11y_action(
        &mut self,
        _ctx: &mut AppCtx,
        _id: AquaWindowId,
        request: &A11yActionRequest,
    ) {
        self.drive(RuntimeInput::Access(request.clone()));
    }

    fn on_dialog(
        &mut self,
        _ctx: &mut AppCtx,
        _id: AquaWindowId,
        request: aqua_protocol::RequestId,
        outcome: aqua_protocol::DialogOutcome,
    ) {
        self.drive(RuntimeInput::Shell(ShellEvent::Dialog { request, outcome }));
    }

    fn build_scene(&mut self, window: &ObservedWindowState, now: Instant) -> Option<Instant> {
        self.drive(RuntimeInput::Tick(now));
        let output = self.inner.step(RuntimeInput::Shell(ShellEvent::Frame {
            window: window.id,
            presentation: now,
        }));
        let wake = output.wake_at;
        self.pending.merge(output);
        self.take_diagnostics()
            .iter()
            .for_each(|line| eprintln!("{line}"));
        wake
    }

    fn step_input(&mut self, _id: AquaWindowId, input: RuntimeInput) {
        self.drive(input);
    }

    fn take_output(&mut self, _id: AquaWindowId) -> RuntimeOutput {
        self.take_pending()
    }

    fn reactive_runtime(&self) -> Option<&ReactiveRuntime> {
        Some(self.inner.runtime())
    }
}

impl Aqua {
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
