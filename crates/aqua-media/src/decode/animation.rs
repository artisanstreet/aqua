use std::io::Cursor;
use std::num::NonZeroU32;
use std::time::Duration;

use image::{AnimationDecoder, ImageDecoder};

use super::{DecodedImage, MediaError, MAX_IMAGE_DIM};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationLoops {
    Infinite,
    Count(NonZeroU32),
}

#[derive(Debug, Clone)]
pub struct AnimationFrame {
    pub image: DecodedImage,
    pub delay: Duration,
}

#[derive(Debug, Clone)]
pub struct DecodedAnimation {
    frames: Vec<AnimationFrame>,
    ends: Vec<u128>,
    loops: AnimationLoops,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimationSample {
    pub frame: usize,
    pub next_frame_at: Option<Duration>,
    pub complete: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct AnimationBudget {
    pub max_bytes: u64,
    pub max_frames: usize,
    pub minimum_delay: Duration,
}

impl Default for AnimationBudget {
    fn default() -> Self {
        Self {
            max_bytes: 64 * 1024 * 1024,
            max_frames: 512,
            minimum_delay: Duration::from_millis(10),
        }
    }
}

impl DecodedAnimation {
    pub fn new(frames: Vec<AnimationFrame>, loops: AnimationLoops) -> Result<Self, MediaError> {
        if frames.is_empty() {
            return Err(MediaError::Decode("animation has no frames".into()));
        }
        let mut elapsed = Duration::ZERO;
        let mut ends = Vec::with_capacity(frames.len());
        for frame in &frames {
            if frame.delay.is_zero() {
                return Err(MediaError::Decode("animation frame delay is zero".into()));
            }
            elapsed = elapsed
                .checked_add(frame.delay)
                .ok_or_else(|| MediaError::Decode("animation duration overflow".into()))?;
            ends.push(elapsed.as_nanos());
        }
        Ok(Self {
            frames,
            ends,
            loops,
        })
    }

    pub fn frames(&self) -> &[AnimationFrame] {
        &self.frames
    }

    pub fn loops(&self) -> AnimationLoops {
        self.loops
    }

    pub fn sample(&self, elapsed: Duration) -> AnimationSample {
        if self.frames.len() == 1 {
            return AnimationSample {
                frame: 0,
                next_frame_at: None,
                complete: true,
            };
        }
        let duration = (*self.ends.last().unwrap_or(&1)).max(1);
        let now = elapsed.as_nanos();
        if let AnimationLoops::Count(count) = self.loops {
            if now / duration >= u128::from(count.get()) {
                return AnimationSample {
                    frame: self.frames.len() - 1,
                    next_frame_at: None,
                    complete: true,
                };
            }
        }
        let within = now % duration;
        let frame = self
            .ends
            .partition_point(|end| *end <= within)
            .min(self.frames.len() - 1);
        let next = now - within + self.ends[frame];
        let next_frame_at = u64::try_from(next / 1_000_000_000)
            .ok()
            .map(|seconds| Duration::new(seconds, (next % 1_000_000_000) as u32));
        AnimationSample {
            frame,
            next_frame_at: if self.frames.len() > 1 {
                next_frame_at
            } else {
                None
            },
            complete: false,
        }
    }
}

pub fn decode_preview_gif(bytes: &[u8], max_bytes: u64) -> Result<DecodedAnimation, MediaError> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIM);
    limits.max_image_height = Some(MAX_IMAGE_DIM);
    limits.max_alloc = Some(max_bytes.min(64 * 1024 * 1024));
    let mut decoder =
        image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).map_err(decode_error)?;
    decoder.set_limits(limits).map_err(decode_error)?;
    let mut frames = Vec::new();
    let mut ends = Vec::new();
    let mut retained = 0u64;
    let mut elapsed = Duration::ZERO;
    for frame in decoder.into_frames() {
        if frames.len() == 120 {
            return Err(MediaError::OverBudget);
        }
        let frame = frame.map_err(decode_error)?;
        let delay = Duration::from(frame.delay());
        let buffer = frame.into_buffer();
        retained = retained
            .checked_add(buffer.as_raw().len() as u64)
            .ok_or(MediaError::OverBudget)?;
        if retained > max_bytes.min(64 * 1024 * 1024) {
            return Err(MediaError::OverBudget);
        }
        elapsed = elapsed.checked_add(delay).ok_or(MediaError::OverBudget)?;
        ends.push(elapsed.as_nanos());
        frames.push(AnimationFrame {
            image: DecodedImage::from_rgba(buffer.width(), buffer.height(), buffer.into_raw())?,
            delay,
        });
    }
    if frames.is_empty() {
        return Err(MediaError::Decode("animation has no frames".into()));
    }
    Ok(DecodedAnimation {
        frames,
        ends,
        loops: AnimationLoops::Infinite,
    })
}

pub fn decode_animation(
    bytes: &[u8],
    budget: AnimationBudget,
) -> Result<DecodedAnimation, MediaError> {
    if budget.max_frames == 0 || budget.max_bytes == 0 || budget.minimum_delay.is_zero() {
        return Err(MediaError::OverBudget);
    }
    let format = super::still::sniff(bytes).ok_or(MediaError::UnsupportedFormat)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIM);
    limits.max_image_height = Some(MAX_IMAGE_DIM);
    limits.max_alloc = Some(budget.max_bytes);
    match format {
        super::StillFormat::Gif => {
            let mut decoder =
                image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).map_err(decode_error)?;
            decoder.set_limits(limits).map_err(decode_error)?;
            collect_frames(decoder, budget)
        }
        super::StillFormat::WebP => {
            let mut decoder =
                image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)).map_err(decode_error)?;
            decoder.set_limits(limits).map_err(decode_error)?;
            match decoder.has_animation() {
                true => collect_frames(decoder, budget),
                false => still(bytes, budget),
            }
        }
        super::StillFormat::Png => {
            let mut decoder =
                image::codecs::png::PngDecoder::new(Cursor::new(bytes)).map_err(decode_error)?;
            decoder.set_limits(limits).map_err(decode_error)?;
            match decoder.is_apng().map_err(decode_error)? {
                true => collect_frames(decoder.apng().map_err(decode_error)?, budget),
                false => still(bytes, budget),
            }
        }
        _ => still(bytes, budget),
    }
}

fn still(bytes: &[u8], budget: AnimationBudget) -> Result<DecodedAnimation, MediaError> {
    let (width, height) = super::still::dimensions(bytes)?;
    if u64::from(width) * u64::from(height) * 4 > budget.max_bytes {
        return Err(MediaError::OverBudget);
    }
    let image = super::decode_bytes(bytes)?;
    if image.pixels.len() as u64 > budget.max_bytes {
        return Err(MediaError::OverBudget);
    }
    DecodedAnimation::new(
        vec![AnimationFrame {
            image,
            delay: budget.minimum_delay,
        }],
        AnimationLoops::Count(NonZeroU32::MIN),
    )
}

fn collect_frames<'a>(
    decoder: impl AnimationDecoder<'a>,
    budget: AnimationBudget,
) -> Result<DecodedAnimation, MediaError> {
    let loops = match decoder.loop_count() {
        image::metadata::LoopCount::Infinite => AnimationLoops::Infinite,
        image::metadata::LoopCount::Finite(count) => AnimationLoops::Count(count),
    };
    let mut bytes = 0u64;
    let mut frames = Vec::new();
    for frame in decoder.into_frames() {
        if frames.len() >= budget.max_frames {
            return Err(MediaError::OverBudget);
        }
        let frame = frame.map_err(decode_error)?;
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        let delay =
            Duration::from_nanos(u64::from(numerator) * 1_000_000 / u64::from(denominator.max(1)))
                .max(budget.minimum_delay);
        let buffer = frame.into_buffer();
        bytes = bytes
            .checked_add(buffer.as_raw().len() as u64)
            .ok_or(MediaError::OverBudget)?;
        if bytes > budget.max_bytes {
            return Err(MediaError::OverBudget);
        }
        let image = DecodedImage::from_rgba(buffer.width(), buffer.height(), buffer.into_raw())?;
        frames.push(AnimationFrame { image, delay });
    }
    DecodedAnimation::new(frames, loops)
}

fn decode_error(error: image::ImageError) -> MediaError {
    match error {
        image::ImageError::Limits(_) => MediaError::OverBudget,
        error => MediaError::Decode(super::truncate(error.to_string())),
    }
}
