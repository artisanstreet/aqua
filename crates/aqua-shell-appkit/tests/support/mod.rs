#![allow(dead_code, unused_imports)]

use aqua_shell_appkit::testing::{available, Facility};
use aqua_shell_appkit::WindowOptions;

pub use aqua_shell_appkit::testing::cases::{case, case_with, main, Case};
pub use aqua_shell_appkit::testing::script::{
    drive, events_of, inputs, script, stage, Context, Entry, Flow, Stage, POLL,
};

pub fn appkit() -> bool {
    available(Facility::WindowServer)
}

pub fn posting() -> bool {
    appkit() && available(Facility::PostEvent)
}

pub fn options(title: &str, logical: (f64, f64)) -> WindowOptions {
    WindowOptions {
        title: title.into(),
        logical_size: logical,
        ..WindowOptions::default()
    }
}

pub fn wait_for(limit: std::time::Duration, mut done: impl FnMut() -> bool + 'static) -> bool {
    let reached = std::rc::Rc::new(std::cell::Cell::new(false));
    let flag = reached.clone();
    let started = std::time::Instant::now();
    drive(
        limit + std::time::Duration::from_secs(5),
        move |_, _| match (done(), started.elapsed() >= limit) {
            (true, _) => {
                flag.set(true);
                Flow::Done
            }
            (false, true) => Flow::Done,
            (false, false) => Flow::Wait(POLL),
        },
    );
    reached.get()
}
