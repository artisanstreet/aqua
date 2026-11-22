use std::rc::Rc;
use std::time::Duration;

use aqua_core::{ClickSettings, Oklch};
use aqua_graph::Extent;
use aqua_ime_appkit::AppKitIme;
use aqua_protocol::prefs::SystemPrefs;
use aqua_protocol::{AquaWindowId, ObservedWindowState, WindowOptions};
use aqua_shell::AppId;

use super::gpu;
use super::inspector::HostInspector;
use super::looping::Host;
use super::platform::shell::{prefs, Window};
use super::window_appkit::{AppKitWindow, InspectorSink};
use crate::native_window::NativeWindow;
use crate::runtime::Application;

pub(super) struct Desktop {
    seen: (SystemPrefs, ClickSettings),
}

impl Desktop {
    pub(super) fn new(_app_id: Option<AppId>) -> Desktop {
        Desktop {
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
        Rc::new(AppKitWindow { window })
    }

    pub(super) fn attach(
        &mut self,
        id: AquaWindowId,
        window: Window,
        observed: &ObservedWindowState,
        options: &WindowOptions,
    ) {
        prefs::follow_changes(window.waker());
        if let Some(inspector) = self.inspector.as_ref().map(HostInspector::handle) {
            let now = self.aqua.clock();
            if let Some(deadline) = self.app.prepare_accessibility(observed, now) {
                self.aqua.scheduler.request_animation_deadline(id, deadline);
            }
            self.aqua
                .attach_access(id, Box::new(InspectorSink { inspector }));
        }
        self.aqua.attach_ime(id, Box::new(AppKitIme::new(window)));
        super::startup_mark("window_shown");
        if let Err(error) = window.show() {
            eprintln!("aqua: window {} did not show: {error}", id.raw());
            return;
        }
        super::startup_mark("window_visible");
        let Some(lease) = window.layer_lease() else {
            eprintln!("aqua: window {} has no layer to render into", id.raw());
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
