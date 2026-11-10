use std::collections::{HashMap, HashSet};

use aqua_core::{DisplayItem, NodeId, Size};
use aqua_motion::{
    Binding, MotionCommand, MotionEvent, MotionValue, Property, ReducedMotion, Sampled, TrackId,
    TrackSet,
};
use aqua_protocol::{
    Feedback, FrameId, RenderCommand, RenderEvent, RenderInput, RenderOutput, RenderWake, SceneDiff,
};
use aqua_scene::Scene;

pub mod property;
pub mod scroll;

type Overrides = Vec<(Property, MotionValue)>;

fn shifted(property: Property, value: MotionValue, shift: f32) -> MotionValue {
    match (property, value) {
        (Property::Edge(_), MotionValue::Scalar(position)) => MotionValue::Scalar(position + shift),
        _ => value,
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Submitted {
    revision: u64,
    viewport: Option<(Size, f32)>,
}

pub struct RenderSide {
    scene: Scene,
    local: HashMap<NodeId, NodeId>,
    local_slots: Vec<Option<(u32, NodeId)>>,
    base: HashMap<NodeId, DisplayItem>,
    overrides: HashMap<NodeId, Overrides>,
    bindings: HashMap<TrackId, Vec<(Binding, f32)>>,
    tracks: TrackSet,
    dirty: Vec<NodeId>,
    seen: HashSet<NodeId>,
    sampled: Sampled,
    viewport: Option<(Size, f32)>,
    next_frame: FrameId,
    submitted: Option<Submitted>,
    overlay: Vec<NodeId>,
}

impl Default for RenderSide {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderSide {
    #[must_use]
    pub fn new() -> Self {
        Self {
            scene: Scene::new(),
            local: HashMap::new(),
            local_slots: Vec::new(),
            base: HashMap::new(),
            overrides: HashMap::new(),
            bindings: HashMap::new(),
            tracks: TrackSet::new(ReducedMotion::Full),
            dirty: Vec::new(),
            seen: HashSet::new(),
            sampled: Sampled::default(),
            viewport: None,
            next_frame: 1,
            submitted: None,
            overlay: Vec::new(),
        }
    }

    #[must_use]
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }

    #[must_use]
    pub fn tracks(&self) -> &TrackSet {
        &self.tracks
    }

    #[must_use]
    pub fn viewport(&self) -> Option<(Size, f32)> {
        self.viewport
    }

    pub fn set_overlay(&mut self, items: Vec<DisplayItem>) {
        let keep = items.len().min(self.overlay.len());
        for id in self.overlay.split_off(keep) {
            self.scene.remove(id);
        }
        let mut items = items.into_iter();
        for (id, item) in self.overlay.iter().zip(items.by_ref()) {
            self.scene.update(*id, item);
        }
        for item in items {
            self.overlay.push(self.scene.insert(item));
        }
        if let Some(submitted) = self.submitted.as_mut() {
            submitted.revision = self.scene.revision();
        }
        if self.overlay.is_empty() {
            return;
        }
        let overlay: HashSet<NodeId> = self.overlay.iter().copied().collect();
        let ids: Vec<NodeId> = self
            .scene
            .ordered()
            .map(|(id, _)| id)
            .filter(|id| !overlay.contains(id))
            .chain(self.overlay.iter().copied())
            .collect();
        let on_top = self
            .scene
            .ordered()
            .map(|(id, _)| id)
            .skip(ids.len() - self.overlay.len())
            .eq(self.overlay.iter().copied());
        if !on_top {
            self.scene.set_order(&ids);
        }
        if let Some(submitted) = self.submitted.as_mut() {
            submitted.revision = self.scene.revision();
        }
    }

    #[must_use]
    pub fn presented(&self, node: NodeId) -> Option<&DisplayItem> {
        self.local_id(node).and_then(|id| self.scene.get(id))
    }

    #[inline]
    fn local_id(&self, node: NodeId) -> Option<NodeId> {
        let raw = node.raw();
        self.local_slots
            .get(raw.index as usize)
            .and_then(|entry| *entry)
            .filter(|&(generation, _)| generation == raw.generation)
            .map(|(_, id)| id)
            .or_else(|| self.local.get(&node).copied())
    }

    pub fn step(&mut self, input: RenderInput) -> RenderOutput {
        let mut events = Vec::new();
        for feedback in input.feedback {
            events.push(self.feedback(feedback));
        }
        for command in input.commands {
            match command {
                RenderCommand::Scene(diff) => self.apply_scene(diff),
                RenderCommand::Motion(motion) => {
                    events.extend(
                        self.apply_motion(motion)
                            .into_iter()
                            .map(RenderEvent::Motion),
                    );
                }
                RenderCommand::Viewport { size, scale } => self.viewport = Some((size, scale)),
            }
        }
        let mut sampled = std::mem::take(&mut self.sampled);
        self.tracks
            .sample_into(input.present_at.as_secs_f64(), &mut sampled);
        for presented in &sampled.values {
            self.present(presented.track, presented.value);
        }
        events.extend(sampled.events.iter().copied().map(RenderEvent::Motion));
        self.sampled = sampled;
        self.refresh();
        let frame = self.next_submission();
        RenderOutput {
            events,
            frame,
            wake: self.wake(),
        }
    }

    #[must_use]
    pub fn wake(&self) -> RenderWake {
        match self.tracks.is_idle() {
            true => RenderWake::Idle,
            false => RenderWake::NextFrame,
        }
    }

    pub fn feedback(&mut self, feedback: Feedback) -> RenderEvent {
        match feedback {
            Feedback::Presented(frame) => RenderEvent::Presented(frame),
            Feedback::DeviceLost => RenderEvent::DeviceLost,
            Feedback::DeviceRecovered => {
                self.submitted = None;
                RenderEvent::DeviceRecovered
            }
        }
    }

    fn next_submission(&mut self) -> Option<FrameId> {
        let current = Submitted {
            revision: self.scene.revision(),
            viewport: self.viewport,
        };
        match self.submitted == Some(current) {
            true => None,
            false => {
                self.submitted = Some(current);
                let id = self.next_frame;
                self.next_frame += 1;
                Some(id)
            }
        }
    }

    fn apply_scene(&mut self, diff: SceneDiff) {
        for (node, item) in diff.upserts {
            match self.base.contains_key(&node) || self.overrides.contains_key(&node) {
                true => {
                    self.base.insert(node, item);
                    self.dirty.push(node);
                }
                false => self.write(node, item),
            }
        }
        for patch in diff.fills {
            if let Some(id) = self.local_id(patch.node) {
                if let Some(base) = self.base.get_mut(&patch.node) {
                    if !base.set_fill(patch.fill) {
                        continue;
                    }
                    self.dirty.push(patch.node);
                } else {
                    self.scene.update_fill(id, patch.fill);
                }
            }
        }
        for mut batch in diff.srgba_fills {
            batch.retain_mut(|patch| {
                let Some(id) = self.local_id(patch.node) else {
                    return false;
                };
                if let Some(base) = self.base.get_mut(&patch.node) {
                    if base.set_srgba(patch.fill) {
                        self.dirty.push(patch.node);
                    }
                    false
                } else {
                    patch.node = id;
                    true
                }
            });
            self.scene.apply_srgba_batch(batch);
        }
        if let Some(order) = diff.order {
            let keep: HashSet<NodeId> = order.iter().copied().collect();
            let gone: Vec<NodeId> = self
                .local
                .keys()
                .filter(|node| !keep.contains(node))
                .copied()
                .collect();
            for node in gone {
                if let Some(id) = self.local.remove(&node) {
                    self.scene.remove(id);
                }
                if let Some(slot) = self.local_slots.get_mut(node.raw().index as usize) {
                    if slot.is_some_and(|(generation, _)| generation == node.raw().generation) {
                        *slot = None;
                    }
                }
                self.base.remove(&node);
                self.overrides.remove(&node);
            }
            let ids: Vec<NodeId> = order
                .iter()
                .filter_map(|node| self.local.get(node).copied())
                .chain(self.overlay.iter().copied())
                .collect();
            self.scene.set_order(&ids);
        }
    }

    fn write(&mut self, node: NodeId, item: DisplayItem) {
        match self.local.get(&node) {
            Some(id) => {
                self.scene.update(*id, item);
            }
            None => {
                let id = self.scene.insert(item);
                self.local.insert(node, id);
                let raw = node.raw();
                let index = raw.index as usize;
                if self.local_slots.len() <= index {
                    self.local_slots.resize(index + 1, None);
                }
                self.local_slots[index] = Some((raw.generation, id));
            }
        }
    }

    fn apply_motion(&mut self, command: MotionCommand) -> Vec<MotionEvent> {
        match command {
            MotionCommand::Spawn {
                track,
                binding: Some(binding),
                ..
            } => self.bind(track, binding, 0.0),
            MotionCommand::Bind {
                track,
                binding,
                shift,
            } => self.bind(track, binding, shift),
            MotionCommand::Remove { track } | MotionCommand::Unbind { track } => self.unbind(track),
            _ => {}
        }
        self.tracks.apply(command)
    }

    fn bind(&mut self, track: TrackId, binding: Binding, shift: f32) {
        let list = self.bindings.entry(track).or_default();
        match list.iter_mut().find(|(existing, _)| *existing == binding) {
            Some(entry) => entry.1 = shift,
            None => list.push((binding, shift)),
        }
        if !self.base.contains_key(&binding.node) {
            if let Some(item) = self.presented(binding.node).cloned() {
                self.base.insert(binding.node, item);
            }
        }
        if let Some(value) = self.tracks.value(track) {
            self.set_override(binding, shifted(binding.property, value, shift));
        }
    }

    fn present(&mut self, track: TrackId, value: MotionValue) {
        let Some(list) = self.bindings.get(&track) else {
            return;
        };
        for (binding, shift) in list {
            record_override(
                &mut self.overrides,
                &mut self.dirty,
                *binding,
                shifted(binding.property, value, *shift),
            );
        }
    }

    fn unbind(&mut self, track: TrackId) {
        let Some(released) = self.bindings.remove(&track) else {
            return;
        };
        for (binding, _) in released {
            if let Some(overrides) = self.overrides.get_mut(&binding.node) {
                overrides.retain(|(property, _)| *property != binding.property);
            }
            let still_bound = self
                .bindings
                .values()
                .flatten()
                .any(|(other, _)| other.node == binding.node);
            match still_bound {
                true => self.dirty.push(binding.node),
                false => {
                    self.overrides.remove(&binding.node);
                    if let Some(item) = self.base.remove(&binding.node) {
                        self.write(binding.node, item);
                    }
                }
            }
        }
    }

    fn set_override(&mut self, binding: Binding, value: MotionValue) {
        record_override(&mut self.overrides, &mut self.dirty, binding, value);
    }

    fn refresh(&mut self) {
        self.seen.clear();
        let mut dirty = std::mem::take(&mut self.dirty);
        for node in dirty.drain(..) {
            if !self.seen.insert(node) {
                continue;
            }
            let Some(mut item) = self.base.get(&node).cloned() else {
                continue;
            };
            if let Some(overrides) = self.overrides.get(&node) {
                for (property, value) in overrides {
                    let _ = property::apply(&mut item, *property, *value);
                }
            }
            self.write(node, item);
        }
        self.dirty = dirty;
    }
}

fn record_override(
    overrides: &mut HashMap<NodeId, Overrides>,
    dirty: &mut Vec<NodeId>,
    binding: Binding,
    value: MotionValue,
) {
    let entry = overrides.entry(binding.node).or_default();
    match entry
        .iter_mut()
        .find(|(property, _)| *property == binding.property)
    {
        Some(existing) => existing.1 = value,
        None => entry.push((binding.property, value)),
    }
    dirty.push(binding.node);
}
