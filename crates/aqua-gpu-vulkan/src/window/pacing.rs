use std::collections::VecDeque;
use std::time::Duration;

use aqua_graph::Extent;
use aqua_protocol::PresentHint;

const WINDOW: usize = 240;
const FALLBACK_INTERVAL: Duration = Duration::from_nanos(16_666_667);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameWait {
    Ready,
    TimedOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presentation {
    Shown {
        at: Option<aqua_protocol::Instant>,
        refresh: Option<Duration>,
    },
    Discarded,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the surface pacer failed: {reason}")]
pub struct PacerError {
    pub reason: String,
}

pub trait SurfacePacer {
    fn before_present(&mut self, buffer: Extent);
    fn wait_ready(&mut self, timeout: Duration) -> Result<FrameWait, PacerError>;
    fn take_presented(&mut self) -> Vec<Presentation>;
    fn refresh(&self) -> Option<Duration>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Free,
    Pending { since: std::time::Instant },
    Starved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    None,
    Poll,
    For(Duration),
}

pub fn starvation_bound(interval: Duration) -> Duration {
    interval * 2
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSlot {
    slot: Slot,
}

impl Default for FrameSlot {
    fn default() -> Self {
        Self { slot: Slot::Free }
    }
}

impl FrameSlot {
    pub fn slot(&self) -> Slot {
        self.slot
    }

    pub fn plan(&self, now: std::time::Instant, hint: PresentHint, interval: Duration) -> Wait {
        match (self.slot, hint) {
            (Slot::Free, _) => Wait::None,
            (Slot::Starved, _) | (Slot::Pending { .. }, PresentHint::Immediate) => Wait::Poll,
            (Slot::Pending { since }, PresentHint::Vsync) => {
                Wait::For((since + starvation_bound(interval)).saturating_duration_since(now))
            }
        }
    }

    pub fn resolve(&mut self, now: std::time::Instant, outcome: FrameWait, interval: Duration) {
        self.slot = match (self.slot, outcome) {
            (_, FrameWait::Ready) => Slot::Free,
            (Slot::Pending { since }, FrameWait::TimedOut)
                if now.saturating_duration_since(since) >= starvation_bound(interval) =>
            {
                Slot::Starved
            }
            (slot, FrameWait::TimedOut) => slot,
        };
    }

    pub fn starve(&mut self) {
        self.slot = Slot::Starved;
    }

    pub fn presented(&mut self, now: std::time::Instant) {
        self.slot = match self.slot {
            Slot::Starved => Slot::Starved,
            Slot::Free | Slot::Pending { .. } => Slot::Pending { since: now },
        };
    }
}

pub fn vblanks_spanned(elapsed: Duration, interval: Duration) -> u32 {
    match interval.is_zero() {
        true => 0,
        false => {
            let ratio = elapsed.as_secs_f64() / interval.as_secs_f64();
            (ratio - 0.25).max(0.0).round() as u32
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaceReport {
    pub interval: Duration,
    pub frames: u64,
    pub missed: u64,
    pub discarded: u64,
    pub mean: Duration,
    pub worst: Duration,
    pub last_shown: Option<aqua_protocol::Instant>,
}

#[derive(Clone, Debug)]
pub struct Pacer {
    interval: Option<Duration>,
    recent: VecDeque<Duration>,
    frames: u64,
    missed: u64,
    discarded: u64,
    worst: Duration,
    last_shown: Option<aqua_protocol::Instant>,
}

impl Default for Pacer {
    fn default() -> Self {
        Self {
            interval: None,
            recent: VecDeque::new(),
            frames: 0,
            missed: 0,
            discarded: 0,
            worst: Duration::ZERO,
            last_shown: None,
        }
    }
}

impl Pacer {
    pub fn interval(&self, advertised: Option<Duration>) -> Duration {
        self.interval
            .or(advertised)
            .filter(|interval| !interval.is_zero())
            .unwrap_or(FALLBACK_INTERVAL)
    }

    pub fn observe(&mut self, presentation: Presentation) {
        match presentation {
            Presentation::Discarded => self.discarded += 1,
            Presentation::Shown { at, refresh } => {
                if let Some(refresh) = refresh.filter(|refresh| !refresh.is_zero()) {
                    self.interval = Some(refresh);
                }
                let between = at
                    .zip(self.last_shown)
                    .map(|(at, last)| at.saturating_duration_since(last));
                if at.is_some() {
                    self.last_shown = at;
                }
                self.frames += 1;
                if let Some(between) = between {
                    let interval = self.interval(None);
                    self.missed += u64::from(vblanks_spanned(between, interval).saturating_sub(1));
                    self.worst = self.worst.max(between);
                    self.recent.push_back(between);
                    while self.recent.len() > WINDOW {
                        self.recent.pop_front();
                    }
                }
            }
        }
    }

    pub fn restart(&mut self) {
        self.recent.clear();
        self.last_shown = None;
    }

    pub fn report(&self) -> PaceReport {
        let mean = match self.recent.len() {
            0 => Duration::ZERO,
            count => self.recent.iter().sum::<Duration>() / count as u32,
        };
        PaceReport {
            interval: self.interval(None),
            frames: self.frames,
            missed: self.missed,
            discarded: self.discarded,
            mean,
            worst: self.worst,
            last_shown: self.last_shown,
        }
    }
}
