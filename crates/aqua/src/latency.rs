use std::collections::VecDeque;
use std::sync::atomic::Ordering;

use aqua_core::InputEvent;

use crate::render::host::{epoch_ns, RenderStats};

const SUMMARY_EVERY: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Summary,
    Detail,
}

#[derive(Clone, Copy, Debug)]
struct Stamp {
    at: u64,
    input_done: u64,
}

#[derive(Clone, Copy, Debug)]
struct Flight {
    stamp: Stamp,
    scene: u64,
    frame_start: u64,
    build_done: u64,
    pushed: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct Sample {
    total: f64,
    input: f64,
    queue: f64,
    build: f64,
    handoff: f64,
    gpu_wait: f64,
    step: f64,
    execute: f64,
}

pub struct InputLatency {
    mode: Mode,
    pending: Vec<Stamp>,
    started: Option<u64>,
    frame_start: u64,
    build_done: u64,
    flights: VecDeque<Flight>,
    samples: Vec<Sample>,
    frames: u64,
    last_queued: u64,
}

pub fn counts_as_keystroke(event: &InputEvent) -> bool {
    matches!(event, InputEvent::TextInput { .. } | InputEvent::KeyDown(_))
}

fn ms(from: u64, to: u64) -> f64 {
    to.saturating_sub(from) as f64 / 1.0e6
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    let index = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[index]
}

impl InputLatency {
    pub fn from_environment() -> Option<Self> {
        let mode = match std::env::var("AQUA_INPUT_LATENCY").ok()?.as_str() {
            "1" => Mode::Summary,
            "2" => Mode::Detail,
            _ => return None,
        };
        eprintln!("aqua: input latency probe enabled ({mode:?})");
        Some(Self {
            mode,
            pending: Vec::new(),
            started: None,
            frame_start: 0,
            build_done: 0,
            flights: VecDeque::new(),
            samples: Vec::new(),
            frames: 0,
            last_queued: 0,
        })
    }

    pub fn begin_input(&mut self) {
        self.started = Some(epoch_ns());
    }

    pub fn end_input(&mut self, event: &InputEvent) {
        let Some(at) = self.started.take() else {
            return;
        };
        if counts_as_keystroke(event) {
            self.pending.push(Stamp {
                at,
                input_done: epoch_ns(),
            });
        }
    }

    pub fn end_text(&mut self) {
        if let Some(at) = self.started.take() {
            self.pending.push(Stamp {
                at,
                input_done: epoch_ns(),
            });
        }
    }

    pub fn frame_started(&mut self) {
        self.frame_start = epoch_ns();
    }

    pub fn build_finished(&mut self) {
        self.build_done = epoch_ns();
    }

    pub fn scene_pushed(&mut self, queued: u64) {
        let pushed_at = epoch_ns();
        let stamps = std::mem::take(&mut self.pending);
        let changed = queued > self.last_queued;
        self.last_queued = queued;
        let Some(stamp) = stamps.into_iter().min_by_key(|stamp| stamp.at) else {
            return;
        };
        if !changed {
            if self.mode == Mode::Detail {
                eprintln!("aqua-key-latency: dropped reason=no-scene-change");
            }
            return;
        }
        self.flights.push_back(Flight {
            stamp,
            scene: queued,
            frame_start: self.frame_start,
            build_done: self.build_done,
            pushed: pushed_at,
        });
    }

    pub fn presented(&mut self, stats: &RenderStats) {
        self.frames += 1;
        let presented = stats.scenes_presented.load(Ordering::Acquire);
        let picked = stats.picked_ns.load(Ordering::Relaxed);
        let waited = stats.waited_ns.load(Ordering::Relaxed);
        let stepped = stats.stepped_ns.load(Ordering::Relaxed);
        let done = stats.presented_ns.load(Ordering::Relaxed);
        while self
            .flights
            .front()
            .is_some_and(|flight| flight.scene <= presented)
        {
            let Some(flight) = self.flights.pop_front() else {
                break;
            };
            let sample = Sample {
                total: ms(flight.stamp.at, done),
                input: ms(flight.stamp.at, flight.stamp.input_done),
                queue: ms(flight.stamp.input_done, flight.frame_start),
                build: ms(flight.frame_start, flight.build_done),
                handoff: ms(flight.pushed, picked),
                gpu_wait: ms(picked, waited),
                step: ms(waited, stepped),
                execute: ms(stepped, done),
            };
            if self.mode == Mode::Detail {
                eprintln!(
                    "aqua-key-latency: total_ms={:.2} input={:.2} queue={:.2} build={:.2} handoff={:.2} gpu_wait={:.2} step={:.2} execute={:.2}",
                    sample.total, sample.input, sample.queue, sample.build, sample.handoff,
                    sample.gpu_wait, sample.step, sample.execute,
                );
            }
            self.samples.push(sample);
        }
        if self.samples.len() >= SUMMARY_EVERY {
            self.summarize();
        }
    }

    fn summarize(&mut self) {
        let mut totals: Vec<f64> = self.samples.iter().map(|s| s.total).collect();
        totals.sort_by(f64::total_cmp);
        let mean = |pick: fn(&Sample) -> f64| {
            self.samples.iter().map(pick).sum::<f64>() / self.samples.len() as f64
        };
        eprintln!(
            "aqua-key-latency-summary: frames={} n={} p50_ms={:.2} p90_ms={:.2} p99_ms={:.2} max_ms={:.2} mean_input={:.2} mean_queue={:.2} mean_build={:.2} mean_handoff={:.2} mean_gpu_wait={:.2} mean_step={:.2} mean_execute={:.2}",
            self.frames,
            totals.len(),
            percentile(&totals, 0.5),
            percentile(&totals, 0.9),
            percentile(&totals, 0.99),
            totals[totals.len() - 1],
            mean(|s| s.input),
            mean(|s| s.queue),
            mean(|s| s.build),
            mean(|s| s.handoff),
            mean(|s| s.gpu_wait),
            mean(|s| s.step),
            mean(|s| s.execute),
        );
        self.samples.clear();
        self.frames = 0;
    }
}
