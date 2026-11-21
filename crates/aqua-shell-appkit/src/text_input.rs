#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Utf16Range {
    pub start: usize,
    pub len: usize,
}

impl Utf16Range {
    pub fn end(self) -> usize {
        self.start + self.len
    }
}

pub trait TextInputClient {
    fn marked_range(&self) -> Option<Utf16Range>;
    fn selected_range(&self) -> Option<Utf16Range>;
    fn set_marked_text(
        &mut self,
        text: &str,
        selected: Utf16Range,
        replacement: Option<Utf16Range>,
    );
    fn unmark_text(&mut self);
    fn insert_text(&mut self, text: &str, replacement: Option<Utf16Range>);
    fn attributed_substring(&self, range: Utf16Range) -> Option<(String, Utf16Range)>;
    fn first_rect(&self, range: Utf16Range) -> Option<((f64, f64, f64, f64), Utf16Range)>;
    fn character_index(&self, screen_point: (f64, f64)) -> Option<usize>;
}
