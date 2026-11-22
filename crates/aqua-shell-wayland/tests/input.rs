#![cfg(target_os = "linux")]

mod support;

use aqua_core::{InputEvent, Point, PointerButton, PointerEvent, PointerKind};
use aqua_shell_wayland::testing::VirtualPointer;
use aqua_shell_wayland::Window;
use std::time::Duration;
use support::{drive, floating, inputs, options, ready, Flow};

#[test]
fn a_virtual_click_arrives_as_pointer_down_and_up_at_the_logical_position() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    floating(&compositor, "com.aqua.test.click");
    let pointer = VirtualPointer::new(&compositor).unwrap();
    let window = Window::create(&options("com.aqua.test.click", (400.0, 300.0))).unwrap();
    let probe = compositor.clone();
    let mut phase = 0;
    let log = drive(&[window], Duration::from_secs(5), move |context| {
        let up = context.saw(|event| {
            matches!(
                event,
                aqua_shell::ShellEvent::Input(InputEvent::PointerUp(..))
            )
        });
        let entered = context.saw(|event| *event == aqua_shell::ShellEvent::PointerEntered);
        match (up, phase, entered, ready(&probe, "com.aqua.test.click")) {
            (true, _, _, _) => return Flow::Done,
            (false, 0, _, Some(rect)) => {
                phase = 1;
                pointer
                    .move_to((rect.x + 100) as u32, (rect.y + 50) as u32)
                    .unwrap();
            }
            (false, 1, true, _) => {
                phase = 2;
                pointer.press(VirtualPointer::LEFT).unwrap();
                pointer.release(VirtualPointer::LEFT).unwrap();
            }
            _ => {}
        }
        Flow::Wait(Duration::from_millis(20))
    });
    let at = Point::new(100.0, 50.0);
    let event = |buttons| PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: at,
        buttons,
    };
    let presses: Vec<InputEvent> = inputs(&log)
        .into_iter()
        .filter(|input| {
            matches!(
                input,
                InputEvent::PointerDown(..) | InputEvent::PointerUp(..)
            )
        })
        .collect();
    assert_eq!(
        presses,
        vec![
            InputEvent::PointerDown(event(1), PointerButton::Primary),
            InputEvent::PointerUp(event(0), PointerButton::Primary),
        ]
    );
}

#[test]
fn one_wheel_notch_scrolls_the_same_distance_as_on_windows() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    floating(&compositor, "com.aqua.test.wheel");
    let pointer = VirtualPointer::new(&compositor).unwrap();
    let window = Window::create(&options("com.aqua.test.wheel", (400.0, 300.0))).unwrap();
    let probe = compositor.clone();
    let mut phase = 0;
    let log = drive(&[window], Duration::from_secs(5), move |context| {
        let seen = context.saw(|event| {
            matches!(
                event,
                aqua_shell::ShellEvent::Input(InputEvent::Scroll { .. })
            )
        });
        let entered = context.saw(|event| *event == aqua_shell::ShellEvent::PointerEntered);
        match (seen, phase, entered, ready(&probe, "com.aqua.test.wheel")) {
            (true, _, _, _) => return Flow::Done,
            (false, 0, _, Some(rect)) => {
                phase = 1;
                pointer
                    .move_to((rect.x + 200) as u32, (rect.y + 150) as u32)
                    .unwrap();
            }
            (false, 1, true, _) => {
                phase = 2;
                pointer.wheel(true, 1).unwrap();
            }
            _ => {}
        }
        Flow::Wait(Duration::from_millis(20))
    });
    let scrolls: Vec<InputEvent> = inputs(&log)
        .into_iter()
        .filter(|input| matches!(input, InputEvent::Scroll { .. }))
        .collect();
    let windows_notch_down = aqua_shell::input::scroll(true, -120);
    assert_eq!(
        scrolls,
        vec![InputEvent::Scroll {
            at: Point::new(200.0, 150.0),
            delta: windows_notch_down
        }]
    );
    assert_eq!(windows_notch_down.y, 16.0);
}
