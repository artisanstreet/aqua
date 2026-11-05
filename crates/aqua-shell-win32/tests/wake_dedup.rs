#![cfg(windows)]

use aqua_protocol::{Instant, WindowId};
use aqua_shell_win32::{now, quit, run, Handler, ShellEvent, Window, WindowOptions};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

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
                waker.wake();
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
fn a_thousand_wakes_before_the_loop_turns_deliver_one_wake_and_a_later_wake_still_arrives() {
    let window = Window::create(&WindowOptions {
        title: "aqua-wake-dedup".into(),
        logical_size: (100.0, 100.0),
        visible: false,
        ..Default::default()
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
    window.destroy();
}
