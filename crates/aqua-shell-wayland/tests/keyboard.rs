#![cfg(target_os = "linux")]

mod support;

use aqua_core::{InputEvent, Key, KeyEvent, Modifiers};
use aqua_shell_wayland::testing::{Options, VirtualKeyboard};
use aqua_shell_wayland::{ShellEvent, Window};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use support::{drive, inputs, options, ready, Flow};

fn typed(log: &[support::Entry]) -> Vec<String> {
    inputs(log)
        .into_iter()
        .filter_map(|input| match input {
            InputEvent::TextInput { text } => Some(text),
            _ => None,
        })
        .collect()
}

fn script(
    compositor: std::rc::Rc<aqua_shell_wayland::testing::Compositor>,
    app_id: &'static str,
    window: Window,
    keyboard: VirtualKeyboard,
    mut keys: impl FnMut(&mut VirtualKeyboard) + 'static,
    done: impl Fn(&support::Context<'_>) -> bool + 'static,
) -> Vec<support::Entry> {
    let keyboard = Rc::new(RefCell::new(keyboard));
    let mut sent = false;
    drive(&[window], Duration::from_secs(5), move |context| {
        match (done(context), sent, ready(&compositor, app_id).is_some()) {
            (true, _, _) => Flow::Done,
            (false, false, true) => {
                sent = true;
                keys(&mut keyboard.borrow_mut());
                Flow::Wait(Duration::from_millis(20))
            }
            _ => Flow::Wait(Duration::from_millis(20)),
        }
    })
}

#[test]
fn typing_on_a_norwegian_layout_with_a_dead_key_produces_one_composed_character() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    const APP: &str = "com.aqua.test.dead";
    let window = Window::create(&options(APP, (300.0, 200.0))).unwrap();
    let keyboard = VirtualKeyboard::with_layout(&compositor, "no", "").unwrap();
    let log = script(
        compositor.clone(),
        APP,
        window,
        keyboard,
        |keyboard| {
            keyboard.tap(VirtualKeyboard::KEY_RIGHTBRACE).unwrap();
            keyboard.tap(VirtualKeyboard::KEY_U).unwrap();
        },
        |context| {
            context.saw(|event| {
                matches!(
                    event,
                    ShellEvent::Input(InputEvent::KeyUp(KeyEvent {
                        key: Key::Char('u'),
                        ..
                    }))
                )
            })
        },
    );
    assert_eq!(typed(&log), vec!["ü".to_string()]);
}

#[test]
fn alt_gr_does_not_report_control() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    const APP: &str = "com.aqua.test.altgr";
    let window = Window::create(&options(APP, (300.0, 200.0))).unwrap();
    let keyboard = VirtualKeyboard::with_layout(&compositor, "no", "").unwrap();
    let log = script(
        compositor.clone(),
        APP,
        window,
        keyboard,
        |keyboard| {
            keyboard.press(VirtualKeyboard::KEY_RIGHTALT).unwrap();
            keyboard.tap(VirtualKeyboard::KEY_2).unwrap();
            keyboard.release(VirtualKeyboard::KEY_RIGHTALT).unwrap();
        },
        |context| {
            context.saw(|event| matches!(event, ShellEvent::Input(InputEvent::TextInput { .. })))
        },
    );
    assert_eq!(typed(&log), vec!["@".to_string()]);
    let modifiers: Vec<Modifiers> = inputs(&log)
        .into_iter()
        .filter_map(|input| match input {
            InputEvent::KeyDown(event) | InputEvent::KeyUp(event) => Some(event.modifiers),
            InputEvent::ModifiersChanged(modifiers) => Some(modifiers),
            _ => None,
        })
        .collect();
    assert!(!modifiers.is_empty());
    assert!(
        modifiers
            .iter()
            .all(|modifiers| !modifiers.contains(Modifiers::CONTROL)
                && !modifiers.contains(Modifiers::ALT)),
        "AltGr leaked Control or Alt: {modifiers:?}"
    );
}

#[test]
fn holding_a_key_repeats_at_the_compositor_rate_after_the_delay() {
    let Some(compositor) = support::compositor_with(Options {
        config: vec![
            "input type:keyboard repeat_delay 300\ninput type:keyboard repeat_rate 25".into(),
        ],
        ..Options::default()
    }) else {
        return;
    };
    const APP: &str = "com.aqua.test.repeat";
    let window = Window::create(&options(APP, (300.0, 200.0))).unwrap();
    let keyboard = Rc::new(RefCell::new(
        VirtualKeyboard::with_layout(&compositor, "us", "").unwrap(),
    ));
    let mut pressed_at = None;
    let held = keyboard.clone();
    let probe = compositor.clone();
    let log = drive(&[window], Duration::from_secs(5), move |context| {
        match (pressed_at, ready(&probe, APP).is_some()) {
            (None, true) => {
                held.borrow_mut().press(VirtualKeyboard::KEY_A).unwrap();
                pressed_at = Some(context.now);
                Flow::Wait(Duration::from_millis(700))
            }
            (Some(at), _)
                if context.now.saturating_duration_since(at) >= Duration::from_millis(700) =>
            {
                held.borrow_mut().release(VirtualKeyboard::KEY_A).unwrap();
                Flow::Done
            }
            (Some(_), _) => Flow::Wait(Duration::from_millis(20)),
            (None, false) => Flow::Wait(Duration::from_millis(20)),
        }
    });
    let downs: Vec<(bool, aqua_protocol::Instant)> = log
        .iter()
        .filter_map(|(_, event, at)| match event {
            ShellEvent::Input(InputEvent::KeyDown(KeyEvent {
                key: Key::Char('a'),
                repeat,
                ..
            })) => Some((*repeat, *at)),
            _ => None,
        })
        .collect();
    let first = downs.first().expect("the key press arrived");
    assert!(!first.0, "the first key down is not a repeat");
    let repeats: Vec<_> = downs.iter().filter(|(repeat, _)| *repeat).collect();
    assert!(
        repeats.len() >= 8,
        "{} repeats in 700 ms at 25/s after 300 ms",
        repeats.len()
    );
    let delay = repeats[0].1.saturating_duration_since(first.1);
    assert!(
        delay >= Duration::from_millis(295),
        "the first repeat came after {delay:?}"
    );
    assert_eq!(typed(&log).len(), downs.len());
}
