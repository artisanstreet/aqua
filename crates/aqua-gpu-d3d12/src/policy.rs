use std::time::Duration;

pub const SHRINK_AFTER_FRAMES: u32 = 240;
pub const GROWTH_SLACK_PERCENT: u64 = 25;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeapDecision {
    Keep,
    Resize { bytes: u64 },
    Reject { needed: u64, ceiling: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeapGate {
    ceiling: Option<u64>,
    alignment: u64,
    idle_frames: u32,
}

fn align(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}

impl HeapGate {
    pub fn new(ceiling: Option<u64>, alignment: u64) -> Self {
        Self {
            ceiling,
            alignment: alignment.max(1),
            idle_frames: 0,
        }
    }

    pub fn ceiling(&self) -> Option<u64> {
        self.ceiling
    }

    pub fn reset(&mut self) {
        self.idle_frames = 0;
    }

    pub fn decide(&mut self, current: u64, needed: u64) -> HeapDecision {
        let exact = align(needed, self.alignment);
        if let Some(ceiling) = self.ceiling.filter(|ceiling| exact > *ceiling) {
            return HeapDecision::Reject { needed, ceiling };
        }
        if exact > current {
            self.idle_frames = 0;
            let slack = align(needed + needed * GROWTH_SLACK_PERCENT / 100, self.alignment);
            let target = match self.ceiling {
                Some(ceiling) => slack.min(ceiling).max(exact),
                None => slack,
            };
            return HeapDecision::Resize { bytes: target };
        }
        if exact * 2 > current {
            self.idle_frames = 0;
            return HeapDecision::Keep;
        }
        self.idle_frames += 1;
        match self.idle_frames >= SHRINK_AFTER_FRAMES {
            true => {
                self.idle_frames = 0;
                HeapDecision::Resize { bytes: exact }
            }
            false => HeapDecision::Keep,
        }
    }
}

pub fn ticks_to_duration(start: u64, end: u64, frequency: u64) -> Option<Duration> {
    match (frequency, end.checked_sub(start)) {
        (0, _) | (_, None) => None,
        (frequency, Some(ticks)) => {
            let nanos = ticks as u128 * 1_000_000_000 / frequency as u128;
            Some(Duration::from_nanos(nanos.min(u64::MAX as u128) as u64))
        }
    }
}
