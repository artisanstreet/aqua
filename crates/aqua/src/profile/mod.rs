use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use aqua_protocol::probe::{self, Event, Metric};

pub mod alloc;
pub mod recorder;
pub mod report;

pub use aqua_protocol::probe::{Metric as ProfileMetric, Phase};
pub use recorder::Role;

struct Session {
    reporting: bool,
    config: report::Config,
    output: report::Output,
    sequence: AtomicU64,
}

static SESSION: OnceLock<Session> = OnceLock::new();

struct Local {
    recorder: recorder::Recorder,
    summary: Option<(Role, report::Summary)>,
}

thread_local! {
    static LOCAL: RefCell<Option<Local>> = const { RefCell::new(None) };
}

fn with_local<R>(work: impl FnOnce(&mut Local) -> R) -> Option<R> {
    LOCAL
        .try_with(|slot| {
            let mut slot = slot.try_borrow_mut().ok()?;
            let local = slot.get_or_insert_with(|| Local {
                recorder: recorder::Recorder::new(),
                summary: None,
            });
            Some(work(local))
        })
        .ok()
        .flatten()
}

fn sink(event: Event) {
    with_local(|local| local.recorder.event(event));
}

pub fn init_from_environment() -> bool {
    let reporting = std::env::var_os("AQUA_PROFILE").is_some_and(|value| value == "1");
    if !reporting && !crate::debug::active() {
        return false;
    }
    let installed = SESSION
        .set(Session {
            reporting,
            config: report::Config::from_environment(),
            output: report::Output::from_environment(),
            sequence: AtomicU64::new(0),
        })
        .is_ok();
    if installed {
        probe::install(sink);
        if let Some(session) = SESSION.get().filter(|session| session.reporting) {
            session.output.line(&format!(
                "aqua-profile-start heap_counters={} spike_ms={:.1}",
                alloc::counting(),
                session.config.spike_nanos as f64 / 1_000_000.0
            ));
        }
    }
    installed
}

#[must_use]
pub fn enabled() -> bool {
    SESSION.get().is_some()
}

pub fn begin_frame() {
    if enabled() {
        with_local(|local| local.recorder.begin());
    }
}

pub fn note(text: impl FnOnce() -> String) {
    if enabled() {
        let text = text();
        with_local(|local| local.recorder.note(text));
    }
}

pub fn count(metric: Metric, value: u64) {
    probe::count(metric, value);
}

pub fn end_frame(role: Role, label: u64) {
    let Some(session) = SESSION.get() else {
        return;
    };
    with_local(|local| {
        let frame = local.recorder.finish(role, label);
        crate::debug::frame(&frame);
        if !session.reporting {
            return;
        }
        let sequence = session.sequence.fetch_add(1, Ordering::Relaxed);
        if frame.work_nanos() >= session.config.spike_nanos {
            session
                .output
                .line(&report::spike_line(&session.config, &frame, sequence));
        }
        let summary = match &mut local.summary {
            Some((current, summary)) if *current == role => summary,
            slot => &mut slot.insert((role, report::Summary::new(role))).1,
        };
        if let Some(line) = summary.record(&frame) {
            session.output.line(&line);
        }
    });
}
