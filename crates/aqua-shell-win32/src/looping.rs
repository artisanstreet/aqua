use crate::event::{Handler, ShellError, ShellEvent};
use crate::state::{self, HANDLER, IN_HANDLER, ORIGIN, Queued};
use crate::timing;
use crate::wndproc::{WM_WAKE, hwnd_from};
use aqua_protocol::Instant;
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CancelWaitableTimer, CreateWaitableTimerExW,
    SetWaitableTimer,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const TIMER_ALL_ACCESS: u32 = 0x1F0003;

#[derive(Debug, Clone, Copy)]
pub struct Waker {
    hwnd: isize,
}

impl Waker {
    pub(crate) fn new(hwnd: isize) -> Self {
        Self { hwnd }
    }

    pub fn wake(&self) {
        let _ = unsafe {
            PostMessageW(
                Some(hwnd_from(self.hwnd)),
                WM_WAKE,
                Default::default(),
                Default::default(),
            )
        };
    }
}

unsafe impl Send for Waker {}
unsafe impl Sync for Waker {}

pub fn now() -> Instant {
    let origin = *ORIGIN.get_or_init(std::time::Instant::now);
    Instant::from_nanos(u64::try_from(origin.elapsed().as_nanos()).unwrap_or(u64::MAX))
}

pub fn schedule(at: Instant) {
    state::with(|s| s.wake_at = Some(s.wake_at.map_or(at, |existing| existing.min(at))));
}

pub fn quit() {
    unsafe { PostQuitMessage(0) };
}

pub fn set_waitables(handles: Vec<isize>) {
    state::with(|s| s.waitables = handles);
}

pub(crate) fn turn() {
    if IN_HANDLER.get() {
        return;
    }
    let Some(handler) = HANDLER.get() else {
        return;
    };
    IN_HANDLER.set(true);
    let handler = unsafe { &mut *handler };
    let mut worked = false;
    while let Some(item) = state::with(|s| s.queue.pop_front()) {
        worked = true;
        match item {
            Queued::Window(id, event) => handler.window_event(id, event),
            Queued::Waitable(index) => handler.waitable(index),
            Queued::Wake => handler.woken(),
        }
    }
    let current = now();
    let due = state::with(|s| timing::due(current, s.wake_at));
    if worked || due {
        let next = handler.tick(current);
        state::with(|s| s.wake_at = next);
    }
    let redraws: Vec<_> = state::with(|s| {
        s.windows
            .values_mut()
            .filter_map(|w| std::mem::take(&mut w.redraw_pending).then_some(w.id))
            .collect()
    });
    for id in redraws {
        handler.window_event(id, ShellEvent::Redraw);
    }
    IN_HANDLER.set(false);
}

fn redraw_pending() -> bool {
    state::with(|s| s.windows.values().any(|w| w.redraw_pending) || !s.queue.is_empty())
}

pub fn run<H: Handler>(handler: H) -> Result<(), ShellError> {
    if HANDLER.get().is_some() {
        return Err(ShellError::LoopAlreadyRunning);
    }
    let mut boxed: Box<dyn Handler> = Box::new(handler);
    HANDLER.set(Some(&mut *boxed as *mut dyn Handler));
    let timer = unsafe {
        CreateWaitableTimerExW(
            None,
            None,
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
            TIMER_ALL_ACCESS,
        )
    }
    .ok();
    let mut armed: Option<Instant> = None;
    loop {
        let mut msg = MSG::default();
        let mut quit = false;
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            if msg.message == WM_QUIT {
                quit = true;
                break;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if quit {
            break;
        }
        turn();
        let current = now();
        let (wake_at, waitables) = state::with(|s| (s.wake_at, s.waitables.clone()));
        let mut handles: Vec<HANDLE> = Vec::with_capacity(waitables.len() + 1);
        if let Some(timer) = timer {
            if armed != wake_at {
                match wake_at {
                    Some(at) => {
                        let due = timing::relative_due_time_100ns(current, at);
                        let _ = unsafe { SetWaitableTimer(timer, &due, 0, None, None, false) };
                    }
                    None => {
                        let _ = unsafe { CancelWaitableTimer(timer) };
                    }
                }
                armed = wake_at;
            }
            handles.push(timer);
        }
        let offset = handles.len();
        handles.extend(waitables.iter().map(|h| HANDLE(*h as *mut _)));
        let timeout = match (redraw_pending(), timer.is_some()) {
            (true, _) => 0,
            (false, true) => INFINITE_WAIT,
            (false, false) => timing::timeout_ms(current, wake_at),
        };
        let waited = unsafe {
            MsgWaitForMultipleObjectsEx(Some(&handles), timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
        let index = waited.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
        if timer.is_some() && index == 0 {
            armed = None;
        }
        if index < handles.len() && index >= offset {
            state::with(|s| s.queue.push_back(Queued::Waitable(index - offset)));
        }
    }
    HANDLER.set(None);
    drop(boxed);
    Ok(())
}

const INFINITE_WAIT: u32 = u32::MAX;
