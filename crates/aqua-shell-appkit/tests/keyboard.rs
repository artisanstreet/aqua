#[cfg(target_os = "macos")]
mod support;

fn main() {
    #[cfg(target_os = "macos")]
    support::main(&cases::ALL);
    #[cfg(not(target_os = "macos"))]
    println!("keyboard: skipped; requires macOS");
}

#[cfg(target_os = "macos")]
mod cases {
    use super::support::{self, case, inputs, script, stage, Case, Stage};
    use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey};
    use aqua_shell_appkit::testing::inject::{self, Keystroke};
    use aqua_shell_appkit::testing::{self, post};
    use aqua_shell_appkit::{ShellEvent, Window};
    use std::time::Duration;

    pub const ALL: [Case; 7] = [
        case(
            "command_c_arrives_as_a_logo_shortcut_and_types_nothing",
            command_c,
        ),
        case("command_c_typed_on_a_cyrillic_layout_is_still_c", cyrillic),
        case("a_held_key_repeats_with_the_repeat_flag", repeat),
        case("enter_reaches_the_app_as_a_key_without_text", enter),
        case(
            "modifier_changes_report_command_as_logo_and_option_as_alt",
            modifiers,
        ),
        case(
            "option_e_then_e_types_e_acute_through_the_dead_key",
            dead_key,
        ),
        case(
            "a_key_posted_through_the_window_server_reaches_the_view",
            posted_key,
        ),
    ];

    const LIMIT: Duration = Duration::from_secs(20);
    const KEY_A: u16 = 0;
    const KEY_C: u16 = 8;
    const KEY_E: u16 = 14;
    const KEY_RETURN: u16 = 0x24;
    const KEY_COMMAND: u16 = 0x37;
    const KEY_OPTION: u16 = 0x3A;

    fn focused(title: &str) -> Window {
        Window::create(&support::options(title, (320.0, 240.0))).expect("window")
    }

    fn activate() -> Stage {
        stage("activate", testing::activate, |cx| {
            cx.saw(|event| *event == ShellEvent::Focused(true))
        })
    }

    fn stroke(code: u16, characters: &'static str, modifiers: Modifiers) -> Keystroke<'static> {
        Keystroke {
            code,
            characters,
            ignoring_modifiers: characters,
            modifiers,
            repeat: false,
        }
    }

    fn key_up_seen(cx: &support::Context<'_>) -> bool {
        cx.saw(|event| matches!(event, ShellEvent::Input(InputEvent::KeyUp(_))))
    }

    fn typed(log: &[support::Entry]) -> String {
        inputs(log)
            .into_iter()
            .filter_map(|input| match input {
                InputEvent::TextInput { text } => Some(text),
                _ => None,
            })
            .collect()
    }

    fn downs(log: &[support::Entry]) -> Vec<KeyEvent> {
        inputs(log)
            .into_iter()
            .filter_map(|input| match input {
                InputEvent::KeyDown(key) => Some(key),
                _ => None,
            })
            .collect()
    }

    fn shortcut(characters: &'static str) {
        if !support::appkit() {
            return;
        }
        let window = focused("command c");
        let keys = stroke(KEY_C, characters, Modifiers::LOGO);
        let log = script(
            LIMIT,
            vec![
                activate(),
                stage(
                    "command c",
                    move || {
                        inject::key_down(window, keys);
                        inject::key_up(window, keys);
                    },
                    key_up_seen,
                ),
            ],
        );
        assert_eq!(
            downs(&log),
            vec![KeyEvent {
                key: Key::Char('c'),
                modifiers: Modifiers::LOGO,
                repeat: false
            }]
        );
        assert_eq!(typed(&log), "", "a shortcut types nothing");
    }

    fn command_c() {
        shortcut("c");
    }

    fn cyrillic() {
        shortcut("\u{441}");
    }

    fn repeat() {
        if !support::appkit() {
            return;
        }
        let window = focused("repeat");
        let held = stroke(KEY_A, "a", Modifiers::NONE);
        let log = script(
            LIMIT,
            vec![
                activate(),
                stage(
                    "hold a",
                    move || {
                        inject::key_down(window, held);
                        inject::key_down(
                            window,
                            Keystroke {
                                repeat: true,
                                ..held
                            },
                        );
                        inject::key_down(
                            window,
                            Keystroke {
                                repeat: true,
                                ..held
                            },
                        );
                        inject::key_up(window, held);
                    },
                    key_up_seen,
                ),
            ],
        );
        let repeats: Vec<bool> = downs(&log).iter().map(|key| key.repeat).collect();
        assert_eq!(repeats, vec![false, true, true]);
        assert_eq!(typed(&log), "aaa");
    }

    fn enter() {
        if !support::appkit() {
            return;
        }
        let window = focused("enter");
        let keys = stroke(KEY_RETURN, "\r", Modifiers::NONE);
        let log = script(
            LIMIT,
            vec![
                activate(),
                stage(
                    "enter",
                    move || {
                        inject::key_down(window, keys);
                        inject::key_up(window, keys);
                    },
                    key_up_seen,
                ),
            ],
        );
        assert_eq!(
            downs(&log).iter().map(|key| key.key).collect::<Vec<_>>(),
            vec![Key::Named(NamedKey::Enter)]
        );
        assert_eq!(typed(&log), "");
    }

    fn modifiers() {
        if !support::appkit() {
            return;
        }
        let window = focused("modifiers");
        let both = Modifiers::LOGO.union(Modifiers::ALT);
        let log = script(
            LIMIT,
            vec![stage(
                "flags",
                move || {
                    inject::modifiers_changed(window, KEY_COMMAND, Modifiers::LOGO);
                    inject::modifiers_changed(window, KEY_OPTION, both);
                    inject::modifiers_changed(window, KEY_OPTION, Modifiers::NONE);
                },
                |cx| {
                    cx.count(|event| {
                        matches!(event, ShellEvent::Input(InputEvent::ModifiersChanged(_)))
                    }) == 3
                },
            )],
        );
        let changes: Vec<Modifiers> = inputs(&log)
            .into_iter()
            .filter_map(|input| match input {
                InputEvent::ModifiersChanged(modifiers) => Some(modifiers),
                _ => None,
            })
            .collect();
        assert_eq!(changes, vec![Modifiers::LOGO, both, Modifiers::NONE]);
    }

    fn post_stroke(code: u16, modifiers: Modifiers) {
        post::key(code, true, modifiers);
        post::key(code, false, modifiers);
    }

    fn dead_key() {
        if !support::posting() {
            return;
        }
        let _window = focused("dead key");
        let log = script(
            LIMIT,
            vec![
                activate(),
                stage(
                    "option e",
                    || {
                        post::key(KEY_OPTION, true, Modifiers::ALT);
                        post_stroke(KEY_E, Modifiers::ALT);
                        post::key(KEY_OPTION, false, Modifiers::NONE);
                    },
                    |cx| cx.elapsed() >= Duration::from_millis(300),
                ),
                stage(
                    "e",
                    || post_stroke(KEY_E, Modifiers::NONE),
                    |cx| {
                        cx.saw(|event| {
                            matches!(event, ShellEvent::Input(InputEvent::TextInput { .. }))
                        })
                    },
                ),
            ],
        );
        assert_eq!(
            typed(&log),
            "\u{e9}",
            "the dead key composes exactly one e acute"
        );
    }

    fn posted_key() {
        if !support::posting() {
            return;
        }
        let _window = focused("posted");
        let log = script(
            LIMIT,
            vec![
                activate(),
                stage("a", || post_stroke(KEY_A, Modifiers::NONE), key_up_seen),
            ],
        );
        assert_eq!(
            downs(&log),
            vec![KeyEvent {
                key: Key::Char('a'),
                modifiers: Modifiers::NONE,
                repeat: false
            }]
        );
        assert_eq!(typed(&log), "a");
    }
}
