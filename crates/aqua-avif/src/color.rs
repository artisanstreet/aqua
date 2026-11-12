use crate::decoder::{Chroma, Picture, Plane};
use crate::{AvifError, Rgba};

const MATRIX_IDENTITY: u32 = 0;
const MATRIX_BT709: u32 = 1;
const MATRIX_BT2020_NCL: u32 = 9;
const MATRIX_BT2020_CL: u32 = 10;

#[derive(Clone, Copy)]
struct Levels {
    luma_offset: f32,
    luma_scale: f32,
    chroma_offset: f32,
    chroma_scale: f32,
}

impl Levels {
    fn new(depth: u32, full_range: bool) -> Self {
        let peak = ((1u32 << depth) - 1) as f32;
        let unit = (1u32 << (depth - 8)) as f32;
        let chroma_offset = (1u32 << (depth - 1)) as f32;
        match full_range {
            true => Self {
                luma_offset: 0.0,
                luma_scale: 1.0 / peak,
                chroma_offset,
                chroma_scale: 1.0 / peak,
            },
            false => Self {
                luma_offset: 16.0 * unit,
                luma_scale: 1.0 / (219.0 * unit),
                chroma_offset,
                chroma_scale: 1.0 / (224.0 * unit),
            },
        }
    }

    fn luma(self, sample: f32) -> f32 {
        (sample - self.luma_offset) * self.luma_scale
    }

    fn chroma(self, sample: f32) -> f32 {
        (sample - self.chroma_offset) * self.chroma_scale
    }
}

fn byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

fn weights(matrix: u32) -> (f32, f32) {
    match matrix {
        MATRIX_BT709 => (0.2126, 0.0722),
        MATRIX_BT2020_NCL | MATRIX_BT2020_CL => (0.2627, 0.0593),
        _ => (0.299, 0.114),
    }
}

struct ChromaPlanes<'a> {
    blue: Plane<'a>,
    red: Plane<'a>,
    horizontal: usize,
    vertical: usize,
}

pub fn rgba(picture: &Picture<'_>) -> Result<Rgba, AvifError> {
    let depth = picture.depth();
    if !(8..=16).contains(&depth) {
        return Err(AvifError::Unsupported("bit depth"));
    }
    let (width, height) = (picture.width(), picture.height());
    let luma = picture.plane(0)?;
    if luma.width != width || luma.height != height {
        return Err(AvifError::Unsupported("plane extent"));
    }
    let chroma = match picture.chroma()? {
        Chroma::Monochrome => None,
        Chroma::Subsampled {
            horizontal,
            vertical,
        } => Some(ChromaPlanes {
            blue: picture.plane(1)?,
            red: picture.plane(2)?,
            horizontal: usize::from(horizontal),
            vertical: usize::from(vertical),
        }),
    };
    let levels = Levels::new(depth, picture.full_range());
    let matrix = picture.matrix();
    let (red_weight, blue_weight) = weights(matrix);
    let green_weight = 1.0 - red_weight - blue_weight;
    let mut pixels = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let light = luma.sample(x, y);
            let color = match &chroma {
                None => {
                    let value = byte(levels.luma(light));
                    [value, value, value]
                }
                Some(planes) => {
                    let (cx, cy) = (x >> planes.horizontal, y >> planes.vertical);
                    let (blue, red) = (planes.blue.sample(cx, cy), planes.red.sample(cx, cy));
                    match matrix {
                        MATRIX_IDENTITY => [
                            byte(levels.luma(red)),
                            byte(levels.luma(light)),
                            byte(levels.luma(blue)),
                        ],
                        _ => {
                            let light = levels.luma(light);
                            let red = light + 2.0 * (1.0 - red_weight) * levels.chroma(red);
                            let blue = light + 2.0 * (1.0 - blue_weight) * levels.chroma(blue);
                            let green =
                                (light - red_weight * red - blue_weight * blue) / green_weight;
                            [byte(red), byte(green), byte(blue)]
                        }
                    }
                }
            };
            pixels.extend_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
    Ok(Rgba {
        width: u32::try_from(width).map_err(|_| AvifError::Unsupported("plane extent"))?,
        height: u32::try_from(height).map_err(|_| AvifError::Unsupported("plane extent"))?,
        pixels,
    })
}

pub fn apply_alpha(
    image: &mut Rgba,
    picture: &Picture<'_>,
    premultiplied: bool,
) -> Result<(), AvifError> {
    let depth = picture.depth();
    if !(8..=16).contains(&depth) {
        return Err(AvifError::Unsupported("bit depth"));
    }
    let plane = picture.plane(0)?;
    if plane.width != image.width as usize || plane.height != image.height as usize {
        return Err(AvifError::Unsupported("alpha extent"));
    }
    let levels = Levels::new(depth, picture.full_range());
    for (index, pixel) in image.pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let alpha = levels
            .luma(plane.sample(index % plane.width, index / plane.width))
            .clamp(0.0, 1.0);
        if premultiplied && alpha > 0.0 {
            for channel in &mut pixel[..3] {
                *channel = byte(f32::from(*channel) / 255.0 / alpha);
            }
        }
        pixel[3] = byte(alpha);
    }
    Ok(())
}
