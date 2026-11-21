#[cfg(target_os = "macos")]
mod support;

fn main() {
    #[cfg(target_os = "macos")]
    support::main(&cases::ALL);
    #[cfg(not(target_os = "macos"))]
    println!("looping: skipped; requires macOS");
}

#[cfg(target_os = "macos")]
mod cases {
    use super::support::{self, case, Case};
    use aqua_protocol::{Instant, WindowId};
    use aqua_shell_appkit::{now, quit, run, schedule, Handler, ShellError, ShellEvent, Window};
    use objc2_core_foundation::{
        kCFRunLoopDefaultMode, CFAbsoluteTimeGetCurrent, CFRunLoop, CFRunLoopTimer,
    };
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;

    pub const ALL: [Case; 6] = [
        case(
            "a_wake_from_another_thread_runs_woken_once_even_when_sent_twice",
            wake_dedup,
        ),
        case(
            "a_precise_deadline_fires_within_two_display_frames",
            precise_deadline,
        ),
        case("a_coarse_deadline_fires_within_its_leeway", coarse_deadline),
        case(
            "ticks_keep_running_during_a_live_resize_tracking_loop",
            tracking_loop,
        ),
        case(
            "quit_returns_from_run_and_the_loop_can_run_again",
            quit_and_rerun,
        ),
        case("run_refuses_to_start_twice", refuses_reentry),
    ];

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
            assert!(at < self.deadline, "the script timed out");
            self.ticks += 1;
            match self.ticks {
                1 => {
                    let waker = self.window.waker();
                    (0..1000).for_each(|_| waker.wake());
                    Some(at + Duration::from_millis(100))
                }
                2 => {
                    let waker = self.window.waker();
                    std::thread::spawn(move || {
                        waker.wake();
                        waker.wake();
                    })
                    .join()
                    .expect("the waking thread finishes");
                    Some(at + Duration::from_millis(100))
                }
                _ => {
                    quit();
                    None
                }
            }
        }
    }

    fn wake_dedup() {
        if !support::appkit() {
            return;
        }
        let window = Window::create(&aqua_shell_appkit::WindowOptions {
            visible: false,
            ..support::options("wake", (100.0, 100.0))
        })
        .expect("window");
        let woken = Rc::new(Cell::new(0));
        schedule(now());
        run(Counting {
            window,
            woken: woken.clone(),
            ticks: 0,
            deadline: now() + Duration::from_secs(20),
        })
        .expect("loop");
        assert_eq!(
            woken.get(),
            2,
            "one burst from the main thread and one from another thread"
        );
    }

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

    struct Deadlines {
        ahead: Duration,
        target: Option<Instant>,
        samples: Rc<std::cell::RefCell<Vec<Duration>>>,
        wanted: usize,
    }

    impl Handler for Deadlines {
        fn window_event(&mut self, _: WindowId, _: ShellEvent) {}

        fn tick(&mut self, at: Instant) -> Option<Instant> {
            match self.target {
                Some(target) if at < target => return Some(target),
                Some(target) => self
                    .samples
                    .borrow_mut()
                    .push(at.saturating_duration_since(target)),
                None => {}
            }
            match self.samples.borrow().len() >= self.wanted {
                true => {
                    quit();
                    None
                }
                false => {
                    let target = now() + self.ahead;
                    self.target = Some(target);
                    Some(target)
                }
            }
        }
    }

    fn precise_deadline() {
        if !support::appkit() {
            return;
        }
        let samples = Rc::new(std::cell::RefCell::new(Vec::new()));
        schedule(now());
        run(Deadlines {
            ahead: Duration::from_millis(8),
            target: None,
            samples: samples.clone(),
            wanted: 30,
        })
        .expect("the loop runs");
        let mut samples = samples.borrow().clone();
        samples.sort();
        let median = samples[samples.len() / 2];
        let p90 = samples[samples.len() * 9 / 10];
        println!("precise lateness: median {median:?}, p90 {p90:?}");
        assert!(
            median < Duration::from_millis(34) && p90 < Duration::from_millis(50),
            "a precise deadline is at most two display frames late on the runner: median {median:?}, p90 {p90:?}, samples {samples:?}"
        );
    }

    fn coarse_deadline() {
        if !support::appkit() {
            return;
        }
        let late = lateness(Duration::from_millis(400));
        println!("coarse lateness: {late:?}");
        assert!(
            late < Duration::from_millis(40 + 25),
            "a 400 ms deadline has 40 ms of leeway, fired {late:?} late"
        );
    }

    thread_local! {
        static TICKS: Cell<u32> = const { Cell::new(0) };
        static DURING: Cell<Option<u32>> = const { Cell::new(None) };
    }

    unsafe extern "C-unwind" fn track(_timer: *mut CFRunLoopTimer, _info: *mut std::ffi::c_void) {
        let before = TICKS.get();
        let started = std::time::Instant::now();
        aqua_shell_appkit::testing::track_events_for(Duration::from_millis(400));
        println!("the tracking loop ran for {:?}", started.elapsed());
        DURING.set(Some(TICKS.get() - before));
    }

    struct Tracking {
        armed: Option<objc2_core_foundation::CFRetained<CFRunLoopTimer>>,
        deadline: Instant,
    }

    impl Handler for Tracking {
        fn window_event(&mut self, _: WindowId, _: ShellEvent) {}

        fn tick(&mut self, at: Instant) -> Option<Instant> {
            assert!(at < self.deadline, "the script timed out");
            TICKS.set(TICKS.get() + 1);
            if self.armed.is_none() {
                let timer = unsafe {
                    CFRunLoopTimer::new(
                        None,
                        CFAbsoluteTimeGetCurrent(),
                        0.0,
                        0,
                        0,
                        Some(track),
                        std::ptr::null_mut(),
                    )
                }
                .expect("a one-shot timer");
                CFRunLoop::main()
                    .expect("main run loop")
                    .add_timer(Some(&timer), unsafe { kCFRunLoopDefaultMode });
                self.armed = Some(timer);
            }
            match DURING.get() {
                Some(_) => {
                    quit();
                    None
                }
                None => Some(at + Duration::from_millis(20)),
            }
        }
    }

    fn tracking_loop() {
        if !support::appkit() {
            return;
        }
        let _window =
            Window::create(&support::options("tracking", (200.0, 200.0))).expect("window");
        schedule(now());
        run(Tracking {
            armed: None,
            deadline: now() + Duration::from_secs(20),
        })
        .expect("loop");
        let during = DURING.get().expect("the tracking loop ran");
        println!("ticks during a 400 ms tracking loop: {during}");
        assert!(
            during >= 5,
            "only {during} ticks ran inside the tracking loop"
        );
    }

    struct QuitFirst {
        ticks: Rc<Cell<u32>>,
    }

    impl Handler for QuitFirst {
        fn window_event(&mut self, _: WindowId, _: ShellEvent) {}

        fn tick(&mut self, _: Instant) -> Option<Instant> {
            self.ticks.set(self.ticks.get() + 1);
            quit();
            None
        }
    }

    fn quit_and_rerun() {
        if !support::appkit() {
            return;
        }
        let ticks = Rc::new(Cell::new(0));
        for round in 1..=2 {
            schedule(now());
            run(QuitFirst {
                ticks: ticks.clone(),
            })
            .expect("the loop returns after quit");
            assert_eq!(ticks.get(), round, "round {round} ticked once and returned");
        }
    }

    struct Nested {
        answer: Rc<Cell<Option<Result<(), ShellError>>>>,
    }

    impl Handler for Nested {
        fn window_event(&mut self, _: WindowId, _: ShellEvent) {}

        fn tick(&mut self, _: Instant) -> Option<Instant> {
            self.answer.set(Some(run(QuitFirst {
                ticks: Rc::new(Cell::new(0)),
            })));
            quit();
            None
        }
    }

    fn refuses_reentry() {
        if !support::appkit() {
            return;
        }
        let answer = Rc::new(Cell::new(None));
        schedule(now());
        run(Nested {
            answer: answer.clone(),
        })
        .expect("the outer loop runs");
        assert_eq!(answer.take(), Some(Err(ShellError::LoopAlreadyRunning)));
    }
}
