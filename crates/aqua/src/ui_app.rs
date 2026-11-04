use aqua_core::{InputEvent as CoreInputEvent, Key, Modifiers, NamedKey};
use aqua_platform::{
    a11y::A11yActionRequest,
    chrome::DragRegions,
    services::{Clipboard, ClipboardError},
    AquaWindowId, ObservedWindowState, WindowEvent,
};
use aqua_protocol::{A11yNode, Input, Instant, Output};
use aqua_reactive::{DepTuple, Runtime as ReactiveRuntime, Scope};
use aqua_render::Scene;
use aqua_ui::element::Element;

use crate::runtime::{AppCtx, Application, Aqua};

pub use aqua_runtime::{Runtime as UiApp, RuntimeError as UiAppError};

#[cfg(windows)]
struct NativeImeClient(aqua_runtime::NativeTextService);

#[cfg(windows)]
impl aqua_platform::ime_windows::NativeImeClient for NativeImeClient {
    fn selected_bounds(&self) -> Option<aqua_platform::services::ImeCursorArea> {
        let bounds = self.0.selected_bounds()?;
        Some(aqua_platform::services::ImeCursorArea {
            origin: aqua_platform::geometry::LogicalPosition {
                x: bounds.x as f64,
                y: bounds.y as f64,
            },
            size: aqua_platform::geometry::LogicalSize::new(bounds.w as f64, bounds.h as f64),
        })
    }

    fn replace(&self, text: &str, selected: Option<std::ops::Range<usize>>) {
        self.0.replace_utf16(text, selected);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardOutcome {
    Unhandled,
    Handled { redraw: bool },
}

pub fn dispatch_clipboard(
    app: &mut UiApp,
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

pub fn dispatch_input_with_clipboard(
    app: &mut UiApp,
    clipboard: &mut dyn Clipboard,
    event: &CoreInputEvent,
) -> Result<Output, ClipboardError> {
    dispatch_input_with_clipboard_outcome(app, clipboard, event).map(|result| result.output)
}

pub fn dispatch_input_with_clipboard_outcome(
    app: &mut UiApp,
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

pub fn dispatch_input(
    app: &mut UiApp,
    ctx: &mut AppCtx,
    id: AquaWindowId,
    event: &CoreInputEvent,
) -> bool {
    let _ = app.step(Input::Tick(ctx.now()));
    match dispatch_input_with_clipboard_outcome(app, ctx.clipboard(), event) {
        Ok(outcome) => {
            if !outcome.output.redraw.is_empty() {
                ctx.request_input_redraw(id, outcome.output.redraw);
            }
            outcome.consumed
        }
        Err(error) => {
            eprintln!("aqua: {error}");
            true
        }
    }
}

fn request_redraw(ctx: &mut AppCtx, id: AquaWindowId, output: Result<Output, UiAppError>) {
    if let Ok(output) = output {
        if !output.redraw.is_empty() {
            ctx.request_frame(id, output.redraw);
        }
    }
}

impl Application for UiApp {
    fn on_external_file_drag(
        &mut self,
        ctx: &mut AppCtx,
        id: AquaWindowId,
        event: &aqua_platform::events::ExternalFileDrag,
    ) {
        use aqua_platform::events::ExternalFileDrag as Native;
        use aqua_ui::input::ExternalFileDragEvent as Ui;
        let event = match event {
            Native::Enter { paths, position } => Ui::Enter {
                paths: paths.clone(),
                position: *position,
            },
            Native::Move { position } => Ui::Move {
                position: *position,
            },
            Native::Submit { position } => Ui::Submit {
                position: *position,
            },
            Native::Exit => Ui::Exit,
        };
        if self.external_file_drag(event) {
            ctx.request_frame(id, aqua_protocol::FrameReason::INPUT);
        }
    }
    fn input_invalidation_epoch(&self, _id: AquaWindowId) -> u64 {
        self.tree().invalidation_epoch()
    }

    fn color_profile(&self) -> aqua_core::ColorProfile {
        UiApp::color_profile(self)
    }

    fn on_click_settings(&mut self, settings: aqua_core::ClickSettings) {
        self.set_click_settings(settings);
    }
    fn cursor(&self) -> aqua_core::CursorIcon {
        UiApp::cursor(self)
    }

    fn on_window_event(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, event: &WindowEvent) {
        let viewport = match event {
            WindowEvent::Resized(_, size) => Some((size.as_core_size(), self.scale())),
            WindowEvent::ScaleChanged(_, scale) => Some((self.viewport(), *scale as f32)),
            _ => None,
        };
        if let Some((size, scale)) = viewport {
            let _ = self.step(Input::Viewport { size, scale });
        }
    }

    fn on_input(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &CoreInputEvent) {
        dispatch_input(self, ctx, id, event);
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

    fn on_a11y_action(&mut self, ctx: &mut AppCtx, id: AquaWindowId, request: &A11yActionRequest) {
        let output = self.step(Input::Access(request.clone()));
        request_redraw(ctx, id, output);
    }

    fn ime_cursor_area(&self) -> Option<aqua_core::Rect> {
        self.ime_anchor()
    }

    #[cfg(windows)]
    fn native_ime_client(
        &self,
    ) -> Option<std::rc::Rc<dyn aqua_platform::ime_windows::NativeImeClient>> {
        Some(std::rc::Rc::new(NativeImeClient(
            self.native_text_service()?,
        )))
    }

    fn chrome_regions(&self, _window: &ObservedWindowState) -> Option<DragRegions> {
        UiApp::chrome_regions(self).cloned()
    }

    fn prepare_accessibility(
        &mut self,
        window: &ObservedWindowState,
        now: Instant,
    ) -> Option<Instant> {
        self.build_scene(window, now)
    }

    fn a11y_nodes(&self) -> Option<(aqua_core::NodeId, aqua_core::NodeId, Vec<A11yNode>)> {
        UiApp::a11y_nodes(self)
    }

    fn build_scene(&mut self, window: &ObservedWindowState, now: Instant) -> Option<Instant> {
        let _ = self.step(Input::Viewport {
            size: window.inner_size.as_core_size(),
            scale: window.scale_factor as f32,
        });
        match self.step(Input::Frame(now)) {
            Ok(output) => output.wake_at,
            Err(e) => {
                eprintln!("aqua: build failed: {e}");
                None
            }
        }
    }

    fn scene_mut(&mut self, _id: AquaWindowId) -> Option<&mut Scene> {
        Some(UiApp::scene_mut(self))
    }

    fn reactive_runtime(&self) -> Option<&ReactiveRuntime> {
        Some(self.runtime())
    }
}

impl Aqua {
    pub fn run_ui(self, factory: impl FnOnce(&Scope) -> Element) -> Result<(), String> {
        let mut app = UiApp::new(self.fonts().clone(), self.runs().clone());
        app.apply_startup_motion_policy(self.motion_reduced());
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

#[cfg(test)]
mod chrome_regions_tests {
    use super::*;
    use aqua_platform::geometry::LogicalSize;
    use aqua_text::{SharedFonts, SharedRuns};

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
        assert!(Application::chrome_regions(&app, &observed()).is_none());
        let regions = DragRegions {
            draggable: vec![(0.0, 0.0, 200.0, 40.0)],
            no_drag: vec![(160.0, 4.0, 196.0, 36.0)],
            ..Default::default()
        };
        app.set_chrome_regions(Some(regions.clone()));
        assert_eq!(
            Application::chrome_regions(&app, &observed()),
            Some(regions)
        );
        app.set_chrome_regions(None);
        assert!(Application::chrome_regions(&app, &observed()).is_none());
    }
}
