use aqua_a11y_uia::{Connection, UiaAdapter};
use aqua_core::{ClickSettings, CursorIcon, Rect};
use aqua_protocol::chrome::DragRegions;
use aqua_protocol::monitor::DisplayInfo;
use aqua_protocol::{A11yActionRequest, AccessTreeUpdate, BadgeError, TaskbarBadge};
use aqua_shell_win32::Window;
use aqua_transport::Inspector;

use crate::native_window::NativeWindow;
use crate::runtime::AccessSink;

pub struct Win32Window {
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

impl NativeWindow for Win32Window {
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
        self.window.set_logical_size(width, height);
    }

    fn set_visible(&self, visible: bool) {
        match visible {
            true => self.window.show(),
            false => self.window.hide(),
        }
    }

    fn set_minimized(&self, minimized: bool) {
        match minimized {
            true => self.window.minimize(),
            false => self.window.show(),
        }
    }

    fn set_maximized(&self, maximized: bool) {
        if self.window.is_maximized() != maximized {
            self.window.toggle_maximize();
        }
    }

    fn is_maximized(&self) -> bool {
        self.window.is_maximized()
    }

    fn activate(&self) {
        self.window.show();
    }

    fn set_outer_position(&self, x: i32, y: i32) {
        self.window.set_position(x, y);
    }

    fn center(&self) {
        let (left, top, right, bottom) = self.window.outer_rect();
        if let Some(monitor) = self.window.monitor() {
            let (wl, wt, wr, wb) = monitor.work_area;
            let x = wl + ((wr - wl) - (right - left)) / 2;
            let y = wt + ((wb - wt) - (bottom - top)) / 2;
            self.window.set_position(x, y);
        }
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
            name: None,
            physical_bounds: rect_of(monitor.bounds),
            physical_work_area: rect_of(monitor.work_area),
            scale_factor: monitor.scale,
        })
    }

    fn set_badge(&self, badge: Option<TaskbarBadge>) -> Result<(), BadgeError> {
        aqua_shell_win32::taskbar::set_badge(&self.window.native_handle(), badge)
    }

    fn destroy(&self) {
        self.window.destroy();
    }

    fn attach_composition(&self, content: &dyn std::any::Any) -> bool {
        content
            .downcast_ref::<super::d3d12::CompositionContent>()
            .is_some_and(|content| self.window.attach_composition_surface(&content.0).is_ok())
    }
}

pub struct UiaSink {
    pub adapter: UiaAdapter,
    pub inspector: Option<Inspector>,
}

impl AccessSink for UiaSink {
    fn needs_snapshot(&self) -> bool {
        self.inspector.is_some() || self.adapter.connection() == Connection::Connected
    }

    fn connection_epoch(&self) -> u64 {
        u64::from(self.adapter.connection() == Connection::Connected)
    }

    fn apply(&mut self, update: &AccessTreeUpdate) {
        if let Some(inspector) = &self.inspector {
            super::inspector::publish_tree(inspector, update);
        }
        self.adapter.apply(update);
    }

    fn drain_actions(&mut self) -> Vec<A11yActionRequest> {
        self.adapter.drain_actions()
    }
}
