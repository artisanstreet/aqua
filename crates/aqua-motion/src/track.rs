use crate::curve::{
    MotionError, MotionKind, MotionValue, ReducedMotion, SPRING_MAX_DURATION_SECS, Seconds,
    SpringRegime, SpringSegment, launch_segments, resolve_kind, rest_segments, spring_targets,
};

#[derive(Clone, Debug)]
pub struct Track {
    from: MotionValue,
    to: MotionValue,
    kind: MotionKind,
    reduced: ReducedMotion,
    start_secs: Seconds,
    last_secs: Seconds,
    end_secs: Seconds,
    settle_secs: Seconds,
    segments: [SpringSegment; 4],
    current: MotionValue,
    settled: bool,
}

impl Track {
    #[must_use]
    pub fn new(
        initial: MotionValue,
        kind: MotionKind,
        reduced: ReducedMotion,
        now: Seconds,
    ) -> Self {
        Self {
            from: initial,
            to: initial,
            kind: resolve_kind(kind, reduced),
            reduced,
            start_secs: now,
            last_secs: now,
            end_secs: now,
            settle_secs: now,
            segments: rest_segments(now, initial.channels()),
            current: initial,
            settled: true,
        }
    }

    #[must_use]
    pub fn value(&self) -> MotionValue {
        self.current
    }

    #[must_use]
    pub fn target(&self) -> MotionValue {
        self.to
    }

    #[must_use]
    pub fn kind(&self) -> MotionKind {
        self.kind
    }

    #[must_use]
    pub fn reduced(&self) -> ReducedMotion {
        self.reduced
    }

    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.settled
    }

    #[must_use]
    pub fn spring_segments(&self) -> &[SpringSegment] {
        &self.segments[..self.to.channel_count()]
    }

    #[must_use]
    pub fn next_deadline(&self, now: Seconds) -> Option<Seconds> {
        (!self.settled).then(|| now.max(self.last_secs))
    }

    #[must_use]
    pub fn tween_end(&self) -> Option<Seconds> {
        match self.kind {
            MotionKind::Tween { .. } if !self.settled => Some(self.end_secs),
            _ => None,
        }
    }

    #[must_use]
    pub fn settle_deadline(&self) -> Option<Seconds> {
        (!self.settled).then_some(self.settle_secs)
    }

    pub fn retarget(
        &mut self,
        to: MotionValue,
        kind: MotionKind,
        now: Seconds,
    ) -> Result<(), MotionError> {
        if !MotionValue::variant_eq(&self.current, &to) {
            return Err(MotionError::VariantMismatch);
        }
        let now = now.max(self.last_secs);
        let velocities = self.velocities_at(now);
        self.from = self.current;
        self.to = to;
        self.kind = resolve_kind(kind, self.reduced);
        self.start_secs = now;
        self.last_secs = now;
        self.settled = false;
        match self.kind {
            MotionKind::Tween { duration_secs, .. } => {
                self.end_secs = now + duration_secs.max(0.0);
                self.settle_secs = self.end_secs;
                self.segments = rest_segments(now, to.channels());
            }
            MotionKind::Spring(params) => {
                self.end_secs = now + SPRING_MAX_DURATION_SECS;
                let basis = self.current.channels();
                let targets = spring_targets(&basis, &to);
                self.segments = launch_segments(now, basis, velocities, targets);
                let regime = SpringRegime::of(params);
                let bound = self
                    .segments
                    .iter()
                    .take(to.channel_count())
                    .map(|segment| {
                        regime.settle_bound(
                            f64::from(segment.x0) - f64::from(segment.target),
                            f64::from(segment.v0),
                        )
                    })
                    .fold(0.0, f64::max);
                self.settle_secs = now + bound;
            }
            MotionKind::Instant => {
                self.end_secs = now;
                self.settle_secs = now;
                self.segments = rest_segments(now, to.channels());
            }
        }
        Ok(())
    }

    pub fn set_reduced(&mut self, reduced: ReducedMotion, now: Seconds) -> bool {
        self.reduced = reduced;
        if self.progress(now) {
            self.settle(now);
            return true;
        }
        if reduced == ReducedMotion::Reduce {
            self.from = self.current;
            self.kind = MotionKind::Instant;
            self.end_secs = now.max(self.last_secs);
            self.settle_secs = self.end_secs;
        }
        false
    }

    pub fn progress(&mut self, now: Seconds) -> bool {
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
                    self.current = MotionValue::blend(&self.from, &self.to, t).unwrap_or(self.to);
                    self.last_secs = now;
                    false
                }
            }
            MotionKind::Spring(params) => {
                let regime = SpringRegime::of(params);
                let states = self.segments.map(|segment| segment.sample_in(regime, now));
                let count = self.to.channel_count();
                let at_rest = states
                    .iter()
                    .zip(&self.segments)
                    .take(count)
                    .all(|(&(x, v), segment)| SpringSegment::is_settled_at(x, v, segment.target));
                self.last_secs = now;
                if at_rest || now - self.start_secs >= SPRING_MAX_DURATION_SECS {
                    self.current = self.to;
                    true
                } else {
                    self.current = MotionValue::from_channels(&self.to, &states.map(|(x, _)| x));
                    false
                }
            }
        }
    }

    pub fn sample(&mut self, now: Seconds) -> MotionValue {
        if !self.settled && self.progress(now) {
            self.settle(now);
        }
        self.current
    }

    pub fn settle(&mut self, now: Seconds) {
        self.current = self.to;
        self.last_secs = now.max(self.last_secs);
        self.settled = true;
    }

    pub fn halt(&mut self) {
        self.settled = true;
    }

    pub fn finish(&mut self) {
        if self.settled {
            return;
        }
        self.current = self.to;
        self.last_secs = self.last_secs.max(self.start_secs);
        self.settled = true;
    }

    fn velocities_at(&self, now: Seconds) -> [f32; 4] {
        match (self.kind, self.settled) {
            (MotionKind::Spring(params), false) => {
                let regime = SpringRegime::of(params);
                self.segments
                    .map(|segment| segment.sample_in(regime, now).1)
            }
            _ => [0.0; 4],
        }
    }
}
