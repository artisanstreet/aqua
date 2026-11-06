use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const FALLBACK_INTERVAL: Duration = Duration::from_nanos(16_666_667);
const MIN_INTERVAL: Duration = Duration::from_micros(2_000);
const MAX_INTERVAL: Duration = Duration::from_millis(200);
const WINDOW: usize = 240;

#[derive(Default)]
pub struct VblankGuard {
    finished: Option<Instant>,
    early_waits: u8,
}

impl VblankGuard {
    pub fn observe(&mut self, started: Instant, finished: Instant, interval: Duration) -> bool {
        let idle = self
            .finished
            .is_none_or(|last| started.duration_since(last) >= interval);
        let blocked = finished.duration_since(started) >= Duration::from_millis(1);
        if idle || blocked {
            self.early_waits = 0;
        }
        self.finished = Some(finished);
        if blocked {
            return true;
        }
        self.early_waits = self.early_waits.saturating_add(1);
        self.early_waits < 3
    }
}

pub fn interval_from_ticks(period: u64, frequency: u64) -> Duration {
    match (period, frequency) {
        (0, _) | (_, 0) => FALLBACK_INTERVAL,
        (period, frequency) => {
            let nanos = (period as u128 * 1_000_000_000) / frequency as u128;
            let interval = Duration::from_nanos(nanos.min(u64::MAX as u128) as u64);
            match interval < MIN_INTERVAL || interval > MAX_INTERVAL {
                true => FALLBACK_INTERVAL,
                false => interval,
            }
        }
    }
}

pub fn reconcile_interval(composition: Duration, monitor_hz: u32) -> Duration {
    match monitor_hz {
        0 => composition,
        hz => {
            let nominal = Duration::from_secs_f64(1.0 / hz as f64);
            let drift =
                (composition.as_secs_f64() - nominal.as_secs_f64()).abs() / nominal.as_secs_f64();
            match drift <= 0.01 {
                true => composition,
                false => nominal,
            }
        }
    }
}

pub fn ticks_until_next_vblank(now: u64, vblank: u64, period: u64) -> u64 {
    match period {
        0 => 0,
        period => match now <= vblank {
            true => vblank - now,
            false => period - (now - vblank) % period,
        },
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
    pub mean: Duration,
    pub worst: Duration,
}

#[derive(Clone, Debug)]
pub struct Pacer {
    interval: Duration,
    recent: VecDeque<Duration>,
    frames: u64,
    missed: u64,
    worst: Duration,
}

impl Pacer {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            recent: VecDeque::new(),
            frames: 0,
            missed: 0,
            worst: Duration::ZERO,
        }
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub fn set_interval(&mut self, interval: Duration) {
        self.interval = interval;
        self.recent.clear();
    }

    pub fn observe(&mut self, between_presents: Duration) {
        self.frames += 1;
        self.missed += vblanks_spanned(between_presents, self.interval).saturating_sub(1) as u64;
        self.worst = self.worst.max(between_presents);
        self.recent.push_back(between_presents);
        while self.recent.len() > WINDOW {
            self.recent.pop_front();
        }
    }

    pub fn restart(&mut self) {
        self.recent.clear();
    }

    pub fn report(&self) -> PaceReport {
        let mean = match self.recent.len() {
            0 => Duration::ZERO,
            count => self.recent.iter().sum::<Duration>() / count as u32,
        };
        PaceReport {
            interval: self.interval,
            frames: self.frames,
            missed: self.missed,
            mean,
            worst: self.worst,
        }
    }
}
