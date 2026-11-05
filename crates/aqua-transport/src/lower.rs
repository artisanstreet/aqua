use aqua_core::{DisplayItem, Oklch, Rect, Size};
use aqua_graph::{PaintCommand, Quad, Shadow};
use aqua_protocol::{FrameReport, Instant, UploadBatch};
use aqua_scene::Scene;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lowered {
    pub commands: Vec<PaintCommand>,
    pub uploads: UploadBatch,
    pub unsupported: u32,
    pub approximated: u32,
    pub glyphs: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LowerContext {
    pub size: Size,
    pub scale: f32,
    pub now: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrimRequest {
    IdleFrames(u64),
    Everything,
}

pub trait Lowering: 'static {
    fn lower(&mut self, scene: &Scene, context: &LowerContext) -> Lowered;

    fn sync(&mut self) {}

    fn device_recovered(&mut self) {}

    fn frame_reported(&mut self, _report: &FrameReport) {}

    fn reclaim(&mut self, _commands: Vec<PaintCommand>) {}

    fn trim(&mut self, _request: TrimRequest) -> usize {
        0
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GeometryLowering;

impl Lowering for GeometryLowering {
    fn lower(&mut self, scene: &Scene, context: &LowerContext) -> Lowered {
        lower_items(scene.ordered().map(|(_, item)| item), context.scale)
    }
}

struct State {
    scale: f32,
    offsets: Vec<[f32; 2]>,
    lowered: Lowered,
}

impl State {
    fn offset(&self) -> [f32; 2] {
        self.offsets.last().copied().unwrap_or([0.0, 0.0])
    }

    fn device(&self, rect: Rect) -> Rect {
        let [dx, dy] = self.offset();
        Rect {
            x: (rect.x + dx) * self.scale,
            y: (rect.y + dy) * self.scale,
            w: rect.w * self.scale,
            h: rect.h * self.scale,
        }
    }

    fn push(&mut self, command: PaintCommand) {
        self.lowered.commands.push(command);
    }

    fn skip(&mut self) {
        self.lowered.unsupported += 1;
    }

    fn quad(&mut self, bounds: Rect, radius: f32, border: f32, fill: Oklch, border_color: Oklch) {
        let visible = fill.alpha > 0.0 || (border > 0.0 && border_color.alpha > 0.0);
        if !visible {
            return;
        }
        self.push(PaintCommand::Quad(Quad {
            bounds: self.device(bounds),
            radius: radius * self.scale,
            border_width: border * self.scale,
            fill,
            border_color,
        }));
    }

    fn shadow(
        &mut self,
        bounds: Rect,
        radius: f32,
        offset: [f32; 2],
        blur: f32,
        spread: f32,
        color: Oklch,
    ) {
        if color.alpha <= 0.0 {
            return;
        }
        self.push(PaintCommand::Shadow(Shadow {
            bounds: self.device(bounds),
            radius: radius * self.scale,
            offset: [offset[0] * self.scale, offset[1] * self.scale],
            sigma: blur.max(0.0) * 0.5 * self.scale,
            spread: spread * self.scale,
            color,
        }));
    }

    fn item(&mut self, item: &DisplayItem) {
        match item {
            DisplayItem::Rect {
                bounds,
                radius,
                fill,
                border_width,
                border_color,
                shadow,
            } => {
                if let Some((dx, dy, blur, color)) = shadow {
                    self.shadow(*bounds, *radius, [*dx, *dy], *blur, 0.0, *color);
                }
                self.quad(*bounds, *radius, *border_width, *fill, *border_color);
            }
            DisplayItem::Quad {
                bounds,
                corners,
                fill,
                border_widths,
                border_color,
                dashed,
            } => {
                let width = border_widths
                    .left
                    .max(border_widths.top)
                    .max(border_widths.right)
                    .max(border_widths.bottom);
                if *dashed {
                    self.skip();
                }
                self.quad(*bounds, corners.top_left, width, *fill, *border_color);
            }
            DisplayItem::Shadow {
                bounds,
                corners,
                layer,
            } => match layer.inset {
                true => self.skip(),
                false => self.shadow(
                    *bounds,
                    corners.top_left,
                    [layer.dx, layer.dy],
                    layer.blur,
                    layer.spread,
                    layer.color,
                ),
            },
            DisplayItem::PushClip { bounds, .. }
            | DisplayItem::PushRoundedClip { bounds, .. }
            | DisplayItem::PushGpuiContentMask { bounds } => {
                let rect = self.device(*bounds);
                self.push(PaintCommand::PushClip(rect));
            }
            DisplayItem::PopClip => self.push(PaintCommand::PopClip),
            DisplayItem::PushOpacity(opacity) => self.push(PaintCommand::PushOpacity(*opacity)),
            DisplayItem::PopOpacity => self.push(PaintCommand::PopOpacity),
            DisplayItem::PushContentBlur { bounds, radius } => {
                let rect = self.device(*bounds);
                self.push(PaintCommand::PushContentBlur {
                    bounds: rect,
                    radius: radius * self.scale,
                });
            }
            DisplayItem::PopContentBlur => self.push(PaintCommand::PopContentBlur),
            DisplayItem::PushTransform(transform) => {
                let linear = (transform.a - 1.0).abs() > f32::EPSILON
                    || (transform.d - 1.0).abs() > f32::EPSILON
                    || transform.b != 0.0
                    || transform.c != 0.0;
                if linear {
                    self.skip();
                }
                let [dx, dy] = self.offset();
                self.offsets.push([dx + transform.tx, dy + transform.ty]);
            }
            DisplayItem::PopTransform => {
                self.offsets.pop();
            }
            DisplayItem::BackdropBlurRounded {
                bounds,
                radius,
                corners,
            } => {
                let rect = self.device(*bounds);
                self.push(PaintCommand::BackdropBlur(aqua_graph::BackdropBlur {
                    bounds: rect,
                    radius: radius * self.scale,
                    corner_radius: corners.top_left * self.scale,
                }));
            }
            DisplayItem::BackdropBlur { bounds, radius, .. } => {
                let rect = self.device(*bounds);
                self.push(PaintCommand::BackdropBlur(aqua_graph::BackdropBlur {
                    bounds: rect,
                    radius: radius * self.scale,
                    corner_radius: 0.0,
                }));
            }
            DisplayItem::PushTextColorMap(_)
            | DisplayItem::PopTextColorMap
            | DisplayItem::PushGpuiTextColorMap(_)
            | DisplayItem::PushGpuiTextShimmer(_)
            | DisplayItem::PopGpuiTextColorMap
            | DisplayItem::PushGpuiTextColor(_)
            | DisplayItem::PopGpuiTextColor
            | DisplayItem::PushGpuiTextOpacity(_)
            | DisplayItem::PopGpuiTextOpacity
            | DisplayItem::PushGpuiSourceTextColor(_)
            | DisplayItem::PopGpuiSourceTextColor => {}
            _ => self.skip(),
        }
    }
}

pub fn lower_items<'a>(items: impl IntoIterator<Item = &'a DisplayItem>, scale: f32) -> Lowered {
    let mut state = State {
        scale,
        offsets: Vec::new(),
        lowered: Lowered::default(),
    };
    for item in items {
        state.item(item);
    }
    balance(&mut state.lowered.commands);
    state.lowered
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Group {
    Clip,
    Opacity,
    Blur,
}

impl Group {
    fn opened_by(command: &PaintCommand) -> Option<Group> {
        match command {
            PaintCommand::PushClip(_) => Some(Group::Clip),
            PaintCommand::PushOpacity(_) => Some(Group::Opacity),
            PaintCommand::PushContentBlur { .. } => Some(Group::Blur),
            _ => None,
        }
    }

    fn closed_by(command: &PaintCommand) -> Option<Group> {
        match command {
            PaintCommand::PopClip => Some(Group::Clip),
            PaintCommand::PopOpacity => Some(Group::Opacity),
            PaintCommand::PopContentBlur => Some(Group::Blur),
            _ => None,
        }
    }

    fn close(self) -> PaintCommand {
        match self {
            Group::Clip => PaintCommand::PopClip,
            Group::Opacity => PaintCommand::PopOpacity,
            Group::Blur => PaintCommand::PopContentBlur,
        }
    }
}

fn balance(commands: &mut Vec<PaintCommand>) {
    let mut open: Vec<Group> = Vec::new();
    let mut kept: Vec<PaintCommand> = Vec::with_capacity(commands.len());
    for command in commands.drain(..) {
        match (Group::opened_by(&command), Group::closed_by(&command)) {
            (Some(group), _) => {
                open.push(group);
                kept.push(command);
            }
            (None, Some(group)) if open.last() == Some(&group) => {
                open.pop();
                kept.push(command);
            }
            (None, Some(_)) => {}
            (None, None) => kept.push(command),
        }
    }
    kept.extend(open.into_iter().rev().map(Group::close));
    *commands = kept;
}
