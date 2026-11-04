use aqua_core::{InputEvent, Point};
use aqua_ui::events;

use super::Runtime;
use crate::glide;

impl Runtime {
    pub(super) fn begin_wheel(&mut self, event: &InputEvent) -> bool {
        let InputEvent::Scroll { at, delta } = event else {
            return false;
        };
        let Some(pixels) = glide::notch_pixels(*delta) else {
            return false;
        };
        if self.host.service().modal_blocker().is_some() {
            return false;
        }
        let vertical = aqua_core::ScrollDelta { x: 0.0, y: pixels };
        let Some(node) = events::scroll_target(&self.tree, *at, vertical) else {
            return false;
        };
        let (Some(offset), max) = (self.tree.scroll_offset(node), self.tree.scroll_limit(node))
        else {
            return false;
        };
        let target = (offset.y + pixels).clamp(0.0, max.y.max(0.0));
        let moved = self.tree.scroll_to(node, Point::new(offset.x, target));
        let applied = self
            .tree
            .scroll_offset(node)
            .map_or(0.0, |now| now.y - offset.y);
        if moved && applied != 0.0 {
            self.glides.push(node, applied);
            self.extend_selection_after_scroll();
        }
        true
    }
}
