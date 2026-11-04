use aqua_ime_tsf::Keystrokes;
use aqua_shell_win32::{MessageHook, RawMessage};

pub struct TsfHook {
    keystrokes: Keystrokes,
}

impl TsfHook {
    pub fn new() -> Self {
        Self {
            keystrokes: Keystrokes::new(),
        }
    }
}

impl MessageHook for TsfHook {
    fn pre_translate(&mut self, message: &RawMessage) -> bool {
        self.keystrokes
            .pre_translate(message.message, message.wparam, message.lparam)
    }
}
