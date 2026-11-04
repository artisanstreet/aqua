use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const REPORT_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RunCause {
    pub arrived: bool,
    pub forced: bool,
    pub trim: bool,
    pub repaint: bool,
    pub wake_due: bool,
    pub inbox: bool,
    pub feedback: bool,
    pub visibility: bool,
    pub framed: bool,
    pub presented: bool,
}

struct Ledger {
    window: Instant,
    runs: BTreeMap<RunCause, u32>,
}

thread_local! {
    static LEDGER: RefCell<Option<Ledger>> = const { RefCell::new(None) };
}

pub fn enabled() -> bool {
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("AQUA_WAKE_STATS").is_some_and(|value| value == "1"))
}

fn flags(cause: &RunCause) -> String {
    [
        (cause.arrived, "arrived"),
        (cause.forced, "forced"),
        (cause.trim, "trim"),
        (cause.repaint, "repaint"),
        (cause.wake_due, "wake_due"),
        (cause.inbox, "inbox"),
        (cause.feedback, "feedback"),
        (cause.visibility, "visibility"),
        (cause.framed, "framed"),
        (cause.presented, "presented"),
    ]
    .into_iter()
    .filter_map(|(set, name)| set.then_some(name))
    .collect::<Vec<_>>()
    .join("+")
}

pub fn record(cause: RunCause) {
    if !enabled() {
        return;
    }
    LEDGER.with(|cell| {
        let mut slot = cell.borrow_mut();
        let ledger = slot.get_or_insert_with(|| Ledger {
            window: Instant::now(),
            runs: BTreeMap::new(),
        });
        *ledger.runs.entry(cause).or_default() += 1;
        let elapsed = ledger.window.elapsed();
        if elapsed < REPORT_INTERVAL {
            return;
        }
        let secs = elapsed.as_secs_f64();
        let causes: Vec<String> = ledger
            .runs
            .iter()
            .map(|(cause, count)| format!("{}={:.1}", flags(cause), f64::from(*count) / secs))
            .collect();
        eprintln!("aqua-render-runs: secs={secs:.2} [{}]", causes.join(" "));
        ledger.runs.clear();
        ledger.window = Instant::now();
    });
}
