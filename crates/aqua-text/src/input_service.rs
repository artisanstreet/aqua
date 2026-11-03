use std::ops::Range;

use crate::Selection;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputChange {
    Unchanged,
    Changed,
    Rejected,
}

pub trait TextInputService {
    fn text(&self) -> &str;
    fn selection(&self) -> Selection;
    fn marked_range(&self) -> Option<Range<usize>>;
    fn text_for_utf16_range(&self, range: Range<usize>) -> Option<&str>;
    fn selected_utf16_range(&self, ignore_disabled: bool) -> Option<(Range<usize>, bool)>;
    fn marked_utf16_range(&self) -> Option<Range<usize>>;
    fn replace_utf16_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected_utf16: Option<Range<usize>>,
    ) -> InputChange;
    fn unmark_text(&mut self);
}
