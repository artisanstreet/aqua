#[cfg(target_os = "macos")]
mod support;

fn main() {
    #[cfg(target_os = "macos")]
    support::main(&cases::ALL);
    #[cfg(not(target_os = "macos"))]
    println!("chrome: skipped; requires macOS");
}

#[cfg(target_os = "macos")]
mod cases {
    use super::support::{self, case, case_with, events_of, inputs, script, stage, Case};
    use aqua_core::{InputEvent, PointerButton};
    use aqua_protocol::DragRegions;
    use aqua_shell_appkit::testing::inject::{Button, Motion};
    use aqua_shell_appkit::testing::{self, post, Caption};
    use aqua_shell_appkit::{Chrome, ShellEvent, Window, WindowOptions};
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;

    pub const ALL: [Case; 8] = [
        case(
            "an_overlay_window_keeps_its_traffic_lights_centred_in_the_title_band",
            centred,
        ),
        case(
            "the_overlay_traffic_lights_still_minimize_the_window",
            lights_work,
        ),
        case(
            "a_hidden_titlebar_window_has_no_caption_buttons_but_still_resizes",
            hidden,
        ),
        case("a_press_in_a_drag_region_starts_a_window_drag", drag),
        case("a_press_in_a_no_drag_hole_reaches_the_app", hole),
        case_with(
            "double_clicking_the_caption_minimizes_when_the_user_chose_minimize",
            double_click_minimizes,
            &["-AppleActionOnDoubleClick", "Minimize"],
        ),
        case_with(
            "double_clicking_the_caption_zooms_by_default",
            double_click_zooms,
            &["-AppleActionOnDoubleClick", "Maximize"],
        ),
        case(
            "a_native_title_bar_window_keeps_its_title_and_buttons",
            native,
        ),
    ];

    const LIMIT: Duration = Duration::from_secs(25);
    const BAND: f64 = 40.0;

    fn overlay(title: &str) -> WindowOptions {
        WindowOptions {
            chrome: Chrome::Custom,
            titlebar_height: BAND,
            caption_buttons: true,
            ..support::options(title, (480.0, 320.0))
        }
    }

    fn shown() -> support::Stage {
        stage(
            "shown",
            || {},
            |cx| cx.saw(|event| *event == ShellEvent::Covered(false)),
        )
    }

    fn assert_centred(window: Window, when: &str) {
        for which in [Caption::Close, Caption::Minimize, Caption::Zoom] {
            let (rect, visible) =
                testing::caption_button(window, which).expect("overlay windows keep their buttons");
            assert!(visible, "{when}: {which:?} is hidden");
            let (_, centre) = rect.centre();
            assert!(
                (centre - BAND / 2.0).abs() <= 1.0,
                "{when}: {which:?} is centred at {centre}, not in the middle of the {BAND} pt band ({rect:?})"
            );
        }
    }

    fn centred() {
        if !support::appkit() {
            return;
        }
        let window = Window::create(&overlay("centred")).expect("window");
        script(
            LIMIT,
            vec![
                shown(),
                stage(
                    "measure",
                    move || assert_centred(window, "after showing"),
                    |_| true,
                ),
                stage(
                    "resize",
                    move || window.set_logical_size(600.0, 400.0).expect("resize"),
                    |cx| cx.saw(|event| matches!(event, ShellEvent::Resized { .. })),
                ),
                stage(
                    "measure again",
                    move || assert_centred(window, "after resizing"),
                    |_| true,
                ),
            ],
        );
    }

    fn lights_work() {
        if !support::appkit() {
            return;
        }
        let window = Window::create(&overlay("lights")).expect("window");
        script(
            LIMIT,
            vec![
                shown(),
                stage(
                    "yellow light",
                    move || testing::click_caption_button(window, Caption::Minimize),
                    |cx| cx.saw(|event| *event == ShellEvent::Minimized(true)),
                ),
            ],
        );
    }

    fn hidden() {
        if !support::posting() {
            return;
        }
        let window = Window::create(&WindowOptions {
            chrome: Chrome::Custom,
            caption_buttons: false,
            ..support::options("hidden", (400.0, 300.0))
        })
        .expect("window");
        let before = Rc::new(Cell::new(0.0));
        let start = before.clone();
        let edge = (401.0, 150.0);
        script(
            LIMIT,
            vec![
                shown(),
                stage(
                    "no buttons",
                    move || {
                        for which in [Caption::Close, Caption::Minimize, Caption::Zoom] {
                            let visible = testing::caption_button(window, which)
                                .is_some_and(|(_, visible)| visible);
                            assert!(!visible, "{which:?} shows on a hidden title bar");
                        }
                        start.set(testing::frame(window).expect("frame").width);
                    },
                    |_| true,
                ),
                stage(
                    "hover edge",
                    move || post::mouse(window, Button::Left, Motion::Moved, edge, 0),
                    |_| true,
                ),
                stage(
                    "grab edge",
                    move || post::mouse(window, Button::Left, Motion::Down, edge, 1),
                    |_| true,
                ),
                stage(
                    "drag edge",
                    move || {
                        post::mouse(
                            window,
                            Button::Left,
                            Motion::Dragged,
                            (edge.0 + 80.0, edge.1),
                            1,
                        )
                    },
                    |_| true,
                ),
                stage(
                    "release edge",
                    move || {
                        post::mouse(window, Button::Left, Motion::Up, (edge.0 + 80.0, edge.1), 1)
                    },
                    |cx| {
                        cx.log.iter().any(|(_, event, _)| matches!(event, ShellEvent::Resized { width, .. } if *width > 400))
                            && cx.elapsed() >= Duration::from_millis(300)
                    },
                ),
            ],
        );
        let after = testing::frame(window).expect("frame").width;
        assert!(
            after > before.get() + 40.0,
            "the right edge drag grew the window from {} to {after}",
            before.get()
        );
    }

    fn drag() {
        if !support::posting() {
            return;
        }
        let window = Window::create(&overlay("drag")).expect("window");
        let origin = Rc::new(Cell::new((0.0, 0.0)));
        let start = origin.clone();
        let grab = (240.0, 20.0);
        let steps = 8;
        let mut stages = vec![
            shown(),
            stage(
                "press caption",
                move || {
                    let frame = testing::frame(window).expect("frame");
                    start.set((frame.x, frame.y));
                    post::mouse(window, Button::Left, Motion::Moved, grab, 0);
                    post::mouse(window, Button::Left, Motion::Down, grab, 1);
                },
                |_| true,
            ),
        ];
        let screen_grab = Rc::new(Cell::new(None));
        for step in 1..=steps {
            let fraction = f64::from(step) / f64::from(steps);
            let anchor = screen_grab.clone();
            stages.push(stage(
                "drag",
                move || {
                    let from = anchor.get().unwrap_or_else(|| {
                        let point = testing::screen_point(window, grab).expect("point");
                        anchor.set(Some(point));
                        point
                    });
                    post::mouse_at(
                        Button::Left,
                        Motion::Dragged,
                        (from.0 + 80.0 * fraction, from.1 + 50.0 * fraction),
                        1,
                    );
                },
                |_| true,
            ));
        }
        let anchor = screen_grab.clone();
        stages.push(stage(
            "release",
            move || {
                let from = anchor.get().expect("the drag started");
                post::mouse_at(Button::Left, Motion::Up, (from.0 + 80.0, from.1 + 50.0), 1);
            },
            |cx| cx.elapsed() >= Duration::from_millis(400),
        ));
        let log = script(LIMIT, stages);
        let (x, y) = origin.get();
        let frame = testing::frame(window).expect("frame");
        println!("dragged from ({x}, {y}) to ({}, {})", frame.x, frame.y);
        assert!(
            (frame.x - x - 80.0).abs() <= 2.0 && (y - frame.y - 50.0).abs() <= 2.0,
            "the window moved from ({x}, {y}) to ({}, {}) instead of by (80, 50)",
            frame.x,
            frame.y
        );
        assert!(
            !inputs(&log)
                .iter()
                .any(|input| matches!(input, InputEvent::PointerDown(..))),
            "a caption press is not app input"
        );
    }

    fn hole() {
        if !support::posting() {
            return;
        }
        let window = Window::create(&overlay("hole")).expect("window");
        window.set_drag_regions(&DragRegions {
            draggable: Vec::new(),
            no_drag: vec![(100.0, 0.0, 200.0, BAND)],
            caption_controls: Vec::new(),
        });
        let at = (150.0, 20.0);
        let log = script(
            LIMIT,
            vec![
                shown(),
                stage(
                    "press hole",
                    move || {
                        post::mouse(window, Button::Left, Motion::Moved, at, 0);
                        post::mouse(window, Button::Left, Motion::Down, at, 1);
                        post::mouse(window, Button::Left, Motion::Up, at, 1);
                    },
                    |cx| {
                        cx.saw(|event| {
                            matches!(event, ShellEvent::Input(InputEvent::PointerUp(..)))
                        })
                    },
                ),
            ],
        );
        let down = inputs(&log)
            .into_iter()
            .find_map(|input| match input {
                InputEvent::PointerDown(pointer, PointerButton::Primary) => Some(pointer),
                _ => None,
            })
            .expect("the press reached the app");
        assert!(
            (down.position.x - 150.0).abs() <= 1.0 && (down.position.y - 20.0).abs() <= 1.0,
            "the press arrived at {:?}",
            down.position
        );
    }

    fn double_click(expect: ShellEvent) {
        if !support::posting() {
            return;
        }
        let window = Window::create(&overlay("double")).expect("window");
        let at = (240.0, 20.0);
        let wanted = expect.clone();
        let log = script(
            LIMIT,
            vec![
                shown(),
                stage(
                    "double click",
                    move || {
                        post::mouse(window, Button::Left, Motion::Moved, at, 0);
                        post::mouse(window, Button::Left, Motion::Down, at, 1);
                        post::mouse(window, Button::Left, Motion::Up, at, 1);
                        post::mouse(window, Button::Left, Motion::Down, at, 2);
                        post::mouse(window, Button::Left, Motion::Up, at, 2);
                    },
                    move |cx| cx.saw(|event| *event == wanted),
                ),
            ],
        );
        assert!(events_of(&log, window.id()).contains(&expect));
    }

    fn double_click_minimizes() {
        double_click(ShellEvent::Minimized(true));
    }

    fn double_click_zooms() {
        double_click(ShellEvent::Maximized(true));
    }

    fn native() {
        if !support::appkit() {
            return;
        }
        let window =
            Window::create(&support::options("native title", (320.0, 240.0))).expect("window");
        script(
            LIMIT,
            vec![
                shown(),
                stage(
                    "inspect",
                    move || {
                        let (rect, visible) =
                            testing::caption_button(window, Caption::Close).expect("close button");
                        assert!(visible);
                        assert!(
                            rect.y < 0.0,
                            "a native title bar sits above the content view: {rect:?}"
                        );
                        assert_eq!(window.title(), "native title");
                    },
                    |_| true,
                ),
            ],
        );
    }
}
