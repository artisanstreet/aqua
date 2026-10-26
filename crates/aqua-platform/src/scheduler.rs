//! Demand-driven deadline scheduler.
//!
//! Idle windows schedule NO frames. Work is requested with a [`FrameReason`];
//! animation/media register deadlines. The facade asks [`Scheduler`] what to
//! draw now and when to wake next, then drives `winit`'s
//! `set_control_flow(WaitUntil(next))`. There is no fixed tick.
//!
//! Backpressure: at most [`Scheduler::max_in_flight`] unpresented frames per
//! window are allowed; further ready windows are deferred (counted in
//! [`FrameStats::deferred_backpressure`]) rather than queued, so an extreme
//! frame-rate target can never pile up obsolete frames.
//!
//! Determinism: every transition takes the caller's `now: Instant`. The
//! scheduler never reads the system clock itself, so tests drive exact
//! timelines and production passes the event-loop timestamp through.
//!
//! Frame-rate target: any `NonZeroU32` is accepted (`None` = unpaced,
//! demand-only). There is intentionally NO arbitrary upper bound: the target
//! only paces newly *produced* frames (`1/fps` minimum interval, clamped to
//! a minimum of 1µs so exotic targets cannot produce a zero duration), and
//! real throughput stays bounded by display vsync, the presentation policy,
//! and backpressure. An unchanged app sleeps even at 900 Hz.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use crate::geometry::AquaWindowId;

bitflags::bitflags! {
    /// WHY a frame was requested. Recorded per window, OR-merged, and
    /// exposed to the inspector via [`Scheduler::pending_reasons`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FrameReason: u32 {
        const INPUT      = 0b0000_0001;
        const DAMAGE     = 0b0000_0010;
        const ANIMATION  = 0b0000_0100;
        const MEDIA      = 0b0000_1000;
        const EXPOSE     = 0b0000_0001_0000;
        const RESIZE     = 0b0000_0010_0000;
        const MATERIAL   = 0b0000_0100_0000;
        const EXPLICIT   = 0b0000_1000_0000;
    }
}

/// Default in-flight cap per window.
pub const DEFAULT_MAX_IN_FLIGHT: u32 = 2;

/// Floor for the pacing interval so absurd targets (e.g. u32::MAX Hz) yield
/// a usable 1µs gate instead of a zero duration.
pub const MIN_FRAME_INTERVAL: Duration = Duration::from_micros(1);

struct Pending {
    reasons: FrameReason,
    /// Animation/media deadline, if any.
    deadline: Option<Instant>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FrameStats {
    pub requested: u64,
    pub produced: u64,
    pub coalesced: u64,
    pub deferred_backpressure: u64,
}

/// Demand-driven multi-window frame scheduler.
pub struct Scheduler {
    fps_target: Option<NonZeroU32>,
    max_in_flight: u32,
    pending: HashMap<AquaWindowId, Pending>,
    in_flight: HashMap<AquaWindowId, u32>,
    /// Earliest time each window may *produce* again (pacing). Evaluated
    /// lazily in [`ready_windows`](Self::ready_windows) against the current
    /// target, so runtime target changes apply to already-pending work.
    pacing_gate: HashMap<AquaWindowId, Instant>,
    stats: FrameStats,
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
            max_in_flight: DEFAULT_MAX_IN_FLIGHT,
            pending: HashMap::new(),
            in_flight: HashMap::new(),
            pacing_gate: HashMap::new(),
            stats: FrameStats::default(),
        }
    }

    /// Set the pacing target. `None` = unpaced (produce on demand).
    /// `Some(fps)` enforces a `1/fps` minimum interval between produced
    /// frames per window. Any non-zero value is accepted — see the module
    /// docs for why there is no upper bound. Changing the target clears
    /// existing pacing gates so the new policy applies to already-pending
    /// work immediately (a target *increase* must not keep throttling a
    /// request made under the old gate).
    pub fn set_fps_target(&mut self, target: Option<NonZeroU32>) {
        self.fps_target = target;
        self.pacing_gate.clear();
    }

    /// Remove pacing; equivalent to `set_fps_target(None)`.
    pub fn clear_fps_target(&mut self) {
        self.set_fps_target(None);
    }

    pub fn fps_target(&self) -> Option<NonZeroU32> {
        self.fps_target
    }

    pub fn min_frame_interval(&self) -> Option<Duration> {
        self.fps_target.map(|fps| {
            let ideal = Duration::from_secs_f64(1.0 / f64::from(fps.get()));
            ideal.max(MIN_FRAME_INTERVAL)
        })
    }

    pub fn set_max_in_flight(&mut self, n: u32) {
        self.max_in_flight = n.max(1);
    }

    /// Request a frame for `window`. Merges reasons; repeated requests
    /// before production coalesce (counted in [`FrameStats::coalesced`]).
    pub fn request_frame(&mut self, window: AquaWindowId, reason: FrameReason) {
        self.stats.requested += 1;
        match self.pending.get_mut(&window) {
            Some(p) => {
                p.reasons |= reason;
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

    /// Register an animation/media deadline. Creates pending work even with
    /// no damage so [`next_wakeup`](Self::next_wakeup) wakes for it; an
    /// animation-*only* window becomes ready once `now >= deadline`.
    /// (Damage combined with a future deadline is drawable immediately —
    /// the deadline only gates pure animation ticks.)
    pub fn request_animation_deadline(&mut self, window: AquaWindowId, deadline: Instant) {
        self.stats.requested += 1;
        match self.pending.get_mut(&window) {
            Some(p) => {
                p.reasons |= FrameReason::ANIMATION;
                p.deadline = Some(match p.deadline {
                    Some(d) => d.min(deadline),
                    None => deadline,
                });
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

    pub fn pending_reasons(&self, window: AquaWindowId) -> Option<FrameReason> {
        self.pending.get(&window).map(|p| p.reasons)
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    fn gate_blocks(&self, id: AquaWindowId, now: Instant) -> bool {
        match self.pacing_gate.get(&id) {
            Some(gate) => now < *gate,
            None => false,
        }
    }

    /// Windows ready to draw at `now`: pending, past the pacing gate and any
    /// animation-only deadline, and under the in-flight cap. Sorted by id
    /// for deterministic tests. Does NOT remove them; call
    /// [`mark_produced`](Self::mark_produced) after submitting.
    pub fn ready_windows(&mut self, now: Instant) -> Vec<(AquaWindowId, FrameReason)> {
        let mut ready = Vec::new();
        let mut deferred = 0u64;
        // Deterministic order.
        let mut ids: Vec<AquaWindowId> = self.pending.keys().copied().collect();
        ids.sort();
        for id in ids {
            let p = &self.pending[&id];
            if self.gate_blocks(id, now) {
                continue;
            }
            if p.reasons == FrameReason::ANIMATION {
                match p.deadline {
                    Some(d) if now < d => continue,
                    _ => {}
                }
            }
            let flying = self.in_flight.get(&id).copied().unwrap_or(0);
            if flying >= self.max_in_flight {
                deferred += 1;
                continue;
            }
            ready.push((id, p.reasons));
        }
        self.stats.deferred_backpressure += deferred;
        ready
    }

    /// Record that frames were submitted: clears pending reasons, opens an
    /// in-flight slot, and arms the pacing gate for the next frame. Each
    /// produced window must later call [`mark_presented`](Self::mark_presented).
    pub fn mark_produced(&mut self, windows: &[AquaWindowId], now: Instant) {
        let interval = self.min_frame_interval();
        for id in windows {
            if self.pending.remove(id).is_some() {
                self.stats.produced += 1;
            }
            *self.in_flight.entry(*id).or_insert(0) += 1;
            if let Some(dt) = interval {
                self.pacing_gate.insert(*id, now + dt);
            }
        }
    }

    /// Record GPU presentation; releases backpressure slots.
    ///
    /// MUST be wired to actual presentation completion, not to submission:
    /// calling it immediately after queueing `present()` bounds a nominal
    /// counter while real frames pile up in the driver. `wgpu` surfaces
    /// expose no present-completion callback, so the earliest honest signal
    /// is the NEXT successful frame acquire (which fences the previous
    /// present on every backend) — or `mark_presented` on the redraw path
    /// only after `acquire_and_draw` reports the previous frame presented.
    /// See `docs/SCHEDULER.md` for the wiring pattern.
    pub fn mark_presented(&mut self, window: AquaWindowId) {
        if let Some(n) = self.in_flight.get_mut(&window) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.in_flight.remove(&window);
            }
        }
    }

    /// When the event loop should wake next, or `None` for no *drawable*
    /// work (idle sleep — no ticks, and no busy-loop under backpressure).
    ///
    /// Rules, in order per pending window:
    ///
    /// 1. Windows at the in-flight cap are ignored here: their wake-up is
    ///    the completion event (`mark_presented`), never a timer. Waking
    ///    for them would spin `ready_windows` -> empty -> wake.
    /// 2. Animation-only windows wake at `max(pacing gate, deadline)`:
    ///    waking at the gate alone would fire one avoidable early wakeup
    ///    and then wait for the deadline anyway.
    /// 3. Damage (or due animation) wakes at the pacing gate when gated,
    ///    else immediately.
    ///
    /// Other drawable windows are unaffected by one blocked window.
    pub fn next_wakeup(&self, now: Instant) -> Option<Instant> {
        let mut best: Option<Instant> = None;
        let mut consider = |t: Instant| {
            best = Some(match best {
                Some(b) => b.min(t),
                None => t,
            });
        };
        for (id, p) in &self.pending {
            // (1) Backpressure: completion wakes this window, not a timer.
            let flying = self.in_flight.get(id).copied().unwrap_or(0);
            if flying >= self.max_in_flight {
                continue;
            }
            if p.reasons == FrameReason::ANIMATION {
                match p.deadline {
                    Some(d) if d > now => {
                        // (2) Fuse gate + deadline into one wakeup.
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
            // (3) Damage or due animation: gated or immediate.
            match self.pacing_gate.get(id) {
                Some(g) if *g > now => consider(*g),
                _ => return Some(now),
            }
        }
        best
    }

    /// Drop all state for a closed window.
    pub fn remove_window(&mut self, window: AquaWindowId) {
        self.pending.remove(&window);
        self.in_flight.remove(&window);
        self.pacing_gate.remove(&window);
    }

    pub fn stats(&self) -> &FrameStats {
        &self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> AquaWindowId {
        AquaWindowId::new(n).unwrap()
    }

    /// Fixed clock: tests supply every timestamp, never the OS clock.
    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn fps_target_accepts_any_nonzero_clears_to_unpaced() {
        let mut s = Scheduler::new();
        assert_eq!(s.fps_target(), None);
        s.set_fps_target(Some(NonZeroU32::new(60).unwrap()));
        assert_eq!(s.fps_target().unwrap().get(), 60);
        // No arbitrary upper bound: a high target is legal; throughput stays
        // bounded by vsync/presentation policy + backpressure instead.
        s.set_fps_target(Some(NonZeroU32::new(10_000).unwrap()));
        assert_eq!(s.fps_target().unwrap().get(), 10_000);
        // Interval clamp keeps exotic targets usable (never zero).
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
        // Not ready before the deadline...
        assert!(s.ready_windows(now).is_empty());
        // ...but the loop knows when to wake.
        assert_eq!(s.next_wakeup(now), Some(deadline));
        // Ready after the deadline.
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
        // One frame in flight; new damage must defer, not queue behind it.
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
        // Re-request immediately: pacing gate (~16.6ms) blocks.
        s.request_frame(id(4), FrameReason::DAMAGE);
        assert!(s.ready_windows(start).is_empty());
        let wake = s
            .next_wakeup(start)
            .expect("pacing gate must schedule a wakeup");
        assert!(wake > start);
        // Deterministic runtime change to unpaced releases the OLD gate:
        // the already-pending request becomes drawable at the same `now`.
        s.clear_fps_target();
        let ready = s.ready_windows(start);
        assert_eq!(ready.len(), 1, "target change must release stale gate");
        // And raising the target later re-arms pacing from the new produce.
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
        // Window 11 still pending; window 10 drained.
        assert_eq!(s.pending_reasons(id(10)), None);
        assert!(s.pending_reasons(id(11)).is_some());
        s.remove_window(id(11));
        assert!(s.pending_reasons(id(11)).is_none());
    }

    /// Regression: a backpressure-blocked window must NOT schedule a wakeup.
    /// Before the fix, `next_wakeup` returned `Some(now)` for the blocked
    /// window while `ready_windows` yielded nothing — a timer busy-loop.
    /// Completion (`mark_presented`), not a timer, wakes it.
    #[test]
    fn blocked_window_sleeps_until_completion() {
        let mut s = Scheduler::new();
        s.set_max_in_flight(1);
        let now = t0();
        s.request_frame(id(20), FrameReason::DAMAGE);
        s.mark_produced(&[id(20)], now);
        // New damage arrives while the frame is still in flight.
        s.request_frame(id(20), FrameReason::DAMAGE);
        assert!(s.ready_windows(now).is_empty());
        assert_eq!(
            s.next_wakeup(now),
            None,
            "blocked window must sleep; completion wakes it"
        );
        // A completion event re-arms the window immediately.
        s.mark_presented(id(20));
        assert_eq!(s.next_wakeup(now), Some(now));
        assert_eq!(s.ready_windows(now).len(), 1);
    }

    /// One blocked window must not silence other drawable windows.
    #[test]
    fn blocked_window_does_not_starve_others() {
        let mut s = Scheduler::new();
        s.set_max_in_flight(1);
        let now = t0();
        s.request_frame(id(21), FrameReason::DAMAGE);
        s.mark_produced(&[id(21)], now);
        s.request_frame(id(21), FrameReason::DAMAGE); // blocked
        s.request_frame(id(22), FrameReason::INPUT); // free
        assert_eq!(s.next_wakeup(now), Some(now));
        let ready = s.ready_windows(now);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].0, id(22));
    }

    /// Gate + future animation deadline fuse into ONE wakeup at the max:
    /// waking at the gate alone would fire, find the animation unready,
    /// and wait for the deadline anyway.
    #[test]
    fn animation_wakeup_fuses_gate_and_deadline() {
        let mut s = Scheduler::new();
        s.set_fps_target(Some(NonZeroU32::new(60).unwrap()));
        let start = t0();
        // Produce once to arm a pacing gate, then register animation work
        // whose deadline lands well after the gate.
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
        // And a deadline BEFORE the gate still waits for the gate.
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

    /// Overload recovery: a produce storm under a tight cap coalesces to one
    /// in-flight frame; completions drain exactly what was produced and the
    /// app settles back to idle (no leftover pending, no wakeup).
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
        // Storm continues while saturated: still exactly one slot.
        for _ in 0..1000 {
            s.request_frame(id(30), FrameReason::DAMAGE);
        }
        assert!(s.ready_windows(now).is_empty());
        s.mark_presented(id(30));
        // Drain: one more frame covers ALL coalesced damage, then idle.
        assert_eq!(s.ready_windows(now).len(), 1);
        s.mark_produced(&[id(30)], now);
        s.mark_presented(id(30));
        assert!(!s.has_pending());
        assert_eq!(s.next_wakeup(now), None);
        assert_eq!(s.stats().produced, 2);
    }

    /// Resize storms (compositor sends many in one burst) produce exactly
    /// one frame carrying the merged reason.
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

    /// Closing a window mid-flight releases its slot without panicking and
    /// without leaking it into another window's accounting; a late
    /// `mark_presented` for the removed window is a safe no-op.
    #[test]
    fn remove_window_during_flight_no_leak() {
        let mut s = Scheduler::new();
        s.set_max_in_flight(1);
        let now = t0();
        s.request_frame(id(32), FrameReason::DAMAGE);
        s.mark_produced(&[id(32)], now);
        s.remove_window(id(32));
        s.mark_presented(id(32)); // late completion: must not panic/underflow
        s.request_frame(id(32), FrameReason::DAMAGE);
        assert_eq!(s.ready_windows(now).len(), 1, "fresh state after removal");
    }

    /// `mark_presented` without production saturates at zero (no underflow
    /// into a huge count that would wedge the cap check).
    #[test]
    fn presented_without_produce_saturates() {
        let mut s = Scheduler::new();
        let now = t0();
        s.mark_presented(id(33));
        s.mark_presented(id(33));
        s.request_frame(id(33), FrameReason::DAMAGE);
        assert_eq!(s.ready_windows(now).len(), 1);
    }

    /// Deadline supersede: earliest wins regardless of registration order;
    /// re-registering an earlier animation pulls the wakeup forward.
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
        s.request_animation_deadline(id(34), late); // re-adding later changes nothing
        assert_eq!(s.next_wakeup(now), Some(early));
    }
}
