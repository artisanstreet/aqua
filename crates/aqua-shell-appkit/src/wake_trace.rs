use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    AppKit,
    Waker,
    Timer,
    LiveResize,
    Slow { step: &'static str, millis: u32 },
}

impl Source {
    fn label(self) -> String {
        match self {
            Source::AppKit => "loop:appkit".into(),
            Source::Waker => "loop:waker".into(),
            Source::Timer => "loop:timer".into(),
            Source::LiveResize => "loop:live-resize".into(),
            Source::Slow { step, millis } => format!("slow:{step}:{millis}ms"),
        }
    }
}

#[derive(Debug, Default)]
pub struct Tally {
    counts: BTreeMap<String, u32>,
}

impl Tally {
    pub fn record(&mut self, source: Source) {
        *self.counts.entry(source.label()).or_default() += 1;
    }

    pub fn total(&self) -> u32 {
        self.counts.values().sum()
    }

    fn has_slow(&self) -> bool {
        self.counts.keys().any(|label| label.starts_with("slow:"))
    }

    pub fn render(&self, window: Duration) -> String {
        let secs = window.as_secs_f64().max(f64::EPSILON);
        let mut entries: Vec<(&String, &u32)> = self.counts.iter().collect();
        entries.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let parts: Vec<String> = entries
            .into_iter()
            .map(|(source, count)| format!("{source}={:.1}", f64::from(*count) / secs))
            .collect();
        format!(
            "aqua-shell-wakes: secs={secs:.2} total={:.1} [{}]",
            f64::from(self.total()) / secs,
            parts.join(",")
        )
    }
}

const REPORT_INTERVAL: Duration = Duration::from_secs(1);
const BURST_THRESHOLD: u32 = 30;
const SLOW_STEP_MS: u32 = 2;

struct Trace {
    tally: Tally,
    since: std::time::Instant,
    threshold: u32,
}

thread_local! {
    static TRACE: RefCell<Option<Trace>> = const { RefCell::new(None) };
}

pub fn enable_from_env() {
    let threshold = match std::env::var("AQUA_WAKE_STATS").ok().as_deref() {
        Some("1") => Some(0),
        Some("burst") => Some(BURST_THRESHOLD),
        _ => None,
    };
    TRACE.with_borrow_mut(|trace| {
        *trace = threshold.map(|threshold| Trace {
            tally: Tally::default(),
            since: std::time::Instant::now(),
            threshold,
        })
    });
}

pub fn enabled() -> bool {
    TRACE.with_borrow(|trace| trace.is_some())
}

pub fn record(source: Source) {
    TRACE.with_borrow_mut(|trace| {
        if let Some(trace) = trace.as_mut() {
            trace.tally.record(source);
        }
    });
}

pub fn timed<T>(step: &'static str, body: impl FnOnce() -> T) -> T {
    match enabled() {
        false => body(),
        true => {
            let started = std::time::Instant::now();
            let value = body();
            let millis = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
            if millis >= SLOW_STEP_MS {
                record(Source::Slow { step, millis });
            }
            value
        }
    }
}

pub fn flush_if_due() {
    TRACE.with_borrow_mut(|trace| {
        let Some(trace) = trace.as_mut() else {
            return;
        };
        let elapsed = trace.since.elapsed();
        if elapsed < REPORT_INTERVAL {
            return;
        }
        if trace.tally.total() > 0
            && (trace.tally.total() >= trace.threshold || trace.tally.has_slow())
        {
            eprintln!("{}", trace.tally.render(elapsed));
        }
        trace.tally = Tally::default();
        trace.since = std::time::Instant::now();
    });
}
