use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use aqua_core::vector::{Brush, Drawing, FillRule, LineCap, LineJoin, PathCommand, VectorSource};
use aqua_core::{Oklch, Point};
use tiny_skia::{Paint, PathBuilder, Pixmap, Shader, Transform};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VectorError {
    InvalidGeometry,
    InvalidGradient,
    InvalidStroke,
    InvalidSvg(String),
    ExternalResource,
    TooLarge,
    OverBudget,
}

impl std::fmt::Display for VectorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "vector rendering: {self:?}")
    }
}

impl std::error::Error for VectorError {}

#[derive(Debug, Clone, Copy)]
pub struct VectorBudget {
    pub bytes: u64,
    pub retained_source_bytes: u64,
    pub entries: usize,
    pub max_side: u32,
    pub max_pixels: u64,
    pub max_source_bytes: usize,
    pub max_commands: usize,
}

impl Default for VectorBudget {
    fn default() -> Self {
        Self {
            bytes: 32 * 1024 * 1024,
            retained_source_bytes: 16 * 1024 * 1024,
            entries: 128,
            max_side: 8192,
            max_pixels: 4 * 1024 * 1024,
            max_source_bytes: 4 * 1024 * 1024,
            max_commands: 1_000_000,
        }
    }
}

pub struct RasterizedVector {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl RasterizedVector {
    pub fn alpha(&self) -> Vec<u8> {
        self.pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| pixel[3])
            .collect()
    }
}

pub fn rasterize(
    source: &VectorSource,
    width: u32,
    height: u32,
    budget: VectorBudget,
) -> Result<RasterizedVector, VectorError> {
    dimensions(width, height, budget)?;
    let mut pixmap = Pixmap::new(width, height).ok_or(VectorError::TooLarge)?;
    match source {
        VectorSource::Drawing(drawing) => render_drawing(drawing, &mut pixmap, budget)?,
        VectorSource::Svg(svg) => {
            if svg.source.len() > budget.max_source_bytes {
                return Err(VectorError::TooLarge);
            }
            let document = roxmltree::Document::parse(&svg.source)
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
            let rejected = Arc::new(AtomicBool::new(false));
            let external = rejected.clone();
            let embedded = rejected.clone();
            let default_data = resvg::usvg::ImageHrefResolver::default_data_resolver();
            let options = resvg::usvg::Options {
                image_href_resolver: resvg::usvg::ImageHrefResolver {
                    resolve_string: Box::new(move |_, _| {
                        external.store(true, Ordering::Relaxed);
                        None
                    }),
                    resolve_data: Box::new(move |mime, data, options| {
                        let dimensions =
                            image::ImageReader::new(std::io::Cursor::new(data.as_slice()))
                                .with_guessed_format()
                                .ok()
                                .and_then(|reader| reader.into_dimensions().ok());
                        if data.len() > budget.max_source_bytes
                            || dimensions.is_some_and(|(width, height)| {
                                u64::from(width) * u64::from(height) > budget.max_pixels
                            })
                        {
                            embedded.store(true, Ordering::Relaxed);
                            return None;
                        }
                        let result = default_data(mime, data, options);
                        if result.is_none() {
                            embedded.store(true, Ordering::Relaxed);
                        }
                        result
                    }),
                },
                ..Default::default()
            };
            let tree = resvg::usvg::Tree::from_str(&svg.source, &options)
                .map_err(|error| VectorError::InvalidSvg(error.to_string()))?;
            if rejected.load(Ordering::Relaxed) {
                return Err(VectorError::ExternalResource);
            }
            let scale =
                (width as f32 / tree.size().width()).min(height as f32 / tree.size().height());
            let translate = Transform::from_translate(
                (width as f32 - tree.size().width() * scale) * 0.5,
                (height as f32 - tree.size().height() * scale) * 0.5,
            );
            resvg::render(
                &tree,
                translate.pre_scale(scale, scale),
                &mut pixmap.as_mut(),
            );
            if let Some(tint) = svg.tint {
                let [r, g, b, a] = tint.to_srgba();
                for pixel in pixmap.pixels_mut() {
                    let alpha = f32::from(pixel.alpha()) / 255.0 * a;
                    let [r, g, b, a] = [r * alpha, g * alpha, b * alpha, alpha]
                        .map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8);
                    *pixel =
                        tiny_skia::PremultipliedColorU8::from_rgba(r.min(a), g.min(a), b.min(a), a)
                            .ok_or(VectorError::InvalidGeometry)?;
                }
            }
        }
    }
    Ok(RasterizedVector {
        width,
        height,
        pixels: pixmap.take(),
    })
}

fn dimensions(width: u32, height: u32, budget: VectorBudget) -> Result<(), VectorError> {
    match width > 0
        && height > 0
        && width <= budget.max_side
        && height <= budget.max_side
        && u64::from(width) * u64::from(height) <= budget.max_pixels
    {
        true => Ok(()),
        false => Err(VectorError::TooLarge),
    }
}

fn color(color: Oklch) -> Result<tiny_skia::Color, VectorError> {
    let [r, g, b, a] = color.to_srgba();
    tiny_skia::Color::from_rgba(r, g, b, a).ok_or(VectorError::InvalidGeometry)
}

fn point(point: Point) -> Result<tiny_skia::Point, VectorError> {
    match point.x.is_finite() && point.y.is_finite() {
        true => Ok(tiny_skia::Point::from_xy(point.x, point.y)),
        false => Err(VectorError::InvalidGeometry),
    }
}

fn shader(brush: &Brush) -> Result<Shader<'static>, VectorError> {
    let stops = |stops: &[aqua_core::vector::GradientStop]| {
        if stops.is_empty()
            || stops
                .iter()
                .any(|stop| !stop.position.is_finite() || !(0.0..=1.0).contains(&stop.position))
            || stops
                .windows(2)
                .any(|stops| stops[0].position > stops[1].position)
        {
            return Err(VectorError::InvalidGradient);
        }
        stops
            .iter()
            .map(|stop| {
                Ok(tiny_skia::GradientStop::new(
                    stop.position,
                    color(stop.color)?,
                ))
            })
            .collect::<Result<Vec<_>, VectorError>>()
    };
    match brush {
        Brush::Solid(value) => Ok(Shader::SolidColor(color(*value)?)),
        Brush::Linear {
            start,
            end,
            stops: colors,
        } => tiny_skia::LinearGradient::new(
            point(*start)?,
            point(*end)?,
            stops(colors)?,
            tiny_skia::SpreadMode::Pad,
            Transform::identity(),
        )
        .ok_or(VectorError::InvalidGradient),
        Brush::Radial {
            center,
            focal,
            radius,
            stops: colors,
        } => {
            if !radius.is_finite() || *radius <= 0.0 {
                return Err(VectorError::InvalidGradient);
            }
            tiny_skia::RadialGradient::new(
                point(*focal)?,
                0.0,
                point(*center)?,
                *radius,
                stops(colors)?,
                tiny_skia::SpreadMode::Pad,
                Transform::identity(),
            )
            .ok_or(VectorError::InvalidGradient)
        }
    }
}

fn path_outline(path: &aqua_core::vector::PaintedPath) -> Result<tiny_skia::Path, VectorError> {
    let mut builder = PathBuilder::new();
    let mut current = None;
    let mut start = None;
    for command in &path.commands {
        match command {
            PathCommand::Move(value) => {
                let value = point(*value)?;
                builder.move_to(value.x, value.y);
                current = Some(value);
                start = current;
            }
            PathCommand::Line(value) => {
                let value = point(*value)?;
                builder.line_to(value.x, value.y);
                current = Some(value);
            }
            PathCommand::Quadratic { control, to } => {
                let control = point(*control)?;
                let to = point(*to)?;
                builder.quad_to(control.x, control.y, to.x, to.y);
                current = Some(to);
            }
            PathCommand::Cubic { first, second, to } => {
                let first = point(*first)?;
                let second = point(*second)?;
                let to = point(*to)?;
                builder.cubic_to(first.x, first.y, second.x, second.y, to.x, to.y);
                current = Some(to);
            }
            PathCommand::Arc {
                radii,
                rotation_radians,
                large,
                sweep,
                to,
            } => {
                let from = current.ok_or(VectorError::InvalidGeometry)?;
                let radii = point(*radii)?;
                let to = point(*to)?;
                if radii.x < 0.0 || radii.y < 0.0 || !rotation_radians.is_finite() {
                    return Err(VectorError::InvalidGeometry);
                }
                let arc = kurbo::SvgArc {
                    from: kurbo::Point::new(f64::from(from.x), f64::from(from.y)),
                    to: kurbo::Point::new(f64::from(to.x), f64::from(to.y)),
                    radii: kurbo::Vec2::new(f64::from(radii.x), f64::from(radii.y)),
                    x_rotation: f64::from(*rotation_radians),
                    large_arc: *large,
                    sweep: *sweep,
                };
                match kurbo::Arc::from_svg_arc(&arc) {
                    Some(arc) => arc.to_cubic_beziers(0.01, |first, second, end| {
                        builder.cubic_to(
                            first.x as f32,
                            first.y as f32,
                            second.x as f32,
                            second.y as f32,
                            end.x as f32,
                            end.y as f32,
                        )
                    }),
                    None => builder.line_to(to.x, to.y),
                }
                current = Some(to);
            }
            PathCommand::Close => {
                builder.close();
                current = start;
            }
        }
    }
    builder.finish().ok_or(VectorError::InvalidGeometry)
}

fn stroke_style(stroke: &aqua_core::vector::Stroke) -> Result<tiny_skia::Stroke, VectorError> {
    if !stroke.width.is_finite()
        || stroke.width <= 0.0
        || !stroke.dash_offset.is_finite()
        || stroke
            .dash
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return Err(VectorError::InvalidStroke);
    }
    let dash = match stroke.dash.is_empty() {
        true => None,
        false => Some(
            tiny_skia::StrokeDash::new(stroke.dash.clone(), stroke.dash_offset)
                .ok_or(VectorError::InvalidStroke)?,
        ),
    };
    let style = tiny_skia::Stroke {
        width: stroke.width,
        line_cap: match stroke.cap {
            LineCap::Butt => tiny_skia::LineCap::Butt,
            LineCap::Round => tiny_skia::LineCap::Round,
            LineCap::Square => tiny_skia::LineCap::Square,
        },
        line_join: match stroke.join {
            LineJoin::Miter => tiny_skia::LineJoin::Miter,
            LineJoin::Round => tiny_skia::LineJoin::Round,
            LineJoin::Bevel => tiny_skia::LineJoin::Bevel,
        },
        dash,
        ..Default::default()
    };
    Ok(style)
}

pub(super) fn outline_commands(outline: &tiny_skia::Path) -> Vec<aqua_graph::slug::PathCommand> {
    use aqua_graph::slug::PathCommand as CurveCommand;
    use tiny_skia::PathSegment;
    outline
        .segments()
        .map(|segment| match segment {
            PathSegment::MoveTo(p) => CurveCommand::Move([p.x, p.y]),
            PathSegment::LineTo(p) => CurveCommand::Line([p.x, p.y]),
            PathSegment::QuadTo(control, p) => {
                CurveCommand::Quad([control.x, control.y], [p.x, p.y])
            }
            PathSegment::CubicTo(a, b, p) => {
                CurveCommand::Cubic([a.x, a.y], [b.x, b.y], [p.x, p.y])
            }
            PathSegment::Close => CurveCommand::Close,
        })
        .collect()
}

pub fn curve_outline(
    path: &aqua_core::vector::PaintedPath,
    unit: f32,
) -> Result<Option<aqua_graph::slug::SlugGlyph>, VectorError> {
    use aqua_graph::slug::SlugGlyph;
    if !unit.is_finite()
        || unit <= 0.0
        || path.commands.len() > VectorBudget::default().max_commands
    {
        return Err(VectorError::InvalidGeometry);
    }
    if path.fill.is_some() && path.stroke.is_some() {
        return Ok(None);
    }
    let mut outline = path_outline(path)?;
    if let Some(stroke) = &path.stroke {
        let style = stroke_style(stroke)?;
        if let Some(dash) = &style.dash {
            outline = outline.dash(dash, 1.0).ok_or(VectorError::InvalidStroke)?;
        }
        outline = outline
            .stroke(&style, 1.0)
            .ok_or(VectorError::InvalidStroke)?;
    }
    let matrix = path.transform;
    let transform = Transform::from_scale(1.0 / unit, 1.0 / unit).pre_concat(Transform::from_row(
        matrix.a, matrix.b, matrix.c, matrix.d, matrix.tx, matrix.ty,
    ));
    let outline = outline
        .transform(transform)
        .ok_or(VectorError::InvalidGeometry)?;
    let commands = outline_commands(&outline);
    let Some(brush) = path
        .fill
        .as_ref()
        .or_else(|| path.stroke.as_ref().map(|stroke| &stroke.brush))
    else {
        return Ok(None);
    };
    let prepared = shader(brush)?;
    let brush = match (prepared, brush) {
        (Shader::SolidColor(_), Brush::Linear { stops, .. } | Brush::Radial { stops, .. }) => {
            Brush::Solid(stops.last().ok_or(VectorError::InvalidGradient)?.color)
        }
        _ => brush.clone(),
    };
    let stops = match &brush {
        Brush::Solid(_) => 1,
        Brush::Linear { stops, .. } | Brush::Radial { stops, .. } => stops.len(),
    };
    if stops > 256 {
        return Ok(None);
    }
    if let Brush::Radial {
        center,
        focal,
        radius,
        ..
    } = &brush
    {
        if (focal.x - center.x).hypot(focal.y - center.y) >= *radius {
            return Ok(None);
        }
    }
    let Some(inverse) = transform.invert() else {
        return Ok(None);
    };
    let mut glyph = SlugGlyph::from_path(&commands);
    if let Some(glyph) = &mut glyph {
        glyph.fill_rule = match path.fill.is_some() {
            true => path.fill_rule,
            false => FillRule::NonZero,
        };
        glyph.paint = Some(aqua_graph::curve_paint::CurvePaint {
            brush: brush.clone(),
            from_em: aqua_core::Transform2D {
                a: inverse.sx,
                b: inverse.ky,
                c: inverse.kx,
                d: inverse.sy,
                tx: inverse.tx,
                ty: inverse.ty,
            },
        });
    }
    Ok(glyph)
}

fn render_drawing(
    drawing: &Drawing,
    pixmap: &mut Pixmap,
    budget: VectorBudget,
) -> Result<(), VectorError> {
    let bounds = drawing.view_box;
    if ![bounds.x, bounds.y, bounds.w, bounds.h]
        .iter()
        .all(|value| value.is_finite())
        || bounds.w <= 0.0
        || bounds.h <= 0.0
        || drawing
            .paths
            .iter()
            .map(|path| path.commands.len())
            .sum::<usize>()
            > budget.max_commands
    {
        return Err(VectorError::InvalidGeometry);
    }
    let base = Transform::from_scale(
        pixmap.width() as f32 / bounds.w,
        pixmap.height() as f32 / bounds.h,
    )
    .pre_translate(-bounds.x, -bounds.y);
    for path in &drawing.paths {
        let outline = path_outline(path)?;
        let matrix = path.transform;
        if ![matrix.a, matrix.b, matrix.c, matrix.d, matrix.tx, matrix.ty]
            .iter()
            .all(|value| value.is_finite())
        {
            return Err(VectorError::InvalidGeometry);
        }
        let transform = base.pre_concat(Transform::from_row(
            matrix.a, matrix.b, matrix.c, matrix.d, matrix.tx, matrix.ty,
        ));
        if !transform.is_finite() {
            return Err(VectorError::InvalidGeometry);
        }
        if let Some(fill) = &path.fill {
            let paint = Paint {
                shader: shader(fill)?,
                anti_alias: true,
                ..Default::default()
            };
            let rule = match path.fill_rule {
                FillRule::NonZero => tiny_skia::FillRule::Winding,
                FillRule::EvenOdd => tiny_skia::FillRule::EvenOdd,
            };
            pixmap.fill_path(&outline, &paint, rule, transform, None);
        }
        if let Some(stroke) = &path.stroke {
            let style = stroke_style(stroke)?;
            let paint = Paint {
                shader: shader(&stroke.brush)?,
                anti_alias: true,
                ..Default::default()
            };
            pixmap.stroke_path(&outline, &paint, &style, transform, None);
        }
    }
    Ok(())
}

pub fn retained_source_bytes(source: &VectorSource) -> u64 {
    let brush_bytes = |brush: &Brush| match brush {
        Brush::Solid(_) => 0,
        Brush::Linear { stops, .. } | Brush::Radial { stops, .. } => {
            std::mem::size_of_val(stops.as_ref()) as u64 + 16
        }
    };
    match source {
        VectorSource::Svg(svg) => {
            svg.source.len() as u64 + std::mem::size_of_val(svg.as_ref()) as u64 + 32
        }
        VectorSource::Drawing(drawing) => {
            let mut bytes = std::mem::size_of_val(drawing.as_ref()) as u64
                + 16
                + (drawing.paths.capacity() * std::mem::size_of::<aqua_core::vector::PaintedPath>())
                    as u64;
            for path in &drawing.paths {
                bytes = bytes.saturating_add(
                    (path.commands.capacity() * std::mem::size_of::<PathCommand>()) as u64,
                );
                if let Some(fill) = &path.fill {
                    bytes = bytes.saturating_add(brush_bytes(fill));
                }
                if let Some(stroke) = &path.stroke {
                    bytes = bytes.saturating_add(
                        brush_bytes(&stroke.brush)
                            + (stroke.dash.capacity() * std::mem::size_of::<f32>()) as u64,
                    );
                }
            }
            bytes
        }
    }
}
