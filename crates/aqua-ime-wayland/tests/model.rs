use aqua_core::{ImeEvent as Composition, Rect};
use aqua_ime_wayland::model::{Batch, Excerpt, Link, Outgoing, Session, SURROUNDING_LIMIT};
use aqua_protocol::{ImeCommand, ImeDocument, ImeEvent, ImeGeometry, Utf16Range};

fn document(text: &str, window_start: u32, selection: (u32, u32), reversed: bool) -> ImeDocument {
    ImeDocument {
        window_start,
        total_len: window_start + text.encode_utf16().count() as u32,
        text: text.to_owned(),
        selection: Utf16Range::new(selection.0, selection.1),
        selection_reversed: reversed,
        area: Rect {
            x: 10.0,
            y: 20.0,
            w: 2.0,
            h: 16.0,
        },
        geometry: ImeGeometry::UNAVAILABLE,
    }
}

#[derive(Default)]
struct Wire {
    commits: u32,
    sent: Vec<&'static str>,
}

impl Link for Wire {
    fn enable(&mut self, _: &Outgoing) -> Option<u32> {
        self.commits += 1;
        self.sent.push("enable");
        Some(self.commits)
    }

    fn update(&mut self, _: &Outgoing) -> Option<u32> {
        self.commits += 1;
        self.sent.push("update");
        Some(self.commits)
    }

    fn disable(&mut self) -> Option<u32> {
        self.commits += 1;
        self.sent.push("disable");
        Some(self.commits)
    }
}

fn drained(session: &mut Session) -> Vec<ImeEvent> {
    let mut out = Vec::new();
    session.drain(&mut out);
    out
}

fn commit(text: &str, serial: u32) -> Batch {
    Batch {
        commit: Some(text.into()),
        serial,
        ..Batch::default()
    }
}

fn preedit(text: &str, serial: u32) -> Batch {
    Batch {
        preedit: Some((text.into(), Some((text.len(), text.len())))),
        serial,
        ..Batch::default()
    }
}

fn composing(text: &str) -> (Session, Wire) {
    let mut wire = Wire::default();
    let mut session = Session::new(true);
    session
        .command(
            ImeCommand::Enable(document("ab", 0, (2, 2), false)),
            &mut wire,
        )
        .unwrap();
    session.done(preedit(text, wire.commits));
    drained(&mut session);
    (session, wire)
}

#[test]
fn utf16_selection_maps_to_utf8_surrounding_offsets_for_astral_text() {
    let excerpt = Excerpt::of(&document("a😀b日c", 10, (13, 15), false));
    assert_eq!(excerpt.text, "a😀b日c");
    assert_eq!(excerpt.base, 10);
    assert_eq!((excerpt.anchor, excerpt.cursor), (5, 9));
    assert_eq!(excerpt.utf16(i64::from(excerpt.cursor)), 15);
    let reversed = Excerpt::of(&document("a😀b日c", 10, (13, 15), true));
    assert_eq!((reversed.cursor, reversed.anchor), (5, 9));
    let outside = Excerpt::of(&document("a😀b", 10, (2, 40), false));
    assert_eq!((outside.anchor, outside.cursor), (0, 6));
}

#[test]
fn surrounding_text_is_clamped_to_4000_bytes_around_the_caret() {
    let text = "æøå 😀 ".repeat(1000);
    let units: Vec<u16> = text.encode_utf16().collect();
    let caret = (units.len() / 2) as u32;
    let excerpt = Excerpt::of(&document(&text, 7, (7 + caret, 7 + caret), false));
    assert!(
        excerpt.text.len() <= SURROUNDING_LIMIT,
        "{}",
        excerpt.text.len()
    );
    assert!(excerpt.text.len() > SURROUNDING_LIMIT - 8);
    assert!(text.contains(&excerpt.text));
    assert_eq!(excerpt.utf16(i64::from(excerpt.cursor)), 7 + caret);
    let middle = excerpt.cursor as usize;
    assert!(
        middle.abs_diff(excerpt.text.len() / 2) <= 8,
        "caret at {middle}"
    );
    let head = Excerpt::of(&document(&text, 0, (3, 3), false));
    assert_eq!((head.base, head.text.len() <= SURROUNDING_LIMIT), (0, true));
    assert_eq!(head.utf16(i64::from(head.cursor)), 3);
}

#[test]
fn delete_surrounding_then_commit_is_one_replace() {
    let mut wire = Wire::default();
    let mut session = Session::new(true);
    session
        .command(
            ImeCommand::Enable(document("hello wörld", 0, (11, 11), false)),
            &mut wire,
        )
        .unwrap();
    session.done(Batch {
        commit: Some("ld!".into()),
        delete: Some((2, 0)),
        serial: wire.commits,
        ..Batch::default()
    });
    assert_eq!(
        drained(&mut session),
        vec![ImeEvent::Replace {
            range: Utf16Range::new(9, 11),
            text: "ld!".into(),
        }]
    );
}

#[test]
fn two_commits_before_the_app_syncs_land_in_order() {
    let mut wire = Wire::default();
    let mut session = Session::new(true);
    session
        .command(
            ImeCommand::Enable(document("ab", 0, (2, 2), false)),
            &mut wire,
        )
        .unwrap();
    session.done(commit("日", wire.commits));
    session.done(commit("😀", wire.commits));
    assert_eq!(
        drained(&mut session),
        vec![
            ImeEvent::Replace {
                range: Utf16Range::new(2, 2),
                text: "日".into(),
            },
            ImeEvent::Replace {
                range: Utf16Range::new(3, 3),
                text: "😀".into(),
            },
        ]
    );
}

#[test]
fn a_preedit_cleared_without_a_commit_cancels_the_composition() {
    let (mut session, wire) = composing("にほ");
    session.done(Batch {
        serial: wire.commits,
        ..Batch::default()
    });
    assert_eq!(
        drained(&mut session),
        vec![ImeEvent::Composition(Composition::Cancel)]
    );
    assert!(!session.is_composing());
}

#[test]
fn committing_the_composition_inserts_the_preedit_once_even_when_the_input_method_echoes_it() {
    let (mut session, mut wire) = composing("にほん");
    session
        .command(ImeCommand::CommitComposition, &mut wire)
        .unwrap();
    assert_eq!(wire.sent[wire.sent.len() - 2..], ["disable", "enable"]);
    session.done(commit("にほん", wire.commits - 1));
    session.done(commit("にほん", wire.commits));
    session.done(commit("x", wire.commits));
    assert_eq!(
        drained(&mut session),
        vec![
            ImeEvent::Composition(Composition::Commit {
                text: "にほん".into()
            }),
            ImeEvent::Replace {
                range: Utf16Range::new(5, 5),
                text: "x".into(),
            },
        ]
    );
}

#[test]
fn cancelling_the_composition_ignores_a_late_commit_of_the_discarded_preedit() {
    let (mut session, mut wire) = composing("にほん");
    session
        .command(ImeCommand::CancelComposition, &mut wire)
        .unwrap();
    session.done(commit("日本", wire.commits - 2));
    session.done(commit("にほん", wire.commits));
    session.done(preedit("か", wire.commits));
    assert_eq!(
        drained(&mut session),
        vec![
            ImeEvent::Composition(Composition::Cancel),
            ImeEvent::Composition(Composition::Start),
            ImeEvent::Composition(Composition::Update {
                marked: "か".into(),
                cursor: Some((3, 3)),
            }),
        ]
    );
}

#[test]
fn a_fresh_commit_equal_to_the_cancelled_preedit_is_kept_once_a_new_composition_began() {
    let (mut session, mut wire) = composing("a");
    session
        .command(ImeCommand::CancelComposition, &mut wire)
        .unwrap();
    session.done(preedit("a", wire.commits));
    session.done(commit("a", wire.commits));
    let events = drained(&mut session);
    assert_eq!(
        events.last(),
        Some(&ImeEvent::Composition(Composition::Commit {
            text: "a".into()
        }))
    );
}

#[test]
fn losing_focus_mid_composition_commits_it_like_windows_does() {
    let (mut session, mut wire) = composing("かな");
    session.left();
    assert_eq!(
        drained(&mut session),
        vec![ImeEvent::Composition(Composition::Commit {
            text: "かな".into()
        })]
    );
    session.done(commit("late", wire.commits));
    assert_eq!(drained(&mut session), Vec::new());
    session.entered(&mut wire);
    assert_eq!(wire.sent.last(), Some(&"enable"));
    session.done(commit("z", wire.commits));
    assert_eq!(
        drained(&mut session),
        vec![ImeEvent::Replace {
            range: Utf16Range::new(4, 4),
            text: "z".into(),
        }]
    );
}

#[test]
fn enabling_before_focus_waits_for_enter_then_activates() {
    let mut wire = Wire::default();
    let mut session = Session::new(false);
    session
        .command(
            ImeCommand::Enable(document("", 0, (0, 0), false)),
            &mut wire,
        )
        .unwrap();
    assert!(wire.sent.is_empty());
    session
        .command(
            ImeCommand::Update(document("q", 0, (1, 1), false)),
            &mut wire,
        )
        .unwrap();
    session.entered(&mut wire);
    assert_eq!(wire.sent, ["enable"]);
    session.done(commit("r", wire.commits));
    assert_eq!(
        drained(&mut session),
        vec![ImeEvent::Replace {
            range: Utf16Range::new(1, 1),
            text: "r".into(),
        }]
    );
}

#[test]
fn an_unchanged_document_sends_nothing_so_in_flight_input_method_commits_stay_valid() {
    let (mut session, mut wire) = composing("にほ");
    let sent = wire.sent.len();
    session
        .command(
            ImeCommand::Update(document("ab", 0, (2, 2), false)),
            &mut wire,
        )
        .unwrap();
    assert_eq!(wire.sent.len(), sent);
    session
        .command(
            ImeCommand::Update(document("abc", 0, (3, 3), false)),
            &mut wire,
        )
        .unwrap();
    assert_eq!(wire.sent[sent..], ["update"]);
}
