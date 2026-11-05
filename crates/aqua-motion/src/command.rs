use aqua_core::NodeId;
use aqua_core::id::RawId;

use crate::curve::{MotionKind, MotionValue, ReducedMotion, Seconds};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TrackId(RawId);

impl TrackId {
    #[must_use]
    pub const fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> RawId {
        self.0
    }

    #[must_use]
    pub const fn key(self) -> (u32, u32) {
        (self.0.index, self.0.generation)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Edge {
    Left,
    Top,
    Right,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Property {
    Opacity,
    Translate,
    Fill,
    BorderColor,
    TextColor,
    Edge(Edge),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Binding {
    pub node: NodeId,
    pub property: Property,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MotionCommand {
    Spawn {
        track: TrackId,
        binding: Option<Binding>,
        initial: MotionValue,
        kind: MotionKind,
        at: Seconds,
    },
    SetTarget {
        track: TrackId,
        generation: u32,
        to: MotionValue,
        kind: MotionKind,
        at: Seconds,
    },
    Shift {
        track: TrackId,
        generation: u32,
        delta: MotionValue,
        to: MotionValue,
        kind: MotionKind,
        at: Seconds,
    },
    SetReduced {
        reduced: ReducedMotion,
        at: Seconds,
    },
    Cancel {
        track: TrackId,
    },
    Finish {
        track: TrackId,
    },
    Remove {
        track: TrackId,
    },
    Bind {
        track: TrackId,
        binding: Binding,
        shift: f32,
    },
    Unbind {
        track: TrackId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionEvent {
    Settled { track: TrackId, generation: u32 },
}
