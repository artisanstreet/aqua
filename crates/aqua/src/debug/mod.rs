use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

pub mod history;
pub mod log;
pub mod panel;
#[cfg(windows)]
pub mod window;

use history::{History, Sample, Thread};

use crate::profile::recorder::{Finished, Role};
use crate::render::host::RenderStats;

pub const PANEL_ENV: &str = "AQUA_DEBUG_PANEL";
pub const LOG_ENV: &str = "AQUA_DEBUG_LOG";

struct Debug {
    origin: Instant,
    panel: bool,
    log: Option<log::Log>,
    history: Mutex<History>,
    watched: Mutex<Option<Watched>>,
}

#[derive(Clone)]
pub struct Watched {
    pub stats: Arc<RenderStats>,
    pub hwnd: isize,
}

static DEBUG: OnceLock<Debug> = OnceLock::new();

pub fn init_from_environment() -> bool {
    let panel = std::env::var_os(PANEL_ENV).is_some_and(|value| value == "1");
    let log = std::env::var_os(LOG_ENV)
        .filter(|path| !path.is_empty())
        .and_then(|path| match log::Log::open(path.as_ref()) {
            Ok(log) => Some(log),
            Err(error) => {
                eprintln!(
                    "aqua: debug log {} unavailable: {error}",
                    path.to_string_lossy()
                );
                None
            }
        });
    if !panel && log.is_none() {
        return false;
    }
    DEBUG
        .set(Debug {
            origin: Instant::now(),
            panel,
            log,
            history: Mutex::new(History::default()),
            watched: Mutex::new(None),
        })
        .is_ok()
}

#[must_use]
pub fn active() -> bool {
    DEBUG.get().is_some()
}

#[must_use]
pub fn panel() -> bool {
    DEBUG.get().is_some_and(|debug| debug.panel)
}

fn nanos_since(origin: Instant) -> u64 {
    u64::try_from(origin.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

#[must_use]
pub fn now() -> u64 {
    DEBUG.get().map_or(0, |debug| nanos_since(debug.origin))
}

pub fn frame(finished: &Finished) {
    let Some(debug) = DEBUG.get() else {
        return;
    };
    let heap = finished.total_heap();
    let sample = Sample {
        thread: match finished.role {
            Role::App => Thread::App,
            Role::Render => Thread::Render,
        },
        window: finished.label,
        at: nanos_since(debug.origin),
        wall: finished.wall_nanos,
        phases: finished.nanos,
        metrics: finished.metrics,
        allocs: heap.allocs,
        bytes: heap.bytes,
        notes: finished.notes.clone(),
    };
    if let Some(log) = &debug.log {
        log.line(&log::frame_line(&sample));
    }
    if debug.panel {
        lock(&debug.history).record(sample);
    }
}

pub fn presented(window: u64) {
    let Some(debug) = DEBUG.get() else {
        return;
    };
    let at = nanos_since(debug.origin);
    let mut history = lock(&debug.history);
    let previous = history.last_present();
    history.present(at);
    drop(history);
    if let Some(log) = &debug.log {
        log.line(&log::present_line(
            window,
            at,
            previous.map(|previous| at.saturating_sub(previous)),
        ));
    }
}

pub fn with_history<R>(read: impl FnOnce(&History) -> R) -> Option<R> {
    DEBUG.get().map(|debug| read(&lock(&debug.history)))
}

fn lock(history: &Mutex<History>) -> std::sync::MutexGuard<'_, History> {
    history
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn watch(stats: Arc<RenderStats>, hwnd: isize) {
    if let Some(debug) = DEBUG.get() {
        debug
            .watched
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get_or_insert(Watched { stats, hwnd });
    }
}

#[must_use]
pub fn watched() -> Option<Watched> {
    DEBUG.get().and_then(|debug| {
        debug
            .watched
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    })
}
