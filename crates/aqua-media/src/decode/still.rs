use std::io::Cursor;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbaImage};

use super::{truncate, DecodedImage, MediaError, MAX_IMAGE_DIM};

pub const MAX_SOURCE_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_SOURCE_DIM: u32 = 65_535;
pub const MAX_SOURCE_PIXELS: u64 = 128 * 1024 * 1024;
pub const MAX_DETAIL_PIXELS: u64 = 32 * 1024 * 1024;

const SERIAL_PIXELS: u64 = 32 * 1024 * 1024;
const DECODER_THREADS: usize = 8;
const DETAIL_STEPS: u32 = 10;
const AVIF_BRANDS: [&[u8; 4]; 2] = [b"avif", b"avis"];
const HEIC_BRANDS: [&[u8; 4]; 10] = [
    b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"hevm", b"hevs", b"mif1", b"msf1",
];

static LARGE_DECODE: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StillFormat {
    Png,
    Jpeg,
    Gif,
    WebP,
    Bmp,
    Tiff,
    Ico,
    Avif,
    Heic,
}

impl StillFormat {
    fn container(self) -> Option<ImageFormat> {
        match self {
            Self::Png => Some(ImageFormat::Png),
            Self::Jpeg => Some(ImageFormat::Jpeg),
            Self::Gif => Some(ImageFormat::Gif),
            Self::WebP => Some(ImageFormat::WebP),
            Self::Bmp => Some(ImageFormat::Bmp),
            Self::Tiff => Some(ImageFormat::Tiff),
            Self::Ico => Some(ImageFormat::Ico),
            Self::Avif | Self::Heic => None,
        }
    }
}

fn brands(bytes: &[u8]) -> impl Iterator<Item = &[u8; 4]> {
    let declared = bytes
        .first_chunk::<4>()
        .map_or(0, |size| u32::from_be_bytes(*size)) as usize;
    let end = declared.min(bytes.len());
    let major = bytes
        .get(8..12)
        .and_then(|brand| brand.first_chunk::<4>())
        .filter(|_| bytes.get(4..8) == Some(b"ftyp"));
    let compatible = major
        .and_then(|_| bytes.get(16..end))
        .unwrap_or_default()
        .as_chunks::<4>()
        .0;
    major.into_iter().chain(compatible)
}

pub fn sniff(bytes: &[u8]) -> Option<StillFormat> {
    let listed = |known: &[&[u8; 4]]| brands(bytes).any(|brand| known.contains(&brand));
    if listed(&AVIF_BRANDS) {
        return Some(StillFormat::Avif);
    }
    if listed(&HEIC_BRANDS) {
        return Some(StillFormat::Heic);
    }
    match image::guess_format(bytes).ok()? {
        ImageFormat::Png => Some(StillFormat::Png),
        ImageFormat::Jpeg => Some(StillFormat::Jpeg),
        ImageFormat::Gif => Some(StillFormat::Gif),
        ImageFormat::WebP => Some(StillFormat::WebP),
        ImageFormat::Bmp => Some(StillFormat::Bmp),
        ImageFormat::Tiff => Some(StillFormat::Tiff),
        ImageFormat::Ico => Some(StillFormat::Ico),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Detail(u32);

impl Detail {
    pub const SMALLEST: Self = Self(256);
    pub const STANDARD: Self = Self(2048);
    pub const FULL: Self = Self(MAX_IMAGE_DIM);

    fn step(index: u32) -> u32 {
        (256.0 * 2f64.powf(f64::from(index) / 2.0)).round() as u32
    }

    pub fn covering(long_side: f32) -> Self {
        (0..=DETAIL_STEPS)
            .map(Self::step)
            .find(|side| *side as f32 >= long_side)
            .map_or(Self::FULL, Self)
    }

    pub fn lower(self) -> Option<Self> {
        (0..=DETAIL_STEPS)
            .map(Self::step)
            .take_while(|side| *side < self.0)
            .last()
            .map(Self)
    }

    pub fn long_side(self) -> u32 {
        self.0
    }

    pub fn fit(self, source: (u32, u32)) -> (u32, u32) {
        let (width, height) = (f64::from(source.0.max(1)), f64::from(source.1.max(1)));
        let scale = (f64::from(self.0) / width.max(height))
            .min((MAX_DETAIL_PIXELS as f64 / (width * height)).sqrt())
            .min(1.0);
        let side = |length: f64| ((length * scale).round() as u32).clamp(1, MAX_IMAGE_DIM);
        let (mut width, mut height) = (side(width), side(height));
        while u64::from(width) * u64::from(height) > MAX_DETAIL_PIXELS {
            match width >= height {
                true => width -= 1,
                false => height -= 1,
            }
        }
        (width, height)
    }
}

#[derive(Clone, Debug)]
pub struct Still {
    pub image: DecodedImage,
    pub source: (u32, u32),
}

fn failed(error: image::ImageError) -> MediaError {
    match error {
        image::ImageError::Limits(_) => MediaError::OverBudget,
        error => MediaError::Decode(truncate(error.to_string())),
    }
}

fn threads() -> usize {
    std::thread::available_parallelism()
        .map_or(4, std::num::NonZero::get)
        .min(DECODER_THREADS)
}

fn heic_options() -> heic_rs::DecodeOptions {
    heic_rs::DecodeOptions::default()
        .with_layout(heic_rs::PixelLayout::Rgba8)
        .with_max_pixels(Some(MAX_SOURCE_PIXELS))
        .with_threads(Some(threads()))
}

fn extent(bytes: &[u8], format: StillFormat) -> Result<(u32, u32), MediaError> {
    match (format, format.container()) {
        (_, Some(container)) => ImageReader::with_format(Cursor::new(bytes), container)
            .into_dimensions()
            .map_err(failed),
        (StillFormat::Avif, _) => {
            aqua_avif::dimensions(bytes).map_err(|error| MediaError::Decode(error.to_string()))
        }
        _ => heic_rs::probe(bytes)
            .map(|info| (info.width, info.height))
            .map_err(|error| MediaError::Decode(truncate(error.to_string()))),
    }
}

fn rgba(width: u32, height: u32, pixels: Vec<u8>) -> Result<DynamicImage, MediaError> {
    RgbaImage::from_raw(width, height, pixels)
        .map(DynamicImage::ImageRgba8)
        .ok_or_else(|| {
            MediaError::Decode("RGBA byte length does not match image dimensions".into())
        })
}

fn load(bytes: &[u8], format: StillFormat) -> Result<DynamicImage, MediaError> {
    match (format, format.container()) {
        (_, Some(container)) => {
            let mut reader = ImageReader::with_format(Cursor::new(bytes), container);
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(MAX_SOURCE_DIM);
            limits.max_image_height = Some(MAX_SOURCE_DIM);
            limits.max_alloc = Some(MAX_SOURCE_PIXELS * 8);
            reader.limits(limits);
            let mut decoder = reader.into_decoder().map_err(failed)?;
            let orientation = decoder
                .orientation()
                .unwrap_or(image::metadata::Orientation::NoTransforms);
            let mut image = DynamicImage::from_decoder(decoder).map_err(failed)?;
            image.apply_orientation(orientation);
            Ok(image)
        }
        (StillFormat::Avif, _) => aqua_avif::decode(bytes, MAX_SOURCE_PIXELS)
            .map_err(|error| MediaError::Decode(error.to_string()))
            .and_then(|image| rgba(image.width, image.height, image.pixels)),
        _ => heic_rs::decode(bytes, &heic_options())
            .map_err(|error| MediaError::Decode(truncate(error.to_string())))
            .and_then(|image| rgba(image.width, image.height, image.data)),
    }
}

fn admitted(bytes: &[u8]) -> Result<(StillFormat, (u32, u32)), MediaError> {
    if bytes.is_empty() {
        return Err(MediaError::Empty);
    }
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(MediaError::SourceTooLarge(bytes.len()));
    }
    let format = sniff(bytes).ok_or(MediaError::UnsupportedFormat)?;
    let (width, height) = catch_unwind(|| extent(bytes, format))
        .map_err(|_| MediaError::Decode("image header is malformed".into()))??;
    if width == 0 || height == 0 {
        return Err(MediaError::Decode("zero-sized image".into()));
    }
    if width > MAX_SOURCE_DIM || height > MAX_SOURCE_DIM {
        return Err(MediaError::DimensionsTooLarge(width, height));
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_SOURCE_PIXELS {
        return Err(MediaError::TooManyPixels(pixels));
    }
    Ok((format, (width, height)))
}

fn oriented(
    bytes: &[u8],
    format: StillFormat,
    extent: (u32, u32),
) -> Result<DynamicImage, MediaError> {
    let _serial = (u64::from(extent.0) * u64::from(extent.1) > SERIAL_PIXELS).then(|| {
        LARGE_DECODE
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    });
    catch_unwind(AssertUnwindSafe(|| load(bytes, format)))
        .map_err(|_| MediaError::Decode("image data is malformed".into()))?
}

pub fn dimensions(bytes: &[u8]) -> Result<(u32, u32), MediaError> {
    admitted(bytes).map(|(_, extent)| extent)
}

pub fn decode_still(bytes: &[u8], detail: Detail, max_bytes: u64) -> Result<Still, MediaError> {
    let (format, extent) = admitted(bytes)?;
    let image = oriented(bytes, format, extent)?;
    let source = (image.width(), image.height());
    let (width, height) = detail.fit(source);
    if u64::from(width) * u64::from(height) * 4 > max_bytes {
        return Err(MediaError::OverBudget);
    }
    let filter = match u64::from(width) * 2 <= u64::from(source.0) {
        true => image::imageops::FilterType::Triangle,
        false => image::imageops::FilterType::CatmullRom,
    };
    let image = match (width, height) == source {
        true => image,
        false => image.resize_exact(width, height, filter),
    };
    Ok(Still {
        image: DecodedImage::from_rgba(width, height, image.into_rgba8().into_raw())?,
        source,
    })
}

pub fn decode_whole(bytes: &[u8]) -> Result<DecodedImage, MediaError> {
    let (format, (width, height)) = admitted(bytes)?;
    if width > MAX_IMAGE_DIM || height > MAX_IMAGE_DIM {
        return Err(MediaError::DimensionsTooLarge(width, height));
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > super::MAX_IMAGE_PIXELS {
        return Err(MediaError::TooManyPixels(pixels));
    }
    let image = oriented(bytes, format, (width, height))?.into_rgba8();
    DecodedImage::from_rgba(image.width(), image.height(), image.into_raw())
}
