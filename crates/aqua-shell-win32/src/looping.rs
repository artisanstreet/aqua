use crate::event::{win32, Handler, ShellError, ShellEvent};
use crate::state::{self, Queued, HANDLER, IN_HANDLER, ORIGIN};
use crate::timing;
use crate::wake_trace::{self, Source};
use crate::wndproc::{hwnd_from, WM_WAKE};
use aqua_protocol::Instant;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{HANDLE, LPARAM, LRESULT, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM};
use windows::Win32::System::Threading::{
    CancelWaitableTimer, CreateWaitableTimerExW, SetWaitableTimerEx,
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
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
        post_wake(self.hwnd);
    }
}

static WAKE_PENDING: AtomicBool = AtomicBool::new(false);

pub(crate) fn post_wake(hwnd: isize) {
    if WAKE_PENDING.swap(true, Ordering::AcqRel) {
        return;
    }
    let posted = unsafe {
        PostMessageW(
            Some(hwnd_from(hwnd)),
            WM_WAKE,
            Default::default(),
            Default::default(),
        )
    };
    if posted.is_err() {
        WAKE_PENDING.store(false, Ordering::Release);
    }
}

pub(crate) fn wake_delivered() {
    WAKE_PENDING.store(false, Ordering::Release);
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
    if wake_trace::hooked() {
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

#[derive(Clone, Copy)]
enum MessageWait {
    Inactive,
    Waiting { wake_at: Option<Instant> },
    Notified,
}

thread_local! {
    static WATCH: std::cell::RefCell<crate::occlusion::Watch> =
        std::cell::RefCell::new(crate::occlusion::Watch::default());
    static MESSAGE_WAIT: std::cell::Cell<MessageWait> =
        const { std::cell::Cell::new(MessageWait::Inactive) };
}

pub(crate) fn message_waiting() -> bool {
    matches!(MESSAGE_WAIT.get(), MessageWait::Waiting { .. })
}

pub(crate) fn message_wait_needs_wake(wake_at: Option<Instant>, pending: bool) -> bool {
    match MESSAGE_WAIT.get() {
        MessageWait::Waiting { wake_at: previous } => pending || wake_at != previous,
        _ => false,
    }
}

pub(crate) fn wake_message_wait() {
    if message_waiting() {
        let posted = unsafe {
            PostThreadMessageW(
                windows::Win32::System::Threading::GetCurrentThreadId(),
                WM_NULL,
                WPARAM(0),
                LPARAM(0),
            )
        };
        if posted.is_ok() {
            MESSAGE_WAIT.set(MessageWait::Notified);
        }
    }
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
    wake_trace::timed("watch", || watch_occlusion(pending));
    wake_trace::timed("pump", crate::occlusion::pump);
    crate::gaps::turn_started();
    IN_HANDLER.set(true);
    let handler = unsafe { &mut *handler };
    let mut worked = false;
    while let Some(item) = state::with(|s| s.queue.pop_front()) {
        worked = true;
        match item {
            Queued::Window(id, event) => {
                wake_trace::timed("window", || handler.window_event(id, event))
            }
            Queued::Waitable(index) => wake_trace::timed("waitable", || handler.waitable(index)),
            Queued::Wake => wake_trace::timed("woken", || handler.woken()),
        }
    }
    let current = now();
    let due = state::with(|s| timing::due(current, s.wake_at));
    if worked || due {
        let next = wake_trace::timed("tick", || handler.tick(current));
        state::with(|s| s.wake_at = next);
    }
    let redraws: Vec<_> = state::with(|s| {
        s.windows
            .values_mut()
            .filter_map(|w| std::mem::take(&mut w.redraw_pending).then_some(w.id))
            .collect()
    });
    for id in redraws {
        wake_trace::timed("redraw", || handler.window_event(id, ShellEvent::Redraw));
    }
    crate::gaps::turn_ended();
    IN_HANDLER.set(false);
    watch_occlusion(false);
}

fn redraw_pending() -> bool {
    state::with(|s| s.windows.values().any(|w| w.redraw_pending) || !s.queue.is_empty())
}

fn dispatch(message: &MSG) {
    wake_trace::record(Source::Message(message.message));
    let consumed = crate::hooks::pre_translate(&crate::hooks::RawMessage {
        hwnd: message.hwnd.0 as isize,
        message: message.message,
        wparam: message.wParam.0,
        lparam: message.lParam.0,
    });
    if consumed {
        state::with(|s| s.queue.push_back(Queued::Wake));
    } else {
        wake_trace::dispatching(true);
        unsafe {
            let _ = TranslateMessage(message);
            DispatchMessageW(message);
        }
        wake_trace::dispatching(false);
    }
}

struct MessageTimer(usize);

impl MessageTimer {
    fn arm(current: Instant, wake_at: Option<Instant>) -> Option<Self> {
        let Some(at) = wake_at else {
            return Some(Self(0));
        };
        let timing::TimerPlan::Coarse { leeway_ms } = timing::plan(current, at) else {
            return None;
        };
        let after_ms = timing::timeout_ms(current, Some(at)).min(USER_TIMER_MAXIMUM - leeway_ms);
        let id = unsafe { SetCoalescableTimer(None, 0, after_ms, None, leeway_ms) };
        (id != 0).then_some(Self(id))
    }
}

impl Drop for MessageTimer {
    fn drop(&mut self) {
        if self.0 != 0 {
            let _ = unsafe { KillTimer(None, self.0) };
        }
    }
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
    let coarse = unsafe { CreateWaitableTimerExW(None, None, 0, TIMER_ALL_ACCESS) }.ok();
    let mut armed: Option<Instant> = None;
    let mut outcome = Ok(());
    loop {
        let mut msg = MSG::default();
        let mut quit = false;
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            if msg.message == WM_QUIT {
                quit = true;
                break;
            }
            dispatch(&msg);
        }
        if quit {
            break;
        }
        turn();
        wake_trace::flush_if_due();
        crate::null_trace::flush_if_due();
        let current = now();
        let (handler_wake, waitables) = state::with(|s| (s.wake_at, s.waitables.clone()));
        let wake_at = [handler_wake, watch_deadline()].into_iter().flatten().min();
        if waitables.is_empty() && !redraw_pending() {
            if let Some(message_timer) = MessageTimer::arm(current, wake_at) {
                if armed.take().is_some() {
                    for timer in timer.iter().chain(coarse.iter()) {
                        let _ = unsafe { CancelWaitableTimer(*timer) };
                    }
                }
                MESSAGE_WAIT.set(MessageWait::Waiting {
                    wake_at: handler_wake,
                });
                wake_trace::record(Source::MessageWait {
                    timed: wake_at.is_some(),
                });
                let received = unsafe { GetMessageW(&mut msg, None, 0, 0) };
                MESSAGE_WAIT.set(MessageWait::Inactive);
                match received.0 {
                    -1 => {
                        outcome = Err(win32("GetMessageW", windows::core::Error::from_thread()));
                        break;
                    }
                    0 => break,
                    _ => {
                        drop(message_timer);
                        crate::null_trace::waited();
                        dispatch(&msg);
                    }
                }
                continue;
            }
        }
        if armed != wake_at {
            let plan = wake_at.map(|at| timing::plan(current, at));
            let arm = |timer: Option<HANDLE>, leeway_ms: Option<u32>| {
                let Some(timer) = timer else {
                    return;
                };
                match (wake_at, leeway_ms) {
                    (Some(at), Some(leeway)) => {
                        let due = timing::relative_due_time_100ns(current, at);
                        let _ =
                            unsafe { SetWaitableTimerEx(timer, &due, 0, None, None, None, leeway) };
                    }
                    _ => {
                        let _ = unsafe { CancelWaitableTimer(timer) };
                    }
                }
            };
            let (precise_leeway, coarse_leeway) = match plan {
                Some(timing::TimerPlan::Precise) => (Some(0), None),
                Some(timing::TimerPlan::Coarse { leeway_ms }) if coarse.is_some() => {
                    (None, Some(leeway_ms))
                }
                Some(timing::TimerPlan::Coarse { .. }) => (Some(0), None),
                None => (None, None),
            };
            arm(timer, precise_leeway);
            arm(coarse, coarse_leeway);
            armed = wake_at;
        }
        let mut handles: Vec<HANDLE> = Vec::with_capacity(waitables.len() + 1);
        timer
            .iter()
            .chain(coarse.iter())
            .for_each(|t| handles.push(*t));
        let offset = handles.len();
        handles.extend(waitables.iter().map(|h| HANDLE(*h as *mut _)));
        let timers = offset;
        let timeout = match (redraw_pending(), timer.is_some()) {
            (true, _) => 0,
            (false, true) => INFINITE_WAIT,
            (false, false) => timing::timeout_ms(current, wake_at),
        };
        let waited = unsafe {
            MsgWaitForMultipleObjectsEx(Some(&handles), timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
        crate::null_trace::waited();
        let index = waited.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
        let timer_fired = index < timers;
        match (timer_fired, index >= offset && index < handles.len()) {
            (true, _) => wake_trace::record(Source::Timer),
            (_, true) => wake_trace::record(Source::Waitable),
            _ if waited.0 == WAIT_TIMEOUT.0 => wake_trace::record(Source::Timeout),
            _ if wake_trace::enabled() && index == handles.len() => {
                let status = unsafe { GetQueueStatus(QS_ALLINPUT) };
                wake_trace::record(Source::Queue(status >> 16));
            }
            _ => wake_trace::record(Source::Other(waited.0)),
        }
        if timer_fired {
            armed = None;
        }
        if index < handles.len() && index >= offset {
            state::with(|s| s.queue.push_back(Queued::Waitable(index - offset)));
        }
    }
    HANDLER.set(None);
    drop(boxed);
    outcome
}

const INFINITE_WAIT: u32 = u32::MAX;
