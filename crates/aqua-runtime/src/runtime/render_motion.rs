use std::collections::{HashMap, HashSet};

use aqua_core::{DisplayItem, NodeId, Transform2D};
use aqua_motion::{Binding, MotionCommand, MotionValue, Property, TrackId};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum MotionSlot {
    Opacity,
    Translate,
    Fill,
    Border,
    TextColor,
    Frame,
}

fn slot_for(property: Property) -> MotionSlot {
    match property {
        Property::Opacity => MotionSlot::Opacity,
        Property::Translate => MotionSlot::Translate,
        Property::Fill => MotionSlot::Fill,
        Property::BorderColor => MotionSlot::Border,
        Property::TextColor => MotionSlot::TextColor,
        Property::Edge(_) => MotionSlot::Frame,
    }
}

pub(super) type MotionKeys = HashMap<(NodeId, MotionSlot), Vec<SceneKey>>;

pub(super) fn forget_motion_node(keys: &mut MotionKeys, node: NodeId) {
    if keys.is_empty() {
        return;
    }
    for slot in [
        MotionSlot::Opacity,
        MotionSlot::Translate,
        MotionSlot::Fill,
        MotionSlot::Border,
        MotionSlot::TextColor,
        MotionSlot::Frame,
    ] {
        keys.remove(&(node, slot));
    }
}

pub(super) fn note_motion_item(
    keys: &mut MotionKeys,
    node: NodeId,
    key: SceneKey,
    item: &DisplayItem,
) {
    let mut first = |slot: MotionSlot| {
        let entry = keys.entry((node, slot)).or_default();
        if entry.is_empty() {
            entry.push(key);
        }
    };
    let frame =
        |keys: &mut MotionKeys| keys.entry((node, MotionSlot::Frame)).or_default().push(key);
    match item {
        DisplayItem::PushOpacity(_) => first(MotionSlot::Opacity),
        DisplayItem::PushTransform(_) => first(MotionSlot::Translate),
        DisplayItem::Text { .. } => first(MotionSlot::TextColor),
        DisplayItem::Rect {
            fill,
            border_width,
            shadow,
            ..
        } => {
            let carrier = shadow.is_some() && fill.alpha <= 0.0 && *border_width <= 0.0;
            if !carrier {
                first(MotionSlot::Fill);
                if *border_width > 0.0 {
                    first(MotionSlot::Border);
                }
            }
            frame(keys);
        }
        DisplayItem::Quad { border_widths, .. } => {
            let widest = border_widths
                .left
                .max(border_widths.top)
                .max(border_widths.right)
                .max(border_widths.bottom);
            match widest > 0.0 {
                true => first(MotionSlot::Border),
                false => first(MotionSlot::Fill),
            }
            frame(keys);
        }
        DisplayItem::Shadow { .. } => frame(keys),
        _ => {}
    }
}

impl Runtime {
    pub(super) fn reconcile_motion(&mut self) {
        let key = (
            self.tree.content_version(),
            self.tree.layout_version(),
            self.viewport.w.to_bits(),
            self.viewport.h.to_bits(),
        );
        if self.motion_seen == Some(key) {
            return;
        }
        if !self.tree.has_motion_specs() && !self.motion_rt.is_active() {
            self.motion_rt.seed();
            return;
        }
        let now = self.clock;
        self.motion_rt
            .reconcile(self.host.tracks_mut(), &self.tree, now);
        let groups = self.motion_rt.groups(&self.tree);
        self.tree.set_motion_groups(groups);
        self.motion_seen = Some((
            self.tree.content_version(),
            self.tree.layout_version(),
            self.viewport.w.to_bits(),
            self.viewport.h.to_bits(),
        ));
    }

    pub fn set_motion_theme(&mut self, theme: aqua_ui::motion::MotionTheme) {
        let now = self.clock;
        self.motion_rt.set_theme(self.host.tracks_mut(), theme, now);
    }

    fn scene_targets(&self, binding: Binding) -> Vec<(NodeId, f32)> {
        let shift = 0.0;
        self.motion_keys
            .get(&(binding.node, slot_for(binding.property)))
            .into_iter()
            .flatten()
            .filter_map(|key| self.scene_map.get(key).copied())
            .map(|scene| (scene, shift))
            .collect()
    }

    fn route_motion(&mut self, drained: Vec<MotionCommand>) -> Vec<MotionCommand> {
        let owned: HashMap<TrackId, Binding> = self.motion_rt.bindings().into_iter().collect();
        let mut out = Vec::with_capacity(drained.len());
        for command in drained {
            match command {
                MotionCommand::Spawn {
                    track,
                    binding: Some(_),
                    initial,
                    kind,
                    at,
                } if owned.contains_key(&track) => out.push(MotionCommand::Spawn {
                    track,
                    binding: None,
                    initial,
                    kind,
                    at,
                }),
                MotionCommand::Remove { track } => {
                    self.motion_sent.remove(&track);
                    out.push(command);
                }
                other => out.push(other),
            }
        }
        for (track, binding) in owned {
            let desired = self.scene_targets(binding);
            let previous = self.motion_sent.get(&track);
            if previous == Some(&desired) || (previous.is_none() && desired.is_empty()) {
                continue;
            }
            if previous.is_some_and(|sent| !sent.is_empty()) {
                out.push(MotionCommand::Unbind { track });
            }
            for (scene, shift) in &desired {
                out.push(MotionCommand::Bind {
                    track,
                    binding: Binding {
                        node: *scene,
                        property: binding.property,
                    },
                    shift: *shift,
                });
            }
            self.motion_sent.insert(track, desired);
        }
        out
    }

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
            if matches!(key, SceneKey::Tree { .. } | SceneKey::ScrollOffset { .. })
                && root != portal
            {
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
        let drained = self.host.tracks_mut().drain_commands();
        self.route_motion(drained)
            .into_iter()
            .map(RenderCommand::Motion)
            .collect()
    }

    pub(super) fn deliver_motion_event(&mut self, event: &RenderEvent) -> bool {
        match event {
            RenderEvent::Motion(motion) => {
                self.host.tracks_mut().handle(*motion);
                let rows = !self
                    .motion_rt
                    .handle(self.host.tracks_mut(), &mut self.tree, *motion)
                    .is_empty();
                let popups = self.host.finish_popup_exits(&mut self.tree);
                rows || popups
            }
            _ => false,
        }
    }
}
