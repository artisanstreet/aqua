//! `aqua-text`: shared text infrastructure for the Aqua GUI framework.
//!
//! Cross-platform font discovery, shaping-aware measurement, shaped-run
//! extraction, and the editing model (grapheme-safe movement/deletion,
//! selection, single/multi-line policy, IME preedit, bounded undo/redo).
//! No window, GPU or platform dependency — everything here runs headless,
//! which is what makes it unit-testable on any host.
//!
//! # Version coordination
//!
//! The renderer (`aqua-render`) shapes and rasterizes with
//! `cosmic-text 0.19` through `glyphon 0.12` on `wgpu 30`. This crate uses
//! the same `cosmic-text 0.19`, and glyphon's `TextRenderer` borrows the
//! caller's `FontSystem`, so one [`FontManager`] shared via [`SharedFonts`]
//! keeps measurement and pixels on identical shaping. See
//! [`TextLayout::buffer`]. The pinned versions are also asserted here:
//!
//! ```
//! assert_eq!(aqua_text::COMPAT_COSMIC_TEXT, "0.19");
//! assert_eq!(aqua_text::COMPAT_GLYPHON, "0.12");
//! assert_eq!(aqua_text::COMPAT_WGPU, "30");
//! ```
//!
//! # Crate map
//!
//! * [`FontManager`]/[`SharedFonts`] — font database ownership and sharing.
//! * [`TextStyle`] family — renderer-neutral styling + rich [`StyledSpan`]s.
//! * [`TextModel`] — string + grapheme/word operations.
//! * [`Selection`] — anchor/caret.
//! * [`TextLayout`]/[`LaidOut`] — measurement + shaped runs + caret/hit.
//! * [`RunRegistry`]/[`RunId`] — `TextRun` mint side (contract §4).
//! * [`SharedLayoutCache`] — bounded cross-widget layout reuse.
//! * [`TextEditor`] — the editing model `Text(editable: true)` builds on.
//! * [`measure_text`] — one-shot UI measure callback.

pub mod editor;
pub mod font;
pub mod layout;
pub mod model;
pub mod runs;
pub mod selection;
pub mod style;

pub use editor::TextEditor;
pub use font::{FontManager, SharedFonts};
pub use layout::{
    measure_text, CaretRect, LaidGlyph, LaidLine, LaidOut, Rect, SharedLayoutCache, TextLayout,
};
pub use model::TextModel;
pub use runs::{ResolveError, RunHandle, RunId, RunRegistry, SharedRuns};
pub use selection::Selection;
pub use style::{
    FontFamily, FontSlant, FontStretch, FontWeight, LineMode, StyledSpan, TextAlign, TextStyle,
    WrapMode,
};

/// Pinned interoperable dependency versions. `aqua-render` must use the
/// same `cosmic-text`/`glyphon`/`wgpu` major versions; bump these together
/// with `docs/TEXT.md` and the renderer contract.
pub const COMPAT_COSMIC_TEXT: &str = "0.19";
pub const COMPAT_GLYPHON: &str = "0.12";
pub const COMPAT_WGPU: &str = "30";

/// Crate API generation. Bump when public signatures change incompatibly
/// so peers can detect skew through `docs/IMPLEMENTATION_CONTRACT.md`.
pub const AQUA_TEXT_API_VERSION: u32 = 1;
