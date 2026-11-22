#[cfg(target_os = "macos")]
mod support;

fn main() {
    #[cfg(target_os = "macos")]
    support::main(&cases::ALL);
    #[cfg(not(target_os = "macos"))]
    println!("input: skipped; requires macOS");
}

#[cfg(target_os = "macos")]
mod cases {
    use super::support::{self, case, inputs, script, stage, Case};
    use aqua_core::{InputEvent, PointerButton, ScrollDelta};
    use aqua_shell_appkit::testing::inject::{self, Button, Motion, Wheel};
    use aqua_shell_appkit::testing::post;
    use aqua_shell_appkit::{ShellEvent, Window};
    use std::time::Duration;

    pub const ALL: [Case; 5] = [
        case("one_wheel_notch_scrolls_sixteen_units", wheel_notch),
        case(
            "trackpad_deltas_pass_through_in_points_including_momentum",
            trackpad,
        ),
        case("the_middle_and_side_buttons_map_like_windows", buttons),
        case(
            "pointer_positions_are_view_local_points_from_the_top_left",
            positions,
        ),
        case("entering_and_leaving_the_view_reports_pointer_edges", edges),
    ];

    const LIMIT: Duration = Duration::from_secs(20);

    fn window(title: &str) -> Window {
        Window::create(&support::options(title, (320.0, 240.0))).expect("window")
    }

    fn shown() -> support::Stage {
        stage(
            "shown",
            || {},
            |cx| cx.saw(|event| *event == ShellEvent::Covered(false)),
        )
    }

    fn scrolls(log: &[support::Entry]) -> Vec<(f32, f32, ScrollDelta)> {
        inputs(log)
            .into_iter()
            .filter_map(|input| match input {
                InputEvent::Scroll { at, delta } => Some((at.x, at.y, delta)),
                _ => None,
            })
            .collect()
    }

    fn scroll_count(count: usize) -> impl Fn(&support::Context<'_>) -> bool {
        move |cx| {
            cx.count(|event| matches!(event, ShellEvent::Input(InputEvent::Scroll { .. }))) >= count
        }
    }

    fn wheel_notch() {
        if !support::appkit() {
            return;
        }
        let window = window("wheel");
        let log = script(
            LIMIT,
            vec![
                shown(),
                stage(
                    "wheel",
                    move || {
                        inject::scroll(window, (100.0, 80.0), Wheel::Lines { dx: 0, dy: 1 });
                        inject::scroll(window, (100.0, 80.0), Wheel::Lines { dx: 0, dy: -2 });
                    },
                    scroll_count(2),
                ),
            ],
        );
        let deltas: Vec<ScrollDelta> = scrolls(&log)
            .into_iter()
            .map(|(_, _, delta)| delta)
            .collect();
        assert_eq!(
            deltas,
            vec![
                ScrollDelta { x: 0.0, y: -16.0 },
                ScrollDelta { x: 0.0, y: 32.0 }
            ],
            "one notch up scrolls 16 units up, two down scroll 32 down"
        );
        let (x, y, _) = scrolls(&log)[0];
        assert!(
            (x - 100.0).abs() <= 1.0 && (y - 80.0).abs() <= 1.0,
            "the scroll happened at ({x}, {y})"
        );
    }

    fn trackpad() {
        if !support::appkit() {
            return;
        }
        let window = window("trackpad");
        let log = script(
            LIMIT,
            vec![
                shown(),
                stage(
                    "swipe",
                    move || {
                        inject::scroll(
                            window,
                            (60.0, 60.0),
                            Wheel::Pixels {
                                dx: 0,
                                dy: -30,
                                momentum: false,
                            },
                        );
                        inject::scroll(
                            window,
                            (60.0, 60.0),
                            Wheel::Pixels {
                                dx: 5,
                                dy: -12,
                                momentum: false,
                            },
                        );
                        inject::scroll(
                            window,
                            (60.0, 60.0),
                            Wheel::Pixels {
                                dx: 0,
                                dy: -32,
                                momentum: true,
                            },
                        );
                        inject::scroll(
                            window,
                            (60.0, 60.0),
                            Wheel::Pixels {
                                dx: 0,
                                dy: -7,
                                momentum: true,
                            },
                        );
                    },
                    scroll_count(4),
                ),
            ],
        );
        let deltas: Vec<(f32, f32)> = scrolls(&log)
            .into_iter()
            .map(|(_, _, delta)| (delta.x, delta.y))
            .collect();
        let expected = [(0.0, 30.0), (-5.0, 12.0), (0.0, 32.0), (0.0, 7.0)];
        assert_eq!(deltas.len(), expected.len(), "{deltas:?}");
        for ((x, y), (want_x, want_y)) in deltas.iter().zip(expected) {
            assert!(
                (x - want_x).abs() < 1e-3 && (y - want_y).abs() < 1e-3,
                "points arrived as {deltas:?}, wanted {expected:?}"
            );
        }
        let momentum_on_grid = deltas[2];
        assert!(
            aqua_runtime::glide::notch_pixels(ScrollDelta {
                x: momentum_on_grid.0,
                y: momentum_on_grid.1
            })
            .is_none(),
            "a 32 point momentum step must not glide like two wheel notches"
        );
    }

    fn buttons() {
        if !support::appkit() {
            return;
        }
        let window = window("buttons");
        let at = (50.0, 50.0);
        let log = script(
            LIMIT,
            vec![
                shown(),
                stage(
                    "buttons",
                    move || {
                        for button in [
                            Button::Right,
                            Button::Other(2),
                            Button::Other(3),
                            Button::Other(4),
                        ] {
                            inject::mouse(window, button, Motion::Down, at);
                            inject::mouse(window, button, Motion::Up, at);
                        }
                    },
                    |cx| {
                        cx.count(|event| {
                            matches!(event, ShellEvent::Input(InputEvent::PointerUp(..)))
                        }) >= 4
                    },
                ),
            ],
        );
        let presses: Vec<(PointerButton, u8)> = inputs(&log)
            .into_iter()
            .filter_map(|input| match input {
                InputEvent::PointerDown(pointer, button) => Some((button, pointer.buttons)),
                _ => None,
            })
            .collect();
        assert_eq!(
            presses,
            vec![
                (PointerButton::Secondary, 0x02),
                (PointerButton::Middle, 0x04),
                (PointerButton::Extra(3), 0x08),
                (PointerButton::Extra(4), 0x10),
            ]
        );
    }

    fn positions() {
        if !support::appkit() {
            return;
        }
        let window = window("positions");
        let log = script(
            LIMIT,
            vec![
                shown(),
                stage(
                    "press",
                    move || {
                        inject::mouse(window, Button::Left, Motion::Down, (30.0, 50.0));
                        inject::mouse(window, Button::Left, Motion::Dragged, (40.0, 70.0));
                        inject::mouse(window, Button::Left, Motion::Up, (40.0, 70.0));
                    },
                    |cx| {
                        cx.saw(|event| {
                            matches!(event, ShellEvent::Input(InputEvent::PointerUp(..)))
                        })
                    },
                ),
            ],
        );
        let trail: Vec<(f32, f32, u8)> = inputs(&log)
            .into_iter()
            .filter_map(|input| match input {
                InputEvent::PointerDown(pointer, _)
                | InputEvent::PointerMove(pointer)
                | InputEvent::PointerUp(pointer, _) => {
                    Some((pointer.position.x, pointer.position.y, pointer.buttons))
                }
                _ => None,
            })
            .collect();
        let expected = [(30.0, 50.0, 0x01), (40.0, 70.0, 0x01), (40.0, 70.0, 0x00)];
        assert_eq!(trail.len(), expected.len(), "{trail:?}");
        for ((x, y, buttons), (want_x, want_y, want_buttons)) in trail.iter().zip(expected) {
            assert!(
                (x - want_x).abs() <= 1.0 && (y - want_y).abs() <= 1.0 && *buttons == want_buttons,
                "{trail:?}"
            );
        }
    }

    fn edges() {
        if !support::posting() {
            return;
        }
        let window = window("edges");
        let log = script(
            LIMIT,
            vec![
                shown(),
                stage(
                    "outside",
                    move || post::mouse(window, Button::Left, Motion::Moved, (-60.0, 120.0), 0),
                    |_| true,
                ),
                stage(
                    "inside",
                    move || post::mouse(window, Button::Left, Motion::Moved, (160.0, 120.0), 0),
                    |cx| cx.saw(|event| *event == ShellEvent::PointerEntered),
                ),
                stage(
                    "leave",
                    move || post::mouse(window, Button::Left, Motion::Moved, (400.0, 120.0), 0),
                    |cx| cx.saw(|event| *event == ShellEvent::PointerLeft),
                ),
            ],
        );
        let entered = log
            .iter()
            .position(|(_, event, _)| *event == ShellEvent::PointerEntered)
            .expect("entered");
        let left = log
            .iter()
            .position(|(_, event, _)| *event == ShellEvent::PointerLeft)
            .expect("left");
        assert!(entered < left);
    }
}
