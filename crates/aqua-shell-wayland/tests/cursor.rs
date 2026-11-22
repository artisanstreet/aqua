#![cfg(target_os = "linux")]

mod support;

use aqua_core::CursorIcon;
use aqua_shell_wayland::testing::{capture, Compositor, Options, VirtualPointer};
use aqua_shell_wayland::{Chrome, ShellEvent, Window};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use support::{drive, floating, options, ready, Flow};

const SIZE: u32 = 32;

fn cursor_images(
    compositor: Rc<Compositor>,
    app_id: &'static str,
) -> Vec<(CursorIcon, Vec<[u8; 4]>)> {
    floating(&compositor, app_id);
    let pointer = VirtualPointer::new(&compositor).unwrap();
    let window = Window::create(&aqua_shell_wayland::WindowOptions {
        chrome: Chrome::Custom,
        ..options(app_id, (400.0, 300.0))
    })
    .unwrap();
    let shots = Rc::new(RefCell::new(Vec::new()));
    let taken = shots.clone();
    let icons = [CursorIcon::Hidden, CursorIcon::Text, CursorIcon::Default];
    let mut phase = 0usize;
    let mut at = (0, 0);
    drive(&[window], Duration::from_secs(10), move |context| {
        let entered = context.saw(|event| *event == ShellEvent::PointerEntered);
        match (phase, entered, ready(&compositor, app_id)) {
            (0, _, Some(rect)) => {
                at = ((rect.x + 200) as u32, (rect.y + 150) as u32);
                pointer.move_to(at.0, at.1).unwrap();
                phase = 1;
            }
            (1, true, _) => {
                window.set_cursor(icons[0]);
                phase = 2;
            }
            (step, _, _) if step >= 2 && step - 2 < icons.len() => {
                let shot = capture(&compositor, true).unwrap();
                taken
                    .borrow_mut()
                    .push((icons[step - 2], shot.region(at.0 - 8, at.1 - 8, SIZE)));
                if let Some(next) = icons.get(step - 1) {
                    window.set_cursor(*next);
                }
                phase += 1;
                return Flow::Wait(Duration::from_millis(150));
            }
            (step, _, _) if step >= 2 => return Flow::Done,
            _ => {}
        }
        Flow::Wait(Duration::from_millis(150))
    });
    shots.take()
}

fn assert_distinct(shots: &[(CursorIcon, Vec<[u8; 4]>)]) {
    let background = [0x80u8, 0x80, 0x80, 0xff];
    let image = |icon| {
        shots
            .iter()
            .find(|(shot, _)| *shot == icon)
            .map(|(_, pixels)| pixels.clone())
            .unwrap()
    };
    let hidden = image(CursorIcon::Hidden);
    assert!(
        hidden.iter().all(|pixel| pixel[..3] == background[..3]),
        "a hidden cursor leaves only the window's pixels"
    );
    let text = image(CursorIcon::Text);
    let arrow = image(CursorIcon::Default);
    assert_ne!(text, hidden, "the text cursor is drawn");
    assert_ne!(arrow, hidden, "the arrow is drawn");
    assert_ne!(text, arrow, "the text cursor is not the arrow");
}

#[test]
fn a_text_cursor_request_sets_the_text_shape() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    assert_distinct(&cursor_images(compositor, "com.aqua.test.shape"));
}

#[test]
fn without_cursor_shape_the_theme_cursor_is_drawn_by_the_app() {
    let Some(compositor) = support::compositor_with(Options {
        hidden_globals: vec!["wp_cursor_shape_manager_v1".into()],
        ..Options::default()
    }) else {
        return;
    };
    assert_distinct(&cursor_images(compositor, "com.aqua.test.themed"));
}
