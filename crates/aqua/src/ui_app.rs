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
            redraw = cut && app.delete_selected_text();
        }
        return Ok(ClipboardOutcome::Handled { redraw });
    }
    if paste {
        let redraw = clipboard
            .get_text()?
            .is_some_and(|text| app.paste_text(&text));
        return Ok(ClipboardOutcome::Handled { redraw });
    }
    Ok(ClipboardOutcome::Unhandled)
}

fn request_redraw(ctx: &mut AppCtx, id: AquaWindowId, output: Result<Output, UiAppError>) {
    if let Ok(output) = output {
        if !output.redraw.is_empty() {
            ctx.request_frame(id, output.redraw);
        }
    }
}

impl Application for UiApp {
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
        let _ = self.step(Input::Tick(ctx.now()));
        match dispatch_clipboard(self, ctx.clipboard(), event) {
            Ok(ClipboardOutcome::Handled { redraw }) => {
                if redraw {
                    ctx.request_frame(id, aqua_protocol::FrameReason::INPUT);
                }
                return;
            }
            Err(error) => {
                eprintln!("aqua: {error}");
                return;
            }
            Ok(ClipboardOutcome::Unhandled) => {}
        }
        let output = self.step(Input::Event(event.clone()));
        request_redraw(ctx, id, output);
    }

    fn on_a11y_action(&mut self, ctx: &mut AppCtx, id: AquaWindowId, request: &A11yActionRequest) {
        let output = self.step(Input::Access(request.clone()));
        request_redraw(ctx, id, output);
    }

    fn ime_cursor_area(&self) -> Option<aqua_core::Rect> {
        self.ime_anchor()
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
