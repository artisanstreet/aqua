use aqua_text::navigation::{
    double_click_range, paragraph_range_at, word_range_at, SelectionUnit, WordBehavior,
    WordBoundaries,
};

fn run(marked: &str, step: impl Fn(&str, usize) -> usize) -> String {
    let offset = marked.find('|').expect("caret marker");
    let text = marked.replacen('|', "", 1);
    let result = step(&text, offset);
    format!("{}|{}", &text[..result], &text[result..])
}

fn next(marked: &str) -> String {
    run(marked, |text, offset| {
        WordBoundaries::new(text).next(text, offset, WordBehavior::StopAtWordEnd)
    })
}

fn next_skipping(marked: &str) -> String {
    run(marked, |text, offset| {
        WordBoundaries::new(text).next(text, offset, WordBehavior::SkipSpaces)
    })
}

fn previous(marked: &str) -> String {
    run(marked, |text, offset| {
        WordBoundaries::new(text).previous(text, offset)
    })
}

#[test]
fn next_word_matches_blink_basic() {
    assert_eq!(next("|(1) abc def"), "(|1) abc def");
    assert_eq!(next("(|1) abc def"), "(1|) abc def");
    assert_eq!(next("(1|) abc def"), "(1)| abc def");
    assert_eq!(next("(1)| abc def"), "(1) abc| def");
    assert_eq!(next("(1) a|bc def"), "(1) abc| def");
    assert_eq!(next("(1) abc| def"), "(1) abc def|");
    assert_eq!(next("(1) abc def|"), "(1) abc def|");
}

#[test]
fn next_word_skipping_spaces_matches_blink_windows() {
    assert_eq!(next_skipping("|(1) abc def"), "(|1) abc def");
    assert_eq!(next_skipping("(|1) abc def"), "(1|) abc def");
    assert_eq!(next_skipping("(1|) abc def"), "(1) |abc def");
    assert_eq!(next_skipping("(1)| abc def"), "(1) |abc def");
    assert_eq!(next_skipping("(1) |abc def"), "(1) abc |def");
    assert_eq!(next_skipping("(1) abc| def"), "(1) abc |def");
    assert_eq!(next_skipping("(1) abc |def"), "(1) abc def|");
}

#[test]
fn next_word_matches_blink_punctuation() {
    assert_eq!(next("|abc.def"), "abc|.def");
    assert_eq!(next("abc|.def"), "abc.|def");
    assert_eq!(next("abc.|def"), "abc.def|");
    assert_eq!(next("|abc...def"), "abc|...def");
    assert_eq!(next("abc.|..def"), "abc...|def");
    assert_eq!(next("abc...|def"), "abc...def|");
    assert_eq!(next("|abc ((())) def"), "abc| ((())) def");
    assert_eq!(next("abc |((())) def"), "abc ((()))| def");
    assert_eq!(next("|abc 32.3 def"), "abc| 32.3 def");
    assert_eq!(next("abc |32.3 def"), "abc 32.3| def");
}

#[test]
fn next_word_matches_blink_emoji() {
    assert_eq!(next("|abc \u{1F602} def"), "abc| \u{1F602} def");
    assert_eq!(next("abc |\u{1F602} def"), "abc \u{1F602}| def");
    assert_eq!(next_skipping("|abc \u{1F602} def"), "abc |\u{1F602} def");
    assert_eq!(next_skipping("abc |\u{1F602} def"), "abc \u{1F602} |def");
}

#[test]
fn next_word_stops_at_line_breaks() {
    assert_eq!(next("|abc\ndef"), "abc|\ndef");
    assert_eq!(next("abc|\ndef"), "abc\n|def");
    assert_eq!(next_skipping("abc |  \ndef"), "abc   |\ndef");
}

#[test]
fn previous_word_matches_blink() {
    assert_eq!(previous("(1) abc def|"), "(1) abc |def");
    assert_eq!(previous("(1) abc |def"), "(1) |abc def");
    assert_eq!(previous("(1) a|bc def"), "(1) |abc def");
    assert_eq!(previous("(1) |abc def"), "(1|) abc def");
    assert_eq!(previous("(1|) abc def"), "(|1) abc def");
    assert_eq!(previous("(|1) abc def"), "|(1) abc def");
    assert_eq!(previous("abc.def|"), "abc.|def");
    assert_eq!(previous("abc.|def"), "abc|.def");
    assert_eq!(previous("abc ((()))| def"), "abc |((())) def");
    assert_eq!(previous("abc\n|def"), "abc|\ndef");
}

#[test]
fn identifiers_and_contractions_are_one_word() {
    assert_eq!(next("|snake_case rest"), "snake_case| rest");
    assert_eq!(next("|can't stop"), "can't| stop");
    assert_eq!(previous("snake_case|"), "|snake_case");
}

#[test]
fn double_click_selects_the_word_under_the_pointer() {
    let text = "Two things that aren't my builds:";
    assert_eq!(&text[word_range_at(text, 5)], "things");
    assert_eq!(&text[word_range_at(text, 18)], "aren't");
    assert_eq!(&text[word_range_at(text, text.len())], ":");
    let windows = double_click_range(text, 5, WordBehavior::SkipSpaces);
    assert_eq!(&text[windows], "things ");
    let linux = double_click_range(text, 5, WordBehavior::StopAtWordEnd);
    assert_eq!(&text[linux], "things");
    let path = "see main.rs:12";
    assert_eq!(&path[word_range_at(path, 5)], "main");
}

#[test]
fn double_click_never_selects_a_line_break() {
    let text = "abc\n\ndef";
    assert_eq!(word_range_at(text, 4), 4..4);
    assert_eq!(&text[word_range_at(text, 3)], "abc");
    let trailing = double_click_range("abc  \nx", 0, WordBehavior::SkipSpaces);
    assert_eq!(trailing, 0..5);
}

#[test]
fn triple_click_selects_the_line_with_its_break() {
    let text = "one\ntwo three\nfour";
    assert_eq!(&text[paragraph_range_at(text, 6)], "two three\n");
    assert_eq!(&text[paragraph_range_at(text, 16)], "four");
}

#[test]
fn click_counts_map_to_units() {
    assert_eq!(SelectionUnit::from_click_count(1), SelectionUnit::Character);
    assert_eq!(SelectionUnit::from_click_count(2), SelectionUnit::Word);
    assert_eq!(SelectionUnit::from_click_count(3), SelectionUnit::Paragraph);
    assert_eq!(SelectionUnit::from_click_count(4), SelectionUnit::Paragraph);
}

#[test]
fn unicode_words_keep_script_boundaries_and_combining_sequences() {
    for (text, first, second) in [
        ("привет мир", "привет ".len(), "привет мир".len()),
        ("مرحبا عالم", "مرحبا ".len(), "مرحبا عالم".len()),
        ("नमस्ते दुनिया", "नमस्ते ".len(), "नमस्ते दुनिया".len()),
        (
            "cafe\u{301} next",
            "cafe\u{301} ".len(),
            "cafe\u{301} next".len(),
        ),
    ] {
        let words = WordBoundaries::new(text);
        assert_eq!(words.next(text, 0, WordBehavior::SkipSpaces), first);
        assert_eq!(words.next(text, first, WordBehavior::SkipSpaces), second);
        assert_eq!(words.previous(text, second), first);
        assert_eq!(words.previous(text, first), 0);
    }
}

#[test]
fn word_drag_omits_initial_windows_trailing_spaces() {
    let text = "one   two...three\nfour";
    let unit = SelectionUnit::Word;
    assert_eq!(unit.range_at(text, 1), 0..6);
    assert_eq!(unit.drag_range_at(text, 7), 6..9);
    assert_eq!(unit.drag_range_at(text, 13), 12..17);
    assert_eq!(unit.drag_range_at(text, 10), 10..11);
    assert_eq!(unit.range_at(text, 17), 12..17);
}

#[test]
fn dictionary_word_units_cover_thai_japanese_and_chinese() {
    for words in [
        vec!["ทุก", "สอง", "สัปดาห์"],
        vec!["こんにちは", "世界"],
        vec!["你好", "世界"],
    ] {
        let text = words.concat();
        let boundaries = WordBoundaries::new(&text);
        let mut offset = 0;
        for word in words {
            let end = offset + word.len();
            assert_eq!(
                boundaries.next(&text, offset, WordBehavior::SkipSpaces),
                end
            );
            assert_eq!(boundaries.previous(&text, end), offset);
            assert_eq!(word_range_at(&text, offset), offset..end);
            assert_eq!(
                SelectionUnit::Word.drag_range_at(&text, offset),
                offset..end
            );
            offset = end;
        }
    }
}

#[test]
fn mixed_dictionary_scripts_keep_windows_spaces_and_chromium_punctuation() {
    let text = "ทุกสองสัปดาห์  こんにちは世界.next";
    let boundaries = WordBoundaries::new(text);
    let thai_end = "ทุกสองสัปดาห์".len();
    let japanese_start = thai_end + 2;
    let greeting_end = japanese_start + "こんにちは".len();
    let world_end = greeting_end + "世界".len();
    assert_eq!(
        boundaries.next(text, "ทุกสอง".len(), WordBehavior::SkipSpaces),
        japanese_start
    );
    assert_eq!(
        double_click_range(text, "ทุกสอง".len(), WordBehavior::SkipSpaces),
        "ทุกสอง".len()..japanese_start
    );
    assert_eq!(
        boundaries.next(text, japanese_start, WordBehavior::SkipSpaces),
        greeting_end
    );
    assert_eq!(
        boundaries.next(text, greeting_end, WordBehavior::SkipSpaces),
        world_end
    );
    assert_eq!(
        boundaries.next(text, world_end, WordBehavior::SkipSpaces),
        world_end + 1
    );
    assert_eq!(boundaries.previous(text, text.len()), world_end + 1);
}
