use aqua_core::id::RawId;
use aqua_core::{Arena, NodeId};
use std::time::Duration;

pub mod runtime;

pub use aqua_style::motion::{
    Animate, AnimatedProperty, Motion, MotionRef, MotionSpec, MotionTheme, MotionToken, Presence,
};

pub use aqua_motion::{
    Completion, Easing, MotionError, MotionKind, MotionValue, ReducedMotion, Seconds, SpringParams,
    SpringSegment, Track, SPRING_MAX_DURATION_SECS,
};

pub struct Animated {
    track: Track,
    completion: Option<PendingCompletion>,
}

struct PendingCompletion {
    target: Option<MotionValue>,
    callback: Box<dyn FnMut(Completion)>,
}

impl Animated {
    pub fn new(
        initial: MotionValue,
        kind: MotionKind,
        reduced: ReducedMotion,
        now: Seconds,
    ) -> Self {
        Self {
            track: Track::new(initial, kind, reduced, now),
            completion: None,
        }
    }

    pub fn on_complete(&mut self, callback: Box<dyn FnMut(Completion)>) {
        self.completion = Some(PendingCompletion {
            target: None,
            callback,
        });
    }

    #[must_use]
    pub fn value(&self) -> MotionValue {
        self.track.value()
    }

    #[must_use]
    pub fn target(&self) -> MotionValue {
        self.track.target()
    }

    #[must_use]
    pub fn kind(&self) -> MotionKind {
        self.track.kind()
    }

    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.track.is_settled()
    }

    #[must_use]
    pub fn spring_segments(&self) -> &[SpringSegment] {
        self.track.spring_segments()
    }

    pub fn set_target(&mut self, to: MotionValue, now: Seconds) -> Result<(), MotionError> {
        if std::mem::discriminant(&self.track.value()) != std::mem::discriminant(&to) {
            return Err(MotionError::VariantMismatch);
        }
        let was_settled = self.track.is_settled();
        let reached = self.track.progress(now);
        if let Some(mut pending) = self.completion.take() {
            if pending.target.is_none() {
                pending.target = Some(to);
                self.completion = Some(pending);
            } else if reached && !was_settled {
                (pending.callback)(Completion::Completed);
            }
        }
        let kind = self.track.kind();
        self.track.retarget(to, kind, now)
    }

    pub fn set_reduced_motion(&mut self, reduced: ReducedMotion, now: Seconds) {
        if self.track.set_reduced(reduced, now) {
            self.fire(Completion::Completed);
        }
    }

    pub fn sample(&mut self, now: Seconds) -> MotionValue {
        if self.track.is_settled() {
            self.fire(Completion::Completed);
            return self.track.value();
        }
        let value = self.track.sample(now);
        if self.track.is_settled() {
            self.fire(Completion::Completed);
        }
        value
    }

    pub fn cancel(&mut self) {
        if self.track.is_settled() {
            return;
        }
        self.track.halt();
        self.fire(Completion::Cancelled);
    }

    pub fn finish(&mut self) {
        if self.track.is_settled() {
            return;
        }
        self.track.finish();
        self.fire(Completion::Completed);
    }

    #[must_use]
    pub fn next_deadline(&self, now: Seconds) -> Option<Seconds> {
        self.track.next_deadline(now)
    }

    #[must_use]
    pub fn tween_end(&self) -> Option<Seconds> {
        self.track.tween_end()
    }

    pub fn sample_duration(&mut self, now: Duration) -> MotionValue {
        self.sample(now.as_secs_f64())
    }

    pub fn set_target_duration(
        &mut self,
        to: MotionValue,
        now: Duration,
    ) -> Result<(), MotionError> {
        self.set_target(to, now.as_secs_f64())
    }

    fn fire(&mut self, completion: Completion) {
        if let Some(mut pending) = self.completion.take() {
            (pending.callback)(completion);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MotionHandle(RawId);

impl MotionHandle {
    #[must_use]
    pub const fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> RawId {
        self.0
    }
}

struct Slot {
    owner: Option<NodeId>,
    anim: Animated,
}

pub struct MotionController {
    arena: Arena<Slot>,
    reduced: ReducedMotion,
}

impl Default for MotionController {
    fn default() -> Self {
        Self::new(ReducedMotion::Full)
    }
}

impl MotionController {
    #[must_use]
    pub fn new(reduced: ReducedMotion) -> Self {
        Self {
            arena: Arena::new(),
            reduced,
        }
    }

    #[must_use]
    pub fn reduced_motion(&self) -> ReducedMotion {
        self.reduced
    }

    pub fn set_reduced_motion(&mut self, reduced: ReducedMotion, now: Seconds) {
        self.reduced = reduced;
        for (_, slot) in self.arena.iter_mut() {
            slot.anim.set_reduced_motion(reduced, now);
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.arena.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.arena.is_empty()
    }

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

    pub fn sample(&mut self, handle: MotionHandle, now: Seconds) -> Option<MotionValue> {
        self.arena
            .get_mut(handle.0)
            .map(|slot| slot.anim.sample(now))
    }

    #[must_use]
    pub fn peek(&self, handle: MotionHandle) -> Option<MotionValue> {
        self.arena.get(handle.0).map(|slot| slot.anim.value())
    }

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

    pub fn cancel(&mut self, handle: MotionHandle) -> bool {
        match self.arena.get_mut(handle.0) {
            Some(slot) => {
                slot.anim.cancel();
                true
            }
            None => false,
        }
    }

    pub fn finish(&mut self, handle: MotionHandle) -> bool {
        match self.arena.get_mut(handle.0) {
            Some(slot) => {
                slot.anim.finish();
                true
            }
            None => false,
        }
    }

    #[must_use]
    pub fn is_settled(&self, handle: MotionHandle) -> bool {
        self.arena
            .get(handle.0)
            .is_none_or(|slot| slot.anim.is_settled())
    }

    #[must_use]
    pub fn owner_of(&self, handle: MotionHandle) -> Option<Option<NodeId>> {
        self.arena.get(handle.0).map(|slot| slot.owner)
    }

    #[must_use]
    pub fn next_deadline(&self, now: Seconds) -> Option<Seconds> {
        self.arena
            .iter()
            .filter_map(|(_, slot)| slot.anim.next_deadline(now))
            .reduce(Seconds::min)
    }

    pub fn advance(&mut self, now: Seconds) -> Vec<MotionHandle> {
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

    pub fn advance_duration(&mut self, now: Duration) -> Vec<MotionHandle> {
        self.advance(now.as_secs_f64())
    }

    pub fn sample_duration(&mut self, handle: MotionHandle, now: Duration) -> Option<MotionValue> {
        self.sample(handle, now.as_secs_f64())
    }

    pub fn set_target_duration(
        &mut self,
        handle: MotionHandle,
        to: MotionValue,
        now: Duration,
    ) -> Result<(), MotionError> {
        self.set_target(handle, to, now.as_secs_f64())
    }

    #[must_use]
    pub fn next_deadline_duration(&self, now: Duration) -> Option<Duration> {
        self.next_deadline(now.as_secs_f64())
            .and_then(|secs| Duration::try_from_secs_f64(secs).ok())
    }

    pub fn remove(&mut self, handle: MotionHandle) -> bool {
        self.arena.remove(handle.0).is_ok()
    }

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
    use aqua_core::Oklch;
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

    #[test]
    fn tween_samples_midpoint_and_settles_at_end() {
        let mut anim = Animated::new(scalar(0.0), tween(1.0), ReducedMotion::Full, 0.0);
        anim.set_target(scalar(100.0), 0.0).unwrap();
        assert_eq!(anim.sample(0.5), scalar(50.0));
        assert!(!anim.is_settled());
        assert_eq!(anim.sample(1.0), scalar(100.0));
        assert!(anim.is_settled());
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
        let spawn = || {
            let mut anim = Animated::new(
                scalar(0.0),
                MotionKind::Spring(SpringParams::SNAPPY),
                ReducedMotion::Full,
                0.0,
            );
            anim.set_target(scalar(1.0), 0.0).unwrap();
            anim
        };
        let mut jumped = spawn();
        jumped.sample(0.0019);

        let mut stepped = spawn();
        stepped.sample(0.001);
        stepped.sample(0.0019);

        let mut fine = spawn();
        (1..=19).for_each(|i| {
            fine.sample(f64::from(i) * 0.0001);
        });

        let MotionValue::Scalar(a) = jumped.value() else {
            panic!("scalar")
        };
        let MotionValue::Scalar(b) = stepped.value() else {
            panic!("scalar")
        };
        let MotionValue::Scalar(c) = fine.value() else {
            panic!("scalar")
        };
        assert!(a > 0.0, "spring did not move: {a}");
        assert!((a - b).abs() < 1e-6, "jumped={a} stepped={b}");
        assert!((a - c).abs() < 1e-6, "jumped={a} fine={c}");
    }

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
        assert_eq!(anim.sample(0.12), mid);
    }

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

    #[test]
    fn controller_aggregates_deadlines_and_completions() {
        let mut controller = MotionController::new(ReducedMotion::Full);
        let a = controller.spawn(Some(node(1)), scalar(0.0), tween(0.1), 0.0);
        let b = controller.spawn(Some(node(2)), scalar(0.0), tween(0.3), 0.0);
        controller.set_target(a, scalar(1.0), 0.0).unwrap();
        controller.set_target(b, scalar(1.0), 0.0).unwrap();

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
        assert!(seen.borrow().is_empty());
        assert!(controller.sample(owned, 0.5).is_none());
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
