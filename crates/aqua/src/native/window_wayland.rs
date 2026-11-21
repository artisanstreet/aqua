use std::cell::OnceCell;
use std::rc::Rc;

use aqua_a11y_atspi::{AtspiAdapter, Connection};
use aqua_core::{ClickSettings, CursorIcon, Rect};
use aqua_portal::taskbar::Launcher;
use aqua_protocol::chrome::DragRegions;
use aqua_protocol::monitor::DisplayInfo;
use aqua_protocol::{A11yActionRequest, AccessTreeUpdate, BadgeError, TaskbarBadge, WindowRequest};
use aqua_shell::AppId;
use aqua_shell_wayland::{Unsupported, Window};
use aqua_transport::Inspector;

use super::platform::shell::prefs;
use crate::native_window::NativeWindow;
use crate::runtime::AccessSink;

pub struct Badge {
    app_id: Option<AppId>,
    launcher: OnceCell<Result<Launcher, BadgeError>>,
}

impl Badge {
    pub fn new(app_id: Option<AppId>) -> Rc<Badge> {
        Rc::new(Badge {
            app_id,
            launcher: OnceCell::new(),
        })
    }

    fn set(&self, badge: Option<TaskbarBadge>) -> Result<(), BadgeError> {
        match self
            .launcher
            .get_or_init(|| Launcher::new(self.app_id.clone()))
        {
            Ok(launcher) => launcher.set_badge(badge),
            Err(error) => Err(error.clone()),
        }
    }
}

pub struct WaylandWindow {
    pub window: Window,
    pub badge: Rc<Badge>,
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

impl NativeWindow for WaylandWindow {
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
            false => report("restore", self.window.restore_minimized()),
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
        report("activate", self.window.activate());
    }

    fn set_outer_position(&self, x: i32, y: i32) {
        report("move", self.window.set_position(x, y));
    }

    fn center(&self) {
        report("center", Err(Unsupported::Position));
    }

    fn set_cursor(&self, cursor: CursorIcon) {
        self.window.set_cursor(cursor);
    }

    fn set_drag_regions(&self, regions: &DragRegions) {
        self.window.set_drag_regions(regions);
    }

    fn click_settings(&self) -> ClickSettings {
        prefs::click_settings()
    }

    fn display(&self) -> Option<DisplayInfo> {
        self.window.monitor().map(|monitor| DisplayInfo {
            name: Some(monitor.name),
            physical_bounds: rect_of(monitor.bounds),
            physical_work_area: rect_of(monitor.work_area),
            scale_factor: monitor.scale,
        })
    }

    fn set_badge(&self, badge: Option<TaskbarBadge>) -> Result<(), BadgeError> {
        self.badge.set(badge)
    }

    fn destroy(&self) {
        self.window.destroy();
    }

    fn unsupported(&self, request: &WindowRequest) -> Option<&'static str> {
        let constrained = || self.window.is_maximized() || self.window.is_fullscreen();
        match request {
            WindowRequest::SetOuterPosition(..) | WindowRequest::Center(_) => {
                Some(Unsupported::Position.reason())
            }
            WindowRequest::SetMinimized(_, false) => Some(Unsupported::Unminimize.reason()),
            WindowRequest::SetInnerSize(..) if constrained() => {
                Some(Unsupported::ConstrainedSize.reason())
            }
            _ => None,
        }
    }
}

pub struct AtspiSink {
    pub adapter: AtspiAdapter,
    pub window: Window,
    pub inspector: Option<Inspector>,
}

impl AccessSink for AtspiSink {
    fn needs_snapshot(&self) -> bool {
        self.inspector.is_some() || self.adapter.connection() == Connection::Connected
    }

    fn connection_epoch(&self) -> u64 {
        self.adapter.epoch()
    }

    fn apply(&mut self, update: &AccessTreeUpdate) {
        let started = std::time::Instant::now();
        let _span = aqua_protocol::probe::span(aqua_protocol::probe::Phase::Build);
        if let Some(inspector) = &self.inspector {
            super::inspector::publish_tree(inspector, update);
        }
        self.adapter.set_window_name(&self.window.title());
        self.adapter.apply(update);
        crate::profile::note(|| {
            format!(
                "access_apply={}us nodes={}",
                started.elapsed().as_micros(),
                update.nodes.len()
            )
        });
    }

    fn drain_actions(&mut self) -> Vec<A11yActionRequest> {
        self.adapter.drain_actions()
    }

    fn window_focus(&mut self, focused: bool) {
        self.adapter.focus(focused);
    }
}
