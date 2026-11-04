use aqua_core::{InputEvent, Point, ScrollDelta};
use aqua_ui::events;

use super::Runtime;
use crate::glide::{self, Glide};

const FRAME_INTERVAL: f64 = 1.0 / 60.0;

impl Runtime {
    pub(super) fn begin_wheel(&mut self, event: &InputEvent) -> bool {
        match event {
            InputEvent::PointerDown(..) | InputEvent::KeyDown(_) => {
                self.wheel = None;
                false
            }
            InputEvent::Scroll { at, delta } => {
                let Some(pixels) = glide::notch_pixels(*delta) else {
                    self.wheel = None;
                    return false;
                };
                if self.host.service().modal_blocker().is_some() {
                    return false;
                }
                let vertical = ScrollDelta { x: 0.0, y: pixels };
                let Some(node) = events::scroll_target(&self.tree, *at, vertical) else {
                    return false;
                };
                let (Some(offset), max) =
                    (self.tree.scroll_offset(node), self.tree.scroll_limit(node))
                else {
                    return false;
                };
                self.wheel = Some(glide::push(
                    self.wheel, node, pixels, offset.y, max.y, self.clock,
                ));
                true
            }
            _ => false,
        }
    }

    pub(super) fn wheel_deadline(&self) -> Option<f64> {
        self.wheel.map(|_| self.clock + FRAME_INTERVAL)
    }

    pub(super) fn advance_wheel(&mut self) -> bool {
        let Some(active) = self.wheel else {
            return false;
        };
        let Some(offset) = self.tree.scroll_offset(active.node) else {
            self.wheel = None;
            return false;
        };
        let elapsed = (self.clock - active.advanced_at).clamp(1.0 / 240.0, 0.05) as f32;
        let (applied, left) = glide::step(active.remaining, elapsed);
        self.wheel = (left != 0.0).then_some(Glide {
            remaining: left,
            advanced_at: self.clock,
            ..active
        });
        let changed = self
            .tree
            .scroll_to(active.node, Point::new(offset.x, offset.y + applied));
        if changed {
            self.extend_selection_after_scroll();
        }
        if self.wheel.is_some() && !changed {
            self.wheel = None;
        }
        changed
    }
}
