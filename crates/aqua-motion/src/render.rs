use std::collections::{BTreeMap, BTreeSet};

use crate::command::{Binding, MotionCommand, MotionEvent, TrackId};
use crate::curve::{MotionValue, ReducedMotion, Seconds};
use crate::track::Track;

struct Slot {
    id: TrackId,
    binding: Option<Binding>,
    generation: u32,
    track: Track,
    dirty: bool,
}

impl Slot {
    fn is_active(&self) -> bool {
        self.dirty || !self.track.is_settled()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Presented {
    pub track: TrackId,
    pub binding: Option<Binding>,
    pub value: MotionValue,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sampled {
    pub values: Vec<Presented>,
    pub events: Vec<MotionEvent>,
}

pub struct TrackSet {
    reduced: ReducedMotion,
    slots: BTreeMap<(u32, u32), Slot>,
    active: BTreeSet<(u32, u32)>,
}

impl Default for TrackSet {
    fn default() -> Self {
        Self::new(ReducedMotion::Full)
    }
}

impl TrackSet {
    #[must_use]
    pub fn new(reduced: ReducedMotion) -> Self {
        Self {
            reduced,
            slots: BTreeMap::new(),
            active: BTreeSet::new(),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.active.is_empty()
    }

    #[must_use]
    pub fn value(&self, track: TrackId) -> Option<MotionValue> {
        self.slots.get(&track.key()).map(|slot| slot.track.value())
    }

    #[must_use]
    pub fn settle_deadline(&self) -> Option<Seconds> {
        self.active
            .iter()
            .filter_map(|key| self.slots.get(key)?.track.settle_deadline())
            .reduce(Seconds::min)
    }

    pub fn apply(&mut self, command: MotionCommand) -> Vec<MotionEvent> {
        match command {
            MotionCommand::Spawn {
                track,
                binding,
                initial,
                kind,
                at,
            } => {
                let slot = Slot {
                    id: track,
                    binding,
                    generation: 0,
                    track: Track::new(initial, kind, self.reduced, at),
                    dirty: true,
                };
                self.slots.insert(track.key(), slot);
                self.active.insert(track.key());
                Vec::new()
            }
            MotionCommand::SetTarget {
                track,
                generation,
                to,
                kind,
                at,
            } => {
                if let Some(slot) = self.slots.get_mut(&track.key()) {
                    if slot.track.retarget(to, kind, at).is_ok() {
                        slot.generation = generation;
                        slot.dirty = true;
                        self.active.insert(track.key());
                    }
                }
                Vec::new()
            }
            MotionCommand::Shift {
                track,
                generation,
                delta,
                to,
                kind,
                at,
            } => {
                if let Some(slot) = self.slots.get_mut(&track.key()) {
                    if slot.track.shift(delta, to, kind, at).is_ok() {
                        slot.generation = generation;
                        slot.dirty = true;
                        self.active.insert(track.key());
                    }
                }
                Vec::new()
            }
            MotionCommand::SetReduced { reduced, at } => {
                self.reduced = reduced;
                let mut events = Vec::new();
                for slot in self.slots.values_mut() {
                    let was_settled = slot.track.is_settled();
                    let settled_now = slot.track.set_reduced(reduced, at);
                    slot.dirty = true;
                    self.active.insert(slot.id.key());
                    if settled_now && !was_settled {
                        events.push(MotionEvent::Settled {
                            track: slot.id,
                            generation: slot.generation,
                        });
                    }
                }
                events
            }
            MotionCommand::Cancel { track } => {
                if let Some(slot) = self.slots.get_mut(&track.key()) {
                    slot.track.halt();
                }
                Vec::new()
            }
            MotionCommand::Finish { track } => {
                if let Some(slot) = self.slots.get_mut(&track.key()) {
                    slot.track.finish();
                    slot.dirty = true;
                    self.active.insert(track.key());
                }
                Vec::new()
            }
            MotionCommand::Remove { track } => {
                self.slots.remove(&track.key());
                self.active.remove(&track.key());
                Vec::new()
            }
            MotionCommand::Bind { .. } | MotionCommand::Unbind { .. } => Vec::new(),
        }
    }

    pub fn sample(&mut self, at: Seconds) -> Sampled {
        let keys: Vec<(u32, u32)> = self.active.iter().copied().collect();
        let mut sampled = Sampled::default();
        for key in keys {
            let Some(slot) = self.slots.get_mut(&key) else {
                self.active.remove(&key);
                continue;
            };
            let was_settled = slot.track.is_settled();
            let value = slot.track.sample(at);
            slot.dirty = false;
            sampled.values.push(Presented {
                track: slot.id,
                binding: slot.binding,
                value,
            });
            if !was_settled && slot.track.is_settled() {
                sampled.events.push(MotionEvent::Settled {
                    track: slot.id,
                    generation: slot.generation,
                });
            }
            if !slot.is_active() {
                self.active.remove(&key);
            }
        }
        sampled
    }
}
