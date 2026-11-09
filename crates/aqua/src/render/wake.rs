use std::sync::{Condvar, Mutex};
use std::time::Duration;

pub(super) struct Signal {
    raised: Mutex<bool>,
    condvar: Condvar,
    #[cfg(windows)]
    precise: Option<precise::Signal>,
}

impl Default for Signal {
    fn default() -> Self {
        Self {
            raised: Mutex::new(false),
            condvar: Condvar::new(),
            #[cfg(windows)]
            precise: precise::Signal::new(),
        }
    }
}

impl Signal {
    pub fn notify(&self) {
        #[cfg(windows)]
        if let Some(precise) = &self.precise {
            precise.notify();
            return;
        }
        *self
            .raised
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        self.condvar.notify_one();
    }

    pub fn wait(&self, timeout: Option<Duration>) {
        #[cfg(windows)]
        if let Some(precise) = &self.precise {
            precise.wait(timeout);
            return;
        }
        let mut raised = self
            .raised
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !*raised {
            raised = match timeout {
                Some(timeout) => {
                    self.condvar
                        .wait_timeout(raised, timeout)
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .0
                }
                None => self
                    .condvar
                    .wait(raised)
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            };
        }
        *raised = false;
    }
}

#[cfg(windows)]
mod precise {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::time::Duration;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Threading::{
        CancelWaitableTimer, CreateEventW, CreateWaitableTimerExW, SetEvent, SetWaitableTimerEx,
        WaitForMultipleObjects, WaitForSingleObject, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
    };

    pub struct Signal {
        event: OwnedHandle,
        timer: OwnedHandle,
    }

    impl Signal {
        pub fn new() -> Option<Self> {
            let event = unsafe { CreateEventW(None, false, false, None) }.ok()?;
            let event = unsafe { OwnedHandle::from_raw_handle(event.0) };
            let timer = unsafe {
                CreateWaitableTimerExW(None, None, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, 0x1F0003)
            }
            .ok()?;
            let timer = unsafe { OwnedHandle::from_raw_handle(timer.0) };
            Some(Signal { event, timer })
        }
    }

    impl Signal {
        pub fn notify(&self) {
            let _ = unsafe { SetEvent(HANDLE(self.event.as_raw_handle())) };
        }

        pub fn wait(&self, timeout: Option<Duration>) {
            let event = HANDLE(self.event.as_raw_handle());
            let timer = HANDLE(self.timer.as_raw_handle());
            match timeout {
                Some(timeout) => {
                    let ticks = timeout.as_nanos().div_ceil(100).max(1);
                    let due = -i64::try_from(ticks).unwrap_or(i64::MAX);
                    match unsafe { SetWaitableTimerEx(timer, &due, 0, None, None, None, 0) } {
                        Ok(()) => {
                            unsafe { WaitForMultipleObjects(&[event, timer], false, u32::MAX) };
                            let _ = unsafe { CancelWaitableTimer(timer) };
                        }
                        Err(_) => {
                            let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
                            unsafe { WaitForSingleObject(event, millis) };
                        }
                    }
                }
                None => {
                    unsafe { WaitForSingleObject(event, u32::MAX) };
                }
            }
        }
    }
}
