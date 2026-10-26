//! Selection: an anchor and a caret as byte indices.
//!
//! The caret is where typed text appears; the anchor is the fixed end while
//! extending. When both are equal the selection is a plain caret.

/// A selection (or caret) in a text model, as byte indices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Selection {
    /// Fixed end while shift-extending.
    pub anchor: usize,
    /// Moving end; typed text is inserted here.
    pub caret: usize,
}

impl Selection {
    pub fn caret_at(pos: usize) -> Self {
        Self {
            anchor: pos,
            caret: pos,
        }
    }

    pub fn new(anchor: usize, caret: usize) -> Self {
        Self { anchor, caret }
    }

    /// True when this is a bare caret with no selected range.
    pub fn is_caret(&self) -> bool {
        self.anchor == self.caret
    }

    /// The selected byte range in ascending order (empty for a caret).
    pub fn range(&self) -> std::ops::Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }

    /// Collapse to a caret at the current caret end.
    pub fn collapse(&mut self) {
        self.anchor = self.caret;
    }

    /// Collapse to a caret at `pos`.
    pub fn collapse_to(&mut self, pos: usize) {
        self.anchor = pos;
        self.caret = pos;
    }

    /// Move the caret, collapsing any selection.
    pub fn move_to(&mut self, pos: usize) {
        self.collapse_to(pos);
    }

    /// Move the caret, keeping the anchor (shift-extend).
    pub fn extend_to(&mut self, pos: usize) {
        self.caret = pos;
    }

    /// Clamp both ends into `0..=len`.
    pub fn clamp(&mut self, len: usize) {
        self.anchor = self.anchor.min(len);
        self.caret = self.caret.min(len);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_orders_ends() {
        let s = Selection::new(8, 3);
        assert_eq!(s.range(), 3..8);
        assert!(!s.is_caret());
    }
}
