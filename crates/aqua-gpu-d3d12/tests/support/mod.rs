use std::cell::RefCell;
use std::sync::{Mutex, MutexGuard};

static SOFTWARE: Mutex<()> = Mutex::new(());

pub fn software() -> MutexGuard<'static, ()> {
    SOFTWARE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

thread_local! {
    static HELD: RefCell<Option<MutexGuard<'static, ()>>> = const { RefCell::new(None) };
}

#[allow(dead_code)]
pub fn hold_software() {
    HELD.with(|held| {
        let mut held = held.borrow_mut();
        if held.is_none() {
            *held = Some(software());
        }
    });
}
