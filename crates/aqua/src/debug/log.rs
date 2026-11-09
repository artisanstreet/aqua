use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use aqua_protocol::probe::{Metric, Phase};
use serde_json::{Map, Value, json};

use super::history::{Sample, Thread};

const FLUSH_EVERY: Duration = Duration::from_millis(500);

pub struct Log {
    out: Mutex<(BufWriter<File>, Instant)>,
}

fn micros(nanos: u64) -> f64 {
    (nanos as f64 / 1_000.0 * 10.0).round() / 10.0
}

#[must_use]
pub fn frame_line(sample: &Sample) -> String {
    let phases: Map<String, Value> = Phase::ALL
        .iter()
        .filter(|phase| sample.phases[phase.index()] > 0)
        .map(|phase| (phase.name().to_owned(), json!(micros(sample.phases[phase.index()]))))
        .collect();
    let metrics: Map<String, Value> = Metric::ALL
        .iter()
        .filter(|metric| sample.metrics[metric.index()] > 0)
        .map(|metric| (metric.name().to_owned(), json!(sample.metrics[metric.index()])))
        .collect();
    json!({
        "kind": "frame",
        "thread": match sample.thread {
            Thread::App => "app",
            Thread::Render => "render",
        },
        "window": sample.window,
        "at_us": micros(sample.at),
        "wall_us": micros(sample.wall),
        "work_us": micros(sample.work()),
        "phases_us": phases,
        "metrics": metrics,
        "heap": { "allocs": sample.allocs, "bytes": sample.bytes },
        "notes": sample.notes,
    })
    .to_string()
}

#[must_use]
pub fn present_line(window: u64, at: u64, interval: Option<u64>) -> String {
    json!({
        "kind": "present",
        "window": window,
        "at_us": micros(at),
        "interval_us": interval.map(micros),
    })
    .to_string()
}

impl Log {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = File::options().create(true).append(true).open(path)?;
        Ok(Self {
            out: Mutex::new((BufWriter::new(file), Instant::now())),
        })
    }

    pub fn line(&self, text: &str) {
        let mut guard = self.out.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (out, flushed) = &mut *guard;
        let _ = writeln!(out, "{text}");
        if flushed.elapsed() >= FLUSH_EVERY {
            let _ = out.flush();
            *flushed = Instant::now();
        }
    }
}
