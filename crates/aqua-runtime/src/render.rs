use std::collections::{HashMap, HashSet};

use aqua_core::{DisplayItem, NodeId, Size};
use aqua_motion::{
    Binding, MotionCommand, MotionEvent, MotionValue, Property, ReducedMotion, TrackId, TrackSet,
};
use aqua_protocol::{
    Feedback, FrameId, RenderCommand, RenderEvent, RenderInput, RenderOutput, RenderWake, SceneDiff,
};
use aqua_scene::Scene;

pub mod property;
pub mod scroll;

type Overrides = Vec<(Property, MotionValue)>;

#[derive(Clone, Copy, PartialEq)]
struct Submitted {
    revision: u64,
    viewport: Option<(Size, f32)>,
}

pub struct RenderSide {
    scene: Scene,
    local: HashMap<NodeId, NodeId>,
    base: HashMap<NodeId, DisplayItem>,
    overrides: HashMap<NodeId, Overrides>,
    bindings: HashMap<TrackId, Binding>,
    tracks: TrackSet,
    dirty: Vec<NodeId>,
    viewport: Option<(Size, f32)>,
    next_frame: FrameId,
    submitted: Option<Submitted>,
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
            base: HashMap::new(),
            overrides: HashMap::new(),
            bindings: HashMap::new(),
            tracks: TrackSet::new(ReducedMotion::Full),
            dirty: Vec::new(),
            viewport: None,
            next_frame: 1,
            submitted: None,
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

    #[must_use]
    pub fn presented(&self, node: NodeId) -> Option<&DisplayItem> {
        self.local.get(&node).and_then(|id| self.scene.get(*id))
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
        let sampled = self.tracks.sample(input.present_at.as_secs_f64());
        for presented in sampled.values {
            if let Some(binding) = presented.binding {
                self.set_override(binding, presented.value);
            }
        }
        events.extend(sampled.events.into_iter().map(RenderEvent::Motion));
        self.refresh();
        let frame = self.next_submission();
        let wake = match self.tracks.is_idle() {
            true => RenderWake::Idle,
            false => RenderWake::NextFrame,
        };
        RenderOutput {
            events,
            frame,
            wake,
        }
    }

    fn feedback(&mut self, feedback: Feedback) -> RenderEvent {
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
            match self.base.contains_key(&node) {
                true => {
                    self.base.insert(node, item);
                    self.dirty.push(node);
                }
                false => self.write(node, item),
            }
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
                self.base.remove(&node);
                self.overrides.remove(&node);
            }
            let ids: Vec<NodeId> = order
                .iter()
                .filter_map(|node| self.local.get(node).copied())
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
            }
        }
    }

    fn apply_motion(&mut self, command: MotionCommand) -> Vec<MotionEvent> {
        match command {
            MotionCommand::Spawn {
                track,
                binding: Some(binding),
                ..
            } => self.bind(track, binding),
            MotionCommand::Remove { track } => self.unbind(track),
            _ => {}
        }
        self.tracks.apply(command)
    }

    fn bind(&mut self, track: TrackId, binding: Binding) {
        self.bindings.insert(track, binding);
        if !self.base.contains_key(&binding.node) {
            if let Some(item) = self.presented(binding.node).cloned() {
                self.base.insert(binding.node, item);
            }
        }
    }

    fn unbind(&mut self, track: TrackId) {
        let Some(binding) = self.bindings.remove(&track) else {
            return;
        };
        let still_bound = self
            .bindings
            .values()
            .any(|other| other.node == binding.node);
        match still_bound {
            true => {
                if let Some(overrides) = self.overrides.get_mut(&binding.node) {
                    overrides.retain(|(property, _)| *property != binding.property);
                }
                self.dirty.push(binding.node);
            }
            false => {
                self.overrides.remove(&binding.node);
                if let Some(item) = self.base.remove(&binding.node) {
                    self.write(binding.node, item);
                }
            }
        }
    }

    fn set_override(&mut self, binding: Binding, value: MotionValue) {
        let overrides = self.overrides.entry(binding.node).or_default();
        match overrides
            .iter_mut()
            .find(|(property, _)| *property == binding.property)
        {
            Some(entry) => entry.1 = value,
            None => overrides.push((binding.property, value)),
        }
        self.dirty.push(binding.node);
    }

    fn refresh(&mut self) {
        let mut seen = HashSet::new();
        let dirty = std::mem::take(&mut self.dirty);
        for node in dirty {
            if !seen.insert(node) {
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
    }
}
