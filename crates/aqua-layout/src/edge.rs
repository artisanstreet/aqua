use std::collections::HashMap;

use aqua_core::{NodeId, Rect};
use aqua_motion::{AppMotion, Binding, Edge, MotionKind, MotionValue, Property, Seconds, TrackId};

pub const EDGES: [Edge; 4] = [Edge::Left, Edge::Top, Edge::Right, Edge::Bottom];

const EDGE_EPSILON: f32 = 1e-3;

const fn slot(edge: Edge) -> usize {
    match edge {
        Edge::Left => 0,
        Edge::Top => 1,
        Edge::Right => 2,
        Edge::Bottom => 3,
    }
}

#[must_use]
pub fn edge_value(rect: Rect, edge: Edge) -> f32 {
    match edge {
        Edge::Left => rect.x,
        Edge::Top => rect.y,
        Edge::Right => rect.x + rect.w,
        Edge::Bottom => rect.y + rect.h,
    }
}

#[must_use]
pub fn moved_edges(previous: Rect, next: Rect) -> Vec<Edge> {
    EDGES
        .into_iter()
        .filter(|edge| (edge_value(previous, *edge) - edge_value(next, *edge)).abs() > EDGE_EPSILON)
        .collect()
}

#[must_use]
pub fn presented_rect(target: Rect, edges: impl IntoIterator<Item = (Edge, f32)>) -> Rect {
    let mut sides = EDGES.map(|edge| edge_value(target, edge));
    for (edge, value) in edges {
        sides[slot(edge)] = value;
    }
    Rect {
        x: sides[0],
        y: sides[1],
        w: (sides[2] - sides[0]).max(0.0),
        h: (sides[3] - sides[1]).max(0.0),
    }
}

struct Entry {
    target: Rect,
    tracks: [Option<TrackId>; 4],
}

#[derive(Default)]
pub struct EdgeAnimator {
    entries: HashMap<NodeId, Entry>,
}

impl EdgeAnimator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn target(&self, node: NodeId) -> Option<Rect> {
        self.entries.get(&node).map(|entry| entry.target)
    }

    #[must_use]
    pub fn track(&self, node: NodeId, edge: Edge) -> Option<TrackId> {
        self.entries
            .get(&node)
            .and_then(|entry| entry.tracks[slot(edge)])
    }

    #[must_use]
    pub fn tracks(&self, node: NodeId) -> Vec<TrackId> {
        self.entries
            .get(&node)
            .map(|entry| entry.tracks.iter().flatten().copied().collect())
            .unwrap_or_default()
    }

    pub fn observe(
        &mut self,
        motion: &mut AppMotion,
        node: NodeId,
        next: Rect,
        kind: MotionKind,
        at: Seconds,
    ) -> Vec<Edge> {
        let Some(entry) = self.entries.get_mut(&node) else {
            self.entries.insert(
                node,
                Entry {
                    target: next,
                    tracks: [None; 4],
                },
            );
            return Vec::new();
        };
        let moved = moved_edges(entry.target, next);
        for edge in &moved {
            let from = edge_value(entry.target, *edge);
            let to = MotionValue::Scalar(edge_value(next, *edge));
            let track = *entry.tracks[slot(*edge)].get_or_insert_with(|| {
                motion.spawn(
                    Some(node),
                    Some(Binding {
                        node,
                        property: Property::Edge(*edge),
                    }),
                    MotionValue::Scalar(from),
                    kind,
                    at,
                )
            });
            let _ = motion.set_target_with(track, to, kind, at);
        }
        entry.target = next;
        moved
    }

    pub fn retime(&mut self, motion: &mut AppMotion, node: NodeId, kind: MotionKind, at: Seconds) {
        let Some(entry) = self.entries.get(&node) else {
            return;
        };
        for edge in EDGES {
            if let Some(track) = entry.tracks[slot(edge)] {
                if !motion.is_settled(track) {
                    let to = MotionValue::Scalar(edge_value(entry.target, edge));
                    let _ = motion.set_target_with(track, to, kind, at);
                }
            }
        }
    }

    pub fn forget(&mut self, motion: &mut AppMotion, node: NodeId) -> Vec<TrackId> {
        self.entries.remove(&node);
        motion.dispose_owner(node)
    }
}
