#![cfg(target_os = "linux")]

mod support;

use aqua_shell_wayland::Window;
use std::time::Duration;
use support::options;

#[test]
fn the_monitor_reports_the_output_in_physical_pixels_and_its_refresh() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    compositor.command("output HEADLESS-1 scale 1.25").unwrap();
    let window = Window::create(&options("com.aqua.test.monitor", (300.0, 200.0))).unwrap();
    support::drive(&[window], Duration::from_secs(5), |context| {
        match context.focused() {
            true => support::Flow::Done,
            false => support::Flow::Wait(Duration::from_millis(20)),
        }
    });
    let monitor = window.monitor().expect("the window is on an output");
    assert_eq!(monitor.name, "HEADLESS-1");
    assert_eq!(monitor.bounds, (0, 0, 1280, 800));
    assert_eq!(monitor.work_area, monitor.bounds);
    assert_eq!(monitor.scale, 1.25);
    assert_eq!(window.scale(), 1.25);
    assert_eq!(window.logical_size(), (1024, 640));
    assert_eq!(window.inner_size(), (1280, 800));
    let refresh = window
        .refresh_interval()
        .expect("the mode has a refresh rate");
    assert!(
        refresh.abs_diff(Duration::from_micros(16_667)) < Duration::from_micros(50),
        "refresh interval {refresh:?}"
    );
}
