use aqua_core::{geometry::Rect, Oklch};
use aqua_graph::{Gradient, QuadInstance};

pub type Pixel = [f32; 4];

pub const TRANSPARENT: Pixel = [0.0; 4];

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<Pixel>,
}

impl Layer {
    pub fn filled(width: usize, height: usize, pixel: Pixel) -> Self {
        Self {
            width,
            height,
            pixels: vec![pixel; width * height],
        }
    }

    pub fn transparent(width: usize, height: usize) -> Self {
        Self::filled(width, height, TRANSPARENT)
    }

    pub fn at(&self, x: usize, y: usize) -> Pixel {
        self.pixels[y * self.width + x]
    }

    pub fn put(&mut self, x: usize, y: usize, pixel: Pixel) {
        self.pixels[y * self.width + x] = pixel;
    }
}

pub fn over(source: Pixel, destination: Pixel) -> Pixel {
    std::array::from_fn(|channel| source[channel] + destination[channel] * (1.0 - source[3]))
}

pub fn scaled(pixel: Pixel, factor: f32) -> Pixel {
    pixel.map(|channel| channel * factor)
}

pub fn premultiplied(color: Oklch) -> Pixel {
    color.to_gpu()
}

fn straight(color: Oklch) -> Pixel {
    color.to_linear()
}

fn length(x: f32, y: f32) -> f32 {
    (x * x + y * y).sqrt()
}

pub fn saturate(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

#[derive(Clone, Copy, Debug)]
pub struct QuadShape {
    pub center: (f32, f32),
    pub half: (f32, f32),
    pub radius: f32,
    pub edge: f32,
}

impl QuadShape {
    pub fn new(bounds: Rect, radius: f32, border_width: f32) -> Self {
        Self {
            center: (bounds.x + bounds.w * 0.5, bounds.y + bounds.h * 0.5),
            half: (bounds.w * 0.5, bounds.h * 0.5),
            radius,
            edge: if border_width == 0.0 {
                -0.5
            } else {
                border_width
            },
        }
    }

    pub fn outer_distance(&self, px: f32, py: f32) -> f32 {
        let relative = (px - self.center.0, py - self.center.1);
        let qx = relative.0.abs() - self.half.0 + self.radius;
        let qy = relative.1.abs() - self.half.1 + self.radius;
        match self.radius == 0.0 {
            true => qx.max(qy),
            false => length(qx.max(0.0), qy.max(0.0)) + qx.max(qy).min(0.0) - self.radius,
        }
    }

    pub fn inner_distance(&self, px: f32, py: f32, outer: f32) -> f32 {
        let relative = (px - self.center.0, py - self.center.1);
        let offset = (
            relative.0.abs() - self.half.0,
            relative.1.abs() - self.half.1,
        );
        let corner = (offset.0 + self.radius, offset.1 + self.radius);
        let straight = (offset.0 + self.edge, offset.1 + self.edge);
        if corner.0 <= 0.0 || corner.1 <= 0.0 {
            -straight.0.max(straight.1)
        } else if straight.0 > 0.0 || straight.1 > 0.0 {
            -1.0
        } else {
            -(outer + self.edge)
        }
    }

    pub fn coverage(&self, px: f32, py: f32) -> f32 {
        saturate(0.5 - self.outer_distance(px, py))
    }
}

pub fn clip_coverage(clip: Option<Rect>, px: f32, py: f32) -> f32 {
    match clip {
        None => 1.0,
        Some(rect) => QuadShape::new(rect, 0.0, 0.0).coverage(px, py),
    }
}

pub fn gradient_color(gradient: &Gradient, px: f32, py: f32) -> Pixel {
    let direction = (
        gradient.end[0] - gradient.start[0],
        gradient.end[1] - gradient.start[1],
    );
    let span = direction.0 * direction.0 + direction.1 * direction.1;
    let along = match span > 0.0 {
        true => {
            ((px - gradient.start[0]) * direction.0 + (py - gradient.start[1]) * direction.1) / span
        }
        false => 0.0,
    };
    let [first, second] = gradient.stops;
    let width = second.offset - first.offset;
    let t = match width > 0.0 {
        true => saturate((along - first.offset) / width),
        false => (along >= second.offset) as u32 as f32,
    };
    let oklab = |color: Oklch| {
        let hue = color.h.to_radians();
        [
            color.l,
            color.c * hue.cos(),
            color.c * hue.sin(),
            color.alpha,
        ]
    };
    let (from, to) = (oklab(first.color), oklab(second.color));
    let mix: [f32; 4] =
        std::array::from_fn(|channel| from[channel] + (to[channel] - from[channel]) * t);
    Oklch::oklch(
        mix[0],
        length(mix[1], mix[2]),
        mix[2].atan2(mix[1]).to_degrees(),
        mix[3],
    )
    .to_gpu()
}

fn quad_color(shape: &QuadShape, fill: Pixel, border: Oklch, px: f32, py: f32) -> Pixel {
    let outer = shape.outer_distance(px, py);
    let inner = shape.inner_distance(px, py, outer);
    let mut color = fill;
    if inner.max(outer) < 0.5 {
        let border = straight(border);
        let alpha = border[3];
        let blended = [
            border[0] * alpha + fill[0] * (1.0 - alpha),
            border[1] * alpha + fill[1] * (1.0 - alpha),
            border[2] * alpha + fill[2] * (1.0 - alpha),
            alpha + fill[3] * (1.0 - alpha),
        ];
        let mix = saturate(0.5 - inner);
        color =
            std::array::from_fn(|channel| fill[channel] + (blended[channel] - fill[channel]) * mix);
    }
    scaled(color, saturate(0.5 - outer))
}

pub fn pixel_window(
    region: Rect,
    clip: Option<Rect>,
    width: usize,
    height: usize,
) -> (usize, usize, usize, usize) {
    let (mut left, mut top, mut right, mut bottom) =
        (region.x, region.y, region.right(), region.bottom());
    if let Some(clip) = clip {
        left = left.max(clip.x - 1.0);
        top = top.max(clip.y - 1.0);
        right = right.min(clip.right() + 1.0);
        bottom = bottom.min(clip.bottom() + 1.0);
    }
    let x0 = (left.floor().max(0.0) as usize).min(width);
    let y0 = (top.floor().max(0.0) as usize).min(height);
    let x1 = (right.ceil().max(0.0) as usize).min(width);
    let y1 = (bottom.ceil().max(0.0) as usize).min(height);
    (x0, y0, x1.max(x0), y1.max(y0))
}

pub fn expand(rect: Rect, by: f32) -> Rect {
    Rect::from_edges(
        rect.x - by,
        rect.y - by,
        rect.right() + by,
        rect.bottom() + by,
    )
}

pub fn draw_quad(layer: &mut Layer, instance: &QuadInstance) {
    let (x0, y0, x1, y1) = pixel_window(
        expand(instance.bounds, 1.0),
        instance.clip,
        layer.width,
        layer.height,
    );
    let shape = QuadShape::new(instance.bounds, instance.radius, instance.border_width);
    let flat_fill = premultiplied(instance.fill);
    (y0..y1).for_each(|y| {
        (x0..x1).for_each(|x| {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let cover = clip_coverage(instance.clip, px, py);
            if cover <= 0.0 {
                return;
            }
            let fill = match &instance.gradient {
                Some(gradient) => gradient_color(gradient, px, py),
                None => flat_fill,
            };
            let color = scaled(
                quad_color(&shape, fill, instance.border_color, px, py),
                cover * instance.opacity,
            );
            layer.put(x, y, over(color, layer.at(x, y)));
        });
    });
}

pub fn composite_layer(destination: &mut Layer, source: &Layer, opacity: f32) {
    destination
        .pixels
        .iter_mut()
        .zip(&source.pixels)
        .for_each(|(target, pixel)| {
            *target = over(scaled(*pixel, opacity), *target);
        });
}

pub fn to_rgba8(layer: &Layer) -> Vec<u8> {
    layer
        .pixels
        .iter()
        .flat_map(|pixel| pixel.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8))
        .collect()
}
