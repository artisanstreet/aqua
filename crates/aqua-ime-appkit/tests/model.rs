use aqua_core::{ImeEvent as Composition, Rect};
use aqua_ime_appkit::model::{Effect, Inserted, Refusal, ScreenMap, Session};
use aqua_protocol::{ImeCommand, ImeDocument, ImeEvent, ImeGeometry, ImeLine, ImeStop, Utf16Range};

fn range(start: u32, end: u32) -> Utf16Range {
    Utf16Range::new(start, end)
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

fn line(top: f32, bottom: f32, first: u32, count: u32, x: f32) -> ImeLine {
    ImeLine {
        top,
        bottom,
        stops: (0..=count)
            .map(|index| ImeStop {
                offset: first + index,
                x: x + 8.0 * index as f32,
            })
            .collect(),
    }
}

fn document(text: &str, selection: Utf16Range) -> ImeDocument {
    let length = text.encode_utf16().count() as u32;
    ImeDocument {
        window_start: 0,
        total_len: length,
        text: text.into(),
        selection,
        selection_reversed: false,
        area: rect(0.0, 0.0, 300.0, 32.0),
        geometry: ImeGeometry {
            clip: rect(0.0, 0.0, 300.0, 200.0),
            lines: vec![line(10.0, 30.0, 0, length, 4.0)],
        },
    }
}

fn enabled(text: &str, selection: Utf16Range) -> Session {
    let mut session = Session::new();
    let effects = session
        .command(ImeCommand::Enable(document(text, selection)))
        .expect("enabling is never refused");
    assert_eq!(effects, vec![Effect::Install, Effect::Invalidate]);
    session
}

fn drained(session: &mut Session) -> Vec<ImeEvent> {
    let mut out = Vec::new();
    session.drain(&mut out);
    out
}

fn composition(event: Composition) -> ImeEvent {
    ImeEvent::Composition(event)
}

fn commit(text: &str) -> ImeEvent {
    composition(Composition::Commit { text: text.into() })
}

fn update(marked: &str, cursor: (usize, usize)) -> ImeEvent {
    composition(Composition::Update {
        marked: marked.into(),
        cursor: Some(cursor),
    })
}

#[test]
fn typing_outside_a_composition_stays_on_the_ordinary_text_path() {
    let mut session = enabled("hello", range(5, 5));
    assert_eq!(session.insert_text("!", None), Inserted::Typed);
    assert!(drained(&mut session).is_empty());
    assert_eq!(session.selected_range(), Some(range(6, 6)));
    assert_eq!(
        session.attributed_substring(range(0, 6)),
        Some(("hello!".into(), range(0, 6)))
    );
}

#[test]
fn marked_text_shows_as_a_composition_and_commits_once() {
    let mut session = enabled("ab", range(2, 2));
    session.set_marked_text("k", range(1, 1), None);
    session.set_marked_text("\u{304b}", range(1, 1), None);
    assert_eq!(session.marked_range(), Some(range(2, 3)));
    assert_eq!(session.selected_range(), Some(range(3, 3)));
    assert_eq!(session.insert_text("\u{304b}", None), Inserted::Taken);
    assert_eq!(
        drained(&mut session),
        vec![
            composition(Composition::Start),
            update("k", (1, 1)),
            update("\u{304b}", (3, 3)),
            commit("\u{304b}"),
        ]
    );
    assert_eq!(session.marked_range(), None);
    assert_eq!(session.selected_range(), Some(range(3, 3)));
    session.unmark_text();
    assert!(
        drained(&mut session).is_empty(),
        "nothing is committed twice"
    );
}

#[test]
fn marked_text_is_part_of_the_document_the_input_method_reads() {
    let mut session = enabled("ab", range(1, 1));
    session.set_marked_text("xy", range(0, 2), None);
    assert_eq!(
        session.attributed_substring(range(0, 4)),
        Some(("axyb".into(), range(0, 4)))
    );
    assert_eq!(session.selected_range(), Some(range(1, 3)));
}

#[test]
fn a_commit_the_app_asks_for_swallows_the_input_methods_echo_once() {
    let mut session = enabled("", range(0, 0));
    session.set_marked_text("\u{306b}", range(1, 1), None);
    let effects = session
        .command(ImeCommand::CommitComposition)
        .expect("committing is never refused");
    assert_eq!(effects, vec![Effect::Discard]);
    assert_eq!(session.insert_text("\u{306b}", None), Inserted::Taken);
    assert_eq!(
        drained(&mut session),
        vec![
            composition(Composition::Start),
            update("\u{306b}", (3, 3)),
            commit("\u{306b}"),
        ]
    );
    session.key_pressed();
    assert_eq!(
        session.insert_text("\u{306b}", None),
        Inserted::Typed,
        "the same text typed again on a new key is real input"
    );
}

#[test]
fn losing_the_window_commits_the_marked_text_once() {
    let mut session = enabled("x", range(1, 1));
    session.set_marked_text("\u{65e5}", range(1, 1), None);
    session.deactivated();
    assert_eq!(session.insert_text("\u{65e5}", None), Inserted::Taken);
    session.unmark_text();
    let events = drained(&mut session);
    assert_eq!(
        events
            .iter()
            .filter(|event| **event == commit("\u{65e5}"))
            .count(),
        1,
        "{events:?}"
    );
    assert_eq!(
        session.attributed_substring(range(0, 2)),
        Some(("x\u{65e5}".into(), range(0, 2)))
    );
}

#[test]
fn cancel_discards_the_marked_text() {
    let mut session = enabled("ab", range(2, 2));
    session.set_marked_text("\u{304b}", range(1, 1), None);
    let effects = session
        .command(ImeCommand::CancelComposition)
        .expect("cancelling is never refused");
    assert_eq!(effects, vec![Effect::Discard]);
    assert_eq!(session.insert_text("\u{304b}", None), Inserted::Taken);
    assert_eq!(
        drained(&mut session),
        vec![
            composition(Composition::Start),
            update("\u{304b}", (3, 3)),
            composition(Composition::Cancel),
        ]
    );
    assert_eq!(
        session.attributed_substring(range(0, 2)),
        Some(("ab".into(), range(0, 2)))
    );
}

#[test]
fn the_input_method_reconverts_the_selection_and_commits_in_place() {
    let mut session = enabled("\u{65e5}\u{672c}\u{8a9e}", range(0, 2));
    let selected = session.selected_range().expect("a field is focused");
    assert_eq!(
        session.attributed_substring(selected),
        Some(("\u{65e5}\u{672c}".into(), range(0, 2)))
    );
    session.set_marked_text("\u{306b}\u{307b}\u{3093}", range(0, 3), Some(selected));
    assert_eq!(session.marked_range(), Some(range(0, 3)));
    assert_eq!(
        session.insert_text("\u{4e8c}\u{672c}", None),
        Inserted::Taken
    );
    assert_eq!(
        drained(&mut session),
        vec![
            ImeEvent::Replace {
                range: range(0, 2),
                text: String::new()
            },
            composition(Composition::Start),
            update("\u{306b}\u{307b}\u{3093}", (0, 9)),
            commit("\u{4e8c}\u{672c}"),
        ]
    );
    assert_eq!(
        session.attributed_substring(range(0, 3)),
        Some(("\u{4e8c}\u{672c}\u{8a9e}".into(), range(0, 3)))
    );
}

#[test]
fn cancelling_a_reconversion_puts_the_original_text_back() {
    let mut session = enabled("\u{65e5}\u{672c}\u{8a9e}", range(0, 2));
    session.set_marked_text("\u{306b}\u{307b}\u{3093}", range(0, 3), Some(range(0, 2)));
    drained(&mut session);
    session
        .command(ImeCommand::CancelComposition)
        .expect("cancelling is never refused");
    assert_eq!(
        drained(&mut session),
        vec![
            composition(Composition::Cancel),
            ImeEvent::Replace {
                range: range(0, 0),
                text: "\u{65e5}\u{672c}".into()
            },
        ]
    );
    assert_eq!(
        session.attributed_substring(range(0, 3)),
        Some(("\u{65e5}\u{672c}\u{8a9e}".into(), range(0, 3)))
    );
}

#[test]
fn replacing_a_range_maps_to_document_offsets() {
    let mut accent = enabled("cafe", range(4, 4));
    assert_eq!(
        accent.insert_text("\u{e9}", Some(range(3, 4))),
        Inserted::Taken
    );
    assert_eq!(
        drained(&mut accent),
        vec![ImeEvent::Replace {
            range: range(3, 4),
            text: "\u{e9}".into()
        }]
    );
    let mut composing = enabled("abcd", range(2, 2));
    composing.set_marked_text("xy", range(2, 2), None);
    drained(&mut composing);
    assert_eq!(
        composing.insert_text("Z", Some(range(4, 5))),
        Inserted::Taken
    );
    assert_eq!(
        drained(&mut composing),
        vec![
            composition(Composition::Cancel),
            ImeEvent::Replace {
                range: range(2, 3),
                text: "Z".into()
            },
        ]
    );
}

#[test]
fn a_requested_range_outside_the_excerpt_is_not_found() {
    let mut session = Session::new();
    let mut far = document("abc", range(101, 101));
    far.window_start = 100;
    far.total_len = 400;
    session
        .command(ImeCommand::Enable(far))
        .expect("enabling is never refused");
    assert_eq!(session.attributed_substring(range(10, 12)), None);
    assert_eq!(
        session.attributed_substring(range(101, 200)),
        Some(("bc".into(), range(101, 103)))
    );
}

#[test]
fn the_caret_rect_is_in_screen_coordinates_with_a_bottom_left_origin() {
    let session = enabled("hello", range(2, 2));
    let map = ScreenMap {
        left: 100.0,
        top: 800.0,
    };
    let (caret, actual) = session
        .first_rect(range(2, 2), map)
        .expect("the caret has a rect");
    assert_eq!(caret, (120.0, 770.0, 0.0, 20.0));
    assert_eq!(actual, range(2, 2));
    let (word, _) = session
        .first_rect(range(1, 4), map)
        .expect("the range has a rect");
    assert_eq!(word, (112.0, 770.0, 24.0, 20.0));
    assert_eq!(session.character_index((121.0, 780.0), map), Some(2));
    assert_eq!(session.character_index((121.0, 700.0), map), None);
}

#[test]
fn a_range_across_lines_reports_only_its_first_line() {
    let mut session = Session::new();
    let mut two_lines = document("abcdef", range(0, 0));
    two_lines.geometry.lines = vec![line(0.0, 20.0, 0, 3, 0.0), line(20.0, 40.0, 3, 3, 0.0)];
    session
        .command(ImeCommand::Enable(two_lines))
        .expect("enabling is never refused");
    let map = ScreenMap {
        left: 0.0,
        top: 500.0,
    };
    let (rect, actual) = session
        .first_rect(range(1, 5), map)
        .expect("the range has a rect");
    assert_eq!(rect, (8.0, 480.0, 16.0, 20.0));
    assert_eq!(actual, range(1, 3));
}

#[test]
fn disabling_while_composing_commits_and_removes_the_client() {
    let mut session = enabled("", range(0, 0));
    session.set_marked_text("\u{304b}", range(1, 1), None);
    drained(&mut session);
    let effects = session
        .command(ImeCommand::Disable)
        .expect("disabling is never refused");
    assert_eq!(effects, vec![Effect::Discard, Effect::Remove]);
    assert_eq!(drained(&mut session), vec![commit("\u{304b}")]);
    assert_eq!(session.insert_text("a", None), Inserted::Typed);
    assert!(drained(&mut session).is_empty());
}

#[test]
fn the_app_cannot_start_a_reconversion() {
    let mut session = enabled("abc", range(0, 3));
    assert_eq!(
        session.command(ImeCommand::Reconvert),
        Err(Refusal::Reconversion)
    );
    assert!(session.is_active());
}
