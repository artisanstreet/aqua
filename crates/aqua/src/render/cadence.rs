use std::collections::VecDeque;
use std::time::{Duration, Instant};

const WINDOW: usize = 9;
const MINIMUM_SAMPLES: usize = 7;
const FLOOR: Duration = Duration::from_millis(2);
const CEILING: Duration = Duration::from_millis(20);
const HALF_BELOW: Duration = Duration::from_micros(7_500);

#[derive(Debug, Default)]
pub struct Cadence {
    last: Option<Instant>,
    gaps: VecDeque<Duration>,
    halved: bool,
}

impl Cadence {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&mut self, presented: Instant, halved: bool) {
        if let Some(last) = self.last.replace(presented) {
            let gap = presented.saturating_duration_since(last);
            if !self.halved && (FLOOR..=CEILING).contains(&gap) {
                if self.gaps.len() == WINDOW {
                    self.gaps.pop_front();
                }
                self.gaps.push_back(gap);
            }
        }
        self.halved = halved;
    }

    #[must_use]
    pub fn period(&self) -> Option<Duration> {
        if self.gaps.len() < MINIMUM_SAMPLES {
            return None;
        }
        let mut sorted: Vec<Duration> = self.gaps.iter().copied().collect();
        sorted.sort_unstable();
        sorted.get(sorted.len() / 2).copied()
    }

    #[must_use]
    pub fn half_rate(&self, gliding: bool) -> bool {
        gliding && self.period().is_some_and(|period| period < HALF_BELOW)
    }
}
