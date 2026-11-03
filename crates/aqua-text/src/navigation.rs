use std::ops::Range;

use icu_segmenter::{options::WordBreakInvariantOptions, WordSegmenter};
use unicode_segmentation::UnicodeSegmentation as _;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WordBehavior {
    SkipSpaces,
    StopAtWordEnd,
}

impl WordBehavior {
    #[must_use]
    pub const fn platform() -> Self {
        Self::SkipSpaces
    }
}

const fn is_line_break(character: char) -> bool {
    matches!(character, '\n' | '\r')
}

const fn is_blink_whitespace(character: char) -> bool {
    matches!(character, '\u{00a0}' | ' ' | '\n' | '\t')
}

fn is_astral(character: char) -> bool {
    u32::from(character) > 0xffff
}

fn is_word_break(character: char) -> bool {
    (!character.is_control() && !is_blink_whitespace(character))
        || is_astral(character)
        || is_line_break(character)
        || character == '_'
}

fn is_word_boundary(character: char) -> bool {
    is_astral(character)
        || (!character.is_alphanumeric() && !character.is_whitespace() && !character.is_control())
}

fn char_at(text: &str, offset: usize) -> Option<char> {
    text.get(offset..)?.chars().next()
}

fn char_before(text: &str, offset: usize) -> Option<char> {
    text.get(..offset)?.chars().next_back()
}

fn splits_word(text: &str, offset: usize, character: char) -> bool {
    match character {
        ':' | '\u{fe55}' | '\u{ff1a}' => true,
        '.' | '\u{ff0e}' => {
            !(char_before(text, offset).is_some_and(char::is_numeric)
                && char_at(text, offset + character.len_utf8()).is_some_and(char::is_numeric))
        }
        _ => false,
    }
}

fn word_breaks(text: &str) -> Vec<usize> {
    let segmenter = WordSegmenter::new_dictionary(WordBreakInvariantOptions::default());
    let mut breaks: Vec<usize> = segmenter.segment_str(text).collect();
    for (offset, character) in text.char_indices() {
        if splits_word(text, offset, character) {
            breaks.push(offset);
            breaks.push(offset + character.len_utf8());
        }
    }
    breaks.sort_unstable();
    breaks.dedup();
    let mut graphemes: Vec<usize> = text
        .grapheme_indices(true)
        .map(|(offset, _)| offset)
        .collect();
    graphemes.push(text.len());
    breaks.retain(|offset| graphemes.binary_search(offset).is_ok());
    breaks
}

fn following(breaks: &[usize], offset: usize) -> Option<usize> {
    breaks
        .get(breaks.partition_point(|value| *value <= offset))
        .copied()
}

fn preceding(breaks: &[usize], offset: usize) -> Option<usize> {
    breaks
        .partition_point(|value| *value < offset)
        .checked_sub(1)
        .map(|index| breaks[index])
}

fn skip_whitespace(text: &str, offset: usize, behavior: WordBehavior) -> usize {
    if behavior != WordBehavior::SkipSpaces {
        return offset;
    }
    text[offset..]
        .char_indices()
        .find(|(_, character)| !character.is_whitespace() || is_line_break(*character))
        .map_or(text.len(), |(index, _)| offset + index)
}

pub struct WordBoundaries(Vec<usize>);
impl WordBoundaries {
    pub fn range_at(&self, text: &str, offset: usize) -> Range<usize> {
        word_range_with_breaks(text, offset, &self.0)
    }
    pub fn selection_range(
        &self,
        unit: SelectionUnit,
        text: &str,
        offset: usize,
        dragging: bool,
    ) -> Range<usize> {
        if unit != SelectionUnit::Word {
            return if dragging {
                unit.drag_range_at(text, offset)
            } else {
                unit.range_at(text, offset)
            };
        }
        let word = self.range_at(text, offset);
        if dragging || word.is_empty() {
            word
        } else {
            word.start..skip_whitespace(text, word.end, WordBehavior::platform())
        }
    }

    pub fn new(text: &str) -> Self {
        Self(word_breaks(text))
    }
    pub fn next(&self, text: &str, offset: usize, behavior: WordBehavior) -> usize {
        next_with_breaks(text, offset, behavior, &self.0)
    }
    pub fn previous(&self, text: &str, offset: usize) -> usize {
        previous_with_breaks(text, offset, &self.0)
    }
}

fn next_with_breaks(text: &str, offset: usize, behavior: WordBehavior, breaks: &[usize]) -> usize {
    let offset = floor_char_boundary(text, offset);
    if offset >= text.len() {
        return text.len();
    }
    let skips = behavior == WordBehavior::SkipSpaces;
    let mut punctuation: Option<usize> = None;
    let mut runner = following(breaks, offset);
    while let Some(at) = runner {
        let next = char_at(text, at);
        let previous = char_before(text, at);
        let pending = if skips { punctuation } else { None };
        if next.is_some_and(is_line_break) {
            return skip_whitespace(text, pending.unwrap_or(at), behavior);
        }
        if next.is_some_and(is_word_boundary) {
            if previous.is_some_and(char::is_alphanumeric) {
                return skip_whitespace(text, at, behavior);
            }
            if skips && punctuation.is_none() && previous.is_some_and(is_blink_whitespace) {
                punctuation = Some(at);
            }
            runner = following(breaks, at);
            continue;
        }
        if let Some(pending) = pending {
            return skip_whitespace(text, pending, behavior);
        }
        if next.is_some() {
            if previous.is_some_and(is_word_break) {
                return skip_whitespace(text, at, behavior);
            }
            if skips && previous.is_some_and(is_blink_whitespace) && next.is_some_and(is_word_break)
            {
                return skip_whitespace(text, at, behavior);
            }
        }
        runner = following(breaks, at);
    }
    text.len()
}

fn previous_with_breaks(text: &str, offset: usize, breaks: &[usize]) -> usize {
    let offset = floor_char_boundary(text, offset);
    if offset == 0 {
        return 0;
    }
    let mut punctuation: Option<usize> = None;
    let mut runner = preceding(breaks, offset);
    while let Some(at) = runner {
        let next = char_at(text, at);
        if next.is_some_and(is_word_boundary) {
            if char_before(text, at).is_some_and(char::is_alphanumeric) {
                return at;
            }
            punctuation = Some(at);
            runner = preceding(breaks, at);
            continue;
        }
        if let Some(punctuation) = punctuation {
            return punctuation;
        }
        if next.is_some_and(is_word_break) {
            return at;
        }
        runner = preceding(breaks, at);
    }
    0
}

#[must_use]
pub fn word_range_at(text: &str, offset: usize) -> Range<usize> {
    WordBoundaries::new(text).range_at(text, offset)
}

fn word_range_with_breaks(text: &str, offset: usize, breaks: &[usize]) -> Range<usize> {
    let offset = floor_char_boundary(text, offset);
    let at_line_end = char_at(text, offset).is_none_or(is_line_break);
    if at_line_end {
        let start = preceding(breaks, offset).unwrap_or(offset);
        if char_at(text, start).is_some_and(is_line_break) {
            return offset..offset;
        }
        return start..offset;
    }
    let start = breaks
        .partition_point(|value| *value <= offset)
        .checked_sub(1)
        .map_or(0, |index| breaks[index]);
    let end = following(breaks, offset).unwrap_or(text.len());
    start..end
}

#[must_use]
pub fn double_click_range(text: &str, offset: usize, behavior: WordBehavior) -> Range<usize> {
    let word = word_range_at(text, offset);
    if word.is_empty() {
        return word;
    }
    word.start..skip_whitespace(text, word.end, behavior)
}

#[must_use]
pub fn paragraph_range_at(text: &str, offset: usize) -> Range<usize> {
    let offset = floor_char_boundary(text, offset);
    let start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    let end = text[offset..]
        .find('\n')
        .map_or(text.len(), |index| offset + index + 1);
    start..end
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SelectionUnit {
    #[default]
    Character,
    Word,
    Paragraph,
}

impl SelectionUnit {
    #[must_use]
    pub const fn from_click_count(click_count: usize) -> Self {
        match click_count {
            0 | 1 => Self::Character,
            2 => Self::Word,
            _ => Self::Paragraph,
        }
    }

    #[must_use]
    pub fn range_at(self, text: &str, offset: usize) -> Range<usize> {
        match self {
            Self::Character => offset..offset,
            Self::Word => {
                let word = double_click_range(text, offset, WordBehavior::platform());
                if word.is_empty() {
                    offset..offset
                } else {
                    word
                }
            }
            Self::Paragraph => paragraph_range_at(text, offset),
        }
    }

    #[must_use]
    pub fn drag_range_at(self, text: &str, offset: usize) -> Range<usize> {
        match self {
            Self::Word => {
                let word = word_range_at(text, offset);
                if word.is_empty() {
                    offset..offset
                } else {
                    word
                }
            }
            Self::Character | Self::Paragraph => self.range_at(text, offset),
        }
    }
}

#[must_use]
pub fn floor_char_boundary(text: &str, offset: usize) -> usize {
    text.floor_char_boundary(offset.min(text.len()))
}
