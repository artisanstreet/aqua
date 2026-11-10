use std::collections::VecDeque;

use aqua_protocol::probe::{Metric, Phase};

pub const CAPACITY: usize = 240;
const PHASES: usize = Phase::ALL.len();
const METRICS: usize = Metric::ALL.len();
const SECOND: u64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Thread {
    App,
    Render,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    pub thread: Thread,
    pub window: u64,
    pub at: u64,
    pub wall: u64,
    pub phases: [u64; PHASES],
    pub metrics: [u64; METRICS],
    pub allocs: u64,
    pub bytes: u64,
    pub notes: Vec<String>,
}

impl Sample {
    #[must_use]
    pub fn work(&self) -> u64 {
        Phase::ALL
            .iter()
            .filter(|phase| **phase != Phase::Wait)
            .map(|phase| self.phases[phase.index()])
            .sum()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Spread {
    pub p50: u64,
    pub p95: u64,
    pub max: u64,
}

impl Spread {
    #[must_use]
    pub fn of(mut values: Vec<u64>) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        values.sort_unstable();
        let at = |fraction: f64| {
            let index = ((values.len() - 1) as f64 * fraction).round() as usize;
            values[index]
        };
        Self {
            p50: at(0.5),
            p95: at(0.95),
            max: values[values.len() - 1],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub presents_per_second: usize,
    pub app_updates_per_second: usize,
    pub interval: Spread,
    pub refresh: Option<u64>,
    pub missed: usize,
    pub app: Spread,
    pub render: Spread,
}

#[derive(Clone, Debug, Default)]
pub struct History {
    app: VecDeque<Sample>,
    render: VecDeque<Sample>,
    presents: VecDeque<u64>,
    recent_presents: VecDeque<u64>,
    recent_app_updates: VecDeque<u64>,
}

fn push<T>(ring: &mut VecDeque<T>, value: T) {
    if ring.len() == CAPACITY {
        ring.pop_front();
    }
    ring.push_back(value);
}

impl History {
    pub fn record(&mut self, sample: Sample) {
        match sample.thread {
            Thread::App => {
                self.recent_app_updates.push_back(sample.at);
                while self
                    .recent_app_updates
                    .front()
                    .is_some_and(|first| sample.at.saturating_sub(*first) > SECOND)
                {
                    self.recent_app_updates.pop_front();
                }
                push(&mut self.app, sample);
            }
            Thread::Render => push(&mut self.render, sample),
        }
    }

    pub fn present(&mut self, at: u64) {
        push(&mut self.presents, at);
        self.recent_presents.push_back(at);
        while self
            .recent_presents
            .front()
            .is_some_and(|first| at.saturating_sub(*first) > SECOND)
        {
            self.recent_presents.pop_front();
        }
    }

    #[must_use]
    pub fn last_present(&self) -> Option<u64> {
        self.presents.back().copied()
    }

    #[must_use]
    pub fn samples(&self, thread: Thread) -> &VecDeque<Sample> {
        match thread {
            Thread::App => &self.app,
            Thread::Render => &self.render,
        }
    }

    #[must_use]
    pub fn intervals(&self) -> Vec<u64> {
        self.presents
            .iter()
            .zip(self.presents.iter().skip(1))
            .map(|(earlier, later)| later.saturating_sub(*earlier))
            .collect()
    }

    #[must_use]
    pub fn summary(&self, now: u64, refresh: Option<u64>) -> Summary {
        let intervals = self.intervals();
        let missed = refresh.map_or(0, |refresh| {
            intervals
                .iter()
                .filter(|interval| **interval > refresh + refresh / 2 && **interval < SECOND / 2)
                .count()
        });
        Summary {
            app_updates_per_second: self
                .recent_app_updates
                .iter()
                .filter(|at| now.saturating_sub(**at) <= SECOND)
                .count(),
            presents_per_second: self
                .recent_presents
                .iter()
                .filter(|at| now.saturating_sub(**at) <= SECOND)
                .count(),
            interval: Spread::of(intervals),
            refresh,
            missed,
            app: Spread::of(self.app.iter().map(Sample::work).collect()),
            render: Spread::of(self.render.iter().map(Sample::work).collect()),
        }
    }
}
