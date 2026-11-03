use std::sync::Arc;

use crate::{Oklch, Point, Rect, Transform2D};

#[derive(Clone, Debug, PartialEq)]
pub enum PathCommand {
    Move(Point),
    Line(Point),
    Quadratic {
        control: Point,
        to: Point,
    },
    Cubic {
        first: Point,
        second: Point,
        to: Point,
    },
    Arc {
        radii: Point,
        rotation_radians: f32,
        large: bool,
        sweep: bool,
        to: Point,
    },
    Close,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GradientStop {
    pub position: f32,
    pub color: Oklch,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Brush {
    Solid(Oklch),
    Linear {
        start: Point,
        end: Point,
        stops: Arc<[GradientStop]>,
    },
    Radial {
        center: Point,
        focal: Point,
        radius: f32,
        stops: Arc<[GradientStop]>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum LineCap {
    #[default]
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum LineJoin {
    #[default]
    Miter,
    Round,
    Bevel,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub brush: Brush,
    pub width: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    pub dash: Vec<f32>,
    pub dash_offset: f32,
}

impl Stroke {
    pub fn new(brush: Brush, width: f32) -> Self {
        Self {
            brush,
            width,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            dash: Vec::new(),
            dash_offset: 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaintedPath {
    pub commands: Vec<PathCommand>,
    pub fill: Option<Brush>,
    pub fill_rule: FillRule,
    pub stroke: Option<Stroke>,
    pub transform: Transform2D,
}

impl PaintedPath {
    pub fn fill(commands: Vec<PathCommand>, brush: Brush) -> Self {
        Self {
            commands,
            fill: Some(brush),
            fill_rule: FillRule::NonZero,
            stroke: None,
            transform: Transform2D::IDENTITY,
        }
    }

    pub fn stroke(commands: Vec<PathCommand>, stroke: Stroke) -> Self {
        Self {
            commands,
            fill: None,
            fill_rule: FillRule::NonZero,
            stroke: Some(stroke),
            transform: Transform2D::IDENTITY,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Drawing {
    pub view_box: Rect,
    pub paths: Vec<PaintedPath>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SvgSource {
    pub source: Arc<str>,
    pub tint: Option<Oklch>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum VectorSource {
    Drawing(Arc<Drawing>),
    Svg(Arc<SvgSource>),
}

impl VectorSource {
    pub fn svg(source: impl Into<Arc<str>>) -> Self {
        Self::Svg(Arc::new(SvgSource {
            source: source.into(),
            tint: None,
        }))
    }

    pub fn tinted_svg(source: impl Into<Arc<str>>, tint: Oklch) -> Self {
        Self::Svg(Arc::new(SvgSource {
            source: source.into(),
            tint: Some(tint),
        }))
    }
}
