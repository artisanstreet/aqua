use super::keys;
use super::repeat::{Repeat, RepeatInfo};
use crate::looping::emit;
use crate::state::Shell;
use aqua_core::{InputEvent, KeyEvent, Modifiers};
use aqua_protocol::{Instant, WindowId};
use aqua_shell::input::{control_free, is_shortcut, modifiers};
use aqua_shell::ShellEvent;
use std::os::fd::OwnedFd;
use std::os::unix::fs::FileExt;
use std::time::Duration;
use wayland_client::protocol::wl_keyboard::{self, KeyState, KeymapFormat, WlKeyboard};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use xkbcommon::xkb::{self, compose, Keycode, Keysym};

struct Xkb {
    keymap: xkb::Keymap,
    state: xkb::State,
}

pub(crate) struct Keyboard {
    proxy: WlKeyboard,
    context: xkb::Context,
    xkb: Option<Xkb>,
    compose: Option<compose::State>,
    focus: Option<WindowId>,
    modifiers: Modifiers,
    info: RepeatInfo,
    repeat: Repeat,
}

fn locale() -> std::ffi::OsString {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .filter_map(std::env::var_os)
        .find(|value| !value.is_empty())
        .unwrap_or_else(|| "C".into())
}

fn compose_state(context: &xkb::Context) -> Option<compose::State> {
    let preferred = locale();
    [
        preferred.as_os_str(),
        "C.UTF-8".as_ref(),
        "en_US.UTF-8".as_ref(),
    ]
    .into_iter()
    .find_map(|locale| {
        compose::Table::new_from_locale(context, locale, compose::COMPILE_NO_FLAGS).ok()
    })
    .map(|table| compose::State::new(&table, compose::STATE_NO_FLAGS))
}

fn read_keymap(fd: OwnedFd, size: u32) -> Option<String> {
    let file = std::fs::File::from(fd);
    let mut bytes = vec![0u8; usize::try_from(size).ok()?];
    file.read_exact_at(&mut bytes, 0).ok()?;
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    bytes.truncate(end);
    String::from_utf8(bytes).ok()
}

impl Keyboard {
    pub(crate) fn new(proxy: WlKeyboard) -> Self {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let compose = compose_state(&context);
        Self {
            proxy,
            context,
            xkb: None,
            compose,
            focus: None,
            modifiers: Modifiers::NONE,
            info: RepeatInfo::default(),
            repeat: Repeat::Idle,
        }
    }

    pub(crate) fn forget(&mut self, window: WindowId) {
        if self.focus == Some(window) {
            self.focus = None;
            self.repeat = Repeat::Idle;
        }
    }

    pub(crate) fn repeat_deadline(&self) -> Option<Instant> {
        self.repeat.deadline()
    }

    fn keymap(&mut self, fd: OwnedFd, size: u32) {
        let keymap = read_keymap(fd, size).and_then(|text| {
            xkb::Keymap::new_from_string(
                &self.context,
                text,
                xkb::KEYMAP_FORMAT_TEXT_V1,
                xkb::KEYMAP_COMPILE_NO_FLAGS,
            )
        });
        self.xkb = keymap.map(|keymap| Xkb {
            state: xkb::State::new(&keymap),
            keymap,
        });
        if self.xkb.is_none() {
            eprintln!("aqua: the compositor sent a keymap xkbcommon cannot compile");
        }
    }

    fn set_modifiers(&mut self, next: Modifiers) {
        if std::mem::replace(&mut self.modifiers, next) != next
            && let Some(window) = self.focus
        {
            emit(
                window,
                ShellEvent::Input(InputEvent::ModifiersChanged(next)),
            );
        }
    }

    fn update_modifiers(&mut self, depressed: u32, latched: u32, locked: u32, group: u32) {
        let Some(xkb) = self.xkb.as_mut() else {
            return;
        };
        xkb.state
            .update_mask(depressed, latched, locked, 0, 0, group);
        let active = |name| {
            xkb.state
                .mod_name_is_active(name, xkb::STATE_MODS_EFFECTIVE)
        };
        let next = modifiers(
            active(xkb::MOD_NAME_SHIFT),
            active(xkb::MOD_NAME_CTRL),
            active(xkb::MOD_NAME_ALT),
            active(xkb::MOD_NAME_LOGO),
        );
        self.set_modifiers(next);
    }

    fn text(&mut self, key: Keycode, keysym: Keysym) -> Option<String> {
        let typed = || self.xkb.as_ref().map(|xkb| xkb.state.key_get_utf8(key));
        let composed = self.compose.as_mut().and_then(|compose| {
            match (compose.feed(keysym), compose.status()) {
                (compose::FeedResult::Ignored, _) | (_, compose::Status::Nothing) => None,
                (_, compose::Status::Composing) => Some(None),
                (_, compose::Status::Composed) => {
                    let text = compose.utf8();
                    compose.reset();
                    Some(text)
                }
                (_, compose::Status::Cancelled) => {
                    compose.reset();
                    Some(None)
                }
            }
        });
        match composed {
            Some(text) => text,
            None => typed(),
        }
        .map(|text| control_free(&text))
        .filter(|text| !text.is_empty())
    }

    fn emit_key(&mut self, window: WindowId, key: Keycode, pressed: bool, repeat: bool) -> bool {
        let Some(keysym) = self.xkb.as_ref().map(|xkb| xkb.state.key_get_one_sym(key)) else {
            return false;
        };
        if keys::is_modifier(keysym) {
            return false;
        }
        let event = KeyEvent {
            key: keys::resolve(keysym),
            modifiers: self.modifiers,
            repeat,
        };
        if !pressed {
            emit(window, ShellEvent::Input(InputEvent::KeyUp(event)));
            return false;
        }
        emit(window, ShellEvent::Input(InputEvent::KeyDown(event)));
        let text = match repeat {
            true => self
                .xkb
                .as_ref()
                .map(|xkb| control_free(&xkb.state.key_get_utf8(key)))
                .filter(|text| !text.is_empty()),
            false => self.text(key, keysym),
        };
        if let Some(text) = text.filter(|_| !is_shortcut(self.modifiers)) {
            emit(window, ShellEvent::Input(InputEvent::TextInput { text }));
        }
        self.xkb
            .as_ref()
            .is_some_and(|xkb| xkb.keymap.key_repeats(key))
    }

    fn key(&mut self, raw: u32, pressed: bool) {
        let Some(window) = self.focus else {
            return;
        };
        let key = Keycode::new(raw + 8);
        if !pressed {
            self.repeat.release(key);
        }
        let repeats = self.emit_key(window, key, pressed, false);
        if pressed && repeats {
            self.repeat = Repeat::press(self.info, window, key, crate::clock::now());
        }
    }

    pub(crate) fn fire_repeats(&mut self, now: Instant) {
        if let Some((window, key)) = self.repeat.due(self.info, now) {
            self.emit_key(window, key, true, true);
        }
    }

    fn leave(&mut self) {
        self.repeat = Repeat::Idle;
        if let Some(compose) = self.compose.as_mut() {
            compose.reset();
        }
        self.set_modifiers(Modifiers::NONE);
        self.focus = None;
    }
}

impl Drop for Keyboard {
    fn drop(&mut self) {
        if self.proxy.version() >= 3 {
            self.proxy.release();
        }
    }
}

impl Dispatch<WlKeyboard, ()> for Shell {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let windows = &state.windows;
        let seat = &mut state.seat;
        let Some(keyboard) = seat.keyboard.as_mut() else {
            return;
        };
        match event {
            wl_keyboard::Event::Keymap {
                format: WEnum::Value(KeymapFormat::XkbV1),
                fd,
                size,
            } => keyboard.keymap(fd, size),
            wl_keyboard::Event::Enter {
                serial, surface, ..
            } => {
                seat.serial = Some(serial);
                keyboard.focus = surface
                    .data::<WindowId>()
                    .copied()
                    .filter(|id| windows.contains_key(id));
            }
            wl_keyboard::Event::Leave { .. } => keyboard.leave(),
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => keyboard.update_modifiers(mods_depressed, mods_latched, mods_locked, group),
            wl_keyboard::Event::Key {
                serial,
                key,
                state: WEnum::Value(key_state),
                ..
            } => {
                let pressed = key_state == KeyState::Pressed;
                if pressed {
                    seat.serial = Some(serial);
                }
                keyboard.key(key, pressed);
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                keyboard.info = RepeatInfo {
                    per_second: u32::try_from(rate).unwrap_or(0),
                    delay: Duration::from_millis(u64::try_from(delay).unwrap_or(0)),
                };
                if keyboard.info.per_second == 0 {
                    keyboard.repeat = Repeat::Idle;
                }
            }
            _ => {}
        }
    }
}
