#![cfg(windows)]

mod oracle;

pub use oracle::{
    BlendParams, Canvas, ColorCanvas, FontSource, StrikeImage, GlyphRunSpec, Oracle, OracleError, RenderMode, ShapedGlyph,
    SystemRendering,
};
