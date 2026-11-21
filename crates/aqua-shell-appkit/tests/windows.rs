#[cfg(target_os = "macos")]
mod support;

fn main() {
    #[cfg(target_os = "macos")]
    support::main(&cases::ALL);
    #[cfg(not(target_os = "macos"))]
    println!("windows: skipped; requires macOS");
}

#[cfg(target_os = "macos")]
mod cases {
    use super::support::{self, case, events_of, script, stage, Case};
    use aqua_core::{InputEvent, Key, KeyEvent, Modifiers};
    use aqua_shell_appkit::testing::inject::{self, Keystroke};
    use aqua_shell_appkit::testing::{self, Caption};
    use aqua_shell_appkit::{ShellEvent, Window};
    use std::time::Duration;

    pub const ALL: [Case; 6] = [
        case(
            "a_new_window_reports_its_backing_scale_before_its_physical_size",
            scale_first,
        ),
        case("resizing_emits_physical_sizes", resizing),
        case(
            "minimizing_and_restoring_emit_minimized_both_ways",
            minimizing,
        ),
        case(
            "ordering_a_window_out_marks_it_covered_and_showing_it_uncovers_it",
            covering,
        ),
        case(
            "closing_through_the_red_button_asks_before_destroying",
            red_button,
        ),
        case("command_q_asks_every_window_to_close", command_q),
    ];

    const LIMIT: Duration = Duration::from_secs(20);

    fn resized(event: &ShellEvent) -> bool {
        matches!(event, ShellEvent::Resized { .. })
    }

    fn scale_first() {
        if !support::appkit() {
            return;
        }
        let window = Window::create(&support::options("scale", (320.0, 240.0))).expect("window");
        let scale = window.scale();
        let log = script(
            LIMIT,
            vec![stage(
                "settle",
                || {},
                |cx| cx.saw(|event| *event == ShellEvent::Covered(false)),
            )],
        );
        let events = events_of(&log, window.id());
        let scale_at = events
            .iter()
            .position(|event| matches!(event, ShellEvent::ScaleChanged { .. }))
            .expect("the window reports its scale");
        let size_at = events
            .iter()
            .position(resized)
            .expect("the window reports its size");
        assert!(scale_at < size_at, "{events:?}");
        assert_eq!(events[scale_at], ShellEvent::ScaleChanged { scale });
        assert_eq!(
            events[size_at],
            ShellEvent::Resized {
                width: (320.0 * scale).round() as u32,
                height: (240.0 * scale).round() as u32,
            }
        );
        assert_eq!(
            window.inner_size(),
            ((320.0 * scale) as u32, (240.0 * scale) as u32)
        );
    }

    fn resizing() {
        if !support::appkit() {
            return;
        }
        let window = Window::create(&support::options("resize", (320.0, 240.0))).expect("window");
        let scale = window.scale();
        let wanted = ShellEvent::Resized {
            width: (400.0 * scale).round() as u32,
            height: (300.0 * scale).round() as u32,
        };
        let expected = wanted.clone();
        script(
            LIMIT,
            vec![stage(
                "resize",
                move || {
                    window
                        .set_logical_size(400.0, 300.0)
                        .expect("a normal window resizes")
                },
                move |cx| cx.saw(|event| *event == expected),
            )],
        );
        assert_eq!(window.logical_size(), (400, 300));
        let lease = window
            .layer_lease()
            .expect("a live window leases its layer");
        assert_eq!(
            lease.geometry().physical,
            (
                (400.0 * scale).round() as u32,
                (300.0 * scale).round() as u32
            )
        );
    }

    fn minimizing() {
        if !support::appkit() {
            return;
        }
        let window = Window::create(&support::options("minimize", (320.0, 240.0))).expect("window");
        let log = script(
            LIMIT,
            vec![
                stage(
                    "shown",
                    || {},
                    |cx| cx.saw(|event| *event == ShellEvent::Covered(false)),
                ),
                stage(
                    "minimize",
                    move || window.minimize().expect("a minimizable window minimizes"),
                    |cx| cx.saw(|event| *event == ShellEvent::Minimized(true)),
                ),
                stage(
                    "restore",
                    move || window.restore_minimized(),
                    |cx| cx.saw(|event| *event == ShellEvent::Minimized(false)),
                ),
            ],
        );
        let events = events_of(&log, window.id());
        let down = events
            .iter()
            .position(|event| *event == ShellEvent::Minimized(true))
            .expect("minimized");
        assert!(
            events[down..].contains(&ShellEvent::Covered(true)),
            "a minimized window is covered: {events:?}"
        );
    }

    fn covering() {
        if !support::appkit() {
            return;
        }
        let window = Window::create(&support::options("cover", (320.0, 240.0))).expect("window");
        script(
            LIMIT,
            vec![
                stage(
                    "shown",
                    || {},
                    |cx| cx.saw(|event| *event == ShellEvent::Covered(false)),
                ),
                stage(
                    "hide",
                    move || window.hide(),
                    |cx| cx.saw(|event| *event == ShellEvent::Covered(true)),
                ),
                stage(
                    "show",
                    move || window.show().expect("show"),
                    |cx| cx.saw(|event| *event == ShellEvent::Covered(false)),
                ),
            ],
        );
        assert!(!window.is_covered());
        let lease = window.layer_lease().expect("lease");
        assert!(
            !lease.geometry().covered,
            "the layer lease follows occlusion"
        );
    }

    fn red_button() {
        if !support::appkit() {
            return;
        }
        let window = Window::create(&support::options("close", (320.0, 240.0))).expect("window");
        let _keeps_the_loop_alive = Window::create(&aqua_shell_appkit::WindowOptions {
            visible: false,
            ..support::options("other", (100.0, 100.0))
        })
        .expect("window");
        let log = script(
            LIMIT,
            vec![
                stage(
                    "shown",
                    || {},
                    |cx| cx.saw(|event| *event == ShellEvent::Covered(false)),
                ),
                stage(
                    "click close",
                    move || testing::click_caption_button(window, Caption::Close),
                    |cx| cx.saw(|event| *event == ShellEvent::CloseRequested),
                ),
                stage(
                    "still open",
                    move || assert!(window.is_visible(), "the red button only asks"),
                    |_| true,
                ),
                stage(
                    "destroy",
                    move || window.destroy(),
                    |cx| cx.saw(|event| *event == ShellEvent::Destroyed),
                ),
            ],
        );
        let events = events_of(&log, window.id());
        let asked = events
            .iter()
            .position(|event| *event == ShellEvent::CloseRequested)
            .expect("asked");
        let gone = events
            .iter()
            .position(|event| *event == ShellEvent::Destroyed)
            .expect("destroyed");
        assert!(asked < gone, "{events:?}");
        assert_eq!(window.title(), "", "a destroyed window has no state left");
    }

    fn command_q() {
        if !support::appkit() {
            return;
        }
        let first = Window::create(&support::options("first", (320.0, 240.0))).expect("window");
        let second = Window::create(&support::options("second", (320.0, 240.0))).expect("window");
        let stroke = Keystroke {
            code: 12,
            characters: "q",
            ignoring_modifiers: "q",
            modifiers: Modifiers::LOGO,
            repeat: false,
        };
        let log = script(
            LIMIT,
            vec![
                stage("activate", testing::activate, |cx| {
                    cx.saw(|event| *event == ShellEvent::Focused(true))
                }),
                stage(
                    "command q",
                    move || inject::key_down(second, stroke),
                    move |cx| {
                        cx.saw_on(first.id(), |event| *event == ShellEvent::CloseRequested)
                            && cx.saw_on(second.id(), |event| *event == ShellEvent::CloseRequested)
                    },
                ),
            ],
        );
        let typed_q = support::inputs(&log).into_iter().any(|input| {
            matches!(
                input,
                InputEvent::KeyDown(KeyEvent {
                    key: Key::Char('q'),
                    ..
                })
            )
        });
        assert!(
            !typed_q,
            "the menu owns Command-Q, so the app never sees the key"
        );
    }
}
