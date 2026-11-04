use aqua_graph::FilterParams;

use crate::raster::{Layer, Pixel};
use crate::sample::{Window, bilinear};

fn weighted(taps: &[((f32, f32), f32)], sample: impl Fn(f32, f32) -> Pixel) -> Pixel {
    let total: f32 = taps.iter().map(|(_, weight)| weight).sum();
    let mut sum = [0.0f32; 4];
    for ((dx, dy), weight) in taps {
        let pixel = sample(*dx, *dy);
        (0..4).for_each(|channel| sum[channel] += pixel[channel] * weight);
    }
    sum.map(|channel| channel / total)
}

pub fn blur_down(source: &Layer, target: &mut Layer, params: &FilterParams) {
    let offset = params.values[0];
    let region = params.region;
    let window = Window::of_rect(region, source);
    let ratio = (
        region.w / target.width as f32,
        region.h / target.height as f32,
    );
    let reach = (
        0.5 * offset * ratio.0.max(1.0),
        0.5 * offset * ratio.1.max(1.0),
    );
    for y in 0..target.height {
        for x in 0..target.width {
            let cx = region.x + (x as f32 + 0.5) * ratio.0;
            let cy = region.y + (y as f32 + 0.5) * ratio.1;
            let taps = [
                ((0.0, 0.0), 4.0),
                ((-reach.0, -reach.1), 1.0),
                ((reach.0, -reach.1), 1.0),
                ((-reach.0, reach.1), 1.0),
                ((reach.0, reach.1), 1.0),
            ];
            let value = weighted(&taps, |dx, dy| bilinear(source, cx + dx, cy + dy, window));
            target.put(x, y, value);
        }
    }
}

pub fn blur_up(source: &Layer, target: &mut Layer, params: &FilterParams) {
    let offset = params.values[0];
    let window = Window::whole(source);
    let ratio = (
        source.width as f32 / target.width as f32,
        source.height as f32 / target.height as f32,
    );
    let reach = 0.5 * offset;
    for y in 0..target.height {
        for x in 0..target.width {
            let cx = (x as f32 + 0.5) * ratio.0;
            let cy = (y as f32 + 0.5) * ratio.1;
            let taps = [
                ((-2.0 * reach, 0.0), 1.0),
                ((2.0 * reach, 0.0), 1.0),
                ((0.0, -2.0 * reach), 1.0),
                ((0.0, 2.0 * reach), 1.0),
                ((-reach, -reach), 2.0),
                ((reach, -reach), 2.0),
                ((-reach, reach), 2.0),
                ((reach, reach), 2.0),
            ];
            let value = weighted(&taps, |dx, dy| bilinear(source, cx + dx, cy + dy, window));
            target.put(x, y, value);
        }
    }
}
