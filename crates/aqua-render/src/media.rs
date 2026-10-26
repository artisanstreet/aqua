//! Still-image `Media` primitive: decode, bounded CPU/GPU caches, scene feed.
//!
//! Ownership: this module (media3d agent) owns the `Media` visual-primitive
//! pipeline for `aqua-render`. It does NOT own the scene vocabulary
//! (`DisplayItem::External`, owned by `aqua-core`), the composition itself
//! (owned by [`crate::Renderer`]), or text shaping.
//!
//! Decoding uses the maintained [`image`](https://crates.io/crates/image)
//! crate (0.25, workspace-agreed `png`+`bmp`+`jpeg`+`gif` codecs), never a
//! hand-rolled codec: malformed input, dimension limits, color-type coverage
//! (palette, gray, 16-bit, `tRNS`, interlaced PNG) and CVE-prone parsing all
//! stay inside the audited dependency. Unsupported formats are an explicit
//! [`MediaError::UnsupportedFormat`], never a silent placeholder.
//!
//! Conventions (binding, see `docs/GPU_EXTENSIONS.md`):
//! - CPU pixels are straight-alpha sRGB8 RGBA, row-major, no padding.
//! - GPU upload converts to *premultiplied* linear-light RGBA8
//!   (`Rgba8Unorm`, NOT an `Srgb` format) matching the renderer's linear
//!   working space. Premultiplication is REQUIRED, not a preference: the
//!   compose shader (`compose.wgsl`) blends premultiplied-over and returns
//!   `vec4(rgb * opacity, a * opacity)` — it never multiplies sampled RGB by
//!   sampled alpha. A straight-alpha upload would emit full-bright RGB at
//!   partial alpha (halos/fringes on every translucent texel).
//! - Linear RGBA8 can band on smooth gradients (8-bit linear steps are
//!   coarse in the darks). Quality-sensitive images use
//!   [`ImagePrecision::Float16`] (`Rgba16Float`, 8 B/px, honestly budgeted).
//! - Scene feed travels as `DisplayItem::External` with a `layer` key from
//!   [`media_layer`]; the caller registers the frame's [`wgpu::TextureView`]
//!   via [`crate::Renderer::register_external`] every frame it is shown.
//! - Scene feed travels as `DisplayItem::External` with a `layer` key from
//!   [`media_layer`]; the caller registers the frame's [`wgpu::TextureView`]
//!   via [`crate::Renderer::register_external`] every frame it is shown.
//! - Lifetimes are bounded: [`MediaCache`] caps decoded bytes, and
//!   [`GpuImageCache`] caps GPU bytes/textures with LRU eviction. Evicted or
//!   removed handles go stale and fail closed (generations, like `Scene`).

use std::collections::{HashMap, VecDeque};

use aqua_core::geometry::Rect;
use aqua_core::id::{ImageId, RawId};
use aqua_core::scene::DisplayItem;

/// Maximum decoded side length in pixels. Matches the renderer's
/// `MAX_TARGET_DIM` (8192) so a decoded image can always address its target.
pub const MAX_IMAGE_DIM: u32 = 8192;
/// Maximum decoded pixel count (64 megapixels = 256 MiB RGBA8). Checked from
/// the image header *before* the codec allocates the pixel buffer.
pub const MAX_IMAGE_PIXELS: u64 = 64 * 1024 * 1024;

/// Explicit failure modes. No silent fallbacks: callers and the inspector
/// see exactly why a frame is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaError {
    /// Zero-byte input.
    Empty,
    /// Bytes match no enabled decoder (checked before decoding).
    UnsupportedFormat,
    /// The codec rejected the bytes (truncated, corrupt CRC, bad headers).
    /// The message is truncated to 256 chars so a hostile file cannot flood logs.
    Decode(String),
    /// Header dimensions exceed [`MAX_IMAGE_DIM`] per side.
    DimensionsTooLarge(u32, u32),
    /// Header pixel count exceeds [`MAX_IMAGE_PIXELS`].
    TooManyPixels(u64),
    /// Admission would exceed the cache budget even after evicting everything.
    OverBudget,
    /// Unknown image handle.
    UnknownImage,
    /// Handle names a recycled slot (generation mismatch).
    StaleHandle,
    /// wgpu upload/usage failure.
    Gpu(String),
}

impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MediaError::Empty => write!(f, "media: empty input"),
            MediaError::UnsupportedFormat => {
                write!(
                    f,
                    "media: unsupported format (enabled: png, bmp, jpeg, gif)"
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

// ---------------------------------------------------------------------------
// Decoded image
// ---------------------------------------------------------------------------

/// A decoded still image: straight-alpha sRGB8 RGBA, row-major, no padding.
///
/// Normal form at the import boundary: every codec output (palette, gray,
/// gray+alpha, 16-bit, `tRNS` transparency, BMP bottom-up rows) is converted
/// here, so downstream code has exactly one layout to handle.
#[derive(Clone, Debug)]
pub struct DecodedImage {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl DecodedImage {
    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Straight-alpha sRGB8 RGBA bytes, row-major.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Pixel at `(x, y)` as `[r, g, b, a]`, or `None` when out of bounds.
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

    /// GPU upload form: *premultiplied* linear-light RGBA8 bytes, row-major.
    ///
    /// sRGB bytes convert through [`aqua_core::color::srgb8_to_linear`],
    /// multiply by alpha, and quantize to 8 bits per channel. Alpha stays in
    /// the fourth byte. Premultiplication is mandatory: `compose.wgsl`
    /// blends premultiplied-over and returns
    /// `vec4(rgb * opacity, a * opacity)`, so a straight-alpha upload would
    /// emit full-bright RGB at partial alpha (fringes on translucency).
    ///
    /// Precision note: 8-bit linear steps band on smooth dark gradients.
    /// For quality-sensitive images use [`ImagePrecision::Float16`] via
    /// [`GpuImageCache::upload_precise`].
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

    /// GPU upload form at half-float precision: premultiplied linear-light
    /// RGBA16F bytes (little-endian), row-major. Same math as
    /// [`DecodedImage::to_premultiplied_linear_rgba`] with ~10-bit mantissa
    /// precision instead of 8-bit linear quantization.
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

    /// CPU-side alpha check: premultiplied linear-light `[r, g, b, a]` floats.
    /// Matches the GPU upload math at full precision; the u8 upload path
    /// quantizes the same values.
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

// ---------------------------------------------------------------------------
// Decode (mature codec + header limits)
// ---------------------------------------------------------------------------

/// Decode any enabled format (png, bmp, jpeg, gif) into normal form.
///
/// Limits are enforced from the header *before* the pixel buffer is
/// allocated: oversized dimensions fail without a large allocation, and
/// malformed bytes fail as [`MediaError::Decode`], never a panic.
pub fn decode_bytes(bytes: &[u8]) -> Result<DecodedImage, MediaError> {
    if bytes.is_empty() {
        return Err(MediaError::Empty);
    }
    let format = image::guess_format(bytes).map_err(|_| MediaError::UnsupportedFormat)?;
    match format {
        image::ImageFormat::Png
        | image::ImageFormat::Bmp
        | image::ImageFormat::Jpeg
        | image::ImageFormat::Gif => {}
        _ => return Err(MediaError::UnsupportedFormat),
    }
    // Header-only dimensions first: no pixel allocation for hostile headers.
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

// ---------------------------------------------------------------------------
// CPU cache: bounded decoded store, generational handles
// ---------------------------------------------------------------------------

/// Budget for decoded (CPU) image bytes.
#[derive(Clone, Copy, Debug)]
pub struct MediaBudget {
    /// Hard cap on resident decoded bytes (4 bytes/px RGBA8).
    pub max_decoded_bytes: u64,
    /// Maximum live images (bounds handle-table growth).
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

/// Bounded CPU store for decoded images with generational [`ImageId`]
/// handles. Eviction is LRU across *all* entries (decoded bytes are
/// reproducible from source, so any entry may go); evicted and removed
/// handles go stale and fail closed on lookup.
#[derive(Debug, Default)]
pub struct MediaCache {
    budget: MediaBudget,
    slots: Vec<Slot>,
    free: Vec<(u32, u32)>,
    /// LRU of live slot indices, front = oldest.
    lru: VecDeque<u32>,
    next_evictions: u64,
}

impl MediaCache {
    /// Create a cache with an explicit budget.
    pub fn new(budget: MediaBudget) -> Self {
        Self {
            budget,
            ..Default::default()
        }
    }

    /// Number of live images.
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.alive).count()
    }

    /// Whether no images are stored.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Resident decoded bytes (live entries only; eviction frees immediately).
    pub fn decoded_bytes(&self) -> u64 {
        self.slots.iter().filter(|s| s.alive).map(|s| s.bytes).sum()
    }

    /// Evictions served so far (diagnostics/inspector).
    pub fn evictions(&self) -> u64 {
        self.next_evictions
    }

    /// Decode `bytes` and admit the image, evicting LRU entries as needed.
    pub fn decode_and_insert(&mut self, bytes: &[u8]) -> Result<ImageId, MediaError> {
        self.insert(decode_bytes(bytes)?)
    }

    /// Admit an already-decoded image.
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

    /// Look up a live image. Stale and unknown handles return `None`.
    pub fn get(&self, id: ImageId) -> Option<&DecodedImage> {
        let raw = id.raw();
        self.slots
            .get(raw.index as usize)
            .filter(|s| s.alive && s.generation == raw.generation && raw.is_valid())
            .map(|s| &s.image)
    }

    /// Remove an image. Stale handles fail closed (`false`); removal bumps
    /// the generation so the handle can never alias a future occupant.
    /// Slots at [`LAYER_MAX_GENERATION`] retire instead of recycling (see
    /// [`MediaCache::kill_slot`]).
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

    /// Mark a slot dead, advancing its generation for reuse — or retiring it
    /// permanently once it reaches [`LAYER_MAX_GENERATION`]. Retirement is
    /// the capacity contract: `media_layer` asserts generations stay within
    /// the bound and fails closed beyond it, so a slot is never recycled
    /// past the bound (stale handles can never alias a wrapped generation).
    /// Retired slots stay dead in the table and are never reissued.
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

/// Successor generation for slot reuse, or `None` when the slot must retire:
/// `current + 1` while it stays within [`LAYER_MAX_GENERATION`].
fn next_generation(current: u32) -> Option<u32> {
    (current < LAYER_MAX_GENERATION).then(|| current + 1)
}

// ---------------------------------------------------------------------------
// GPU cache: bounded texture store + layer namespace + scene feed
// ---------------------------------------------------------------------------

/// Budget for GPU-resident media textures.
#[derive(Clone, Copy, Debug)]
pub struct GpuMediaBudget {
    /// Hard cap on resident texture bytes.
    pub max_gpu_bytes: u64,
    /// Maximum pooled textures.
    pub max_textures: usize,
}

impl Default for GpuMediaBudget {
    fn default() -> Self {
        Self {
            max_gpu_bytes: 128 * 1024 * 1024,
            max_textures: 64,
        }
    }
}

/// A GPU-resident image: sampled by the renderer through `view`.
#[derive(Debug)]
pub struct UploadedImage {
    /// Sampled by `DisplayItem::External` via `register_external`.
    pub view: wgpu::TextureView,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Resident bytes (padded upload size).
    pub bytes: u64,
}

/// Layer namespace for `DisplayItem::External.layer`: canonical helpers live
/// in `aqua_core::scene` (single source of truth, contract §4) and are
/// re-exported here so existing `aqua_render::media::…` paths keep working.
/// Encoding is full 32-bit indices + 30-bit generations with a fail-closed
/// assert beyond [`LAYER_MAX_GENERATION`] — no silent truncation.
pub use aqua_core::scene::{
    decode_layer, media_layer, LayerKind, LAYER_MAX_GENERATION, LAYER_TAG_MASK, LAYER_TAG_MEDIA,
};

/// Build the scene item referencing a registered media frame.
pub fn external_item(layer: u64, bounds: Rect, opacity: f32, rounded_clip: f32) -> DisplayItem {
    DisplayItem::External {
        bounds,
        layer,
        opacity: opacity.clamp(0.0, 1.0),
        rounded_clip: rounded_clip.max(0.0),
    }
}

/// f32 -> IEEE 754 half-precision bits, round-to-nearest-even. Used for
/// the [`ImagePrecision::Float16`] upload path without adding a dependency
/// for a 20-line conversion. Subnormals flush toward zero only when the
/// value is below the smallest subnormal (handled by the shift logic);
/// infinities and NaN map to half inf / quiet NaN.
pub fn f32_to_f16_bits(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x7fffff;
    if exp == 255 {
        // Inf or NaN.
        return sign | 0x7c00 | if mant == 0 { 0 } else { 0x0200 };
    }
    let half_exp = exp - 127 + 15;
    if half_exp >= 31 {
        return sign | 0x7c00; // overflow -> inf
    }
    if half_exp <= 0 {
        // Subnormal or underflow: shift mantissa (with hidden 1) down.
        if half_exp < -10 {
            return sign; // too small even for subnormals
        }
        let m = mant | 0x800000;
        let shift = (1 - half_exp) as u32;
        // Round to nearest even.
        let rest = m & ((1 << shift) - 1);
        let half = 1 << (shift - 1);
        let mut out = m >> shift;
        if rest > half || (rest == half && (out & 1) == 1) {
            out += 1;
        }
        return sign | out as u16;
    }
    // Normalized: round mantissa 23 -> 10 bits, nearest even.
    let mut out = (mant >> 13) as u16;
    let rest = mant & 0x1fff;
    if rest > 0x1000 || (rest == 0x1000 && (out & 1) == 1) {
        out += 1;
        if out == 0x0400 {
            // Mantissa overflow carries into the exponent.
            return sign | (((half_exp + 1) as u16) << 10);
        }
    }
    sign | ((half_exp as u16) << 10) | (out & 0x03ff)
}

/// Upload precision: memory/precision trade-off for media textures.
///
/// Linear RGBA8 (4 B/px) matches the base format and is cheapest, but 8-bit
/// linear steps band on smooth dark gradients. RGBA16F (8 B/px, half float)
/// preserves sRGB precision through the linear working space for
/// quality-sensitive images (hero art, gradients); the byte budget accounts
/// for the real cost either way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImagePrecision {
    /// Premultiplied linear `Rgba8Unorm`. Default; bands on gradients.
    #[default]
    Linear8,
    /// Premultiplied linear `Rgba16Float`. 2x bytes, no banding.
    Float16,
}

impl ImagePrecision {
    /// wgpu format for this precision.
    pub fn format(self) -> wgpu::TextureFormat {
        match self {
            ImagePrecision::Linear8 => wgpu::TextureFormat::Rgba8Unorm,
            ImagePrecision::Float16 => wgpu::TextureFormat::Rgba16Float,
        }
    }

    /// Bytes per pixel.
    pub fn bytes_per_pixel(self) -> u32 {
        match self {
            ImagePrecision::Linear8 => 4,
            ImagePrecision::Float16 => 8,
        }
    }
}

/// Align helper: `write_texture` rows must be a multiple of 256 bytes.
fn align256(v: u32) -> u32 {
    v.div_ceil(256) * 256
}

/// Bounded GPU store for uploaded images, keyed by [`ImageId`] generations.
///
/// Upload converts to *premultiplied* linear light (see
/// [`DecodedImage::to_premultiplied_linear_rgba`]): the compose shader
/// blends premultiplied-over and never multiplies sampled RGB by sampled
/// alpha, so straight-alpha bytes would fringe. Eviction is LRU and frees
/// the texture immediately; re-showing an evicted image requires re-upload
/// from the caller-retained [`DecodedImage`] (GPU bytes are not
/// reconstructible here). Handles from an evicted generation fail closed
/// via the `layer` key: the caller must register the fresh view under the
/// fresh key.
#[derive(Debug, Default)]
pub struct GpuImageCache {
    budget: GpuMediaBudget,
    textures: HashMap<(u32, u32), UploadedImage>,
    lru: VecDeque<(u32, u32)>,
    /// Evictions served so far.
    pub evictions: u64,
}

impl GpuImageCache {
    /// Create a cache with an explicit budget.
    pub fn new(budget: GpuMediaBudget) -> Self {
        Self {
            budget,
            ..Default::default()
        }
    }

    /// Resident GPU bytes.
    pub fn resident_bytes(&self) -> u64 {
        self.textures.values().map(|t| t.bytes).sum()
    }

    /// Number of resident textures.
    pub fn len(&self) -> usize {
        self.textures.len()
    }

    /// Whether no textures are resident.
    pub fn is_empty(&self) -> bool {
        self.textures.is_empty()
    }

    /// Look up the resident view for an image handle.
    pub fn view(&self, id: ImageId) -> Option<&UploadedImage> {
        let raw = id.raw();
        self.textures.get(&(raw.index, raw.generation))
    }

    /// Upload (or replace) an image at default [`ImagePrecision::Linear8`].
    /// The `id` generation tags the entry, so a recycled [`ImageId`] never
    /// aliases a stale texture.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: ImageId,
        image: &DecodedImage,
    ) -> Result<&UploadedImage, MediaError> {
        self.upload_precise(device, queue, id, image, ImagePrecision::Linear8)
    }

    /// Upload at an explicit precision. `Float16` doubles the budgeted bytes
    /// per pixel and removes linear-8 banding on gradients.
    pub fn upload_precise(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: ImageId,
        image: &DecodedImage,
        precision: ImagePrecision,
    ) -> Result<&UploadedImage, MediaError> {
        let raw = id.raw();
        if !raw.is_valid() {
            return Err(MediaError::StaleHandle);
        }
        let (w, h) = (image.width.max(1), image.height.max(1));
        let bpp = precision.bytes_per_pixel();
        let pixels: Vec<u8> = match precision {
            ImagePrecision::Linear8 => image.to_premultiplied_linear_rgba(),
            ImagePrecision::Float16 => image.to_premultiplied_linear_f16(),
        };
        let stride = align256(w * bpp);
        let bytes = stride as u64 * h as u64;
        if bytes > self.budget.max_gpu_bytes {
            return Err(MediaError::OverBudget);
        }
        // Pad rows to the 256-byte `write_texture` alignment.
        let mut padded = vec![0u8; bytes as usize];
        let row = w as usize * bpp as usize;
        for (y, dest) in padded
            .chunks_mut(stride as usize)
            .enumerate()
            .take(h as usize)
        {
            dest[..row].copy_from_slice(&pixels[y * row..(y + 1) * row]);
        }
        while self.resident_bytes() + bytes > self.budget.max_gpu_bytes
            || self.textures.len() >= self.budget.max_textures
        {
            if self.evict_oldest_for(bytes).is_none() {
                return Err(MediaError::OverBudget);
            }
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("aqua-media"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Linear working space, PREMULTIPLIED alpha (see module docs).
            format: precision.format(),
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &padded,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let key = (raw.index, raw.generation);
        self.lru.retain(|&k| k != key);
        self.lru.push_back(key);
        self.textures.insert(
            key,
            UploadedImage {
                view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
                width: w,
                height: h,
                bytes,
            },
        );
        Ok(self.textures.get(&key).expect("just inserted"))
    }

    /// Drop the resident texture for `id`. Unknown/stale ids return `false`.
    pub fn release(&mut self, id: ImageId) -> bool {
        let raw = id.raw();
        let key = (raw.index, raw.generation);
        if self.textures.remove(&key).is_some() {
            self.lru.retain(|&k| k != key);
            true
        } else {
            false
        }
    }

    /// Evict the oldest entry that still leaves room, or `None` when every
    /// resident entry is needed to hold `need` (live entries are never
    /// partially evicted here; all entries are evictable, so `None` means
    /// the single texture exceeds the whole budget, checked by the caller).
    fn evict_oldest_for(&mut self, _need: u64) -> Option<()> {
        let key = self.lru.pop_front()?;
        self.textures.remove(&key)?;
        self.evictions += 1;
        Some(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> Vec<u8> {
        // `cargo test` runs with CWD = the package dir; anchor on the
        // manifest dir so the path is CWD-independent.
        let path = format!(
            "{}/../aqua/examples/assets/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read(&path).unwrap_or_else(|e| panic!("fixture {path}: {e}"))
    }

    #[test]
    fn rgba_png_decodes_exact_pixels() {
        let img = decode_bytes(&asset("sample_rgba.png")).expect("decode");
        assert_eq!((img.width(), img.height()), (4, 3));
        assert_eq!(img.pixel_at(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(img.pixel_at(3, 0), Some([255, 255, 255, 255]));
        assert_eq!(img.pixel_at(0, 1), Some([255, 0, 0, 128]));
        assert_eq!(img.pixel_at(3, 1), Some([0, 0, 0, 0]));
        assert_eq!(img.pixel_at(3, 2), Some([128, 128, 128, 200]));
        assert_eq!(img.pixel_at(4, 0), None);
    }

    #[test]
    fn gray_trns_palette_and_16bit_normalize() {
        // Gray + tRNS: value 0 fully transparent, others opaque.
        let g = decode_bytes(&asset("sample_gray_trns.png")).expect("gray");
        assert_eq!((g.width(), g.height()), (2, 2));
        assert_eq!(g.pixel_at(0, 0), Some([0, 0, 0, 0]));
        assert_eq!(g.pixel_at(1, 0), Some([255, 255, 255, 255]));
        assert_eq!(g.pixel_at(0, 1), Some([128, 128, 128, 255]));
        // Palette + tRNS: index 3 carries alpha 64.
        let p = decode_bytes(&asset("sample_palette.png")).expect("palette");
        assert_eq!(p.pixel_at(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(p.pixel_at(1, 0), Some([0, 255, 0, 255]));
        assert_eq!(p.pixel_at(0, 1), Some([0, 0, 255, 255]));
        assert_eq!(p.pixel_at(1, 1), Some([255, 255, 255, 64]));
        // 16-bit RGB downscales to 8-bit primaries.
        let s = decode_bytes(&asset("sample_16bit.png")).expect("16-bit");
        assert_eq!(s.pixel_at(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(s.pixel_at(1, 0), Some([0, 255, 0, 255]));
        assert_eq!(s.pixel_at(0, 1), Some([0, 0, 255, 255]));
        assert_eq!(s.pixel_at(1, 1), Some([255, 255, 255, 255]));
        // Fixed-Huffman and interlaced PNGs decode through the mature codec.
        for name in ["sample_fixed.png", "sample_interlaced.png"] {
            let img = decode_bytes(&asset(name)).expect(name);
            assert_eq!((img.width(), img.height()), (4, 3), "{name}");
            assert_eq!(img.pixel_at(0, 0), Some([255, 0, 0, 255]), "{name}");
        }
    }

    #[test]
    fn bmp_24_and_32_decode_with_alpha() {
        let b24 = decode_bytes(&asset("sample_rgb24.bmp")).expect("bmp24");
        assert_eq!((b24.width(), b24.height()), (3, 2));
        // BMP rows are bottom-up: y=1 is the file's first row (blue,green,red).
        assert_eq!(b24.pixel_at(0, 1), Some([0, 0, 255, 255]));
        assert_eq!(b24.pixel_at(1, 1), Some([0, 255, 0, 255]));
        assert_eq!(b24.pixel_at(0, 0), Some([255, 255, 255, 255]));
        assert_eq!(b24.pixel_at(1, 0), Some([0, 0, 0, 255]));
        let b32 = decode_bytes(&asset("sample_rgba32.bmp")).expect("bmp32");
        assert_eq!((b32.width(), b32.height()), (2, 2));
        // BI_RGB 32-bit has NO alpha channel (4th byte is padding): the
        // mature codec is spec-correct to report opaque. Real BMP alpha
        // needs BITFIELDS masks, which stay out of the enabled set.
        assert_eq!(b32.pixel_at(0, 1), Some([255, 0, 0, 255]));
        assert_eq!(b32.pixel_at(1, 1), Some([255, 255, 255, 255]));
        assert_eq!(b32.pixel_at(0, 0), Some([0, 0, 255, 255]));
        assert_eq!(b32.pixel_at(1, 0), Some([0, 255, 0, 255]));
    }

    #[test]
    fn malformed_inputs_fail_explicitly() {
        assert!(matches!(decode_bytes(&[]), Err(MediaError::Empty)));
        assert!(matches!(
            decode_bytes(b"definitely not an image"),
            Err(MediaError::UnsupportedFormat)
        ));
        // Valid signature, truncated body.
        let mut png = asset("sample_rgba.png");
        png.truncate(40);
        assert!(matches!(decode_bytes(&png), Err(MediaError::Decode(_))));
        // Random corruption inside the IDAT payload fails, never panics
        // (byte 60 sits in compressed data: flipping it breaks the zlib
        // stream and the trailing Adler32 check).
        let mut corrupt = asset("sample_rgba.png");
        corrupt[60] ^= 0xff;
        corrupt[61] ^= 0xaa;
        assert!(matches!(decode_bytes(&corrupt), Err(MediaError::Decode(_))));
        // Hostile header: claims 20000x20000 without allocating it.
        let mut huge = asset("sample_rgba.png");
        huge[16..20].copy_from_slice(&20000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&20000u32.to_be_bytes());
        assert!(matches!(
            decode_bytes(&huge),
            Err(MediaError::DimensionsTooLarge(20000, 20000)) | Err(MediaError::Decode(_))
        ));
    }

    #[test]
    fn upload_bytes_are_premultiplied_linear() {
        let img = decode_bytes(&asset("sample_rgba.png")).expect("decode");
        let lin = img.to_premultiplied_linear_rgba();
        assert_eq!(lin.len(), img.pixels().len());
        // Opaque red: linear red 255, alpha untouched.
        assert_eq!(&lin[0..4], &[255, 0, 0, 255]);
        // Half red: rgb scaled by alpha (255*0.5 ~ 128), alpha byte kept.
        // compose.wgsl multiplies by opacity only, so straight bytes here
        // would emit full-bright red at half alpha (fringe). Premultiplied
        // bytes compose exactly.
        assert_eq!(&lin[16..20], &[128, 0, 0, 128]);
        // Transparent black stays [0,0,0,0]: no hidden RGB.
        assert_eq!(&lin[28..32], &[0, 0, 0, 0]);
        // A fully transparent COLORED texel carries no RGB either.
        let ghost = DecodedImage {
            width: 1,
            height: 1,
            pixels: vec![255, 0, 0, 0],
        };
        assert_eq!(ghost.to_premultiplied_linear_rgba(), &[0, 0, 0, 0]);
        // f32 form matches the u8 path at full precision.
        let pm = img.premultiplied_linear_f32();
        let px = pm[4];
        assert!((px[0] - 0.5).abs() < 0.01 && (px[3] - 128.0 / 255.0).abs() < 0.01);
    }

    #[test]
    fn f16_conversion_covers_range() {
        assert_eq!(f32_to_f16_bits(1.0), 0x3c00);
        assert_eq!(f32_to_f16_bits(0.5), 0x3800);
        assert_eq!(f32_to_f16_bits(0.0), 0x0000);
        assert_eq!(f32_to_f16_bits(-0.0) & 0x8000, 0x8000);
        assert_eq!(f32_to_f16_bits(f32::INFINITY), 0x7c00);
        assert!(f32_to_f16_bits(f32::NAN) & 0x7c00 == 0x7c00);
        // Round-trip spot checks through the half range.
        for v in [0.25f32, 0.1, 2.0, 100.0, 0.001] {
            let back = f16_bits_to_f32_check(v);
            assert!((back - v).abs() / v.max(1e-6) < 0.002, "{v} -> {back}");
        }
        // Premultiplied f16 upload of half red: rgb halves, alpha kept.
        let half_red = DecodedImage {
            width: 1,
            height: 1,
            pixels: vec![255, 0, 0, 128],
        };
        let bytes = half_red.to_premultiplied_linear_f16();
        assert_eq!(bytes.len(), 8);
        let r = u16::from_le_bytes([bytes[0], bytes[1]]);
        let a = u16::from_le_bytes([bytes[6], bytes[7]]);
        // Red channel = linear(1.0) * alpha, i.e. exactly the alpha value.
        assert_eq!(r, f32_to_f16_bits(128.0 / 255.0), "half red channel");
        assert_eq!(a, f32_to_f16_bits(128.0 / 255.0), "half alpha");
    }

    /// Test-only f16 decode for round-trip checks (mirrors the encode grid).
    fn f16_bits_to_f32_check(v: f32) -> f32 {
        let h = f32_to_f16_bits(v);
        let sign = ((h & 0x8000) as u32) << 16;
        let exp = ((h >> 10) & 0x1f) as u32;
        let mant = (h & 0x03ff) as u32;
        let bits = if exp == 0 {
            sign | (mant << 13) // subnormal detail below f16 grid; encode rounds the same way
        } else if exp == 31 {
            sign | 0x7f80_0000 | (mant << 13)
        } else {
            sign | ((exp + 112) << 23) | (mant << 13)
        };
        f32::from_bits(bits)
    }

    #[test]
    fn cache_lru_stale_and_budgets() {
        let mut cache = MediaCache::new(MediaBudget {
            max_decoded_bytes: 4 * 4 * 4 * 2, // two 4x4 RGBA images
            max_images: 8,
        });
        let mk = |r: u8| DecodedImage {
            width: 4,
            height: 4,
            pixels: vec![r; 64],
        };
        let a = cache.insert(mk(1)).unwrap();
        let b = cache.insert(mk(2)).unwrap();
        assert_eq!(cache.decoded_bytes(), 128);
        // Third insert evicts the oldest (a goes stale, b survives).
        let _c = cache.insert(mk(3)).unwrap();
        assert_eq!(cache.evictions(), 1);
        assert!(cache.get(a).is_none());
        assert_eq!(cache.get(b).unwrap().pixels()[0], 2);
        // Single image larger than the whole budget is refused, not half-kept.
        assert_eq!(
            cache
                .insert(DecodedImage {
                    width: 16,
                    height: 16,
                    pixels: vec![0; 1024],
                })
                .unwrap_err(),
            MediaError::OverBudget
        );
        // Removal is stale-fails-closed; double remove fails.
        assert!(cache.remove(b));
        assert!(!cache.remove(b));
        assert!(cache.get(b).is_none());
    }

    #[test]
    fn layer_namespace_and_external_item() {
        let mut cache = MediaCache::new(MediaBudget::default());
        let id = cache
            .insert(DecodedImage {
                width: 2,
                height: 2,
                pixels: vec![9; 16],
            })
            .unwrap();
        let layer = media_layer(id);
        assert_eq!(layer & LAYER_TAG_MEDIA, LAYER_TAG_MEDIA);
        assert_eq!(layer & crate::viewport::LAYER_TAG_VIEWPORT, 0);
        // Canonical decode round-trips the handle coordinates.
        let (kind, back) = decode_layer(layer).expect("known tag decodes");
        assert_eq!(kind, LayerKind::Media);
        assert_eq!(back, id.raw());
        assert_eq!(decode_layer(0x0000_0000_0000_0001), None);
        let item = external_item(
            layer,
            Rect {
                x: 1.0,
                y: 2.0,
                w: 3.0,
                h: 4.0,
            },
            0.5,
            2.0,
        );
        match item {
            DisplayItem::External {
                bounds,
                layer: l,
                opacity,
                rounded_clip,
            } => {
                assert_eq!((bounds.w, bounds.h), (3.0, 4.0));
                assert_eq!(l, layer);
                assert_eq!(opacity, 0.5);
                assert_eq!(rounded_clip, 2.0);
            }
            _ => panic!("expected External"),
        }
    }

    #[test]
    fn layer_roundtrips_max_generation_without_truncation() {
        // Full 32-bit index + 30-bit generation survive the layer key:
        // no 24-bit truncation anywhere in the path.
        let raw = RawId {
            index: u32::MAX,
            generation: LAYER_MAX_GENERATION,
        };
        let layer = media_layer(ImageId::from_raw(raw));
        let (kind, back) = decode_layer(layer).expect("max coords decode");
        assert_eq!(kind, LayerKind::Media);
        assert_eq!(back, raw);
        let vlayer = crate::viewport::viewport_layer(aqua_core::ViewportId::from_raw(raw));
        let (vkind, vback) = decode_layer(vlayer).expect("max coords decode");
        assert_eq!(vkind, LayerKind::Viewport);
        assert_eq!(vback, raw);
    }

    #[test]
    #[should_panic(expected = "overflowed layer capacity")]
    fn layer_fails_closed_above_max_generation() {
        let _ = media_layer(ImageId::from_raw(RawId {
            index: 0,
            generation: LAYER_MAX_GENERATION + 1,
        }));
    }

    #[test]
    fn generation_retires_at_bound_never_wraps() {
        assert_eq!(next_generation(0), Some(1));
        assert_eq!(
            next_generation(LAYER_MAX_GENERATION - 1),
            Some(LAYER_MAX_GENERATION)
        );
        assert_eq!(next_generation(LAYER_MAX_GENERATION), None);
        assert_eq!(next_generation(u32::MAX), None);
    }

    // -- GPU-backed tests: strict skip contract. ONLY a genuine
    // `RequestAdapterError::NotFound` (no usable adapter at all) skips.
    // Device/request failures, pipeline errors, and validation errors FAIL.
    // Set AQUA_REQUIRE_GPU=1 to turn a skip into a hard failure (CI proves
    // lavapipe coverage with `--nocapture` showing every test ran).

    /// Headless device, or `None` ONLY when no adapter exists. Any other
    /// failure panics: it must never be mislabeled "no adapter".
    fn headless_device(caller: &str) -> Option<(wgpu::Device, wgpu::Queue)> {
        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_desc.backends = wgpu::Backends::all();
        let instance = wgpu::Instance::new(instance_desc);
        let adapter =
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })) {
                Ok(adapter) => adapter,
                Err(wgpu::RequestAdapterError::NotFound { .. }) => {
                    if std::env::var_os("AQUA_REQUIRE_GPU").is_some() {
                        panic!("AQUA_REQUIRE_GPU=1 but no adapter for {caller}");
                    }
                    eprintln!("SKIP {caller} (genuine NotFound: no adapter)");
                    return None;
                }
                Err(e) => panic!("{caller}: adapter request failed, must not skip: {e:?}"),
            };
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .unwrap_or_else(|e| {
                    panic!("{caller}: device request failed, must not skip: {e:?}")
                });
        Some((device, queue))
    }

    #[test]
    fn gpu_upload_and_external_draw_roundtrip() {
        let Some((device, queue)) = headless_device("gpu_upload_and_external_draw_roundtrip")
        else {
            return;
        };
        let mut cache = MediaCache::new(MediaBudget::default());
        let img = decode_bytes(&asset("sample_rgba.png")).expect("decode");
        let id = cache.insert(img).unwrap();
        let mut gpu = GpuImageCache::new(GpuMediaBudget::default());
        let layer = media_layer(id);
        let view = gpu
            .upload(&device, &queue, id, cache.get(id).unwrap())
            .expect("upload")
            .view
            .clone();
        assert_eq!((gpu.len(), gpu.evictions), (1, 0));
        assert!(gpu.resident_bytes() > 0);

        // Feed the frame through the real renderer contract.
        let mut renderer = crate::renderer::Renderer::new(
            device.clone(),
            queue.clone(),
            wgpu::TextureFormat::Rgba8Unorm,
        );
        renderer.set_size(64, 64, 1.0).expect("size");
        let mut scene = crate::scene::Scene::new();
        scene.insert(external_item(
            layer,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 64.0,
                h: 64.0,
            },
            1.0,
            0.0,
        ));
        renderer.register_external(layer, view, 4, 3);
        let plan = renderer.prepare(&mut scene);
        renderer.draw(&plan).expect("draw");
        let px = renderer.readback().expect("readback");
        // Opaque-red texel lands near the top-left, converted to linear.
        let c = &px[(8 * 64 + 8) * 4..][..4];
        assert!(c[0] > 200 && c[1] < 30 && c[3] == 255, "drawn texel: {c:?}");
    }

    #[test]
    fn gpu_half_alpha_over_blue_composes_linear() {
        let Some((device, queue)) = headless_device("gpu_half_alpha_over_blue_composes_linear")
        else {
            return;
        };
        // Opaque blue base + full-screen half-red image on top.
        // Premultiplied-over in linear light:
        //   rgb = [0.5,0,0] + [0,0,1]*0.5 = [0.5,0,0.5], a = 1.
        // A straight-alpha upload would emit [1,0,0.5] (too bright): this
        // test pins the premultiplied contract against real composition.
        let mut renderer = crate::renderer::Renderer::new(
            device.clone(),
            queue.clone(),
            wgpu::TextureFormat::Rgba8Unorm,
        );
        renderer.set_size(64, 64, 1.0).expect("size");
        let blue = aqua_core::Oklch::from_hex("#0000ff").unwrap();
        let mut scene = crate::scene::Scene::new();
        scene.insert(DisplayItem::Rect {
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                w: 64.0,
                h: 64.0,
            },
            radius: 0.0,
            fill: blue,
            border_width: 0.0,
            border_color: blue,
            shadow: None,
        });
        let mut cache = MediaCache::new(MediaBudget::default());
        let half_red = DecodedImage {
            width: 4,
            height: 4,
            pixels: [[255u8, 0, 0, 128]; 16].concat(),
        };
        let id = cache.insert(half_red).unwrap();
        let mut gpu = GpuImageCache::new(GpuMediaBudget::default());
        let layer = media_layer(id);
        let view = gpu
            .upload(&device, &queue, id, cache.get(id).unwrap())
            .expect("upload")
            .view
            .clone();
        scene.insert(external_item(
            layer,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 64.0,
                h: 64.0,
            },
            1.0,
            0.0,
        ));
        renderer.register_external(layer, view, 4, 4);
        let plan = renderer.prepare(&mut scene);
        renderer.draw(&plan).expect("draw");
        let px = renderer.readback().expect("readback");
        let c = &px[(32 * 64 + 32) * 4..][..4];
        assert!(
            c[0].abs_diff(128) <= 2 && c[1] <= 2 && c[2].abs_diff(128) <= 2 && c[3] == 255,
            "half red over blue must be [~128,0,~128,255], got {c:?}"
        );
    }

    #[test]
    fn gpu_transparent_texel_contributes_no_rgb() {
        let Some((device, queue)) = headless_device("gpu_transparent_texel_contributes_no_rgb")
        else {
            return;
        };
        // Fully transparent (even colored) texels must not shift the base:
        // premultiplied upload zeroes their RGB; blending adds nothing.
        let mut renderer = crate::renderer::Renderer::new(
            device.clone(),
            queue.clone(),
            wgpu::TextureFormat::Rgba8Unorm,
        );
        renderer.set_size(32, 32, 1.0).expect("size");
        let blue = aqua_core::Oklch::from_hex("#0000ff").unwrap();
        let mut scene = crate::scene::Scene::new();
        scene.insert(DisplayItem::Rect {
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                w: 32.0,
                h: 32.0,
            },
            radius: 0.0,
            fill: blue,
            border_width: 0.0,
            border_color: blue,
            shadow: None,
        });
        let mut cache = MediaCache::new(MediaBudget::default());
        let ghost = DecodedImage {
            width: 2,
            height: 2,
            pixels: [[255u8, 0, 0, 0]; 4].concat(),
        };
        let id = cache.insert(ghost).unwrap();
        let mut gpu = GpuImageCache::new(GpuMediaBudget::default());
        let layer = media_layer(id);
        let view = gpu
            .upload(&device, &queue, id, cache.get(id).unwrap())
            .expect("upload")
            .view
            .clone();
        scene.insert(external_item(
            layer,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 32.0,
                h: 32.0,
            },
            1.0,
            0.0,
        ));
        renderer.register_external(layer, view, 2, 2);
        let plan = renderer.prepare(&mut scene);
        renderer.draw(&plan).expect("draw");
        let px = renderer.readback().expect("readback");
        let c = &px[(16 * 32 + 16) * 4..][..4];
        assert!(
            c[0] <= 2 && c[1] <= 2 && c[2] >= 253 && c[3] == 255,
            "transparent texel must leave blue intact, got {c:?}"
        );
    }

    #[test]
    fn gpu_cache_evicts_lru_under_budget() {
        let Some((device, queue)) = headless_device("gpu_cache_evicts_lru_under_budget") else {
            return;
        };
        let mut gpu = GpuImageCache::new(GpuMediaBudget {
            // Each 4x1 upload pads to one 256-byte row: 700 B holds two,
            // so the third upload must evict.
            max_gpu_bytes: 700,
            max_textures: 8,
        });
        let mk = |r: u8| DecodedImage {
            width: 4,
            height: 1,
            pixels: [[r, 0, 0, 255]; 4].concat(),
        };
        let mut cache = MediaCache::new(MediaBudget::default());
        let ids: Vec<ImageId> = (0..4u8).map(|r| cache.insert(mk(r)).unwrap()).collect();
        for id in &ids {
            gpu.upload(&device, &queue, *id, cache.get(*id).unwrap())
                .expect("upload");
        }
        assert!(gpu.evictions >= 1, "LRU must have evicted");
        assert!(gpu.resident_bytes() <= 700);
        // Evicted entries are gone; the newest survives.
        assert!(gpu.view(ids[3]).is_some());
    }

    #[test]
    fn gpu_release_unknown_is_false() {
        let mut gpu = GpuImageCache::new(GpuMediaBudget::default());
        let fake = ImageId::from_raw(RawId {
            index: 7,
            generation: 0,
        });
        assert!(gpu.view(fake).is_none());
        assert!(!gpu.release(fake));
    }
}
