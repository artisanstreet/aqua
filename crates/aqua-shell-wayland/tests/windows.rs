#![cfg(target_os = "linux")]

mod support;

use aqua_shell_wayland::{pump_sent_messages, ShellEvent, Unsupported, Window};
use std::time::Duration;
use support::{drive, events, floating, options, Flow};

#[test]
fn a_created_window_is_configured_before_create_returns_and_has_the_requested_logical_size() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    floating(&compositor, "com.aqua.test.created");
    let window = Window::create(&options("com.aqua.test.created", (640.0, 480.0))).unwrap();
    assert!(window.is_visible(), "create maps the toplevel");
    assert_eq!(window.inner_size(), (640, 480));
    let probe = compositor.clone();
    let log = drive(&[window], Duration::from_secs(5), move |_| {
        match probe.view_rect("com.aqua.test.created") {
            Some(rect) if rect.width > 0 => Flow::Done,
            _ => Flow::Wait(Duration::from_millis(20)),
        }
    });
    let rect = compositor.view_rect("com.aqua.test.created").unwrap();
    assert_eq!((rect.width, rect.height), (640, 480));
    assert_eq!(
        events(&log).first(),
        Some(&ShellEvent::Resized {
            width: 640,
            height: 480
        })
    );
}

#[test]
fn resizing_through_the_compositor_emits_resized_in_physical_pixels() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    floating(&compositor, "com.aqua.test.resized");
    let window = Window::create(&options("com.aqua.test.resized", (400.0, 300.0))).unwrap();
    let mut asked = false;
    let probe = compositor.clone();
    let log = drive(&[window], Duration::from_secs(5), move |context| {
        let compositor = &probe;
        let resized = context.saw(|event| {
            *event
                == ShellEvent::Resized {
                    width: 800,
                    height: 600,
                }
        });
        match (resized, asked) {
            (true, _) => Flow::Done,
            (false, false) if compositor.view_rect("com.aqua.test.resized").is_some() => {
                asked = true;
                compositor
                    .command("[app_id=\"com.aqua.test.resized\"] resize set 800 600")
                    .unwrap();
                Flow::Wait(Duration::from_millis(20))
            }
            _ => Flow::Wait(Duration::from_millis(20)),
        }
    });
    assert_eq!(window.inner_size(), (800, 600));
    assert!(events(&log).contains(&ShellEvent::Resized {
        width: 800,
        height: 600
    }));
}

#[test]
fn closing_from_the_compositor_emits_close_requested() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let window = Window::create(&options("com.aqua.test.closed", (300.0, 200.0))).unwrap();
    let mut asked = false;
    let probe = compositor.clone();
    let log = drive(&[window], Duration::from_secs(5), move |context| {
        let compositor = &probe;
        match (
            context.saw(|event| *event == ShellEvent::CloseRequested),
            asked,
        ) {
            (true, _) => Flow::Done,
            (false, false) if compositor.view("com.aqua.test.closed").is_some() => {
                asked = true;
                compositor
                    .command("[app_id=\"com.aqua.test.closed\"] kill")
                    .unwrap();
                Flow::Wait(Duration::from_millis(20))
            }
            _ => Flow::Wait(Duration::from_millis(20)),
        }
    });
    assert!(
        window.is_visible(),
        "a close request leaves the window to the app"
    );
    assert_eq!(
        events(&log)
            .iter()
            .filter(|event| **event == ShellEvent::CloseRequested)
            .count(),
        1
    );
}

#[test]
fn moving_the_window_to_a_hidden_workspace_reports_covered() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let window = Window::create(&options("com.aqua.test.covered", (300.0, 200.0))).unwrap();
    let mut phase = 0;
    let probe = compositor.clone();
    let log = drive(&[window], Duration::from_secs(10), move |context| {
        let compositor = &probe;
        support::paint(window);
        let covered = context.count(|event| *event == ShellEvent::Covered(true));
        let uncovered = context.count(|event| *event == ShellEvent::Covered(false));
        match (phase, covered, uncovered) {
            (0, _, _) if compositor.view("com.aqua.test.covered").is_some() => {
                phase = 1;
                compositor
                    .command("[app_id=\"com.aqua.test.covered\"] move container to workspace 2")
                    .unwrap();
            }
            (1, 1, _) => {
                phase = 2;
                compositor.command("workspace 2").unwrap();
            }
            (2, 1, 1) => return Flow::Done,
            _ => {}
        }
        Flow::Wait(Duration::from_millis(20))
    });
    let covered: Vec<ShellEvent> = events(&log)
        .into_iter()
        .filter(|event| matches!(event, ShellEvent::Covered(_)))
        .collect();
    assert_eq!(
        covered,
        vec![ShellEvent::Covered(true), ShellEvent::Covered(false)]
    );
    assert!(!window.is_covered());
}

#[test]
fn positioning_is_reported_as_unsupported() {
    let Some(_compositor) = support::compositor() else {
        return;
    };
    let window = Window::create(&options("com.aqua.test.position", (200.0, 200.0))).unwrap();
    assert_eq!(window.set_position(10, 10), Err(Unsupported::Position));
    assert_eq!(window.outer_rect(), Err(Unsupported::Position));
    assert_eq!(window.restore_minimized(), Err(Unsupported::Unminimize));
    assert_eq!(window.set_topmost(true), Err(Unsupported::Topmost));
}

#[test]
fn a_long_handler_still_answers_the_compositor_through_checkpoints() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    floating(&compositor, "com.aqua.test.busy");
    let window = Window::create(&options("com.aqua.test.busy", (300.0, 200.0))).unwrap();
    let mut seen_during_work = None;
    let probe = compositor.clone();
    drive(&[window], Duration::from_secs(10), move |_| {
        let compositor = &probe;
        if compositor.view_rect("com.aqua.test.busy").is_none() {
            return Flow::Wait(Duration::from_millis(20));
        }
        compositor
            .command("[app_id=\"com.aqua.test.busy\"] resize set 500 400")
            .unwrap();
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_millis(1500) {
            pump_sent_messages(std::panic::Location::caller());
            if seen_during_work.is_none() && window.inner_size() == (500, 400) {
                seen_during_work = Some(started.elapsed());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            seen_during_work.is_some(),
            "the compositor's configure was not dispatched while the handler worked"
        );
        Flow::Done
    });
}

#[test]
fn hiding_and_showing_a_window_maps_the_same_surface_again() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let app = "com.aqua.test.cycle";
    let window = Window::create(&options(app, (300.0, 200.0))).unwrap();
    let probe = compositor.clone();
    let mut phase = 0;
    let mut cycles = 0;
    drive(&[window], Duration::from_secs(10), move |_| {
        let mapped = probe.view(app).is_some();
        match (phase, mapped) {
            (0, true) => {
                window.hide();
                assert!(!window.is_visible());
                phase = 1;
            }
            (1, false) => {
                window.show().expect("the toplevel is configured again");
                assert!(window.is_visible());
                cycles += 1;
                phase = match cycles {
                    3 => 2,
                    _ => 0,
                };
            }
            (2, true) => return Flow::Done,
            _ => {}
        }
        Flow::Wait(Duration::from_millis(20))
    });
    assert!(compositor.view(app).is_some());
}
