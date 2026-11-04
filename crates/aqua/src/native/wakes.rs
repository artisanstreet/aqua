use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use aqua_protocol::Instant as AquaInstant;

use crate::inbox::{Inbox, PostCounts};

const REPORT_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cause {
    Timer,
    WindowMessage,
    InboxWake,
    Dialog,
}

impl Cause {
    const fn label(self) -> &'static str {
        match self {
            Self::Timer => "timer",
            Self::WindowMessage => "window-message",
            Self::InboxWake => "inbox-wake",
            Self::Dialog => "dialog-poll",
        }
    }
}

pub struct WakeLedger {
    inbox: Inbox,
    window: Instant,
    turns: BTreeMap<Cause, u32>,
    messages: BTreeMap<&'static str, u32>,
    idle_ticks: u32,
    expected: Option<AquaInstant>,
    wake_flag: bool,
    message_flag: bool,
}

pub fn enabled() -> bool {
    std::env::var_os("AQUA_WAKE_STATS").is_some_and(|value| value == "1")
}

impl WakeLedger {
    pub fn new(inbox: Inbox) -> Self {
        inbox.account_posts(true);
        Self {
            inbox,
            window: Instant::now(),
            turns: BTreeMap::new(),
            messages: BTreeMap::new(),
            idle_ticks: 0,
            expected: None,
            wake_flag: false,
            message_flag: false,
        }
    }

    pub fn message(&mut self, label: &'static str) {
        *self.messages.entry(label).or_default() += 1;
        self.message_flag = true;
    }

    pub fn woken(&mut self) {
        self.wake_flag = true;
    }

    pub fn tick(&mut self, now: AquaInstant, presented: bool, next: Option<AquaInstant>) {
        let timer_due = self.expected.is_some_and(|due| now >= due);
        let cause = match (self.message_flag, self.wake_flag, timer_due) {
            (true, _, _) => Cause::WindowMessage,
            (false, true, _) => Cause::InboxWake,
            (false, false, true) => Cause::Timer,
            (false, false, false) => Cause::Dialog,
        };
        *self.turns.entry(cause).or_default() += 1;
        if !presented {
            self.idle_ticks += 1;
        }
        self.message_flag = false;
        self.wake_flag = false;
        self.expected = next;
        self.report();
    }

    fn report(&mut self) {
        let elapsed = self.window.elapsed();
        if elapsed < REPORT_INTERVAL {
            return;
        }
        let secs = elapsed.as_secs_f64();
        let rate = |count: u32| f64::from(count) / secs;
        let turns: Vec<String> = self
            .turns
            .iter()
            .map(|(cause, count)| format!("{}={:.1}", cause.label(), rate(*count)))
            .collect();
        let messages: Vec<String> = self
            .messages
            .iter()
            .map(|(label, count)| format!("{label}={:.1}", rate(*count)))
            .collect();
        let posts: PostCounts = self.inbox.take_post_counts();
        let posts: Vec<String> = posts
            .iter()
            .map(|((kind, thread), count)| format!("{kind}@{thread}={:.1}", rate(*count)))
            .collect();
        eprintln!(
            "aqua-wakes: secs={secs:.2} turns[{}] unpresented_ticks={:.1} messages[{}] posts[{}]",
            turns.join(","),
            rate(self.idle_ticks),
            messages.join(","),
            posts.join(",")
        );
        self.turns.clear();
        self.messages.clear();
        self.idle_ticks = 0;
        self.window = Instant::now();
    }
}
