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
