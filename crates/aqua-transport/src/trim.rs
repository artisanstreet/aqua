use std::time::Duration;

const TRIM_INTERVAL_FRAMES: u64 = 300;
const UNUSED_FRAMES: u64 = 1800;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrimInput {
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
}

impl TrimPolicy {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&mut self, input: TrimInput) -> TrimAction {
        if input.requested {
            self.since_trim = 0;
            return TrimAction::Everything;
        }
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
