use aqua_core::geometry::Rect;
use aqua_graph::{Extent, FramePlan, Linear2, PassKind, TextureRef};
use aqua_protocol::SurfacePixels;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tolerance {
    pub interior: u8,
    pub soft: u8,
    pub edge: u8,
    pub edge_band: u8,
}

impl Default for Tolerance {
    fn default() -> Self {
        Self {
            interior: 4,
            soft: 8,
            edge: 24,
            edge_band: 2,
        }
    }
}

impl Tolerance {
    pub const EXACT: Tolerance = Tolerance {
        interior: 0,
        soft: 0,
        edge: 0,
        edge_band: 0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failure {
    pub x: usize,
    pub y: usize,
    pub channel: usize,
    pub expected: u8,
    pub actual: u8,
    pub edge: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comparison {
    pub interior_pixels: usize,
    pub edge_pixels: usize,
    pub soft_pixels: usize,
    pub interior_max: u8,
    pub soft_max: u8,
    pub edge_max: u8,
    pub failing_samples: usize,
    pub failures: Vec<Failure>,
}

impl Comparison {
    pub const RECORDED_FAILURES: usize = 16;

    pub fn passed(&self) -> bool {
        self.failing_samples == 0
    }

    pub fn identical(&self) -> bool {
        self.interior_max == 0 && self.soft_max == 0 && self.edge_max == 0
    }
}

struct Footprint {
    bounds: Rect,
    border: f32,
    radius: f32,
    clip: Option<Rect>,
    soft: Option<Rect>,
    linear: Linear2,
}

fn footprints(plan: &FramePlan) -> Vec<Footprint> {
    let hard = |bounds: Rect, radius: f32, border: f32, clip: Option<Rect>| Footprint {
        bounds,
        border,
        radius,
        clip,
        soft: None,
        linear: Linear2::IDENTITY,
    };
    plan.passes
        .iter()
        .flat_map(|pass| match &pass.kind {
            PassKind::Solids { instances } => instances
                .iter()
                .map(|instance| hard(instance.bounds, 0.0, 0.0, None))
                .collect(),
            PassKind::Instanced { instances, .. } => instances
                .iter()
                .map(|q| Footprint {
                    linear: q.linear,
                    ..hard(q.bounds, q.radius, q.border_width, q.clip)
                })
                .collect::<Vec<_>>(),
            PassKind::Shadows { instances } => instances
                .iter()
                .map(|s| Footprint {
                    soft: Some(s.covered()),
                    ..hard(s.bounds, 0.0, 0.0, s.clip)
                })
                .collect(),
            PassKind::Glyphs { instances, .. } => instances
                .iter()
                .map(|g| hard(g.bounds, 0.0, 0.0, g.clip))
                .collect(),
            PassKind::Slug { instances, .. } => instances
                .iter()
                .map(|g| Footprint {
                    soft: Some(g.covered()),
                    ..hard(g.covered(), 0.0, 0.0, g.clip)
                })
                .collect(),
            PassKind::Images { instances } => instances
                .iter()
                .flat_map(|i| {
                    let image = Footprint {
                        soft: matches!(i.source, TextureRef::Transient(_)).then_some(i.covered()),
                        linear: i.linear,
                        ..hard(i.bounds, i.radius, 0.0, i.clip)
                    };
                    let rounded_clip = i
                        .clip
                        .filter(|_| i.clip_radius > 0.0)
                        .map(|clip| hard(clip, i.clip_radius, 0.0, None));
                    std::iter::once(image).chain(rounded_clip)
                })
                .collect(),
            PassKind::Fullscreen { .. }
            | PassKind::Filter { .. }
            | PassKind::Compute { .. }
            | PassKind::Copy { .. }
            | PassKind::Custom(_) => Vec::new(),
        })
        .collect()
}

fn shape_rects(footprint: &Footprint) -> Vec<Rect> {
    let bounds = footprint.bounds;
    let inset = footprint.border;
    let inner = Rect {
        x: bounds.x + inset,
        y: bounds.y + inset,
        w: (bounds.w - inset * 2.0).max(0.0),
        h: (bounds.h - inset * 2.0).max(0.0),
    };
    [Some(bounds), (inset > 0.0).then_some(inner)]
        .into_iter()
        .flatten()
        .collect()
}

fn to_local(footprint: &Footprint, x: f32, y: f32) -> (f32, f32) {
    match footprint.linear.inverse() {
        Some(inverse) if !footprint.linear.is_identity() => {
            let center = footprint.bounds.center();
            let moved = inverse.apply([x - center.x, y - center.y]);
            (center.x + moved[0], center.y + moved[1])
        }
        _ => (x, y),
    }
}

fn near_boundary(rect: Rect, x: f32, y: f32, band: f32) -> bool {
    let within_x = x >= rect.x - band && x <= rect.right() + band;
    let within_y = y >= rect.y - band && y <= rect.bottom() + band;
    let on_vertical = ((x - rect.x).abs() <= band || (x - rect.right()).abs() <= band) && within_y;
    let on_horizontal =
        ((y - rect.y).abs() <= band || (y - rect.bottom()).abs() <= band) && within_x;
    on_vertical || on_horizontal
}

fn near_rounded_corner(footprint: &Footprint, x: f32, y: f32, band: f32) -> bool {
    let radius = footprint.radius;
    let bounds = footprint.bounds;
    radius > 0.0
        && x >= bounds.x - band
        && x <= bounds.right() + band
        && y >= bounds.y - band
        && y <= bounds.bottom() + band
        && ((x < bounds.x + radius + band) || (x > bounds.right() - radius - band))
        && ((y < bounds.y + radius + band) || (y > bounds.bottom() - radius - band))
}

fn pixel_range(low: f32, high: f32, limit: u32) -> std::ops::Range<usize> {
    let start = low.floor().max(0.0) as usize;
    let end = (high.ceil().max(0.0) as usize).min(limit as usize);
    start.min(end)..end
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Interior,
    Soft,
    Edge,
}

pub fn class_mask(plan: &FramePlan, band: f32) -> Vec<Class> {
    let Extent { width, height } = plan.extent;
    let mut mask = vec![Class::Interior; width as usize * height as usize];
    let mut mark = |x: usize, y: usize, class: Class| {
        let slot = &mut mask[y * width as usize + x];
        *slot = (*slot).max(class);
    };
    for footprint in footprints(plan) {
        if let Some(soft) = footprint.soft {
            for y in pixel_range(soft.y, soft.bottom(), height) {
                for x in pixel_range(soft.x, soft.right(), width) {
                    mark(x, y, Class::Soft);
                }
            }
        }
        let reach = footprint.linear.cover(footprint.bounds);
        let rects = shape_rects(&footprint);
        let scale = match footprint.linear.is_identity() {
            true => 1.0,
            false => footprint.linear.scale_factor().max(1.0e-3),
        };
        let local_band = band / scale + if scale == 1.0 { 0.0 } else { 0.5 };
        let ys = pixel_range(reach.y - band - 1.0, reach.bottom() + band + 1.0, height);
        let xs = pixel_range(reach.x - band - 1.0, reach.right() + band + 1.0, width);
        for y in ys {
            for x in xs.clone() {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let (lx, ly) = to_local(&footprint, px, py);
                let near = rects
                    .iter()
                    .any(|rect| near_boundary(*rect, lx, ly, local_band))
                    || near_rounded_corner(&footprint, lx, ly, local_band)
                    || footprint
                        .clip
                        .is_some_and(|clip| near_boundary(clip, px, py, band));
                if near {
                    mark(x, y, Class::Edge);
                }
            }
        }
    }
    mask
}

pub fn compare(
    plan: &FramePlan,
    expected: &SurfacePixels,
    actual: &SurfacePixels,
    tolerance: Tolerance,
) -> Comparison {
    let width = expected.extent.width as usize;
    let height = expected.extent.height as usize;
    let mask = class_mask(plan, tolerance.edge_band as f32);
    let mut comparison = Comparison {
        interior_pixels: 0,
        edge_pixels: 0,
        soft_pixels: 0,
        interior_max: 0,
        soft_max: 0,
        edge_max: 0,
        failing_samples: 0,
        failures: Vec::new(),
    };
    for y in 0..height {
        for x in 0..width {
            let class = mask.get(y * width + x).copied().unwrap_or(Class::Interior);
            let edge = class == Class::Edge;
            let limit = match class {
                Class::Edge => tolerance.edge,
                Class::Soft => tolerance.soft,
                Class::Interior => tolerance.interior,
            };
            match class {
                Class::Edge => comparison.edge_pixels += 1,
                Class::Soft => comparison.soft_pixels += 1,
                Class::Interior => comparison.interior_pixels += 1,
            }
            for channel in 0..4 {
                let index = (y * width + x) * 4 + channel;
                let (want, got) = (expected.rgba8[index], actual.rgba8[index]);
                let difference = want.abs_diff(got);
                match class {
                    Class::Edge => comparison.edge_max = comparison.edge_max.max(difference),
                    Class::Soft => comparison.soft_max = comparison.soft_max.max(difference),
                    Class::Interior => {
                        comparison.interior_max = comparison.interior_max.max(difference)
                    }
                }
                if difference > limit {
                    comparison.failing_samples += 1;
                    if comparison.failures.len() < Comparison::RECORDED_FAILURES {
                        comparison.failures.push(Failure {
                            x,
                            y,
                            channel,
                            expected: want,
                            actual: got,
                            edge,
                        });
                    }
                }
            }
        }
    }
    comparison
}
