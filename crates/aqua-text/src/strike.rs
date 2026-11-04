use std::io::Cursor;

use skrifa::bitmap::{BitmapData, BitmapGlyph, Origin};
use skrifa::instance::Size;
use skrifa::raw::TableProvider;
use skrifa::{GlyphId, MetadataProvider};

use crate::raster::{Bitmap, GlyphImage, SUBPIXEL_STEPS};

#[derive(Clone, Debug, PartialEq)]
pub struct Premultiplied {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 4]>,
}

pub fn decode_png(bytes: &[u8]) -> Option<Premultiplied> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buffer).ok()?;
    let (width, height) = (info.width, info.height);
    let count = width as usize * height as usize;
    let data = &buffer[..info.buffer_size()];
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Grayscale => 1,
        png::ColorType::Indexed => return None,
    };
    (data.len() >= count * channels).then_some(())?;
    let unit = |value: u8| f32::from(value) / 255.0;
    let pixels = data
        .chunks_exact(channels)
        .take(count)
        .map(|texel| {
            let (rgb, alpha) = match channels {
                4 => ([texel[0], texel[1], texel[2]], texel[3]),
                3 => ([texel[0], texel[1], texel[2]], 255),
                2 => ([texel[0]; 3], texel[1]),
                _ => ([texel[0]; 3], 255),
            };
            let alpha = unit(alpha);
            [
                unit(rgb[0]) * alpha,
                unit(rgb[1]) * alpha,
                unit(rgb[2]) * alpha,
                alpha,
            ]
        })
        .collect();
    Some(Premultiplied {
        width,
        height,
        pixels,
    })
}

pub fn from_bgra(bytes: &[u8], width: u32, height: u32) -> Option<Premultiplied> {
    let count = width as usize * height as usize;
    (bytes.len() >= count * 4).then_some(())?;
    let pixels = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .take(count)
        .map(|texel| {
            [
                f32::from(texel[2]) / 255.0,
                f32::from(texel[1]) / 255.0,
                f32::from(texel[0]) / 255.0,
                f32::from(texel[3]) / 255.0,
            ]
        })
        .collect();
    Some(Premultiplied {
        width,
        height,
        pixels,
    })
}

fn taps(source: u32, index: u32, phase: f32, extent: f32) -> Vec<(usize, f32)> {
    let scale = source as f32 / extent;
    let last_texel = source as f32 - 1.0;
    let inside = (index as f32 + 1.0).min(phase + extent) - (index as f32).max(phase);
    let coverage = inside.clamp(0.0, 1.0);
    match scale >= 1.0 {
        true => {
            let (start, end) = (
                ((index as f32 - phase) * scale).max(0.0),
                (((index + 1) as f32 - phase) * scale).min(source as f32),
            );
            let first = start.floor() as usize;
            let last = (end.ceil() as usize).min(source as usize);
            (first..last)
                .map(|texel| {
                    let overlap = end.min(texel as f32 + 1.0) - start.max(texel as f32);
                    (texel, overlap.max(0.0) / scale)
                })
                .collect()
        }
        false => {
            let center = (index as f32 + 0.5 - phase) * scale - 0.5;
            let base = center.floor();
            let fraction = center - base;
            let clamp = |texel: f32| texel.clamp(0.0, last_texel) as usize;
            vec![
                (clamp(base), (1.0 - fraction) * coverage),
                (clamp(base + 1.0), fraction * coverage),
            ]
        }
    }
}

pub fn resample(image: &Premultiplied, width: u32, height: u32) -> Premultiplied {
    resample_shifted(image, width, height, 0.0)
}

pub fn resample_shifted(
    image: &Premultiplied,
    width: u32,
    height: u32,
    shift: f32,
) -> Premultiplied {
    let (width, height) = (width.max(1), height.max(1));
    let shift = shift.clamp(0.0, 0.999);
    let columns = (width as f32 + shift).ceil() as u32;
    let horizontal: Vec<Vec<(usize, f32)>> = (0..columns)
        .map(|x| taps(image.width, x, shift, width as f32))
        .collect();
    let vertical: Vec<Vec<(usize, f32)>> = (0..height)
        .map(|y| taps(image.height, y, 0.0, height as f32))
        .collect();
    let width = columns;
    let source_width = image.width as usize;
    let mut across = vec![[0.0f32; 4]; width as usize * image.height as usize];
    for row in 0..image.height as usize {
        for (x, weights) in horizontal.iter().enumerate() {
            let mut sum = [0.0f32; 4];
            for (texel, weight) in weights {
                let pixel = image.pixels[row * source_width + texel];
                (0..4).for_each(|channel| sum[channel] += pixel[channel] * weight);
            }
            across[row * width as usize + x] = sum;
        }
    }
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for weights in &vertical {
        for x in 0..width as usize {
            let mut sum = [0.0f32; 4];
            for (row, weight) in weights {
                let pixel = across[row * width as usize + x];
                (0..4).for_each(|channel| sum[channel] += pixel[channel] * weight);
            }
            pixels.push(sum);
        }
    }
    Premultiplied {
        width,
        height,
        pixels,
    }
}

fn bytes_of(image: &Premultiplied) -> Vec<u8> {
    image
        .pixels
        .iter()
        .flat_map(|pixel| pixel.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8))
        .collect()
}

fn alphas_of(image: &Premultiplied) -> Vec<u8> {
    image
        .pixels
        .iter()
        .map(|pixel| (pixel[3].clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect()
}

pub fn from_alpha(alpha: &[u8], width: u32, height: u32) -> Option<Premultiplied> {
    let count = width as usize * height as usize;
    (alpha.len() >= count).then_some(())?;
    let pixels = alpha
        .iter()
        .take(count)
        .map(|value| {
            let value = f32::from(*value) / 255.0;
            [value; 4]
        })
        .collect();
    Some(Premultiplied {
        width,
        height,
        pixels,
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrikePlacement {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

pub fn placement(glyph: &BitmapGlyph<'_>, size: f32, units_per_em: f32) -> StrikePlacement {
    let scale_x = size / glyph.ppem_x.max(1.0);
    let scale_y = size / glyph.ppem_y.max(1.0);
    let font_scale = size / units_per_em.max(1.0);
    let width = glyph.width as f32 * scale_x;
    let height = glyph.height as f32 * scale_y;
    match glyph.placement_origin {
        Origin::TopLeft => StrikePlacement {
            left: glyph.inner_bearing_x * scale_x,
            top: glyph.inner_bearing_y * scale_y,
            width,
            height,
        },
        Origin::BottomLeft => StrikePlacement {
            left: glyph.bearing_x * font_scale + glyph.inner_bearing_x * scale_x,
            top: glyph.bearing_y * font_scale + glyph.inner_bearing_y * scale_y + height,
            width,
            height,
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrikeKind {
    Any,
    MasksAtExactSize,
}

pub fn has_mask_strikes(font: &skrifa::FontRef<'_>) -> bool {
    font.data_for_tag(skrifa::Tag::new(b"EBLC")).is_some()
        && font.data_for_tag(skrifa::Tag::new(b"EBDT")).is_some()
}

pub fn rasterize_strike(
    font: &skrifa::FontRef<'_>,
    glyph: GlyphId,
    size: f32,
    units_per_em: f32,
    subpixel: u8,
    kind: StrikeKind,
) -> Option<GlyphImage> {
    let strikes = font.bitmap_strikes();
    let found = strikes.glyph_for_size(Size::new(size), glyph)?;
    let (decoded, is_mask) = match &found.data {
        BitmapData::Png(bytes) => (decode_png(bytes)?, false),
        BitmapData::Bgra(bytes) => (from_bgra(bytes, found.width, found.height)?, false),
        BitmapData::Mask(mask) => {
            let mut alpha = vec![0u8; found.width as usize * found.height as usize];
            mask.decode_to_slice(found.width, found.height, &mut alpha)
                .ok()?;
            (from_alpha(&alpha, found.width, found.height)?, true)
        }
    };
    let exact = (found.ppem_y - size).abs() < 0.5 && (found.ppem_x - size).abs() < 0.5;
    if is_mask && kind == StrikeKind::MasksAtExactSize && !exact {
        return None;
    }
    let at = placement(&found, size, units_per_em);
    let width = at.width.round().max(1.0) as u32;
    let height = at.height.round().max(1.0) as u32;
    let shift = f32::from(subpixel) / f32::from(SUBPIXEL_STEPS);
    let scaled = match (decoded.width == width, decoded.height == height, subpixel) {
        (true, true, 0) => decoded,
        _ => resample_shifted(&decoded, width, height, shift),
    };
    let bitmap = Bitmap {
        left: at.left.round() as i32,
        top: at.top.round() as i32,
        width: scaled.width,
        height: scaled.height,
        data: match is_mask {
            true => alphas_of(&scaled),
            false => bytes_of(&scaled),
        },
    };
    Some(match is_mask {
        true => GlyphImage::Mask(bitmap),
        false => GlyphImage::Color(bitmap),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, pixel: [f32; 4]) -> Premultiplied {
        Premultiplied {
            width,
            height,
            pixels: vec![pixel; (width * height) as usize],
        }
    }

    fn encode(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(rgba)
            .unwrap();
        out
    }

    #[test]
    fn png_decoding_premultiplies_straight_alpha() {
        let decoded = decode_png(&encode(2, 1, &[255, 0, 0, 128, 0, 255, 0, 255])).unwrap();
        assert_eq!((decoded.width, decoded.height), (2, 1));
        let half = 128.0 / 255.0;
        assert!((decoded.pixels[0][0] - half).abs() < 1e-5);
        assert!((decoded.pixels[0][3] - half).abs() < 1e-5);
        assert_eq!(decoded.pixels[1], [0.0, 1.0, 0.0, 1.0]);
    }

    #[test]
    fn corrupt_png_is_rejected() {
        assert!(decode_png(&[1, 2, 3, 4]).is_none());
    }

    #[test]
    fn downscaling_preserves_total_energy_and_flat_color() {
        let flat = resample(&solid(8, 8, [0.5, 0.25, 0.0, 0.5]), 3, 3);
        assert!(flat
            .pixels
            .iter()
            .all(|p| (p[3] - 0.5).abs() < 1e-5 && (p[0] - 0.5).abs() < 1e-5));
        let mut checker = solid(4, 4, [0.0; 4]);
        for index in 0..16 {
            if (index % 4 + index / 4) % 2 == 0 {
                checker.pixels[index] = [1.0, 1.0, 1.0, 1.0];
            }
        }
        let halved = resample(&checker, 2, 2);
        assert!(halved.pixels.iter().all(|p| (p[3] - 0.5).abs() < 1e-5));
    }

    #[test]
    fn upscaling_interpolates_between_texels() {
        let mut ramp = solid(2, 1, [0.0; 4]);
        ramp.pixels[1] = [1.0, 1.0, 1.0, 1.0];
        let wide = resample(&ramp, 8, 1);
        assert!(wide
            .pixels
            .windows(2)
            .all(|pair| pair[0][3] <= pair[1][3] + 1e-6));
        assert!(wide.pixels[0][3] < 0.01 && wide.pixels[7][3] > 0.99);
    }

    #[test]
    fn bgra_is_swizzled_to_rgba() {
        let converted = from_bgra(&[10, 20, 30, 255], 1, 1).unwrap();
        assert_eq!(converted.pixels[0][0], 30.0 / 255.0);
        assert_eq!(converted.pixels[0][2], 10.0 / 255.0);
    }

    fn be16(value: u16) -> [u8; 2] {
        value.to_be_bytes()
    }

    fn sbix_font(png: &[u8], ppem: u16, origin: (i16, i16)) -> Vec<u8> {
        let mut head = vec![0u8; 54];
        head[0..2].copy_from_slice(&be16(1));
        head[12..16].copy_from_slice(&0x5F0F3CF5u32.to_be_bytes());
        head[18..20].copy_from_slice(&be16(1000));
        let mut maxp = 0x0000_5000u32.to_be_bytes().to_vec();
        maxp.extend_from_slice(&be16(2));
        let glyph_len = 8 + png.len() as u32;
        let strike_header = 4 + 4 * 3;
        let mut strike = Vec::new();
        strike.extend_from_slice(&be16(ppem));
        strike.extend_from_slice(&be16(72));
        for offset in [strike_header, strike_header, strike_header + glyph_len] {
            strike.extend_from_slice(&offset.to_be_bytes());
        }
        strike.extend_from_slice(&origin.0.to_be_bytes());
        strike.extend_from_slice(&origin.1.to_be_bytes());
        strike.extend_from_slice(b"png ");
        strike.extend_from_slice(png);
        let mut sbix = Vec::new();
        sbix.extend_from_slice(&be16(1));
        sbix.extend_from_slice(&be16(1));
        sbix.extend_from_slice(&1u32.to_be_bytes());
        sbix.extend_from_slice(&12u32.to_be_bytes());
        sbix.extend_from_slice(&strike);
        let tables: [(&[u8; 4], Vec<u8>); 3] = [(b"head", head), (b"maxp", maxp), (b"sbix", sbix)];
        let mut out = vec![0, 1, 0, 0];
        out.extend_from_slice(&be16(tables.len() as u16));
        out.extend_from_slice(&[0u8; 6]);
        let mut offset = 12 + 16 * tables.len();
        let mut body = Vec::new();
        for (tag, data) in &tables {
            out.extend_from_slice(*tag);
            out.extend_from_slice(&0u32.to_be_bytes());
            out.extend_from_slice(&(offset as u32).to_be_bytes());
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            body.extend_from_slice(data);
            let padding = (4 - data.len() % 4) % 4;
            body.extend_from_slice(&vec![0u8; padding]);
            offset += data.len() + padding;
        }
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn sbix_strikes_scale_and_place_relative_to_the_glyph_origin() {
        let rgba: Vec<u8> = (0..32 * 32).flat_map(|_| [200u8, 100, 50, 255]).collect();
        let data = sbix_font(&encode(32, 32, &rgba), 32, (2, -4));
        let font = skrifa::FontRef::new(&data).expect("synthetic font parses");
        assert_eq!(font.bitmap_strikes().len(), 1);
        let bitmap = rasterize_strike(&font, GlyphId::new(1), 16.0, 1000.0, 0, StrikeKind::Any)
            .expect("strike")
            .bitmap()
            .clone();
        assert_eq!((bitmap.width, bitmap.height), (16, 16));
        assert_eq!((bitmap.left, bitmap.top), (1, 14));
        assert_eq!(&bitmap.data[..4], &[200, 100, 50, 255]);
        assert!(
            rasterize_strike(&font, GlyphId::new(0), 16.0, 1000.0, 0, StrikeKind::Any).is_none()
        );
    }

    #[test]
    fn cbdt_strikes_hang_from_the_baseline_by_their_bearings() {
        let glyph = BitmapGlyph {
            data: BitmapData::Bgra(&[]),
            bearing_x: 0.0,
            bearing_y: 0.0,
            inner_bearing_x: 3.0,
            inner_bearing_y: 100.0,
            ppem_x: 109.0,
            ppem_y: 109.0,
            advance: Some(136.0),
            width: 136,
            height: 128,
            placement_origin: Origin::TopLeft,
        };
        let at = placement(&glyph, 54.5, 2048.0);
        assert_eq!(at.left, 1.5);
        assert_eq!(at.top, 50.0);
        assert_eq!((at.width, at.height), (68.0, 64.0));
    }
}

#[cfg(test)]
mod shift_tests {
    use super::*;

    fn bar() -> Premultiplied {
        Premultiplied {
            width: 4,
            height: 1,
            pixels: vec![[1.0; 4]; 4],
        }
    }

    fn total(image: &Premultiplied) -> f32 {
        image.pixels.iter().map(|pixel| pixel[3]).sum()
    }

    #[test]
    fn shifting_conserves_ink_and_widens_by_one_column() {
        let unshifted = resample_shifted(&bar(), 4, 1, 0.0);
        let shifted = resample_shifted(&bar(), 4, 1, 0.25);
        assert_eq!(unshifted.width, 4);
        assert_eq!(shifted.width, 5);
        assert!((total(&unshifted) - 4.0).abs() < 1e-4);
        assert!((total(&shifted) - 4.0).abs() < 1e-4);
        assert!((shifted.pixels[0][3] - 0.75).abs() < 1e-4);
        assert!((shifted.pixels[4][3] - 0.25).abs() < 1e-4);
    }

    #[test]
    fn shifting_conserves_ink_when_scaling_up_and_down() {
        let up = resample_shifted(&bar(), 8, 1, 0.5);
        let down = resample_shifted(&bar(), 2, 1, 0.5);
        assert!(up.pixels.iter().all(|pixel| pixel[3] <= 1.0 + 1e-4));
        assert!((total(&down) - 2.0).abs() < 1e-4);
        assert!(total(&up) > 7.0 && total(&up) < 8.5);
    }

    #[test]
    fn eight_bit_masks_become_alpha_only_pixels() {
        let image = from_alpha(&[0, 128, 255, 64], 2, 2).unwrap();
        assert_eq!(alphas_of(&image), vec![0, 128, 255, 64]);
        assert!(from_alpha(&[0, 1], 2, 2).is_none());
    }
}
