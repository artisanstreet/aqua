#![cfg(windows)]

mod oracle;

pub use oracle::{
    BlendParams, Canvas, FontSource, GlyphRunSpec, Oracle, OracleError, RenderMode, ShapedGlyph,
    SystemRendering,
};
