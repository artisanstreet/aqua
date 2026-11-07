#![cfg(windows)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

use aqua_core::InputEvent;
use aqua_protocol::{Instant, WindowId};
use aqua_shell_win32::{now, quit, run, set_waitables, Handler, ShellEvent, Window, WindowOptions};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::{CreateEventW, GetCurrentThreadId, SetEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, PostThreadMessageW, SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_CHAR, WM_NULL,
    WM_QUIT,
};

enum Phase {
    Starting(Sender<()>),
    Waiting,
    Done,
}

struct Idle {
    phase: Phase,
    events: Rc<RefCell<Vec<String>>>,
    completed: Sender<()>,
    input: Sender<()>,
    deadline: Option<Instant>,
}

impl Handler for Idle {
    fn window_event(&mut self, _: WindowId, event: ShellEvent) {
        if let ShellEvent::Input(InputEvent::TextInput { text }) = event {
            if text == "x" {
                let _ = self.input.send(());
            }
            self.events.borrow_mut().push(text);
        }
    }

    fn woken(&mut self) {
        if matches!(self.phase, Phase::Waiting) {
            self.events.borrow_mut().push("wake".into());
            self.phase = Phase::Done;
            let _ = self.completed.send(());
            quit();
        }
    }

    fn tick(&mut self, at: Instant) -> Option<Instant> {
        if matches!(self.phase, Phase::Starting(_)) {
            let Phase::Starting(started) = std::mem::replace(&mut self.phase, Phase::Waiting)
            else {
                unreachable!()
            };
            let _ = started.send(());
        }
        Some(*self.deadline.get_or_insert(at + Duration::from_secs(30)))
    }
}

#[test]
fn sent_messages_and_the_first_posted_input_survive_a_coarse_message_wait() {
    let window = Window::create(&WindowOptions {
        title: "aqua-idle-wait".into(),
        logical_size: (100.0, 100.0),
        visible: false,
        ..Default::default()
    })
    .unwrap();
    let raw = window.id().raw();
    let thread = unsafe { GetCurrentThreadId() };
    let waker = window.waker();
    let (started, ready) = channel();
    let (completed, completion) = channel();
    let (input, input_seen) = channel();
    let worker = std::thread::spawn(move || {
        let ready = ready.recv_timeout(Duration::from_secs(2)).is_ok();
        std::thread::sleep(Duration::from_secs(2));
        let hwnd = HWND(raw as *mut _);
        let sent = ready
            && (0..100).all(|_| {
                unsafe {
                    SendMessageTimeoutW(
                        hwnd,
                        WM_NULL,
                        WPARAM(0),
                        LPARAM(0),
                        SMTO_ABORTIFHUNG,
                        1000,
                        None,
                    )
                }
                .0 != 0
            });
        let text_sent = unsafe {
            SendMessageTimeoutW(
                hwnd,
                WM_CHAR,
                WPARAM(b'x' as usize),
                LPARAM(1),
                SMTO_ABORTIFHUNG,
                1000,
                None,
            )
        }
        .0 != 0;
        let delivered = input_seen.recv_timeout(Duration::from_secs(2)).is_ok();
        let posted =
            unsafe { PostMessageW(Some(hwnd), WM_CHAR, WPARAM(b'y' as usize), LPARAM(1)) }.is_ok();
        waker.wake();
        let completed = completion.recv_timeout(Duration::from_secs(2)).is_ok();
        if !completed {
            let _ = unsafe { PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
        sent && text_sent && delivered && posted && completed
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    waker.wake();
    let result = run(Idle {
        phase: Phase::Starting(started),
        events: events.clone(),
        completed,
        input,
        deadline: None,
    });
    window.destroy();
    assert!(worker.join().unwrap());
    result.unwrap();
    assert_eq!(*events.borrow(), ["x", "y", "wake"]);
}

enum TimerPhase {
    Starting,
    Armed(Instant),
    Fired,
}

struct Deadline {
    phase: TimerPhase,
    fired: Rc<Cell<bool>>,
    completed: Sender<()>,
}

impl Handler for Deadline {
    fn window_event(&mut self, _: WindowId, _: ShellEvent) {}

    fn tick(&mut self, at: Instant) -> Option<Instant> {
        match self.phase {
            TimerPhase::Starting => {
                let deadline = at + Duration::from_millis(350);
                self.phase = TimerPhase::Armed(deadline);
                Some(deadline)
            }
            TimerPhase::Armed(deadline) if at >= deadline => {
                self.phase = TimerPhase::Fired;
                self.fired.set(true);
                let _ = self.completed.send(());
                quit();
                None
            }
            TimerPhase::Armed(deadline) => Some(deadline),
            TimerPhase::Fired => None,
        }
    }
}

#[test]
fn a_coarse_deadline_fires_without_external_messages() {
    let window = Window::create(&WindowOptions {
        visible: false,
        ..Default::default()
    })
    .unwrap();
    let thread = unsafe { GetCurrentThreadId() };
    let (completed, completion) = channel();
    let watchdog = std::thread::spawn(move || {
        let fired = completion.recv_timeout(Duration::from_secs(3)).is_ok();
        if !fired {
            let _ = unsafe { PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
        fired
    });
    let fired = Rc::new(Cell::new(false));
    window.waker().wake();
    let result = run(Deadline {
        phase: TimerPhase::Starting,
        fired: fired.clone(),
        completed,
    });
    window.destroy();
    assert!(watchdog.join().unwrap());
    result.unwrap();
    assert!(fired.get());
}

struct ExternalWait {
    completed: Sender<usize>,
}

impl Handler for ExternalWait {
    fn window_event(&mut self, _: WindowId, _: ShellEvent) {}

    fn waitable(&mut self, index: usize) {
        let _ = self.completed.send(index);
        set_waitables(Vec::new());
        quit();
    }

    fn tick(&mut self, _: Instant) -> Option<Instant> {
        Some(now() + Duration::from_secs(1))
    }
}

#[test]
fn external_waitables_still_interrupt_a_coarse_deadline() {
    let window = Window::create(&WindowOptions {
        visible: false,
        ..Default::default()
    })
    .unwrap();
    let event = unsafe { CreateEventW(None, true, false, None) }.unwrap();
    set_waitables(vec![event.0 as isize]);
    let raw_event = event.0 as isize;
    let thread = unsafe { GetCurrentThreadId() };
    let (completed, completion) = channel();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        unsafe { SetEvent(HANDLE(raw_event as *mut _)) }.unwrap();
        let index = completion.recv_timeout(Duration::from_secs(2));
        if index.is_err() {
            let _ = unsafe { PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
        index
    });
    window.waker().wake();
    let result = run(ExternalWait { completed });
    set_waitables(Vec::new());
    window.destroy();
    unsafe { CloseHandle(event) }.unwrap();
    result.unwrap();
    assert_eq!(worker.join().unwrap().unwrap(), 0);
}
