use std::time::Instant;

use aqua_protocol::probe::{Event, Metric, Phase};

use super::alloc::{self, HeapCounts};

const PHASES: usize = Phase::ALL.len();
const METRICS: usize = Metric::ALL.len();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    App,
    Render,
}

impl Role {
    pub const fn name(self) -> &'static str {
        match self {
            Role::App => "app",
            Role::Render => "render",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Finished {
    pub role: Role,
    pub label: u64,
    pub nanos: [u64; PHASES],
    pub heap: [HeapCounts; PHASES],
    pub metrics: [u64; METRICS],
    pub notes: Vec<String>,
    pub wall_nanos: u64,
}

impl Finished {
    pub fn work_nanos(&self) -> u64 {
        self.nanos
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != Phase::Wait.index())
            .map(|(_, nanos)| *nanos)
            .sum()
    }

    pub fn total_heap(&self) -> HeapCounts {
        self.heap
            .iter()
            .fold(HeapCounts::default(), |sum, heap| HeapCounts {
                allocs: sum.allocs + heap.allocs,
                bytes: sum.bytes + heap.bytes,
                frees: sum.frees + heap.frees,
                reallocs: sum.reallocs + heap.reallocs,
            })
    }
}

pub struct Recorder {
    current: Option<Phase>,
    stack: Vec<Phase>,
    since: Instant,
    mark: HeapCounts,
    nanos: [u64; PHASES],
    heap: [HeapCounts; PHASES],
    metrics: [u64; METRICS],
    notes: Vec<String>,
    begun: Option<Instant>,
}

fn add(into: &mut HeapCounts, delta: HeapCounts) {
    into.allocs += delta.allocs;
    into.bytes += delta.bytes;
    into.frees += delta.frees;
    into.reallocs += delta.reallocs;
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            current: None,
            stack: Vec::with_capacity(8),
            since: Instant::now(),
            mark: alloc::snapshot(),
            nanos: [0; PHASES],
            heap: [HeapCounts::default(); PHASES],
            metrics: [0; METRICS],
            notes: Vec::new(),
            begun: None,
        }
    }

    fn charge(&mut self, now: Instant, heap: HeapCounts) {
        if let Some(phase) = self.current {
            let index = phase.index();
            let elapsed = now.saturating_duration_since(self.since);
            self.nanos[index] += u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
            add(&mut self.heap[index], heap.since(self.mark));
        }
        self.since = now;
        self.mark = heap;
    }

    pub fn event(&mut self, event: Event) {
        match event {
            Event::Enter(phase) => {
                self.charge(Instant::now(), alloc::snapshot());
                if let Some(parent) = self.current {
                    self.stack.push(parent);
                }
                self.current = Some(phase);
            }
            Event::Exit(_) => {
                self.charge(Instant::now(), alloc::snapshot());
                self.current = self.stack.pop();
            }
            Event::Count(metric, value) => {
                self.metrics[metric.index()] += value;
            }
        }
    }

    pub fn note(&mut self, text: String) {
        self.notes.push(text);
    }

    pub fn begin(&mut self) {
        self.begun = Some(Instant::now());
    }

    pub fn finish(&mut self, role: Role, label: u64) -> Finished {
        let now = Instant::now();
        self.charge(now, alloc::snapshot());
        let wall = self.begun.take().map_or(0, |begun| {
            u64::try_from(now.duration_since(begun).as_nanos()).unwrap_or(0)
        });
        let finished = Finished {
            role,
            label,
            nanos: self.nanos,
            heap: self.heap,
            metrics: self.metrics,
            notes: std::mem::take(&mut self.notes),
            wall_nanos: wall,
        };
        self.nanos = [0; PHASES];
        self.heap = [HeapCounts::default(); PHASES];
        self.metrics = [0; METRICS];
        self.stack.clear();
        finished
    }
}
