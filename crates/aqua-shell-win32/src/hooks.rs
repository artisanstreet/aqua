use std::cell::RefCell;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawMessage {
    pub hwnd: isize,
    pub message: u32,
    pub wparam: usize,
    pub lparam: isize,
}

pub trait MessageHook: 'static {
    fn pre_translate(&mut self, _message: &RawMessage) -> bool {
        false
    }

    fn window_message(&mut self, _message: &RawMessage) -> Option<isize> {
        None
    }
}

thread_local! {
    static HOOK: RefCell<Option<Box<dyn MessageHook>>> = const { RefCell::new(None) };
}

pub fn set_message_hook(hook: impl MessageHook) {
    HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(windows)]
fn with_hook<R>(call: impl FnOnce(&mut dyn MessageHook) -> R) -> Option<R> {
    let mut taken = HOOK.with(|slot| slot.borrow_mut().take())?;
    let result = call(taken.as_mut());
    HOOK.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(taken);
        }
    });
    Some(result)
}

#[cfg(windows)]
pub(crate) fn pre_translate(message: &RawMessage) -> bool {
    with_hook(|hook| hook.pre_translate(message)).unwrap_or(false)
}

#[cfg(windows)]
pub(crate) fn window_message(message: &RawMessage) -> Option<isize> {
    with_hook(|hook| hook.window_message(message)).flatten()
}
