use std::sync::Arc;

use aqua_core::geometry::Rect;
use aqua_graph::{
    BackdropBlur, Glyph, GlyphRun, Gradient, GradientQuad, Image, PaintCommand, Quad, Shadow,
    SlugRun,
};
use aqua_media::atlas::{PageFormat, PageId};
use aqua_text::layout::LaidOut;

pub const SLACK: f32 = 128.0;
pub const MIN_COMMANDS: usize = 16;
const ENTRY_LIMIT: usize = 16;
const BYTES_LIMIT: usize = 4 << 20;
const EDGE_PAD: f32 = 2.0;
const SHADOW_SIGMAS: f32 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taint {
    Clean,
    Tainted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Environment {
    pub scale: u32,
    pub surface: (u32, u32),
    pub clip: Option<[u32; 4]>,
    pub open: usize,
    pub blurred: bool,
    pub parent: [u32; 4],
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub glyphs: u32,
    pub curves: u32,
    pub culled: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub epoch: u64,
    pub fonts: u64,
    pub slug: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Context {
    pub affine: aqua_core::Transform2D,
    pub clip: Option<Rect>,
    pub blurred: bool,
}

pub struct Entry {
    pub starts: Vec<u32>,
    pub contexts: Vec<Option<Context>>,
    pub matching: Vec<u32>,
    pub segment: bool,
    pub len: usize,
    pub ids: Vec<u32>,
    pub revision: u64,
    pub origin: (f32, f32),
    pub environment: Environment,
    pub stamp: Stamp,
    pub commands: Vec<PaintCommand>,
    pub tally: Tally,
    pub pages: Vec<PageId>,
    pub laid: Vec<(u64, Arc<LaidOut>)>,
    pub paths: Vec<aqua_media::residency::VectorKey>,
    pub bytes: usize,
}

#[derive(Default)]
pub struct SpanCache {
    entries: aqua_core::hash::FastMap<aqua_core::NodeId, Entry>,
    bytes: usize,
}

impl SpanCache {
    pub fn get(&self, id: aqua_core::NodeId) -> Option<&Entry> {
        self.entries.get(&id)
    }

    pub fn insert(&mut self, id: aqua_core::NodeId, entry: Entry) {
        if entry.bytes > BYTES_LIMIT {
            return;
        }
        if self.entries.len() >= ENTRY_LIMIT || self.bytes + entry.bytes > BYTES_LIMIT {
            self.clear();
        }
        self.bytes += entry.bytes;
        if let Some(old) = self.entries.insert(id, entry) {
            self.bytes -= old.bytes;
        }
    }

    pub fn take(&mut self, id: aqua_core::NodeId) -> Option<Entry> {
        let entry = self.entries.remove(&id)?;
        self.bytes -= entry.bytes;
        Some(entry)
    }

    pub fn remove(&mut self, id: aqua_core::NodeId) {
        if let Some(old) = self.entries.remove(&id) {
            self.bytes -= old.bytes;
        }
    }

    pub fn retire_path(
        &mut self,
        path: aqua_media::residency::VectorKey,
        previous_epoch: u64,
        epoch: u64,
    ) {
        self.entries.retain(|_, entry| {
            if entry.paths.contains(&path) {
                self.bytes -= entry.bytes;
                return false;
            }
            if entry.stamp.epoch == previous_epoch {
                entry.stamp.epoch = epoch;
            }
            true
        });
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

pub fn clip_bits(rect: Rect) -> [u32; 4] {
    [
        rect.x.to_bits(),
        rect.y.to_bits(),
        rect.w.to_bits(),
        rect.h.to_bits(),
    ]
}

pub fn page_of(handle: aqua_graph::plan::TextureHandle) -> Option<PageId> {
    match handle.0 {
        value if value >= u32::MAX - 1 => None,
        value => Some(PageId {
            format: match value % 2 {
                0 => PageFormat::Mask8,
                _ => PageFormat::Color8,
            },
            index: value / 2,
        }),
    }
}

pub fn pages_of(commands: &[PaintCommand]) -> Vec<PageId> {
    let mut pages: Vec<PageId> = Vec::new();
    let mut note = |handle| {
        if let Some(page) = page_of(handle) {
            if !pages.contains(&page) {
                pages.push(page);
            }
        }
    };
    for command in commands {
        match command {
            PaintCommand::Glyphs(run) => note(run.atlas),
            PaintCommand::Image(image) => note(image.source),
            _ => {}
        }
    }
    pages
}

pub fn cacheable(commands: &[PaintCommand]) -> Taint {
    match commands.iter().any(|command| {
        matches!(
            command,
            PaintCommand::Linear { .. }
                | PaintCommand::PushCustomEffect { .. }
                | PaintCommand::PushColorMatrix { .. }
        )
    }) {
        true => Taint::Tainted,
        false => Taint::Clean,
    }
}

pub fn bytes_of(commands: &[PaintCommand]) -> usize {
    commands
        .iter()
        .map(|command| match command {
            PaintCommand::Glyphs(run) => run.glyphs.len() * std::mem::size_of::<Glyph>(),
            PaintCommand::Slug(run) => {
                run.glyphs.len() * std::mem::size_of::<aqua_graph::SlugInstance>()
            }
            _ => 0,
        } + std::mem::size_of::<PaintCommand>())
        .sum()
}

fn outside(rect: Rect, region: Rect, pad: f32) -> bool {
    rect.x - pad >= region.right()
        || region.x >= rect.right() + pad
        || rect.y - pad >= region.bottom()
        || region.y >= rect.bottom() + pad
}

fn moved(rect: Rect, dx: f32, dy: f32) -> Rect {
    Rect {
        x: rect.x + dx,
        y: rect.y + dy,
        ..rect
    }
}

fn shadow_reach(shadow: &Shadow) -> f32 {
    shadow.sigma.abs() * SHADOW_SIGMAS
        + shadow.spread.abs()
        + shadow.offset[0].abs().max(shadow.offset[1].abs())
}

pub fn replay(
    out: &mut Vec<PaintCommand>,
    commands: &[PaintCommand],
    dx: f32,
    dy: f32,
    region: Option<Rect>,
) -> u32 {
    let mut dropped = 0;
    let hidden = |rect: Rect, pad: f32| region.is_some_and(|region| outside(rect, region, pad));
    for command in commands {
        let shifted = match command {
            PaintCommand::Quad(quad) => {
                let bounds = moved(quad.bounds, dx, dy);
                match hidden(bounds, EDGE_PAD) {
                    true => None,
                    false => Some(PaintCommand::Quad(Quad { bounds, ..*quad })),
                }
            }
            PaintCommand::GradientQuad(item) => {
                let bounds = moved(item.quad.bounds, dx, dy);
                match hidden(bounds, EDGE_PAD) {
                    true => None,
                    false => Some(PaintCommand::GradientQuad(GradientQuad {
                        quad: Quad {
                            bounds,
                            ..item.quad
                        },
                        gradient: Gradient {
                            start: [item.gradient.start[0] + dx, item.gradient.start[1] + dy],
                            end: [item.gradient.end[0] + dx, item.gradient.end[1] + dy],
                            ..item.gradient
                        },
                    })),
                }
            }
            PaintCommand::Shadow(shadow) => {
                let bounds = moved(shadow.bounds, dx, dy);
                match hidden(bounds, shadow_reach(shadow) + EDGE_PAD) {
                    true => None,
                    false => Some(PaintCommand::Shadow(Shadow { bounds, ..*shadow })),
                }
            }
            PaintCommand::Glyphs(run) => Some(PaintCommand::Glyphs(GlyphRun {
                atlas: run.atlas,
                coverage: run.coverage,
                glyphs: run
                    .glyphs
                    .iter()
                    .map(|glyph| Glyph {
                        bounds: moved(glyph.bounds, dx, dy),
                        ..*glyph
                    })
                    .collect(),
            })),
            PaintCommand::Image(image) => {
                let bounds = moved(image.bounds, dx, dy);
                match hidden(bounds, EDGE_PAD) {
                    true => None,
                    false => Some(PaintCommand::Image(Image { bounds, ..*image })),
                }
            }
            PaintCommand::Slug(run) => Some(PaintCommand::Slug(SlugRun {
                store: run.store,
                glyphs: run
                    .glyphs
                    .iter()
                    .map(|instance| {
                        let mut instance = *instance;
                        instance.origin = [instance.origin[0] + dx, instance.origin[1] + dy];
                        instance.clip = instance.clip.map(|clip| moved(clip, dx, dy));
                        instance
                    })
                    .collect(),
            })),
            PaintCommand::BackdropBlur(blur) => Some(PaintCommand::BackdropBlur(BackdropBlur {
                bounds: moved(blur.bounds, dx, dy),
                ..*blur
            })),
            PaintCommand::PushClip(rect) => Some(PaintCommand::PushClip(moved(*rect, dx, dy))),
            PaintCommand::PushRoundedClip { rect, radius } => Some(PaintCommand::PushRoundedClip {
                rect: moved(*rect, dx, dy),
                radius: *radius,
            }),
            PaintCommand::PushContentBlur { bounds, radius } => {
                Some(PaintCommand::PushContentBlur {
                    bounds: moved(*bounds, dx, dy),
                    radius: *radius,
                })
            }
            other => Some(other.clone()),
        };
        match shifted {
            Some(command) => out.push(command),
            None => dropped += 1,
        }
    }
    dropped
}

pub const NONE: u32 = u32::MAX;

pub struct Unit {
    pub first: usize,
    pub last: usize,
    pub commands: Vec<PaintCommand>,
}

pub fn replay_around(
    out: &mut Vec<PaintCommand>,
    entry: &Entry,
    units: &[Unit],
    dx: f32,
    dy: f32,
    region: Option<Rect>,
) -> u32 {
    let mut dropped = 0;
    let mut next = 0usize;
    let outer = match entry.segment {
        true => None,
        false => region,
    };
    for unit in units {
        let from = entry.starts[next] as usize;
        let to = entry.starts[unit.first] as usize;
        let inner = match entry.segment {
            true => entry.contexts[unit.first].and_then(|context| context.clip),
            false => region,
        };
        dropped += replay(out, &entry.commands[from..to], dx, dy, outer);
        dropped += replay(out, &unit.commands, 0.0, 0.0, inner);
        next = unit.last + 1;
    }
    let from = entry.starts[next] as usize;
    dropped += replay(out, &entry.commands[from..], dx, dy, outer);
    dropped
}
