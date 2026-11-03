use unicode_segmentation::UnicodeSegmentation;

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

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.revision += 1;
    }

    pub fn insert(&mut self, at: usize, s: &str) -> std::ops::Range<usize> {
        let at = self.snap_insert_pos(at);
        self.text.insert_str(at, s);
        self.revision += 1;
        at..at + s.len()
    }

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

    pub fn prev_grapheme(&self, at: usize) -> usize {
        let at = at.min(self.text.len());
        self.text
            .grapheme_indices(true)
            .filter(|&(i, _)| i < at)
            .map(|(i, _)| i)
            .next_back()
            .unwrap_or(0)
    }

    pub fn next_grapheme(&self, at: usize) -> usize {
        let at = at.min(self.text.len());
        self.text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .find(|&i| i > at)
            .unwrap_or(self.text.len())
    }

    pub fn is_grapheme_boundary(&self, at: usize) -> bool {
        if at == 0 || at == self.text.len() {
            return at <= self.text.len();
        }
        self.text.grapheme_indices(true).any(|(i, _)| i == at)
    }

    pub fn prev_word(&self, at: usize) -> usize {
        let target = crate::navigation::WordBoundaries::new(&self.text).previous(&self.text, at);
        self.prev_boundary_or_self(target)
    }

    pub fn next_word(&self, at: usize) -> usize {
        let target = crate::navigation::WordBoundaries::new(&self.text).next(
            &self.text,
            at,
            crate::navigation::WordBehavior::platform(),
        );
        self.next_boundary_or_self(target)
    }

    pub(crate) fn replace_exact(&mut self, range: std::ops::Range<usize>, text: &str) {
        self.text.replace_range(range, text);
        self.revision += 1;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combining_mark_is_one_grapheme() {
        let m = TextModel::new("é");
        assert_eq!(m.text().len(), 3);
        assert_eq!(m.grapheme_count(), 1);
        assert_eq!(m.next_grapheme(0), 3);
        assert_eq!(m.prev_grapheme(3), 0);
    }

    #[test]
    fn emoji_zwj_sequence_is_one_grapheme() {
        let family = "👨‍👩‍👧";
        let m = TextModel::new(family);
        assert!(m.text().len() > 4);
        assert_eq!(m.grapheme_count(), 1);
        assert_eq!(m.next_grapheme(0), m.len_bytes());
    }

    #[test]
    fn delete_snaps_to_cluster() {
        let mut m = TextModel::new("aéb");
        let removed = m.delete(2..2 + 1);
        assert_eq!(removed, "é");
        assert_eq!(m.text(), "ab");
    }

    #[test]
    fn word_movement() {
        let m = TextModel::new("hello world  foo");
        assert_eq!(m.next_word(0), 6);
        assert_eq!(m.next_word(5), 6);
        assert_eq!(m.next_word(6), 13);
        assert_eq!(m.prev_word(11), 6);
        assert_eq!(m.prev_word(6), 0);
        assert_eq!(m.next_word(m.len_bytes()), m.len_bytes());
    }
}
