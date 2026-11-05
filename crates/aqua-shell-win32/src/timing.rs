use aqua_protocol::Instant;
use std::time::Duration;

pub fn until(now: Instant, at: Instant) -> Duration {
    at.saturating_duration_since(now)
}

pub fn due(now: Instant, wake_at: Option<Instant>) -> bool {
    wake_at.is_some_and(|at| at <= now)
}

pub fn relative_due_time_100ns(now: Instant, at: Instant) -> i64 {
    let ticks = until(now, at).as_nanos().div_ceil(100).max(1);
    -i64::try_from(ticks).unwrap_or(i64::MAX)
}

pub fn timeout_ms(now: Instant, wake_at: Option<Instant>) -> u32 {
    wake_at.map_or(u32::MAX, |at| {
        u32::try_from(until(now, at).as_nanos().div_ceil(1_000_000)).unwrap_or(u32::MAX - 1)
    })
}

pub const PRECISE_HORIZON: Duration = Duration::from_millis(100);
pub const MAX_LEEWAY: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerPlan {
    Precise,
    Coarse { leeway_ms: u32 },
}

pub fn plan(now: Instant, at: Instant) -> TimerPlan {
    let distance = until(now, at);
    match distance > PRECISE_HORIZON {
        false => TimerPlan::Precise,
        true => TimerPlan::Coarse {
            leeway_ms: u32::try_from((distance / 10).min(MAX_LEEWAY).as_millis())
                .unwrap_or(u32::MAX),
        },
    }
}
