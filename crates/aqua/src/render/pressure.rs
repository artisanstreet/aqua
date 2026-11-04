use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_secs(2);

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
fn query(handle: windows::Win32::Foundation::HANDLE) -> Option<bool> {
    use windows::Win32::System::Memory::QueryMemoryResourceNotification;
    let mut state = windows::core::BOOL(0);
    unsafe { QueryMemoryResourceNotification(handle, &mut state) }
        .ok()
        .map(|()| state.as_bool())
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
        self.handle.and_then(query).unwrap_or(false)
    }

    #[cfg(not(windows))]
    fn sample(&self) -> bool {
        false
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
