use super::client::{millis, Client};
use super::compositor::{rect_of, Compositor};
use super::HarnessError;
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::wl_pointer::{Axis, AxisSource, ButtonState};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

pub(crate) struct Quiet;

impl Dispatch<WlRegistry, GlobalListContents> for Quiet {
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

delegate_noop!(Quiet: ZwlrVirtualPointerManagerV1);
delegate_noop!(Quiet: ZwlrVirtualPointerV1);

pub struct VirtualPointer {
    client: Client<Quiet>,
    device: ZwlrVirtualPointerV1,
    extent: (u32, u32),
    started: std::time::Instant,
}

impl VirtualPointer {
    pub const LEFT: u32 = 0x110;
    pub const RIGHT: u32 = 0x111;
    pub const MIDDLE: u32 = 0x112;

    pub fn new(compositor: &Compositor) -> Result<Self, HarnessError> {
        let client = Client::connect(compositor.display(), "virtual pointer", Quiet)?;
        let manager: ZwlrVirtualPointerManagerV1 = client.bind(2, ())?;
        let device = manager.create_virtual_pointer(None, &client.handle(), ());
        let extent = compositor
            .outputs()
            .ok()
            .and_then(|outputs| outputs.as_array().and_then(|all| all.first().cloned()))
            .and_then(|output| rect_of(&output["rect"]))
            .map_or((1280, 800), |rect| (rect.width as u32, rect.height as u32));
        client.flush()?;
        Ok(Self {
            client,
            device,
            extent,
            started: std::time::Instant::now(),
        })
    }

    fn time(&self) -> u32 {
        millis(self.started)
    }

    pub fn move_to(&self, x: u32, y: u32) -> Result<(), HarnessError> {
        self.device
            .motion_absolute(self.time(), x, y, self.extent.0, self.extent.1);
        self.device.frame();
        self.client.flush()
    }

    pub fn press(&self, button: u32) -> Result<(), HarnessError> {
        self.device
            .button(self.time(), button, ButtonState::Pressed);
        self.device.frame();
        self.client.flush()
    }

    pub fn release(&self, button: u32) -> Result<(), HarnessError> {
        self.device
            .button(self.time(), button, ButtonState::Released);
        self.device.frame();
        self.client.flush()
    }

    pub fn wheel(&self, vertical: bool, notches: i32) -> Result<(), HarnessError> {
        let axis = match vertical {
            true => Axis::VerticalScroll,
            false => Axis::HorizontalScroll,
        };
        self.device.axis_source(AxisSource::Wheel);
        self.device
            .axis_discrete(self.time(), axis, f64::from(notches) * 15.0, notches);
        self.device.frame();
        self.client.flush()
    }
}

impl Drop for VirtualPointer {
    fn drop(&mut self) {
        self.device.destroy();
        let _ = self.client.flush();
    }
}
