use aqua_core::geometry::Rect;
use aqua_graph::{Blend, ImageInstance};

use crate::raster::{over, pixel_window, scaled, Layer, Pixel, Placement, QuadShape};

#[derive(Clone, Copy, Debug)]
pub struct Window {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Window {
    pub fn of_rect(rect: Rect, layer: &Layer) -> Self {
        Self {
            left: rect.x.clamp(0.0, layer.width as f32),
            top: rect.y.clamp(0.0, layer.height as f32),
            right: rect.right().clamp(0.0, layer.width as f32),
            bottom: rect.bottom().clamp(0.0, layer.height as f32),
        }
    }

    pub fn whole(layer: &Layer) -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            right: layer.width as f32,
            bottom: layer.height as f32,
        }
    }
}

fn axis(position: f32, low: f32, high: f32) -> (usize, usize, f32) {
    let (first, last) = (
        low.floor() as i64,
        (high.ceil() as i64 - 1).max(low.floor() as i64),
    );
    let clamped = match high - low >= 1.0 {
        true => position.clamp(low + 0.5, high - 0.5),
        false => (low + high) * 0.5,
    };
    let shifted = clamped - 0.5;
    let base = shifted.floor();
    let fraction = shifted - base;
    let near = (base as i64).clamp(first, last) as usize;
    let far = (base as i64 + 1).clamp(first, last) as usize;
    (near, far, fraction)
}

pub fn bilinear(layer: &Layer, x: f32, y: f32, window: Window) -> Pixel {
    if layer.width == 0
        || layer.height == 0
        || window.right <= window.left
        || window.bottom <= window.top
    {
        return [0.0; 4];
    }
    let (x0, x1, fx) = axis(x, window.left, window.right);
    let (y0, y1, fy) = axis(y, window.top, window.bottom);
    let top = lerp(layer.at(x0, y0), layer.at(x1, y0), fx);
    let bottom = lerp(layer.at(x0, y1), layer.at(x1, y1), fx);
    lerp(top, bottom, fy)
}

fn lerp(a: Pixel, b: Pixel, t: f32) -> Pixel {
    std::array::from_fn(|channel| a[channel] + (b[channel] - a[channel]) * t)
}

fn texel_position(bounds: Rect, texels: Rect, px: f32, py: f32) -> (f32, f32) {
    (
        texels.x + (px - bounds.x) / bounds.w * texels.w,
        texels.y + (py - bounds.y) / bounds.h * texels.h,
    )
}

pub fn draw_image(layer: &mut Layer, source: &Layer, image: &ImageInstance) {
    if image.bounds.w <= 0.0 || image.bounds.h <= 0.0 {
        return;
    }
    let covered = image.covered();
    let expanded = Rect::from_edges(
        covered.x - 1.0,
        covered.y - 1.0,
        covered.right() + 1.0,
        covered.bottom() + 1.0,
    );
    let (x0, y0, x1, y1) = pixel_window(expanded, image.clip, layer.width, layer.height);
    let placement = Placement::new(image.bounds, image.linear);
    let shape = QuadShape::new(image.bounds, image.radius, 0.0).scaled_by(placement.scale);
    let clip_shape = image
        .clip
        .map(|clip| QuadShape::new(clip, image.clip_radius, 0.0));
    let window = Window::of_rect(image.texels, source);
    (y0..y1).for_each(|y| {
        (x0..x1).for_each(|x| {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let (lx, ly) = placement.local(px, py);
            let cover =
                shape.coverage(lx, ly) * clip_shape.map_or(1.0, |clip| clip.coverage(px, py));
            if cover <= 0.0 {
                return;
            }
            let (u, v) = texel_position(image.bounds, image.texels, lx, ly);
            let sample = bilinear(source, u, v, window);
            layer.put(
                x,
                y,
                blend(image.blend, sample, cover * image.opacity, layer.at(x, y)),
            );
        });
    });
}

pub fn blend(mode: Blend, source: Pixel, weight: f32, destination: Pixel) -> Pixel {
    match mode {
        Blend::Over => over(scaled(source, weight), destination),
        Blend::Replace => std::array::from_fn(|channel| {
            source[channel] * weight + destination[channel] * (1.0 - weight)
        }),
    }
}
