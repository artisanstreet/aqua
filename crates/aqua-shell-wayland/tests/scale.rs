#![cfg(target_os = "linux")]

mod support;

use aqua_shell_wayland::testing::Options;
use aqua_shell_wayland::{ShellEvent, Window};
use std::time::Duration;
use support::{drive, events, floating, options, Flow};

fn rescaled(
    compositor: std::rc::Rc<aqua_shell_wayland::testing::Compositor>,
    app_id: &'static str,
    scale: &'static str,
) -> Vec<ShellEvent> {
    floating(&compositor, app_id);
    let window = Window::create(&options(app_id, (400.0, 300.0))).unwrap();
    let mut asked = false;
    let log = drive(&[window], Duration::from_secs(5), move |context| {
        let changed = context.saw(|event| matches!(event, ShellEvent::ScaleChanged { .. }))
            && context.count(|event| matches!(event, ShellEvent::Resized { .. })) >= 2;
        match (changed, asked, support::mapped(&compositor, app_id)) {
            (true, _, _) => Flow::Done,
            (false, false, Some(_)) => {
                asked = true;
                compositor
                    .command(&format!("output HEADLESS-1 scale {scale}"))
                    .unwrap();
                Flow::Wait(Duration::from_millis(20))
            }
            _ => Flow::Wait(Duration::from_millis(20)),
        }
    });
    events(&log)
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                ShellEvent::ScaleChanged { .. } | ShellEvent::Resized { .. }
            )
        })
        .collect()
}

#[test]
fn fractional_scale_emits_scale_changed_then_resized() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    assert_eq!(
        rescaled(compositor, "com.aqua.test.fractional", "1.5"),
        vec![
            ShellEvent::Resized {
                width: 400,
                height: 300
            },
            ShellEvent::ScaleChanged { scale: 1.5 },
            ShellEvent::Resized {
                width: 600,
                height: 450
            },
        ]
    );
}

#[test]
fn without_fractional_scale_the_integer_buffer_scale_is_used() {
    let Some(compositor) = support::compositor_with(Options {
        hidden_globals: vec!["wp_fractional_scale_manager_v1".into()],
        ..Options::default()
    }) else {
        return;
    };
    assert_eq!(
        rescaled(compositor, "com.aqua.test.integer", "2"),
        vec![
            ShellEvent::Resized {
                width: 400,
                height: 300
            },
            ShellEvent::ScaleChanged { scale: 2.0 },
            ShellEvent::Resized {
                width: 800,
                height: 600
            },
        ]
    );
}
