use super::client::{failed, Client};
use super::compositor::Compositor;
use super::peer::{write_pipe, Peer, Serve};
use super::HarnessError;
use rustix::fs::{memfd_create, MemfdFlags};
use std::io::Write;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_data_device::WlDataDevice;
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::WlDataOffer;
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_pointer::{self, ButtonState, WlPointer};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_shm::{Format, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{
    delegate_noop, event_created_child, Connection, Dispatch, Proxy, QueueHandle, WEnum,
};
use wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::client::xdg_toplevel::{self, XdgToplevel};
use wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DragReport {
    pub mapped: bool,
    pub size: (i32, i32),
    pub hovered: bool,
    pub started: bool,
    pub accepted: Option<String>,
    pub dropped: bool,
    pub finished: bool,
    pub cancelled: bool,
}

pub struct Source {
    app_id: String,
    uris: Arc<Vec<u8>>,
    report: Arc<Mutex<DragReport>>,
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    manager: Option<WlDataDeviceManager>,
    device: Option<WlDataDevice>,
    surface: Option<WlSurface>,
    roles: Option<(XdgSurface, XdgToplevel)>,
    size: (i32, i32),
    dragging: Option<WlDataSource>,
}

pub struct DragSource {
    _peer: Peer<()>,
    report: Arc<Mutex<DragReport>>,
}

impl Source {
    fn update(&self, change: impl FnOnce(&mut DragReport)) {
        change(&mut self.report.lock().expect("drag reports are never poisoned"));
    }

    fn paint(&mut self, qh: &QueueHandle<Self>) {
        let (Some(shm), Some(surface)) = (&self.shm, &self.surface) else {
            return;
        };
        let (width, height) = self.size;
        let Ok(fd) = memfd_create("aqua-test-drag-source", MemfdFlags::CLOEXEC) else {
            return;
        };
        let mut file = std::fs::File::from(fd);
        let bytes = vec![0xffu8; (width * height * 4) as usize];
        if file.write_all(&bytes).is_err() {
            return;
        }
        let pool = shm.create_pool(file.as_fd(), width * height * 4, qh, ());
        let buffer = pool.create_buffer(0, width, height, width * 4, Format::Xrgb8888, qh, ());
        pool.destroy();
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width, height);
        surface.commit();
        self.update(|report| {
            report.mapped = true;
            report.size = (width, height);
        });
    }

    fn start_drag(&mut self, serial: u32, qh: &QueueHandle<Self>) {
        let (Some(manager), Some(device), Some(surface)) =
            (&self.manager, &self.device, &self.surface)
        else {
            return;
        };
        let source = manager.create_data_source(qh, ());
        source.offer("text/uri-list".into());
        if source.version() >= 3 {
            source.set_actions(DndAction::Copy);
        }
        device.start_drag(Some(&source), surface, None, serial);
        self.dragging = Some(source);
        self.update(|report| report.started = true);
    }
}

impl Serve for Source {
    type Command = ();

    fn start(client: &mut Client<Self>) -> Result<(), HarnessError> {
        let qh = client.handle();
        let compositor: WlCompositor = client.bind(4, ())?;
        let shm: WlShm = client.bind(1, ())?;
        let wm_base: XdgWmBase = client.bind(1, ())?;
        let seat: WlSeat = client.bind(5, ())?;
        let manager: WlDataDeviceManager = client.bind(3, ())?;
        seat.get_pointer(&qh, ());
        let device = manager.get_data_device(&seat, &qh, ());
        let surface = compositor.create_surface(&qh, ());
        let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg.get_toplevel(&qh, ());
        toplevel.set_app_id(client.state.app_id.clone());
        toplevel.set_title(client.state.app_id.clone());
        surface.commit();
        let state = &mut client.state;
        state.compositor = Some(compositor);
        state.shm = Some(shm);
        state.manager = Some(manager);
        state.device = Some(device);
        state.surface = Some(surface);
        state.roles = Some((xdg, toplevel));
        client.roundtrip()
    }

    fn command(_: &mut Client<Self>, _: ()) {}
}

impl DragSource {
    pub fn start(
        compositor: &Compositor,
        app_id: &str,
        paths: &[PathBuf],
    ) -> Result<Self, HarnessError> {
        let report = Arc::new(Mutex::new(DragReport::default()));
        let uris = crate::formats::uri_list(paths).into_bytes();
        if uris.is_empty() {
            return Err(failed("drag source", "no path became a file URI"));
        }
        let source = Source {
            app_id: app_id.to_owned(),
            uris: Arc::new(uris),
            report: report.clone(),
            compositor: None,
            shm: None,
            manager: None,
            device: None,
            surface: None,
            roles: None,
            size: (240, 240),
            dragging: None,
        };
        let peer = Peer::spawn(compositor.display().to_owned(), "drag source", source)?;
        Ok(Self {
            _peer: peer,
            report,
        })
    }

    pub fn report(&self) -> DragReport {
        self.report
            .lock()
            .expect("drag reports are never poisoned")
            .clone()
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for Source {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgWmBase, ()> for Source {
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

impl Dispatch<XdgSurface, ()> for Source {
    fn event(
        state: &mut Self,
        xdg: &XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            state.paint(qh);
        }
    }
}

impl Dispatch<XdgToplevel, ()> for Source {
    fn event(
        state: &mut Self,
        _: &XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure { width, height, .. } = event
            && width > 0
            && height > 0
        {
            state.size = (width, height);
        }
    }
}

impl Dispatch<WlPointer, ()> for Source {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { .. } => state.update(|report| report.hovered = true),
            wl_pointer::Event::Leave { .. } => state.update(|report| report.hovered = false),
            wl_pointer::Event::Button {
                serial,
                state: WEnum::Value(ButtonState::Pressed),
                ..
            } if state.dragging.is_none() => state.start_drag(serial, qh),
            _ => {}
        }
    }
}

impl Dispatch<WlDataSource, ()> for Source {
    fn event(
        state: &mut Self,
        _: &WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Target { mime_type } => {
                state.update(|report| report.accepted = mime_type)
            }
            wl_data_source::Event::Send { fd, .. } => write_pipe(fd, state.uris.to_vec()),
            wl_data_source::Event::DndDropPerformed => state.update(|report| report.dropped = true),
            wl_data_source::Event::DndFinished => state.update(|report| report.finished = true),
            wl_data_source::Event::Cancelled => state.update(|report| report.cancelled = true),
            _ => {}
        }
    }
}

impl Dispatch<WlDataDevice, ()> for Source {
    fn event(
        _: &mut Self,
        _: &WlDataDevice,
        _: wayland_client::protocol::wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    event_created_child!(Source, WlDataDevice, [
        wayland_client::protocol::wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
    ]);
}

delegate_noop!(Source: WlCompositor);
delegate_noop!(Source: ignore WlShm);
delegate_noop!(Source: WlShmPool);
delegate_noop!(Source: ignore WlBuffer);
delegate_noop!(Source: ignore WlSurface);
delegate_noop!(Source: ignore WlSeat);
delegate_noop!(Source: WlDataDeviceManager);
delegate_noop!(Source: ignore WlDataOffer);
