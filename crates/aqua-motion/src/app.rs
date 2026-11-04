use aqua_core::{Arena, NodeId};

use crate::command::{Binding, MotionCommand, MotionEvent, TrackId};
use crate::curve::{Completion, MotionError, MotionKind, MotionValue, ReducedMotion, Seconds};

struct Pending {
    bound: Option<u32>,
    callback: Box<dyn FnMut(Completion)>,
}

struct Slot {
    owner: Option<NodeId>,
    kind: MotionKind,
    generation: u32,
    target: MotionValue,
    settled: bool,
    pending: Option<Pending>,
}

pub struct AppMotion {
    arena: Arena<Slot>,
    reduced: ReducedMotion,
    outbox: Vec<MotionCommand>,
}

impl Default for AppMotion {
    fn default() -> Self {
        Self::new(ReducedMotion::Full)
    }
}

impl AppMotion {
    #[must_use]
    pub fn new(reduced: ReducedMotion) -> Self {
        Self {
            arena: Arena::new(),
            reduced,
            outbox: Vec::new(),
        }
    }

    #[must_use]
    pub fn reduced_motion(&self) -> ReducedMotion {
        self.reduced
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.arena.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.arena.is_empty()
    }

    #[must_use]
    pub fn is_settled(&self, track: TrackId) -> bool {
        self.arena.get(track.raw()).is_none_or(|slot| slot.settled)
    }

    #[must_use]
    pub fn all_settled(&self) -> bool {
        self.arena.iter().all(|(_, slot)| slot.settled)
    }

    #[must_use]
    pub fn target(&self, track: TrackId) -> Option<MotionValue> {
        self.arena.get(track.raw()).map(|slot| slot.target)
    }

    #[must_use]
    pub fn owner_of(&self, track: TrackId) -> Option<Option<NodeId>> {
        self.arena.get(track.raw()).map(|slot| slot.owner)
    }

    pub fn spawn(
        &mut self,
        owner: Option<NodeId>,
        binding: Option<Binding>,
        initial: MotionValue,
        kind: MotionKind,
        at: Seconds,
    ) -> TrackId {
        let track = TrackId::from_raw(self.arena.insert(Slot {
            owner,
            kind,
            generation: 0,
            target: initial,
            settled: true,
            pending: None,
        }));
        self.outbox.push(MotionCommand::Spawn {
            track,
            binding,
            initial,
            kind,
            at,
        });
        track
    }

    pub fn set_target(
        &mut self,
        track: TrackId,
        to: MotionValue,
        at: Seconds,
    ) -> Result<(), MotionError> {
        let kind = self
            .arena
            .get(track.raw())
            .ok_or(MotionError::UnknownHandle)?
            .kind;
        self.set_target_with(track, to, kind, at)
    }

    pub fn set_target_with(
        &mut self,
        track: TrackId,
        to: MotionValue,
        kind: MotionKind,
        at: Seconds,
    ) -> Result<(), MotionError> {
        let slot = self
            .arena
            .get_mut(track.raw())
            .ok_or(MotionError::UnknownHandle)?;
        if std::mem::discriminant(&slot.target) != std::mem::discriminant(&to) {
            return Err(MotionError::VariantMismatch);
        }
        slot.generation = slot.generation.wrapping_add(1);
        slot.kind = kind;
        slot.target = to;
        slot.settled = false;
        slot.pending = slot
            .pending
            .take()
            .and_then(|mut pending| match pending.bound {
                None => {
                    pending.bound = Some(slot.generation);
                    Some(pending)
                }
                Some(_) => None,
            });
        self.outbox.push(MotionCommand::SetTarget {
            track,
            generation: slot.generation,
            to,
            kind,
            at,
        });
        Ok(())
    }

    pub fn on_complete(&mut self, track: TrackId, callback: Box<dyn FnMut(Completion)>) -> bool {
        match self.arena.get_mut(track.raw()) {
            Some(slot) => {
                slot.pending = Some(Pending {
                    bound: None,
                    callback,
                });
                true
            }
            None => false,
        }
    }

    pub fn cancel(&mut self, track: TrackId) -> bool {
        let Some(slot) = self.arena.get_mut(track.raw()) else {
            return false;
        };
        if !slot.settled {
            slot.settled = true;
            if let Some(mut pending) = slot.pending.take() {
                (pending.callback)(Completion::Cancelled);
            }
            self.outbox.push(MotionCommand::Cancel { track });
        }
        true
    }

    pub fn finish(&mut self, track: TrackId) -> bool {
        let Some(slot) = self.arena.get_mut(track.raw()) else {
            return false;
        };
        if !slot.settled {
            slot.settled = true;
            if let Some(mut pending) = slot.pending.take() {
                (pending.callback)(Completion::Completed);
            }
            self.outbox.push(MotionCommand::Finish { track });
        }
        true
    }

    pub fn remove(&mut self, track: TrackId) -> bool {
        let removed = self.arena.remove(track.raw()).is_ok();
        if removed {
            self.outbox.push(MotionCommand::Remove { track });
        }
        removed
    }

    pub fn dispose_owner(&mut self, owner: NodeId) -> Vec<TrackId> {
        let owned: Vec<TrackId> = self
            .arena
            .iter()
            .filter(|(_, slot)| slot.owner == Some(owner))
            .map(|(raw, _)| TrackId::from_raw(raw))
            .collect();
        owned
            .into_iter()
            .filter(|track| self.remove(*track))
            .collect()
    }

    pub fn set_reduced_motion(&mut self, reduced: ReducedMotion, at: Seconds) {
        self.reduced = reduced;
        self.outbox.push(MotionCommand::SetReduced { reduced, at });
    }

    pub fn handle(&mut self, event: MotionEvent) {
        let MotionEvent::Settled { track, generation } = event;
        let Some(slot) = self.arena.get_mut(track.raw()) else {
            return;
        };
        if slot.generation != generation {
            return;
        }
        slot.settled = true;
        if slot
            .pending
            .as_ref()
            .is_some_and(|p| p.bound == Some(generation))
        {
            if let Some(mut pending) = slot.pending.take() {
                (pending.callback)(Completion::Completed);
            }
        }
    }

    pub fn drain_commands(&mut self) -> Vec<MotionCommand> {
        std::mem::take(&mut self.outbox)
    }
}
