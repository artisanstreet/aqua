#![allow(dead_code)]

use std::cell::RefCell;
use std::sync::{Mutex, MutexGuard};

static EXCLUSIVE: Mutex<()> = Mutex::new(());

thread_local! {
    static HELD: RefCell<Option<MutexGuard<'static, ()>>> = const { RefCell::new(None) };
}

pub fn hold_exclusive() {
    HELD.with(|held| {
        let mut held = held.borrow_mut();
        if held.is_none() {
            *held = Some(
                EXCLUSIVE
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            );
        }
    });
}
