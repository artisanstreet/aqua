use aqua_core::geometry::{CornerRadii, Rect};
use aqua_core::Oklch;
use aqua_graph::{Gradient, GradientQuad, PaintCommand, Quad, Shadow, ShadowCast};

const DASH_LIMIT: usize = 256;
const SEAM_REACH: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fidelity {
    Exact,
    Approximated,
}

impl Fidelity {
    #[must_use]
    pub fn and(self, other: Fidelity) -> Fidelity {
        match (self, other) {
            (Fidelity::Exact, Fidelity::Exact) => Fidelity::Exact,
            _ => Fidelity::Approximated,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Radii {
    pub top_left: f32,
    pub top_right: f32,
    pub bottom_right: f32,
    pub bottom_left: f32,
}

impl Radii {
    #[must_use]
    pub fn uniform(radius: f32) -> Self {
        Self {
            top_left: radius,
            top_right: radius,
            bottom_right: radius,
            bottom_left: radius,
        }
    }

    #[must_use]
    pub fn scaled(corners: CornerRadii, scale: f32) -> Self {
        Self {
            top_left: corners.top_left.max(0.0) * scale,
            top_right: corners.top_right.max(0.0) * scale,
            bottom_right: corners.bottom_right.max(0.0) * scale,
            bottom_left: corners.bottom_left.max(0.0) * scale,
        }
    }

    #[must_use]
    pub fn is_uniform(self) -> bool {
        self.top_left == self.top_right
            && self.top_left == self.bottom_right
            && self.top_left == self.bottom_left
    }

    #[must_use]
    pub fn max(self) -> f32 {
        self.top_left
            .max(self.top_right)
            .max(self.bottom_right)
            .max(self.bottom_left)
    }

    #[must_use]
    pub fn fitted(self, bounds: Rect) -> Self {
        let ratio = |room: f32, sum: f32| match sum > room && sum > 0.0 {
            true => room / sum,
            false => 1.0,
        };
        let factor = ratio(bounds.w, self.top_left + self.top_right)
            .min(ratio(bounds.w, self.bottom_left + self.bottom_right))
            .min(ratio(bounds.h, self.top_left + self.bottom_left))
            .min(ratio(bounds.h, self.top_right + self.bottom_right))
            .max(0.0);
        Self {
            top_left: self.top_left * factor,
            top_right: self.top_right * factor,
            bottom_right: self.bottom_right * factor,
            bottom_left: self.bottom_left * factor,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sides {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Sides {
    #[must_use]
    pub fn uniform(width: f32) -> Self {
        Self {
            top: width,
            right: width,
            bottom: width,
            left: width,
        }
    }

    #[must_use]
    pub fn is_uniform(self) -> bool {
        self.top == self.right && self.top == self.bottom && self.top == self.left
    }

    #[must_use]
    pub fn max(self) -> f32 {
        self.top.max(self.right).max(self.bottom).max(self.left)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    Solid(Oklch),
    Gradient(Gradient),
}

impl Paint {
    fn visible(self) -> bool {
        match self {
            Paint::Solid(color) => color.alpha > 0.0,
            Paint::Gradient(gradient) => gradient.stops.iter().any(|stop| stop.color.alpha > 0.0),
        }
    }

    fn command(self, bounds: Rect, radius: f32, border_width: f32, border: Oklch) -> PaintCommand {
        match self {
            Paint::Solid(fill) => PaintCommand::Quad(Quad {
                bounds,
                radius,
                border_width,
                fill,
                border_color: border,
            }),
            Paint::Gradient(gradient) => PaintCommand::GradientQuad(GradientQuad {
                quad: Quad {
                    bounds,
                    radius,
                    border_width,
                    fill: gradient.stops[0].color,
                    border_color: border,
                },
                gradient,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Border {
    pub widths: Sides,
    pub color: Oklch,
    pub dashed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowSpec {
    pub dx: f32,
    pub dy: f32,
    pub sigma: f32,
    pub spread: f32,
    pub color: Oklch,
    pub inset: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    pub clip: Option<Rect>,
    pub radius: f32,
}

fn clipped(
    out: &mut Vec<PaintCommand>,
    clip: Option<Rect>,
    body: impl FnOnce(&mut Vec<PaintCommand>),
) {
    match clip {
        Some(rect) => {
            out.push(PaintCommand::PushClip(rect));
            body(out);
            out.push(PaintCommand::PopClip);
        }
        None => body(out),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Pieces {
    items: [Piece; 4],
    len: usize,
}

impl Pieces {
    fn of<const N: usize>(items: [Piece; N]) -> Self {
        let blank = Piece {
            clip: None,
            radius: 0.0,
        };
        let mut all = [blank; 4];
        all[..N].copy_from_slice(&items);
        Self { items: all, len: N }
    }
}

impl std::ops::Deref for Pieces {
    type Target = [Piece];

    fn deref(&self) -> &[Piece] {
        &self.items[..self.len]
    }
}

impl IntoIterator for Pieces {
    type Item = Piece;
    type IntoIter = std::iter::Take<std::array::IntoIter<Piece, 4>>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter().take(self.len)
    }
}

#[must_use]
pub fn pieces(bounds: Rect, radii: Radii, reach: f32) -> Pieces {
    let radii = radii.fitted(bounds);
    let left = bounds.x - reach;
    let top = bounds.y - reach;
    let right = bounds.right() + reach;
    let bottom = bounds.bottom() + reach;
    let mid_x = (bounds.x + bounds.w * 0.5).round();
    let mid_y = (bounds.y + bounds.h * 0.5).round();
    let area = |l: f32, t: f32, r: f32, b: f32| Some(Rect::from_edges(l, t, r, b));
    match (
        radii.is_uniform(),
        radii.top_left == radii.top_right && radii.bottom_left == radii.bottom_right,
        radii.top_left == radii.bottom_left && radii.top_right == radii.bottom_right,
    ) {
        (true, _, _) => Pieces::of([Piece {
            clip: None,
            radius: radii.top_left,
        }]),
        (false, true, _) => Pieces::of([
            Piece {
                clip: area(left, top, right, mid_y),
                radius: radii.top_left,
            },
            Piece {
                clip: area(left, mid_y, right, bottom),
                radius: radii.bottom_left,
            },
        ]),
        (false, false, true) => Pieces::of([
            Piece {
                clip: area(left, top, mid_x, bottom),
                radius: radii.top_left,
            },
            Piece {
                clip: area(mid_x, top, right, bottom),
                radius: radii.top_right,
            },
        ]),
        (false, false, false) => Pieces::of([
            Piece {
                clip: area(left, top, mid_x, mid_y),
                radius: radii.top_left,
            },
            Piece {
                clip: area(mid_x, top, right, mid_y),
                radius: radii.top_right,
            },
            Piece {
                clip: area(mid_x, mid_y, right, bottom),
                radius: radii.bottom_right,
            },
            Piece {
                clip: area(left, mid_y, mid_x, bottom),
                radius: radii.bottom_left,
            },
        ]),
    }
}

fn ring(out: &mut Vec<PaintCommand>, bounds: Rect, radii: Radii, width: f32, color: Oklch) {
    pieces(bounds, radii, SEAM_REACH)
        .into_iter()
        .for_each(|piece| {
            clipped(out, piece.clip, |out| {
                out.push(PaintCommand::Quad(Quad {
                    bounds,
                    radius: piece.radius,
                    border_width: width,
                    fill: Oklch::TRANSPARENT,
                    border_color: color,
                }));
            });
        });
}

fn fill(out: &mut Vec<PaintCommand>, bounds: Rect, radii: Radii, paint: Paint) {
    pieces(bounds, radii, SEAM_REACH)
        .into_iter()
        .for_each(|piece| {
            clipped(out, piece.clip, |out| {
                out.push(paint.command(bounds, piece.radius, 0.0, Oklch::TRANSPARENT));
            });
        });
}

fn fill_with_border(
    out: &mut Vec<PaintCommand>,
    bounds: Rect,
    radii: Radii,
    paint: Option<Paint>,
    width: f32,
    color: Oklch,
) {
    let paint = paint.unwrap_or(Paint::Solid(Oklch::TRANSPARENT));
    pieces(bounds, radii, SEAM_REACH)
        .into_iter()
        .for_each(|piece| {
            clipped(out, piece.clip, |out| {
                out.push(paint.command(bounds, piece.radius, width, color));
            });
        });
}

#[must_use]
pub fn dashes(start: f32, end: f32, width: f32) -> Vec<(f32, f32)> {
    let length = end - start;
    match length > 0.0 && width > 0.0 {
        false => Vec::new(),
        true => {
            let count = ((length / (3.0 * width)).round() as usize).clamp(1, DASH_LIMIT);
            let period = length / count as f32;
            let dash = period * 2.0 / 3.0;
            (0..count)
                .map(|index| (start + index as f32 * period + (period - dash) * 0.5, dash))
                .collect()
        }
    }
}

fn dash_regions(bounds: Rect, radii: Radii, width: f32) -> Vec<Rect> {
    let radii = radii.fitted(bounds);
    let reach = |radius: f32| radius.max(width);
    let (tl, tr, br, bl) = (
        reach(radii.top_left),
        reach(radii.top_right),
        reach(radii.bottom_right),
        reach(radii.bottom_left),
    );
    let corner = |x: f32, y: f32, w: f32, h: f32| Rect { x, y, w, h };
    let mut regions = vec![
        corner(bounds.x, bounds.y, tl, tl),
        corner(bounds.right() - tr, bounds.y, tr, tr),
        corner(bounds.right() - br, bounds.bottom() - br, br, br),
        corner(bounds.x, bounds.bottom() - bl, bl, bl),
    ];
    dashes(bounds.x + tl, bounds.right() - tr, width)
        .into_iter()
        .for_each(|(at, length)| regions.push(corner(at, bounds.y, length, width)));
    dashes(bounds.x + bl, bounds.right() - br, width)
        .into_iter()
        .for_each(|(at, length)| regions.push(corner(at, bounds.bottom() - width, length, width)));
    dashes(bounds.y + tl, bounds.bottom() - bl, width)
        .into_iter()
        .for_each(|(at, length)| regions.push(corner(bounds.x, at, width, length)));
    dashes(bounds.y + tr, bounds.bottom() - br, width)
        .into_iter()
        .for_each(|(at, length)| regions.push(corner(bounds.right() - width, at, width, length)));
    regions
}

fn strips(bounds: Rect, widths: Sides) -> Vec<Rect> {
    let top = widths.top.min(bounds.h);
    let bottom = widths.bottom.min((bounds.h - top).max(0.0));
    let inner_top = bounds.y + top;
    let inner_height = (bounds.h - top - bottom).max(0.0);
    [
        (widths.top > 0.0).then_some(Rect {
            x: bounds.x,
            y: bounds.y,
            w: bounds.w,
            h: top,
        }),
        (widths.bottom > 0.0).then_some(Rect {
            x: bounds.x,
            y: bounds.bottom() - bottom,
            w: bounds.w,
            h: bottom,
        }),
        (widths.left > 0.0).then_some(Rect {
            x: bounds.x,
            y: inner_top,
            w: widths.left.min(bounds.w),
            h: inner_height,
        }),
        (widths.right > 0.0).then_some(Rect {
            x: bounds.right() - widths.right.min(bounds.w),
            y: inner_top,
            w: widths.right.min(bounds.w),
            h: inner_height,
        }),
    ]
    .into_iter()
    .flatten()
    .collect()
}

pub fn emit_box(
    out: &mut Vec<PaintCommand>,
    bounds: Rect,
    radii: Radii,
    paint: Option<Paint>,
    border: Option<Border>,
) {
    let paint = paint.filter(|paint| paint.visible());
    let border = border.filter(|border| border.color.alpha > 0.0 && border.widths.max() > 0.0);
    match (paint, border) {
        (None, None) => {}
        (paint, None) => {
            if let Some(paint) = paint {
                fill(out, bounds, radii, paint);
            }
        }
        (paint, Some(border)) if border.widths.is_uniform() && !border.dashed => {
            fill_with_border(out, bounds, radii, paint, border.widths.top, border.color);
        }
        (paint, Some(border)) => {
            if let Some(paint) = paint {
                fill(out, bounds, radii, paint);
            }
            let widest = border.widths.max();
            let regions = match border.dashed {
                true => dash_regions(bounds, radii, widest),
                false => strips(bounds, border.widths),
            };
            regions.into_iter().for_each(|region| {
                clipped(out, Some(region), |out| {
                    ring(out, bounds, radii, widest, border.color);
                });
            });
        }
    }
}

fn inset_planes(bounds: Rect, spec: ShadowSpec) -> Vec<Rect> {
    let hole = Rect::from_edges(
        bounds.x + spec.dx + spec.spread,
        bounds.y + spec.dy + spec.spread,
        bounds.right() + spec.dx - spec.spread,
        bounds.bottom() + spec.dy - spec.spread,
    );
    let margin = 3.0 * spec.sigma + 2.0;
    let near = |a: f32, b: f32| a.min(b) - margin;
    let far = |a: f32, b: f32| a.max(b) + margin;
    let (low_y, high_y) = (bounds.y - margin, bounds.bottom() + margin);
    let (low_x, high_x) = (bounds.x - margin, bounds.right() + margin);
    vec![
        Rect::from_edges(near(bounds.x, hole.x), low_y, hole.x, high_y),
        Rect::from_edges(
            hole.right(),
            low_y,
            far(bounds.right(), hole.right()),
            high_y,
        ),
        Rect::from_edges(low_x, near(bounds.y, hole.y), high_x, hole.y),
        Rect::from_edges(
            low_x,
            hole.bottom(),
            high_x,
            far(bounds.bottom(), hole.bottom()),
        ),
    ]
}

pub fn emit_shadow(
    out: &mut Vec<PaintCommand>,
    bounds: Rect,
    radii: Radii,
    spec: ShadowSpec,
) -> Fidelity {
    match (spec.color.alpha > 0.0, spec.inset) {
        (false, _) => Fidelity::Exact,
        (true, false) => {
            let reach =
                3.0 * spec.sigma + spec.spread.max(0.0) + spec.dx.abs().max(spec.dy.abs()) + 2.0;
            pieces(bounds, radii, reach).into_iter().for_each(|piece| {
                clipped(out, piece.clip, |out| {
                    out.push(PaintCommand::Shadow(Shadow {
                        bounds,
                        radius: piece.radius,
                        offset: [spec.dx, spec.dy],
                        sigma: spec.sigma,
                        spread: spec.spread,
                        color: spec.color,
                        cast: ShadowCast::Outside,
                    }));
                });
            });
            Fidelity::Exact
        }
        (true, true)
            if spec.sigma == 0.0 && spec.dx == 0.0 && spec.dy == 0.0 && spec.spread >= 0.0 =>
        {
            ring(out, bounds, radii, spec.spread, spec.color);
            Fidelity::Exact
        }
        (true, true) => {
            let radius = radii.fitted(bounds).max();
            out.push(match radius >= 0.75 {
                true => PaintCommand::PushRoundedClip {
                    rect: bounds,
                    radius,
                },
                false => PaintCommand::PushClip(bounds),
            });
            let hollow = bounds.w - 2.0 * spec.spread <= 0.0 || bounds.h - 2.0 * spec.spread <= 0.0;
            match hollow {
                true => out.push(PaintCommand::Quad(Quad {
                    bounds,
                    radius: 0.0,
                    border_width: 0.0,
                    fill: spec.color,
                    border_color: Oklch::TRANSPARENT,
                })),
                false => inset_planes(bounds, spec).into_iter().for_each(|plane| {
                    out.push(PaintCommand::Shadow(Shadow {
                        bounds: plane,
                        radius: 0.0,
                        offset: [0.0, 0.0],
                        sigma: spec.sigma,
                        spread: 0.0,
                        color: spec.color,
                        cast: ShadowCast::Whole,
                    }));
                }),
            }
            out.push(PaintCommand::PopClip);
            match radii.is_uniform() {
                true => Fidelity::Exact,
                false => Fidelity::Approximated,
            }
        }
    }
}
