use std::time::Duration;

use super::selection::Adapter;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LossCause {
    Removed,
    Hung,
    Reset,
    DriverInternal,
    InvalidCall,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryStep {
    Reselect,
    FallBackToWarp,
}

#[derive(Clone, Debug)]
pub struct LossHistory {
    window: Duration,
    limit: usize,
    losses: Vec<Duration>,
}

impl LossHistory {
    pub fn new(window: Duration, limit: usize) -> Self {
        Self {
            window,
            limit,
            losses: Vec::new(),
        }
    }

    pub fn record(&mut self, at: Duration, adapter: Adapter) -> RecoveryStep {
        self.losses
            .retain(|earlier| at.saturating_sub(*earlier) <= self.window);
        self.losses.push(at);
        match (adapter, self.losses.len() >= self.limit) {
            (Adapter::Hardware, true) => RecoveryStep::FallBackToWarp,
            _ => RecoveryStep::Reselect,
        }
    }
}

const DEVICE_REMOVED: i32 = 0x887A0005u32 as i32;
const DEVICE_HUNG: i32 = 0x887A0006u32 as i32;
const DEVICE_RESET: i32 = 0x887A0007u32 as i32;
const DRIVER_INTERNAL_ERROR: i32 = 0x887A0020u32 as i32;
const INVALID_CALL: i32 = 0x887A0001u32 as i32;

pub fn classify(code: i32) -> Option<LossCause> {
    match code {
        DEVICE_REMOVED => Some(LossCause::Removed),
        DEVICE_HUNG => Some(LossCause::Hung),
        DEVICE_RESET => Some(LossCause::Reset),
        DRIVER_INTERNAL_ERROR => Some(LossCause::DriverInternal),
        INVALID_CALL => Some(LossCause::InvalidCall),
        _ => None,
    }
}
