use std::sync::{Mutex, MutexGuard};

static SOFTWARE: Mutex<()> = Mutex::new(());

pub fn software() -> MutexGuard<'static, ()> {
    SOFTWARE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
