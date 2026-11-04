use aqua_text::raster::GlyphImage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Content {
    Mask,
    Color,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Placement {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct Image {
    pub content: Content,
    pub placement: Placement,
    pub data: Vec<u8>,
}

impl From<GlyphImage> for Image {
    fn from(image: GlyphImage) -> Self {
        let (content, bitmap) = match image {
            GlyphImage::Mask(bitmap) => (Content::Mask, bitmap),
            GlyphImage::Color(bitmap) => (Content::Color, bitmap),
        };
        Self {
            content,
            placement: Placement {
                left: bitmap.left,
                top: bitmap.top,
                width: bitmap.width,
                height: bitmap.height,
            },
            data: bitmap.data,
        }
    }
}
