use crate::Point;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClickSettings {
    interval: Duration,
    max_distance: Point,
}

impl ClickSettings {
    pub fn new(interval: Duration, max_distance: Point) -> Option<Self> {
        if interval.is_zero()
            || !max_distance.x.is_finite()
            || !max_distance.y.is_finite()
            || max_distance.x < 0.0
            || max_distance.y < 0.0
        {
            return None;
        }
        Some(Self {
            interval,
            max_distance,
        })
    }

    pub fn interval(self) -> Duration {
        self.interval
    }

    pub fn max_distance(self) -> Point {
        self.max_distance
    }
}

impl Default for ClickSettings {
    fn default() -> Self {
        Self {
            interval: Duration::from_millis(500),
            max_distance: Point::new(4.0, 4.0),
        }
    }
}
