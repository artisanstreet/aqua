#![cfg(windows)]

mod oracle;

pub use oracle::{
    BlendParams, Canvas, ColorCanvas, FontSource, GlyphRunSpec, Oracle, OracleError, RenderMode,
    ShapedGlyph, StrikeImage, SystemRendering,
};
