use crate::globals::Globals;
use crate::state::{self, Shell};
use crate::window::Window;
use aqua_protocol::WindowId;
use aqua_shell::ShellError;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::{
    self, ChangeCause, ContentHint, ContentPurpose, ZwpTextInputV3,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Surrounding {
    pub text: String,
    pub cursor: u32,
    pub anchor: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextInputState {
    pub surrounding: Option<Surrounding>,
    pub cursor: Option<CursorRect>,
    pub changed_by_input_method: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preedit {
    pub text: String,
    pub cursor: Option<(usize, usize)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInputBatch {
    pub preedit: Option<Preedit>,
    pub commit: Option<String>,
    pub delete: Option<(u32, u32)>,
    pub serial: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextInputEvent {
    Entered,
    Left,
    Done(TextInputBatch),
}

#[derive(Default)]
pub(crate) struct Transport {
    proxy: Option<ZwpTextInputV3>,
    focus: Option<WindowId>,
    commits: u32,
    pending: TextInputBatch,
    queues: HashMap<WindowId, Vec<TextInputEvent>>,
}

impl Transport {
    pub(crate) fn attach(&mut self, globals: &Globals, seat: &WlSeat, qh: &QueueHandle<Shell>) {
        if let (None, Some(manager)) = (&self.proxy, &globals.text_input) {
            self.proxy = Some(manager.get_text_input(seat, qh, ()));
        }
    }

    pub(crate) fn release(&mut self) {
        self.lose_focus();
        if let Some(proxy) = self.proxy.take() {
            proxy.destroy();
        }
    }

    pub(crate) fn forget(&mut self, window: WindowId) {
        self.queues.remove(&window);
        if self.focus == Some(window) {
            self.focus = None;
            self.pending = TextInputBatch::default();
        }
    }

    fn push(&mut self, window: WindowId, event: TextInputEvent) {
        if let Some(queue) = self.queues.get_mut(&window) {
            queue.push(event);
            crate::looping::wake_turn();
        }
    }

    fn lose_focus(&mut self) {
        self.pending = TextInputBatch::default();
        if let Some(window) = self.focus.take() {
            self.push(window, TextInputEvent::Left);
        }
    }

    fn focused_proxy(&self, window: WindowId) -> Option<&ZwpTextInputV3> {
        self.proxy.as_ref().filter(|_| self.focus == Some(window))
    }

    fn commit(&mut self) -> u32 {
        if let Some(proxy) = &self.proxy {
            proxy.commit();
            self.commits = self.commits.wrapping_add(1);
        }
        self.commits
    }

    fn send_state(proxy: &ZwpTextInputV3, state: &TextInputState) {
        if let Some(surrounding) = &state.surrounding {
            proxy.set_surrounding_text(
                surrounding.text.clone(),
                i32::try_from(surrounding.cursor).unwrap_or(i32::MAX),
                i32::try_from(surrounding.anchor).unwrap_or(i32::MAX),
            );
        }
        proxy.set_text_change_cause(match state.changed_by_input_method {
            true => ChangeCause::InputMethod,
            false => ChangeCause::Other,
        });
        if let Some(cursor) = state.cursor {
            proxy.set_cursor_rectangle(cursor.x, cursor.y, cursor.width, cursor.height);
        }
    }

    fn enable(&mut self, window: WindowId, state: &TextInputState) -> Option<u32> {
        let proxy = self.focused_proxy(window)?;
        proxy.enable();
        proxy.set_content_type(ContentHint::None, ContentPurpose::Normal);
        Self::send_state(proxy, state);
        Some(self.commit())
    }

    fn update(&mut self, window: WindowId, state: &TextInputState) -> Option<u32> {
        Self::send_state(self.focused_proxy(window)?, state);
        Some(self.commit())
    }

    fn disable(&mut self, window: WindowId) -> Option<u32> {
        self.focused_proxy(window)?.disable();
        Some(self.commit())
    }
}

pub struct TextInput {
    window: WindowId,
    _thread_bound: PhantomData<Rc<()>>,
}

fn unavailable() -> ShellError {
    ShellError::Wayland(
        "zwp_text_input_manager_v3",
        "the compositor offers no text-input-v3 on this seat".into(),
    )
}

impl TextInput {
    pub fn attach(window: Window) -> Result<TextInput, ShellError> {
        let id = window.id();
        state::try_with(|shell| {
            if shell.text_input.proxy.is_none() {
                return Err(unavailable());
            }
            if !shell.windows.contains_key(&id) {
                return Err(ShellError::Wayland(
                    "zwp_text_input_v3",
                    "the window was destroyed".into(),
                ));
            }
            shell.text_input.queues.entry(id).or_default();
            Ok(())
        })
        .unwrap_or_else(|| Err(unavailable()))?;
        Ok(TextInput {
            window: id,
            _thread_bound: PhantomData,
        })
    }

    pub fn window(&self) -> WindowId {
        self.window
    }

    pub fn is_focused(&self) -> bool {
        state::try_with(|shell| shell.text_input.focus == Some(self.window)).unwrap_or(false)
    }

    fn request(&self, send: impl FnOnce(&mut Transport, WindowId) -> Option<u32>) -> Option<u32> {
        let serial = state::try_with(|shell| send(&mut shell.text_input, self.window)).flatten();
        if let Err(error) = state::flush() {
            eprintln!("aqua: ime: {error}");
        }
        serial
    }

    pub fn enable(&self, state: &TextInputState) -> Option<u32> {
        self.request(|transport, window| transport.enable(window, state))
    }

    pub fn update(&self, state: &TextInputState) -> Option<u32> {
        self.request(|transport, window| transport.update(window, state))
    }

    pub fn disable(&self) -> Option<u32> {
        self.request(|transport, window| transport.disable(window))
    }

    pub fn drain(&self, out: &mut Vec<TextInputEvent>) {
        state::try_with(|shell| {
            if let Some(queue) = shell.text_input.queues.get_mut(&self.window) {
                out.append(queue);
            }
        });
    }
}

impl Drop for TextInput {
    fn drop(&mut self) {
        let window = self.window;
        state::try_with(|shell| {
            if shell.text_input.focus == Some(window) {
                shell.text_input.disable(window);
            }
            shell.text_input.queues.remove(&window);
        });
        let _ = state::flush();
    }
}

fn byte_cursor(begin: i32, end: i32) -> Option<(usize, usize)> {
    match (usize::try_from(begin), usize::try_from(end)) {
        (Ok(begin), Ok(end)) => Some((begin, end)),
        _ => None,
    }
}

impl Dispatch<ZwpTextInputV3, ()> for Shell {
    fn event(
        state: &mut Self,
        _: &ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let transport = &mut state.text_input;
        match event {
            zwp_text_input_v3::Event::Enter { surface } => {
                transport.lose_focus();
                let window = surface
                    .data::<WindowId>()
                    .copied()
                    .filter(|id| state.windows.contains_key(id));
                transport.focus = window;
                if let Some(window) = window {
                    transport.push(window, TextInputEvent::Entered);
                }
            }
            zwp_text_input_v3::Event::Leave { .. } => transport.lose_focus(),
            zwp_text_input_v3::Event::PreeditString {
                text,
                cursor_begin,
                cursor_end,
            } => {
                transport.pending.preedit =
                    text.filter(|text| !text.is_empty()).map(|text| Preedit {
                        text,
                        cursor: byte_cursor(cursor_begin, cursor_end),
                    });
            }
            zwp_text_input_v3::Event::CommitString { text } => {
                transport.pending.commit = text.filter(|text| !text.is_empty());
            }
            zwp_text_input_v3::Event::DeleteSurroundingText {
                before_length,
                after_length,
            } => {
                transport.pending.delete =
                    Some((before_length, after_length)).filter(|lengths| *lengths != (0, 0));
            }
            zwp_text_input_v3::Event::Done { serial } => {
                let batch = TextInputBatch {
                    serial,
                    ..std::mem::take(&mut transport.pending)
                };
                if let Some(window) = transport.focus {
                    transport.push(window, TextInputEvent::Done(batch));
                }
            }
            _ => {}
        }
    }
}
