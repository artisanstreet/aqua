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
