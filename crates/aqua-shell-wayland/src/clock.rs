use aqua_protocol::Instant;
use rustix::time::{clock_gettime, ClockId, Timespec};
use std::sync::OnceLock;
use std::time::Duration;

static ORIGIN: OnceLock<u64> = OnceLock::new();

fn nanos(at: Timespec) -> u64 {
    let seconds = u64::try_from(at.tv_sec).unwrap_or(0);
    let fraction = u64::try_from(at.tv_nsec).unwrap_or(0);
    seconds
        .saturating_mul(1_000_000_000)
        .saturating_add(fraction)
}

fn origin() -> u64 {
    *ORIGIN.get_or_init(|| nanos(clock_gettime(ClockId::Monotonic)))
}

pub fn now() -> Instant {
    let origin = origin();
    Instant::from_nanos(nanos(clock_gettime(ClockId::Monotonic)).saturating_sub(origin))
}

pub fn from_monotonic(seconds: u64, nanoseconds: u32) -> Instant {
    let at = seconds
        .saturating_mul(1_000_000_000)
        .saturating_add(u64::from(nanoseconds));
    Instant::from_nanos(at.saturating_sub(origin()))
}

pub(crate) fn to_monotonic(at: Instant) -> Timespec {
    let total = at.as_nanos().saturating_add(origin());
    Timespec {
        tv_sec: i64::try_from(total / 1_000_000_000).unwrap_or(i64::MAX),
        tv_nsec: i64::try_from(total % 1_000_000_000).unwrap_or(0),
    }
}

pub(crate) fn timespec(duration: Duration) -> Timespec {
    Timespec {
        tv_sec: i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
        tv_nsec: i64::from(duration.subsec_nanos()),
    }
}
