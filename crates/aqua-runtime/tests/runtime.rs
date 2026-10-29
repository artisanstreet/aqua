use aqua_core::{DisplayItem, InputEvent, Point, PointerButton, PointerEvent, PointerKind, Size};
use aqua_protocol::{FrameReason, Input, Instant};
use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Text as UiText};
use aqua_ui::motion::{Easing, MotionKind, MotionValue};
use aqua_ui::style::{px, Style};

fn mounted() -> Runtime {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            UiBox::new()
                .child(
                    UiText::builder()
                        .arg("draft")
                        .editable(true)
                        .style(Style::new().width(px(200.0)).height(px(32.0)))
                        .build(),
                )
                .build()
        })
        .unwrap();
    runtime
}

fn press(at: Point) -> InputEvent {
    InputEvent::PointerDown(
        PointerEvent {
            contact: 0,
            kind: PointerKind::Mouse,
            position: at,
            buttons: 1,
        },
        PointerButton::Primary,
    )
}

fn moved(at: Point) -> InputEvent {
    InputEvent::PointerMove(PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: at,
        buttons: 0,
    })
}

fn script() -> Vec<Input> {
    vec![
        Input::Viewport {
            size: Size::new(400.0, 300.0),
            scale: 1.0,
        },
        Input::Frame(Instant::from_secs_f64(0.0)),
        Input::Tick(Instant::from_secs_f64(0.1)),
        Input::Event(press(Point::new(10.0, 10.0))),
        Input::Event(InputEvent::TextInput {
            text: "!".to_string(),
        }),
        Input::Frame(Instant::from_secs_f64(0.116)),
    ]
}

fn scene_items(runtime: &Runtime) -> Vec<DisplayItem> {
    runtime
        .scene()
        .ordered()
        .map(|(_, item)| item.clone())
        .collect()
}

fn tween(secs: f64) -> MotionKind {
    MotionKind::Tween {
        duration_secs: secs,
        easing: Easing::Linear,
    }
}

#[test]
fn replaying_an_input_log_reproduces_the_scene() {
    let mut first = mounted();
    let mut second = mounted();
    let first_outputs: Vec<_> = script()
        .into_iter()
        .map(|input| first.step(input).unwrap())
        .collect();
    let second_outputs: Vec<_> = script()
        .into_iter()
        .map(|input| second.step(input).unwrap())
        .collect();
    assert_eq!(first_outputs, second_outputs);
    assert!(!first.scene().is_empty());
    assert_eq!(scene_items(&first), scene_items(&second));
}

#[test]
fn frame_wake_follows_motion_in_protocol_time() {
    let mut runtime = mounted();
    runtime
        .step(Input::Viewport {
            size: Size::new(400.0, 300.0),
            scale: 1.0,
        })
        .unwrap();
    let motion = runtime.host_mut().motion_mut();
    let handle = motion.spawn(None, MotionValue::Scalar(0.0), tween(0.2), 0.0);
    motion
        .set_target(handle, MotionValue::Scalar(1.0), 0.0)
        .unwrap();

    let now = Instant::from_secs_f64(0.05);
    let wake = runtime.step(Input::Frame(now)).unwrap().wake_at;
    assert!(wake.is_some_and(|at| at >= now && at <= Instant::from_secs_f64(0.2)));

    let settled = runtime
        .step(Input::Frame(Instant::from_secs_f64(1.0)))
        .unwrap();
    assert_eq!(settled.wake_at, None);
}

#[test]
fn unconsumed_input_during_motion_requests_only_animation() {
    let mut runtime = mounted();
    runtime
        .step(Input::Viewport {
            size: Size::new(400.0, 300.0),
            scale: 1.0,
        })
        .unwrap();
    runtime
        .step(Input::Frame(Instant::from_secs_f64(0.0)))
        .unwrap();
    let motion = runtime.host_mut().motion_mut();
    let handle = motion.spawn(None, MotionValue::Scalar(0.0), tween(0.5), 0.0);
    motion
        .set_target(handle, MotionValue::Scalar(1.0), 0.0)
        .unwrap();

    let output = runtime
        .step(Input::Event(moved(Point::new(390.0, 290.0))))
        .unwrap();
    assert!(output.redraw.contains(FrameReason::ANIMATION));
    assert!(!output.redraw.contains(FrameReason::INPUT));

    runtime
        .step(Input::Frame(Instant::from_secs_f64(2.0)))
        .unwrap();
    let idle = runtime
        .step(Input::Event(moved(Point::new(390.0, 290.0))))
        .unwrap();
    assert!(idle.redraw.is_empty());
}

#[test]
fn time_never_runs_backwards() {
    let mut runtime = mounted();
    runtime
        .step(Input::Tick(Instant::from_secs_f64(2.0)))
        .unwrap();
    runtime
        .step(Input::Tick(Instant::from_secs_f64(1.0)))
        .unwrap();
    assert_eq!(runtime.now_secs(), 2.0);
}
