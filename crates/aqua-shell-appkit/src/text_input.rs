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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Insertion {
    Typed,
    Taken,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub trait TextInputClient {
    fn key_pressed(&mut self);
    fn deactivated(&mut self);
    fn marked_range(&self) -> Option<Utf16Range>;
    fn selected_range(&self) -> Option<Utf16Range>;
    fn set_marked_text(
        &mut self,
        text: &str,
        selected: Utf16Range,
        replacement: Option<Utf16Range>,
    );
    fn unmark_text(&mut self);
    fn insert_text(&mut self, text: &str, replacement: Option<Utf16Range>) -> Insertion;
    fn attributed_substring(&self, range: Utf16Range) -> Option<(String, Utf16Range)>;
    fn first_rect(&self, range: Utf16Range) -> Option<(ScreenRect, Utf16Range)>;
    fn character_index(&self, screen_point: (f64, f64)) -> Option<usize>;
}
