use aqua_protocol::{Instant, WindowId};
use std::time::Duration;
use xkbcommon::xkb::Keycode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RepeatInfo {
    pub per_second: u32,
    pub delay: Duration,
}

impl Default for RepeatInfo {
    fn default() -> Self {
        Self {
            per_second: 25,
            delay: Duration::from_millis(600),
        }
    }
}

impl RepeatInfo {
    fn interval(self) -> Option<Duration> {
        (self.per_second > 0).then(|| Duration::from_secs(1) / self.per_second)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Repeat {
    #[default]
    Idle,
    Held {
        window: WindowId,
        key: Keycode,
        next: Instant,
    },
}

impl Repeat {
    pub(crate) fn press(info: RepeatInfo, window: WindowId, key: Keycode, now: Instant) -> Self {
        match info.interval() {
            Some(_) => Repeat::Held {
                window,
                key,
                next: now + info.delay,
            },
            None => Repeat::Idle,
        }
    }

    pub(crate) fn release(&mut self, released: Keycode) {
        if matches!(self, Repeat::Held { key, .. } if *key == released) {
            *self = Repeat::Idle;
        }
    }

    pub(crate) fn deadline(self) -> Option<Instant> {
        match self {
            Repeat::Held { next, .. } => Some(next),
            Repeat::Idle => None,
        }
    }

    pub(crate) fn due(&mut self, info: RepeatInfo, now: Instant) -> Option<(WindowId, Keycode)> {
        let (Repeat::Held { window, key, next }, Some(interval)) = (*self, info.interval()) else {
            return None;
        };
        if next > now {
            return None;
        }
        let following = next + interval;
        *self = Repeat::Held {
            window,
            key,
            next: match following > now {
                true => following,
                false => now + interval,
            },
        };
        Some((window, key))
    }
}
