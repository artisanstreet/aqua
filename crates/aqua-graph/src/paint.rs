use aqua_core::{Oklch, geometry::Rect};

use crate::analysis::{intersect, union};
use crate::blur::{BlurRequest, blur_chain};
use crate::graph::{PlanLimits, RenderGraph};
use crate::plan::{
    Extent, FramePlan, GlyphInstance, Gradient, ImageInstance, Pass, PassKind, QuadInstance,
    ShaderId, ShadowInstance, TextureFormat, TextureHandle, TextureRef,
};
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
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    pub bounds: Rect,
    pub texels: Rect,
    pub color: Oklch,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlyphRun {
    pub atlas: TextureHandle,
    pub glyphs: Vec<Glyph>,
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
    BackdropBlur(BackdropBlur),
    PushClip(Rect),
    PopClip,
    PushOpacity(f32),
    PopOpacity,
    PushContentBlur { bounds: Rect, radius: f32 },
    PopContentBlur,
}

#[derive(Clone, Debug)]
enum Leaf {
    Quad(Quad, Option<Gradient>),
    Shadow(Shadow),
    Glyphs(GlyphRun),
    Image(Image),
    Backdrop(BackdropBlur),
}

#[derive(Clone, Debug)]
enum Node {
    Leaf(Leaf),
    Clip(Rect, Vec<Node>),
    Group(f32, Vec<Node>),
    ContentBlur {
        bounds: Rect,
        radius: f32,
        children: Vec<Node>,
    },
}

enum Frame {
    Clip(Rect),
    Group(f32),
    ContentBlur { bounds: Rect, radius: f32 },
}

fn parse(commands: &[PaintCommand]) -> Result<Vec<Node>, PlanError> {
    let mut stack: Vec<(Option<Frame>, Vec<Node>)> = vec![(None, Vec::new())];
    let push = |stack: &mut Vec<(Option<Frame>, Vec<Node>)>, node: Node| {
        stack.last_mut().expect("root frame").1.push(node)
    };
    for command in commands {
        match command {
            PaintCommand::Quad(quad) => push(&mut stack, Node::Leaf(Leaf::Quad(*quad, None))),
            PaintCommand::GradientQuad(item) => push(
                &mut stack,
                Node::Leaf(Leaf::Quad(item.quad, Some(item.gradient))),
            ),
            PaintCommand::Shadow(shadow) => push(&mut stack, Node::Leaf(Leaf::Shadow(*shadow))),
            PaintCommand::Glyphs(run) => push(&mut stack, Node::Leaf(Leaf::Glyphs(run.clone()))),
            PaintCommand::Image(image) => push(&mut stack, Node::Leaf(Leaf::Image(*image))),
            PaintCommand::BackdropBlur(blur) => push(&mut stack, Node::Leaf(Leaf::Backdrop(*blur))),
            PaintCommand::PushClip(rect) => stack.push((Some(Frame::Clip(*rect)), Vec::new())),
            PaintCommand::PushOpacity(opacity) => {
                stack.push((Some(Frame::Group(*opacity)), Vec::new()))
            }
            PaintCommand::PushContentBlur { bounds, radius } => stack.push((
                Some(Frame::ContentBlur {
                    bounds: *bounds,
                    radius: *radius,
                }),
                Vec::new(),
            )),
            PaintCommand::PopClip => {
                let (frame, children) = stack.pop().ok_or(PlanError::UnbalancedPop)?;
                match frame {
                    Some(Frame::Clip(rect)) => push(&mut stack, Node::Clip(rect, children)),
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
            PaintCommand::PopContentBlur => {
                let (frame, children) = stack.pop().ok_or(PlanError::UnbalancedPop)?;
                match frame {
                    Some(Frame::ContentBlur { bounds, radius }) => push(
                        &mut stack,
                        Node::ContentBlur {
                            bounds,
                            radius,
                            children,
                        },
                    ),
                    _ => return Err(PlanError::UnbalancedPop),
                }
            }
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
        Leaf::Quad(quad, _) => Some(quad.bounds),
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
            }
            .covered(),
        ),
        Leaf::Glyphs(run) => run.glyphs.iter().map(|glyph| glyph.bounds).reduce(union),
        Leaf::Image(image) => Some(image.bounds),
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
            Node::Group(_, children) => {
                let mut inner = Vec::new();
                units(children, clip, &mut inner);
                if let Some(total) = inner.into_iter().reduce(union) {
                    out.push(total);
                }
            }
            Node::ContentBlur {
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
struct Context {
    clip: Option<Rect>,
    opacity: f32,
    target: TextureRef,
}

struct Emitter {
    graph: RenderGraph,
    extent: Extent,
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
            Node::Group(opacity, children) => {
                let combined = context.opacity * opacity.clamp(0.0, 1.0);
                if combined <= 0.0 || children.is_empty() {
                    return;
                }
                if foldable(children, context.clip) {
                    self.emit(
                        children,
                        Context {
                            opacity: combined,
                            ..context
                        },
                    );
                    return;
                }
                let layer = self.graph.create(self.extent, TextureFormat::Rgba16Float);
                self.emit(
                    children,
                    Context {
                        opacity: 1.0,
                        target: layer,
                        ..context
                    },
                );
                if !self.written(layer) {
                    return;
                }
                self.draw(
                    PassKind::Fullscreen {
                        shader: ShaderId::LayerOver,
                        opacity: combined,
                    },
                    vec![layer],
                    context.target,
                );
            }
            Node::ContentBlur {
                bounds,
                radius,
                children,
            } => self.content_blur(*bounds, *radius, children, context),
        }
    }

    fn content_blur(&mut self, bounds: Rect, radius: f32, children: &[Node], context: Context) {
        if radius <= BLUR_EPSILON {
            return self.emit(children, context);
        }
        let Some(region) =
            clipped(bounds, context.clip).and_then(|r| clipped(r, Some(self.extent.bounds())))
        else {
            return;
        };
        let layer = self.graph.create(self.extent, TextureFormat::Rgba16Float);
        self.emit(
            children,
            Context {
                clip: Some(region),
                opacity: 1.0,
                target: layer,
            },
        );
        if self.written(layer) {
            self.blurred(layer, region, region, radius, 0.0, context);
        }
    }

    fn blurred(
        &mut self,
        source: TextureRef,
        region: Rect,
        placement: Rect,
        radius: f32,
        corner: f32,
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
                    clip: context.clip,
                }],
            },
            vec![output.texture],
            context.target,
        );
    }

    fn leaf(&mut self, leaf: &Leaf, context: Context) {
        let Context {
            clip,
            opacity,
            target,
        } = context;
        match leaf {
            Leaf::Quad(quad, gradient) => self.draw(
                PassKind::Instanced {
                    shader: ShaderId::Quad,
                    instances: vec![QuadInstance {
                        bounds: quad.bounds,
                        radius: quad.radius,
                        border_width: quad.border_width,
                        fill: quad.fill,
                        border_color: quad.border_color,
                        clip,
                        gradient: *gradient,
                        opacity,
                    }],
                },
                Vec::new(),
                target,
            ),
            Leaf::Shadow(shadow) => self.draw(
                PassKind::Shadows {
                    instances: vec![ShadowInstance {
                        bounds: shadow.bounds,
                        radius: shadow.radius,
                        offset: shadow.offset,
                        sigma: shadow.sigma,
                        spread: shadow.spread,
                        color: shadow.color,
                        clip,
                        opacity,
                    }],
                },
                Vec::new(),
                target,
            ),
            Leaf::Glyphs(run) => self.draw(
                PassKind::Glyphs {
                    atlas: run.atlas,
                    instances: run
                        .glyphs
                        .iter()
                        .map(|glyph| GlyphInstance {
                            bounds: glyph.bounds,
                            texels: glyph.texels,
                            color: glyph.color,
                            clip,
                            opacity,
                        })
                        .collect(),
                },
                vec![TextureRef::Persistent(run.atlas)],
                target,
            ),
            Leaf::Image(image) => self.draw(
                PassKind::Images {
                    instances: vec![ImageInstance {
                        source: TextureRef::Persistent(image.source),
                        bounds: image.bounds,
                        texels: image.texels,
                        radius: image.radius,
                        opacity,
                        clip,
                    }],
                },
                vec![TextureRef::Persistent(image.source)],
                target,
            ),
            Leaf::Backdrop(blur) => {
                if blur.radius <= BLUR_EPSILON {
                    return;
                }
                let Some(region) =
                    clipped(blur.bounds, clip).and_then(|r| clipped(r, Some(self.extent.bounds())))
                else {
                    return;
                };
                if !self.written(target) {
                    return;
                }
                let snapshot = crate::blur::snapshot(&mut self.graph, target, region);
                let local = Rect {
                    x: 0.0,
                    y: 0.0,
                    w: snapshot.bounds.w,
                    h: snapshot.bounds.h,
                };
                self.blurred(
                    snapshot.texture,
                    local,
                    snapshot.bounds,
                    blur.radius,
                    blur.corner_radius,
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
        extent,
    };
    emitter.emit(
        &tree,
        Context {
            clip: None,
            opacity: 1.0,
            target: TextureRef::Surface,
        },
    );
    emitter.graph.plan(limits)
}
