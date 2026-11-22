#![cfg(target_os = "linux")]

mod support;

use aqua_protocol::{Instant, WindowId};
use aqua_shell_wayland::{now, quit, run, schedule, set_waitables, Handler, ShellEvent, Window};
use rustix::event::{eventfd, EventfdFlags};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

struct Deadline {
    target: Instant,
    fired: Rc<Cell<Option<Instant>>>,
    started: bool,
}

impl Handler for Deadline {
    fn window_event(&mut self, _: WindowId, _: ShellEvent) {}

    fn tick(&mut self, at: Instant) -> Option<Instant> {
        match std::mem::replace(&mut self.started, true) {
            false => Some(self.target),
            true if at >= self.target => {
                self.fired.set(Some(at));
                quit();
                None
            }
            true => Some(self.target),
        }
    }
}

fn lateness(ahead: Duration) -> Duration {
    let target = now() + ahead;
    let fired = Rc::new(Cell::new(None));
    schedule(now());
    run(Deadline {
        target,
        fired: fired.clone(),
        started: false,
    })
    .expect("the loop runs");
    fired
        .get()
        .expect("the deadline fired")
        .saturating_duration_since(target)
}

#[test]
fn a_coarse_deadline_fires_without_external_messages() {
    let Some(_compositor) = support::compositor() else {
        return;
    };
    let late = lateness(Duration::from_millis(400));
    assert!(
        late < Duration::from_millis(40 + 25),
        "a 400 ms deadline has 40 ms of leeway, fired {late:?} late"
    );
}

#[test]
fn a_precise_deadline_fires_within_two_milliseconds() {
    let Some(_compositor) = support::compositor() else {
        return;
    };
    let mut samples: Vec<Duration> = (0..50)
        .map(|_| lateness(Duration::from_millis(8)))
        .collect();
    samples.sort();
    let median = samples[samples.len() / 2];
    let p90 = samples[samples.len() * 90 / 100 - 1];
    assert!(
        median < Duration::from_micros(500) && p90 < Duration::from_millis(2),
        "median lateness {median:?}, p90 {p90:?}, samples {samples:?}"
    );
}

struct Counting {
    window: Window,
    woken: Rc<Cell<u32>>,
    ticks: u32,
    deadline: Instant,
}

impl Handler for Counting {
    fn window_event(&mut self, _: WindowId, _: ShellEvent) {}

    fn woken(&mut self) {
        self.woken.set(self.woken.get() + 1);
    }

    fn tick(&mut self, at: Instant) -> Option<Instant> {
        assert!(at < self.deadline, "script timed out");
        self.ticks += 1;
        match self.ticks {
            1 => {
                let waker = self.window.waker();
                (0..1000).for_each(|_| waker.wake());
                Some(at + Duration::from_millis(50))
            }
            2 => {
                let waker = self.window.waker();
                std::thread::spawn(move || waker.wake())
                    .join()
                    .expect("the waking thread finishes");
                Some(at + Duration::from_millis(50))
            }
            _ => {
                quit();
                None
            }
        }
    }
}

#[test]
fn a_waker_from_another_thread_wakes_the_loop_once_per_burst() {
    let Some(_compositor) = support::compositor() else {
        return;
    };
    let window = Window::create(&aqua_shell_wayland::WindowOptions {
        visible: false,
        ..support::options("com.aqua.test.wake", (100.0, 100.0))
    })
    .expect("window");
    let woken = Rc::new(Cell::new(0));
    run(Counting {
        window,
        woken: woken.clone(),
        ticks: 0,
        deadline: now() + Duration::from_secs(20),
    })
    .expect("loop");
    assert_eq!(woken.get(), 2);
}

struct Waiting {
    deadline: Instant,
    signalled: Rc<RefCell<Option<Instant>>>,
    started: bool,
}

impl Handler for Waiting {
    fn window_event(&mut self, _: WindowId, _: ShellEvent) {}

    fn waitable(&mut self, index: usize) {
        assert_eq!(index, 0);
        *self.signalled.borrow_mut() = Some(now());
        quit();
    }

    fn tick(&mut self, at: Instant) -> Option<Instant> {
        match std::mem::replace(&mut self.started, true) {
            false => Some(self.deadline),
            true if at >= self.deadline => {
                quit();
                None
            }
            true => Some(self.deadline),
        }
    }
}

#[test]
fn external_fds_interrupt_a_coarse_deadline() {
    let Some(_compositor) = support::compositor() else {
        return;
    };
    let fd = eventfd(0, EventfdFlags::CLOEXEC).expect("eventfd");
    let writer = fd.try_clone().expect("dup");
    set_waitables(vec![fd]);
    let started = now();
    let signaller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        rustix::io::write(&writer, &1u64.to_ne_bytes()).expect("signal");
    });
    let signalled = Rc::new(RefCell::new(None));
    schedule(now());
    run(Waiting {
        deadline: now() + Duration::from_secs(5),
        signalled: signalled.clone(),
        started: false,
    })
    .expect("loop");
    signaller.join().expect("signaller");
    set_waitables(Vec::new());
    let at = signalled.borrow().expect("the fd interrupted the wait");
    let waited = at.saturating_duration_since(started);
    assert!(
        waited < Duration::from_millis(1000),
        "the waitable took {waited:?} to interrupt a 5 s deadline"
    );
}
