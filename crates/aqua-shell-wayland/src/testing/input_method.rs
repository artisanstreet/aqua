use super::client::{failed, Client};
use super::compositor::Compositor;
use super::HarnessError;
use rustix::event::{poll, PollFd, PollFlags};
use rustix::fs::{memfd_create, MemfdFlags};
use std::io::Write;
use std::os::fd::AsFd;
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_shm::{Format, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle};
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::{
    self, ZwpInputMethodV2,
};
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_popup_surface_v2::{
    self, ZwpInputPopupSurfaceV2,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observed {
    pub active: bool,
    pub activations: u32,
    pub deactivations: u32,
    pub surrounding: Option<(String, u32, u32)>,
    pub rectangle: Option<(i32, i32, i32, i32)>,
    pub unavailable: bool,
}

#[derive(Default)]
pub struct Method {
    pending_active: Option<bool>,
    pending_surrounding: Option<(String, u32, u32)>,
    dones: u32,
    observed: Observed,
}

pub const POPUP: i32 = 4;
pub const POPUP_BGRA: [u8; 3] = [0x00, 0x00, 0xff];

pub struct InputMethod {
    client: Client<Method>,
    method: ZwpInputMethodV2,
    _popup: (WlSurface, ZwpInputPopupSurfaceV2),
}

#[derive(Debug, Clone, Default)]
pub struct Edit<'a> {
    pub preedit: Option<(&'a str, i32, i32)>,
    pub commit: Option<&'a str>,
    pub delete: Option<(u32, u32)>,
}

fn pixel(client: &Client<Method>, surface: &WlSurface) -> Result<(), HarnessError> {
    let shm: WlShm = client.bind(1, ())?;
    let fd = memfd_create("aqua-test-popup", MemfdFlags::CLOEXEC)
        .map_err(|error| failed(client.name, error))?;
    let mut file = std::fs::File::from(fd);
    let pixels = [0x00, 0x00, 0xff, 0xff].repeat((POPUP * POPUP) as usize);
    file.write_all(&pixels)
        .map_err(|error| failed(client.name, error))?;
    let pool = shm.create_pool(file.as_fd(), POPUP * POPUP * 4, &client.handle(), ());
    let buffer = pool.create_buffer(
        0,
        POPUP,
        POPUP,
        POPUP * 4,
        Format::Xrgb8888,
        &client.handle(),
        (),
    );
    pool.destroy();
    surface.attach(Some(&buffer), 0, 0);
    surface.commit();
    Ok(())
}

impl InputMethod {
    pub fn new(compositor: &Compositor) -> Result<Self, HarnessError> {
        let mut client = Client::connect(compositor.display(), "input method", Method::default())?;
        let seat: WlSeat = client.bind(1, ())?;
        let manager: ZwpInputMethodManagerV2 = client.bind(1, ())?;
        let surfaces: WlCompositor = client.bind(4, ())?;
        let method = manager.get_input_method(&seat, &client.handle(), ());
        let surface = surfaces.create_surface(&client.handle(), ());
        let popup = method.get_input_popup_surface(&surface, &client.handle(), ());
        pixel(&client, &surface)?;
        client.roundtrip()?;
        if client.state.observed.unavailable {
            return Err(failed(
                "input method",
                "the seat already has an input method",
            ));
        }
        Ok(Self {
            client,
            method,
            _popup: (surface, popup),
        })
    }

    pub fn poll(&mut self) -> Result<&Observed, HarnessError> {
        self.client.flush()?;
        if let Some(guard) = self.client.queue.prepare_read() {
            let mut fds = [PollFd::from_borrowed_fd(
                guard.connection_fd(),
                PollFlags::IN,
            )];
            let ready = poll(&mut fds, Some(&rustix::time::Timespec::default()))
                .map_err(|error| failed(self.client.name, error))?;
            if ready > 0 {
                guard
                    .read()
                    .map_err(|error| failed(self.client.name, error))?;
            }
        }
        self.client
            .queue
            .dispatch_pending(&mut self.client.state)
            .map_err(|error| failed(self.client.name, error))?;
        Ok(&self.client.state.observed)
    }

    pub fn observed(&self) -> &Observed {
        &self.client.state.observed
    }

    pub fn send(&mut self, edit: Edit<'_>) -> Result<(), HarnessError> {
        self.poll()?;
        if let Some((before, after)) = edit.delete {
            self.method.delete_surrounding_text(before, after);
        }
        if let Some(text) = edit.commit {
            self.method.commit_string(text.to_owned());
        }
        if let Some((text, begin, end)) = edit.preedit {
            self.method.set_preedit_string(text.to_owned(), begin, end);
        }
        self.method.commit(self.client.state.dones);
        self.client.flush()
    }
}

impl Drop for InputMethod {
    fn drop(&mut self) {
        self.method.destroy();
        let _ = self.client.flush();
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for Method {
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

impl Dispatch<ZwpInputMethodV2, ()> for Method {
    fn event(
        state: &mut Self,
        _: &ZwpInputMethodV2,
        event: zwp_input_method_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_v2::Event::Activate => state.pending_active = Some(true),
            zwp_input_method_v2::Event::Deactivate => state.pending_active = Some(false),
            zwp_input_method_v2::Event::SurroundingText {
                text,
                cursor,
                anchor,
            } => state.pending_surrounding = Some((text, cursor, anchor)),
            zwp_input_method_v2::Event::Done => {
                state.dones = state.dones.wrapping_add(1);
                let observed = &mut state.observed;
                match state.pending_active.take() {
                    Some(true) => {
                        observed.active = true;
                        observed.activations += 1;
                    }
                    Some(false) => {
                        observed.active = false;
                        observed.deactivations += 1;
                        observed.surrounding = None;
                    }
                    None => {}
                }
                if let Some(surrounding) = state.pending_surrounding.take() {
                    observed.surrounding = Some(surrounding);
                }
            }
            zwp_input_method_v2::Event::Unavailable => state.observed.unavailable = true,
            _ => {}
        }
    }
}

impl Dispatch<ZwpInputPopupSurfaceV2, ()> for Method {
    fn event(
        state: &mut Self,
        _: &ZwpInputPopupSurfaceV2,
        event: zwp_input_popup_surface_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_input_popup_surface_v2::Event::TextInputRectangle {
            x,
            y,
            width,
            height,
        } = event
        {
            state.observed.rectangle = Some((x, y, width, height));
        }
    }
}

delegate_noop!(Method: ignore WlSeat);
delegate_noop!(Method: ZwpInputMethodManagerV2);
delegate_noop!(Method: WlCompositor);
delegate_noop!(Method: ignore WlSurface);
delegate_noop!(Method: ignore WlShm);
delegate_noop!(Method: WlShmPool);
delegate_noop!(Method: ignore WlBuffer);
