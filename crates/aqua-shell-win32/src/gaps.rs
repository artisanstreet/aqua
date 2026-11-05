use std::collections::HashMap;
use std::panic::Location;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const MIN_GAP: Duration = Duration::from_micros(1000);
const REPORT_EVERY: Duration = Duration::from_secs(1);
const REPORT_ROWS: usize = 6;

type Site = (&'static str, u32);

#[derive(Default)]
struct Stat {
    count: u32,
    total: Duration,
    max: Duration,
}

struct Tracker {
    last: Option<(Instant, Site)>,
    stats: HashMap<(Site, Site), Stat>,
    reported: Instant,
}

thread_local! {
    static TRACKER: std::cell::RefCell<Option<Tracker>> = const { std::cell::RefCell::new(None) };
}

static ENABLED: OnceLock<bool> = OnceLock::new();

const TURN_START: Site = ("turn_start", 0);
const TURN_END: Site = ("turn_end", 0);

fn enabled() -> bool {
    *ENABLED.get_or_init(|| std::env::var_os("AQUA_GAP_TRACE").is_some_and(|value| value != "0"))
}

fn site(location: &'static Location<'static>) -> Site {
    (location.file(), location.line())
}

fn visit(at: Site) {
    let now = Instant::now();
    TRACKER.with(|cell| {
        let mut slot = cell.borrow_mut();
        let tracker = slot.get_or_insert_with(|| Tracker {
            last: None,
            stats: HashMap::new(),
            reported: now,
        });
        if let Some((then, from)) = tracker.last.replace((now, at)) {
            let gap = now.duration_since(then);
            if gap >= MIN_GAP {
                let stat = tracker.stats.entry((from, at)).or_default();
                stat.count += 1;
                stat.total += gap;
                stat.max = stat.max.max(gap);
            }
        }
        if at == TURN_START {
            tracker.last = Some((now, at));
        }
        if now.duration_since(tracker.reported) >= REPORT_EVERY && !tracker.stats.is_empty() {
            report(tracker);
            tracker.reported = now;
        }
    });
}

fn report(tracker: &mut Tracker) {
    let mut rows: Vec<_> = tracker.stats.drain().collect();
    rows.sort_by_key(|(_, stat)| std::cmp::Reverse(stat.max));
    for ((from, to), stat) in rows.into_iter().take(REPORT_ROWS) {
        eprintln!(
            "aqua-gap from={}:{} to={}:{} n={} max_ms={:.2} total_ms={:.2}",
            short(from.0),
            from.1,
            short(to.0),
            to.1,
            stat.count,
            stat.max.as_secs_f64() * 1e3,
            stat.total.as_secs_f64() * 1e3
        );
    }
}

fn short(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

pub(crate) fn reached(location: &'static Location<'static>) {
    if enabled() {
        visit(site(location));
    }
}

pub(crate) fn turn_started() {
    if enabled() {
        TRACKER.with(|cell| {
            if let Some(tracker) = cell.borrow_mut().as_mut() {
                tracker.last = None;
            }
        });
        visit(TURN_START);
    }
}

pub(crate) fn turn_ended() {
    if enabled() {
        visit(TURN_END);
    }
}
