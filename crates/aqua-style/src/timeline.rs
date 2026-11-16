use std::time::Duration;

use aqua_core::scene::GpuiShadowLayer;
use aqua_core::{GpuiBackground, GpuiGradientStop, GpuiHsla, Transform2D};
use aqua_motion::Easing;

use crate::Paint;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key<T> {
    pub at: f32,
    pub value: T,
    pub easing: Easing,
}

impl<T> Key<T> {
    #[must_use]
    pub const fn new(at: f32, value: T, easing: Easing) -> Self {
        Self { at, value, easing }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Lane {
    Opacity(Vec<Key<f32>>),
    Translate(Vec<Key<(f32, f32)>>),
    Scale {
        origin: (f32, f32),
        keys: Vec<Key<f32>>,
    },
    Shadow(Vec<Key<GpuiShadowLayer>>),
    Background(Vec<Key<GpuiBackground>>),
    Blur(Vec<Key<f32>>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Timeline {
    duration: f32,
    offset: f32,
    hold: bool,
    lanes: Vec<Lane>,
}

impl Timeline {
    #[must_use]
    pub fn new(duration: Duration) -> Self {
        Self {
            duration: duration.as_secs_f32(),
            offset: 0.0,
            hold: false,
            lanes: Vec::new(),
        }
    }
    #[must_use]
    pub fn elapsed(mut self, elapsed: Duration) -> Self {
        self.offset = elapsed.as_secs_f32();
        self
    }

    #[must_use]
    pub const fn hold(mut self) -> Self {
        self.hold = true;
        self
    }

    #[must_use]
    pub fn lane(mut self, lane: Lane) -> Self {
        self.lanes.push(lane);
        self
    }

    #[must_use]
    pub fn opacity(self, keys: impl Into<Vec<Key<f32>>>) -> Self {
        self.lane(Lane::Opacity(keys.into()))
    }

    #[must_use]
    pub fn translate(self, keys: impl Into<Vec<Key<(f32, f32)>>>) -> Self {
        self.lane(Lane::Translate(keys.into()))
    }
    #[must_use]
    pub fn scale(self, origin: (f32, f32), keys: impl Into<Vec<Key<f32>>>) -> Self {
        self.lane(Lane::Scale {
            origin,
            keys: keys.into(),
        })
    }
    #[must_use]
    pub fn shadow(self, keys: impl Into<Vec<Key<GpuiShadowLayer>>>) -> Self {
        self.lane(Lane::Shadow(keys.into()))
    }
    #[must_use]
    pub fn background(self, keys: impl Into<Vec<Key<GpuiBackground>>>) -> Self {
        self.lane(Lane::Background(keys.into()))
    }

    #[must_use]
    pub fn blur(self, keys: impl Into<Vec<Key<f32>>>) -> Self {
        self.lane(Lane::Blur(keys.into()))
    }

    #[must_use]
    pub const fn duration(&self) -> f32 {
        self.duration
    }

    #[must_use]
    pub const fn offset(&self) -> f32 {
        self.offset
    }

    #[must_use]
    pub fn finished(&self, elapsed: f32) -> bool {
        elapsed >= self.duration
    }
    #[must_use]
    pub fn sample(&self, rest: &Paint, size: (f32, f32), elapsed: f32) -> Paint {
        let mut paint = rest.clone();
        if self.finished(elapsed) && !self.hold {
            return paint;
        }
        let progress = if self.duration > 0.0 {
            (elapsed / self.duration).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let mut motion = Transform2D::IDENTITY;
        for lane in &self.lanes {
            match lane {
                Lane::Opacity(keys) => {
                    if let Some(opacity) = at(keys, progress) {
                        paint.opacity = Some(paint.opacity.unwrap_or(1.0) * opacity);
                    }
                }
                Lane::Translate(keys) => {
                    if let Some((x, y)) = at(keys, progress) {
                        motion = motion.then(Transform2D::translate(x, y));
                    }
                }
                Lane::Scale { origin, keys } => {
                    let Some(scale) = at(keys, progress) else {
                        continue;
                    };
                    let (ox, oy) = (origin.0 * size.0, origin.1 * size.1);
                    motion = Transform2D::translate(-ox, -oy)
                        .then(Transform2D::scale(scale, scale))
                        .then(Transform2D::translate(ox, oy))
                        .then(motion);
                }
                Lane::Shadow(keys) => paint.gpui_shadows.extend(at(keys, progress)),
                Lane::Background(keys) => {
                    paint.gpui_background = at(keys, progress).or(paint.gpui_background);
                }
                Lane::Blur(keys) => paint.content_blur = at(keys, progress).or(paint.content_blur),
            }
        }
        if motion != Transform2D::IDENTITY {
            paint.transform = Some(match paint.transform {
                Some(rest) => motion.then(rest),
                None => motion,
            });
        }
        paint
    }
}

pub trait Blend: Copy {
    #[must_use]
    fn blend(self, to: Self, t: f32) -> Self;
}

impl Blend for f32 {
    fn blend(self, to: Self, t: f32) -> Self {
        (to - self).mul_add(t, self)
    }
}

impl Blend for (f32, f32) {
    fn blend(self, to: Self, t: f32) -> Self {
        (self.0.blend(to.0, t), self.1.blend(to.1, t))
    }
}

impl Blend for GpuiHsla {
    fn blend(self, to: Self, t: f32) -> Self {
        let turn = (to.h - self.h + 0.5).rem_euclid(1.0) - 0.5;
        let (from_h, to_h) = match (self.s == 0.0, to.s == 0.0) {
            (true, false) => (to.h, to.h),
            (false, true) => (self.h, self.h),
            _ => (self.h, self.h + turn),
        };
        Self {
            h: from_h.blend(to_h, t).rem_euclid(1.0),
            s: self.s.blend(to.s, t),
            l: self.l.blend(to.l, t),
            a: self.a.blend(to.a, t),
        }
    }
}

impl Blend for GpuiShadowLayer {
    fn blend(self, to: Self, t: f32) -> Self {
        Self {
            dx: self.dx.blend(to.dx, t),
            dy: self.dy.blend(to.dy, t),
            blur: self.blur.blend(to.blur, t),
            spread: self.spread.blend(to.spread, t),
            inset: if t < 0.5 { self.inset } else { to.inset },
            color: self.color.blend(to.color, t),
        }
    }
}

impl Blend for GpuiGradientStop {
    fn blend(self, to: Self, t: f32) -> Self {
        Self {
            percentage: self.percentage.blend(to.percentage, t),
            color: self.color.blend(to.color, t),
        }
    }
}

impl Blend for GpuiBackground {
    fn blend(self, to: Self, t: f32) -> Self {
        match (self, to) {
            (Self::Solid(from), Self::Solid(to)) => Self::Solid(from.blend(to, t)),
            (
                Self::LinearGradient {
                    angle: from_angle,
                    color_space,
                    stops: [from_first, from_second],
                },
                Self::LinearGradient {
                    angle: to_angle,
                    color_space: to_space,
                    stops: [to_first, to_second],
                },
            ) if color_space == to_space => Self::LinearGradient {
                angle: from_angle.blend(to_angle, t),
                color_space,
                stops: [
                    from_first.blend(to_first, t),
                    from_second.blend(to_second, t),
                ],
            },
            (from, to) => {
                if t < 0.5 {
                    from
                } else {
                    to
                }
            }
        }
    }
}

fn at<T: Blend>(keys: &[Key<T>], progress: f32) -> Option<T> {
    let next = keys.iter().position(|key| key.at > progress);
    match next {
        Some(0) => Some(keys[0].value),
        Some(index) => {
            let (from, to) = (&keys[index - 1], &keys[index]);
            let span = to.at - from.at;
            let t = if span > 0.0 {
                (progress - from.at) / span
            } else {
                1.0
            };
            Some(
                from.value
                    .blend(to.value, to.easing.sample(t.clamp(0.0, 1.0))),
            )
        }
        None => keys.last().map(|key| key.value),
    }
}
