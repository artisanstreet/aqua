pub mod document;
pub mod editor;
pub mod font;
pub mod layout;
pub mod model;
pub mod navigation;
pub mod runs;
pub mod selection;
pub mod style;

pub use editor::TextEditor;
pub use font::{FontManager, SharedFonts};
pub use layout::{
    measure_text, CaretAffinity, CaretPosition, CaretRect, LaidGlyph, LaidLine, LaidOut, Rect,
    SharedLayoutCache, TextLayout,
};
pub use model::TextModel;
pub use runs::{ResolveError, RunHandle, RunId, RunRegistry, SharedRuns};
pub use selection::Selection;
pub use style::{
    FontFamily, FontSlant, FontStretch, FontWeight, LineMode, StyledSpan, TextAlign, TextStyle,
    WrapMode,
};

pub const COMPAT_COSMIC_TEXT: &str = "0.19";
pub const COMPAT_GLYPHON: &str = "0.12";
pub const COMPAT_WGPU: &str = "30";

pub const AQUA_TEXT_API_VERSION: u32 = 1;
