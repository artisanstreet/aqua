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

    fn retired(&self) -> bool {
        false
    }
}

thread_local! {
    static HOOKS: RefCell<Vec<Box<dyn MessageHook>>> = const { RefCell::new(Vec::new()) };
}

pub fn set_message_hook(hook: impl MessageHook) {
    HOOKS.with(|slot| *slot.borrow_mut() = vec![Box::new(hook)]);
}

pub fn add_message_hook(hook: impl MessageHook) {
    HOOKS.with(|slot| slot.borrow_mut().push(Box::new(hook)));
}

#[cfg(windows)]
fn with_hooks<R>(call: impl FnOnce(&mut [Box<dyn MessageHook>]) -> R) -> R {
    let mut taken = HOOKS.with(|slot| std::mem::take(&mut *slot.borrow_mut()));
    let result = call(&mut taken);
    taken.retain(|hook| !hook.retired());
    HOOKS.with(|slot| {
        let mut slot = slot.borrow_mut();
        taken.append(&mut slot);
        *slot = taken;
    });
    result
}

#[cfg(windows)]
pub(crate) fn pre_translate(message: &RawMessage) -> bool {
    with_hooks(|hooks| hooks.iter_mut().any(|hook| hook.pre_translate(message)))
}

#[cfg(windows)]
pub(crate) fn window_message(message: &RawMessage) -> Option<isize> {
    with_hooks(|hooks| {
        hooks
            .iter_mut()
            .find_map(|hook| hook.window_message(message))
    })
}
