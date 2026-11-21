#[cfg(target_os = "macos")]
mod support;

fn main() {
    #[cfg(target_os = "macos")]
    support::main(&cases::ALL);
    #[cfg(not(target_os = "macos"))]
    println!("ime: skipped; requires macOS");
}

#[cfg(target_os = "macos")]
mod cases {
    use super::support::{self, case, inputs, script, stage, Case, Stage};
    use aqua_core::{ImeEvent as Composition, InputEvent, Modifiers, Rect};
    use aqua_ime_appkit::AppKitIme;
    use aqua_protocol::{
        ImeBackend, ImeCommand, ImeDocument, ImeEvent, ImeGeometry, ImeLine, ImeStop, Utf16Range,
    };
    use aqua_shell_appkit::testing::input_source::{self, KOTOERI_HIRAGANA};
    use aqua_shell_appkit::testing::{self, post, text_input};
    use aqua_shell_appkit::{ShellEvent, Window};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    pub const ALL: [Case; 6] = [
        case(
            "marked_text_shows_as_a_composition_and_commits_once",
            commits_once,
        ),
        case("commit_on_blur_inserts_the_marked_text_once", blur),
        case("cancel_discards_the_marked_text", cancel),
        case(
            "the_input_method_can_reconvert_the_selection_through_the_attributed_substring",
            reconvert,
        ),
        case(
            "first_rect_for_the_caret_lies_inside_the_window_on_screen",
            first_rect,
        ),
        case("kotoeri_composes_through_posted_keys", kotoeri),
    ];

    const LIMIT: Duration = Duration::from_secs(30);
    const KEY_K: u16 = 40;
    const KEY_A: u16 = 0;
    const KEY_RETURN: u16 = 36;

    type Shared = Rc<RefCell<AppKitIme>>;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    fn document(text: &str, selection: (u32, u32)) -> ImeDocument {
        let length = text.encode_utf16().count() as u32;
        ImeDocument {
            window_start: 0,
            total_len: length,
            text: text.into(),
            selection: Utf16Range::new(selection.0, selection.1),
            selection_reversed: false,
            area: rect(20.0, 40.0, 280.0, 24.0),
            geometry: ImeGeometry {
                clip: rect(0.0, 0.0, 320.0, 240.0),
                lines: vec![ImeLine {
                    top: 40.0,
                    bottom: 64.0,
                    stops: (0..=length)
                        .map(|offset| ImeStop {
                            offset,
                            x: 24.0 + 10.0 * offset as f32,
                        })
                        .collect(),
                }],
            },
        }
    }

    fn field(title: &str, text: &str, selection: (u32, u32)) -> (Window, Shared) {
        let window = Window::create(&support::options(title, (320.0, 240.0))).expect("window");
        let ime = Rc::new(RefCell::new(AppKitIme::new(window)));
        ime.borrow_mut()
            .command(ImeCommand::Enable(document(text, selection)))
            .expect("enabling is never refused");
        (window, ime)
    }

    fn activate() -> Stage {
        stage("activate", testing::activate, |cx| {
            cx.saw(|event| *event == ShellEvent::Focused(true))
        })
    }

    fn drained(ime: &Shared) -> Vec<ImeEvent> {
        let mut out = Vec::new();
        ime.borrow_mut().drain(&mut out);
        out
    }

    fn composition(event: Composition) -> ImeEvent {
        ImeEvent::Composition(event)
    }

    fn commits(events: &[ImeEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                ImeEvent::Composition(Composition::Commit { text }) => Some(text.clone()),
                _ => None,
            })
            .collect()
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

    fn commits_once() {
        if !support::appkit() {
            return;
        }
        let (window, ime) = field("compose", "ab", (2, 2));
        let seen = Rc::new(RefCell::new(None));
        let during = seen.clone();
        let log = script(
            LIMIT,
            vec![
                activate(),
                stage(
                    "compose and commit",
                    move || {
                        text_input::set_marked(window, "\u{304b}", (1, 0), None);
                        *during.borrow_mut() = Some((
                            text_input::marked_range(window),
                            text_input::selected_range(window),
                            text_input::attributed_substring(window, (0, 3)),
                        ));
                        text_input::insert(window, "\u{304b}", None);
                        text_input::unmark(window);
                    },
                    |_| true,
                ),
            ],
        );
        assert_eq!(
            *seen.borrow(),
            Some((
                Some((2, 1)),
                Some((3, 0)),
                Some(("ab\u{304b}".into(), (0, 3)))
            )),
            "the input method sees the marked text in the document"
        );
        assert_eq!(
            drained(&ime),
            vec![
                composition(Composition::Start),
                composition(Composition::Update {
                    marked: "\u{304b}".into(),
                    cursor: Some((3, 3))
                }),
                composition(Composition::Commit {
                    text: "\u{304b}".into()
                }),
            ]
        );
        assert_eq!(text_input::marked_range(window), None);
        assert_eq!(
            typed(&log),
            "",
            "a committed composition is not typed again"
        );
    }

    fn blur() {
        if !support::appkit() {
            return;
        }
        let (window, ime) = field("blur", "", (0, 0));
        let log = script(
            LIMIT,
            vec![
                activate(),
                stage(
                    "compose and hide the key window",
                    move || {
                        text_input::set_marked(window, "\u{65e5}", (1, 0), None);
                        window.hide();
                    },
                    move |cx| cx.saw_on(window.id(), |event| *event == ShellEvent::Focused(false)),
                ),
                stage(
                    "the input method repeats its text",
                    move || text_input::insert(window, "\u{65e5}", None),
                    |_| true,
                ),
            ],
        );
        let events = drained(&ime);
        assert_eq!(commits(&events), vec!["\u{65e5}".to_owned()], "{events:#?}");
        assert_eq!(typed(&log), "");
    }

    fn cancel() {
        if !support::appkit() {
            return;
        }
        let (window, ime) = field("cancel", "ab", (2, 2));
        let cancelling = ime.clone();
        let log = script(
            LIMIT,
            vec![
                activate(),
                stage(
                    "compose and cancel",
                    move || {
                        text_input::set_marked(window, "\u{304b}", (1, 0), None);
                        cancelling
                            .borrow_mut()
                            .command(ImeCommand::CancelComposition)
                            .expect("cancelling is never refused");
                    },
                    |_| true,
                ),
            ],
        );
        let events = drained(&ime);
        assert_eq!(events.last(), Some(&composition(Composition::Cancel)));
        assert!(commits(&events).is_empty(), "{events:#?}");
        assert_eq!(text_input::marked_range(window), None);
        assert_eq!(
            text_input::attributed_substring(window, (0, 2)),
            Some(("ab".into(), (0, 2)))
        );
        assert_eq!(typed(&log), "");
    }

    fn reconvert() {
        if !support::appkit() {
            return;
        }
        let (window, ime) = field("reconvert", "\u{65e5}\u{672c}\u{8a9e}", (0, 2));
        let read = Rc::new(RefCell::new(None));
        let reading = read.clone();
        script(
            LIMIT,
            vec![
                activate(),
                stage(
                    "reconvert",
                    move || {
                        let selected =
                            text_input::selected_range(window).expect("the field has a selection");
                        *reading.borrow_mut() = text_input::attributed_substring(window, selected);
                        text_input::set_marked(
                            window,
                            "\u{306b}\u{307b}\u{3093}",
                            (0, 3),
                            Some(selected),
                        );
                        text_input::insert(window, "\u{4e8c}\u{672c}", None);
                    },
                    |_| true,
                ),
            ],
        );
        assert_eq!(*read.borrow(), Some(("\u{65e5}\u{672c}".into(), (0, 2))));
        let events = drained(&ime);
        assert_eq!(
            events.first(),
            Some(&ImeEvent::Replace {
                range: Utf16Range::new(0, 2),
                text: String::new()
            })
        );
        assert_eq!(commits(&events), vec!["\u{4e8c}\u{672c}".to_owned()]);
        assert_eq!(
            text_input::attributed_substring(window, (0, 3)),
            Some(("\u{4e8c}\u{672c}\u{8a9e}".into(), (0, 3)))
        );
    }

    fn first_rect() {
        if !support::appkit() {
            return;
        }
        let (window, _ime) = field("caret", "hello", (2, 2));
        let answer = Rc::new(RefCell::new(None));
        let asking = answer.clone();
        script(
            LIMIT,
            vec![
                activate(),
                stage(
                    "ask",
                    move || {
                        *asking.borrow_mut() = Some(text_input::first_rect(window, (2, 0)));
                    },
                    |_| true,
                ),
            ],
        );
        let ((x, y, width, height), actual) = answer.borrow().expect("the rect was asked for");
        let frame = testing::frame(window).expect("the window is on screen");
        let view = window.view_on_screen().expect("the view is on screen");
        assert_eq!(actual, (2, 0));
        assert_eq!((width, height), (0.0, 24.0));
        assert_eq!(x, view.x + 44.0, "the caret sits after two characters");
        assert_eq!(
            y,
            view.y + view.height - 64.0,
            "the caret bottom is 64 points below the view top"
        );
        assert!(
            x >= frame.x
                && x <= frame.x + frame.width
                && y >= frame.y
                && y + height <= frame.y + frame.height,
            "the caret {x},{y} lies inside the window frame {frame:?}"
        );
    }

    fn kotoeri() {
        if !support::input_method() {
            return;
        }
        let selected = input_source::select(KOTOERI_HIRAGANA).unwrap_or_else(|error| {
            panic!("AQUA_REQUIRE_MAC_IME needs Kotoeri selectable: {error}")
        });
        println!("input source: {:?}", input_source::current());
        let (_window, ime) = field("kotoeri", "", (0, 0));
        let composing = ime.clone();
        let settled = ime.clone();
        let log = script(
            LIMIT,
            vec![
                activate(),
                stage(
                    "type k a",
                    || {
                        post::tap(KEY_K, Modifiers::NONE);
                        post::tap(KEY_A, Modifiers::NONE);
                    },
                    move |cx| {
                        composing.borrow().is_composing()
                            || cx.saw(|event| {
                                matches!(event, ShellEvent::Input(InputEvent::TextInput { .. }))
                            })
                    },
                ),
                stage(
                    "return",
                    || post::tap(KEY_RETURN, Modifiers::NONE),
                    move |_| !settled.borrow().is_composing(),
                ),
            ],
        );
        drop(selected);
        let events = drained(&ime);
        println!("kotoeri events: {events:?}");
        assert_eq!(
            typed(&log),
            "",
            "no Latin text reaches the app: {events:#?}"
        );
        let last_marked = events
            .iter()
            .rev()
            .find_map(|event| match event {
                ImeEvent::Composition(Composition::Update { marked, .. }) => Some(marked.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("Kotoeri marked text while typing: {events:#?}"));
        assert!(
            !last_marked.is_ascii(),
            "the marked text is Japanese: {last_marked:?}"
        );
        assert_eq!(
            commits(&events),
            vec![last_marked],
            "Return commits the marked text exactly once"
        );
    }
}
