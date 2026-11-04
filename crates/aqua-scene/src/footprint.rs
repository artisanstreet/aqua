use std::collections::HashMap;

use aqua_core::geometry::{Point, Transform2D};

use crate::damage::DamageTracker;
use crate::scene::{DisplayItem, NodeId, Rect, Scene, TextRun};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Paint(Rect),
    Group,
    Structural,
    Unmeasured,
    Pop,
}

#[derive(Clone, Debug)]
struct Entry {
    revision: u64,
    kind: Kind,
    groups: Vec<NodeId>,
}

#[derive(Clone, Debug, Default)]
pub struct Footprints {
    entries: HashMap<NodeId, Entry>,
    groups: HashMap<NodeId, Rect>,
}

struct Open {
    id: NodeId,
    transform: Transform2D,
    extent: Rect,
}

fn empty() -> Rect {
    Rect {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    }
}

fn transformed(rect: Rect, transform: Transform2D) -> Rect {
    let corners = [
        Point::new(rect.x, rect.y),
        Point::new(rect.right(), rect.y),
        Point::new(rect.x, rect.bottom()),
        Point::new(rect.right(), rect.bottom()),
    ]
    .map(|corner| transform.apply(corner));
    let fold = |pick: fn(Point) -> f32, merge: fn(f32, f32) -> f32, seed: f32| {
        corners
            .iter()
            .fold(seed, |acc, corner| merge(acc, pick(*corner)))
    };
    Rect::from_edges(
        fold(|p| p.x, f32::min, f32::INFINITY),
        fold(|p| p.y, f32::min, f32::INFINITY),
        fold(|p| p.x, f32::max, f32::NEG_INFINITY),
        fold(|p| p.y, f32::max, f32::NEG_INFINITY),
    )
}

fn is_group_open(item: &DisplayItem) -> bool {
    matches!(
        item,
        DisplayItem::PushTransform(_) | DisplayItem::PushOpacity(_)
    )
}

fn is_group_close(item: &DisplayItem) -> bool {
    matches!(item, DisplayItem::PopTransform | DisplayItem::PopOpacity)
}

fn local_rect(item: &DisplayItem, ink: &dyn Fn(&TextRun) -> Option<Rect>) -> Kind {
    match item {
        DisplayItem::Text { run, .. } => ink(run).map_or(Kind::Unmeasured, Kind::Paint),
        DisplayItem::GpuiSvg { .. } | DisplayItem::GpuiTransformedSvg { .. } => Kind::Unmeasured,
        other => match other.bounds() {
            Some(rect) => Kind::Paint(rect),
            None if is_group_close(other)
                || matches!(other, DisplayItem::PopClip | DisplayItem::PopContentBlur) =>
            {
                Kind::Pop
            }
            None => Kind::Structural,
        },
    }
}

impl Footprints {
    pub fn of_scene(scene: &Scene, ink: &dyn Fn(&TextRun) -> Option<Rect>) -> Self {
        let mut out = Footprints::default();
        let mut stack: Vec<Open> = Vec::new();
        for (id, item) in scene.ordered() {
            let revision = scene.item_revision(id).unwrap_or(0);
            let chain: Vec<NodeId> = stack.iter().map(|open| open.id).collect();
            let world = stack
                .last()
                .map_or(Transform2D::IDENTITY, |open| open.transform);
            let kind = match is_group_open(item) {
                true => {
                    let own = match item {
                        DisplayItem::PushTransform(local) => local.then(world),
                        _ => world,
                    };
                    stack.push(Open {
                        id,
                        transform: own,
                        extent: empty(),
                    });
                    Kind::Group
                }
                false if is_group_close(item) => {
                    if let Some(open) = stack.pop() {
                        out.groups.insert(open.id, open.extent);
                        if let Some(parent) = stack.last_mut() {
                            parent.extent = parent.extent.union(open.extent);
                        }
                    }
                    Kind::Pop
                }
                false => match local_rect(item, ink) {
                    Kind::Paint(rect) => {
                        let placed = transformed(rect, world);
                        if let Some(top) = stack.last_mut() {
                            top.extent = top.extent.union(placed);
                        }
                        Kind::Paint(placed)
                    }
                    other => other,
                },
            };
            out.entries.insert(
                id,
                Entry {
                    revision,
                    kind,
                    groups: chain,
                },
            );
        }
        for open in stack.drain(..).rev() {
            out.groups.insert(open.id, open.extent);
        }
        out
    }

    #[must_use]
    pub fn group(&self, id: NodeId) -> Option<Rect> {
        self.groups.get(&id).copied()
    }

    #[must_use]
    pub fn item(&self, id: NodeId) -> Option<Rect> {
        match self.entries.get(&id)?.kind {
            Kind::Paint(rect) => Some(rect),
            _ => None,
        }
    }

    fn revision(&self, id: NodeId) -> Option<u64> {
        self.entries.get(&id).map(|entry| entry.revision)
    }
}

fn param_changed(group: NodeId, this: &Footprints, other: &Footprints) -> bool {
    match (this.revision(group), other.revision(group)) {
        (Some(a), Some(b)) => a != b,
        _ => true,
    }
}

fn covered(entry: &Entry, this: &Footprints, other: &Footprints) -> bool {
    entry
        .groups
        .iter()
        .any(|group| param_changed(*group, this, other))
}

impl DamageTracker {
    pub fn record_scene_diff(&mut self, before: &Footprints, after: &Footprints) {
        for (id, entry) in &after.entries {
            match entry.kind {
                Kind::Group => {
                    if param_changed(*id, after, before) {
                        let new = after.groups.get(id).copied();
                        let old = before.groups.get(id).copied();
                        self.record_optional(new, old);
                    }
                }
                Kind::Pop => {}
                Kind::Structural | Kind::Unmeasured => {
                    let unchanged = before
                        .entries
                        .get(id)
                        .is_some_and(|prior| prior.revision == entry.revision);
                    if !unchanged {
                        self.invalidate_full(crate::damage::InvalidationReason::StructuralChanged);
                    }
                }
                Kind::Paint(rect) => {
                    let prior = before.entries.get(id);
                    let same = prior.is_some_and(|prior| {
                        prior.revision == entry.revision && prior.kind == entry.kind
                    });
                    if same {
                        continue;
                    }
                    let new = (!covered(entry, after, before)).then_some(rect);
                    let old = prior.and_then(|prior| match prior.kind {
                        Kind::Paint(old) if !covered(prior, before, after) => Some(old),
                        _ => None,
                    });
                    self.record_optional(new, old);
                }
            }
        }
        for (id, entry) in &before.entries {
            if after.entries.contains_key(id) {
                continue;
            }
            match entry.kind {
                Kind::Group => self.record_optional(None, before.groups.get(id).copied()),
                Kind::Paint(rect) if !covered(entry, before, after) => {
                    self.record_optional(None, Some(rect));
                }
                Kind::Structural | Kind::Unmeasured => {
                    self.invalidate_full(crate::damage::InvalidationReason::StructuralChanged);
                }
                _ => {}
            }
        }
    }

    fn record_optional(&mut self, new: Option<Rect>, old: Option<Rect>) {
        match (new, old) {
            (Some(new), old) => self.record_rect_change(new, old),
            (None, Some(old)) => self.record_rect_change(old, None),
            (None, None) => {}
        }
    }
}
