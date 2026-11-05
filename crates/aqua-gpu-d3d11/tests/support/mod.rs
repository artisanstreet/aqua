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
        GetCurrentProcess, GetCurrentThread, GetPriorityClass, GetThreadPriority,
        HIGH_PRIORITY_CLASS, PROCESS_CREATION_FLAGS, SetPriorityClass, SetThreadPriority,
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
            GetCurrentProcess, GetCurrentThread, PROCESS_CREATION_FLAGS, SetPriorityClass,
            SetThreadPriority, THREAD_PRIORITY,
        };
        let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY(self.thread)) };
        let _ =
            unsafe { SetPriorityClass(GetCurrentProcess(), PROCESS_CREATION_FLAGS(self.class)) };
    }
}

#[allow(dead_code)]
pub fn integrated() -> aqua_gpu_d3d11::hal::selection::AdapterPreference {
    use aqua_gpu_d3d11::hal::selection::{Adapter, AdapterPreference};
    let factory = aqua_gpu_d3d11::hal::create_factory(false).expect("dxgi factory");
    let mut hardware: Vec<_> = aqua_gpu_d3d11::hal::list_adapters(&factory)
        .into_iter()
        .filter(|adapter| adapter.kind == Adapter::Hardware)
        .collect();
    hardware.sort_by_key(|adapter| adapter.dedicated_video_bytes);
    assert!(
        hardware.len() >= 2,
        "an integrated GPU next to the discrete one is required: {hardware:?}"
    );
    AdapterPreference::Luid(hardware[0].luid)
}
