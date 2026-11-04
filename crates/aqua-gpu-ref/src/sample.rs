use aqua_core::geometry::Rect;
use aqua_graph::ImageInstance;

use crate::raster::{Layer, Pixel, QuadShape, clip_coverage, over, pixel_window, scaled};

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
    let expanded = Rect::from_edges(
        image.bounds.x - 1.0,
        image.bounds.y - 1.0,
        image.bounds.right() + 1.0,
        image.bounds.bottom() + 1.0,
    );
    let (x0, y0, x1, y1) = pixel_window(expanded, image.clip, layer.width, layer.height);
    let shape = QuadShape::new(image.bounds, image.radius, 0.0);
    let window = Window::of_rect(image.texels, source);
    (y0..y1).for_each(|y| {
        (x0..x1).for_each(|x| {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let cover = shape.coverage(px, py) * clip_coverage(image.clip, px, py);
            if cover <= 0.0 {
                return;
            }
            let (u, v) = texel_position(image.bounds, image.texels, px, py);
            let sample = bilinear(source, u, v, window);
            layer.put(
                x,
                y,
                over(scaled(sample, cover * image.opacity), layer.at(x, y)),
            );
        });
    });
}
