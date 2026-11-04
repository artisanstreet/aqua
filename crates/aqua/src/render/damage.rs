use std::collections::HashMap;

use aqua_core::{DisplayItem, Rect, Size};
use aqua_scene::{DamageTracker, Footprints, Scene};
use aqua_text::{RunId, SharedRuns};

const MAX_REGIONS: usize = 8;
const ANTIALIAS_PAD: f32 = 1.0;
const OVERHANG_PAD: f32 = 6.0;
const OVERHANG_RATIO: f32 = 0.15;

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
    animated: bool,
    blurred: bool,
}

fn traits_of(scene: &Scene) -> SceneTraits {
    scene
        .ordered()
        .fold(SceneTraits::default(), |found, (_, item)| match item {
            DisplayItem::PushGpuiQuadPulse(_) | DisplayItem::PushGpuiTextShimmer(_) => {
                SceneTraits {
                    animated: true,
                    ..found
                }
            }
            DisplayItem::BackdropBlur { .. }
            | DisplayItem::BackdropBlurRounded { .. }
            | DisplayItem::PushContentBlur { .. } => SceneTraits {
                blurred: true,
                ..found
            },
            _ => found,
        })
}

fn ink_boxes(scene: &Scene, runs: &SharedRuns) -> HashMap<u64, Rect> {
    let mut boxes: HashMap<u64, Rect> = HashMap::new();
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
        let traits = traits_of(scene);
        let plan = match (&self.committed, self.viewport) {
            (Some(before), Some(previous)) if previous == viewport && !traits.animated => {
                diff(before, &after, viewport, traits.blurred)
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

fn diff(before: &Footprints, after: &Footprints, viewport: Viewport, blurred: bool) -> Plan {
    let physical = Rect {
        x: 0.0,
        y: 0.0,
        w: (viewport.width * viewport.scale).round().max(1.0),
        h: (viewport.height * viewport.scale).round().max(1.0),
    };
    let mut tracker = DamageTracker::new(viewport.scale);
    tracker.set_frame_size(physical.w, physical.h);
    tracker.record_scene_diff(before, after);
    let mut set = tracker.take();
    match (set.full, blurred && !set.damage.is_empty()) {
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
