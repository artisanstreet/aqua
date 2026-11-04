use aqua_core::Oklch;
use std::f32::consts::TAU;

pub type Seconds = f64;

pub const SPRING_MAX_DURATION_SECS: Seconds = 8.0;

const SETTLE_POS_EPS: f32 = 1e-3;
const SETTLE_VEL_EPS: f32 = 5e-2;
const CRITICAL_EPS: f64 = 1e-6;
const MIN_SPRING_DURATION_SECS: f32 = 0.01;
const MAX_BOUNCE: f32 = 0.95;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReducedMotion {
    Full,
    Reduce,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Easing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

impl Easing {
    #[must_use]
    pub fn sample(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseIn => t * t * t,
            Easing::EaseOut => 1.0 - (1.0 - t).powi(3),
            Easing::EaseInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringParams {
    pub stiffness: f32,
    pub damping: f32,
}

impl SpringParams {
    pub const SMOOTH: SpringParams = SpringParams {
        stiffness: 170.0,
        damping: 26.0,
    };
    pub const SNAPPY: SpringParams = SpringParams {
        stiffness: 320.0,
        damping: 30.0,
    };

    #[must_use]
    pub const fn new(stiffness: f32, damping: f32) -> Self {
        Self { stiffness, damping }
    }

    #[must_use]
    pub fn from_duration_bounce(duration_secs: f32, bounce: f32) -> Self {
        let duration = duration_secs.max(MIN_SPRING_DURATION_SECS);
        let bounce = match bounce.is_nan() {
            true => 0.0,
            false => bounce.clamp(0.0, MAX_BOUNCE),
        };
        let omega = TAU / duration;
        let zeta = 1.0 - bounce;
        Self {
            stiffness: omega * omega,
            damping: 2.0 * zeta * omega,
        }
    }

    pub(crate) fn clamped(self) -> (f32, f32) {
        (
            self.stiffness.clamp(1.0, 5000.0),
            self.damping.clamp(0.0, 200.0),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SpringRegime {
    Underdamped { decay: f64, omega_d: f64 },
    Critical { omega: f64 },
    Overdamped { slow: f64, fast: f64 },
}

impl SpringRegime {
    pub(crate) fn of(params: SpringParams) -> Self {
        let (stiffness, damping) = params.clamped();
        let omega = f64::from(stiffness).sqrt();
        let zeta = f64::from(damping) / (2.0 * omega);
        match zeta {
            z if (z - 1.0).abs() < CRITICAL_EPS => SpringRegime::Critical { omega },
            z if z < 1.0 => SpringRegime::Underdamped {
                decay: z * omega,
                omega_d: omega * (1.0 - z * z).sqrt(),
            },
            z => {
                let fast = -omega * (z + (z * z - 1.0).sqrt());
                SpringRegime::Overdamped {
                    slow: omega * omega / fast,
                    fast,
                }
            }
        }
    }

    pub(crate) fn displacement(self, a: f64, v0: f64, t: f64) -> (f64, f64) {
        match self {
            SpringRegime::Underdamped { decay, omega_d } => {
                let b = (v0 + decay * a) / omega_d;
                let envelope = (-decay * t).exp();
                let (sin, cos) = (omega_d * t).sin_cos();
                (
                    envelope * (a * cos + b * sin),
                    envelope * (v0 * cos - (decay * b + omega_d * a) * sin),
                )
            }
            SpringRegime::Critical { omega } => {
                let b = v0 + omega * a;
                let envelope = (-omega * t).exp();
                (envelope * (a + b * t), envelope * (v0 - omega * b * t))
            }
            SpringRegime::Overdamped { slow, fast } => {
                let c_fast = (v0 - slow * a) / (fast - slow);
                let c_slow = a - c_fast;
                let e_slow = (slow * t).exp();
                let e_fast = (fast * t).exp();
                (
                    c_slow * e_slow + c_fast * e_fast,
                    slow * c_slow * e_slow + fast * c_fast * e_fast,
                )
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringSegment {
    pub t0: Seconds,
    pub x0: f32,
    pub v0: f32,
    pub target: f32,
}

impl SpringSegment {
    #[must_use]
    pub const fn at_rest(t0: Seconds, x: f32) -> Self {
        Self {
            t0,
            x0: x,
            v0: 0.0,
            target: x,
        }
    }

    #[must_use]
    pub fn sample(&self, params: SpringParams, now: Seconds) -> (f32, f32) {
        self.sample_in(SpringRegime::of(params), now)
    }

    #[must_use]
    pub fn retarget(&self, params: SpringParams, now: Seconds, target: f32) -> Self {
        let (x0, v0) = self.sample(params, now);
        Self {
            t0: now,
            x0,
            v0,
            target,
        }
    }

    pub(crate) fn sample_in(&self, regime: SpringRegime, now: Seconds) -> (f32, f32) {
        let target = f64::from(self.target);
        let (dx, v) = regime.displacement(
            f64::from(self.x0) - target,
            f64::from(self.v0),
            (now - self.t0).max(0.0),
        );
        ((target + dx) as f32, v as f32)
    }

    pub(crate) fn is_settled_at(x: f32, v: f32, target: f32) -> bool {
        (x - target).abs() <= SETTLE_POS_EPS && v.abs() <= SETTLE_VEL_EPS
    }
}

pub(crate) fn rest_segments(now: Seconds, channels: [f32; 4]) -> [SpringSegment; 4] {
    channels.map(|x| SpringSegment::at_rest(now, x))
}

pub(crate) fn launch_segments(
    now: Seconds,
    positions: [f32; 4],
    velocities: [f32; 4],
    targets: [f32; 4],
) -> [SpringSegment; 4] {
    std::array::from_fn(|i| SpringSegment {
        t0: now,
        x0: positions[i],
        v0: velocities[i],
        target: targets[i],
    })
}

pub(crate) fn shortest_turn(from: f32, to: f32) -> f32 {
    let delta = (to - from).rem_euclid(1.0);
    match delta > 0.5 {
        true => delta - 1.0,
        false => delta,
    }
}

pub(crate) fn spring_targets(basis: &[f32; 4], to: &MotionValue) -> [f32; 4] {
    let target = to.channels();
    match to {
        MotionValue::Color(_) => [
            target[0],
            target[1],
            basis[2] + shortest_turn(basis[2], target[2]),
            target[3],
        ],
        _ => target,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MotionKind {
    Tween {
        duration_secs: Seconds,
        easing: Easing,
    },
    Spring(SpringParams),
    Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completion {
    Completed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MotionValue {
    Scalar(f32),
    Pair(f32, f32),
    Color(Oklch),
}

impl MotionValue {
    #[must_use]
    pub fn blend(from: &MotionValue, to: &MotionValue, t: f32) -> Option<MotionValue> {
        let t = t.clamp(0.0, 1.0);
        match (from, to) {
            (MotionValue::Scalar(a), MotionValue::Scalar(b)) => {
                Some(MotionValue::Scalar(a + (b - a) * t))
            }
            (MotionValue::Pair(ax, ay), MotionValue::Pair(bx, by)) => {
                Some(MotionValue::Pair(ax + (bx - ax) * t, ay + (by - ay) * t))
            }
            (MotionValue::Color(a), MotionValue::Color(b)) => {
                Some(MotionValue::Color(a.lerp(*b, t)))
            }
            _ => None,
        }
    }

    #[must_use]
    pub fn offset_by(&self, delta: &MotionValue) -> Option<MotionValue> {
        match (self, delta) {
            (MotionValue::Scalar(a), MotionValue::Scalar(b)) => Some(MotionValue::Scalar(a + b)),
            (MotionValue::Pair(ax, ay), MotionValue::Pair(bx, by)) => {
                Some(MotionValue::Pair(ax + bx, ay + by))
            }
            _ => None,
        }
    }

    pub(crate) fn channel_count(&self) -> usize {
        match self {
            MotionValue::Scalar(_) => 1,
            MotionValue::Pair(_, _) => 2,
            MotionValue::Color(_) => 4,
        }
    }

    pub(crate) fn channels(&self) -> [f32; 4] {
        match *self {
            MotionValue::Scalar(v) => [v, 0.0, 0.0, 0.0],
            MotionValue::Pair(x, y) => [x, y, 0.0, 0.0],
            MotionValue::Color(c) => [c.l, c.c, c.h / 360.0, c.alpha],
        }
    }

    pub(crate) fn from_channels(model: &MotionValue, channels: &[f32; 4]) -> MotionValue {
        match *model {
            MotionValue::Scalar(_) => MotionValue::Scalar(channels[0]),
            MotionValue::Pair(_, _) => MotionValue::Pair(channels[0], channels[1]),
            MotionValue::Color(_) => MotionValue::Color(Oklch::oklch(
                channels[0],
                channels[1].max(0.0),
                channels[2] * 360.0,
                channels[3],
            )),
        }
    }

    pub(crate) fn variant_eq(a: &MotionValue, b: &MotionValue) -> bool {
        std::mem::discriminant(a) == std::mem::discriminant(b)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionError {
    UnknownHandle,
    VariantMismatch,
}

pub(crate) fn resolve_kind(kind: MotionKind, reduced: ReducedMotion) -> MotionKind {
    match reduced {
        ReducedMotion::Full => kind,
        ReducedMotion::Reduce => MotionKind::Instant,
    }
}

impl SpringRegime {
    pub(crate) fn settle_bound(self, a: f64, v0: f64) -> f64 {
        let (rate, position, velocity) = match self {
            SpringRegime::Underdamped { decay, omega_d } => {
                let b = (v0 + decay * a) / omega_d;
                (
                    decay,
                    a.abs() + b.abs(),
                    v0.abs() + decay * b.abs() + omega_d * a.abs(),
                )
            }
            SpringRegime::Critical { omega } => {
                let b = v0 + omega * a;
                (
                    omega / 2.0,
                    a.abs() + 2.0 * b.abs() / omega,
                    v0.abs() + 2.0 * b.abs(),
                )
            }
            SpringRegime::Overdamped { slow, fast } => {
                let c_fast = (v0 - slow * a) / (fast - slow);
                let c_slow = a - c_fast;
                (
                    slow.abs(),
                    c_slow.abs() + c_fast.abs(),
                    (slow * c_slow).abs() + (fast * c_fast).abs(),
                )
            }
        };
        let ratio = (position / f64::from(SETTLE_POS_EPS))
            .max(velocity / f64::from(SETTLE_VEL_EPS))
            .max(1.0);
        match rate > 0.0 {
            true => (ratio.ln() / rate).min(SPRING_MAX_DURATION_SECS),
            false => SPRING_MAX_DURATION_SECS,
        }
    }
}
