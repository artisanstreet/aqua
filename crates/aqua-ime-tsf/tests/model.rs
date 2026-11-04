use aqua_core::{ImeEvent as Composition, Rect};
use aqua_ime_tsf::model::{Model, Selection};
use aqua_protocol::{ImeDocument, ImeEvent, Utf16Range};

fn document(text: &str, selection: (u32, u32)) -> ImeDocument {
    ImeDocument {
        window_start: 0,
        total_len: text.encode_utf16().count() as u32,
        text: text.to_string(),
        selection: Utf16Range::new(selection.0, selection.1),
        selection_reversed: false,
        area: Rect {
            x: 10.0,
            y: 20.0,
            w: 2.0,
            h: 16.0,
        },
    }
}

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn drain(model: &mut Model) -> Vec<ImeEvent> {
    let mut out = Vec::new();
    model.drain(&mut out);
    out
}

#[test]
fn composition_lifecycle_emits_start_updates_and_commit() {
    let mut model = Model::new(&document("ab", (2, 2)));
    model.start_composition(Some(2..2));
    model.insert_at_selection(&units("k")).unwrap();
    model.update_composition(2..3);
    model.replace(2, 3, &units("か")).unwrap();
    model.end_composition();
    assert_eq!(
        drain(&mut model),
        vec![
            ImeEvent::Composition(Composition::Start),
            ImeEvent::Composition(Composition::Update {
                marked: "k".into(),
                cursor: Some((1, 1))
            }),
            ImeEvent::Composition(Composition::Update {
                marked: "か".into(),
                cursor: Some((3, 3))
            }),
            ImeEvent::Composition(Composition::Commit { text: "か".into() }),
        ]
    );
    assert_eq!(model.composition(), None);
    assert_eq!(model.total_len(), 3);
}

#[test]
fn cursor_offsets_are_bytes_of_marked_text_across_surrogate_pairs() {
    let mut model = Model::new(&document("", (0, 0)));
    model.start_composition(Some(0..0));
    model.insert_at_selection(&units("a😀b")).unwrap();
    model.update_composition(0..4);
    model
        .set_selection(Selection {
            start: 3,
            end: 3,
            end_active: true,
        })
        .unwrap();
    let events = drain(&mut model);
    assert_eq!(
        events.last(),
        Some(&ImeEvent::Composition(Composition::Update {
            marked: "a😀b".into(),
            cursor: Some((5, 5))
        }))
    );
}

#[test]
fn text_insertion_outside_composition_commits_at_selection_and_replaces_elsewhere() {
    let mut model = Model::new(&document("hello world", (5, 5)));
    model.insert_at_selection(&units(",")).unwrap();
    model.replace(0, 5, &units("HELLO")).unwrap();
    assert_eq!(
        drain(&mut model),
        vec![
            ImeEvent::Composition(Composition::Commit { text: ",".into() }),
            ImeEvent::Replace {
                range: Utf16Range::new(0, 5),
                text: "HELLO".into()
            },
        ]
    );
    let (text, start) = model.get_text(0, None).unwrap();
    assert_eq!(
        (String::from_utf16_lossy(text), start),
        ("HELLO, world".into(), 0)
    );
}

#[test]
fn reconversion_replaces_selected_range_through_text_store() {
    let mut model = Model::new(&document("変換します", (0, 2)));
    model.start_composition(Some(0..2));
    model.replace(0, 2, &units("へんかん")).unwrap();
    model.update_composition(0..4);
    let events = drain(&mut model);
    assert_eq!(
        events.last(),
        Some(&ImeEvent::Composition(Composition::Update {
            marked: "へんかん".into(),
            cursor: Some((12, 12))
        }))
    );
    assert_eq!(model.composition(), Some(0..4));
}

#[test]
fn empty_composition_ends_as_cancel() {
    let mut model = Model::new(&document("x", (1, 1)));
    model.start_composition(None);
    model.end_composition();
    assert_eq!(
        drain(&mut model),
        vec![
            ImeEvent::Composition(Composition::Start),
            ImeEvent::Composition(Composition::Cancel)
        ]
    );
}

#[test]
fn emptying_a_composition_over_a_selection_deletes_the_selected_text() {
    let mut model = Model::new(&document("hello world", (5, 11)));
    model.start_composition(Some(5..11));
    model.update_composition(5..5);
    model.replace(5, 11, &[]).unwrap();
    model.end_composition();
    let events = drain(&mut model);
    assert_eq!(
        &events[events.len() - 2..],
        [
            ImeEvent::Composition(Composition::Cancel),
            ImeEvent::Replace {
                range: Utf16Range::new(5, 11),
                text: String::new()
            }
        ]
    );
}

#[test]
fn discard_removes_composition_text_and_reports_the_change() {
    let mut model = Model::new(&document("ab", (2, 2)));
    model.start_composition(Some(2..2));
    model.insert_at_selection(&units("xyz")).unwrap();
    model.update_composition(2..5);
    let change = model.discard_composition().unwrap();
    assert_eq!((change.start, change.old_end, change.new_end), (2, 5, 2));
    assert_eq!(model.total_len(), 2);
    assert_eq!(model.selection().start, 2);
}

#[test]
fn app_updates_are_diffed_and_ignored_while_composing() {
    let mut model = Model::new(&document("hello world", (0, 0)));
    let notify = model.apply_document(&document("hello brave world", (12, 12)));
    let change = notify.text.unwrap();
    assert_eq!((change.start, change.old_end, change.new_end), (6, 6, 12));
    assert!(notify.selection);

    let unchanged = model.apply_document(&document("hello brave world", (12, 12)));
    assert!(unchanged.is_empty());

    model.start_composition(Some(12..12));
    let during = model.apply_document(&document("something else", (0, 0)));
    assert_eq!(during.text, None);
    assert_eq!(model.total_len(), 17);
}

#[test]
fn windowed_documents_report_total_length_and_reject_edits_outside_window() {
    let mut text = document("middle", (104, 104));
    text.window_start = 100;
    text.total_len = 5000;
    let mut model = Model::new(&text);
    assert_eq!(model.total_len(), 5000);
    assert!(model.replace(0, 1, &units("x")).is_err());
    let (slice, start) = model.get_text(90, None).unwrap();
    assert_eq!(
        (String::from_utf16_lossy(slice), start),
        ("middle".into(), 100)
    );
    model.replace(100, 101, &units("M")).unwrap();
    assert_eq!(model.total_len(), 5000);
}

#[test]
fn composition_wrapped_around_a_just_made_insertion_does_not_commit_twice() {
    let mut model = Model::new(&document("hello world", (5, 5)));
    model.insert_at_selection(&units("!")).unwrap();
    model.start_composition(Some(5..6));
    model.end_composition();
    assert_eq!(
        drain(&mut model),
        vec![
            ImeEvent::Composition(Composition::Start),
            ImeEvent::Composition(Composition::Update {
                marked: "!".into(),
                cursor: Some((1, 1))
            }),
            ImeEvent::Composition(Composition::Commit { text: "!".into() }),
        ]
    );
}

#[test]
fn composition_wrapped_around_a_remote_replacement_selects_the_replaced_range_first() {
    let mut model = Model::new(&document("hello world", (11, 11)));
    model.replace(0, 5, &units("HELLO")).unwrap();
    model.start_composition(Some(0..5));
    model.end_composition();
    let events = drain(&mut model);
    assert_eq!(
        events[0],
        ImeEvent::Select {
            range: Utf16Range::new(0, 5),
            reversed: false
        }
    );
    assert_eq!(events[1], ImeEvent::Composition(Composition::Start));
    assert_eq!(
        events.last(),
        Some(&ImeEvent::Composition(Composition::Commit {
            text: "HELLO".into()
        }))
    );
    assert_eq!(events.len(), 4);
}
