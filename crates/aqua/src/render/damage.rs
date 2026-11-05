use aqua_core::geometry::{Point, Transform2D};
use aqua_core::hash::FastMap;
use aqua_core::{DisplayItem, Rect, Size};
use aqua_scene::{DamageTracker, Footprints, Scene};
use aqua_text::{RunId, SharedRuns};

const MAX_REGIONS: usize = 8;
const ANTIALIAS_PAD: f32 = 1.0;
const OVERHANG_PAD: f32 = 6.0;
const OVERHANG_RATIO: f32 = 0.15;
const BLUR_REACH_PAD: f32 = 8.0;

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
}

#[derive(Default)]
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
    let reach = radius.abs() * 2.0 + BLUR_REACH_PAD;
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
    for (_, item) in scene.ordered() {
        let world = worlds.last().copied().unwrap_or(Transform2D::IDENTITY);
        match item {
            DisplayItem::PushTransform(local) => worlds.push(local.then(world)),
            DisplayItem::PopTransform => {
                worlds.pop();
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
            DisplayItem::Text { run, .. } => {
                if let Some(map) = maps.iter().rev().find_map(|map| *map) {
                    let local = ink.get(&run.id).map_or(map, |ink| ink.union(map));
                    traits.animated.push(transformed(local, world));
                }
            }
            DisplayItem::BackdropBlur { bounds, radius }
            | DisplayItem::BackdropBlurRounded { bounds, radius, .. }
            | DisplayItem::PushContentBlur { bounds, radius } => traits
                .blurs
                .push(blur_zone(transformed(*bounds, world), *radius)),
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

    pub fn plan(&mut self, scene: &Scene, runs: &SharedRuns, size: Size, scale: f32) -> Plan {
        let viewport = Viewport {
            width: size.w,
            height: size.h,
            scale,
        };
        let ink = ink_boxes(scene, runs);
        let after = Footprints::of_scene(scene, &|run| ink.get(&run.id).copied());
        let traits = traits_of(scene, &ink);
        let plan = match (&self.committed, self.viewport) {
            (Some(before), Some(previous)) if previous == viewport => {
                diff(before, &after, viewport, &traits.blurs, &traits.animated)
            }
            _ => Plan::Full,
        };
        self.staged = Some(after);
        self.staged_viewport = Some(viewport);
        plan
    }

    pub fn commit(&mut self) {
        if let Some(staged) = self.staged.take() {
            self.committed = Some(staged);
            self.viewport = self.staged_viewport.take();
        }
    }

    pub fn invalidate(&mut self) {
        self.committed = None;
        self.viewport = None;
        self.staged = None;
        self.staged_viewport = None;
    }
}

fn diff(
    before: &Footprints,
    after: &Footprints,
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
    tracker.record_scene_diff(before, after);
    animated
        .iter()
        .for_each(|rect| tracker.record_rect_change(*rect, None));
    let mut set = tracker.take();
    let scale = viewport.scale;
    let behind_blur = set.damage.iter().any(|damage| {
        blurs.iter().any(|zone| {
            let zone = Rect {
                x: zone.x * scale,
                y: zone.y * scale,
                w: zone.w * scale,
                h: zone.h * scale,
            };
            damage.intersect(zone).w > 0.0 && damage.intersect(zone).h > 0.0
        })
    });
    match (set.full, behind_blur) {
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
