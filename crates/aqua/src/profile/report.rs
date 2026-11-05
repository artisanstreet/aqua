use std::fmt::Write as _;
use std::fs::File;
use std::io::Write as _;
use std::sync::Mutex;
use std::time::Instant;

use aqua_protocol::probe::{Metric, Phase};

use super::recorder::{Finished, Role};

const SPIKE_DEFAULT_MS: f64 = 4.0;
const SUMMARY_SECONDS: f64 = 1.0;

pub struct Config {
    pub spike_nanos: u64,
    pub origin: Instant,
}

impl Config {
    pub fn from_environment() -> Self {
        let millis = std::env::var("AQUA_PROFILE_SPIKE_MS")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(SPIKE_DEFAULT_MS);
        Self {
            spike_nanos: (millis * 1_000_000.0) as u64,
            origin: Instant::now(),
        }
    }
}

pub struct Output {
    file: Mutex<Option<File>>,
}

impl Output {
    pub fn from_environment() -> Self {
        let file = std::env::var_os("AQUA_PROFILE_OUT")
            .and_then(|path| File::options().create(true).append(true).open(path).ok());
        Self {
            file: Mutex::new(file),
        }
    }

    pub fn line(&self, text: &str) {
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        match guard.as_mut() {
            Some(file) => {
                let _ = writeln!(file, "{text}");
            }
            None => eprintln!("{text}"),
        }
    }
}

fn millis(nanos: u64) -> f64 {
    nanos as f64 / 1_000_000.0
}

pub fn spike_line(config: &Config, frame: &Finished, sequence: u64) -> String {
    let elapsed = config.origin.elapsed().as_secs_f64() * 1000.0;
    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let heap = frame.total_heap();
    let mut line = String::with_capacity(512);
    let _ = write!(
        line,
        "aqua-spike role={} label={} seq={sequence} t_ms={elapsed:.1} unix_ms={unix_ms} work_ms={:.2} wall_ms={:.2} allocs={} alloc_bytes={} reallocs={} frees={}",
        frame.role.name(),
        frame.label,
        millis(frame.work_nanos()),
        millis(frame.wall_nanos),
        heap.allocs,
        heap.bytes,
        heap.reallocs,
        heap.frees,
    );
    line.push_str(" phases_ms={");
    let mut first = true;
    for phase in Phase::ALL {
        let nanos = frame.nanos[phase.index()];
        if nanos == 0 {
            continue;
        }
        if !first {
            line.push(',');
        }
        first = false;
        let h = frame.heap[phase.index()];
        let _ = write!(
            line,
            "{}:{:.2}(a{}/{}B)",
            phase.name(),
            millis(nanos),
            h.allocs,
            h.bytes
        );
    }
    line.push('}');
    line.push_str(" counts={");
    let mut first = true;
    for metric in Metric::ALL {
        let value = frame.metrics[metric.index()];
        if value == 0 {
            continue;
        }
        if !first {
            line.push(',');
        }
        first = false;
        let _ = write!(line, "{}:{value}", metric.name());
    }
    line.push('}');
    let attributed: u64 = frame.nanos.iter().sum();
    let other = frame.wall_nanos.saturating_sub(attributed);
    if other > 500_000 {
        let _ = write!(line, " unattributed_ms={:.2}", millis(other));
    }
    let dominant = Phase::ALL
        .into_iter()
        .filter(|phase| *phase != Phase::Wait)
        .max_by_key(|phase| frame.nanos[phase.index()])
        .map_or("none", Phase::name);
    let _ = write!(
        line,
        " dominant={dominant} notes=[{}]",
        frame.notes.join(";")
    );
    line
}

pub struct Summary {
    role: Role,
    started: Instant,
    samples: Vec<u64>,
    phase_max: [u64; Phase::ALL.len()],
    phase_sum: [u64; Phase::ALL.len()],
    over_4: u32,
    over_8: u32,
    over_16: u32,
    allocs: u64,
}

impl Summary {
    pub fn new(role: Role) -> Self {
        Self {
            role,
            started: Instant::now(),
            samples: Vec::with_capacity(512),
            phase_max: [0; Phase::ALL.len()],
            phase_sum: [0; Phase::ALL.len()],
            over_4: 0,
            over_8: 0,
            over_16: 0,
            allocs: 0,
        }
    }

    pub fn record(&mut self, frame: &Finished) -> Option<String> {
        let work = frame.work_nanos();
        self.samples.push(work);
        for phase in Phase::ALL {
            let index = phase.index();
            self.phase_max[index] = self.phase_max[index].max(frame.nanos[index]);
            self.phase_sum[index] += frame.nanos[index];
        }
        self.over_4 += u32::from(work > 4_000_000);
        self.over_8 += u32::from(work > 8_333_000);
        self.over_16 += u32::from(work > 16_667_000);
        self.allocs += frame.total_heap().allocs;
        (self.started.elapsed().as_secs_f64() >= SUMMARY_SECONDS).then(|| self.flush())
    }

    fn flush(&mut self) -> String {
        self.samples.sort_unstable();
        let count = self.samples.len();
        let at = |fraction: f64| {
            self.samples
                .get(((count as f64 * fraction) as usize).min(count.saturating_sub(1)))
                .copied()
                .unwrap_or(0)
        };
        let mut line = String::with_capacity(256);
        let _ = write!(
            line,
            "aqua-profile role={} frames={count} work_p50_ms={:.3} p90_ms={:.3} p99_ms={:.3} max_ms={:.3} over4={} over8.3={} over16.7={} allocs_per_frame={:.1} phase_max_ms={{",
            self.role.name(),
            millis(at(0.5)),
            millis(at(0.9)),
            millis(at(0.99)),
            millis(self.samples.last().copied().unwrap_or(0)),
            self.over_4,
            self.over_8,
            self.over_16,
            self.allocs as f64 / count.max(1) as f64,
        );
        let mut first = true;
        for phase in Phase::ALL {
            let nanos = self.phase_max[phase.index()];
            if nanos == 0 {
                continue;
            }
            if !first {
                line.push(',');
            }
            first = false;
            let _ = write!(line, "{}:{:.2}", phase.name(), millis(nanos));
        }
        line.push_str("} phase_avg_ms={");
        let mut first = true;
        for phase in Phase::ALL {
            let nanos = self.phase_sum[phase.index()] / count.max(1) as u64;
            if self.phase_max[phase.index()] == 0 {
                continue;
            }
            if !first {
                line.push(',');
            }
            first = false;
            let _ = write!(line, "{}:{:.3}", phase.name(), millis(nanos));
        }
        line.push('}');
        self.phase_sum = [0; Phase::ALL.len()];
        self.samples.clear();
        self.phase_max = [0; Phase::ALL.len()];
        self.over_4 = 0;
        self.over_8 = 0;
        self.over_16 = 0;
        self.allocs = 0;
        self.started = Instant::now();
        line
    }
}
