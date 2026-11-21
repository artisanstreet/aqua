use crate::wake_trace::{self, Source};
use aqua_protocol::{Instant, WindowId};
use aqua_shell::timing::{self, TimerPlan};
use aqua_shell::{Handler, ShellError, ShellEvent};
use objc2_app_kit::{NSEventTrackingRunLoopMode, NSModalPanelRunLoopMode};
use objc2_core_foundation::{
    kCFRunLoopCommonModes, CFAbsoluteTimeGetCurrent, CFRetained, CFRunLoop, CFRunLoopMode,
    CFRunLoopSource, CFRunLoopSourceContext, CFRunLoopTimer,
};
use objc2_foundation::{MainThreadMarker, NSString};
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::panic::AssertUnwindSafe;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

pub(crate) enum Queued {
    Window(WindowId, ShellEvent),
    Wake,
}

#[derive(Default)]
struct LoopState {
    queue: VecDeque<Queued>,
    wake_at: Option<Instant>,
    quit: bool,
    panic: Option<Box<dyn Any + Send>>,
}

enum Phase {
    Idle,
    Running { handler: Box<dyn Handler> },
    InTurn,
}

thread_local! {
    static LOOP: RefCell<LoopState> = RefCell::new(LoopState::default());
    static PHASE: RefCell<Phase> = const { RefCell::new(Phase::Idle) };
    static TIMER: RefCell<Option<ArmedTimer>> = const { RefCell::new(None) };
    static STOPPING: Cell<bool> = const { Cell::new(false) };
}

struct WakeSource(NonNull<CFRunLoopSource>);

unsafe impl Send for WakeSource {}
unsafe impl Sync for WakeSource {}

static WAKE_SOURCE: OnceLock<WakeSource> = OnceLock::new();
static WAKE_PENDING: AtomicBool = AtomicBool::new(false);

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
        let source = WAKE_SOURCE
            .get()
            .expect("invariant: the wake source is installed before any Waker exists");
        unsafe { source.0.as_ref() }.signal();
        CFRunLoop::main()
            .expect("invariant: the process has a main run loop")
            .wake_up();
    }
}

unsafe extern "C-unwind" fn wake_perform(_info: *mut c_void) {
    WAKE_PENDING.store(false, Ordering::Release);
    wake_trace::record(Source::Waker);
    LOOP.with_borrow_mut(|state| state.queue.push_back(Queued::Wake));
    drain();
}

unsafe extern "C-unwind" fn timer_fired(_timer: *mut CFRunLoopTimer, _info: *mut c_void) {
    wake_trace::record(Source::Timer);
    TIMER.with_borrow_mut(|timer| {
        if let Some(timer) = timer.as_mut() {
            timer.armed = None;
        }
    });
    drain();
}

struct ArmedTimer {
    timer: CFRetained<CFRunLoopTimer>,
    armed: Option<(Instant, u32)>,
}

const PARKED: f64 = 1.0e10;

impl ArmedTimer {
    fn arm(&mut self, at: Option<Instant>, leeway_ms: u32) {
        if self.armed == at.map(|at| (at, leeway_ms)) {
            return;
        }
        let fire = at.map_or(PARKED, |at| {
            CFAbsoluteTimeGetCurrent() + timing::until(now(), at).as_secs_f64()
        });
        unsafe { self.timer.set_tolerance(f64::from(leeway_ms) / 1000.0) };
        self.timer.set_next_fire_date(fire);
        self.armed = at.map(|at| (at, leeway_ms));
    }
}

pub(crate) fn install(_mtm: MainThreadMarker) {
    if WAKE_SOURCE.get().is_some() {
        return;
    }
    let main = CFRunLoop::main().expect("invariant: the process has a main run loop");
    for mode in unsafe { [NSEventTrackingRunLoopMode, NSModalPanelRunLoopMode] } {
        let bridged: &CFRunLoopMode =
            unsafe { &*(mode as *const NSString).cast::<CFRunLoopMode>() };
        main.add_common_mode(Some(bridged));
    }
    let modes = unsafe { kCFRunLoopCommonModes };
    let mut context = CFRunLoopSourceContext {
        version: 0,
        info: std::ptr::null_mut(),
        retain: None,
        release: None,
        copyDescription: None,
        equal: None,
        hash: None,
        schedule: None,
        cancel: None,
        perform: Some(wake_perform),
    };
    let source = unsafe { CFRunLoopSource::new(None, 0, &mut context) }
        .expect("invariant: CFRunLoopSourceCreate succeeds for a version 0 source");
    main.add_source(Some(&source), modes);
    let timer = unsafe {
        CFRunLoopTimer::new(
            None,
            CFAbsoluteTimeGetCurrent() + PARKED,
            PARKED,
            0,
            0,
            Some(timer_fired),
            std::ptr::null_mut(),
        )
    }
    .expect("invariant: CFRunLoopTimerCreate succeeds");
    main.add_timer(Some(&timer), modes);
    TIMER.set(Some(ArmedTimer { timer, armed: None }));
    let raw = CFRetained::into_raw(source);
    let _ = WAKE_SOURCE.set(WakeSource(raw.cast()));
}

pub fn now() -> Instant {
    crate::clock::now()
}

pub fn schedule(at: Instant) {
    LOOP.with_borrow_mut(|state| {
        state.wake_at = Some(state.wake_at.map_or(at, |existing| existing.min(at)))
    });
    if running() {
        rearm();
    }
}

pub fn quit() {
    LOOP.with_borrow_mut(|state| state.quit = true);
}

pub fn pump_sent_messages(_location: &'static std::panic::Location<'static>) {}

pub(crate) fn emit(id: WindowId, event: ShellEvent) {
    LOOP.with_borrow_mut(|state| state.queue.push_back(Queued::Window(id, event)));
}

pub(crate) fn running() -> bool {
    PHASE.with_borrow(|phase| !matches!(phase, Phase::Idle))
}

pub(crate) fn callback_done() {
    wake_trace::record(Source::AppKit);
    drain();
}

fn turn(handler: &mut dyn Handler) {
    let mut worked = false;
    while let Some(item) = LOOP.with_borrow_mut(|state| state.queue.pop_front()) {
        worked = true;
        match item {
            Queued::Window(id, event) => {
                wake_trace::timed("window", || handler.window_event(id, event))
            }
            Queued::Wake => wake_trace::timed("woken", || handler.woken()),
        }
    }
    let current = now();
    let due = LOOP.with_borrow(|state| timing::due(current, state.wake_at));
    if worked || due {
        let next = wake_trace::timed("tick", || handler.tick(current));
        LOOP.with_borrow_mut(|state| state.wake_at = next);
    }
    for id in crate::state::take_redraws() {
        wake_trace::timed("redraw", || handler.window_event(id, ShellEvent::Redraw));
    }
}

fn drain() {
    let taken = PHASE.with_borrow_mut(|phase| match std::mem::replace(phase, Phase::InTurn) {
        Phase::Running { handler } => Some(handler),
        other => {
            *phase = other;
            None
        }
    });
    let Some(mut handler) = taken else {
        return;
    };
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| turn(handler.as_mut())));
    PHASE.set(Phase::Running { handler });
    if let Err(payload) = outcome {
        LOOP.with_borrow_mut(|state| {
            state.panic.get_or_insert(payload);
            state.quit = true;
        });
    }
    wake_trace::flush_if_due();
    match LOOP.with_borrow(|state| state.quit) {
        true => stop(),
        false => rearm(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wait {
    Now,
    Forever,
    Until { at: Instant, leeway_ms: u32 },
}

fn plan(current: Instant, pending: bool, deadline: Option<Instant>) -> Wait {
    match (pending, deadline) {
        (true, _) => Wait::Now,
        (false, None) => Wait::Forever,
        (false, Some(at)) if at <= current => Wait::Now,
        (false, Some(at)) => match timing::plan(current, at) {
            TimerPlan::Precise => Wait::Until { at, leeway_ms: 0 },
            TimerPlan::Coarse { leeway_ms } => Wait::Until { at, leeway_ms },
        },
    }
}

fn rearm() {
    let current = now();
    let (pending, deadline) = LOOP.with_borrow(|state| {
        (
            !state.queue.is_empty() || crate::state::redraw_pending(),
            state.wake_at,
        )
    });
    TIMER.with_borrow_mut(|timer| {
        let timer = timer
            .as_mut()
            .expect("invariant: the loop timer is installed before the loop runs");
        match plan(current, pending, deadline) {
            Wait::Now => timer.arm(Some(current), 0),
            Wait::Forever => timer.arm(None, 0),
            Wait::Until { at, leeway_ms } => timer.arm(Some(at), leeway_ms),
        }
    });
}

fn stop() {
    if STOPPING.replace(true) {
        return;
    }
    crate::app::stop(MainThreadMarker::new().expect("invariant: the loop runs on the main thread"));
}

pub fn run<H: Handler>(handler: H) -> Result<(), ShellError> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Err(ShellError::AppKit(
            "NSApplication.run",
            "the AppKit loop must run on the main thread".into(),
        ));
    };
    let entered = PHASE.with_borrow_mut(|phase| match phase {
        Phase::Idle => {
            *phase = Phase::Running {
                handler: Box::new(handler),
            };
            true
        }
        _ => false,
    });
    if !entered {
        return Err(ShellError::LoopAlreadyRunning);
    }
    crate::app::ensure(mtm);
    install(mtm);
    wake_trace::enable_from_env();
    STOPPING.set(false);
    TIMER.with_borrow_mut(|timer| {
        timer
            .as_mut()
            .expect("invariant: install creates the loop timer")
            .arm(Some(now()), 0)
    });
    crate::app::run(mtm);
    PHASE.set(Phase::Idle);
    TIMER.with_borrow_mut(|timer| {
        if let Some(timer) = timer.as_mut() {
            timer.arm(None, 0);
        }
    });
    let panic = LOOP.with_borrow_mut(|state| {
        state.quit = false;
        state.queue.clear();
        state.wake_at = None;
        state.panic.take()
    });
    match panic {
        Some(payload) => std::panic::resume_unwind(payload),
        None => Ok(()),
    }
}
