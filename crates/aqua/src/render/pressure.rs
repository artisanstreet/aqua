use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_secs(2);

static SIMULATED: AtomicBool = AtomicBool::new(false);

pub fn simulate_low_memory(low: bool) {
    SIMULATED.store(low, Ordering::Release);
}

#[derive(Debug)]
pub struct MemoryPressure {
    last_poll: Option<Instant>,
    low: bool,
    #[cfg(windows)]
    handle: Option<windows::Win32::Foundation::HANDLE>,
}

#[cfg(windows)]
fn open() -> Option<windows::Win32::Foundation::HANDLE> {
    use windows::Win32::System::Memory::{
        CreateMemoryResourceNotification, LowMemoryResourceNotification,
    };
    unsafe { CreateMemoryResourceNotification(LowMemoryResourceNotification) }.ok()
}

#[cfg(windows)]
static INJECTED: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

#[cfg(windows)]
pub fn watch_notification(handle: Option<windows::Win32::Foundation::HANDLE>) {
    INJECTED.store(
        handle.map_or(0, |handle| handle.0 as isize),
        Ordering::Release,
    );
}

#[cfg(windows)]
fn signalled(handle: windows::Win32::Foundation::HANDLE) -> bool {
    use windows::Win32::Foundation::WAIT_OBJECT_0;
    use windows::Win32::System::Threading::WaitForSingleObject;
    let state = unsafe { WaitForSingleObject(handle, 0) };
    state == WAIT_OBJECT_0
}

impl MemoryPressure {
    pub fn new() -> Self {
        Self {
            last_poll: None,
            low: false,
            #[cfg(windows)]
            handle: open(),
        }
    }

    #[cfg(windows)]
    fn sample(&self) -> bool {
        let injected = match INJECTED.load(Ordering::Acquire) {
            0 => None,
            raw => Some(windows::Win32::Foundation::HANDLE(raw as *mut _)),
        };
        SIMULATED.load(Ordering::Acquire)
            || self.handle.is_some_and(signalled)
            || injected.is_some_and(signalled)
    }

    #[cfg(not(windows))]
    fn sample(&self) -> bool {
        SIMULATED.load(Ordering::Acquire)
    }

    pub fn rising_edge(&mut self, now: Instant) -> bool {
        let due = self
            .last_poll
            .is_none_or(|last| now.duration_since(last) >= POLL_INTERVAL);
        if !due {
            return false;
        }
        self.last_poll = Some(now);
        let low = self.sample();
        let edge = low && !self.low;
        self.low = low;
        edge
    }
}

impl Default for MemoryPressure {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
impl Drop for MemoryPressure {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
        }
    }
}
