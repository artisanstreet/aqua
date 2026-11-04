use std::collections::HashSet;

use aqua_core::{DisplayItem, NodeId, Transform2D};
use aqua_motion::{MotionValue, Property};
use aqua_protocol::{RenderCommand, RenderEvent};
use aqua_ui::{overlay::OverlayId, overlay_host::PopupMotion, tree::NodeKind};

use super::{Runtime, SceneKey};

const OPACITY_SLOT: u8 = 0;
const TRANSLATE_SLOT: u8 = 1;

fn slot_of(property: Property) -> Option<u8> {
    match property {
        Property::Opacity => Some(OPACITY_SLOT),
        Property::Translate => Some(TRANSLATE_SLOT),
        _ => None,
    }
}

fn group_open(property: Property, from: MotionValue) -> Option<DisplayItem> {
    match (property, from) {
        (Property::Opacity, MotionValue::Scalar(opacity)) => {
            Some(DisplayItem::PushOpacity(opacity))
        }
        (Property::Translate, MotionValue::Pair(x, y)) => {
            Some(DisplayItem::PushTransform(Transform2D::translate(x, y)))
        }
        _ => None,
    }
}

fn group_close(property: Property) -> Option<DisplayItem> {
    match property {
        Property::Opacity => Some(DisplayItem::PopOpacity),
        Property::Translate => Some(DisplayItem::PopTransform),
        _ => None,
    }
}

fn opens_clip(item: &DisplayItem) -> bool {
    matches!(
        item,
        DisplayItem::PushClip { .. }
            | DisplayItem::PushRoundedClip { .. }
            | DisplayItem::PushGpuiContentMask { .. }
    )
}

impl Runtime {
    pub fn animate_popup(&mut self, overlay: OverlayId, motion: PopupMotion) -> bool {
        let animated = self.host.animate_popup(overlay, motion, self.clock);
        if animated {
            self.cached_version = None;
        }
        animated
    }

    pub(super) fn wrap_motion_groups(
        &self,
        items: Vec<(SceneKey, DisplayItem)>,
    ) -> Vec<(SceneKey, DisplayItem)> {
        let wraps: Vec<(NodeId, Property, MotionValue)> = self
            .host
            .popup_wraps()
            .filter(|(_, property, _)| slot_of(*property).is_some())
            .collect();
        let mut out = Vec::with_capacity(items.len() + 2 * wraps.len());
        let mut scrolls_open: HashSet<NodeId> = HashSet::new();
        let mut scrolls_done: HashSet<NodeId> = HashSet::new();
        let mut portal: Option<NodeId> = None;
        let mut roots: std::collections::HashMap<NodeId, Option<NodeId>> =
            std::collections::HashMap::new();
        let close_portal = |out: &mut Vec<(SceneKey, DisplayItem)>, root: NodeId| {
            for (_, property, _) in wraps.iter().rev().filter(|(r, ..)| *r == root) {
                if let (Some(slot), Some(item)) = (slot_of(*property), group_close(*property)) {
                    out.push((SceneKey::PortalEnd { node: root, slot }, item));
                }
            }
        };
        for (key, item) in items {
            let node = key.node();
            let root = match wraps.is_empty() {
                true => None,
                false => *roots.entry(node).or_insert_with(|| self.portal_root(node)),
            };
            if matches!(key, SceneKey::Tree { .. }) && root != portal {
                if let Some(previous) = portal.take() {
                    close_portal(&mut out, previous);
                }
                portal = root;
                if let Some(root) = root {
                    for (_, property, from) in wraps.iter().filter(|(r, ..)| *r == root) {
                        if let (Some(slot), Some(open)) =
                            (slot_of(*property), group_open(*property, *from))
                        {
                            out.push((SceneKey::PortalStart { node: root, slot }, open));
                        }
                    }
                }
            }
            let is_scroll = matches!(key, SceneKey::Tree { .. })
                && self
                    .tree
                    .get(node)
                    .is_some_and(|record| matches!(record.kind, NodeKind::Scroll(_)));
            match (is_scroll, &item) {
                (true, item)
                    if opens_clip(item)
                        && !scrolls_done.contains(&node)
                        && !scrolls_open.contains(&node) =>
                {
                    scrolls_open.insert(node);
                    out.push((key, item.clone()));
                    out.push((
                        SceneKey::ScrollContentStart { node },
                        DisplayItem::PushTransform(Transform2D::IDENTITY),
                    ));
                }
                (true, DisplayItem::PopClip) if scrolls_open.remove(&node) => {
                    scrolls_done.insert(node);
                    out.push((
                        SceneKey::ScrollContentEnd { node },
                        DisplayItem::PopTransform,
                    ));
                    out.push((key, item));
                }
                _ => out.push((key, item)),
            }
        }
        if let Some(last) = portal {
            close_portal(&mut out, last);
        }
        out
    }

    fn portal_root(&self, node: NodeId) -> Option<NodeId> {
        let mut current = Some(node);
        while let Some(id) = current {
            if self.tree.is_portal(id) {
                return Some(id);
            }
            current = self.tree.get(id).and_then(|record| record.parent);
        }
        None
    }

    pub(super) fn motion_commands(&mut self) -> Vec<RenderCommand> {
        let now = self.clock;
        let wanted = self.host.motion().reduced_motion();
        if self.host.tracks().reduced_motion() != wanted {
            self.host.tracks_mut().set_reduced_motion(wanted, now);
        }
        let scene_map = &self.scene_map;
        let tree = &self.tree;
        self.host.bind_popup_tracks(now, |root, property| {
            let slot = slot_of(property)?;
            scene_map
                .get(&SceneKey::PortalStart { node: root, slot })
                .copied()
        });
        let tracks = self.host.tracks_mut();
        self.glides.flush(
            tracks,
            now,
            |node| {
                scene_map
                    .get(&SceneKey::ScrollContentStart { node })
                    .copied()
            },
            |node| tree.get(node).is_some(),
        );
        self.host
            .tracks_mut()
            .drain_commands()
            .into_iter()
            .map(RenderCommand::Motion)
            .collect()
    }

    pub(super) fn deliver_motion_event(&mut self, event: &RenderEvent) {
        if let RenderEvent::Motion(motion) = event {
            self.host.tracks_mut().handle(*motion);
        }
    }
}
