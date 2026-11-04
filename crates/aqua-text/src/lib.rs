#![forbid(unsafe_code)]
pub mod document;
pub mod editor;
pub mod fallback;
pub mod font;
pub mod input_service;
pub mod layout;
mod lines;
pub mod model;
pub mod navigation;
pub mod overflow;
pub mod raster;
pub mod residency;
pub mod runs;
pub mod selection;
mod shape;
pub mod slug;
pub mod strike;
pub mod style;

pub use aqua_graph::tone;
pub use editor::TextEditor;
pub use font::{FontEntry, FontId, FontManager, PrewarmJob, PrewarmedEntry, SharedFonts};
pub use layout::{
    measure_text, CaretAffinity, CaretPosition, CaretRect, DecorationKind, LaidGlyph, LaidLine,
    LaidOut, Rect, SharedLayoutCache, TextBackground, TextDecoration, TextInteractionRange,
    TextLayout,
};
pub use model::TextModel;
pub use runs::{ResolveError, RunHandle, RunId, RunRegistry, SharedRuns};
pub use selection::Selection;
pub use style::{
    FontFamily, FontSlant, FontStretch, FontWeight, InlineCellError, LineMode, StyledSpan,
    TextAlign, TextStyle, WrapMode,
};

pub const AQUA_TEXT_API_VERSION: u32 = 1;
