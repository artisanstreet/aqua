pub const SUBPIXEL_BINS: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub font: u64,
    pub glyph: u32,
    pub size_sixty_fourths: u32,
    pub subpixel: u8,
    pub variant: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphBitmap {
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
    pub coverage: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColorGlyphBitmap {
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
    pub premultiplied_rgba: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlyphRaster {
    Mask(GlyphBitmap),
    Color(ColorGlyphBitmap),
}

pub trait GlyphSource {
    fn rasterize(&mut self, key: &GlyphKey) -> Option<GlyphRaster>;
}

impl<F> GlyphSource for F
where
    F: FnMut(&GlyphKey) -> Option<GlyphRaster>,
{
    fn rasterize(&mut self, key: &GlyphKey) -> Option<GlyphRaster> {
        self(key)
    }
}

pub trait GlyphRasterizer {
    fn rasterize(&mut self, key: &GlyphKey) -> Option<GlyphBitmap>;
}

impl<F> GlyphRasterizer for F
where
    F: FnMut(&GlyphKey) -> Option<GlyphBitmap>,
{
    fn rasterize(&mut self, key: &GlyphKey) -> Option<GlyphBitmap> {
        self(key)
    }
}

pub fn subpixel_bin(x: f32) -> (i32, u8) {
    let floor = x.floor();
    let bin = ((x - floor) * SUBPIXEL_BINS as f32).round() as i32;
    match bin >= SUBPIXEL_BINS as i32 {
        true => (floor as i32 + 1, 0),
        false => (floor as i32, bin as u8),
    }
}
