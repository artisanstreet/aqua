use std::collections::{HashMap, HashSet};
use std::num::NonZeroU32;
use std::time::Duration;

use aqua_protocol::{FrameReason, Instant, WindowId};

pub const DEFAULT_MAX_IN_FLIGHT: u32 = 2;

pub const MIN_FRAME_INTERVAL: Duration = Duration::from_micros(1);

pub fn gpui_pulse_interval(
    refresh_interval: Duration,
    _active: bool,
    _last_invalidating_input: Option<Instant>,
    _now: Instant,
) -> Duration {
    refresh_interval
}

struct Pending {
    reasons: FrameReason,
    deadline: Option<Instant>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FrameStats {
    pub requested: u64,
    pub produced: u64,
    pub coalesced: u64,
    pub deferred_backpressure: u64,
}

pub struct Scheduler {
    fps_target: Option<NonZeroU32>,
    window_fps: HashMap<WindowId, Option<NonZeroU32>>,
    max_in_flight: u32,
    pending: HashMap<WindowId, Pending>,
    pulse_deadlines: HashMap<WindowId, Instant>,
    in_flight: HashMap<WindowId, u32>,
    pacing_gate: HashMap<WindowId, Instant>,
    resize_gate: HashMap<WindowId, Instant>,
    awaiting_present: HashSet<WindowId>,
    stats: FrameStats,
}

fn animation_only(reasons: FrameReason) -> bool {
    !reasons.is_empty() && (FrameReason::ANIMATION | FrameReason::GPUI_PULSE).contains(reasons)
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            fps_target: None,
            window_fps: HashMap::new(),
            max_in_flight: DEFAULT_MAX_IN_FLIGHT,
            pending: HashMap::new(),
            pulse_deadlines: HashMap::new(),
            in_flight: HashMap::new(),
            pacing_gate: HashMap::new(),
            resize_gate: HashMap::new(),
            awaiting_present: HashSet::new(),
            stats: FrameStats::default(),
        }
    }

    pub fn set_fps_target(&mut self, target: Option<NonZeroU32>) {
        self.fps_target = target;
        self.pacing_gate
            .retain(|window, _| self.window_fps.contains_key(window));
    }

    pub fn set_window_fps_target(&mut self, window: WindowId, target: Option<NonZeroU32>) {
        self.window_fps.insert(window, target);
        self.pacing_gate.remove(&window);
    }

    pub fn reset_window_fps_target(&mut self, window: WindowId) {
        self.window_fps.remove(&window);
        self.pacing_gate.remove(&window);
    }

    pub fn window_fps_target(&self, window: WindowId) -> Option<NonZeroU32> {
        self.window_fps
            .get(&window)
            .copied()
            .unwrap_or(self.fps_target)
    }

    pub fn clear_fps_target(&mut self) {
        self.set_fps_target(None);
    }

    pub fn fps_target(&self) -> Option<NonZeroU32> {
        self.fps_target
    }

    pub fn min_frame_interval(&self) -> Option<Duration> {
        Self::frame_interval(self.fps_target)
    }

    fn frame_interval(target: Option<NonZeroU32>) -> Option<Duration> {
        target.map(|fps| {
            let ideal = Duration::from_secs_f64(1.0 / f64::from(fps.get()));
            ideal.max(MIN_FRAME_INTERVAL)
        })
    }

    pub fn set_max_in_flight(&mut self, n: u32) {
        self.max_in_flight = n.max(1);
    }

    pub fn request_frame(&mut self, window: WindowId, reason: FrameReason) {
        self.stats.requested += 1;
        match self.pending.get_mut(&window) {
            Some(p) => {
                p.reasons |= reason;
                p.deadline = None;
                self.stats.coalesced += 1;
            }
            None => {
                self.pending.insert(
                    window,
                    Pending {
                        reasons: reason,
                        deadline: None,
                    },
                );
            }
        }
    }

    pub fn request_animation_deadline(&mut self, window: WindowId, deadline: Instant) {
        self.stats.requested += 1;
        match self.pending.get_mut(&window) {
            Some(p) => {
                p.reasons |= FrameReason::ANIMATION;
                p.deadline = p.deadline.map(|d| d.min(deadline));
                self.stats.coalesced += 1;
            }
            None => {
                self.pending.insert(
                    window,
                    Pending {
                        reasons: FrameReason::ANIMATION,
                        deadline: Some(deadline),
                    },
                );
            }
        }
    }

    pub fn pending_reasons(&self, window: WindowId) -> Option<FrameReason> {
        let mut reasons = self
            .pending
            .get(&window)
            .map_or(FrameReason::empty(), |p| p.reasons);
        if self.pulse_deadlines.contains_key(&window) {
            reasons |= FrameReason::GPUI_PULSE;
        }
        (!reasons.is_empty()).then_some(reasons)
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty() || !self.pulse_deadlines.is_empty()
    }

    pub fn request_gpui_pulse_deadline(&mut self, window: WindowId, deadline: Instant) {
        self.pulse_deadlines.insert(window, deadline);
    }

    fn gate_blocks(&self, id: WindowId, now: Instant) -> bool {
        match self.pacing_gate.get(&id) {
            Some(gate) => now < *gate,
            None => false,
        }
    }

    pub fn ready_windows(&mut self, now: Instant) -> Vec<(WindowId, FrameReason)> {
        let mut ready = Vec::new();
        let mut deferred = 0u64;
        let mut ids: Vec<WindowId> = self
            .pending
            .keys()
            .chain(self.pulse_deadlines.keys())
            .copied()
            .collect();
        ids.sort();
        ids.dedup();
        for id in ids {
            if self.gate_blocks(id, now) {
                continue;
            }
            let mut reasons = self
                .pending
                .get(&id)
                .filter(|p| {
                    p.reasons != FrameReason::ANIMATION || p.deadline.is_none_or(|d| now >= d)
                })
                .map_or(FrameReason::empty(), |p| p.reasons);
            if self
                .pulse_deadlines
                .get(&id)
                .is_some_and(|deadline| now >= *deadline)
            {
                reasons |= FrameReason::GPUI_PULSE;
            }
            if reasons.is_empty() || self.resize_blocked(id, reasons, now) {
                continue;
            }
            if animation_only(reasons) && self.awaiting_present.contains(&id) {
                continue;
            }
            let flying = self.in_flight.get(&id).copied().unwrap_or(0);
            if flying >= self.max_in_flight {
                deferred += 1;
                continue;
            }
            ready.push((id, reasons));
        }
        self.stats.deferred_backpressure += deferred;
        ready
    }

    pub fn mark_produced(&mut self, windows: &[WindowId], now: Instant) {
        for id in windows {
            let interval = Self::frame_interval(self.window_fps_target(*id));
            let pulse_ready = self
                .pulse_deadlines
                .get(id)
                .is_some_and(|deadline| now >= *deadline);
            if pulse_ready {
                self.pulse_deadlines.remove(id);
            }
            let normal_ready = self.pending.get(id).is_some_and(|p| {
                !pulse_ready
                    || p.reasons != FrameReason::ANIMATION
                    || p.deadline.is_none_or(|d| now >= d)
            });
            if normal_ready {
                self.pending.remove(id);
            }
            if normal_ready || pulse_ready {
                self.stats.produced += 1;
            }
            *self.in_flight.entry(*id).or_insert(0) += 1;
            if let Some(dt) = interval {
                self.pacing_gate.insert(*id, now + dt);
            }
        }
    }

    pub fn defer_resize_frames(&mut self, window: WindowId, until: Instant) {
        let gate = self.resize_gate.entry(window).or_insert(until);
        *gate = (*gate).max(until);
    }

    pub fn resize_gate(&self, window: WindowId) -> Option<Instant> {
        self.resize_gate.get(&window).copied()
    }

    fn resize_blocked(&self, id: WindowId, reasons: FrameReason, now: Instant) -> bool {
        reasons.contains(FrameReason::RESIZE)
            && self.resize_gate.get(&id).is_some_and(|gate| now < *gate)
    }

    pub fn await_present(&mut self, window: WindowId) {
        self.awaiting_present.insert(window);
    }

    pub fn present_arrived(&mut self, window: WindowId) {
        self.awaiting_present.remove(&window);
    }

    pub fn mark_presented(&mut self, window: WindowId) {
        if let Some(n) = self.in_flight.get_mut(&window) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.in_flight.remove(&window);
            }
        }
    }

    pub fn next_wakeup(&self, now: Instant) -> Option<Instant> {
        let mut best: Option<Instant> = None;
        let mut consider = |t: Instant| {
            best = Some(match best {
                Some(b) => b.min(t),
                None => t,
            });
        };
        for (id, p) in &self.pending {
            let flying = self.in_flight.get(id).copied().unwrap_or(0);
            if flying >= self.max_in_flight {
                continue;
            }
            if animation_only(p.reasons) && self.awaiting_present.contains(id) {
                continue;
            }
            if p.reasons == FrameReason::ANIMATION {
                match p.deadline {
                    Some(d) if d > now => {
                        let mut t = d;
                        if let Some(g) = self.pacing_gate.get(id) {
                            if *g > t {
                                t = *g;
                            }
                        }
                        consider(t);
                        continue;
                    }
                    _ => {}
                }
            }
            let pacing = self.pacing_gate.get(id).copied().filter(|g| *g > now);
            let resize = self
                .resize_gate
                .get(id)
                .copied()
                .filter(|g| *g > now && p.reasons.contains(FrameReason::RESIZE));
            match pacing.into_iter().chain(resize).max() {
                Some(gate) => consider(gate),
                None => return Some(now),
            }
        }
        for (id, deadline) in &self.pulse_deadlines {
            if self.in_flight.get(id).copied().unwrap_or(0) >= self.max_in_flight {
                continue;
            }
            if self.awaiting_present.contains(id) {
                continue;
            }
            let mut at = *deadline;
            if let Some(gate) = self.pacing_gate.get(id) {
                at = at.max(*gate);
            }
            consider(at.max(now));
        }
        best
    }

    pub fn remove_window(&mut self, window: WindowId) {
        self.pending.remove(&window);
        self.pulse_deadlines.remove(&window);
        self.in_flight.remove(&window);
        self.pacing_gate.remove(&window);
        self.resize_gate.remove(&window);
        self.awaiting_present.remove(&window);
        self.window_fps.remove(&window);
    }

    pub fn stats(&self) -> &FrameStats {
        &self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> WindowId {
        WindowId::new(n).unwrap()
    }

    fn t0() -> Instant {
        Instant::from_nanos(1_000_000_000)
    }

    #[test]
    fn fps_target_accepts_any_nonzero_clears_to_unpaced() {
        let mut s = Scheduler::new();
        assert_eq!(s.fps_target(), None);
        s.set_fps_target(Some(NonZeroU32::new(60).unwrap()));
        assert_eq!(s.fps_target().unwrap().get(), 60);
        s.set_fps_target(Some(NonZeroU32::new(10_000).unwrap()));
        assert_eq!(s.fps_target().unwrap().get(), 10_000);
        assert!(s.min_frame_interval().unwrap() >= MIN_FRAME_INTERVAL);
        s.clear_fps_target();
        assert_eq!(s.fps_target(), None);
        assert_eq!(s.min_frame_interval(), None);
    }

    #[test]
    fn reasons_merge_and_coalesce() {
        let mut s = Scheduler::new();
        let now = t0();
        s.request_frame(id(1), FrameReason::INPUT);
        s.request_frame(id(1), FrameReason::DAMAGE);
        assert_eq!(
            s.pending_reasons(id(1)).unwrap(),
            FrameReason::INPUT | FrameReason::DAMAGE
        );
        assert_eq!(s.stats().requested, 2);
        assert_eq!(s.stats().coalesced, 1);
        let ready = s.ready_windows(now);
        assert_eq!(ready.len(), 1);
        s.mark_produced(&[id(1)], now);
        assert_eq!(s.stats().produced, 1);
        assert!(!s.has_pending());
    }

    #[test]
    fn animation_frames_wait_for_the_previous_present() {
        let mut s = Scheduler::new();
        let now = t0();
        s.request_frame(id(5), FrameReason::ANIMATION);
        assert_eq!(s.ready_windows(now).len(), 1);
        s.mark_produced(&[id(5)], now);
        s.mark_presented(id(5));
        s.await_present(id(5));
        s.request_frame(id(5), FrameReason::ANIMATION);
        let later = now + Duration::from_millis(50);
        assert!(s.ready_windows(later).is_empty());
        assert_eq!(s.next_wakeup(later), None);
        s.present_arrived(id(5));
        assert_eq!(s.ready_windows(later).len(), 1);
        assert_eq!(s.next_wakeup(later), Some(later));
    }

    #[test]
    fn input_frames_do_not_wait_for_a_present() {
        let mut s = Scheduler::new();
        let now = t0();
        s.await_present(id(6));
        s.request_frame(id(6), FrameReason::ANIMATION);
        s.request_frame(id(6), FrameReason::INPUT);
        assert_eq!(s.ready_windows(now).len(), 1);
    }

    #[test]
    fn a_later_deadline_never_postpones_an_immediate_animation_frame() {
        let now = t0();
        let later = now + Duration::from_millis(120);
        let mut first = Scheduler::new();
        first.request_frame(id(7), FrameReason::ANIMATION);
        first.request_animation_deadline(id(7), later);
        assert_eq!(first.ready_windows(now).len(), 1);
        let mut second = Scheduler::new();
        second.request_animation_deadline(id(7), later);
        second.request_frame(id(7), FrameReason::ANIMATION);
        assert_eq!(second.ready_windows(now).len(), 1);
    }

    #[test]
    fn no_idle_ticks() {
        let s = Scheduler::new();
        assert!(!s.has_pending());
        assert_eq!(s.next_wakeup(t0()), None);
    }

    #[test]
    fn animation_deadline_drives_wakeup() {
        let mut s = Scheduler::new();
        let now = t0();
        let deadline = now + Duration::from_millis(100);
        s.request_animation_deadline(id(2), deadline);
        assert!(s.ready_windows(now).is_empty());
        assert_eq!(s.next_wakeup(now), Some(deadline));
        let later = deadline + Duration::from_millis(1);
        let ready = s.ready_windows(later);
        assert_eq!(ready.len(), 1);
        assert!(ready[0].1.contains(FrameReason::ANIMATION));
    }

    #[test]
    fn damage_with_future_animation_deadline_draws_now() {
        let mut s = Scheduler::new();
        let now = t0();
        s.request_frame(id(2), FrameReason::DAMAGE);
        s.request_animation_deadline(id(2), now + Duration::from_secs(1));
        assert_eq!(s.ready_windows(now).len(), 1);
        assert_eq!(s.next_wakeup(now), Some(now));
    }

    #[test]
    fn backpressure_defers_without_queue_growth() {
        let mut s = Scheduler::new();
        s.set_max_in_flight(1);
        let now = t0();
        s.request_frame(id(3), FrameReason::DAMAGE);
        s.mark_produced(&[id(3)], now);
        s.request_frame(id(3), FrameReason::DAMAGE);
        assert!(s.ready_windows(now).is_empty());
        assert_eq!(s.stats().deferred_backpressure, 1);
        s.mark_presented(id(3));
        assert_eq!(s.ready_windows(now).len(), 1);
    }

    #[test]
    fn pacing_enforced_then_released_by_runtime_target_change() {
        let mut s = Scheduler::new();
        s.set_fps_target(Some(NonZeroU32::new(60).unwrap()));
        let start = t0();
        s.request_frame(id(4), FrameReason::DAMAGE);
        s.mark_produced(&[id(4)], start);
        s.mark_presented(id(4));
        s.request_frame(id(4), FrameReason::DAMAGE);
        assert!(s.ready_windows(start).is_empty());
        let wake = s
            .next_wakeup(start)
            .expect("pacing gate must schedule a wakeup");
        assert!(wake > start);
        s.clear_fps_target();
        let ready = s.ready_windows(start);
        assert_eq!(ready.len(), 1, "target change must release stale gate");
        s.set_fps_target(Some(NonZeroU32::new(120).unwrap()));
        s.mark_produced(&[id(4)], start);
        s.mark_presented(id(4));
        s.request_frame(id(4), FrameReason::DAMAGE);
        assert!(s.ready_windows(start).is_empty());
        let gate = start + Duration::from_secs_f64(1.0 / 120.0);
        assert_eq!(s.next_wakeup(start), Some(gate));
    }

    #[test]
    fn multiwindow_independent() {
        let mut s = Scheduler::new();
        let now = t0();
        s.request_frame(id(10), FrameReason::INPUT);
        s.request_frame(id(11), FrameReason::DAMAGE);
        let mut ready = s.ready_windows(now);
        ready.sort_by_key(|(i, _)| *i);
        assert_eq!(ready.len(), 2);
        s.mark_produced(&[id(10)], now);
        assert_eq!(s.pending_reasons(id(10)), None);
        assert!(s.pending_reasons(id(11)).is_some());
        s.remove_window(id(11));
        assert!(s.pending_reasons(id(11)).is_none());
    }

    #[test]
    fn blocked_window_sleeps_until_completion() {
        let mut s = Scheduler::new();
        s.set_max_in_flight(1);
        let now = t0();
        s.request_frame(id(20), FrameReason::DAMAGE);
        s.mark_produced(&[id(20)], now);
        s.request_frame(id(20), FrameReason::DAMAGE);
        assert!(s.ready_windows(now).is_empty());
        assert_eq!(
            s.next_wakeup(now),
            None,
            "blocked window must sleep; completion wakes it"
        );
        s.mark_presented(id(20));
        assert_eq!(s.next_wakeup(now), Some(now));
        assert_eq!(s.ready_windows(now).len(), 1);
    }

    #[test]
    fn blocked_window_does_not_starve_others() {
        let mut s = Scheduler::new();
        s.set_max_in_flight(1);
        let now = t0();
        s.request_frame(id(21), FrameReason::DAMAGE);
        s.mark_produced(&[id(21)], now);
        s.request_frame(id(21), FrameReason::DAMAGE);
        s.request_frame(id(22), FrameReason::INPUT);
        assert_eq!(s.next_wakeup(now), Some(now));
        let ready = s.ready_windows(now);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].0, id(22));
    }

    #[test]
    fn animation_wakeup_fuses_gate_and_deadline() {
        let mut s = Scheduler::new();
        s.set_fps_target(Some(NonZeroU32::new(60).unwrap()));
        let start = t0();
        s.request_animation_deadline(id(23), start + Duration::from_millis(10));
        s.mark_produced(&[id(23)], start);
        s.request_animation_deadline(id(23), start + Duration::from_millis(100));
        let gate = start + Duration::from_secs_f64(1.0 / 60.0);
        let deadline = start + Duration::from_millis(100);
        assert!(deadline > gate);
        assert_eq!(
            s.next_wakeup(start),
            Some(deadline),
            "must wake once at max(gate, deadline)"
        );
        assert!(s.ready_windows(start).is_empty());
        let mut s2 = Scheduler::new();
        s2.set_fps_target(Some(NonZeroU32::new(60).unwrap()));
        s2.request_frame(id(24), FrameReason::DAMAGE);
        s2.mark_produced(&[id(24)], start);
        s2.request_animation_deadline(id(24), start + Duration::from_millis(1));
        assert_eq!(
            s2.next_wakeup(start),
            Some(gate),
            "early deadline still gated by pacing"
        );
    }

    #[test]
    fn overload_recovers_to_idle() {
        let mut s = Scheduler::new();
        s.set_max_in_flight(1);
        let now = t0();
        for _ in 0..1000 {
            s.request_frame(id(30), FrameReason::DAMAGE);
        }
        assert_eq!(s.stats().requested, 1000);
        assert_eq!(s.stats().coalesced, 999);
        s.mark_produced(&[id(30)], now);
        for _ in 0..1000 {
            s.request_frame(id(30), FrameReason::DAMAGE);
        }
        assert!(s.ready_windows(now).is_empty());
        s.mark_presented(id(30));
        assert_eq!(s.ready_windows(now).len(), 1);
        s.mark_produced(&[id(30)], now);
        s.mark_presented(id(30));
        assert!(!s.has_pending());
        assert_eq!(s.next_wakeup(now), None);
        assert_eq!(s.stats().produced, 2);
    }

    #[test]
    fn resize_storm_coalesces() {
        let mut s = Scheduler::new();
        let now = t0();
        for _ in 0..50 {
            s.request_frame(id(31), FrameReason::RESIZE);
        }
        let ready = s.ready_windows(now);
        assert_eq!(ready.len(), 1);
        assert!(ready[0].1.contains(FrameReason::RESIZE));
    }

    #[test]
    fn remove_window_during_flight_no_leak() {
        let mut s = Scheduler::new();
        s.set_max_in_flight(1);
        let now = t0();
        s.request_frame(id(32), FrameReason::DAMAGE);
        s.mark_produced(&[id(32)], now);
        s.remove_window(id(32));
        s.mark_presented(id(32));
        s.request_frame(id(32), FrameReason::DAMAGE);
        assert_eq!(s.ready_windows(now).len(), 1, "fresh state after removal");
    }

    #[test]
    fn presented_without_produce_saturates() {
        let mut s = Scheduler::new();
        let now = t0();
        s.mark_presented(id(33));
        s.mark_presented(id(33));
        s.request_frame(id(33), FrameReason::DAMAGE);
        assert_eq!(s.ready_windows(now).len(), 1);
    }

    #[test]
    fn deadline_supersede_keeps_earliest() {
        let mut s = Scheduler::new();
        let now = t0();
        let late = now + Duration::from_millis(500);
        let early = now + Duration::from_millis(50);
        s.request_animation_deadline(id(34), late);
        assert_eq!(s.next_wakeup(now), Some(late));
        s.request_animation_deadline(id(34), early);
        assert_eq!(s.next_wakeup(now), Some(early));
        s.request_animation_deadline(id(34), late);
        assert_eq!(s.next_wakeup(now), Some(early));
    }
}
