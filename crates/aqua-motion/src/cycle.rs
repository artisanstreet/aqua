use std::collections::HashMap;
use std::hash::Hash;

use crate::curve::Seconds;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cycle {
    Pulse { period: Seconds, floor: f32 },
    Sweep { delay: Seconds, duration: Seconds },
}

impl Cycle {
    #[must_use]
    pub fn value(self, elapsed: Seconds) -> f32 {
        let elapsed = elapsed.max(0.0);
        match self {
            Cycle::Pulse { period, floor } => match period > 0.0 {
                true => {
                    let progress = (elapsed % period) / period;
                    let swing = (progress as f32 * std::f32::consts::TAU).cos() * 0.5 + 0.5;
                    let floor = floor.clamp(0.0, 1.0);
                    floor + (1.0 - floor) * swing
                }
                false => 1.0,
            },
            Cycle::Sweep { delay, duration } => match duration > 0.0 {
                true => {
                    let delay = delay.max(0.0);
                    let at = elapsed % (delay + duration);
                    match at < delay {
                        true => 0.0,
                        false => (((at - delay) / duration) as f32).min(1.0),
                    }
                }
                false => 0.0,
            },
        }
    }
}

impl Cycle {
    #[must_use]
    pub fn holds_for(self, elapsed: Seconds) -> Option<Seconds> {
        match self {
            Cycle::Pulse { .. } => None,
            Cycle::Sweep { delay, duration } => match duration > 0.0 && delay > 0.0 {
                true => {
                    let at = elapsed.max(0.0) % (delay + duration);
                    (at < delay).then_some(delay - at)
                }
                false => None,
            },
        }
    }
}

#[derive(Debug)]
pub struct CycleSet<K> {
    anchors: HashMap<K, (Cycle, Seconds)>,
    seen: Vec<K>,
}

impl<K> Default for CycleSet<K> {
    fn default() -> Self {
        Self {
            anchors: HashMap::new(),
            seen: Vec::new(),
        }
    }
}

impl<K: Copy + Eq + Hash> CycleSet<K> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin_frame(&mut self) {
        self.seen.clear();
    }

    pub fn end_frame(&mut self) {
        let seen = &self.seen;
        self.anchors.retain(|key, _| seen.contains(key));
    }

    #[must_use]
    pub fn active(&self) -> bool {
        !self.anchors.is_empty()
    }

    pub fn clear(&mut self) {
        self.anchors.clear();
        self.seen.clear();
    }

    pub fn sample(&mut self, key: K, cycle: Cycle, now: Seconds) -> f32 {
        if !self.seen.contains(&key) {
            self.seen.push(key);
        }
        let entry = self.anchors.entry(key).or_insert((cycle, now));
        if entry.0 != cycle {
            *entry = (cycle, now);
        }
        cycle.value(now - entry.1)
    }

    #[must_use]
    pub fn holds_for(&self, key: K, now: Seconds) -> Option<Seconds> {
        let (cycle, anchor) = self.anchors.get(&key)?;
        cycle.holds_for(now - anchor)
    }
}
