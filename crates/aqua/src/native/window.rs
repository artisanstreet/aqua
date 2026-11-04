use aqua_a11y_uia::{Connection, UiaAdapter};
use aqua_core::{ClickSettings, CursorIcon, Rect};
use aqua_platform::chrome::DragRegions;
use aqua_platform::monitor::DisplayInfo;
use aqua_protocol::{A11yActionRequest, AccessTreeUpdate};
use aqua_shell_win32::Window;

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

    fn destroy(&self) {
        self.window.destroy();
    }
}

pub struct UiaSink(pub UiaAdapter);

impl AccessSink for UiaSink {
    fn needs_snapshot(&self) -> bool {
        self.0.connection() == Connection::Connected
    }

    fn apply(&mut self, update: &AccessTreeUpdate) {
        self.0.apply(update);
    }

    fn drain_actions(&mut self) -> Vec<A11yActionRequest> {
        self.0.drain_actions()
    }
}
