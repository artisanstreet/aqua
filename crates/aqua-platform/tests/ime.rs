use std::ops::Range;

use aqua_platform::ime::{
    handle_composition, CompositionSource, CompositionTarget, COMPOSITION_STRING, CURSOR_POSITION,
    RESULT_STRING,
};

struct Source {
    result: Option<Vec<u16>>,
    composition: Option<Vec<u16>>,
    cursor: usize,
    attributes: Option<Vec<u8>>,
}

impl CompositionSource for Source {
    fn result(&mut self) -> Option<Vec<u16>> {
        self.result.clone()
    }
    fn composition(&mut self) -> Option<Vec<u16>> {
        self.composition.clone()
    }
    fn cursor(&mut self) -> usize {
        self.cursor
    }
    fn attributes(&mut self) -> Option<Vec<u8>> {
        self.attributes.clone()
    }
}

#[derive(Default)]
struct Target(Vec<(String, Option<Range<usize>>)>);

impl CompositionTarget for Target {
    fn replace(&mut self, text: &str, selected: Option<Range<usize>>) {
        self.0.push((text.to_owned(), selected));
    }
}

#[test]
fn result_precedes_composition_and_cursor_uses_utf16_attributes() {
    let mut source = Source {
        result: Some("done".encode_utf16().collect()),
        composition: Some("😀x".encode_utf16().collect()),
        cursor: 2,
        attributes: Some(vec![1, 0, 1]),
    };
    let mut target = Target::default();
    assert_eq!(
        handle_composition(
            RESULT_STRING | COMPOSITION_STRING | CURSOR_POSITION,
            &mut source,
            &mut target
        ),
        Some(0)
    );
    assert_eq!(
        target.0,
        vec![("done".into(), None), ("😀x".into(), Some(2..2))]
    );
    source.attributes = Some(vec![1, 1, 1]);
    let mut target = Target::default();
    handle_composition(
        COMPOSITION_STRING | CURSOR_POSITION,
        &mut source,
        &mut target,
    );
    assert_eq!(target.0, vec![("😀x".into(), Some(3..3))]);
}

#[test]
fn empty_or_missing_cursor_preserves_editor_unmark_semantics() {
    let mut source = Source {
        result: None,
        composition: Some(Vec::new()),
        cursor: 0,
        attributes: None,
    };
    let mut target = Target::default();
    assert_eq!(
        handle_composition(
            COMPOSITION_STRING | CURSOR_POSITION,
            &mut source,
            &mut target
        ),
        Some(0)
    );
    assert_eq!(handle_composition(0, &mut source, &mut target), Some(0));
    assert_eq!(target.0, vec![(String::new(), None), (String::new(), None)]);
    assert_eq!(handle_composition(0x10, &mut source, &mut target), None);
}

#[test]
fn missing_composition_retains_preceding_result_then_falls_through() {
    let mut source = Source {
        result: Some(vec![65]),
        composition: None,
        cursor: 0,
        attributes: None,
    };
    let mut target = Target::default();
    assert_eq!(
        handle_composition(RESULT_STRING | COMPOSITION_STRING, &mut source, &mut target),
        None
    );
    assert_eq!(target.0, vec![("A".into(), None)]);
}
