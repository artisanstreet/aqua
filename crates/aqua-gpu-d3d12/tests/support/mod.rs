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

#[allow(dead_code)]
pub struct Prioritized {
    thread: i32,
    class: u32,
}

#[allow(dead_code)]
pub fn prioritized() -> Prioritized {
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentThread, GetPriorityClass, GetThreadPriority, SetPriorityClass,
        SetThreadPriority, HIGH_PRIORITY_CLASS, PROCESS_CREATION_FLAGS,
        THREAD_PRIORITY_TIME_CRITICAL,
    };
    let thread = unsafe { GetCurrentThread() };
    let process = unsafe { GetCurrentProcess() };
    let previous = Prioritized {
        thread: unsafe { GetThreadPriority(thread) },
        class: unsafe { GetPriorityClass(process) },
    };
    let _ = unsafe { SetPriorityClass(process, PROCESS_CREATION_FLAGS(HIGH_PRIORITY_CLASS.0)) };
    let _ = unsafe { SetThreadPriority(thread, THREAD_PRIORITY_TIME_CRITICAL) };
    previous
}

impl Drop for Prioritized {
    fn drop(&mut self) {
        use windows::Win32::System::Threading::{
            GetCurrentProcess, GetCurrentThread, SetPriorityClass, SetThreadPriority,
            PROCESS_CREATION_FLAGS, THREAD_PRIORITY,
        };
        let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY(self.thread)) };
        let _ =
            unsafe { SetPriorityClass(GetCurrentProcess(), PROCESS_CREATION_FLAGS(self.class)) };
    }
}
