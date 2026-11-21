use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use aqua_a11y_atspi::{AtspiAdapter, WakeFn};
use aqua_core::{ClickSettings, Oklch};
use aqua_graph::Extent;
use aqua_ime_wayland::WaylandIme;
use aqua_protocol::prefs::SystemPrefs;
use aqua_protocol::{AquaWindowId, ObservedWindowState, WindowOptions};
use aqua_shell::AppId;

use super::gpu;
use super::inspector::HostInspector;
use super::looping::Host;
use super::platform::shell::{prefs, Window};
use super::window_wayland::{AtspiSink, Badge, WaylandWindow};
use crate::inbox::AquaUserEvent;
use crate::native_window::NativeWindow;
use crate::runtime::Application;

pub(super) struct Desktop {
    badge: Rc<Badge>,
    seen: (SystemPrefs, ClickSettings),
}

impl Desktop {
    pub(super) fn new(app_id: Option<AppId>) -> Desktop {
        Desktop {
            badge: Badge::new(app_id),
            seen: (prefs::query(), prefs::click_settings()),
        }
    }

    pub(super) fn preferences_changed(&mut self) -> bool {
        let now = (prefs::query(), prefs::click_settings());
        let changed = now != self.seen;
        self.seen = now;
        changed
    }
}

fn physical(window: Window) -> Extent {
    let (width, height) = window.inner_size();
    Extent {
        width: width.max(1),
        height: height.max(1),
    }
}

impl<A: Application + 'static> Host<A> {
    pub(super) fn native_window(&self, window: Window) -> Rc<dyn NativeWindow> {
        Rc::new(WaylandWindow {
            window,
            badge: self.desktop.badge.clone(),
        })
    }

    pub(super) fn attach(
        &mut self,
        id: AquaWindowId,
        window: Window,
        observed: &ObservedWindowState,
        options: &WindowOptions,
    ) {
        prefs::follow_changes(window.waker());
        let inbox = self.aqua.inbox();
        let wake: WakeFn = Arc::new(move || {
            inbox.post(AquaUserEvent::A11yActions(id));
        });
        super::startup_mark("atspi_begin");
        let adapter = AtspiAdapter::attach(wake);
        super::startup_mark("atspi_done");
        let now = self.aqua.clock();
        if let Some(deadline) = self.app.prepare_accessibility(observed, now) {
            self.aqua.scheduler.request_animation_deadline(id, deadline);
        }
        self.aqua.attach_access(
            id,
            Box::new(AtspiSink {
                adapter,
                window,
                inspector: self.inspector.as_ref().map(HostInspector::handle),
            }),
        );
        super::startup_mark("ime_begin");
        match WaylandIme::new(window) {
            Ok(ime) => self.aqua.attach_ime(id, Box::new(ime)),
            Err(error) => eprintln!("aqua: text input unavailable: {error}"),
        }
        super::startup_mark("window_shown");
        if let Err(error) = window.show() {
            eprintln!("aqua: window {} did not map: {error}", id.raw());
            return;
        }
        super::startup_mark("window_visible");
        let Some(lease) = window.surface_lease() else {
            eprintln!("aqua: window {} has no surface to render into", id.raw());
            return;
        };
        let clear = match options.transparent {
            true => Oklch::TRANSPARENT,
            false => Oklch::BLACK,
        };
        let factory = gpu::factory(
            id,
            lease,
            physical(window),
            self.aqua.fonts().clone(),
            self.aqua.runs().clone(),
            clear,
        );
        super::startup_mark("factory_built");
        let interval = window.refresh_interval().unwrap_or(Duration::ZERO);
        self.aqua.attach_render(id, factory, interval);
        super::startup_mark("render_attached");
    }
}
