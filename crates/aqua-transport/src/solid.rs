use aqua_core::{DisplayItem, Oklch, Rect};
use aqua_graph::{plan_paint_with, Extent, FramePlan, PassKind, PlanLimits, SolidInstance};
use aqua_scene::Scene;

use crate::LowerContext;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Key {
    geometry: u64,
    width: u32,
    height: u32,
    scale: u32,
}

impl Key {
    pub(crate) fn new(scene: &Scene, context: &LowerContext) -> Self {
        Self {
            geometry: scene.geometry_revision(),
            width: context.size.w.to_bits(),
            height: context.size.h.to_bits(),
            scale: context.scale.to_bits(),
        }
    }
}

pub(crate) struct Solids {
    pub(crate) key: Key,
    revision: u64,
    slots: Vec<Option<(u32, usize)>>,
    pub(crate) plan: FramePlan,
}

impl Solids {
    pub(crate) fn compile(
        scene: &mut Scene,
        context: &LowerContext,
        extent: Extent,
        clear: Oklch,
        limits: PlanLimits,
    ) -> Option<Self> {
        if scene.is_empty() {
            return None;
        }
        let mut instances = Vec::with_capacity(scene.len());
        let mut slots = Vec::new();
        for (node, item) in scene.ordered() {
            let DisplayItem::Rect {
                bounds,
                radius: 0.0,
                fill,
                border_width: 0.0,
                border_color,
                shadow: None,
                ..
            } = item
            else {
                return None;
            };
            if border_color.alpha != 0.0 {
                return None;
            }
            let index = instances.len();
            let raw = node.raw();
            if slots.len() <= raw.index as usize {
                slots.resize(raw.index as usize + 1, None);
            }
            slots[raw.index as usize] = Some((raw.generation, index));
            instances.push(SolidInstance {
                bounds: Rect {
                    x: bounds.x * context.scale,
                    y: bounds.y * context.scale,
                    w: bounds.w * context.scale,
                    h: bounds.h * context.scale,
                },
                fill: *fill,
            });
        }
        let mut plan = plan_paint_with(extent, clear, &[], limits).ok()?;
        let pass = plan.passes.first_mut()?;
        pass.kind = PassKind::Solids { instances };
        scene.acknowledge_changes();
        Some(Self {
            key: Key::new(scene, context),
            revision: scene.revision(),
            slots,
            plan,
        })
    }

    pub(crate) fn update(&mut self, scene: &mut Scene, context: &LowerContext) -> bool {
        if self.key != Key::new(scene, context) {
            return false;
        }
        let Some(changes) = scene.drain_fill_changes(self.revision) else {
            return false;
        };
        let PassKind::Solids { instances } = &mut self.plan.passes[0].kind else {
            return false;
        };
        for patch in &changes {
            let raw = patch.node.raw();
            let Some((generation, index)) =
                self.slots.get(raw.index as usize).and_then(|slot| *slot)
            else {
                return false;
            };
            if generation != raw.generation {
                return false;
            }
            instances[index].fill = patch.fill;
        }
        scene.recycle_fills(changes);
        self.revision = scene.revision();
        true
    }
}
