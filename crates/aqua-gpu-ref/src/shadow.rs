use aqua_core::geometry::Rect;
use aqua_graph::{ShadowCast, ShadowInstance};

use crate::erf::{erf, normal_cdf};
use crate::raster::{
    Layer, QuadShape, clip_coverage, expand, over, pixel_window, premultiplied, scaled,
};

pub const HARD_EDGE_SIGMA: f32 = 0.05;
const MAX_STEP: f64 = 0.25;

#[derive(Clone, Copy, Debug)]
struct Silhouette {
    center: (f64, f64),
    half: (f64, f64),
    radius: f64,
}

impl Silhouette {
    fn of(shadow: &ShadowInstance) -> Self {
        let spread = shadow.spread as f64;
        let half = (
            (shadow.bounds.w as f64 * 0.5 + spread).max(0.0),
            (shadow.bounds.h as f64 * 0.5 + spread).max(0.0),
        );
        Self {
            center: (
                shadow.bounds.x as f64 + shadow.bounds.w as f64 * 0.5 + shadow.offset[0] as f64,
                shadow.bounds.y as f64 + shadow.bounds.h as f64 * 0.5 + shadow.offset[1] as f64,
            ),
            half,
            radius: (shadow.radius as f64 + spread)
                .max(0.0)
                .min(half.0.min(half.1)),
        }
    }

    fn half_width_at(&self, dy: f64) -> f64 {
        let straight = self.half.1 - self.radius;
        match dy.abs() <= straight {
            true => self.half.0,
            false => {
                let into_corner = dy.abs() - straight;
                self.half.0 - self.radius
                    + (self.radius * self.radius - into_corner * into_corner)
                        .max(0.0)
                        .sqrt()
            }
        }
    }

    fn rect(&self) -> Rect {
        Rect::from_edges(
            (self.center.0 - self.half.0) as f32,
            (self.center.1 - self.half.1) as f32,
            (self.center.0 + self.half.0) as f32,
            (self.center.1 + self.half.1) as f32,
        )
    }
}

pub fn blurred_coverage(shadow: &ShadowInstance, px: f32, py: f32) -> f32 {
    let silhouette = Silhouette::of(shadow);
    match shadow.sigma < HARD_EDGE_SIGMA {
        true => QuadShape::new(silhouette.rect(), silhouette.radius as f32, 0.0).coverage(px, py),
        false => gaussian_coverage(&silhouette, shadow.sigma as f64, px as f64, py as f64) as f32,
    }
}

const SATURATION: f64 = 6.0;

struct RowProfile {
    taps: Vec<(f64, f64)>,
    widest: f64,
    narrowest: f64,
    mass: f64,
    scale: f64,
}

impl RowProfile {
    fn at(silhouette: &Silhouette, sigma: f64, py: f64) -> Option<Self> {
        let top = silhouette.center.1 - silhouette.half.1;
        let bottom = silhouette.center.1 + silhouette.half.1;
        let low = top.max(py - 6.0 * sigma);
        let high = bottom.min(py + 6.0 * sigma);
        if high <= low || silhouette.half.0 <= 0.0 {
            return None;
        }
        let steps = (((high - low) / MAX_STEP.min(sigma / 4.0)).ceil() as usize).max(1);
        let step = (high - low) / steps as f64;
        let taps: Vec<(f64, f64)> = (0..steps)
            .map(|index| {
                let y0 = low + step * index as f64;
                let y1 = y0 + step;
                (
                    normal_cdf((y1 - py) / sigma) - normal_cdf((y0 - py) / sigma),
                    silhouette.half_width_at((y0 + y1) * 0.5 - silhouette.center.1),
                )
            })
            .fold(Vec::new(), |mut merged: Vec<(f64, f64)>, tap| {
                match merged.last_mut() {
                    Some(last) if last.1 == tap.1 => last.0 += tap.0,
                    _ => merged.push(tap),
                }
                merged
            });
        Some(Self {
            widest: taps.iter().fold(0.0, |acc, tap| f64::max(acc, tap.1)),
            narrowest: taps
                .iter()
                .fold(f64::INFINITY, |acc, tap| f64::min(acc, tap.1)),
            mass: taps.iter().map(|tap| tap.0).sum(),
            taps,
            scale: sigma * std::f64::consts::SQRT_2,
        })
    }

    fn coverage(&self, dx: f64) -> f64 {
        let reach = dx.abs();
        let limit = SATURATION * self.scale;
        match (reach - self.widest > limit, self.narrowest - reach > limit) {
            (true, _) => 0.0,
            (_, true) => self.mass,
            _ => self
                .taps
                .iter()
                .map(
                    |&(weight, width)| match (reach - width > limit, width - reach > limit) {
                        (true, _) => 0.0,
                        (_, true) => weight,
                        _ => {
                            weight
                                * 0.5
                                * (erf((reach + width) / self.scale)
                                    - erf((reach - width) / self.scale))
                        }
                    },
                )
                .sum(),
        }
    }
}

fn gaussian_coverage(silhouette: &Silhouette, sigma: f64, px: f64, py: f64) -> f64 {
    RowProfile::at(silhouette, sigma, py)
        .map(|row| row.coverage(px - silhouette.center.0))
        .unwrap_or(0.0)
}

pub fn draw_shadow(layer: &mut Layer, shadow: &ShadowInstance) {
    let (x0, y0, x1, y1) = pixel_window(
        expand(shadow.covered(), 1.0),
        shadow.clip,
        layer.width,
        layer.height,
    );
    let color = premultiplied(shadow.color);
    let silhouette = Silhouette::of(shadow);
    let hard = shadow.sigma < HARD_EDGE_SIGMA;
    let hard_shape = QuadShape::new(silhouette.rect(), silhouette.radius as f32, 0.0);
    let element = QuadShape::new(
        shadow.bounds,
        shadow
            .radius
            .max(0.0)
            .min(shadow.bounds.w.min(shadow.bounds.h) * 0.5),
        0.0,
    );
    (y0..y1).for_each(|y| {
        let py = y as f32 + 0.5;
        let profile = match hard {
            true => None,
            false => RowProfile::at(&silhouette, shadow.sigma as f64, py as f64),
        };
        if !hard && profile.is_none() {
            return;
        }
        (x0..x1).for_each(|x| {
            let px = x as f32 + 0.5;
            let cover = clip_coverage(shadow.clip, px, py);
            if cover <= 0.0 {
                return;
            }
            let coverage = match &profile {
                Some(row) => row.coverage(px as f64 - silhouette.center.0) as f32,
                None => hard_shape.coverage(px, py),
            };
            let outside = match shadow.cast {
                ShadowCast::Whole => 1.0,
                ShadowCast::Outside => 1.0 - element.coverage(px, py),
            };
            let strength = coverage * outside * cover * shadow.opacity;
            layer.put(x, y, over(scaled(color, strength), layer.at(x, y)));
        });
    });
}
