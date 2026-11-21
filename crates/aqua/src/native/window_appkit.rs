use aqua_core::{ClickSettings, CursorIcon, Rect};
use aqua_protocol::chrome::DragRegions;
use aqua_protocol::monitor::DisplayInfo;
use aqua_protocol::{A11yActionRequest, AccessTreeUpdate, BadgeError, TaskbarBadge, WindowRequest};
use aqua_shell_appkit::{Unsupported, Window};
use aqua_transport::Inspector;

use crate::native_window::NativeWindow;
use crate::runtime::AccessSink;

pub struct AppKitWindow {
    pub window: Window,
}

fn rect_of((left, top, right, bottom): (i32, i32, i32, i32)) -> Rect {
    Rect {
        x: left as f32,
        y: top as f32,
        w: (right - left) as f32,
        h: (bottom - top) as f32,
    }
}

fn report(operation: &str, result: Result<(), Unsupported>) {
    if let Err(error) = result {
        eprintln!("aqua: {operation}: {error}");
    }
}

impl NativeWindow for AppKitWindow {
    fn scale(&self) -> f64 {
        self.window.scale()
    }

    fn inner_size(&self) -> (u32, u32) {
        self.window.inner_size()
    }

    fn set_title(&self, title: &str) {
        self.window.set_title(title);
    }

    fn set_logical_size(&self, width: f64, height: f64) {
        report("resize", self.window.set_logical_size(width, height));
    }

    fn set_visible(&self, visible: bool) {
        match visible {
            true => {
                if let Err(error) = self.window.show() {
                    eprintln!("aqua: show: {error}");
                }
            }
            false => self.window.hide(),
        }
    }

    fn set_minimized(&self, minimized: bool) {
        match minimized {
            true => report("minimize", self.window.minimize()),
            false => self.window.restore_minimized(),
        }
    }

    fn set_maximized(&self, maximized: bool) {
        if self.window.is_maximized() != maximized {
            report("maximize", self.window.toggle_maximize());
        }
    }

    fn is_maximized(&self) -> bool {
        self.window.is_maximized()
    }

    fn activate(&self) {
        report("activate", self.window.activate());
    }

    fn set_outer_position(&self, x: i32, y: i32) {
        self.window.set_position(x, y);
    }

    fn center(&self) {
        self.window.center();
    }

    fn set_cursor(&self, cursor: CursorIcon) {
        self.window.set_cursor(cursor);
    }

    fn set_drag_regions(&self, regions: &DragRegions) {
        self.window.set_drag_regions(regions);
    }

    fn click_settings(&self) -> ClickSettings {
        self.window.click_settings()
    }

    fn display(&self) -> Option<DisplayInfo> {
        self.window.monitor().map(|monitor| DisplayInfo {
            name: Some(monitor.name),
            physical_bounds: rect_of(monitor.bounds),
            physical_work_area: rect_of(monitor.work_area),
            scale_factor: monitor.scale,
        })
    }

    fn set_badge(&self, _badge: Option<TaskbarBadge>) -> Result<(), BadgeError> {
        Err(BadgeError::Unsupported)
    }

    fn destroy(&self) {
        self.window.destroy();
    }

    fn unsupported(&self, request: &WindowRequest) -> Option<&'static str> {
        match request {
            WindowRequest::SetInnerSize(..) if self.window.is_fullscreen() => {
                Some(Unsupported::FullscreenSize.reason())
            }
            _ => None,
        }
    }
}

pub struct InspectorSink {
    pub inspector: Inspector,
}

impl AccessSink for InspectorSink {
    fn needs_snapshot(&self) -> bool {
        true
    }

    fn apply(&mut self, update: &AccessTreeUpdate) {
        super::inspector::publish_tree(&self.inspector, update);
    }

    fn drain_actions(&mut self) -> Vec<A11yActionRequest> {
        Vec::new()
    }
}
