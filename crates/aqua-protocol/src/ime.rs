use aqua_core::{ImeEvent as Composition, Rect};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Utf16Range {
    pub start: u32,
    pub end: u32,
}

impl Utf16Range {
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    pub const fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    pub const fn is_empty(self) -> bool {
        self.end <= self.start
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImeDocument {
    pub window_start: u32,
    pub total_len: u32,
    pub text: String,
    pub selection: Utf16Range,
    pub selection_reversed: bool,
    pub area: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ImeCommand {
    Enable(ImeDocument),
    Update(ImeDocument),
    Disable,
    Reconvert,
    CommitComposition,
    CancelComposition,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ImeEvent {
    Composition(Composition),
    Replace { range: Utf16Range, text: String },
    Select { range: Utf16Range, reversed: bool },
}

pub trait ImeBackend {
    type Error: std::error::Error;

    fn command(&mut self, command: ImeCommand) -> Result<(), Self::Error>;
    fn drain(&mut self, out: &mut Vec<ImeEvent>);
}
