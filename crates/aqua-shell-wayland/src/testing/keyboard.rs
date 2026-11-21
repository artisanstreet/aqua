use super::client::{failed, millis, Client};
use super::compositor::Compositor;
use super::pointer::Quiet;
use super::HarnessError;
use rustix::fs::{memfd_create, MemfdFlags};
use std::io::Write;
use std::os::fd::AsFd;
use wayland_client::delegate_noop;
use wayland_client::protocol::wl_keyboard::KeyState;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;
use xkbcommon::xkb;

delegate_noop!(Quiet: ignore WlSeat);
delegate_noop!(Quiet: ZwpVirtualKeyboardManagerV1);
delegate_noop!(Quiet: ZwpVirtualKeyboardV1);

pub struct VirtualKeyboard {
    client: Client<Quiet>,
    device: ZwpVirtualKeyboardV1,
    state: xkb::State,
    started: std::time::Instant,
}

impl VirtualKeyboard {
    pub const KEY_2: u32 = 3;
    pub const KEY_A: u32 = 30;
    pub const KEY_C: u32 = 46;
    pub const KEY_U: u32 = 22;
    pub const KEY_RIGHTBRACE: u32 = 27;
    pub const KEY_LEFTCTRL: u32 = 29;
    pub const KEY_RIGHTALT: u32 = 100;

    pub fn with_layout(
        compositor: &Compositor,
        layout: &str,
        variant: &str,
    ) -> Result<Self, HarnessError> {
        let name = "virtual keyboard";
        let mut client = Client::connect(compositor.display(), name, Quiet)?;
        let seat: WlSeat = client.bind(1, ())?;
        let manager: ZwpVirtualKeyboardManagerV1 = client.bind(1, ())?;
        let device = manager.create_virtual_keyboard(&seat, &client.handle(), ());
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(
            &context,
            "evdev",
            "pc105",
            layout,
            variant,
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or_else(|| failed(name, format!("no xkb keymap for layout {layout:?}")))?;
        let mut text = keymap
            .get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1)
            .into_bytes();
        text.push(0);
        let fd = memfd_create("aqua-test-keymap", MemfdFlags::CLOEXEC)
            .map_err(|error| failed(name, error))?;
        let mut file = std::fs::File::from(fd);
        file.write_all(&text).map_err(|error| failed(name, error))?;
        let size = u32::try_from(text.len()).map_err(|error| failed(name, error))?;
        device.keymap(xkb::KEYMAP_FORMAT_TEXT_V1, file.as_fd(), size);
        client.roundtrip()?;
        Ok(Self {
            client,
            device,
            state: xkb::State::new(&keymap),
            started: std::time::Instant::now(),
        })
    }

    fn send(&mut self, key: u32, pressed: bool) -> Result<(), HarnessError> {
        let direction = match pressed {
            true => xkb::KeyDirection::Down,
            false => xkb::KeyDirection::Up,
        };
        self.state.update_key(xkb::Keycode::new(key + 8), direction);
        let state = match pressed {
            true => KeyState::Pressed,
            false => KeyState::Released,
        };
        self.device.key(millis(self.started), key, state.into());
        self.device.modifiers(
            self.state.serialize_mods(xkb::STATE_MODS_DEPRESSED),
            self.state.serialize_mods(xkb::STATE_MODS_LATCHED),
            self.state.serialize_mods(xkb::STATE_MODS_LOCKED),
            self.state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE),
        );
        self.client.flush()
    }

    pub fn press(&mut self, key: u32) -> Result<(), HarnessError> {
        self.send(key, true)
    }

    pub fn release(&mut self, key: u32) -> Result<(), HarnessError> {
        self.send(key, false)
    }

    pub fn tap(&mut self, key: u32) -> Result<(), HarnessError> {
        self.press(key)?;
        self.release(key)
    }
}

impl Drop for VirtualKeyboard {
    fn drop(&mut self) {
        self.device.destroy();
        let _ = self.client.flush();
    }
}
