use std::sync::Arc;
use std::time::Duration;

use aqua_a11y_uia::{UiaAdapter, WakeFn};
use aqua_core::Oklch;
use aqua_graph::Extent;
use aqua_ime_tsf::TsfIme;
use aqua_protocol::{AquaWindowId, ObservedWindowState, WindowOptions};

use super::gpu;
use super::hook::UiaHook;
use super::inspector::HostInspector;
use super::looping::Host;
use super::platform::shell::{self, Window};
use super::window::UiaSink;
use crate::inbox::AquaUserEvent;
use crate::runtime::Application;

fn physical(window: Window) -> Extent {
    let (width, height) = window.inner_size();
    Extent {
        width: width.max(1),
        height: height.max(1),
    }
}

impl<A: Application + 'static> Host<A> {
    pub(super) fn attach(
        &mut self,
        id: AquaWindowId,
        window: Window,
        observed: &ObservedWindowState,
        options: &WindowOptions,
    ) {
        let inbox = self.aqua.inbox();
        let wake: WakeFn = Arc::new(move || {
            inbox.post(AquaUserEvent::A11yActions(id));
        });
        super::startup_mark("uia_begin");
        let attached = UiaAdapter::attach_routed(id.raw() as isize, wake);
        super::startup_mark("uia_done");
        match attached {
            Ok((adapter, router)) => {
                shell::add_message_hook(UiaHook::new(id.raw() as isize, router));
                let now = self.aqua.clock();
                if let Some(deadline) = self.app.prepare_accessibility(observed, now) {
                    self.aqua.scheduler.request_animation_deadline(id, deadline);
                }
                self.aqua.attach_access(
                    id,
                    Box::new(UiaSink {
                        adapter,
                        inspector: self.inspector.as_ref().map(HostInspector::handle),
                    }),
                );
            }
            Err(error) => eprintln!("aqua: uia attach failed: {error:?}"),
        }
        super::startup_mark("ime_begin");
        self.aqua
            .attach_ime(id, Box::new(TsfIme::from_hwnd(id.raw() as isize)));
        super::startup_mark("window_shown");
        window.show();
        super::startup_mark("window_visible");
        let clear = match options.transparent {
            true => Oklch::TRANSPARENT,
            false => Oklch::BLACK,
        };
        let factory = gpu::factory(
            id,
            id.raw() as isize,
            physical(window),
            self.aqua.fonts().clone(),
            self.aqua.runs().clone(),
            clear,
        );
        super::startup_mark("factory_built");
        let interval = gpu::refresh_of(id.raw() as isize)
            .map(Duration::from_nanos)
            .unwrap_or(Duration::ZERO);
        self.aqua.attach_render(id, factory, interval);
        super::startup_mark("render_attached");
    }
}
