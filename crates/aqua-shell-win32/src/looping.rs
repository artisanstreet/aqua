use crate::event::{Handler, ShellError, ShellEvent};
use crate::state::{self, HANDLER, IN_HANDLER, ORIGIN, Queued};
use crate::timing;
use crate::wake_trace::{self, Source};
use crate::wndproc::{WM_WAKE, hwnd_from};
use aqua_protocol::Instant;
use windows::Win32::Foundation::{HANDLE, LPARAM, LRESULT, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM};
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

const PUMP_SPACING: std::time::Duration = std::time::Duration::from_micros(300);

pub fn pump_sent_messages(location: &'static std::panic::Location<'static>) {
    if !state::UI_THREAD.get() {
        return;
    }
    crate::gaps::reached(location);
    let now = std::time::Instant::now();
    if state::LAST_PUMP
        .get()
        .is_some_and(|last| now.duration_since(last) < PUMP_SPACING)
    {
        return;
    }
    state::LAST_PUMP.set(Some(now));
    let mut msg = MSG::default();
    let _ = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE | PM_QS_SENDMESSAGE) };
}

unsafe extern "system" fn trace_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && lparam.0 != 0 {
        let info = unsafe { &*(lparam.0 as *const CWPSTRUCT) };
        let mut buffer = [0u16; 64];
        let length = unsafe { GetClassNameW(info.hwnd, &mut buffer) }.max(0) as usize;
        wake_trace::record(Source::Hooked {
            class: String::from_utf16_lossy(&buffer[..length]),
            message: info.message,
        });
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn install_trace_hook() {
    if wake_trace::enabled() {
        let _ = unsafe {
            SetWindowsHookExW(
                WH_CALLWNDPROC,
                Some(trace_hook),
                None,
                windows::Win32::System::Threading::GetCurrentThreadId(),
            )
        };
    }
}

pub fn quit() {
    unsafe { PostQuitMessage(0) };
}

pub fn set_waitables(handles: Vec<isize>) {
    state::with(|s| s.waitables = handles);
}

thread_local! {
    static WATCH: std::cell::RefCell<crate::occlusion::Watch> =
        std::cell::RefCell::new(crate::occlusion::Watch::default());
}

fn watch_occlusion(active: bool) {
    let current = now();
    let covered = state::with(|s| s.windows.values().any(|w| w.covered || w.minimized));
    let change = WATCH.with(|watch| watch.borrow_mut().step(current, active, covered));
    if let Some(level) = change {
        crate::occlusion::set_level(level);
    }
    if active && WATCH.with(|watch| watch.borrow_mut().refresh_due(current)) {
        crate::occlusion::refresh();
    }
}

fn watch_deadline() -> Option<Instant> {
    WATCH.with(|watch| watch.borrow().next_check())
}

pub(crate) fn turn() {
    if IN_HANDLER.get() {
        return;
    }
    let Some(handler) = HANDLER.get() else {
        return;
    };
    let pending = state::with(|s| !s.queue.is_empty() || timing::due(now(), s.wake_at));
    watch_occlusion(pending);
    crate::occlusion::pump();
    crate::gaps::turn_started();
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
    crate::gaps::turn_ended();
    IN_HANDLER.set(false);
    watch_occlusion(false);
}

fn redraw_pending() -> bool {
    state::with(|s| s.windows.values().any(|w| w.redraw_pending) || !s.queue.is_empty())
}

pub fn run<H: Handler>(handler: H) -> Result<(), ShellError> {
    if HANDLER.get().is_some() {
        return Err(ShellError::LoopAlreadyRunning);
    }
    state::UI_THREAD.set(true);
    wake_trace::enable_from_env();
    install_trace_hook();
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
            wake_trace::record(Source::Message(msg.message));
            let consumed = crate::hooks::pre_translate(&crate::hooks::RawMessage {
                hwnd: msg.hwnd.0 as isize,
                message: msg.message,
                wparam: msg.wParam.0,
                lparam: msg.lParam.0,
            });
            if consumed {
                state::with(|s| s.queue.push_back(Queued::Wake));
            } else {
                wake_trace::dispatching(true);
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                wake_trace::dispatching(false);
            }
        }
        if quit {
            break;
        }
        turn();
        wake_trace::flush_if_due();
        let current = now();
        let (handler_wake, waitables) = state::with(|s| (s.wake_at, s.waitables.clone()));
        let wake_at = [handler_wake, watch_deadline()].into_iter().flatten().min();
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
        match (
            timer.is_some() && index == 0,
            index >= offset && index < handles.len(),
        ) {
            (true, _) => wake_trace::record(Source::Timer),
            (_, true) => wake_trace::record(Source::Waitable),
            _ if waited.0 == WAIT_TIMEOUT.0 => wake_trace::record(Source::Timeout),
            _ if wake_trace::enabled() && index == handles.len() => {
                let status = unsafe { GetQueueStatus(QS_ALLINPUT) };
                wake_trace::record(Source::Queue(status >> 16));
            }
            _ => wake_trace::record(Source::Other(waited.0)),
        }
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
