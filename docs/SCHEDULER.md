# Scheduler app-facing guide (`aqua_platform::Scheduler`)

Demand-driven frame scheduling: idle windows schedule **nothing**. This
guide is for application and facade authors; the authoritative contract is
the `scheduler` module docs.

## Core loop

```rust
// On any state/input/animation change:
scheduler.request_frame(id, FrameReason::DAMAGE);

// Animation/media with a known next change:
scheduler.request_animation_deadline(id, next_change_at);

// In the event-loop wait path:
event_loop.set_control_flow(host::control_flow_for_next_wakeup(
    scheduler.next_wakeup(now),
));

// When woken for drawing:
for (id, reasons) in scheduler.ready_windows(now) {
    draw(id, reasons);                    // submit GPU work
    scheduler.mark_produced(&[id], now);
}
// On ACTUAL presentation completion (see below):
scheduler.mark_presented(id);
```

## Frame-rate target

`set_fps_target(Some(NonZero(60)))` paces *produced* frames to `1/fps`
apart per window. It does NOT start a redraw loop: an unchanged app sleeps
at any target, including 900. `None` (default) is unpaced demand-only.
Any nonzero value is accepted (no arbitrary cap); the minimum interval is
clamped to 1us and real throughput stays bounded by vsync, presentation
policy, and backpressure. Changing the target clears pacing gates, so the
new policy applies to already-pending work immediately.

## Deadlines vs damage

- Damage (input, scene change, resize, expose) draws at the next wakeup.
- Pure animation ticks wait for `max(pacing gate, deadline)` — one wakeup,
  never a gate-then-deadline double wake.
- Damage combined with a future animation deadline draws immediately.
- Overload: repeated requests coalesce (`stats().coalesced`); a 1000-event
  storm produces the minimum frames to catch up, then idles.

## Backpressure: the completion rule (read carefully)

`mark_presented` MUST be wired to **actual presentation completion**, not
to submission. Calling it right after queueing `present()` bounds a nominal
counter while real frames pile up in the driver — the in-flight cap then
protects nothing.

`wgpu` exposes no present-completion callback. The earliest honest signal
available today:

1. Call `surface.present()` / finish frame N.
2. Call `mark_presented(id)` for frame N-1 when frame N's `acquire` (or
   `acquire_and_draw` reporting the previous present) succeeds — the
   acquire fences the previous present on every backend (D3D12/Vulkan/Metal).

Until the facade wires this, treat `in_flight` as nominal and keep
`max_in_flight` small (default 2). Windows at the cap are skipped by
`next_wakeup` (their wake is the completion event), so a saturated window
sleeps instead of busy-looping, and other windows keep drawing.

## Reasons and inspector

`pending_reasons(id)` returns the merged `FrameReason` bitflags
(`INPUT|DAMAGE|ANIMATION|MEDIA|EXPOSE|RESIZE|MATERIAL|EXPLICIT`) for the
pending frame — surface these in the inspector with `stats()`
(`requested/produced/coalesced/deferred_backpressure`) and the configured
`fps_target()`.

## Multiwindow and lifecycle

- Scheduling is per window; one saturated window never starves others.
- `remove_window(id)` on close/destroy drops pending, in-flight, and gates;
  a late `mark_presented` after removal is a safe no-op.
- Occluded windows: `surface_signal` reports `skip_production`; don't
  request frames for them (their expose event re-arms).
- Reduced motion (`prefs::should_reduce_motion`): skip purely decorative
  deadlines; functional animation keeps its deadlines.
