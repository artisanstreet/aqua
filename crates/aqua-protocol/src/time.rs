use std::ops::{Add, AddAssign, Sub};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant {
    nanos: u64,
}

impl Instant {
    pub const ZERO: Instant = Instant { nanos: 0 };

    pub const fn from_nanos(nanos: u64) -> Self {
        Self { nanos }
    }

    pub fn from_secs_f64(secs: f64) -> Self {
        let nanos = match secs {
            s if s.is_nan() || s <= 0.0 => 0.0,
            s => (s * 1e9).min(u64::MAX as f64),
        };
        Self {
            nanos: nanos as u64,
        }
    }

    pub const fn as_nanos(self) -> u64 {
        self.nanos
    }

    pub fn as_secs_f64(self) -> f64 {
        self.nanos as f64 / 1e9
    }

    pub fn since_origin(self) -> Duration {
        Duration::from_nanos(self.nanos)
    }

    pub fn saturating_duration_since(self, earlier: Instant) -> Duration {
        Duration::from_nanos(self.nanos.saturating_sub(earlier.nanos))
    }
}

fn duration_nanos(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

impl Add<Duration> for Instant {
    type Output = Instant;

    fn add(self, rhs: Duration) -> Instant {
        Instant {
            nanos: self.nanos.saturating_add(duration_nanos(rhs)),
        }
    }
}

impl AddAssign<Duration> for Instant {
    fn add_assign(&mut self, rhs: Duration) {
        *self = *self + rhs;
    }
}

impl Sub<Duration> for Instant {
    type Output = Instant;

    fn sub(self, rhs: Duration) -> Instant {
        Instant {
            nanos: self.nanos.saturating_sub(duration_nanos(rhs)),
        }
    }
}

impl Sub for Instant {
    type Output = Duration;

    fn sub(self, rhs: Instant) -> Duration {
        self.saturating_duration_since(rhs)
    }
}
