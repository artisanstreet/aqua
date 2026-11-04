use std::collections::HashMap;

use aqua_core::NodeId;
use aqua_motion::{AppMotion, Binding, MotionValue, Property, Seconds, TrackId};

use crate::glide;

struct Glide {
    track: Option<TrackId>,
    pending: f32,
}

#[derive(Default)]
pub struct ScrollGlides {
    glides: HashMap<NodeId, Glide>,
}

impl ScrollGlides {
    pub fn push(&mut self, node: NodeId, applied: f32) {
        let glide = self.glides.entry(node).or_insert(Glide {
            track: None,
            pending: 0.0,
        });
        glide.pending += applied;
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.glides.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.glides.is_empty()
    }

    #[must_use]
    pub fn track(&self, node: NodeId) -> Option<TrackId> {
        self.glides.get(&node).and_then(|glide| glide.track)
    }

    pub fn flush(
        &mut self,
        tracks: &mut AppMotion,
        now: Seconds,
        resolve: impl Fn(NodeId) -> Option<NodeId>,
        alive: impl Fn(NodeId) -> bool,
    ) {
        self.glides.retain(|node, glide| match alive(*node) {
            true => true,
            false => {
                tracks.dispose_owner(*node);
                glide.track = None;
                false
            }
        });
        for (node, glide) in self.glides.iter_mut() {
            let Some(content) = resolve(*node) else {
                continue;
            };
            let track = *glide.track.get_or_insert_with(|| {
                tracks.spawn(
                    Some(*node),
                    Some(Binding {
                        node: content,
                        property: Property::Translate,
                    }),
                    MotionValue::Pair(0.0, 0.0),
                    glide::kind(),
                    now,
                )
            });
            if glide.pending != 0.0 {
                let delta = MotionValue::Pair(0.0, glide.pending);
                if tracks
                    .shift(
                        track,
                        delta,
                        MotionValue::Pair(0.0, 0.0),
                        glide::kind(),
                        now,
                    )
                    .is_ok()
                {
                    glide.pending = 0.0;
                }
            }
        }
    }
}
