use crate::state;
use crate::wake_trace::{self, Source};
use aqua_protocol::{Instant, WindowId};
use aqua_shell::timing::{self, TimerPlan};
use aqua_shell::{Handler, ShellError, ShellEvent};
use rustix::event::{eventfd, poll, EventfdFlags, PollFd, PollFlags};
use rustix::time::{
    timerfd_create, timerfd_settime, Itimerspec, TimerfdClockId, TimerfdFlags, TimerfdTimerFlags,
    Timespec,
};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::num::NonZeroU64;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

pub(crate) enum Queued {
    Window(WindowId, ShellEvent),
    Waitable(usize),
    Wake,
}

#[derive(Default)]
struct LoopState {
    queue: VecDeque<Queued>,
    wake_at: Option<Instant>,
    waitables: Vec<OwnedFd>,
    quit: bool,
    failure: Option<ShellError>,
}

thread_local! {
    static LOOP: RefCell<LoopState> = RefCell::new(LoopState::default());
    static RUNNING: Cell<bool> = const { Cell::new(false) };
    static LAST_PUMP: Cell<Option<std::time::Instant>> = const { Cell::new(None) };
}

static WAKE_FD: OnceLock<Option<OwnedFd>> = OnceLock::new();
static WAKE_PENDING: AtomicBool = AtomicBool::new(false);

fn wake_fd() -> Option<&'static OwnedFd> {
    WAKE_FD
        .get_or_init(|| eventfd(0, EventfdFlags::NONBLOCK | EventfdFlags::CLOEXEC).ok())
        .as_ref()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Waker {
    _private: (),
}

impl Waker {
    pub(crate) fn new() -> Self {
        Self { _private: () }
    }

    pub fn wake(&self) {
        if WAKE_PENDING.swap(true, Ordering::AcqRel) {
            return;
        }
        let written = wake_fd()
            .map(|fd| rustix::io::write(fd, &1u64.to_ne_bytes()).is_ok())
            .unwrap_or(false);
        if !written {
            WAKE_PENDING.store(false, Ordering::Release);
        }
    }
}

fn drain_wake() -> bool {
    let mut buffer = [0u8; 8];
    let read = wake_fd().is_some_and(|fd| rustix::io::read(fd, &mut buffer).is_ok());
    WAKE_PENDING.store(false, Ordering::Release);
    read
}

pub fn now() -> Instant {
    crate::clock::now()
}

pub fn schedule(at: Instant) {
    LOOP.with_borrow_mut(|state| {
        state.wake_at = Some(state.wake_at.map_or(at, |existing| existing.min(at)))
    });
}

pub fn quit() {
    LOOP.with_borrow_mut(|state| state.quit = true);
}

pub fn set_waitables(fds: Vec<OwnedFd>) {
    LOOP.with_borrow_mut(|state| state.waitables = fds);
}

pub(crate) fn wake_turn() {
    LOOP.with_borrow_mut(|state| state.queue.push_back(Queued::Wake));
}

pub(crate) fn emit(id: WindowId, event: ShellEvent) {
    LOOP.with_borrow_mut(|state| state.queue.push_back(Queued::Window(id, event)));
}

const PUMP_SPACING: Duration = Duration::from_micros(300);

pub fn pump_sent_messages(_location: &'static std::panic::Location<'static>) {
    if !RUNNING.get() || !state::connected() {
        return;
    }
    let started = std::time::Instant::now();
    if LAST_PUMP
        .get()
        .is_some_and(|last| started.duration_since(last) < PUMP_SPACING)
    {
        return;
    }
    LAST_PUMP.set(Some(started));
    let pumped = read_socket(Some(Duration::ZERO)).and_then(|()| state::dispatch_pending());
    match pumped {
        Ok(count) if count > 0 => wake_trace::record(Source::Pumped),
        Ok(_) => {}
        Err(error) => LOOP.with_borrow_mut(|state| state.failure = Some(error)),
    }
}

pub(crate) fn refresh() -> Result<(), ShellError> {
    match state::connected() {
        true => {
            read_socket(Some(Duration::ZERO)).and_then(|()| state::dispatch_pending().map(|_| ()))
        }
        false => Ok(()),
    }
}

fn wayland(request: &'static str, reason: impl std::fmt::Display) -> ShellError {
    ShellError::Wayland(request, reason.to_string())
}

fn read_guard(guard: wayland_client::backend::ReadEventsGuard) -> Result<(), ShellError> {
    match guard.read() {
        Ok(_) => Ok(()),
        Err(wayland_client::backend::WaylandError::Io(error))
            if error.kind() == std::io::ErrorKind::WouldBlock =>
        {
            Ok(())
        }
        Err(error) => Err(wayland("wl_display_read_events", error)),
    }
}

pub(crate) fn read_socket(timeout: Option<Duration>) -> Result<(), ShellError> {
    state::flush()?;
    let Some(guard) = state::prepare_read() else {
        return Ok(());
    };
    let readable = {
        let mut fds = [PollFd::from_borrowed_fd(
            guard.connection_fd(),
            PollFlags::IN,
        )];
        let timeout = timeout.map(crate::clock::timespec);
        match poll(&mut fds, timeout.as_ref()) {
            Ok(_) => fds[0]
                .revents()
                .intersects(PollFlags::IN | PollFlags::ERR | PollFlags::HUP),
            Err(rustix::io::Errno::INTR) => false,
            Err(error) => return Err(wayland("poll", error)),
        }
    };
    match readable {
        true => read_guard(guard),
        false => Ok(()),
    }
}

pub(crate) fn wait_until(
    limit: Duration,
    request: &'static str,
    mut done: impl FnMut() -> Option<bool>,
) -> Result<(), ShellError> {
    let deadline = std::time::Instant::now() + limit;
    loop {
        state::dispatch_pending()?;
        match done() {
            Some(true) => return Ok(()),
            None => return Err(wayland(request, "the window was destroyed while waiting")),
            Some(false) => {}
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(wayland(
                request,
                format!("no answer from the compositor within {limit:?}"),
            ));
        }
        read_socket(Some(remaining))?;
    }
}

fn redraw_pending() -> bool {
    state::try_with(|shell| shell.windows.values().any(|window| window.redraw_pending))
        .unwrap_or(false)
}

fn work_pending() -> bool {
    LOOP.with_borrow(|state| !state.queue.is_empty()) || redraw_pending()
}

fn turn(handler: &mut dyn Handler) {
    let current = now();
    state::try_with(|shell| {
        crate::input::fire_repeats(shell, current);
        crate::occlusion::step(shell, current);
    });
    let mut worked = false;
    while let Some(item) = LOOP.with_borrow_mut(|state| state.queue.pop_front()) {
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
    let due = LOOP.with_borrow(|state| timing::due(current, state.wake_at));
    if worked || due {
        let next = wake_trace::timed("tick", || handler.tick(current));
        LOOP.with_borrow_mut(|state| state.wake_at = next);
    }
    let redraws: Vec<WindowId> = state::try_with(|shell| {
        shell
            .windows
            .values_mut()
            .filter_map(|window| std::mem::take(&mut window.redraw_pending).then_some(window.id))
            .collect()
    })
    .unwrap_or_default();
    for id in redraws {
        wake_trace::timed("redraw", || handler.window_event(id, ShellEvent::Redraw));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wait {
    Now,
    Forever,
    Precise(Instant),
    Coarse { at: Instant, leeway_ms: u32 },
}

fn plan(current: Instant, deadline: Option<Instant>) -> Wait {
    match (work_pending(), deadline) {
        (true, _) => Wait::Now,
        (false, None) => Wait::Forever,
        (false, Some(at)) if at <= current => Wait::Now,
        (false, Some(at)) => match timing::plan(current, at) {
            TimerPlan::Precise => Wait::Precise(at),
            TimerPlan::Coarse { leeway_ms } => Wait::Coarse { at, leeway_ms },
        },
    }
}

struct Timer {
    fd: OwnedFd,
    armed: Option<Instant>,
}

impl Timer {
    fn new() -> Result<Self, ShellError> {
        timerfd_create(
            TimerfdClockId::Monotonic,
            TimerfdFlags::NONBLOCK | TimerfdFlags::CLOEXEC,
        )
        .map(|fd| Self { fd, armed: None })
        .map_err(|error| wayland("timerfd_create", error))
    }

    fn set(&mut self, at: Option<Instant>) {
        if self.armed == at {
            return;
        }
        let zero = Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let value = at.map_or(zero, crate::clock::to_monotonic);
        let _ = timerfd_settime(
            &self.fd,
            TimerfdTimerFlags::ABSTIME,
            &Itimerspec {
                it_interval: zero,
                it_value: value,
            },
        );
        self.armed = at;
    }

    fn fired(&mut self) {
        let mut buffer = [0u8; 8];
        let _ = rustix::io::read(&self.fd, &mut buffer);
        self.armed = None;
    }
}

struct Slack(bool);

impl Slack {
    fn set(leeway_ms: u32) -> Self {
        let nanos = NonZeroU64::new(u64::from(leeway_ms) * 1_000_000);
        Slack(
            nanos.is_some_and(|nanos| rustix::thread::set_current_timer_slack(Some(nanos)).is_ok()),
        )
    }
}

impl Drop for Slack {
    fn drop(&mut self) {
        if self.0 {
            let _ = rustix::thread::set_current_timer_slack(None);
        }
    }
}

const SOCKET: usize = 0;
const WAKER: usize = 1;
const TIMER: usize = 2;
const WAITABLES: usize = 3;

fn block(timer: &mut Timer, wait: Wait, current: Instant) -> Result<(), ShellError> {
    let Some(guard) = state::prepare_read() else {
        return Ok(());
    };
    let (timeout, _slack) = match wait {
        Wait::Now => (Some(Duration::ZERO), None),
        Wait::Forever => {
            timer.set(None);
            (None, None)
        }
        Wait::Precise(at) => {
            timer.set(Some(at));
            (None, None)
        }
        Wait::Coarse { at, leeway_ms } => {
            timer.set(None);
            let millis = timing::timeout_ms(current, Some(at));
            (
                Some(Duration::from_millis(u64::from(millis))),
                Some(Slack::set(leeway_ms)),
            )
        }
    };
    let ready = LOOP.with_borrow(|state| {
        let waker = wake_fd().expect("the waker eventfd exists while the loop runs");
        let mut fds: Vec<PollFd<'_>> = [guard.connection_fd(), waker.as_fd(), timer.fd.as_fd()]
            .into_iter()
            .map(|fd| PollFd::from_borrowed_fd(fd, PollFlags::IN))
            .chain(
                state
                    .waitables
                    .iter()
                    .map(|fd| PollFd::new(fd, PollFlags::IN)),
            )
            .collect();
        let timeout = timeout.map(crate::clock::timespec);
        match poll(&mut fds, timeout.as_ref()) {
            Ok(_) => Ok(fds
                .iter()
                .map(|fd| {
                    fd.revents()
                        .intersects(PollFlags::IN | PollFlags::ERR | PollFlags::HUP)
                })
                .collect::<Vec<bool>>()),
            Err(rustix::io::Errno::INTR) => Ok(Vec::new()),
            Err(error) => Err(wayland("poll", error)),
        }
    })?;
    let fired = |index: usize| ready.get(index).copied().unwrap_or(false);
    match fired(SOCKET) {
        true => {
            wake_trace::record(Source::Wayland);
            read_guard(guard)?;
        }
        false => drop(guard),
    }
    if fired(WAKER) && drain_wake() {
        wake_trace::record(Source::Waker);
        LOOP.with_borrow_mut(|state| state.queue.push_back(Queued::Wake));
    }
    if fired(TIMER) {
        wake_trace::record(Source::Timer);
        timer.fired();
    }
    let waitables: Vec<usize> = (WAITABLES..ready.len())
        .filter(|index| fired(*index))
        .map(|index| index - WAITABLES)
        .collect();
    if !waitables.is_empty() {
        wake_trace::record(Source::Waitable);
    }
    LOOP.with_borrow_mut(|state| {
        state
            .queue
            .extend(waitables.into_iter().map(Queued::Waitable))
    });
    if ready.iter().all(|ready| !ready) && timeout.is_some() {
        wake_trace::record(Source::Timeout);
    }
    Ok(())
}

struct Running;

impl Running {
    fn enter() -> Result<Self, ShellError> {
        match RUNNING.replace(true) {
            true => Err(ShellError::LoopAlreadyRunning),
            false => Ok(Running),
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        RUNNING.set(false);
        LOOP.with_borrow_mut(|state| {
            state.quit = false;
            state.queue.clear();
            state.wake_at = None;
        });
    }
}

pub fn run<H: Handler>(mut handler: H) -> Result<(), ShellError> {
    let _running = Running::enter()?;
    state::ensure()?;
    if wake_fd().is_none() {
        return Err(wayland("eventfd", "the waker could not be created"));
    }
    drain_wake();
    wake_trace::enable_from_env();
    let mut timer = Timer::new()?;
    loop {
        if let Some(failure) = LOOP.with_borrow_mut(|state| state.failure.take()) {
            return Err(failure);
        }
        state::dispatch_pending()?;
        turn(&mut handler);
        wake_trace::flush_if_due();
        if LOOP.with_borrow(|state| state.quit) {
            return Ok(());
        }
        state::flush()?;
        let current = now();
        let deadline = [
            LOOP.with_borrow(|state| state.wake_at),
            state::try_with(crate::input::repeat_deadline).flatten(),
            state::try_with(|shell| crate::occlusion::deadline(shell)).flatten(),
        ]
        .into_iter()
        .flatten()
        .min();
        block(&mut timer, plan(current, deadline), current)?;
    }
}
