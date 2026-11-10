use std::sync::Arc;

use aqua_core::vector::{Brush, FillRule, GradientStop, SvgSource};
use aqua_core::{Oklch, Point, Transform2D};
use aqua_graph::curve_paint::CurvePaint;
use aqua_graph::slug::SlugGlyph;
use resvg::usvg;
use tiny_skia::{Path, Transform};

use super::{VectorBudget, VectorError};

#[derive(Clone, Debug)]
pub enum CurveOp {
    Paint(Box<SlugGlyph>),
    PushOpacity(f32),
    PopOpacity,
}

#[derive(Clone, Debug)]
pub struct CurveScene {
    pub view_box: aqua_core::Rect,
    pub contain: bool,
    pub unit: f32,
    pub ops: Vec<CurveOp>,
}

impl CurveScene {
    pub fn heap_bytes(&self) -> usize {
        self.ops.len() * size_of::<CurveOp>()
            + self
                .ops
                .iter()
                .map(|op| match op {
                    CurveOp::Paint(glyph) => size_of::<SlugGlyph>() + glyph.heap_bytes(),
                    _ => 0,
                })
                .sum::<usize>()
    }
}

fn affine(t: Transform) -> Transform2D {
    Transform2D {
        a: t.sx,
        b: t.ky,
        c: t.kx,
        d: t.sy,
        tx: t.tx,
        ty: t.ty,
    }
}

fn to_transform(t: Transform2D) -> Transform {
    Transform::from_row(t.a, t.b, t.c, t.d, t.tx, t.ty)
}

fn color(value: usvg::Color, alpha: f32, tint: Option<Oklch>) -> Oklch {
    match tint {
        Some(tint) => tint.with_alpha(alpha),
        None => Oklch::from_srgb8(value.red, value.green, value.blue, alpha),
    }
}

fn paint(value: &usvg::Paint, opacity: f32, tint: Option<Oklch>) -> Option<(Brush, Transform)> {
    let stops = |gradient: &usvg::BaseGradient| {
        gradient
            .stops()
            .iter()
            .map(|stop| GradientStop {
                position: stop.offset().get(),
                color: color(stop.color(), stop.opacity().get() * opacity, tint),
            })
            .collect::<Arc<[_]>>()
    };
    match value {
        usvg::Paint::Color(value) => Some((
            Brush::Solid(color(*value, opacity, tint)),
            Transform::identity(),
        )),
        usvg::Paint::LinearGradient(g)
            if g.spread_method() == usvg::SpreadMethod::Pad && g.stops().len() <= 256 =>
        {
            Some((
                Brush::Linear {
                    start: Point::new(g.x1(), g.y1()),
                    end: Point::new(g.x2(), g.y2()),
                    stops: stops(g),
                },
                g.transform(),
            ))
        }
        usvg::Paint::RadialGradient(g)
            if g.spread_method() == usvg::SpreadMethod::Pad
                && g.stops().len() <= 256
                && g.fr().get() == 0.0
                && (g.fx() - g.cx()).hypot(g.fy() - g.cy()) < g.r().get() =>
        {
            Some((
                Brush::Radial {
                    center: Point::new(g.cx(), g.cy()),
                    focal: Point::new(g.fx(), g.fy()),
                    radius: g.r().get(),
                    stops: stops(g),
                },
                g.transform(),
            ))
        }
        _ => None,
    }
}

fn outline(path: &usvg::Path, stroke: bool) -> Option<Path> {
    match stroke {
        false => Some(path.data().clone()),
        true => {
            let style = path.stroke()?.to_tiny_skia();
            let mut outline = path.data().clone();
            if let Some(dash) = &style.dash {
                outline = outline.dash(dash, 16.0)?;
            }
            outline.stroke(&style, 16.0)
        }
    }
}

fn glyph(path: &usvg::Path, stroke: bool, unit: f32, tint: Option<Oklch>) -> Option<SlugGlyph> {
    let (brush, brush_transform) = match stroke {
        true => {
            let style = path.stroke()?;
            paint(style.paint(), style.opacity().get(), tint)?
        }
        false => {
            let fill = path.fill()?;
            paint(fill.paint(), fill.opacity().get(), tint)?
        }
    };
    let normalization = Transform2D::scale(1.0 / unit, 1.0 / unit);
    let world = affine(path.abs_transform()).then(normalization);
    let outline = outline(path, stroke)?.transform(to_transform(world))?;
    let mut glyph = SlugGlyph::from_path(&super::raster::outline_commands(&outline))?;
    glyph.fill_rule = match (!stroke, path.fill().map(|fill| fill.rule())) {
        (true, Some(usvg::FillRule::EvenOdd)) => FillRule::EvenOdd,
        _ => FillRule::NonZero,
    };
    let from_em = to_transform(affine(brush_transform).then(world)).invert()?;
    glyph.paint = Some(CurvePaint {
        brush,
        from_em: affine(from_em),
    });
    Some(glyph)
}

enum Visit<'a> {
    Group(&'a usvg::Group),
    Node(&'a usvg::Node),
    PopOpacity,
}

pub fn svg_curves(
    source: &SvgSource,
    budget: VectorBudget,
) -> Result<Option<CurveScene>, VectorError> {
    if source.source.len() > budget.max_source_bytes {
        return Err(VectorError::TooLarge);
    }
    let document = roxmltree::Document::parse(&source.source)
        .map_err(|error| VectorError::InvalidSvg(error.to_string()))?;
    if document.descendants().count() > budget.max_commands {
        return Err(VectorError::TooLarge);
    }
    if document
        .descendants()
        .flat_map(|node| node.attributes())
        .any(|attribute| {
            attribute.name() == "href"
                && !attribute.value().starts_with('#')
                && !attribute.value().starts_with("data:")
        })
    {
        return Err(VectorError::ExternalResource);
    }
    if document
        .descendants()
        .any(|node| matches!(node.tag_name().name(), "image" | "text" | "foreignObject"))
    {
        return Ok(None);
    }
    let options = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_string: Box::new(|_, _| None),
            resolve_data: Box::new(|_, _, _| None),
        },
        ..Default::default()
    };
    let tree = usvg::Tree::from_str(&source.source, &options)
        .map_err(|error| VectorError::InvalidSvg(error.to_string()))?;
    let size = [tree.size().width(), tree.size().height()];
    let unit = size[0].max(size[1]);
    let mut ops = Vec::new();
    let mut bytes = 0;
    let mut stack = vec![Visit::Group(tree.root())];
    if let Some(tint) = source.tint.filter(|tint| tint.alpha < 1.0) {
        ops.push(CurveOp::PushOpacity(tint.alpha));
    }
    while let Some(visit) = stack.pop() {
        match visit {
            Visit::PopOpacity => ops.push(CurveOp::PopOpacity),
            Visit::Node(usvg::Node::Group(group)) => stack.push(Visit::Group(group)),
            Visit::Group(group) => {
                if group.blend_mode() != usvg::BlendMode::Normal
                    || group.isolate()
                    || group.clip_path().is_some()
                    || group.mask().is_some()
                    || !group.filters().is_empty()
                {
                    return Ok(None);
                }
                if group.opacity().get() < 1.0 {
                    ops.push(CurveOp::PushOpacity(group.opacity().get()));
                    stack.push(Visit::PopOpacity);
                }
                stack.extend(group.children().iter().rev().map(Visit::Node));
            }
            Visit::Node(usvg::Node::Path(path)) => {
                if !path.is_visible() {
                    continue;
                }
                if !path.rendering_mode().use_shape_antialiasing() {
                    return Ok(None);
                }
                let order = match path.paint_order() {
                    usvg::PaintOrder::FillAndStroke => [false, true],
                    usvg::PaintOrder::StrokeAndFill => [true, false],
                };
                for stroke in order {
                    if (stroke && path.stroke().is_none()) || (!stroke && path.fill().is_none()) {
                        continue;
                    }
                    let Some(glyph) = glyph(path, stroke, unit, source.tint) else {
                        return Ok(None);
                    };
                    bytes += size_of::<SlugGlyph>() + glyph.heap_bytes();
                    ops.push(CurveOp::Paint(Box::new(glyph)));
                }
            }
            _ => return Ok(None),
        }
        if ops.len() > 4096 || bytes > budget.retained_source_bytes as usize {
            return Ok(None);
        }
    }
    if source.tint.is_some_and(|tint| tint.alpha < 1.0) {
        ops.push(CurveOp::PopOpacity);
    }
    Ok(Some(CurveScene {
        view_box: aqua_core::Rect {
            x: 0.0,
            y: 0.0,
            w: size[0],
            h: size[1],
        },
        contain: true,
        unit,
        ops,
    }))
}

pub fn drawing_curves(
    drawing: &aqua_core::vector::Drawing,
    budget: VectorBudget,
) -> Result<Option<CurveScene>, VectorError> {
    let unit = drawing.view_box.w.max(drawing.view_box.h);
    if ![
        drawing.view_box.x,
        drawing.view_box.y,
        drawing.view_box.w,
        drawing.view_box.h,
    ]
    .iter()
    .all(|value| value.is_finite())
        || drawing.view_box.w <= 0.0
        || drawing.view_box.h <= 0.0
    {
        return Err(VectorError::InvalidGeometry);
    }
    if drawing
        .paths
        .iter()
        .map(|path| path.commands.len())
        .sum::<usize>()
        > budget.max_commands
    {
        return Err(VectorError::TooLarge);
    }
    let mut ops = Vec::new();
    let mut bytes = 0;
    for path in &drawing.paths {
        for stroke in [false, true] {
            if (stroke && path.stroke.is_none()) || (!stroke && path.fill.is_none()) {
                continue;
            }
            let mut path = path.clone();
            match stroke {
                true => path.fill = None,
                false => path.stroke = None,
            };
            let Some(glyph) = super::curve_outline(&path, unit)? else {
                return Ok(None);
            };
            bytes += size_of::<SlugGlyph>() + glyph.heap_bytes();
            ops.push(CurveOp::Paint(Box::new(glyph)));
            if ops.len() > 4096 || bytes > budget.retained_source_bytes as usize {
                return Ok(None);
            }
        }
    }
    Ok(Some(CurveScene {
        view_box: drawing.view_box,
        contain: false,
        unit,
        ops,
    }))
}
