//! Shared text style types for Aqua.
//!
//! These are plain-data, renderer-neutral types owned by `aqua-text`.
//! They convert to `cosmic-text` attributes internally, so `aqua-core`,
//! `aqua-ui` and `aqua-render` never need to name shaping-engine types.
//! (`aqua-core` does not exist yet; when it does, these types stay here and
//! core/UI depend on this crate for text styling.)

/// A font family selector, in fallback order.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FontFamily {
    /// System sans-serif UI font.
    SansSerif,
    /// System serif font.
    Serif,
    /// System monospace font.
    Monospace,
    /// System cursive font.
    Cursive,
    /// System fantasy font.
    Fantasy,
    /// A named family, e.g. `"DejaVu Sans"`, `"Noto Sans Arabic"`.
    Named(String),
}

impl FontFamily {
    /// Convert to the shaping engine's family type.
    pub fn to_cosmic(&self) -> cosmic_text::Family<'_> {
        match self {
            FontFamily::SansSerif => cosmic_text::Family::SansSerif,
            FontFamily::Serif => cosmic_text::Family::Serif,
            FontFamily::Monospace => cosmic_text::Family::Monospace,
            FontFamily::Cursive => cosmic_text::Family::Cursive,
            FontFamily::Fantasy => cosmic_text::Family::Fantasy,
            FontFamily::Named(name) => cosmic_text::Family::Name(name.as_str()),
        }
    }
}

/// Font weight as a numeric value (100..=900), following CSS conventions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FontWeight(pub u16);

impl FontWeight {
    pub const THIN: FontWeight = FontWeight(100);
    pub const LIGHT: FontWeight = FontWeight(300);
    pub const NORMAL: FontWeight = FontWeight(400);
    pub const MEDIUM: FontWeight = FontWeight(500);
    pub const BOLD: FontWeight = FontWeight(700);
    pub const BLACK: FontWeight = FontWeight(900);

    pub fn to_cosmic(self) -> cosmic_text::Weight {
        cosmic_text::Weight(self.0)
    }
}

impl Default for FontWeight {
    fn default() -> Self {
        FontWeight::NORMAL
    }
}

/// Upright vs slanted face selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FontSlant {
    #[default]
    Normal,
    Italic,
    Oblique,
}

impl FontSlant {
    pub fn to_cosmic(self) -> cosmic_text::Style {
        match self {
            FontSlant::Normal => cosmic_text::Style::Normal,
            FontSlant::Italic => cosmic_text::Style::Italic,
            FontSlant::Oblique => cosmic_text::Style::Oblique,
        }
    }
}

/// Horizontal condensation/expansion of a face.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FontStretch {
    Condensed,
    #[default]
    Normal,
    Expanded,
}

impl FontStretch {
    pub fn to_cosmic(self) -> cosmic_text::Stretch {
        match self {
            FontStretch::Condensed => cosmic_text::Stretch::Condensed,
            FontStretch::Normal => cosmic_text::Stretch::Normal,
            FontStretch::Expanded => cosmic_text::Stretch::Expanded,
        }
    }
}

/// Complete styling for a run of text.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// Fallback-ordered family list. The first entry is the preferred family.
    pub families: Vec<FontFamily>,
    /// Font size in logical pixels (before the layout scale factor).
    pub size_px: f32,
    /// Line height in logical pixels (before the layout scale factor).
    pub line_height_px: f32,
    pub weight: FontWeight,
    pub slant: FontSlant,
    pub stretch: FontStretch,
    /// Extra letter spacing in units of em. `None` means no override.
    pub letter_spacing_em: Option<f32>,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            families: vec![FontFamily::SansSerif],
            size_px: 16.0,
            line_height_px: 22.0,
            weight: FontWeight::NORMAL,
            slant: FontSlant::Normal,
            stretch: FontStretch::Normal,
            letter_spacing_em: None,
        }
    }
}

impl TextStyle {
    /// A short, stable, human-readable key for cache invalidation.
    /// Any field that affects shaping output must appear here.
    pub fn cache_key(&self) -> String {
        let families: Vec<String> = self
            .families
            .iter()
            .map(|f| match f {
                FontFamily::SansSerif => "sans".to_string(),
                FontFamily::Serif => "serif".to_string(),
                FontFamily::Monospace => "mono".to_string(),
                FontFamily::Cursive => "cursive".to_string(),
                FontFamily::Fantasy => "fantasy".to_string(),
                FontFamily::Named(n) => format!("n:{n}"),
            })
            .collect();
        format!(
            "fam={}|size={:.3}|lh={:.3}|w={}|s={:?}|st={:?}|ls={:?}",
            families.join(","),
            self.size_px,
            self.line_height_px,
            self.weight.0,
            self.slant,
            self.stretch,
            self.letter_spacing_em
        )
    }

    /// Shaping attributes borrowing any named family strings from `self`.
    /// The caller must keep `self` (or the owning spans) alive while the
    /// returned attrs are used.
    pub fn to_cosmic_in(&self) -> cosmic_text::Attrs<'_> {
        let family = match self.families.first() {
            Some(f) => f.to_cosmic(),
            None => cosmic_text::Family::SansSerif,
        };
        let mut attrs = cosmic_text::Attrs::new()
            .family(family)
            .weight(self.weight.to_cosmic())
            .style(self.slant.to_cosmic())
            .stretch(self.stretch.to_cosmic());
        if let Some(ls) = self.letter_spacing_em {
            attrs = attrs.letter_spacing(ls);
        }
        attrs
    }

    /// Base shaping attributes for this style (first family only).
    /// Named families fall back to sans-serif here because `Attrs` borrows
    /// the name; prefer [`TextStyle::to_cosmic_in`] in the layout layer.
    pub fn to_cosmic(&self) -> cosmic_text::Attrs<'static> {
        let family = match self.families.first() {
            Some(FontFamily::SansSerif) => cosmic_text::Family::SansSerif,
            Some(FontFamily::Serif) => cosmic_text::Family::Serif,
            Some(FontFamily::Monospace) => cosmic_text::Family::Monospace,
            Some(FontFamily::Cursive) => cosmic_text::Family::Cursive,
            Some(FontFamily::Fantasy) => cosmic_text::Family::Fantasy,
            // `Attrs` borrows the name; the layout layer keeps owned spans
            // alive, so this arm is only used for single static families.
            // Named families go through `to_cosmic_in`.
            Some(FontFamily::Named(_)) => cosmic_text::Family::SansSerif,
            None => cosmic_text::Family::SansSerif,
        };
        let mut attrs = cosmic_text::Attrs::new()
            .family(family)
            .weight(self.weight.to_cosmic())
            .style(self.slant.to_cosmic())
            .stretch(self.stretch.to_cosmic());
        if let Some(ls) = self.letter_spacing_em {
            attrs = attrs.letter_spacing(ls);
        }
        attrs
    }
}

/// One styled run inside a paragraph. Inline styling is data within one
/// layout operation: a styled word must not become a separate layout child.
#[derive(Clone, Debug, PartialEq)]
pub struct StyledSpan {
    pub text: String,
    pub style: TextStyle,
}

impl StyledSpan {
    pub fn new(text: impl Into<String>, style: TextStyle) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: TextStyle::default(),
        }
    }
}

/// Line-wrapping policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WrapMode {
    /// No wrapping; lines break only at explicit newlines.
    None,
    /// Wrap at glyph boundaries.
    Glyph,
    /// Wrap at word boundaries.
    Word,
    /// Wrap at word boundaries, falling back to glyph boundaries for
    /// words that cannot fit on a line by themselves.
    #[default]
    WordOrGlyph,
}

impl WrapMode {
    pub fn to_cosmic(self) -> cosmic_text::Wrap {
        match self {
            WrapMode::None => cosmic_text::Wrap::None,
            WrapMode::Glyph => cosmic_text::Wrap::Glyph,
            WrapMode::Word => cosmic_text::Wrap::Word,
            WrapMode::WordOrGlyph => cosmic_text::Wrap::WordOrGlyph,
        }
    }
}

/// Horizontal alignment of laid-out lines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextAlign {
    #[default]
    Start,
    End,
    Left,
    Right,
    Center,
    Justify,
}

impl TextAlign {
    pub fn to_cosmic(self) -> Option<cosmic_text::Align> {
        match self {
            TextAlign::Start => None, // shaper default: LTR -> Left, RTL -> Right
            TextAlign::End => Some(cosmic_text::Align::End),
            TextAlign::Left => Some(cosmic_text::Align::Left),
            TextAlign::Right => Some(cosmic_text::Align::Right),
            TextAlign::Center => Some(cosmic_text::Align::Center),
            TextAlign::Justify => Some(cosmic_text::Align::Justified),
        }
    }
}

/// Single-line vs multi-line behavior. These are separate policies: a
/// single-line field rejects newlines on input and never wraps, while a
/// multi-line field honors newlines and wraps per [`WrapMode`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LineMode {
    /// One visual line; newline characters are stripped on input.
    #[default]
    Single,
    /// Newlines break lines; wrapping follows [`WrapMode`].
    Multi,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_changes_with_style_fields() {
        let a = TextStyle::default();
        let mut b = a.clone();
        b.weight = FontWeight::BOLD;
        assert_ne!(a.cache_key(), b.cache_key());
        let mut c = a.clone();
        c.size_px = 18.0;
        assert_ne!(a.cache_key(), c.cache_key());
    }
}
