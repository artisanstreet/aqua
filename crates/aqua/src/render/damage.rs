use aqua_core::geometry::{Point, Transform2D};
use aqua_core::hash::FastMap;
use aqua_core::NodeId;
use aqua_core::{DisplayItem, Rect, Size};
use aqua_scene::{DamageTracker, Footprints, Scene};
use aqua_text::{RunId, SharedRuns};

const MAX_REGIONS: usize = 8;
const ANTIALIAS_PAD: f32 = 1.0;
const OVERHANG_PAD: f32 = 6.0;
const OVERHANG_RATIO: f32 = 0.15;
const BLUR_REACH_PAD: f32 = 16.0;
const BLUR_REACH_RADII: f32 = 4.0;
const FULL_AREA_RATIO: f32 = 0.75;

#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    Full,
    Regions(Vec<Rect>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Viewport {
    width: f32,
    height: f32,
    scale: f32,
}

#[derive(Debug, Default)]
pub struct DamagePlanner {
    committed: Option<Footprints>,
    staged: Option<Footprints>,
    viewport: Option<Viewport>,
    staged_viewport: Option<Viewport>,
    committed_revision: Option<u64>,
    staged_revision: Option<u64>,
    traits: Option<(u64, SceneTraits)>,
    seen: FastMap<NodeId, u64>,
}

#[derive(Debug, Default)]
struct SceneTraits {
    animated: Vec<Rect>,
    blurs: Vec<Rect>,
}

fn transformed(rect: Rect, transform: Transform2D) -> Rect {
    let corners = [
        Point::new(rect.x, rect.y),
        Point::new(rect.right(), rect.y),
        Point::new(rect.x, rect.bottom()),
        Point::new(rect.right(), rect.bottom()),
    ]
    .map(|corner| transform.apply(corner));
    let edge = |pick: fn(Point) -> f32, merge: fn(f32, f32) -> f32, seed: f32| {
        corners
            .iter()
            .fold(seed, |found, corner| merge(found, pick(*corner)))
    };
    Rect::from_edges(
        edge(|p| p.x, f32::min, f32::INFINITY),
        edge(|p| p.y, f32::min, f32::INFINITY),
        edge(|p| p.x, f32::max, f32::NEG_INFINITY),
        edge(|p| p.y, f32::max, f32::NEG_INFINITY),
    )
}

fn blur_zone(bounds: Rect, radius: f32) -> Rect {
    let reach = radius.abs() * BLUR_REACH_RADII + BLUR_REACH_PAD;
    Rect {
        x: bounds.x - reach,
        y: bounds.y - reach,
        w: bounds.w + 2.0 * reach,
        h: bounds.h + 2.0 * reach,
    }
}

fn traits_of(scene: &Scene, ink: &FastMap<u64, Rect>) -> SceneTraits {
    let mut traits = SceneTraits::default();
    let mut worlds: Vec<Transform2D> = Vec::new();
    let mut pulses = 0usize;
    let mut maps: Vec<Option<Rect>> = Vec::new();
    let mut clips: Vec<Option<Rect>> = Vec::new();
    for (_, item) in scene.ordered() {
        let world = worlds.last().copied().unwrap_or(Transform2D::IDENTITY);
        match item {
            DisplayItem::PushTransform(local) => worlds.push(local.then(world)),
            DisplayItem::PopTransform => {
                worlds.pop();
            }
            DisplayItem::PushClip { bounds, radius } => {
                clips.push((*radius > 0.0).then(|| transformed(*bounds, world)));
            }
            DisplayItem::PushRoundedClip { bounds, .. } => {
                clips.push(Some(transformed(*bounds, world)));
            }
            DisplayItem::PushGpuiContentMask { .. } => clips.push(None),
            DisplayItem::PopClip => {
                clips.pop();
            }
            DisplayItem::PushGpuiQuadPulse(_) => pulses += 1,
            DisplayItem::PopGpuiQuadPulse => pulses = pulses.saturating_sub(1),
            DisplayItem::PushGpuiTextColorMap(_) => maps.push(None),
            DisplayItem::PushGpuiTextShimmer(shimmer) => maps.push(Some(shimmer.map.bounds)),
            DisplayItem::PopGpuiTextColorMap => {
                maps.pop();
            }
            DisplayItem::Quad { bounds, .. }
            | DisplayItem::GpuiBackground { bounds, .. }
            | DisplayItem::GpuiBorder { bounds, .. }
                if pulses > 0 =>
            {
                traits.animated.push(transformed(*bounds, world));
            }
            DisplayItem::Shader {
                bounds,
                animated: true,
                ..
            } => {
                traits.animated.push(transformed(*bounds, world));
            }
            DisplayItem::Shape { damage, .. } => {
                traits.animated.push(transformed(*damage, world));
            }
            DisplayItem::Text { run, .. } => {
                if let Some(map) = maps.iter().rev().find_map(|map| *map) {
                    let local = ink.get(&run.id).map_or(map, |ink| ink.union(map));
                    traits.animated.push(transformed(local, world));
                }
            }
            DisplayItem::BackdropBlur { bounds, radius }
            | DisplayItem::BackdropBlurRounded { bounds, radius, .. }
            | DisplayItem::PushContentBlur { bounds, radius } => {
                traits
                    .blurs
                    .push(blur_zone(transformed(*bounds, world), *radius));
                for clip in clips.iter().flatten() {
                    if !traits.blurs.contains(clip) {
                        traits.blurs.push(*clip);
                    }
                }
            }
            _ => {}
        }
    }
    traits
}

fn ink_boxes(scene: &Scene, runs: &SharedRuns) -> FastMap<u64, Rect> {
    let mut boxes: FastMap<u64, Rect> = FastMap::default();
    let guard = runs.lock();
    for (_, item) in scene.ordered() {
        let DisplayItem::Text { at, run, .. } = item else {
            continue;
        };
        let Some((width, height)) = guard
            .resolve(RunId(run.id))
            .and_then(|handle| handle.layout.laid_out_shared())
            .map(|laid| laid.size)
        else {
            continue;
        };
        let pad = OVERHANG_PAD + OVERHANG_RATIO * height;
        let rect = Rect {
            x: at[0] - pad,
            y: at[1] - pad,
            w: width + 2.0 * pad,
            h: height + 2.0 * pad,
        };
        boxes
            .entry(run.id)
            .and_modify(|existing| *existing = existing.union(rect))
            .or_insert(rect);
    }
    boxes
}

fn pixel_rect(rect: Rect, frame: Rect) -> Option<Rect> {
    let padded = Rect::from_edges(
        (rect.x - ANTIALIAS_PAD).floor(),
        (rect.y - ANTIALIAS_PAD).floor(),
        (rect.right() + ANTIALIAS_PAD).ceil(),
        (rect.bottom() + ANTIALIAS_PAD).ceil(),
    );
    let clipped = padded.intersect(frame);
    (clipped.w > 0.0 && clipped.h > 0.0).then_some(clipped)
}

impl DamagePlanner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn plan(
        &mut self,
        scene: &Scene,
        runs: &SharedRuns,
        size: Size,
        scale: f32,
        stable: bool,
    ) -> Plan {
        let viewport = Viewport {
            width: size.w,
            height: size.h,
            scale,
        };
        let revision = scene.revision();
        if crate::profile::enabled() {
            self.note_changes(scene);
        }
        let unchanged = stable
            && self.committed.is_some()
            && self.viewport == Some(viewport)
            && self.committed_revision == Some(revision);
        match (unchanged, self.traits.as_ref()) {
            (true, Some((cached, traits))) if *cached == revision => {
                self.staged = None;
                self.staged_viewport = None;
                self.staged_revision = None;
                return diff(None, viewport, &traits.blurs, &traits.animated);
            }
            _ => {}
        }
        let ink = ink_boxes(scene, runs);
        let after = Footprints::of_scene(scene, &|run| ink.get(&run.id).copied());
        let traits = traits_of(scene, &ink);
        let plan = match (&self.committed, self.viewport) {
            (Some(before), Some(previous)) if previous == viewport => diff(
                Some((before, &after)),
                viewport,
                &traits.blurs,
                &traits.animated,
            ),
            _ => Plan::Full,
        };
        self.staged = Some(after);
        self.staged_viewport = Some(viewport);
        self.staged_revision = Some(revision);
        self.traits = Some((revision, traits));
        plan
    }

    pub fn repaint_bounds(&self, region: Rect) -> Rect {
        let Some(viewport) = self.staged_viewport.or(self.viewport) else {
            return region;
        };
        let Some((_, traits)) = &self.traits else {
            return region;
        };
        let scale = viewport.scale;
        let frame = Rect {
            x: 0.0,
            y: 0.0,
            w: viewport.width * scale,
            h: viewport.height * scale,
        };
        let zones: Vec<Rect> = traits
            .blurs
            .iter()
            .map(|zone| {
                Rect::from_edges(
                    (zone.x * scale).floor(),
                    (zone.y * scale).floor(),
                    (zone.right() * scale).ceil(),
                    (zone.bottom() * scale).ceil(),
                )
                .intersect(frame)
            })
            .collect();
        let mut damage = vec![region];
        absorb_blur_zones(&mut damage, &zones);
        damage
            .into_iter()
            .reduce(Rect::union)
            .unwrap_or(region)
            .intersect(frame)
    }

    fn note_changes(&mut self, scene: &Scene) {
        let mut changed: Vec<String> = Vec::new();
        let mut count = 0usize;
        let mut now: FastMap<NodeId, u64> = FastMap::default();
        for (id, item) in scene.ordered() {
            let revision = scene.item_revision(id).unwrap_or(0);
            now.insert(id, revision);
            if self.seen.get(&id) != Some(&revision) {
                count += 1;
                if changed.len() < 4 {
                    let text = format!("{item:?}");
                    changed.push(text.chars().take(70).collect());
                }
            }
        }
        let removed = self.seen.keys().filter(|id| !now.contains_key(id)).count();
        self.seen = now;
        crate::profile::note(|| format!("changed={count} removed={removed} first={changed:?}"));
    }

    pub fn commit(&mut self) {
        if let Some(staged) = self.staged.take() {
            self.committed = Some(staged);
            self.viewport = self.staged_viewport.take();
            self.committed_revision = self.staged_revision.take();
        }
    }

    pub fn invalidate(&mut self) {
        self.committed = None;
        self.viewport = None;
        self.staged = None;
        self.staged_viewport = None;
        self.committed_revision = None;
        self.staged_revision = None;
        self.traits = None;
    }
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

fn absorb_blur_zones(damage: &mut Vec<Rect>, zones: &[Rect]) -> bool {
    let mut absorbed = vec![false; zones.len()];
    loop {
        let mut grew = false;
        for (zone, taken) in zones.iter().zip(absorbed.iter_mut()) {
            if !*taken
                && damage
                    .iter()
                    .copied()
                    .reduce(Rect::union)
                    .is_some_and(|rect| overlaps(rect, *zone))
            {
                damage.push(*zone);
                *taken = true;
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    absorbed.contains(&true)
}

fn diff(
    changes: Option<(&Footprints, &Footprints)>,
    viewport: Viewport,
    blurs: &[Rect],
    animated: &[Rect],
) -> Plan {
    let physical = Rect {
        x: 0.0,
        y: 0.0,
        w: (viewport.width * viewport.scale).round().max(1.0),
        h: (viewport.height * viewport.scale).round().max(1.0),
    };
    let mut tracker = DamageTracker::new(viewport.scale);
    tracker.set_frame_size(physical.w, physical.h);
    if let Some((before, after)) = changes {
        tracker.record_scene_diff(before, after);
    }
    animated
        .iter()
        .for_each(|rect| tracker.record_rect_change(*rect, None));
    let mut set = tracker.take();
    let scale = viewport.scale;
    let widened = !set.full && {
        let zones: Vec<Rect> = blurs
            .iter()
            .map(|zone| Rect {
                x: zone.x * scale,
                y: zone.y * scale,
                w: zone.w * scale,
                h: zone.h * scale,
            })
            .collect();
        absorb_blur_zones(&mut set.damage, &zones)
    };
    let mostly_frame = widened
        && set
            .damage
            .iter()
            .copied()
            .reduce(Rect::union)
            .is_some_and(|bound| bound.w * bound.h > physical.w * physical.h * FULL_AREA_RATIO);
    match (set.full, mostly_frame) {
        (true, _) | (_, true) => Plan::Full,
        _ => {
            set.coalesce(MAX_REGIONS);
            let rects: Vec<Rect> = set
                .damage
                .iter()
                .filter_map(|rect| pixel_rect(*rect, physical))
                .collect();
            let covers_frame = rects.iter().any(|rect| {
                rect.x <= 0.0
                    && rect.y <= 0.0
                    && rect.right() >= physical.w
                    && rect.bottom() >= physical.h
            });
            match (rects.is_empty(), covers_frame) {
                (_, true) => Plan::Full,
                (true, false) => Plan::Regions(vec![Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                }]),
                (false, false) => Plan::Regions(rects),
            }
        }
    }
}
