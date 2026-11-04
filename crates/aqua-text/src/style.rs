use unicode_segmentation::UnicodeSegmentation;
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FontFamily {
    SansSerif,
    Serif,
    Monospace,
    Cursive,
    Fantasy,
    Named(String),
}

impl FontFamily {
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FontFeature {
    pub tag: [u8; 4],
    pub value: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub families: Vec<FontFamily>,
    pub size_px: f32,
    pub line_height_px: f32,
    pub weight: FontWeight,
    pub slant: FontSlant,
    pub stretch: FontStretch,
    pub letter_spacing_em: Option<f32>,
    pub font_features: Vec<FontFeature>,
    pub gpui_color: Option<[f32; 4]>,
    pub gpui_source_color: Option<[f32; 4]>,
    pub underline: bool,
    pub strikethrough: bool,
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
            font_features: Vec::new(),
            gpui_color: None,
            gpui_source_color: None,
            underline: false,
            strikethrough: false,
        }
    }
}

impl TextStyle {
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
            "fam={}|size={:.3}|lh={:.3}|w={}|s={:?}|st={:?}|ls={:?}|features={:?}|color={:?}|source_color={:?}|u={}|strike={}",
            families.join(","),
            self.size_px,
            self.line_height_px,
            self.weight.0,
            self.slant,
            self.stretch,
            self.letter_spacing_em,
            self.font_features,
            self.gpui_color.map(|color| color.map(f32::to_bits)),
            self.gpui_source_color.map(|color| color.map(f32::to_bits)),
            self.underline,
            self.strikethrough,
        )
    }

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
        let mut features = cosmic_text::FontFeatures::new();
        for feature in &self.font_features {
            features.set(cosmic_text::FeatureTag::new(&feature.tag), feature.value);
        }
        attrs.font_features(features)
    }

    pub fn to_cosmic(&self) -> cosmic_text::Attrs<'static> {
        let family = match self.families.first() {
            Some(FontFamily::SansSerif) => cosmic_text::Family::SansSerif,
            Some(FontFamily::Serif) => cosmic_text::Family::Serif,
            Some(FontFamily::Monospace) => cosmic_text::Family::Monospace,
            Some(FontFamily::Cursive) => cosmic_text::Family::Cursive,
            Some(FontFamily::Fantasy) => cosmic_text::Family::Fantasy,
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
        let mut features = cosmic_text::FontFeatures::new();
        for feature in &self.font_features {
            features.set(cosmic_text::FeatureTag::new(&feature.tag), feature.value);
        }
        attrs.font_features(features)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StyledSpan {
    inline_cell_width: Option<f32>,
    pub interaction: Option<u64>,
    pub color_srgb: Option<[u8; 4]>,
    pub color_hsla: Option<[f32; 4]>,
    pub source_color_hsla: Option<[f32; 4]>,
    pub background_hsla: Option<[f32; 4]>,
    pub text: String,
    pub style: TextStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineCellError {
    InvalidWidth,
    ExpectedOneGrapheme,
}

impl StyledSpan {
    pub fn inline_cell(mut self, width: f32) -> Result<Self, InlineCellError> {
        if !width.is_finite() || width <= 0.0 {
            return Err(InlineCellError::InvalidWidth);
        }
        if self.text.graphemes(true).count() != 1 || self.text.contains(['\n', '\r']) {
            return Err(InlineCellError::ExpectedOneGrapheme);
        }
        self.inline_cell_width = Some(width);
        Ok(self)
    }
    pub fn inline_cell_width(&self) -> Option<f32> {
        self.inline_cell_width
    }

    pub fn link(mut self, id: u64) -> Self {
        self.interaction = Some(id);
        self
    }

    pub fn color(mut self, rgba_srgb: [u8; 4]) -> Self {
        self.color_hsla = None;
        self.source_color_hsla = None;
        self.color_srgb = Some(rgba_srgb);
        self
    }
    pub fn gpui_color(mut self, hsla: [f32; 4]) -> Self {
        self.color_srgb = None;
        self.source_color_hsla = None;
        self.color_hsla = Some(hsla);
        self
    }
    pub fn gpui_source_color(mut self, hsla: [f32; 4]) -> Self {
        self.color_srgb = None;
        self.color_hsla = None;
        self.source_color_hsla = Some(hsla);
        self
    }
    pub fn gpui_background(mut self, hsla: [f32; 4]) -> Self {
        self.background_hsla = Some(hsla);
        self
    }
    pub fn new(text: impl Into<String>, style: TextStyle) -> Self {
        Self {
            inline_cell_width: None,
            interaction: None,
            color_srgb: None,
            color_hsla: None,
            source_color_hsla: None,
            background_hsla: None,
            text: text.into(),
            style,
        }
    }

    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            inline_cell_width: None,
            interaction: None,
            color_srgb: None,
            color_hsla: None,
            source_color_hsla: None,
            background_hsla: None,
            text: text.into(),
            style: TextStyle::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WrapMode {
    None,
    Glyph,
    Word,
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
            TextAlign::Start => None,
            TextAlign::End => Some(cosmic_text::Align::End),
            TextAlign::Left => Some(cosmic_text::Align::Left),
            TextAlign::Right => Some(cosmic_text::Align::Right),
            TextAlign::Center => Some(cosmic_text::Align::Center),
            TextAlign::Justify => Some(cosmic_text::Align::Justified),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LineMode {
    #[default]
    Single,
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
