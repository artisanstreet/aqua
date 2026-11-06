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

pub struct UiaHook {
    hwnd: isize,
    router: aqua_a11y_uia::UiaRouter,
}

impl UiaHook {
    pub fn new(hwnd: isize, router: aqua_a11y_uia::UiaRouter) -> Self {
        Self { hwnd, router }
    }
}

impl MessageHook for UiaHook {
    fn window_message(&mut self, message: &RawMessage) -> Option<isize> {
        match message.hwnd == self.hwnd {
            true => self
                .router
                .handle(message.message, message.wparam, message.lparam),
            false => None,
        }
    }

    fn retired(&self) -> bool {
        self.router.retired()
    }
}
