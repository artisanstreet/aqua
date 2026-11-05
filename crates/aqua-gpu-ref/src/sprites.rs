use std::collections::HashMap;

use aqua_core::geometry::Rect;
use aqua_graph::slug::{self, texel_position};
use aqua_graph::tone::{ToneCurve, premultiplied_luminance};
use aqua_graph::{
    Extent, GlyphInstance, ImageInstance, SlugInstance, TextureFormat, TextureHandle, TextureRef,
};

use crate::raster::{Layer, Pixel, Placement, over};
use crate::sample::blend;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefTexture {
    pub format: TextureFormat,
    pub extent: Extent,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureError {
    UnknownHandle,
    UnsupportedFormat,
    OutOfBounds,
    DataSize,
}

impl RefTexture {
    pub fn new(extent: Extent, format: TextureFormat) -> Result<Self, TextureError> {
        match format {
            TextureFormat::R8Unorm | TextureFormat::Rgba8Unorm | TextureFormat::Rgba32Float => {
                Ok(Self {
                    format,
                    extent,
                    data: vec![0; extent.texels() as usize * format.bytes_per_texel() as usize],
                })
            }
            _ => Err(TextureError::UnsupportedFormat),
        }
    }

    pub fn write(
        &mut self,
        x: u32,
        y: u32,
        extent: Extent,
        data: &[u8],
    ) -> Result<(), TextureError> {
        let bytes = self.format.bytes_per_texel() as usize;
        if x.checked_add(extent.width)
            .is_none_or(|end| end > self.extent.width)
            || y.checked_add(extent.height)
                .is_none_or(|end| end > self.extent.height)
        {
            return Err(TextureError::OutOfBounds);
        }
        if data.len() != extent.texels() as usize * bytes {
            return Err(TextureError::DataSize);
        }
        let row_bytes = extent.width as usize * bytes;
        for row in 0..extent.height as usize {
            let destination =
                ((y as usize + row) * self.extent.width as usize + x as usize) * bytes;
            self.data[destination..destination + row_bytes]
                .copy_from_slice(&data[row * row_bytes..(row + 1) * row_bytes]);
        }
        Ok(())
    }

    fn texel(&self, x: i64, y: i64) -> Pixel {
        let bytes = self.format.bytes_per_texel() as usize;
        let index = (y as usize * self.extent.width as usize + x as usize) * bytes;
        let channel = |offset: usize| self.data[index + offset] as f32 / 255.0;
        match self.format {
            TextureFormat::R8Unorm => [channel(0), 0.0, 0.0, 0.0],
            _ => [channel(0), channel(1), channel(2), channel(3)],
        }
    }

    pub fn float_texel(&self, index: u32) -> [f32; 4] {
        let (x, y) = texel_position(index);
        let offset = (y as usize * self.extent.width as usize + x as usize) * 16;
        match self.data.get(offset..offset + 16) {
            Some(bytes) => std::array::from_fn(|lane| {
                f32::from_le_bytes([
                    bytes[lane * 4],
                    bytes[lane * 4 + 1],
                    bytes[lane * 4 + 2],
                    bytes[lane * 4 + 3],
                ])
            }),
            None => [0.0; 4],
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Textures {
    textures: HashMap<TextureHandle, RefTexture>,
}

impl Textures {
    pub fn create(
        &mut self,
        handle: TextureHandle,
        extent: Extent,
        format: TextureFormat,
    ) -> Result<(), TextureError> {
        self.textures
            .insert(handle, RefTexture::new(extent, format)?);
        Ok(())
    }

    pub fn write(
        &mut self,
        handle: TextureHandle,
        x: u32,
        y: u32,
        extent: Extent,
        data: &[u8],
    ) -> Result<(), TextureError> {
        self.textures
            .get_mut(&handle)
            .ok_or(TextureError::UnknownHandle)?
            .write(x, y, extent, data)
    }

    pub fn release(&mut self, handle: TextureHandle) -> bool {
        self.textures.remove(&handle).is_some()
    }

    pub fn get(&self, handle: TextureHandle) -> Option<&RefTexture> {
        self.textures.get(&handle)
    }

    pub fn len(&self) -> usize {
        self.textures.len()
    }

    pub fn is_empty(&self) -> bool {
        self.textures.is_empty()
    }

    pub fn resident_bytes(&self) -> u64 {
        self.textures
            .values()
            .map(|texture| texture.data.len() as u64)
            .sum()
    }
}

const NO_CLIP: Rect = Rect {
    x: -1.0e5,
    y: -1.0e5,
    w: 2.0e5,
    h: 2.0e5,
};

fn rect_distance(px: f32, py: f32, bounds: Rect, radius: f32) -> f32 {
    let half = (bounds.w * 0.5, bounds.h * 0.5);
    let relative = (px - bounds.x - half.0, py - bounds.y - half.1);
    let q = (
        relative.0.abs() - half.0 + radius,
        relative.1.abs() - half.1 + radius,
    );
    match radius == 0.0 {
        true => q.0.max(q.1),
        false => {
            (q.0.max(0.0).powi(2) + q.1.max(0.0).powi(2)).sqrt() + q.0.max(q.1).min(0.0) - radius
        }
    }
}

fn clip_cover(clip: Rect, px: f32, py: f32) -> f32 {
    (0.5 - rect_distance(px, py, clip, 0.0)).clamp(0.0, 1.0)
}

fn pixel_span(low: f32, high: f32, limit: usize) -> std::ops::Range<usize> {
    let start = (low - 0.5).ceil().max(0.0) as usize;
    let end = ((high - 0.5).ceil().max(0.0) as usize).min(limit);
    start.min(end)..end
}

pub fn draw_glyphs(
    layer: &mut Layer,
    atlas: &RefTexture,
    instances: &[GlyphInstance],
    tone: ToneCurve,
) {
    for glyph in instances {
        let clip = glyph.clip.unwrap_or(NO_CLIP);
        let [r, g, b, a] = glyph.color.to_gpu();
        let color = [r, g, b, a].map(|channel| channel * glyph.opacity);
        let luminance = premultiplied_luminance(color);
        let scale = (
            glyph.texels.w / glyph.bounds.w.max(1.0e-4),
            glyph.texels.h / glyph.bounds.h.max(1.0e-4),
        );
        for y in pixel_span(glyph.bounds.y, glyph.bounds.bottom(), layer.height) {
            for x in pixel_span(glyph.bounds.x, glyph.bounds.right(), layer.width) {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let local_x = ((px - glyph.bounds.x) * scale.0)
                    .floor()
                    .clamp(0.0, glyph.texels.w - 1.0);
                let local_y = ((py - glyph.bounds.y) * scale.1)
                    .floor()
                    .clamp(0.0, glyph.texels.h - 1.0);
                let coverage = atlas.texel(
                    (glyph.texels.x + local_x) as i64,
                    (glyph.texels.y + local_y) as i64,
                )[0];
                let weight = tone.alpha(coverage, luminance) * clip_cover(clip, px, py);
                let source = color.map(|channel| channel * weight);
                let index = y * layer.width + x;
                layer.pixels[index] = over(source, layer.pixels[index]);
            }
        }
    }
}

fn fetch(texture: &RefTexture, region: Rect, x: f32, y: f32) -> Pixel {
    let clamped_x = x.clamp(region.x, region.x + region.w - 1.0);
    let clamped_y = y.clamp(region.y, region.y + region.h - 1.0);
    texture.texel(clamped_x as i64, clamped_y as i64)
}

fn lerp(a: Pixel, b: Pixel, t: f32) -> Pixel {
    std::array::from_fn(|channel| a[channel] + (b[channel] - a[channel]) * t)
}

pub fn sample_image(texture: &RefTexture, region: Rect, u: f32, v: f32) -> Pixel {
    let position = (region.x + u * region.w - 0.5, region.y + v * region.h - 0.5);
    let base = (position.0.floor(), position.1.floor());
    let f = (position.0 - base.0, position.1 - base.1);
    let a = fetch(texture, region, base.0, base.1);
    let b = fetch(texture, region, base.0 + 1.0, base.1);
    let c = fetch(texture, region, base.0, base.1 + 1.0);
    let d = fetch(texture, region, base.0 + 1.0, base.1 + 1.0);
    lerp(lerp(a, b, f.0), lerp(c, d, f.0), f.1)
}

pub fn draw_images(layer: &mut Layer, textures: &Textures, instances: &[ImageInstance]) {
    for image in instances {
        let TextureRef::Persistent(handle) = image.source else {
            continue;
        };
        let Some(texture) = textures.get(handle) else {
            continue;
        };
        let clip = image.clip.unwrap_or(NO_CLIP);
        let clip_radius = image.clip.map_or(0.0, |_| image.clip_radius);
        let covered = image.covered();
        let placement = Placement::new(image.bounds, image.linear);
        let low = (
            (covered.x - 1.0).max(clip.x - 1.0),
            (covered.y - 1.0).max(clip.y - 1.0),
        );
        let high = (
            (covered.right() + 1.0).min(clip.right() + 1.0).max(low.0),
            (covered.bottom() + 1.0).min(clip.bottom() + 1.0).max(low.1),
        );
        for y in pixel_span(low.1, high.1, layer.height) {
            for x in pixel_span(low.0, high.0, layer.width) {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let (lx, ly) = placement.local(px, py);
                let cover = (0.5
                    - rect_distance(lx, ly, image.bounds, image.radius) * placement.scale)
                    .clamp(0.0, 1.0)
                    * (0.5 - rect_distance(px, py, clip, clip_radius)).clamp(0.0, 1.0);
                let u = (lx - image.bounds.x) / image.bounds.w.max(1.0e-4);
                let v = (ly - image.bounds.y) / image.bounds.h.max(1.0e-4);
                let color = sample_image(texture, image.texels, u, v);
                let index = y * layer.width + x;
                layer.pixels[index] = blend(
                    image.blend,
                    color,
                    cover * image.opacity,
                    layer.pixels[index],
                );
            }
        }
    }
}

pub fn draw_slug(
    layer: &mut Layer,
    store: &RefTexture,
    instances: &[SlugInstance],
    tone: ToneCurve,
) {
    let fetch = |index: u32| store.float_texel(index);
    for glyph in instances {
        let Some(inverse) = glyph.linear.inverse() else {
            continue;
        };
        let clip = glyph.clip.unwrap_or(NO_CLIP);
        let [r, g, b, a] = glyph.color.to_gpu();
        let color = [r, g, b, a].map(|channel| channel * glyph.opacity);
        let luminance = premultiplied_luminance(color);
        let pixels_per_em = glyph.pixels_per_em();
        let margin = 1.0 / pixels_per_em.max(1.0e-4);
        let [x0, y0, x1, y1] = glyph.em_bounds;
        let covered = glyph.covered();
        for y in pixel_span(covered.y, covered.bottom(), layer.height) {
            for x in pixel_span(covered.x, covered.right(), layer.width) {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let em = inverse.apply([px - glyph.origin[0], py - glyph.origin[1]]);
                if em[0] < x0 - margin
                    || em[0] > x1 + margin
                    || em[1] < y0 - margin
                    || em[1] > y1 + margin
                {
                    continue;
                }
                let coverage = slug::coverage(fetch, glyph.glyph, em, pixels_per_em);
                let weight = tone.alpha(coverage, luminance) * clip_cover(clip, px, py);
                let source = color.map(|channel| channel * weight);
                let index = y * layer.width + x;
                layer.pixels[index] = over(source, layer.pixels[index]);
            }
        }
    }
}
