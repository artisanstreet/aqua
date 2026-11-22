#[cfg(target_os = "macos")]
mod support;

fn main() {
    #[cfg(target_os = "macos")]
    support::main(&cases::ALL);
    #[cfg(not(target_os = "macos"))]
    println!("outputs: skipped; requires macOS");
}

#[cfg(target_os = "macos")]
mod cases {
    use super::support::{self, case, script, stage, Case};
    use aqua_shell_appkit::{ShellEvent, Window};
    use std::time::Duration;

    pub const ALL: [Case; 1] = [case(
        "the_monitor_work_area_excludes_the_menu_bar",
        work_area,
    )];

    fn work_area() {
        if !support::appkit() {
            return;
        }
        let window = Window::create(&support::options("monitor", (320.0, 240.0))).expect("window");
        script(
            Duration::from_secs(20),
            vec![stage(
                "shown",
                || {},
                |cx| cx.saw(|event| *event == ShellEvent::Covered(false)),
            )],
        );
        let monitor = window.monitor().expect("a shown window is on a screen");
        println!("monitor: {monitor:?}");
        let (left, top, right, bottom) = monitor.bounds;
        let (work_left, work_top, work_right, work_bottom) = monitor.work_area;
        assert!(
            work_top > top,
            "the menu bar is above the work area: {monitor:?}"
        );
        assert!(
            work_left >= left && work_right <= right && work_bottom <= bottom,
            "{monitor:?}"
        );
        assert_eq!(monitor.scale, window.scale());
        let refresh = monitor
            .refresh
            .expect("screens report their refresh interval");
        assert!(
            refresh > Duration::from_millis(4) && refresh < Duration::from_millis(50),
            "refresh {refresh:?}"
        );
        let (x0, y0, x1, y1) = window.outer_rect().expect("outer rect");
        assert!(
            x0 >= left && y0 >= work_top && x1 <= right && y1 <= bottom,
            "a centred window lies inside the work area: {:?} in {monitor:?}",
            (x0, y0, x1, y1)
        );
    }
}
