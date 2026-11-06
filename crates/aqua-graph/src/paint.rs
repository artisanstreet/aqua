use std::ops::Deref;
use std::sync::Arc;

use aqua_core::{geometry::Rect, Oklch};

use crate::analysis::{intersect, union};
use crate::blur::{blur_chain, BlurRequest};
use crate::graph::{PlanLimits, RenderGraph};
use crate::linear::Linear2;
use crate::plan::{
    Blend, ColorMatrix, CustomPassSpec, Extent, FilterParams, FramePlan, GlyphInstance, Gradient,
    ImageInstance, LoadOp, Pass, PassKind, QuadInstance, ShaderId, ShadowCast, ShadowInstance,
    TextureFormat, TextureHandle, TextureRef,
};
use crate::slug::SlugInstance;
use crate::validate::PlanError;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    pub bounds: Rect,
    pub radius: f32,
    pub border_width: f32,
    pub fill: Oklch,
    pub border_color: Oklch,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientQuad {
    pub quad: Quad,
    pub gradient: Gradient,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub bounds: Rect,
    pub radius: f32,
    pub offset: [f32; 2],
    pub sigma: f32,
    pub spread: f32,
    pub color: Oklch,
    pub cast: ShadowCast,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    pub bounds: Rect,
    pub texels: Rect,
    pub color: Oklch,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlyphList(Arc<Vec<Glyph>>);

impl GlyphList {
    pub fn retain(&mut self, keep: impl FnMut(&Glyph) -> bool) {
        Arc::make_mut(&mut self.0).retain(keep);
    }

    #[must_use]
    pub fn shares_storage(&self, other: &GlyphList) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Deref for GlyphList {
    type Target = [Glyph];

    fn deref(&self) -> &[Glyph] {
        &self.0
    }
}

impl From<Vec<Glyph>> for GlyphList {
    fn from(glyphs: Vec<Glyph>) -> Self {
        Self(Arc::new(glyphs))
    }
}

impl FromIterator<Glyph> for GlyphList {
    fn from_iter<I: IntoIterator<Item = Glyph>>(glyphs: I) -> Self {
        Self(Arc::new(glyphs.into_iter().collect()))
    }
}

impl PartialEq<Vec<Glyph>> for GlyphList {
    fn eq(&self, other: &Vec<Glyph>) -> bool {
        self.0.as_slice() == other.as_slice()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlyphRun {
    pub atlas: TextureHandle,
    pub glyphs: GlyphList,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlugList(Arc<Vec<SlugInstance>>);

impl Deref for SlugList {
    type Target = [SlugInstance];

    fn deref(&self) -> &[SlugInstance] {
        &self.0
    }
}

impl From<Vec<SlugInstance>> for SlugList {
    fn from(glyphs: Vec<SlugInstance>) -> Self {
        Self(Arc::new(glyphs))
    }
}

impl FromIterator<SlugInstance> for SlugList {
    fn from_iter<I: IntoIterator<Item = SlugInstance>>(glyphs: I) -> Self {
        Self(Arc::new(glyphs.into_iter().collect()))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SlugRun {
    pub store: TextureHandle,
    pub glyphs: SlugList,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Image {
    pub source: TextureHandle,
    pub bounds: Rect,
    pub texels: Rect,
    pub radius: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BackdropBlur {
    pub bounds: Rect,
    pub radius: f32,
    pub corner_radius: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PaintCommand {
    Quad(Quad),
    GradientQuad(GradientQuad),
    Shadow(Shadow),
    Glyphs(GlyphRun),
    Image(Image),
    Slug(SlugRun),
    Linear {
        linear: Linear2,
        leaf: Box<PaintCommand>,
    },
    BackdropBlur(BackdropBlur),
    PushClip(Rect),
    PushRoundedClip {
        rect: Rect,
        radius: f32,
    },
    PopClip,
    PushOpacity(f32),
    PopOpacity,
    PushContentBlur {
        bounds: Rect,
        radius: f32,
    },
    PopContentBlur,
    PushColorMatrix {
        bounds: Rect,
        matrix: ColorMatrix,
    },
    PopColorMatrix,
    PushCustomEffect {
        bounds: Rect,
        spec: CustomPassSpec,
    },
    PopCustomEffect,
}

#[derive(Clone, Debug)]
enum Effect {
    Blur(f32),
    ColorMatrix(ColorMatrix),
    Custom(CustomPassSpec),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EffectKind {
    Blur,
    ColorMatrix,
    Custom,
}

impl Effect {
    fn kind(&self) -> EffectKind {
        match self {
            Effect::Blur(_) => EffectKind::Blur,
            Effect::ColorMatrix(_) => EffectKind::ColorMatrix,
            Effect::Custom(_) => EffectKind::Custom,
        }
    }
}

#[derive(Clone, Debug)]
enum Leaf {
    Quad(Quad, Option<Gradient>, Linear2),
    Shadow(Shadow),
    Glyphs(GlyphRun),
    Slug(SlugRun),
    Image(Image, Linear2),
    Backdrop(BackdropBlur),
}

#[derive(Clone, Debug)]
enum Node {
    Leaf(Leaf),
    Clip(Rect, Vec<Node>),
    RoundedClip {
        rect: Rect,
        radius: f32,
        children: Vec<Node>,
    },
    Group(f32, Vec<Node>),
    Effect {
        bounds: Rect,
        effect: Effect,
        children: Vec<Node>,
    },
}

enum Frame {
    Clip(Rect),
    RoundedClip { rect: Rect, radius: f32 },
    Group(f32),
    Effect { bounds: Rect, effect: Effect },
}

type Stack = Vec<(Option<Frame>, Vec<Node>)>;

fn pop_effect(stack: &mut Stack, expected: EffectKind) -> Result<(), PlanError> {
    let (frame, children) = stack.pop().ok_or(PlanError::UnbalancedPop)?;
    match frame {
        Some(Frame::Effect { bounds, effect }) if effect.kind() == expected => {
            stack
                .last_mut()
                .ok_or(PlanError::UnbalancedPop)?
                .1
                .push(Node::Effect {
                    bounds,
                    effect,
                    children,
                });
            Ok(())
        }
        _ => Err(PlanError::UnbalancedPop),
    }
}

fn parse(commands: &[PaintCommand]) -> Result<Vec<Node>, PlanError> {
    let mut stack: Vec<(Option<Frame>, Vec<Node>)> = vec![(None, Vec::new())];
    let push = |stack: &mut Vec<(Option<Frame>, Vec<Node>)>, node: Node| {
        stack.last_mut().expect("root frame").1.push(node)
    };
    for command in commands {
        match command {
            PaintCommand::Quad(quad) => push(
                &mut stack,
                Node::Leaf(Leaf::Quad(*quad, None, Linear2::IDENTITY)),
            ),
            PaintCommand::GradientQuad(item) => push(
                &mut stack,
                Node::Leaf(Leaf::Quad(
                    item.quad,
                    Some(item.gradient),
                    Linear2::IDENTITY,
                )),
            ),
            PaintCommand::Slug(run) => push(&mut stack, Node::Leaf(Leaf::Slug(run.clone()))),
            PaintCommand::Linear { linear, leaf } => {
                let leaf = match &**leaf {
                    PaintCommand::Quad(quad) => Leaf::Quad(*quad, None, *linear),
                    PaintCommand::GradientQuad(item) => {
                        Leaf::Quad(item.quad, Some(item.gradient), *linear)
                    }
                    PaintCommand::Image(image) => Leaf::Image(*image, *linear),
                    _ => return Err(PlanError::NotTransformable),
                };
                push(&mut stack, Node::Leaf(leaf));
            }
            PaintCommand::Shadow(shadow) => push(&mut stack, Node::Leaf(Leaf::Shadow(*shadow))),
            PaintCommand::Glyphs(run) => push(&mut stack, Node::Leaf(Leaf::Glyphs(run.clone()))),
            PaintCommand::Image(image) => push(
                &mut stack,
                Node::Leaf(Leaf::Image(*image, Linear2::IDENTITY)),
            ),
            PaintCommand::BackdropBlur(blur) => push(&mut stack, Node::Leaf(Leaf::Backdrop(*blur))),
            PaintCommand::PushClip(rect) => stack.push((Some(Frame::Clip(*rect)), Vec::new())),
            PaintCommand::PushRoundedClip { rect, radius } => stack.push((
                Some(Frame::RoundedClip {
                    rect: *rect,
                    radius: *radius,
                }),
                Vec::new(),
            )),
            PaintCommand::PushOpacity(opacity) => {
                stack.push((Some(Frame::Group(*opacity)), Vec::new()))
            }
            PaintCommand::PushContentBlur { bounds, radius } => stack.push((
                Some(Frame::Effect {
                    bounds: *bounds,
                    effect: Effect::Blur(*radius),
                }),
                Vec::new(),
            )),
            PaintCommand::PushColorMatrix { bounds, matrix } => stack.push((
                Some(Frame::Effect {
                    bounds: *bounds,
                    effect: Effect::ColorMatrix(*matrix),
                }),
                Vec::new(),
            )),
            PaintCommand::PushCustomEffect { bounds, spec } => stack.push((
                Some(Frame::Effect {
                    bounds: *bounds,
                    effect: Effect::Custom(spec.clone()),
                }),
                Vec::new(),
            )),
            PaintCommand::PopClip => {
                let (frame, children) = stack.pop().ok_or(PlanError::UnbalancedPop)?;
                match frame {
                    Some(Frame::Clip(rect)) => push(&mut stack, Node::Clip(rect, children)),
                    Some(Frame::RoundedClip { rect, radius }) => push(
                        &mut stack,
                        Node::RoundedClip {
                            rect,
                            radius,
                            children,
                        },
                    ),
                    _ => return Err(PlanError::UnbalancedPop),
                }
            }
            PaintCommand::PopOpacity => {
                let (frame, children) = stack.pop().ok_or(PlanError::UnbalancedPop)?;
                match frame {
                    Some(Frame::Group(opacity)) => push(&mut stack, Node::Group(opacity, children)),
                    _ => return Err(PlanError::UnbalancedPop),
                }
            }
            PaintCommand::PopContentBlur => pop_effect(&mut stack, EffectKind::Blur)?,
            PaintCommand::PopColorMatrix => pop_effect(&mut stack, EffectKind::ColorMatrix)?,
            PaintCommand::PopCustomEffect => pop_effect(&mut stack, EffectKind::Custom)?,
        }
        if stack.is_empty() {
            return Err(PlanError::UnbalancedPop);
        }
    }
    match stack.len() {
        1 => Ok(stack.pop().expect("root frame").1),
        _ => Err(PlanError::UnclosedScope),
    }
}

fn leaf_bounds(leaf: &Leaf) -> Option<Rect> {
    match leaf {
        Leaf::Quad(quad, _, linear) => Some(linear.cover(quad.bounds)),
        Leaf::Shadow(shadow) => Some(
            ShadowInstance {
                bounds: shadow.bounds,
                radius: shadow.radius,
                offset: shadow.offset,
                sigma: shadow.sigma,
                spread: shadow.spread,
                color: shadow.color,
                clip: None,
                opacity: 1.0,
                cast: shadow.cast,
            }
            .covered(),
        ),
        Leaf::Glyphs(run) => run.glyphs.iter().map(|glyph| glyph.bounds).reduce(union),
        Leaf::Slug(run) => run.glyphs.iter().map(SlugInstance::covered).reduce(union),
        Leaf::Image(image, linear) => Some(linear.cover(image.bounds)),
        Leaf::Backdrop(blur) => Some(blur.bounds),
    }
}

fn clipped(rect: Rect, clip: Option<Rect>) -> Option<Rect> {
    match clip {
        Some(clip) => intersect(rect, clip),
        None => Some(rect),
    }
}

fn units(nodes: &[Node], clip: Option<Rect>, out: &mut Vec<Rect>) {
    for node in nodes {
        match node {
            Node::Leaf(leaf) => {
                if let Some(rect) = leaf_bounds(leaf).and_then(|rect| clipped(rect, clip)) {
                    out.push(rect);
                }
            }
            Node::Clip(rect, children) => {
                let next = clipped(*rect, clip);
                if let Some(next) = next {
                    units(children, Some(next), out);
                }
            }
            Node::RoundedClip { rect, children, .. } => {
                if let Some(next) = clipped(*rect, clip) {
                    let mut inner = Vec::new();
                    units(children, Some(next), &mut inner);
                    if let Some(total) = inner.into_iter().reduce(union) {
                        out.push(total);
                    }
                }
            }
            Node::Group(_, children) => {
                let mut inner = Vec::new();
                units(children, clip, &mut inner);
                if let Some(total) = inner.into_iter().reduce(union) {
                    out.push(total);
                }
            }
            Node::Effect {
                bounds, children, ..
            } => {
                let mut inner = Vec::new();
                units(children, clip, &mut inner);
                if !inner.is_empty() {
                    if let Some(rect) = clipped(*bounds, clip) {
                        out.push(rect);
                    }
                }
            }
        }
    }
}

const FOLD_LIMIT: usize = 512;
const CORNER_SPLIT_MIN_PIXELS: f32 = 65536.0;

struct CornerSplit {
    bands: [Rect; 3],
    corners: [Rect; 4],
}

fn corner_split(rect: Rect, radius: f32, bounded: Rect) -> Option<CornerSplit> {
    if bounded.w * bounded.h < CORNER_SPLIT_MIN_PIXELS {
        return None;
    }
    let (left, top, right, bottom) = (rect.x, rect.y, rect.right(), rect.bottom());
    let inner_left = (left + radius).ceil();
    let inner_top = (top + radius).ceil();
    let inner_right = (right - radius).floor();
    let inner_bottom = (bottom - radius).floor();
    (inner_left < inner_right && inner_top < inner_bottom).then(|| CornerSplit {
        bands: [
            Rect::from_edges(inner_left, top, inner_right, inner_top),
            Rect::from_edges(left, inner_top, right, inner_bottom),
            Rect::from_edges(inner_left, inner_bottom, inner_right, bottom),
        ],
        corners: [
            Rect::from_edges(left, top, inner_left, inner_top),
            Rect::from_edges(inner_right, top, right, inner_top),
            Rect::from_edges(left, inner_bottom, inner_left, bottom),
            Rect::from_edges(inner_right, inner_bottom, right, bottom),
        ],
    })
}

fn backdrops(nodes: &[Node], clip: Option<Rect>, out: &mut Vec<Rect>) {
    for node in nodes {
        match node {
            Node::Leaf(Leaf::Backdrop(blur)) => out.extend(clipped(blur.bounds, clip)),
            Node::Leaf(_) => {}
            Node::Clip(rect, children) | Node::RoundedClip { rect, children, .. } => {
                if let Some(next) = clipped(*rect, clip) {
                    backdrops(children, Some(next), out);
                }
            }
            Node::Group(_, children) | Node::Effect { children, .. } => {
                backdrops(children, clip, out)
            }
        }
    }
}

fn reads_backdrop(children: &[Node], clip: Option<Rect>) -> bool {
    let mut found = Vec::new();
    backdrops(children, clip, &mut found);
    !found.is_empty()
}

fn backdrops_inside(children: &[Node], clip: Option<Rect>, band: Rect) -> bool {
    let mut found = Vec::new();
    backdrops(children, clip, &mut found);
    found.into_iter().all(|rect| {
        rect.x >= band.x
            && rect.y >= band.y
            && rect.right() <= band.right()
            && rect.bottom() <= band.bottom()
    })
}

fn foldable(children: &[Node], clip: Option<Rect>) -> bool {
    let mut rects = Vec::new();
    units(children, clip, &mut rects);
    if rects.len() > FOLD_LIMIT {
        return false;
    }
    rects.sort_by(|a, b| a.x.total_cmp(&b.x));
    rects.iter().enumerate().all(|(index, rect)| {
        rects[index + 1..]
            .iter()
            .take_while(|other| other.x < rect.right())
            .all(|other| intersect(*rect, *other).is_none())
    })
}

#[derive(Clone, Copy)]
struct Behind {
    target: TextureRef,
    origin: [f32; 2],
    extent: Extent,
}

impl Behind {
    fn area(&self) -> Rect {
        Rect {
            x: self.origin[0],
            y: self.origin[1],
            w: self.extent.width as f32,
            h: self.extent.height as f32,
        }
    }
}

#[derive(Clone, Copy)]
struct Context {
    clip: Option<Rect>,
    opacity: f32,
    target: TextureRef,
    origin: [f32; 2],
    extent: Extent,
    behind: Option<Behind>,
}

impl Context {
    fn area(&self) -> Rect {
        Rect {
            x: self.origin[0],
            y: self.origin[1],
            w: self.extent.width as f32,
            h: self.extent.height as f32,
        }
    }

    fn local(&self, rect: Rect) -> Rect {
        Rect {
            x: rect.x - self.origin[0],
            y: rect.y - self.origin[1],
            ..rect
        }
    }

    fn local_clip(&self) -> Option<Rect> {
        self.clip.map(|clip| self.local(clip))
    }

    fn local_gradient(&self, gradient: Gradient) -> Gradient {
        Gradient {
            start: [
                gradient.start[0] - self.origin[0],
                gradient.start[1] - self.origin[1],
            ],
            end: [
                gradient.end[0] - self.origin[0],
                gradient.end[1] - self.origin[1],
            ],
            stops: gradient.stops,
        }
    }

    fn layer(&self, target: TextureRef, region: PixelRegion) -> Context {
        Context {
            clip: self.clip,
            opacity: 1.0,
            target,
            origin: [region.rect.x, region.rect.y],
            extent: region.extent,
            behind: Some(self.backdrop()),
        }
    }

    fn backdrop(&self) -> Behind {
        self.behind.unwrap_or(Behind {
            target: self.target,
            origin: self.origin,
            extent: self.extent,
        })
    }
}

#[derive(Clone, Copy)]
struct PixelRegion {
    rect: Rect,
    extent: Extent,
}

fn pixel_region(rect: Rect) -> Option<PixelRegion> {
    let (left, top) = (rect.x.floor(), rect.y.floor());
    let (right, bottom) = (rect.right().ceil(), rect.bottom().ceil());
    (right > left && bottom > top).then(|| PixelRegion {
        rect: Rect::from_edges(left, top, right, bottom),
        extent: Extent::new((right - left) as u32, (bottom - top) as u32),
    })
}

fn children_region(
    children: &[Node],
    clip: Option<Rect>,
    context: &Context,
) -> Option<PixelRegion> {
    let mut inner = Vec::new();
    units(children, clip, &mut inner);
    inner
        .into_iter()
        .reduce(union)
        .and_then(|total| intersect(total, context.area()))
        .and_then(pixel_region)
}

struct Emitter {
    graph: RenderGraph,
}

impl Emitter {
    fn written(&self, target: TextureRef) -> bool {
        target == TextureRef::Surface
            || self
                .graph
                .passes()
                .iter()
                .any(|pass| pass.writes.contains(&target))
    }

    fn draw(&mut self, kind: PassKind, reads: Vec<TextureRef>, target: TextureRef) {
        self.graph.push(Pass::new(kind, reads, vec![target]));
    }

    fn emit(&mut self, nodes: &[Node], context: Context) {
        nodes.iter().for_each(|node| self.node(node, context));
    }

    fn node(&mut self, node: &Node, context: Context) {
        match node {
            Node::Leaf(leaf) => self.leaf(leaf, context),
            Node::Clip(rect, children) => {
                if let Some(clip) = clipped(*rect, context.clip) {
                    self.emit(
                        children,
                        Context {
                            clip: Some(clip),
                            ..context
                        },
                    );
                }
            }
            Node::RoundedClip {
                rect,
                radius,
                children,
            } => self.rounded_clip(*rect, *radius, children, context),
            Node::Group(opacity, children) => {
                let combined = context.opacity * opacity.clamp(0.0, 1.0);
                if combined <= 0.0 || children.is_empty() {
                    return;
                }
                if combined >= 1.0 || foldable(children, context.clip) {
                    self.emit(
                        children,
                        Context {
                            opacity: combined,
                            ..context
                        },
                    );
                    return;
                }
                let Some(region) = children_region(children, context.clip, &context) else {
                    return;
                };
                let layer = self.graph.create(region.extent, TextureFormat::Rgba16Float);
                self.emit(children, context.layer(layer, region));
                if !self.written(layer) {
                    return;
                }
                let covers_target = region.extent == context.extent
                    && region.rect.x == context.origin[0]
                    && region.rect.y == context.origin[1];
                match covers_target {
                    true => self.draw(
                        PassKind::Fullscreen {
                            shader: ShaderId::LayerOver,
                            opacity: combined,
                        },
                        vec![layer],
                        context.target,
                    ),
                    false => self.composite(layer, region, combined, Blend::Over, context),
                }
            }
            Node::Effect {
                bounds,
                effect,
                children,
            } => self.effect(*bounds, effect, children, context),
        }
    }

    fn composite(
        &mut self,
        layer: TextureRef,
        region: PixelRegion,
        opacity: f32,
        blend: Blend,
        context: Context,
    ) {
        self.draw(
            PassKind::Images {
                instances: vec![ImageInstance {
                    source: layer,
                    bounds: context.local(region.rect),
                    texels: region.extent.bounds(),
                    radius: 0.0,
                    opacity,
                    clip: context.local_clip(),
                    clip_radius: 0.0,
                    blend,
                    linear: Linear2::IDENTITY,
                }],
            },
            vec![layer],
            context.target,
        );
    }

    fn rounded_clip(&mut self, rect: Rect, radius: f32, children: &[Node], context: Context) {
        if radius <= BLUR_EPSILON {
            return self.node(&Node::Clip(rect, children.to_vec()), context);
        }
        let Some(bounded) = clipped(rect, context.clip) else {
            return;
        };
        let split = corner_split(rect, radius, bounded)
            .filter(|split| backdrops_inside(children, Some(bounded), split.bands[1]));
        match split {
            Some(split) => {
                split.bands.into_iter().for_each(|band| {
                    if let Some(clip) = clipped(band, context.clip) {
                        self.emit(
                            children,
                            Context {
                                clip: Some(clip),
                                ..context
                            },
                        );
                    }
                });
                split.corners.into_iter().for_each(|corner| {
                    if let Some(clip) = clipped(corner, context.clip) {
                        self.rounded_layer(
                            rect,
                            radius,
                            children,
                            Context {
                                clip: Some(clip),
                                ..context
                            },
                        );
                    }
                });
            }
            None => self.rounded_layer(rect, radius, children, context),
        }
    }

    fn rounded_layer(&mut self, rect: Rect, radius: f32, children: &[Node], context: Context) {
        let Some(bounded) = clipped(rect, context.clip) else {
            return;
        };
        let Some(region) = children_region(children, Some(bounded), &context) else {
            return;
        };
        let layer = self.graph.create(region.extent, TextureFormat::Rgba16Float);
        let seeded = context.behind.is_none() && reads_backdrop(children, Some(bounded));
        let inner = match seeded {
            true => {
                let local = context.local(region.rect);
                let mut seed = Pass::new(
                    PassKind::Copy {
                        source_origin: [local.x as u32, local.y as u32],
                        dest_origin: [0, 0],
                        size: region.extent,
                    },
                    vec![context.target],
                    vec![layer],
                );
                seed.load = LoadOp::DontCare;
                self.graph.push(seed);
                Context {
                    behind: None,
                    ..context.layer(layer, region)
                }
            }
            false => context.layer(layer, region),
        };
        self.emit(children, inner);
        if !self.written(layer) {
            return;
        }
        self.draw(
            PassKind::Images {
                instances: vec![ImageInstance {
                    source: layer,
                    bounds: context.local(region.rect),
                    texels: region.extent.bounds(),
                    radius: 0.0,
                    opacity: context.opacity,
                    clip: Some(context.local(rect)),
                    clip_radius: radius,
                    blend: match seeded {
                        true => Blend::Replace,
                        false => Blend::Over,
                    },
                    linear: Linear2::IDENTITY,
                }],
            },
            vec![layer],
            context.target,
        );
    }

    fn effect(&mut self, bounds: Rect, effect: &Effect, children: &[Node], context: Context) {
        if matches!(effect, Effect::Blur(radius) if *radius <= BLUR_EPSILON) {
            return self.emit(children, context);
        }
        let Some(exact) = clipped(bounds, context.clip).and_then(|r| intersect(r, context.area()))
        else {
            return;
        };
        let Some(region) = pixel_region(exact) else {
            return;
        };
        let layer = self.graph.create(region.extent, TextureFormat::Rgba16Float);
        self.emit(
            children,
            Context {
                clip: Some(exact),
                ..context.layer(layer, region)
            },
        );
        if !self.written(layer) {
            return;
        }
        match effect {
            Effect::Blur(radius) => {
                let local = region.extent.bounds();
                self.blurred(
                    layer,
                    local,
                    context.local(region.rect),
                    *radius,
                    0.0,
                    Blend::Over,
                    context,
                );
            }
            Effect::ColorMatrix(matrix) => {
                let output = self.graph.create(region.extent, TextureFormat::Rgba16Float);
                let mut pass = Pass::new(
                    PassKind::Filter {
                        shader: ShaderId::ColorMatrix,
                        params: FilterParams::color_matrix(region.extent.bounds(), *matrix),
                    },
                    vec![layer],
                    vec![output],
                );
                pass.load = crate::plan::LoadOp::DontCare;
                self.graph.push(pass);
                self.composite(output, region, context.opacity, Blend::Over, context);
            }
            Effect::Custom(spec) => {
                let output = self.graph.create(region.extent, TextureFormat::Rgba16Float);
                let mut spec = spec.clone();
                spec.bounds = region.extent.bounds();
                self.draw(PassKind::Custom(spec), vec![layer], output);
                self.composite(output, region, context.opacity, Blend::Over, context);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn blurred(
        &mut self,
        source: TextureRef,
        region: Rect,
        placement: Rect,
        radius: f32,
        corner: f32,
        blend: Blend,
        context: Context,
    ) {
        let output = blur_chain(
            &mut self.graph,
            BlurRequest {
                source,
                region,
                placement,
                radius,
            },
        );
        self.draw(
            PassKind::Images {
                instances: vec![ImageInstance {
                    source: output.texture,
                    bounds: output.bounds,
                    texels: output.texels,
                    radius: corner,
                    opacity: context.opacity,
                    clip: context.local_clip(),
                    clip_radius: 0.0,
                    blend,
                    linear: Linear2::IDENTITY,
                }],
            },
            vec![output.texture],
            context.target,
        );
    }

    fn leaf(&mut self, leaf: &Leaf, context: Context) {
        let opacity = context.opacity;
        let target = context.target;
        let clip = context.local_clip();
        match leaf {
            Leaf::Quad(quad, gradient, linear) => self.graph.draw_quad(
                target,
                QuadInstance {
                    bounds: context.local(quad.bounds),
                    radius: quad.radius,
                    border_width: quad.border_width,
                    fill: quad.fill,
                    border_color: quad.border_color,
                    clip,
                    gradient: gradient.map(|g| context.local_gradient(g)),
                    opacity,
                    linear: *linear,
                },
            ),
            Leaf::Shadow(shadow) => self.graph.draw_shadow(
                target,
                ShadowInstance {
                    bounds: context.local(shadow.bounds),
                    radius: shadow.radius,
                    offset: shadow.offset,
                    sigma: shadow.sigma,
                    spread: shadow.spread,
                    color: shadow.color,
                    clip,
                    opacity,
                    cast: shadow.cast,
                },
            ),
            Leaf::Glyphs(run) => self.graph.draw_glyphs(
                target,
                run.atlas,
                run.glyphs.iter().map(|glyph| GlyphInstance {
                    bounds: context.local(glyph.bounds),
                    texels: glyph.texels,
                    color: glyph.color,
                    clip,
                    opacity,
                }),
            ),
            Leaf::Slug(run) => self.graph.draw_slug(
                target,
                run.store,
                run.glyphs.iter().map(|glyph| SlugInstance {
                    origin: [
                        glyph.origin[0] - context.origin[0],
                        glyph.origin[1] - context.origin[1],
                    ],
                    clip,
                    opacity: glyph.opacity * opacity,
                    ..*glyph
                }),
            ),
            Leaf::Image(image, linear) => self.draw(
                PassKind::Images {
                    instances: vec![ImageInstance {
                        source: TextureRef::Persistent(image.source),
                        bounds: context.local(image.bounds),
                        texels: image.texels,
                        radius: image.radius,
                        opacity,
                        clip,
                        clip_radius: 0.0,
                        blend: Blend::Over,
                        linear: *linear,
                    }],
                },
                vec![TextureRef::Persistent(image.source)],
                target,
            ),
            Leaf::Backdrop(blur) => {
                if blur.radius <= BLUR_EPSILON {
                    return;
                }
                let source = context.backdrop();
                let Some(visible) = clipped(blur.bounds, context.clip)
                    .and_then(|r| intersect(r, context.area()))
                    .and_then(|r| intersect(r, source.area()))
                else {
                    return;
                };
                if !self.written(source.target) {
                    return;
                }
                self.blurred(
                    source.target,
                    Rect {
                        x: visible.x - source.origin[0],
                        y: visible.y - source.origin[1],
                        ..visible
                    },
                    context.local(visible),
                    blur.radius,
                    blur.corner_radius,
                    Blend::Replace,
                    context,
                );
            }
        }
    }
}

pub const BLUR_EPSILON: f32 = 0.01;

pub fn plan_paint(
    extent: Extent,
    clear: Oklch,
    commands: &[PaintCommand],
) -> Result<FramePlan, PlanError> {
    plan_paint_with(extent, clear, commands, PlanLimits::default())
}

pub fn plan_paint_with(
    extent: Extent,
    clear: Oklch,
    commands: &[PaintCommand],
    limits: PlanLimits,
) -> Result<FramePlan, PlanError> {
    let tree = parse(commands)?;
    let mut emitter = Emitter {
        graph: RenderGraph::new(extent, clear),
    };
    emitter.emit(
        &tree,
        Context {
            clip: None,
            opacity: 1.0,
            target: TextureRef::Surface,
            origin: [0.0, 0.0],
            extent,
            behind: None,
        },
    );
    emitter.graph.plan(limits)
}
