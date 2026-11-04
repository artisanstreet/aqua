pub mod animation;

use std::collections::VecDeque;

use aqua_core::id::{ImageId, RawId};
use aqua_core::scene::LAYER_MAX_GENERATION;

pub const MAX_IMAGE_DIM: u32 = 8192;
pub const MAX_IMAGE_PIXELS: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaError {
    Empty,
    UnsupportedFormat,
    Decode(String),
    DimensionsTooLarge(u32, u32),
    TooManyPixels(u64),
    OverBudget,
    UnknownImage,
    StaleHandle,
    Gpu(String),
}

impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MediaError::Empty => write!(f, "media: empty input"),
            MediaError::UnsupportedFormat => {
                write!(
                    f,
                    "media: unsupported format (enabled: png, bmp, jpeg, gif, webp)"
                )
            }
            MediaError::Decode(m) => write!(f, "media: decode failed: {m}"),
            MediaError::DimensionsTooLarge(w, h) => {
                write!(f, "media: dimensions {w}x{h} exceed {MAX_IMAGE_DIM}px")
            }
            MediaError::TooManyPixels(n) => {
                write!(f, "media: {n} pixels exceed {MAX_IMAGE_PIXELS} cap")
            }
            MediaError::OverBudget => write!(f, "media: cache budget exceeded"),
            MediaError::UnknownImage => write!(f, "media: unknown image handle"),
            MediaError::StaleHandle => write!(f, "media: stale image handle"),
            MediaError::Gpu(m) => write!(f, "media: gpu error: {m}"),
        }
    }
}

impl std::error::Error for MediaError {}

#[derive(Clone, Debug)]
pub struct DecodedImage {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl DecodedImage {
    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, MediaError> {
        if width == 0 || height == 0 {
            return Err(MediaError::Decode("zero-sized image".into()));
        }
        if width > MAX_IMAGE_DIM || height > MAX_IMAGE_DIM {
            return Err(MediaError::DimensionsTooLarge(width, height));
        }
        let count = u64::from(width) * u64::from(height);
        if count > MAX_IMAGE_PIXELS {
            return Err(MediaError::TooManyPixels(count));
        }
        if count * 4 != pixels.len() as u64 {
            return Err(MediaError::Decode(
                "RGBA byte length does not match image dimensions".into(),
            ));
        }
        Ok(Self {
            width,
            height,
            pixels: pixels.into_boxed_slice().into_vec(),
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn pixel_at(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        Some([
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ])
    }

    pub fn to_premultiplied_linear_rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len());
        for px in self.pixels.as_chunks::<4>().0 {
            let a = px[3] as f32 / 255.0;
            let lin = aqua_core::color::srgb8_to_linear([px[0], px[1], px[2]]);
            for c in lin {
                out.push((c * a.clamp(0.0, 1.0) * 255.0).round().clamp(0.0, 255.0) as u8);
            }
            out.push(px[3]);
        }
        out
    }

    pub fn to_premultiplied_linear_f16(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 2);
        for px in self.pixels.as_chunks::<4>().0 {
            let a = px[3] as f32 / 255.0;
            let lin = aqua_core::color::srgb8_to_linear([px[0], px[1], px[2]]);
            for c in lin {
                out.extend_from_slice(&f32_to_f16_bits(c * a).to_le_bytes());
            }
            out.extend_from_slice(&f32_to_f16_bits(a).to_le_bytes());
        }
        out
    }

    pub fn premultiplied_linear_f32(&self) -> Vec<[f32; 4]> {
        self.pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|px| {
                let lin = aqua_core::color::srgb8_to_linear([px[0], px[1], px[2]]);
                let a = px[3] as f32 / 255.0;
                [lin[0] * a, lin[1] * a, lin[2] * a, a]
            })
            .collect()
    }
}

pub fn is_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    let text = match std::str::from_utf8(head) {
        Ok(text) => text,
        Err(error) => std::str::from_utf8(&head[..error.valid_up_to()]).unwrap_or_default(),
    };
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with('<') && text.contains("<svg")
}

pub fn is_gif(bytes: &[u8]) -> bool {
    matches!(image::guess_format(bytes), Ok(image::ImageFormat::Gif))
}

pub fn decode_svg(bytes: &[u8], max_bytes: u64) -> Result<DecodedImage, MediaError> {
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(MediaError::OverBudget);
    }
    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(bytes, &options)
        .map_err(|error| MediaError::Decode(error.to_string()))?;
    let width = tree.size().width() * 2.0;
    let height = tree.size().height() * 2.0;
    if !width.is_finite()
        || !height.is_finite()
        || width.ceil() > MAX_IMAGE_DIM as f32
        || height.ceil() > MAX_IMAGE_DIM as f32
        || width.ceil() as u64 * height.ceil() as u64 * 4 > max_bytes.min(64 * 1024 * 1024)
    {
        return Err(MediaError::OverBudget);
    }
    let mut pixmap = tiny_skia::Pixmap::new(width as u32, height as u32)
        .ok_or_else(|| MediaError::Decode("zero-sized image".into()))?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(2.0, 2.0),
        &mut pixmap.as_mut(),
    );
    let (width, height) = (pixmap.width(), pixmap.height());
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for pixel in pixmap.pixels() {
        let alpha = pixel.alpha();
        let straight = |channel: u8| {
            if alpha == 0 {
                channel
            } else {
                (f32::from(channel) / (f32::from(alpha) / 255.0)) as u8
            }
        };
        pixels.extend_from_slice(&[
            straight(pixel.red()),
            straight(pixel.green()),
            straight(pixel.blue()),
            alpha,
        ]);
    }
    DecodedImage::from_rgba(width, height, pixels)
}

pub fn decode_preview_still(bytes: &[u8], max_bytes: u64) -> Result<DecodedImage, MediaError> {
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(MediaError::OverBudget);
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| MediaError::Decode(error.to_string()))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(max_bytes.min(64 * 1024 * 1024));
    reader.limits(limits);
    let mut image = reader
        .decode()
        .map_err(|error| MediaError::Decode(error.to_string()))?;
    if image.width() > 1920 || image.height() > 1440 {
        image = image.resize(1920, 1440, image::imageops::FilterType::CatmullRom);
    }
    let image = image.into_rgba8();
    if image.as_raw().len() as u64 > max_bytes.min(64 * 1024 * 1024) {
        return Err(MediaError::OverBudget);
    }
    DecodedImage::from_rgba(image.width(), image.height(), image.into_raw())
}

pub fn decode_bytes(bytes: &[u8]) -> Result<DecodedImage, MediaError> {
    if bytes.is_empty() {
        return Err(MediaError::Empty);
    }
    let format = image::guess_format(bytes).map_err(|_| MediaError::UnsupportedFormat)?;
    match format {
        image::ImageFormat::Png
        | image::ImageFormat::Bmp
        | image::ImageFormat::Jpeg
        | image::ImageFormat::Gif
        | image::ImageFormat::WebP => {}
        _ => return Err(MediaError::UnsupportedFormat),
    }
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| MediaError::UnsupportedFormat)?;
    let (w, h) = reader
        .into_dimensions()
        .map_err(|e| MediaError::Decode(truncate(e.to_string())))?;
    if w == 0 || h == 0 {
        return Err(MediaError::Decode("zero-sized image".into()));
    }
    if w > MAX_IMAGE_DIM || h > MAX_IMAGE_DIM {
        return Err(MediaError::DimensionsTooLarge(w, h));
    }
    let count = w as u64 * h as u64;
    if count > MAX_IMAGE_PIXELS {
        return Err(MediaError::TooManyPixels(count));
    }
    let img = image::load_from_memory_with_format(bytes, format)
        .map_err(|e| MediaError::Decode(truncate(e.to_string())))?;
    let rgba = img.to_rgba8();
    let (w2, h2) = (rgba.width(), rgba.height());
    debug_assert_eq!((w, h), (w2, h2), "header/body dimension mismatch");
    Ok(DecodedImage {
        width: w2,
        height: h2,
        pixels: rgba.into_raw(),
    })
}

fn truncate(mut s: String) -> String {
    if s.len() > 256 {
        s.truncate(256);
        s.push('…');
    }
    s
}

#[derive(Clone, Copy, Debug)]
pub struct MediaBudget {
    pub max_decoded_bytes: u64,
    pub max_images: usize,
}

impl Default for MediaBudget {
    fn default() -> Self {
        Self {
            max_decoded_bytes: 64 * 1024 * 1024,
            max_images: 256,
        }
    }
}

struct Slot {
    generation: u32,
    alive: bool,
    image: DecodedImage,
    bytes: u64,
}

impl std::fmt::Debug for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Slot")
            .field("generation", &self.generation)
            .field("alive", &self.alive)
            .field("bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
pub struct MediaCache {
    budget: MediaBudget,
    slots: Vec<Slot>,
    free: Vec<(u32, u32)>,
    lru: VecDeque<u32>,
    next_evictions: u64,
}

impl MediaCache {
    pub fn new(budget: MediaBudget) -> Self {
        Self {
            budget,
            ..Default::default()
        }
    }

    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.alive).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn decoded_bytes(&self) -> u64 {
        self.slots.iter().filter(|s| s.alive).map(|s| s.bytes).sum()
    }

    pub fn evictions(&self) -> u64 {
        self.next_evictions
    }

    pub fn decode_and_insert(&mut self, bytes: &[u8]) -> Result<ImageId, MediaError> {
        self.insert(decode_bytes(bytes)?)
    }

    pub fn insert(&mut self, image: DecodedImage) -> Result<ImageId, MediaError> {
        let bytes = image.width as u64 * image.height as u64 * 4;
        if bytes > self.budget.max_decoded_bytes {
            return Err(MediaError::OverBudget);
        }
        while self.decoded_bytes() + bytes > self.budget.max_decoded_bytes
            || self.len() >= self.budget.max_images
        {
            if !self.evict_oldest() {
                return Err(MediaError::OverBudget);
            }
        }
        let (index, generation) = match self.free.pop() {
            Some((index, generation)) => {
                let slot = &mut self.slots[index as usize];
                slot.generation = generation;
                slot.alive = true;
                slot.image = image;
                slot.bytes = bytes;
                (index, generation)
            }
            None => {
                let index = self.slots.len() as u32;
                self.slots.push(Slot {
                    generation: 0,
                    alive: true,
                    image,
                    bytes,
                });
                (index, 0)
            }
        };
        self.touch(index);
        Ok(ImageId::from_raw(RawId { index, generation }))
    }

    pub fn get(&self, id: ImageId) -> Option<&DecodedImage> {
        let raw = id.raw();
        self.slots
            .get(raw.index as usize)
            .filter(|s| s.alive && s.generation == raw.generation && raw.is_valid())
            .map(|s| &s.image)
    }

    pub fn remove(&mut self, id: ImageId) -> bool {
        let raw = id.raw();
        let Some(slot) = self.slots.get_mut(raw.index as usize) else {
            return false;
        };
        if !slot.alive || slot.generation != raw.generation {
            return false;
        }
        self.kill_slot(raw.index);
        true
    }

    fn touch(&mut self, index: u32) {
        self.lru.retain(|&i| i != index);
        self.lru.push_back(index);
    }

    fn evict_oldest(&mut self) -> bool {
        let Some(oldest) = self.lru.pop_front() else {
            return false;
        };
        if !self.slots.get(oldest as usize).is_some_and(|s| s.alive) {
            return true;
        }
        self.kill_slot(oldest);
        self.next_evictions += 1;
        true
    }

    fn kill_slot(&mut self, index: u32) {
        let slot = &mut self.slots[index as usize];
        slot.alive = false;
        slot.bytes = 0;
        slot.image = DecodedImage {
            width: 0,
            height: 0,
            pixels: Vec::new(),
        };
        if let Some(next) = next_generation(slot.generation) {
            slot.generation = next;
            self.free.push((index, next));
        }
        self.lru.retain(|&i| i != index);
    }
}

fn next_generation(current: u32) -> Option<u32> {
    (current < LAYER_MAX_GENERATION).then(|| current + 1)
}

pub fn f32_to_f16_bits(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x7fffff;
    if exp == 255 {
        return sign | 0x7c00 | if mant == 0 { 0 } else { 0x0200 };
    }
    let half_exp = exp - 127 + 15;
    if half_exp >= 31 {
        return sign | 0x7c00; // overflow -> inf
    }
    if half_exp <= 0 {
        if half_exp < -10 {
            return sign; // too small even for subnormals
        }
        let m = mant | 0x800000;
        let shift = (1 - half_exp) as u32;
        let rest = m & ((1 << shift) - 1);
        let half = 1 << (shift - 1);
        let mut out = m >> shift;
        if rest > half || (rest == half && (out & 1) == 1) {
            out += 1;
        }
        return sign | out as u16;
    }
    let mut out = (mant >> 13) as u16;
    let rest = mant & 0x1fff;
    if rest > 0x1000 || (rest == 0x1000 && (out & 1) == 1) {
        out += 1;
        if out == 0x0400 {
            return sign | (((half_exp + 1) as u16) << 10);
        }
    }
    sign | ((half_exp as u16) << 10) | (out & 0x03ff)
}
