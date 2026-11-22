#![cfg(target_os = "linux")]

mod support;

use aqua_core::InputEvent;
use aqua_protocol::{CaptionControl, CaptionRegion, DragRegions};
use aqua_shell_wayland::testing::{Compositor, Options, Rect, VirtualPointer};
use aqua_shell_wayland::{Chrome, ShellEvent, Window};
use std::rc::Rc;
use std::time::Duration;
use support::{drive, events, floating, inputs, options, ready, Entry, Flow};

const WIDTH: f64 = 400.0;
const HEIGHT: f64 = 300.0;
const STEP: Duration = Duration::from_millis(60);

fn custom(compositor: &Compositor, app_id: &str) -> Window {
    floating(compositor, app_id);
    let window = Window::create(&aqua_shell_wayland::WindowOptions {
        chrome: Chrome::Custom,
        ..options(app_id, (WIDTH, HEIGHT))
    })
    .unwrap();
    window.set_drag_regions(&DragRegions {
        draggable: vec![(0.0, 0.0, WIDTH, 40.0)],
        no_drag: Vec::new(),
        caption_controls: vec![
            CaptionRegion {
                control: CaptionControl::Maximize,
                bounds: (WIDTH - 80.0, 0.0, 40.0, 40.0),
            },
            CaptionRegion {
                control: CaptionControl::Close,
                bounds: (WIDTH - 40.0, 0.0, 40.0, 40.0),
            },
        ],
    });
    window
}

enum Act {
    Move(i32, i32),
    Press,
    Release,
}

fn gesture(
    compositor: Rc<Compositor>,
    pointer: VirtualPointer,
    window: Window,
    app_id: &'static str,
    acts: Vec<Act>,
    settled: impl Fn(&Compositor, Rect, &support::Context<'_>) -> bool + 'static,
) -> (Vec<Entry>, Rect) {
    let mut origin: Option<Rect> = None;
    let mut next = 0usize;
    let mut entered_once = false;
    let mut last: Option<aqua_protocol::Instant> = None;
    let start = Rc::new(std::cell::Cell::new(None));
    let started = start.clone();
    let log = drive(&[window], Duration::from_secs(10), move |context| {
        let entered = context.saw(|event| *event == ShellEvent::PointerEntered);
        let Some(rect) = origin.or_else(|| ready(&compositor, app_id)) else {
            return Flow::Wait(Duration::from_millis(20));
        };
        origin = Some(rect);
        started.set(Some(rect));
        if !entered_once {
            if !entered {
                if let Some(Act::Move(x, y)) = acts.first() {
                    pointer
                        .move_to((rect.x + x) as u32, (rect.y + y) as u32)
                        .unwrap();
                }
                return Flow::Wait(Duration::from_millis(30));
            }
            entered_once = true;
            next = 1;
        }
        if last.is_some_and(|last| context.now.saturating_duration_since(last) < STEP) {
            return Flow::Wait(STEP);
        }
        last = Some(context.now);
        match acts.get(next) {
            Some(Act::Move(x, y)) => pointer
                .move_to((rect.x + x) as u32, (rect.y + y) as u32)
                .unwrap(),
            Some(Act::Press) => pointer.press(VirtualPointer::LEFT).unwrap(),
            Some(Act::Release) => pointer.release(VirtualPointer::LEFT).unwrap(),
            None => {
                return match settled(&compositor, rect, context) {
                    true => Flow::Done,
                    false => Flow::Wait(Duration::from_millis(20)),
                };
            }
        }
        next += 1;
        Flow::Wait(STEP)
    });
    (log, start.get().expect("the window mapped"))
}

fn forwarded_presses(log: &[Entry]) -> usize {
    inputs(log)
        .iter()
        .filter(|input| {
            matches!(
                input,
                InputEvent::PointerDown(..) | InputEvent::PointerUp(..)
            )
        })
        .count()
}

#[test]
fn pressing_a_drag_region_starts_an_interactive_move() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let app = "com.aqua.test.drag";
    let pointer = VirtualPointer::new(&compositor).unwrap();
    let window = custom(&compositor, app);
    let (log, before) = gesture(
        compositor.clone(),
        pointer,
        window,
        app,
        vec![
            Act::Move(100, 20),
            Act::Press,
            Act::Move(160, 60),
            Act::Move(250, 120),
            Act::Release,
        ],
        move |compositor, start, _| {
            compositor
                .view_rect(app)
                .is_some_and(|rect| rect.x == start.x + 150 && rect.y == start.y + 100)
        },
    );
    let after = compositor.view_rect(app).unwrap();
    assert_eq!((after.x - before.x, after.y - before.y), (150, 100));
    assert_eq!(
        forwarded_presses(&log),
        0,
        "the press went to the compositor"
    );
}

#[test]
fn pressing_the_window_edge_starts_a_resize_unless_the_window_is_tiled() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let app = "com.aqua.test.edge";
    let pointer = VirtualPointer::new(&compositor).unwrap();
    let window = custom(&compositor, app);
    let edge = WIDTH as i32 - 2;
    let (log, before) = gesture(
        compositor.clone(),
        pointer,
        window,
        app,
        vec![
            Act::Move(edge, 150),
            Act::Press,
            Act::Move(edge + 50, 150),
            Act::Move(edge + 100, 150),
            Act::Release,
        ],
        move |compositor, start, _| {
            compositor
                .view_rect(app)
                .is_some_and(|rect| rect.width == start.width + 100)
        },
    );
    assert_eq!(compositor.view_rect(app).unwrap().width, before.width + 100);
    assert_eq!(window.inner_size().0 as i32, before.width + 100);
    assert_eq!(forwarded_presses(&log), 0);

    let tiled = "com.aqua.test.tiled";
    let pointer = VirtualPointer::new(&compositor).unwrap();
    let resized = window;
    let window = Window::create(&aqua_shell_wayland::WindowOptions {
        chrome: Chrome::Custom,
        ..options(tiled, (WIDTH, HEIGHT))
    })
    .unwrap();
    resized.destroy();
    let (log, rect) = gesture(
        compositor.clone(),
        pointer,
        window,
        tiled,
        vec![Act::Move(2, 300), Act::Press, Act::Release],
        |_, _, context| {
            context.saw(|event| matches!(event, ShellEvent::Input(InputEvent::PointerUp(..))))
        },
    );
    assert_eq!(compositor.view_rect(tiled).unwrap().width, rect.width);
    assert_eq!(
        forwarded_presses(&log),
        2,
        "a tiled window has no resize border"
    );
}

#[test]
fn releasing_the_close_caption_control_emits_close_requested() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let app = "com.aqua.test.close";
    let pointer = VirtualPointer::new(&compositor).unwrap();
    let window = custom(&compositor, app);
    let close = (WIDTH as i32 - 20, 20);
    let (log, _) = gesture(
        compositor.clone(),
        pointer,
        window,
        app,
        vec![
            Act::Move(close.0, close.1),
            Act::Press,
            Act::Move(200, 150),
            Act::Release,
            Act::Move(close.0, close.1),
            Act::Press,
            Act::Release,
        ],
        |_, _, context| context.saw(|event| *event == ShellEvent::CloseRequested),
    );
    let closes = events(&log)
        .iter()
        .filter(|event| **event == ShellEvent::CloseRequested)
        .count();
    assert_eq!(closes, 1, "a press dragged off the control does not close");
    assert_eq!(forwarded_presses(&log), 0);
    assert!(window.is_visible());
}

#[test]
fn hovering_a_caption_control_emits_caption_hover() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let app = "com.aqua.test.hover";
    let pointer = VirtualPointer::new(&compositor).unwrap();
    let window = custom(&compositor, app);
    let (log, _) = gesture(
        compositor,
        pointer,
        window,
        app,
        vec![
            Act::Move(200, 150),
            Act::Move(WIDTH as i32 - 60, 20),
            Act::Move(WIDTH as i32 - 20, 20),
            Act::Move(200, 150),
        ],
        |_, _, context| context.count(|event| matches!(event, ShellEvent::CaptionHover(_))) >= 3,
    );
    let hovers: Vec<ShellEvent> = events(&log)
        .into_iter()
        .filter(|event| matches!(event, ShellEvent::CaptionHover(_)))
        .collect();
    assert_eq!(
        hovers,
        vec![
            ShellEvent::CaptionHover(Some(CaptionControl::Maximize)),
            ShellEvent::CaptionHover(Some(CaptionControl::Close)),
            ShellEvent::CaptionHover(None),
        ]
    );
}

#[test]
fn native_titlebar_without_server_decorations_falls_back_to_a_drag_band() {
    let Some(compositor) = support::compositor_with(Options {
        hidden_globals: vec!["zxdg_decoration_manager_v1".into()],
        ..Options::default()
    }) else {
        return;
    };
    let app = "com.aqua.test.native";
    floating(&compositor, app);
    let pointer = VirtualPointer::new(&compositor).unwrap();
    let window = Window::create(&options(app, (WIDTH, HEIGHT))).unwrap();
    assert!(!window.has_server_decorations());
    let (log, before) = gesture(
        compositor.clone(),
        pointer,
        window,
        app,
        vec![
            Act::Move(100, 20),
            Act::Press,
            Act::Move(140, 50),
            Act::Move(180, 80),
            Act::Release,
            Act::Move(100, 200),
            Act::Press,
            Act::Release,
        ],
        |_, _, context| {
            context.saw(|event| matches!(event, ShellEvent::Input(InputEvent::PointerUp(..))))
        },
    );
    let after = compositor.view_rect(app).unwrap();
    assert_eq!((after.x - before.x, after.y - before.y), (80, 60));
    assert_eq!(
        forwarded_presses(&log),
        2,
        "below the band the app gets the click"
    );
    assert!(
        !events(&log)
            .iter()
            .any(|event| matches!(event, ShellEvent::CaptionHover(Some(_)))),
        "the fallback draws no caption buttons"
    );
}

#[test]
fn a_native_titlebar_with_server_decorations_leaves_the_whole_surface_to_the_app() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let app = "com.aqua.test.server";
    floating(&compositor, app);
    let pointer = VirtualPointer::new(&compositor).unwrap();
    let window = Window::create(&options(app, (WIDTH, HEIGHT))).unwrap();
    assert!(window.has_server_decorations());
    let (log, _) = gesture(
        compositor,
        pointer,
        window,
        app,
        vec![Act::Move(100, 5), Act::Press, Act::Release],
        |_, _, context| {
            context.saw(|event| matches!(event, ShellEvent::Input(InputEvent::PointerUp(..))))
        },
    );
    assert_eq!(forwarded_presses(&log), 2);
}
