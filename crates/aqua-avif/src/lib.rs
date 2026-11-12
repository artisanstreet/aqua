mod color;
mod decoder;

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AvifError {
    Container(String),
    TooManyPixels(u64),
    Decoder(u32),
    Unsupported(&'static str),
}

impl fmt::Display for AvifError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Container(message) => write!(formatter, "avif: {message}"),
            Self::TooManyPixels(pixels) => {
                write!(formatter, "avif: {pixels} pixels exceed the cap")
            }
            Self::Decoder(code) => write!(formatter, "avif: AV1 decoder error {code}"),
            Self::Unsupported(what) => write!(formatter, "avif: unsupported {what}"),
        }
    }
}

impl std::error::Error for AvifError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

fn parse(bytes: &[u8]) -> Result<avif_parse::AvifData, AvifError> {
    std::panic::catch_unwind(|| avif_parse::read_avif(&mut &bytes[..]))
        .map_err(|_| AvifError::Container("malformed container".into()))?
        .map_err(|error| AvifError::Container(error.to_string()))
}

fn extent(avif: &avif_parse::AvifData, max_pixels: u64) -> Result<(u32, u32), AvifError> {
    let metadata = avif
        .primary_item_metadata()
        .map_err(|error| AvifError::Container(error.to_string()))?;
    let (width, height) = (
        metadata.max_frame_width.get(),
        metadata.max_frame_height.get(),
    );
    let pixels = u64::from(width) * u64::from(height);
    match pixels > max_pixels {
        true => Err(AvifError::TooManyPixels(pixels)),
        false => Ok((width, height)),
    }
}

pub fn dimensions(bytes: &[u8]) -> Result<(u32, u32), AvifError> {
    extent(&parse(bytes)?, u64::MAX)
}

pub fn decode(bytes: &[u8], max_pixels: u64) -> Result<Rgba, AvifError> {
    let avif = parse(bytes)?;
    extent(&avif, max_pixels)?;
    let mut image = decoder::with_picture(&avif.primary_item, max_pixels, color::rgba)??;
    if let Some(alpha) = &avif.alpha_item {
        let premultiplied = avif.premultiplied_alpha;
        decoder::with_picture(alpha, max_pixels, |picture| {
            color::apply_alpha(&mut image, picture, premultiplied)
        })??;
    }
    Ok(image)
}
