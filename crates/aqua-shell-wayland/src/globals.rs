use crate::output::Output;
use crate::state::Shell;
use std::sync::atomic::{AtomicU32, Ordering};
use wayland_client::protocol::{
    wl_compositor::WlCompositor, wl_output::WlOutput, wl_registry, wl_seat::WlSeat, wl_shm::WlShm,
};
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_client::protocol::wl_data_device_manager::WlDataDeviceManager;
use wayland_protocols::wp::presentation_time::client::wp_presentation::{self, WpPresentation};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;
use wayland_protocols::xdg::decoration::zv1::client::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1;
use wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1;

pub(crate) static PRESENTATION_CLOCK: AtomicU32 = AtomicU32::new(u32::MAX);
pub(crate) const CLOCK_MONOTONIC: u32 = 1;

pub(crate) struct Globals {
    pub compositor: Option<WlCompositor>,
    pub shm: Option<WlShm>,
    pub wm_base: Option<XdgWmBase>,
    pub decoration: Option<ZxdgDecorationManagerV1>,
    pub viewporter: Option<WpViewporter>,
    pub fractional: Option<WpFractionalScaleManagerV1>,
    pub cursor_shape: Option<WpCursorShapeManagerV1>,
    pub activation: Option<XdgActivationV1>,
    pub xdg_output: Option<ZxdgOutputManagerV1>,
    pub presentation: Option<WpPresentation>,
    pub data_device: Option<WlDataDeviceManager>,
    pub primary_selection: Option<ZwpPrimarySelectionDeviceManagerV1>,
    pub seat: Option<(u32, WlSeat)>,
    pub hidden: Vec<String>,
}

impl Globals {
    pub(crate) fn new(hidden: Vec<String>) -> Self {
        Self {
            compositor: None,
            shm: None,
            wm_base: None,
            decoration: None,
            viewporter: None,
            fractional: None,
            cursor_shape: None,
            activation: None,
            xdg_output: None,
            presentation: None,
            data_device: None,
            primary_selection: None,
            seat: None,
            hidden,
        }
    }

    pub(crate) fn missing(&self) -> Option<&'static str> {
        match (&self.compositor, &self.wm_base, &self.shm) {
            (None, _, _) => Some("wl_compositor"),
            (_, None, _) => Some("xdg_wm_base"),
            (_, _, None) => Some("wl_shm"),
            _ => None,
        }
    }
}

impl Shell {
    fn attach_seat_devices(&mut self, qh: &QueueHandle<Self>) {
        let Some((_, seat)) = &self.globals.seat else {
            return;
        };
        self.transfer.attach(&self.globals, seat, qh);
    }

    fn global(
        &mut self,
        registry: &wl_registry::WlRegistry,
        name: u32,
        interface: &str,
        version: u32,
        qh: &QueueHandle<Self>,
    ) {
        if self.globals.hidden.iter().any(|hidden| hidden == interface) {
            return;
        }
        let globals = &mut self.globals;
        match interface {
            "wl_compositor" => {
                globals.compositor = Some(registry.bind(name, version.min(6), qh, ()));
            }
            "wl_shm" => globals.shm = Some(registry.bind(name, 1, qh, ())),
            "xdg_wm_base" => globals.wm_base = Some(registry.bind(name, version.min(6), qh, ())),
            "zxdg_decoration_manager_v1" => {
                globals.decoration = Some(registry.bind(name, 1, qh, ()));
            }
            "wp_viewporter" => globals.viewporter = Some(registry.bind(name, 1, qh, ())),
            "wp_fractional_scale_manager_v1" => {
                globals.fractional = Some(registry.bind(name, 1, qh, ()));
            }
            "wp_cursor_shape_manager_v1" => {
                globals.cursor_shape = Some(registry.bind(name, 1, qh, ()));
            }
            "xdg_activation_v1" => globals.activation = Some(registry.bind(name, 1, qh, ())),
            "wp_presentation" => globals.presentation = Some(registry.bind(name, 1, qh, ())),
            "zxdg_output_manager_v1" => {
                let manager: ZxdgOutputManagerV1 = registry.bind(name, version.min(3), qh, ());
                self.outputs
                    .iter_mut()
                    .for_each(|output| output.describe(&manager, qh));
                self.globals.xdg_output = Some(manager);
            }
            "wl_seat" if globals.seat.is_none() => {
                let seat: WlSeat = registry.bind(name, version.min(9), qh, ());
                globals.seat = Some((name, seat));
                self.attach_seat_devices(qh);
            }
            "wl_data_device_manager" => {
                globals.data_device = Some(registry.bind(name, version.min(3), qh, ()));
                self.attach_seat_devices(qh);
            }
            "zwp_primary_selection_device_manager_v1" => {
                globals.primary_selection = Some(registry.bind(name, 1, qh, ()));
                self.attach_seat_devices(qh);
            }
            "wl_output" => {
                let proxy: WlOutput = registry.bind(name, version.min(4), qh, name);
                let mut output = Output::new(name, proxy);
                if let Some(manager) = &self.globals.xdg_output {
                    output.describe(manager, qh);
                }
                self.outputs.push(output);
            }
            _ => {}
        }
    }

    fn global_removed(&mut self, name: u32) {
        if self
            .globals
            .seat
            .as_ref()
            .is_some_and(|(seat, _)| *seat == name)
        {
            self.globals.seat = None;
            self.seat.release();
            self.transfer.release();
        }
        if self.outputs.iter().any(|output| output.name == name) {
            self.outputs.retain(|output| output.name != name);
            let ids: Vec<_> = self.windows.keys().copied().collect();
            for id in ids {
                crate::output::left(self, id, name);
            }
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for Shell {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => state.global(registry, name, &interface, version, qh),
            wl_registry::Event::GlobalRemove { name } => state.global_removed(name),
            _ => {}
        }
    }
}

impl Dispatch<XdgWmBase, ()> for Shell {
    fn event(
        _: &mut Self,
        wm_base: &XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<WpPresentation, ()> for Shell {
    fn event(
        _: &mut Self,
        _: &WpPresentation,
        event: wp_presentation::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            PRESENTATION_CLOCK.store(clk_id, Ordering::Release);
        }
    }
}

delegate_noop!(Shell: WlCompositor);
delegate_noop!(Shell: ignore WlShm);
delegate_noop!(Shell: ZxdgDecorationManagerV1);
delegate_noop!(Shell: WpViewporter);
delegate_noop!(Shell: WpFractionalScaleManagerV1);
delegate_noop!(Shell: WpCursorShapeManagerV1);
delegate_noop!(Shell: XdgActivationV1);
delegate_noop!(Shell: WlDataDeviceManager);
delegate_noop!(Shell: ZwpPrimarySelectionDeviceManagerV1);
delegate_noop!(Shell: ZxdgOutputManagerV1);
delegate_noop!(Shell: ignore wayland_client::protocol::wl_surface::WlSurface);
