#![allow(dead_code, unused_imports)]

use aqua_shell_appkit::testing::{available, Facility};
use aqua_shell_appkit::WindowOptions;

pub use aqua_shell_appkit::testing::cases::{case, main, Case};
pub use aqua_shell_appkit::testing::script::{inputs, script, stage, Context, Entry, Stage};

pub fn appkit() -> bool {
    available(Facility::WindowServer)
}

pub fn input_method() -> bool {
    appkit() && available(Facility::InputMethod)
}

pub fn options(title: &str, logical: (f64, f64)) -> WindowOptions {
    WindowOptions {
        title: title.into(),
        logical_size: logical,
        ..WindowOptions::default()
    }
}
