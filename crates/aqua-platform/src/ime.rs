use std::ops::Range;

pub const COMPOSITION_STRING: u32 = 0x0008;
pub const CURSOR_POSITION: u32 = 0x0080;
pub const RESULT_STRING: u32 = 0x0800;

pub trait CompositionSource {
    fn result(&mut self) -> Option<Vec<u16>>;
    fn composition(&mut self) -> Option<Vec<u16>>;
    fn cursor(&mut self) -> usize;
    fn attributes(&mut self) -> Option<Vec<u8>>;
}

pub trait CompositionTarget {
    fn replace(&mut self, text: &str, selected_utf16: Option<Range<usize>>);
}

pub fn handle_composition(
    flags: u32,
    source: &mut impl CompositionSource,
    target: &mut impl CompositionTarget,
) -> Option<isize> {
    if flags == 0 {
        target.replace("", None);
        return Some(0);
    }
    if flags & RESULT_STRING != 0 {
        let result = source.result()?;
        target.replace(&String::from_utf16_lossy(&result), None);
    }
    if flags & COMPOSITION_STRING != 0 {
        let composition = source.composition()?;
        let selected = if !composition.is_empty() && flags & CURSOR_POSITION != 0 {
            let cursor = source.cursor();
            let attributes = source.attributes().unwrap_or_default();
            let adjacent_input = attributes.get(cursor) == Some(&0)
                || cursor
                    .checked_sub(1)
                    .and_then(|index| attributes.get(index))
                    == Some(&0);
            let position = if adjacent_input {
                cursor
            } else {
                composition.len()
            };
            Some(position..position)
        } else {
            None
        };
        target.replace(&String::from_utf16_lossy(&composition), selected);
    }
    (flags & (RESULT_STRING | COMPOSITION_STRING) != 0).then_some(0)
}
