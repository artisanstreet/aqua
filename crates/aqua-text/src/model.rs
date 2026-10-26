//! The editable text model: a `String` plus grapheme/word-aware operations.
//!
//! All positions in this module are **byte indices** into the model's string
//! and are always kept on grapheme-cluster boundaries by the editing
//! operations. Shaping, bidi and font fallback live in [`crate::layout`];
//! this module only uses `unicode-segmentation` for boundary tables, never
//! reimplementing Unicode algorithms by hand.

use unicode_segmentation::UnicodeSegmentation;

/// Owned text content with a revision counter for cache invalidation.
///
/// The revision bumps on every mutation, even no-op-shaped ones, so layout
/// caches can use a cheap integer validity check.
#[derive(Clone, Debug, Default)]
pub struct TextModel {
    text: String,
    revision: u64,
}

impl TextModel {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            revision: 0,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn len_bytes(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn grapheme_count(&self) -> usize {
        self.text.graphemes(true).count()
    }

    /// Replace the whole content. Always bumps the revision.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.revision += 1;
    }

    /// Insert `s` at byte index `at`. `at` is clamped and snapped back to
    /// the previous grapheme boundary. Returns the byte range of the
    /// inserted text in the new string.
    pub fn insert(&mut self, at: usize, s: &str) -> std::ops::Range<usize> {
        let at = self.snap_insert_pos(at);
        self.text.insert_str(at, s);
        self.revision += 1;
        at..at + s.len()
    }

    /// Delete `range` (bytes). Endpoints are clamped and snapped outward to
    /// grapheme boundaries so a partial cluster is never left behind.
    /// Returns the deleted text.
    pub fn delete(&mut self, range: std::ops::Range<usize>) -> String {
        let start = range.start.min(self.text.len());
        let end = range.end.min(self.text.len()).max(start);
        let start = self.prev_boundary_or_self(start);
        let end = self.next_boundary_or_self(end);
        let removed = self.text[start..end].to_string();
        self.text.replace_range(start..end, "");
        self.revision += 1;
        removed
    }

    /// Byte index of the previous grapheme-cluster boundary at or before
    /// `at` (i.e. the boundary you get by pressing Left once from `at` if
    /// `at` is itself a boundary, else the boundary starting that cluster).
    pub fn prev_grapheme(&self, at: usize) -> usize {
        let at = at.min(self.text.len());
        // Greatest boundary strictly below `at`: the "move Left" target
        // both when `at` is a boundary and when it is mid-cluster.
        self.text
            .grapheme_indices(true)
            .filter(|&(i, _)| i < at)
            .map(|(i, _)| i)
            .next_back()
            .unwrap_or(0)
    }

    /// Byte index of the next grapheme-cluster boundary at or after `at`
    /// (the "move Right once" target).
    pub fn next_grapheme(&self, at: usize) -> usize {
        let at = at.min(self.text.len());
        self.text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .find(|&i| i > at)
            .unwrap_or(self.text.len())
    }

    /// True if `at` is a grapheme-cluster boundary (including both ends).
    pub fn is_grapheme_boundary(&self, at: usize) -> bool {
        if at == 0 || at == self.text.len() {
            return at <= self.text.len();
        }
        self.text.grapheme_indices(true).any(|(i, _)| i == at)
    }

    /// Start byte index of the word containing (or preceding) `at`,
    /// for Ctrl+Left style movement. Words follow UAX #29 via
    /// `unicode-segmentation`; whitespace/punctuation runs are skipped so
    /// repeated presses keep moving.
    pub fn prev_word(&self, at: usize) -> usize {
        let at = at.min(self.text.len());
        // Greatest word-start strictly before `at` whose word is not
        // whitespace/punctuation; if none, the greatest word-start before
        // `at` (so repeated presses keep moving through skip runs).
        let mut best_word = 0;
        let mut best_any = 0;
        let mut found_word = false;
        for (i, word) in self.text.split_word_bound_indices() {
            if i >= at {
                break;
            }
            best_any = i;
            if !is_word_skip(word) {
                best_word = i;
                found_word = true;
            }
        }
        if found_word {
            best_word
        } else {
            best_any.min(at)
        }
    }

    /// End byte index of the word containing (or following) `at`,
    /// for Ctrl+Right style movement.
    pub fn next_word(&self, at: usize) -> usize {
        let at = at.min(self.text.len());
        let len = self.text.len();
        let mut iter = self.text.split_word_bound_indices().peekable();
        while let Some((i, word)) = iter.next() {
            let end = i + word.len();
            if end <= at {
                continue;
            }
            if is_word_skip(word) {
                // Skip whitespace/punctuation, land at end of next real word.
                if let Some((j, w2)) = iter.next() {
                    if is_word_skip(w2) {
                        return (j + w2.len()).min(len);
                    }
                    return (j + w2.len()).min(len);
                }
                return len;
            }
            // Inside or at start of a word: land at its end.
            if i < at {
                return end;
            }
            return end;
        }
        len
    }

    fn snap_insert_pos(&self, at: usize) -> usize {
        self.prev_boundary_or_self(at.min(self.text.len()))
    }

    fn prev_boundary_or_self(&self, at: usize) -> usize {
        if self.is_grapheme_boundary(at) {
            return at;
        }
        self.prev_grapheme(at)
    }

    fn next_boundary_or_self(&self, at: usize) -> usize {
        if at >= self.text.len() || self.is_grapheme_boundary(at) {
            return at;
        }
        self.next_grapheme(at)
    }
}

fn is_word_skip(word: &str) -> bool {
    word.chars().all(|c| c.is_whitespace() || is_word_punct(c))
}

fn is_word_punct(c: char) -> bool {
    matches!(
        c,
        '.' | ','
            | ';'
            | ':'
            | '!'
            | '?'
            | '"'
            | '\''
            | '('
            | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | '/'
            | '\\'
            | '-'
            | '_'
            | '@'
            | '#'
            | '$'
            | '%'
            | '^'
            | '&'
            | '*'
            | '+'
            | '='
            | '<'
            | '>'
            | '|'
            | '~'
            | '`'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combining_mark_is_one_grapheme() {
        let m = TextModel::new("é"); // e + U+0301
        assert_eq!(m.text().len(), 3);
        assert_eq!(m.grapheme_count(), 1);
        assert_eq!(m.next_grapheme(0), 3);
        assert_eq!(m.prev_grapheme(3), 0);
    }

    #[test]
    fn emoji_zwj_sequence_is_one_grapheme() {
        let family = "👨‍👩‍👧"; // ZWJ sequence, several code points
        let m = TextModel::new(family);
        assert!(m.text().len() > 4);
        assert_eq!(m.grapheme_count(), 1);
        assert_eq!(m.next_grapheme(0), m.len_bytes());
    }

    #[test]
    fn delete_snaps_to_cluster() {
        let mut m = TextModel::new("aéb");
        // Byte 2 is inside "e + combining mark" (bytes 1..4).
        let removed = m.delete(2..2 + 1);
        assert_eq!(removed, "é");
        assert_eq!(m.text(), "ab");
    }

    #[test]
    fn word_movement() {
        let m = TextModel::new("hello world  foo");
        assert_eq!(m.next_word(0), 5);
        assert_eq!(m.next_word(5), 11);
        assert_eq!(m.prev_word(11), 6);
        assert_eq!(m.prev_word(6), 0);
        assert_eq!(m.next_word(m.len_bytes()), m.len_bytes());
    }
}
