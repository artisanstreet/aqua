//! The one Aqua motion model: explicit targets, property ownership, playback
//! state, reduced-motion policy and scheduler deadlines.
//!
//! There is exactly one representation for animated presentation state:
//! [`Animated`] (a single value with one target) owned by a
//! [`MotionController]` (scheduler + ownership + disposal). No separate
//! systems for transitions, keyframes or springs: tweens, springs and snaps
//! are [`MotionKind`] variants sampled through the same elapsed-time clock.
//!
//! Rules:
//! - Time is caller-supplied ([`Seconds`], `f64`). Nothing reads a real clock, so
//!   every animation is deterministic under a test driver and the frame
//!   scheduler stays in charge of wakeups. Sub-millisecond tween times work:
//!   use the `*_duration` conveniences to pass [`Duration`] directly.
//! - Retargeting samples the currently displayed value first: reversing a
//!   panel mid-motion never jumps, and springs keep their velocity.
//! - Competing writes to one property go through one [`Animated`]: the newest
//!   target wins, the superseded completion is dropped silently, and a stale
//!   completion can never remove a reopened component. Explicit
//!   [`Animated::cancel`] reports [`Completion::Cancelled`]; owner disposal
//!   drops callbacks silently because the owner is gone.
//! - Colors animate in canonical Oklch (`aqua_core::Oklch`): tweens reuse
//!   [`Oklch::lerp`](aqua_core::Oklch::lerp), so hue follows the shortest
//!   path and near-neutral endpoints inherit the chromatic hue instead of
//!   swinging through arbitrary hues. Springs integrate per channel with the
//!   hue unwrapped to the shortest path at retarget time.
//! - Reduced motion collapses in-flight and future motion to [`MotionKind::Instant`]
//!   (snap to target). Opacity-vs-movement distinctions, if a product needs
//!   them, are separate [`Animated`] values with their own kinds — not a
//!   second motion system.
//!
//! What affects layout vs composition is NOT decided here: the tree owns
//! layout, motion only produces presentation values. Sampling an [`Animated`]
//! never writes the tree; the owner commits sampled values during its update.

use aqua_core::id::RawId;
use aqua_core::{Arena, NodeId, Oklch};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Time, policy, shaping
// ---------------------------------------------------------------------------

/// Seconds on an arbitrary caller-supplied timeline. Passed in, never read
/// from a clock, so tests and the scheduler drive identical code paths.
/// `f64` (not integer milliseconds): high-frame-rate animation needs
/// sub-millisecond precision (1 ms already quantizes a 900 Hz update), and
/// tween durations below a millisecond must stay representable. Callers on
/// [`Duration`] timelines use the `*_duration` conveniences, which convert
/// losslessly via [`Duration::as_secs_f64`]. Non-finite inputs collapse to
/// the last sampled time (monotonic); callers must pass finite values.
pub type Seconds = f64;

/// Maximum simulated span for an unsettled spring before it is snapped to
/// its target (prevents infinite frames on oscillation / rounding).
pub const SPRING_MAX_DURATION_SECS: Seconds = 8.0;

/// Internal simulation quantum for springs. Substeps are fixed for
/// stability, but the trailing remainder is integrated as an exact
/// fractional substep, so the final sample always lands on the exact
/// elapsed time the caller passed.
const SPRING_STEP_SECS: f32 = 0.001;

/// Position/velocity epsilon for spring settling, in channel units. Color hue
/// travels in turns (h / 360) so one epsilon serves every channel.
const SETTLE_POS_EPS: f32 = 1e-3;
const SETTLE_VEL_EPS: f32 = 5e-2;

/// Reduced-motion preference. Collapses motion to snaps; applied live,
/// including to in-flight animations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReducedMotion {
    /// Full motion.
    Full,
    /// Snap to targets instead of animating.
    Reduce,
}

/// Easing for tween sampling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Easing {
    /// Constant velocity.
    Linear,
    /// Cubic accelerate.
    EaseIn,
    /// Cubic decelerate.
    EaseOut,
    /// Cubic accelerate then decelerate.
    EaseInOut,
}

impl Easing {
    /// Shape a unit progress value into a unit eased value.
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

/// Spring response parameters. Units are per-second: `stiffness` in 1/s²,
/// `damping` in 1/s (semi-implicit Euler at 1 ms substeps).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringParams {
    /// Restoring stiffness (1/s²), clamped to `[1, 5000]` at use.
    pub stiffness: f32,
    /// Velocity damping (1/s), clamped to `[0, 200]` at use.
    pub damping: f32,
}

impl SpringParams {
    /// Smooth UI spring: settles in roughly a second without overshoot drama.
    pub const SMOOTH: SpringParams = SpringParams {
        stiffness: 170.0,
        damping: 26.0,
    };
    /// Snappy UI spring: faster, slight overshoot possible.
    pub const SNAPPY: SpringParams = SpringParams {
        stiffness: 320.0,
        damping: 30.0,
    };

    /// Custom parameters (validated/clamped when the spring is sampled).
    #[must_use]
    pub const fn new(stiffness: f32, damping: f32) -> Self {
        Self { stiffness, damping }
    }

    fn clamped(self) -> (f32, f32) {
        (
            self.stiffness.clamp(1.0, 5000.0),
            self.damping.clamp(0.0, 200.0),
        )
    }
}

/// How an [`Animated`] value moves toward its target. One enum, one code
/// path — not competing motion systems.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MotionKind {
    /// Fixed-duration interpolation with easing.
    Tween {
        /// Duration in seconds; `<= 0.0` completes on the next sample.
        duration_secs: Seconds,
        /// Shaping.
        easing: Easing,
    },
    /// Physical response with velocity preservation across retargets.
    Spring(SpringParams),
    /// Reach the target on the next sample (also the reduced-motion form).
    Instant,
}

/// Terminal state reported to completion callbacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completion {
    /// The registered target was reached (or jumped to via `finish`).
    Completed,
    /// Explicit [`Animated::cancel`]. Retargets and owner disposal do NOT
    /// report: the old target was neither reached nor deliberately cancelled.
    Cancelled,
}

/// Values the motion model can animate. One closed vocabulary so targets,
/// interpolation and ownership stay in one controller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MotionValue {
    /// Opacity, offset component, scale, progress...
    Scalar(f32),
    /// 2D offset / scale / size delta.
    Pair(f32, f32),
    /// Canonical Oklch color + alpha.
    Color(Oklch),
}

impl MotionValue {
    /// Blend between same-variant values (`None` on variant mismatch; the
    /// caller keeps its state and reports [`MotionError::VariantMismatch`]).
    /// Colors delegate to [`Oklch::lerp`](aqua_core::Oklch::lerp): shortest
    /// hue path, neutral endpoints inherit the chromatic hue.
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

    /// Channel decomposition for spring integration: `[l, c, hue-turns,
    /// alpha]` for colors (hue in turns keeps one settle epsilon honest),
    /// raw components otherwise.
    fn channels(&self) -> Vec<f32> {
        match *self {
            MotionValue::Scalar(v) => vec![v],
            MotionValue::Pair(x, y) => vec![x, y],
            MotionValue::Color(c) => vec![c.l, c.c, c.h / 360.0, c.alpha],
        }
    }

    /// Rebuild from channels of the same variant as `model`.
    fn from_channels(model: &MotionValue, channels: &[f32]) -> MotionValue {
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

    fn variant_eq(a: &MotionValue, b: &MotionValue) -> bool {
        std::mem::discriminant(a) == std::mem::discriminant(b)
    }
}

// ---------------------------------------------------------------------------
// Animated: one animated property
// ---------------------------------------------------------------------------

/// Error for fallible motion operations. Never panics on stale handles or
/// mismatched value kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionError {
    /// Handle was never issued or its slot was removed/reused.
    UnknownHandle,
    /// `set_target` with a different [`MotionValue`] variant than the
    /// property holds. State is unchanged.
    VariantMismatch,
}

/// One animated property: a current presentation value moving toward an
/// explicit target. Retargets sample the displayed value first (no jumps);
/// springs additionally preserve velocity.
pub struct Animated {
    from: MotionValue,
    to: MotionValue,
    kind: MotionKind,
    reduced: ReducedMotion,
    start_secs: Seconds,
    last_secs: Seconds,
    end_secs: Seconds,
    spring_pos: Vec<f32>,
    spring_vel: Vec<f32>,
    /// Spring integration target in the same (possibly hue-unwrapped)
    /// channel basis as `spring_pos`. `to` stays canonical for snaps.
    spring_target: Vec<f32>,
    current: MotionValue,
    settled: bool,
    completion: Option<PendingCompletion>,
}

/// A registered completion callback plus the target it belongs to.
/// `target: None` means "registered before the next target was set" — the
/// next [`set_target`](Animated::set_target) binds it to that target, so the
/// universal setup pattern (`on_complete` then `set_target`) observes the
/// intended motion. A callback bound to a superseded target is dropped
/// silently and can never fire for a target it did not watch.
struct PendingCompletion {
    target: Option<MotionValue>,
    callback: Box<dyn FnMut(Completion)>,
}

impl Animated {
    /// Create a settled property holding `initial`. Motion starts with
    /// [`set_target`](Animated::set_target). `now` anchors the clock.
    pub fn new(
        initial: MotionValue,
        kind: MotionKind,
        reduced: ReducedMotion,
        now: Seconds,
    ) -> Self {
        let kind = resolve_kind(kind, reduced);
        let channels = initial.channels();
        Self {
            from: initial,
            to: initial,
            kind,
            reduced,
            start_secs: now,
            last_secs: now,
            end_secs: now,
            spring_vel: vec![0.0; channels.len()],
            spring_target: channels.clone(),
            spring_pos: channels,
            current: initial,
            settled: true,
            completion: None,
        }
    }

    /// Register (replacing) the completion callback. Fires exactly once:
    /// [`Completion::Completed`] when the target it ends up bound to is
    /// reached, [`Completion::Cancelled`] on explicit
    /// [`cancel`](Animated::cancel). Registration binds to the next target
    /// set (or the current one when already in flight); retargeting a bound
    /// callback drops it silently, so a stale completion can never remove a
    /// reopened component. Replacing drops the previous callback silently.
    pub fn on_complete(&mut self, callback: Box<dyn FnMut(Completion)>) {
        self.completion = Some(PendingCompletion {
            target: None,
            callback,
        });
    }

    /// Last sampled presentation value (no clock read).
    #[must_use]
    pub fn value(&self) -> MotionValue {
        self.current
    }

    /// Current target (application state; presentation is [`value`](Animated::value)).
    #[must_use]
    pub fn target(&self) -> MotionValue {
        self.to
    }

    /// Effective motion kind (post reduced-motion resolution).
    #[must_use]
    pub fn kind(&self) -> MotionKind {
        self.kind
    }

    /// Whether the presentation value rests at the target.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.settled
    }

    /// Move the target. Samples the displayed value at `now` first, so
    /// mid-motion reversals continue from the visible position; spring
    /// velocity is preserved. Completion binding: a callback registered
    /// since the last `set_target` binds to this target; a callback already
    /// bound to the previous target fires [`Completion::Completed`] if that
    /// target genuinely landed at `now`, and is otherwise dropped silently
    /// (superseded — it must never fire for a target it did not watch).
    pub fn set_target(&mut self, to: MotionValue, now: Seconds) -> Result<(), MotionError> {
        if !MotionValue::variant_eq(&self.current, &to) {
            return Err(MotionError::VariantMismatch);
        }
        let was_settled = self.settled;
        let reached = self.progress(now);
        if let Some(mut pending) = self.completion.take() {
            if pending.target.is_none() {
                // Registered for the upcoming target: bind it here.
                pending.target = Some(to);
                self.completion = Some(pending);
            } else if reached && !was_settled {
                // Bound to the previous target and it genuinely landed.
                (pending.callback)(Completion::Completed);
            }
            // Else: bound to a superseded target — drop silently.
        }
        let now = now.max(self.last_secs);
        self.from = self.current;
        self.to = to;
        self.kind = resolve_kind(self.kind, self.reduced);
        self.start_secs = now;
        self.last_secs = now;
        self.settled = false;
        match self.kind {
            MotionKind::Tween { duration_secs, .. } => {
                self.end_secs = now + duration_secs.max(0.0);
            }
            MotionKind::Spring(_) => {
                self.end_secs = now + SPRING_MAX_DURATION_SECS;
                // Restart the position basis at the displayed value (velocity
                // preserved), unwrap the target hue to the shortest path
                // around it so springs never spin the long way around.
                let basis = self.current.channels();
                let mut target = to.channels();
                if matches!(to, MotionValue::Color(_)) {
                    let delta = (target[2] - basis[2]).rem_euclid(1.0);
                    let shortest = if delta > 0.5 { delta - 1.0 } else { delta };
                    target[2] = basis[2] + shortest;
                }
                self.spring_pos = basis;
                self.spring_target = target;
            }
            MotionKind::Instant => {
                self.end_secs = now;
            }
        }
        Ok(())
    }

    /// Change the reduced-motion policy live. Collapses in-flight motion to
    /// a snap: the next [`sample`](Animated::sample) reaches the target.
    pub fn set_reduced_motion(&mut self, reduced: ReducedMotion, now: Seconds) {
        self.reduced = reduced;
        if self.progress(now) {
            self.settle_and_fire(now, Completion::Completed);
            return;
        }
        if !self.settled && reduced == ReducedMotion::Reduce {
            self.from = self.current;
            self.kind = MotionKind::Instant;
            self.end_secs = now.max(self.last_secs);
        }
    }

    /// Sample the presentation value at `now` (elapsed seconds, caller
    /// clock). Fires a pending completion at most once, on the transition to
    /// settled. Monotonic: earlier timestamps are treated as `last_secs`;
    /// non-finite timestamps collapse to `last_secs`.
    pub fn sample(&mut self, now: Seconds) -> MotionValue {
        if self.settled {
            // A callback registered after settling (or carried by a fresh
            // Instant target) still gets its single delivery.
            if let Some(mut pending) = self.completion.take() {
                (pending.callback)(Completion::Completed);
            }
            return self.current;
        }
        if self.progress(now) {
            self.settle_and_fire(now, Completion::Completed);
        }
        self.current
    }

    /// Freeze at the last sampled value and report
    /// [`Completion::Cancelled`]. Use [`finish`](Animated::finish) to jump to
    /// the target instead.
    pub fn cancel(&mut self) {
        if self.settled {
            return;
        }
        self.settled = true;
        if let Some(mut pending) = self.completion.take() {
            (pending.callback)(Completion::Cancelled);
        }
    }

    /// Snap to the target immediately and report [`Completion::Completed`]
    /// (programmatic skip, e.g. teardown fast-forward).
    pub fn finish(&mut self) {
        if self.settled {
            return;
        }
        self.current = self.to;
        self.last_secs = self.last_secs.max(self.start_secs);
        self.settled = true;
        if let Some(mut pending) = self.completion.take() {
            (pending.callback)(Completion::Completed);
        }
    }

    /// When the scheduler must wake for this property: `None` once settled,
    /// otherwise `Some(now)` — the property needs a frame as soon as
    /// possible (continuous motion has no meaningful future deadline until
    /// it settles; tween end times are observable via
    /// [`tween_end`](Animated::tween_end)).
    #[must_use]
    pub fn next_deadline(&self, now: Seconds) -> Option<Seconds> {
        if self.settled {
            None
        } else {
            Some(now.max(self.last_secs))
        }
    }

    /// Tween end time, if this property is an unsettled tween.
    #[must_use]
    pub fn tween_end(&self) -> Option<Seconds> {
        match self.kind {
            MotionKind::Tween { .. } if !self.settled => Some(self.end_secs),
            _ => None,
        }
    }

    /// [`sample`](Animated::sample) with a [`Duration`] clock.
    /// Sub-millisecond durations convert losslessly via
    /// [`Duration::as_secs_f64`] — tween times below 1 ms stay exact.
    pub fn sample_duration(&mut self, now: Duration) -> MotionValue {
        self.sample(now.as_secs_f64())
    }

    /// [`set_target`](Animated::set_target) with a [`Duration`] clock.
    pub fn set_target_duration(
        &mut self,
        to: MotionValue,
        now: Duration,
    ) -> Result<(), MotionError> {
        self.set_target(to, now.as_secs_f64())
    }

    /// Advance state to `now`; returns true when the target is reached.
    /// Pure state update — completion delivery stays in `sample` /
    /// `set_target` / `set_reduced_motion` so each path fires exactly once.
    fn progress(&mut self, now: Seconds) -> bool {
        if self.settled {
            return true;
        }
        let now = now.max(self.last_secs);
        match self.kind {
            MotionKind::Instant => {
                self.current = self.to;
                self.last_secs = now;
                true
            }
            MotionKind::Tween {
                duration_secs,
                easing,
            } => {
                // Epsilon: f64 addition makes exact end hits (0.05 + 0.1)
                // land an ulp on either side. Snapping a relative 1e-9
                // early is visually nil and keeps completion exact.
                let elapsed = now - self.start_secs;
                if duration_secs <= 0.0
                    || elapsed >= duration_secs * (1.0 - 1e-9)
                    || now >= self.end_secs
                {
                    self.current = self.to;
                    self.last_secs = now;
                    true
                } else {
                    let span = (self.end_secs - self.start_secs).max(f32::EPSILON as Seconds);
                    let t = easing.sample(((now - self.start_secs) / span) as f32);
                    // Same-variant invariant holds: `from`/`to` only change
                    // together in `set_target`, which checks the variant.
                    self.current = MotionValue::blend(&self.from, &self.to, t)
                        .expect("tween endpoints share a variant");
                    self.last_secs = now;
                    false
                }
            }
            MotionKind::Spring(params) => {
                let (stiffness, damping) = params.clamped();
                let mut elapsed = now - self.last_secs;
                if elapsed > SPRING_MAX_DURATION_SECS {
                    elapsed = SPRING_MAX_DURATION_SECS;
                }
                // Fixed 1 ms substeps for stability plus one exact fractional
                // remainder substep, so the sample lands on the exact elapsed
                // time (no 1 ms quantization of the presentation value).
                let h = SPRING_STEP_SECS;
                let full_steps = (elapsed as f32 / h).floor() as usize;
                let remainder = elapsed as f32 - full_steps as f32 * h;
                let target = self.spring_target.clone();
                // Guard: cap on substeps keeps pathological clock jumps bounded.
                let full_steps = full_steps.min(9000);
                for _ in 0..full_steps {
                    integrate_step(
                        &mut self.spring_pos,
                        &mut self.spring_vel,
                        &target,
                        stiffness,
                        damping,
                        h,
                    );
                }
                if remainder > 0.0 {
                    integrate_step(
                        &mut self.spring_pos,
                        &mut self.spring_vel,
                        &target,
                        stiffness,
                        damping,
                        remainder,
                    );
                }
                self.last_secs = now;
                self.current = MotionValue::from_channels(&self.to, &self.spring_pos);
                let reached_timeout = now - self.start_secs >= SPRING_MAX_DURATION_SECS;
                if reached_timeout
                    || Self::channels_settled(&self.spring_pos, &target, &self.spring_vel)
                {
                    self.current = self.to;
                    true
                } else {
                    false
                }
            }
        }
    }

    fn channels_settled(pos: &[f32], target: &[f32], vel: &[f32]) -> bool {
        pos.iter()
            .zip(target)
            .zip(vel)
            .all(|((p, t), v)| (p - t).abs() <= SETTLE_POS_EPS && v.abs() <= SETTLE_VEL_EPS)
    }

    fn settle_and_fire(&mut self, now: Seconds, completion: Completion) {
        self.current = match completion {
            Completion::Completed => self.to,
            Completion::Cancelled => self.current,
        };
        self.last_secs = now.max(self.last_secs);
        self.settled = true;
        if let Some(mut pending) = self.completion.take() {
            (pending.callback)(completion);
        }
    }
}

/// One semi-implicit Euler substep for every spring channel.
fn integrate_step(
    pos: &mut [f32],
    vel: &mut [f32],
    target: &[f32],
    stiffness: f32,
    damping: f32,
    h: f32,
) {
    for ((x, v), t) in pos.iter_mut().zip(vel.iter_mut()).zip(target.iter()) {
        let accel = stiffness * (*t - *x) - damping * *v;
        *v += accel * h;
        *x += *v * h;
    }
}

fn resolve_kind(kind: MotionKind, reduced: ReducedMotion) -> MotionKind {
    match reduced {
        ReducedMotion::Full => kind,
        ReducedMotion::Reduce => match kind {
            MotionKind::Instant => kind,
            _ => MotionKind::Instant,
        },
    }
}

// ---------------------------------------------------------------------------
// Controller: ownership + scheduling over many properties
// ---------------------------------------------------------------------------

/// Stable handle to one animated property in a [`MotionController`].
/// Generational: removing a property never aliases a later one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MotionHandle(RawId);

impl MotionHandle {
    /// Rebuild from raw parts. Resolves only if issued and still live.
    #[must_use]
    pub const fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    /// Raw parts.
    #[must_use]
    pub const fn raw(self) -> RawId {
        self.0
    }
}

struct Slot {
    owner: Option<NodeId>,
    anim: Animated,
}

/// Owns animated properties: targets, scheduler deadlines, per-owner
/// disposal. One controller per window (or per test driver); all motion in
/// it shares the caller's elapsed-time clock.
pub struct MotionController {
    arena: Arena<Slot>,
    reduced: ReducedMotion,
}

impl Default for MotionController {
    /// Idle controller under full motion.
    fn default() -> Self {
        Self::new(ReducedMotion::Full)
    }
}

impl MotionController {
    /// Create an idle controller under the given reduced-motion policy.
    #[must_use]
    pub fn new(reduced: ReducedMotion) -> Self {
        Self {
            arena: Arena::new(),
            reduced,
        }
    }

    /// Current reduced-motion policy.
    #[must_use]
    pub fn reduced_motion(&self) -> ReducedMotion {
        self.reduced
    }

    /// Change the policy live: every in-flight property collapses to a snap
    /// at `now` (see [`Animated::set_reduced_motion`]).
    pub fn set_reduced_motion(&mut self, reduced: ReducedMotion, now: Seconds) {
        self.reduced = reduced;
        for (_, slot) in self.arena.iter_mut() {
            slot.anim.set_reduced_motion(reduced, now);
        }
    }

    /// Number of live properties.
    #[must_use]
    pub fn len(&self) -> usize {
        self.arena.len()
    }

    /// Whether no property is live.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.arena.is_empty()
    }

    /// Spawn a settled property holding `initial` for `owner` (`None` =
    /// unowned / controller-lifetime). Motion begins with `set_target`.
    pub fn spawn(
        &mut self,
        owner: Option<NodeId>,
        initial: MotionValue,
        kind: MotionKind,
        now: Seconds,
    ) -> MotionHandle {
        let raw = self.arena.insert(Slot {
            owner,
            anim: Animated::new(initial, kind, self.reduced, now),
        });
        MotionHandle(raw)
    }

    /// Retarget a property. Unknown handles and variant mismatches report
    /// [`MotionError`] and change nothing.
    pub fn set_target(
        &mut self,
        handle: MotionHandle,
        to: MotionValue,
        now: Seconds,
    ) -> Result<(), MotionError> {
        self.arena
            .get_mut(handle.0)
            .ok_or(MotionError::UnknownHandle)?
            .anim
            .set_target(to, now)
    }

    /// Sample a property's presentation value. `None` for unknown handles.
    pub fn sample(&mut self, handle: MotionHandle, now: Seconds) -> Option<MotionValue> {
        self.arena
            .get_mut(handle.0)
            .map(|slot| slot.anim.sample(now))
    }

    /// Last sampled value without advancing. `None` for unknown handles.
    #[must_use]
    pub fn peek(&self, handle: MotionHandle) -> Option<MotionValue> {
        self.arena.get(handle.0).map(|slot| slot.anim.value())
    }

    /// Register a completion callback. `false` for unknown handles.
    pub fn on_complete(
        &mut self,
        handle: MotionHandle,
        callback: Box<dyn FnMut(Completion)>,
    ) -> bool {
        match self.arena.get_mut(handle.0) {
            Some(slot) => {
                slot.anim.on_complete(callback);
                true
            }
            None => false,
        }
    }

    /// Freeze a property and report [`Completion::Cancelled`]. `false` for
    /// unknown handles.
    pub fn cancel(&mut self, handle: MotionHandle) -> bool {
        match self.arena.get_mut(handle.0) {
            Some(slot) => {
                slot.anim.cancel();
                true
            }
            None => false,
        }
    }

    /// Snap a property to its target with [`Completion::Completed`]. `false`
    /// for unknown handles.
    pub fn finish(&mut self, handle: MotionHandle) -> bool {
        match self.arena.get_mut(handle.0) {
            Some(slot) => {
                slot.anim.finish();
                true
            }
            None => false,
        }
    }

    /// Whether a property rests at its target. Unknown handles read as
    /// settled (nothing needs a frame for them).
    #[must_use]
    pub fn is_settled(&self, handle: MotionHandle) -> bool {
        self.arena
            .get(handle.0)
            .is_none_or(|slot| slot.anim.is_settled())
    }

    /// Logical owner of a property, if the handle is live.
    #[must_use]
    pub fn owner_of(&self, handle: MotionHandle) -> Option<Option<NodeId>> {
        self.arena.get(handle.0).map(|slot| slot.owner)
    }

    /// Earliest time any live property needs a frame: the minimum of the
    /// per-property deadlines, `None` when everything is settled. Feed this
    /// to the platform scheduler (`request_animation_deadline`) — demand
    /// driven, no fixed tick.
    #[must_use]
    pub fn next_deadline(&self, now: Seconds) -> Option<Seconds> {
        self.arena
            .iter()
            .filter_map(|(_, slot)| slot.anim.next_deadline(now))
            .reduce(Seconds::min)
    }

    /// Sample every property at `now`; returns the handles that newly
    /// reached their targets during this call (completions also fire via
    /// callbacks; this list serves scheduler-driven polling without them).
    pub fn advance(&mut self, now: Seconds) -> Vec<MotionHandle> {
        // Snapshot unsettled handles first so only genuine transitions are
        // reported even if callbacks run user code during sampling.
        let pending: Vec<RawId> = self
            .arena
            .iter()
            .filter(|(_, slot)| !slot.anim.is_settled())
            .map(|(raw, _)| raw)
            .collect();
        let mut completed = Vec::new();
        for raw in pending {
            if let Some(slot) = self.arena.get_mut(raw) {
                slot.anim.sample(now);
                if slot.anim.is_settled() {
                    completed.push(MotionHandle(raw));
                }
            }
        }
        completed
    }

    /// [`advance`](MotionController::advance) with a [`Duration`] clock.
    pub fn advance_duration(&mut self, now: Duration) -> Vec<MotionHandle> {
        self.advance(now.as_secs_f64())
    }

    /// [`sample`](MotionController::sample) with a [`Duration`] clock.
    pub fn sample_duration(&mut self, handle: MotionHandle, now: Duration) -> Option<MotionValue> {
        self.sample(handle, now.as_secs_f64())
    }

    /// [`set_target`](MotionController::set_target) with a [`Duration`] clock.
    pub fn set_target_duration(
        &mut self,
        handle: MotionHandle,
        to: MotionValue,
        now: Duration,
    ) -> Result<(), MotionError> {
        self.set_target(handle, to, now.as_secs_f64())
    }

    /// [`next_deadline`](MotionController::next_deadline) as a [`Duration`]
    /// for schedulers on a [`Duration`] timeline (e.g. the platform
    /// `Scheduler::request_animation_deadline`). `None` when settled or when
    /// the deadline is not representable (negative/overflow).
    #[must_use]
    pub fn next_deadline_duration(&self, now: Duration) -> Option<Duration> {
        self.next_deadline(now.as_secs_f64())
            .and_then(|secs| Duration::try_from_secs_f64(secs).ok())
    }

    /// Remove a property without firing its callback (lifecycle end:
    /// superseded, not cancelled — use [`cancel`](MotionController::cancel)
    /// first when the app must observe cancellation). `false` for unknown
    /// handles.
    pub fn remove(&mut self, handle: MotionHandle) -> bool {
        self.arena.remove(handle.0).is_ok()
    }

    /// Remove every property owned by `owner`, dropping their callbacks
    /// silently (the owner is gone; nothing may observe them, and a stale
    /// completion must never remove a reopened component). Returns the
    /// removed handles.
    pub fn dispose_owner(&mut self, owner: NodeId) -> Vec<MotionHandle> {
        let owned: Vec<RawId> = self
            .arena
            .iter()
            .filter(|(_, slot)| slot.owner == Some(owner))
            .map(|(raw, _)| raw)
            .collect();
        let mut removed = Vec::with_capacity(owned.len());
        for raw in owned {
            if self.arena.remove(raw).is_ok() {
                removed.push(MotionHandle(raw));
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn node(index: u32) -> NodeId {
        NodeId::from_raw(RawId {
            index,
            generation: 0,
        })
    }

    fn tween(secs: Seconds) -> MotionKind {
        MotionKind::Tween {
            duration_secs: secs,
            easing: Easing::Linear,
        }
    }

    #[allow(clippy::type_complexity)]
    fn recorder() -> (Box<dyn FnMut(Completion)>, Rc<RefCell<Vec<Completion>>>) {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let inner = Rc::clone(&seen);
        let cb: Box<dyn FnMut(Completion)> = Box::new(move |c| inner.borrow_mut().push(c));
        (cb, seen)
    }

    fn scalar(v: f32) -> MotionValue {
        MotionValue::Scalar(v)
    }

    // -- tweens ---------------------------------------------------------------

    #[test]
    fn tween_samples_midpoint_and_settles_at_end() {
        let mut anim = Animated::new(scalar(0.0), tween(1.0), ReducedMotion::Full, 0.0);
        anim.set_target(scalar(100.0), 0.0).unwrap();
        assert_eq!(anim.sample(0.5), scalar(50.0));
        assert!(!anim.is_settled());
        assert_eq!(anim.sample(1.0), scalar(100.0));
        assert!(anim.is_settled());
        // Settled values are stable under further sampling.
        assert_eq!(anim.sample(5.0), scalar(100.0));
    }

    #[test]
    fn tween_end_is_observable_for_scheduling() {
        let mut anim = Animated::new(scalar(0.0), tween(0.16), ReducedMotion::Full, 0.0);
        anim.set_target(scalar(1.0), 0.0).unwrap();
        assert_eq!(anim.tween_end(), Some(0.16));
        assert_eq!(anim.next_deadline(0.0), Some(0.0));
        anim.sample(0.16);
        assert_eq!(anim.tween_end(), None);
        assert_eq!(anim.next_deadline(0.16), None);
    }

    #[test]
    fn easing_shapes_progress_monotonically() {
        let eased = Easing::EaseOut.sample(0.5);
        assert!(eased > 0.5 && eased < 1.0, "{eased}");
        assert_eq!(Easing::EaseInOut.sample(0.5), 0.5);
        assert_eq!(Easing::Linear.sample(0.25), 0.25);
        for easing in [
            Easing::Linear,
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseInOut,
        ] {
            assert_eq!(easing.sample(0.0), 0.0);
            assert_eq!(easing.sample(1.0), 1.0);
        }
    }

    #[test]
    fn retarget_continues_from_displayed_value_without_jump() {
        let mut anim = Animated::new(scalar(0.0), tween(1.0), ReducedMotion::Full, 0.0);
        anim.set_target(scalar(100.0), 0.0).unwrap();
        assert_eq!(anim.sample(0.5), scalar(50.0));
        // Reverse mid-motion: presentation continues from 50, no jump.
        anim.set_target(scalar(0.0), 0.5).unwrap();
        assert_eq!(anim.sample(0.5), scalar(50.0));
        assert_eq!(anim.sample(0.75), scalar(37.5));
        assert_eq!(anim.sample(1.5), scalar(0.0));
        assert!(anim.is_settled());
    }

    #[test]
    fn completion_fires_once_on_reaching_target() {
        let mut anim = Animated::new(scalar(0.0), tween(0.1), ReducedMotion::Full, 0.0);
        let (cb, seen) = recorder();
        anim.on_complete(cb);
        anim.set_target(scalar(10.0), 0.0).unwrap();
        anim.sample(0.05);
        assert!(seen.borrow().is_empty());
        anim.sample(0.1);
        assert_eq!(*seen.borrow(), vec![Completion::Completed]);
        anim.sample(0.2);
        assert_eq!(seen.borrow().len(), 1);
    }

    #[test]
    fn superseded_completion_never_fires_after_replace() {
        let mut anim = Animated::new(scalar(0.0), tween(0.1), ReducedMotion::Full, 0.0);
        let (old_cb, old_seen) = recorder();
        anim.on_complete(old_cb);
        anim.set_target(scalar(10.0), 0.0).unwrap();
        anim.sample(0.05);
        // Replace before the first target lands: the old callback is dropped.
        let (new_cb, new_seen) = recorder();
        anim.on_complete(new_cb);
        anim.set_target(scalar(20.0), 0.05).unwrap();
        anim.sample(0.15);
        assert!(
            old_seen.borrow().is_empty(),
            "stale completion fired: {:?}",
            old_seen.borrow()
        );
        assert_eq!(*new_seen.borrow(), vec![Completion::Completed]);
        assert_eq!(anim.sample(0.15), scalar(20.0));
    }

    #[test]
    fn cancel_reports_cancelled_and_freezes() {
        let mut anim = Animated::new(scalar(0.0), tween(0.1), ReducedMotion::Full, 0.0);
        let (cb, seen) = recorder();
        anim.on_complete(cb);
        anim.set_target(scalar(10.0), 0.0).unwrap();
        anim.sample(0.05);
        anim.cancel();
        assert_eq!(*seen.borrow(), vec![Completion::Cancelled]);
        assert!(anim.is_settled());
        assert_eq!(anim.value(), scalar(5.0));
        anim.sample(1.0);
        assert_eq!(seen.borrow().len(), 1);
    }

    #[test]
    fn finish_snaps_and_reports_completed() {
        let mut anim = Animated::new(scalar(0.0), tween(10.0), ReducedMotion::Full, 0.0);
        let (cb, seen) = recorder();
        anim.on_complete(cb);
        anim.set_target(scalar(10.0), 0.0).unwrap();
        anim.sample(0.01);
        anim.finish();
        assert_eq!(anim.value(), scalar(10.0));
        assert_eq!(*seen.borrow(), vec![Completion::Completed]);
    }

    #[test]
    fn variant_mismatch_is_an_error_not_a_panic() {
        let mut anim = Animated::new(scalar(0.0), tween(0.1), ReducedMotion::Full, 0.0);
        assert_eq!(
            anim.set_target(MotionValue::Pair(1.0, 2.0), 0.0),
            Err(MotionError::VariantMismatch)
        );
        assert!(anim.is_settled());
        assert_eq!(anim.value(), scalar(0.0));
    }

    #[test]
    fn zero_duration_tween_completes_on_next_sample() {
        let mut anim = Animated::new(
            scalar(1.0),
            MotionKind::Tween {
                duration_secs: 0.0,
                easing: Easing::Linear,
            },
            ReducedMotion::Full,
            0.0,
        );
        anim.set_target(scalar(2.0), 100.0).unwrap();
        assert_eq!(anim.sample(0.1), scalar(2.0));
        assert!(anim.is_settled());
    }

    #[test]
    fn sub_millisecond_tween_stays_exact_via_duration_api() {
        use std::time::Duration;
        // 1.1 ms tween sampled at 550 us: representable only because the
        // clock is f64 seconds, not integer milliseconds.
        let mut anim = Animated::new(
            scalar(0.0),
            MotionKind::Tween {
                duration_secs: Duration::from_micros(1100).as_secs_f64(),
                easing: Easing::Linear,
            },
            ReducedMotion::Full,
            0.0,
        );
        anim.set_target_duration(scalar(1.0), Duration::ZERO)
            .unwrap();
        let mid = anim.sample_duration(Duration::from_micros(550));
        let MotionValue::Scalar(v) = mid else {
            panic!("scalar")
        };
        assert!((v - 0.5).abs() < 1e-6, "v={v}");
        let end = anim.sample_duration(Duration::from_micros(1100));
        assert_eq!(end, scalar(1.0));
        assert!(anim.is_settled());
    }

    #[test]
    fn spring_sample_lands_on_exact_elapsed_not_quantum() {
        // One 1.9 ms jump must take the same substep path as advancing to
        // 1 ms and then to 1.9 ms (1 ms whole step + exact 0.9 ms remainder
        // in both cases) — no 1 ms quantization of the presentation value.
        let mut jumped = Animated::new(
            scalar(0.0),
            MotionKind::Spring(SpringParams::SNAPPY),
            ReducedMotion::Full,
            0.0,
        );
        jumped.set_target(scalar(1.0), 0.0).unwrap();
        jumped.sample(0.0019);

        let mut stepped = Animated::new(
            scalar(0.0),
            MotionKind::Spring(SpringParams::SNAPPY),
            ReducedMotion::Full,
            0.0,
        );
        stepped.set_target(scalar(1.0), 0.0).unwrap();
        stepped.sample(0.001);
        stepped.sample(0.0019);
        let MotionValue::Scalar(a) = jumped.value() else {
            panic!("scalar")
        };
        let MotionValue::Scalar(b) = stepped.value() else {
            panic!("scalar")
        };
        assert!((a - b).abs() < 1e-6, "jumped={a} stepped={b}");

        // And both must agree with fine 100 us stepping to within
        // integration tolerance (different substep sizes, same physics).
        let mut fine = Animated::new(
            scalar(0.0),
            MotionKind::Spring(SpringParams::SNAPPY),
            ReducedMotion::Full,
            0.0,
        );
        fine.set_target(scalar(1.0), 0.0).unwrap();
        let mut now = 0.0;
        for _ in 0..19 {
            now += 0.0001;
            fine.sample(now);
        }
        let MotionValue::Scalar(c) = fine.value() else {
            panic!("scalar")
        };
        assert!((a - c).abs() < 1e-3, "jumped={a} fine={c}");
    }

    // -- reduced motion ---------------------------------------------------------

    #[test]
    fn reduce_collapses_future_and_inflight_motion() {
        let mut anim = Animated::new(scalar(0.0), tween(1.0), ReducedMotion::Reduce, 0.0);
        anim.set_target(scalar(10.0), 0.0).unwrap();
        assert_eq!(anim.kind(), MotionKind::Instant);
        assert_eq!(anim.sample(0.0), scalar(10.0));

        let mut anim = Animated::new(scalar(0.0), tween(1.0), ReducedMotion::Full, 0.0);
        anim.set_target(scalar(10.0), 0.0).unwrap();
        anim.sample(0.4);
        anim.set_reduced_motion(ReducedMotion::Reduce, 0.4);
        assert_eq!(anim.sample(0.4), scalar(10.0));
        assert!(anim.is_settled());
    }

    // -- springs ------------------------------------------------------------------

    #[test]
    fn spring_converges_and_reports_no_deadline_when_settled() {
        let mut anim = Animated::new(
            scalar(0.0),
            MotionKind::Spring(SpringParams::SNAPPY),
            ReducedMotion::Full,
            0.0,
        );
        anim.set_target(scalar(1.0), 0.0).unwrap();
        assert_eq!(anim.next_deadline(0.0), Some(0.0));
        let mut now = 0.0;
        while !anim.is_settled() && now < 10.0 {
            now += 0.016;
            anim.sample(now);
        }
        assert!(
            anim.is_settled(),
            "spring did not settle (last={:?})",
            anim.value()
        );
        assert_eq!(anim.value(), scalar(1.0));
        assert_eq!(anim.next_deadline(now), None);
    }

    #[test]
    fn spring_retarget_preserves_motion_without_jump() {
        let mut anim = Animated::new(
            scalar(0.0),
            MotionKind::Spring(SpringParams::SNAPPY),
            ReducedMotion::Full,
            0.0,
        );
        anim.set_target(scalar(1.0), 0.0).unwrap();
        let mid = anim.sample(0.12);
        assert!(matches!(mid, MotionValue::Scalar(v) if v > 0.0 && v <= 1.5));
        anim.set_target(scalar(0.0), 0.12).unwrap();
        // No discontinuity at the retarget instant.
        assert_eq!(anim.sample(0.12), mid);
    }

    // -- colors ---------------------------------------------------------------------

    #[test]
    fn color_tween_takes_short_hue_path() {
        let a = MotionValue::Color(Oklch::oklch(0.6, 0.1, 10.0, 1.0));
        let b = MotionValue::Color(Oklch::oklch(0.6, 0.1, 350.0, 1.0));
        let mut anim = Animated::new(a, tween(0.1), ReducedMotion::Full, 0.0);
        anim.set_target(b, 0.0).unwrap();
        let MotionValue::Color(mid) = anim.sample(0.05) else {
            panic!("expected color");
        };
        assert!(
            mid.h < 1.0 || mid.h > 359.0,
            "hue took the long path: {}",
            mid.h
        );
    }

    #[test]
    fn color_tween_from_neutral_keeps_chromatic_hue() {
        let gray = MotionValue::Color(Oklch::oklch(0.5, 0.0, 0.0, 1.0));
        let red = MotionValue::Color(Oklch::from_hex("#ff0000").unwrap());
        let mut anim = Animated::new(gray, tween(0.1), ReducedMotion::Full, 0.0);
        anim.set_target(red, 0.0).unwrap();
        let MotionValue::Color(mid) = anim.sample(0.05) else {
            panic!("expected color");
        };
        let MotionValue::Color(target) = red else {
            unreachable!()
        };
        let dh = (mid.h - target.h).abs();
        assert!(
            dh < 5.0 || (mid.h - target.h).abs() > 355.0,
            "hue jumped: {}",
            mid.h
        );
    }

    #[test]
    fn color_spring_settles_on_target() {
        let from = MotionValue::Color(Oklch::oklch(0.5, 0.05, 20.0, 1.0));
        let to = MotionValue::Color(Oklch::oklch(0.7, 0.08, 200.0, 1.0));
        let mut anim = Animated::new(
            from,
            MotionKind::Spring(SpringParams::SNAPPY),
            ReducedMotion::Full,
            0.0,
        );
        anim.set_target(to, 0.0).unwrap();
        let mut now = 0.0;
        while !anim.is_settled() && now < 10.0 {
            now += 0.016;
            anim.sample(now);
        }
        assert!(anim.is_settled());
        assert_eq!(anim.value(), to);
    }

    // -- controller --------------------------------------------------------------------

    #[test]
    fn controller_aggregates_deadlines_and_completions() {
        let mut controller = MotionController::new(ReducedMotion::Full);
        let a = controller.spawn(Some(node(1)), scalar(0.0), tween(0.1), 0.0);
        let b = controller.spawn(Some(node(2)), scalar(0.0), tween(0.3), 0.0);
        controller.set_target(a, scalar(1.0), 0.0).unwrap();
        controller.set_target(b, scalar(1.0), 0.0).unwrap();

        // Runtime scheduling: step a 16 ms frame cadence, advancing only when
        // the controller asks for a frame.
        let mut now = 0.0;
        let mut completed_order = Vec::new();
        while controller.next_deadline(now).is_some() && now < 5.0 {
            for handle in controller.advance(now) {
                completed_order.push(handle);
            }
            now += 0.016;
        }
        assert_eq!(completed_order, vec![a, b]);
        assert_eq!(controller.next_deadline(now), None);
        assert_eq!(controller.sample(a, now), Some(scalar(1.0)));
    }

    #[test]
    fn controller_dispose_owner_is_silent_and_scoped() {
        let mut controller = MotionController::new(ReducedMotion::Full);
        let owned = controller.spawn(Some(node(1)), scalar(0.0), tween(1.0), 0.0);
        let (cb, seen) = recorder();
        assert!(controller.on_complete(owned, cb));
        controller.set_target(owned, scalar(5.0), 0.0).unwrap();
        let other = controller.spawn(Some(node(2)), scalar(0.0), tween(1.0), 0.0);
        controller.set_target(other, scalar(7.0), 0.0).unwrap();

        let removed = controller.dispose_owner(node(1));
        assert_eq!(removed, vec![owned]);
        // The disposed callback is dropped silently: a stale completion must
        // never remove a reopened component.
        assert!(seen.borrow().is_empty());
        assert!(controller.sample(owned, 0.5).is_none());
        // The other owner's motion is untouched and still needs frames.
        assert!(controller.next_deadline(0.5).is_some());
        assert_eq!(controller.sample(other, 0.5), Some(scalar(3.5)));
    }

    #[test]
    fn controller_handles_unknown_handles_without_panicking() {
        let mut controller = MotionController::new(ReducedMotion::Full);
        let ghost = MotionHandle::from_raw(RawId {
            index: 42,
            generation: 0,
        });
        assert_eq!(
            controller.set_target(ghost, scalar(1.0), 0.0),
            Err(MotionError::UnknownHandle)
        );
        assert_eq!(controller.sample(ghost, 0.0), None);
        assert!(!controller.cancel(ghost));
        assert!(!controller.remove(ghost));
        assert!(controller.is_settled(ghost));
        assert_eq!(controller.owner_of(ghost), None);
        // Generational safety: removing then respawning never aliases.
        let h = controller.spawn(None, scalar(0.0), tween(0.01), 0.0);
        controller.remove(h);
        assert_eq!(
            controller.set_target(h, scalar(1.0), 0.0),
            Err(MotionError::UnknownHandle)
        );
    }

    #[test]
    fn controller_reduce_policy_propagates_to_inflight() {
        let mut controller = MotionController::new(ReducedMotion::Full);
        let h = controller.spawn(Some(node(1)), scalar(0.0), tween(1.0), 0.0);
        controller.set_target(h, scalar(9.0), 0.0).unwrap();
        controller.advance(0.4);
        controller.set_reduced_motion(ReducedMotion::Reduce, 0.4);
        assert_eq!(controller.sample(h, 0.4), Some(scalar(9.0)));
        assert_eq!(controller.next_deadline(0.4), None);
    }
}
