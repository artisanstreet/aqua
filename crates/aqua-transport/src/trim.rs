use std::time::Duration;

use aqua_protocol::Instant;

const TRIM_INTERVAL_FRAMES: u64 = 300;
const UNUSED_FRAMES: u64 = 600;
pub const IDLE_TRIM_AFTER: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrimInput {
    pub now: Instant,
    pub uploaded: bool,
    pub cpu_time: Duration,
    pub gpu_time: Option<Duration>,
    pub budget: Duration,
    pub requested: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrimAction {
    Keep,
    Unused(u64),
    Everything,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TrimPolicy {
    since_trim: u64,
    last_active: Option<Instant>,
    idle_trimmed: bool,
}

impl TrimPolicy {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn idle_deadline(&self) -> Option<Instant> {
        self.last_active
            .filter(|_| !self.idle_trimmed)
            .map(|at| at + IDLE_TRIM_AFTER)
    }

    pub fn observe(&mut self, input: TrimInput) -> TrimAction {
        if input.requested {
            self.since_trim = 0;
            if self.idle_deadline().is_some_and(|due| due <= input.now) {
                self.idle_trimmed = true;
            }
            return TrimAction::Everything;
        }
        self.last_active = Some(input.now);
        self.idle_trimmed = false;
        self.since_trim += 1;
        let cost = input
            .gpu_time
            .map_or(input.cpu_time, |gpu| gpu.max(input.cpu_time));
        let slack = cost * 2 <= input.budget;
        match (
            self.since_trim >= TRIM_INTERVAL_FRAMES,
            input.uploaded,
            slack,
        ) {
            (true, false, true) => {
                self.since_trim = 0;
                TrimAction::Unused(UNUSED_FRAMES)
            }
            _ => TrimAction::Keep,
        }
    }
}
