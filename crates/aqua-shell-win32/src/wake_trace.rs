use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    Message(u32),
    Sent(u32),
    Hooked { class: String, message: u32 },
    Timer,
    Waitable,
    Timeout,
    Other(u32),
    Queue(u32),
}

#[derive(Debug, Default)]
pub struct Tally {
    counts: BTreeMap<String, u32>,
}

const NAMES: &[(u32, &str)] = &[
    (0x0001, "create"),
    (0x0003, "move"),
    (0x0005, "size"),
    (0x0006, "activate"),
    (0x0007, "setfocus"),
    (0x0008, "killfocus"),
    (0x000F, "paint"),
    (0x0014, "erasebkgnd"),
    (0x001C, "activateapp"),
    (0x0020, "setcursor"),
    (0x0021, "mouseactivate"),
    (0x0024, "getminmaxinfo"),
    (0x0046, "windowposchanging"),
    (0x0047, "windowposchanged"),
    (0x0081, "nccreate"),
    (0x0083, "nccalcsize"),
    (0x0084, "nchittest"),
    (0x0085, "ncpaint"),
    (0x0086, "ncactivate"),
    (0x0100, "keydown"),
    (0x0101, "keyup"),
    (0x0102, "char"),
    (0x0113, "timer"),
    (0x0200, "mousemove"),
    (0x02A3, "mouseleave"),
    (0x02E0, "dpichanged"),
    (0x031E, "dwmcompositionchanged"),
    (0x031F, "dwmncrenderingchanged"),
    (0x0320, "dwmcolorizationcolorchanged"),
    (0x0321, "dwmwindowmaximizedchange"),
    (0x8001, "wake"),
    (0x0238, "pointerdeviceinrange"),
    (0x0245, "pointerupdate"),
    (0x0249, "pointercapturechanged"),
    (0x024A, "pointerleave"),
    (0x0281, "imesetcontext"),
    (0x0282, "imenotify"),
    (0x0288, "devicechange"),
    (0x001A, "settingchange"),
    (0x0112, "syscommand"),
    (0x0011, "queryendsession"),
    (0x003D, "getobject"),
    (0x0018, "showwindow"),
    (0x0082, "ncdestroy"),
    (0x0002, "destroy"),
    (0x0010, "close"),
    (0x02B1, "wtsessionchange"),
    (0x0319, "appcommand"),
    (0x004E, "notify"),
    (0x0090, "uahdestroywindow"),
    (0x00AE, "ncuahdrawcaption"),
    (0x00AF, "ncuahdrawframe"),
    (0x0127, "changeuistate"),
    (0x0128, "updateuistate"),
    (0x02C0, "gestureinput"),
    (0x0049, "copydata"),
    (0x0313, "contextmenu_ex"),
    (0x007F, "geticon"),
    (0x0361, "dwm_sendiconic"),
];

fn name(message: u32) -> String {
    NAMES
        .iter()
        .find_map(|(id, name)| (*id == message).then(|| (*name).to_string()))
        .unwrap_or_else(|| format!("0x{message:04x}"))
}

const QUEUE_BITS: &[(u32, &str)] = &[
    (0x0001, "key"),
    (0x0002, "mousemove"),
    (0x0004, "mousebutton"),
    (0x0008, "post"),
    (0x0010, "timer"),
    (0x0020, "paint"),
    (0x0040, "send"),
    (0x0080, "hotkey"),
    (0x0100, "allpost"),
    (0x0400, "rawinput"),
    (0x0800, "touch"),
    (0x1000, "pointer"),
];

fn queue_flags(flags: u32) -> String {
    let names: Vec<&str> = QUEUE_BITS
        .iter()
        .filter_map(|(bit, name)| (flags & bit != 0).then_some(*name))
        .collect();
    match names.is_empty() {
        true => "none".into(),
        false => names.join("+"),
    }
}

impl Source {
    fn label(&self) -> String {
        match self {
            Self::Hooked { class, message } => format!("hook:{class}:{}", name(*message)),
            Self::Message(id) => name(*id),
            Self::Sent(id) => format!("sent:{}", name(*id)),
            Self::Timer => "loop:timer".into(),
            Self::Waitable => "loop:waitable".into(),
            Self::Timeout => "loop:timeout".into(),
            Self::Queue(flags) => format!("loop:queue-{}", queue_flags(*flags)),
            Self::Other(code) => format!("loop:wait-result-{code}"),
        }
    }
}

impl Tally {
    pub fn record(&mut self, source: Source) {
        *self.counts.entry(source.label()).or_default() += 1;
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    pub fn total(&self) -> u32 {
        self.counts.values().sum()
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

    pub fn clear(&mut self) {
        self.counts.clear();
    }
}

const REPORT_INTERVAL: Duration = Duration::from_secs(1);

struct Trace {
    tally: Tally,
    since: std::time::Instant,
    dispatching: bool,
}

thread_local! {
    static TRACE: std::cell::RefCell<Option<Trace>> = const { std::cell::RefCell::new(None) };
}

pub fn enable_from_env() {
    if std::env::var_os("AQUA_WAKE_STATS").is_some_and(|value| value == "1") {
        TRACE.with(|trace| {
            *trace.borrow_mut() = Some(Trace {
                tally: Tally::default(),
                since: std::time::Instant::now(),
                dispatching: false,
            });
        });
    }
}

pub fn enabled() -> bool {
    TRACE.with(|trace| trace.borrow().is_some())
}

pub fn record(source: Source) {
    TRACE.with(|trace| {
        if let Some(trace) = trace.borrow_mut().as_mut() {
            trace.tally.record(source);
        }
    });
}

pub fn record_sent(message: u32) {
    TRACE.with(|trace| {
        if let Some(trace) = trace.borrow_mut().as_mut() {
            if !trace.dispatching {
                trace.tally.record(Source::Sent(message));
            }
        }
    });
}

pub fn dispatching(active: bool) {
    TRACE.with(|trace| {
        if let Some(trace) = trace.borrow_mut().as_mut() {
            trace.dispatching = active;
        }
    });
}

pub fn flush_if_due() {
    TRACE.with(|trace| {
        if let Some(trace) = trace.borrow_mut().as_mut() {
            let elapsed = trace.since.elapsed();
            if elapsed >= REPORT_INTERVAL {
                if !trace.tally.is_empty() {
                    eprintln!("{}", trace.tally.render(elapsed));
                }
                trace.tally.clear();
                trace.since = std::time::Instant::now();
            }
        }
    });
}
