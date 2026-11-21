#![cfg(target_os = "linux")]

mod support;

use aqua_shell_wayland::testing::{Compositor, Importer, VirtualPointer};
use aqua_shell_wayland::Window;
use std::time::Duration;
use support::{drive, options, ready, Flow};

fn urgent(compositor: &Compositor, app_id: &str) -> Option<bool> {
    let mut pending = vec![compositor.tree().ok()?];
    while let Some(node) = pending.pop() {
        if node["app_id"] == app_id {
            return node["urgent"].as_bool();
        }
        for key in ["nodes", "floating_nodes"] {
            pending.extend(node[key].as_array().into_iter().flatten().cloned());
        }
    }
    None
}

#[test]
fn an_exported_window_handle_is_importable_by_another_client_until_the_window_closes() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let window = Window::create(&options("com.aqua.test.exported", (320.0, 240.0))).unwrap();
    let handle = window
        .exported_handle()
        .expect("sway exports toplevels through xdg-foreign");
    assert!(!handle.is_empty());
    assert_eq!(
        window.exported_handle().as_deref(),
        Ok(handle.as_str()),
        "a window is exported once and keeps its handle"
    );
    let mut importer = Importer::connect(&compositor).unwrap();
    let unknown = importer.import("not-an-exported-handle").unwrap();
    assert!(
        !importer.alive(unknown).unwrap(),
        "the compositor refuses handles nobody exported"
    );
    let imported = importer.import(&handle).unwrap();
    assert!(
        importer.alive(imported).unwrap(),
        "another client can import the exported window"
    );
    window.destroy();
    drive(&[], Duration::from_secs(5), move |_| {
        match importer.alive(imported).unwrap() {
            true => Flow::Wait(Duration::from_millis(20)),
            false => Flow::Done,
        }
    });
}

#[test]
fn an_activation_token_handed_out_after_a_click_draws_attention_to_another_window() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let ids = [
        "com.aqua.test.token.a",
        "com.aqua.test.token.b",
        "com.aqua.test.token.c",
    ];
    let pointer = VirtualPointer::new(&compositor).unwrap();
    let windows: Vec<Window> = ids
        .iter()
        .map(|id| Window::create(&options(id, (200.0, 150.0))).unwrap())
        .collect();
    let [clicked, attended, ignored] = [windows[0], windows[1], windows[2]];
    let probe = compositor.clone();
    let mut phase = 0;
    let mut target = None;
    drive(&windows, Duration::from_secs(10), move |context| {
        let on_clicked = |wanted: &dyn Fn(&aqua_shell::ShellEvent) -> bool| {
            context
                .log
                .iter()
                .any(|(id, event, _)| *id == clicked.id() && wanted(event))
        };
        let pointed_at = context
            .log
            .iter()
            .rev()
            .find_map(|(id, event, _)| match event {
                aqua_shell::ShellEvent::Input(aqua_core::InputEvent::PointerMove(pointer))
                    if *id == clicked.id() =>
                {
                    Some(pointer.position)
                }
                _ => None,
            });
        let clicked_up = on_clicked(&|event| {
            matches!(
                event,
                aqua_shell::ShellEvent::Input(aqua_core::InputEvent::PointerUp(..))
            )
        });
        match phase {
            0 if ids.iter().all(|id| probe.view_rect(id).is_some()) => {
                probe
                    .command(&format!("[app_id=\"^{}$\"] focus", ids[0]))
                    .unwrap();
                phase = 1;
            }
            1 => {
                if let Some(rect) = ready(&probe, ids[0]) {
                    target = Some(rect);
                    pointer
                        .move_to(
                            (rect.x + rect.width / 2) as u32,
                            (rect.y + rect.height / 2) as u32,
                        )
                        .unwrap();
                    phase = 2;
                }
            }
            2 => {
                let settled = target
                    .zip(ready(&probe, ids[0]))
                    .filter(|(aimed, now)| aimed == now);
                match settled {
                    Some((rect, _))
                        if pointed_at
                            == Some(aqua_core::Point::new(
                                (rect.width / 2) as f32,
                                (rect.height / 2) as f32,
                            )) =>
                    {
                        pointer.press(VirtualPointer::LEFT).unwrap();
                        pointer.release(VirtualPointer::LEFT).unwrap();
                        phase = 3;
                    }
                    Some(_) => {}
                    None => phase = 1,
                }
            }
            3 if clicked_up => {
                let token = clicked
                    .activation_token()
                    .expect("a focused, clicked window gets an activation token");
                assert!(!token.is_empty());
                ignored.activate_with_token("not-a-token").unwrap();
                attended.activate_with_token(&token).unwrap();
                phase = 4;
            }
            4 if urgent(&probe, ids[1]) == Some(true) => {
                assert_eq!(
                    urgent(&probe, ids[2]),
                    Some(false),
                    "an unknown token draws no attention"
                );
                return Flow::Done;
            }
            _ => {}
        }
        Flow::Wait(Duration::from_millis(20))
    });
}
