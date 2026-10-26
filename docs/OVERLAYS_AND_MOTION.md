# Overlays and motion (`aqua-ui/src/overlay.rs`, `aqua-ui/src/motion.rs`,
# `aqua-ui/src/overlay_host.rs`)

Owner: overlays agent. Status: **implemented, tested** (44 overlay/motion
unit + 1 overlay-motion integration + 8 host integration tests green in an
isolated harness; in-workspace `cargo test -p aqua-ui` is currently blocked
by unrelated peer-owned `binding.rs`/`element.rs` compile errors — see §9).

## 1. What these modules own

- `overlay.rs` — the window-level overlay service: anchoring with
  placement fallback, viewport containment, stacking, logical ownership vs
  rendered placement, lifecycle, nested dismissal, Escape / outside-press
  dismissal, modal input blocking, focus save/restore, owner disposal.
- `motion.rs` — the ONE motion model: explicit targets, property
  ownership, playback state, retarget-from-current, cancellation /
  completion, reduced-motion policy, and scheduler deadlines. There is no
  second animation system in `aqua-ui`; do not add one.

Both modules are tree-agnostic and headless: no `winit`/`wgpu`/`taffy`
dependency. Callers supply committed geometry, a two-method focus adapter,
and timestamps.

## 2. Overlay service

### 2.1 Logical ownership vs rendered placement

An overlay is *owned* by a tree node (`OverlayRequest::owner`) for context
and cleanup, but *rendered* in a window-level host above the main scene.
The service never reparents tree nodes and never touches ancestor clips:
[`OverlayService::render_plan`] emits bottom→top `RenderEntry { id,
owner, bounds, modal, host_z }` records; `host_z` is the position inside
the overlay host layer (which sits above the main scene), not a global
z-value competing with application content. No giant-z or clip-escape
hacks exist here. Escaping the application window itself (native popups)
is out of scope — that needs the platform adapter, see §8.

### 2.2 Placement (`place_overlay`, pure + deterministic)

1. Position on the preferred side (or centered, for dialogs).
2. Shift along the cross axis (best-effort alignment, containment wins).
3. On main-axis overflow with `flip: true`, retry the opposite side.
4. Clamp the origin into the viewport margin (containment). Oversized
   overlays pin top-left-visible.

Fitting is judged on the **unclamped** side position; the winner alone is
clamped. (Clamping first would force every side to "fit" and silently
defeat the fallback — this was a real bug caught by the flip test.)
Degenerate inputs yield an empty rect at the viewport origin, never NaN.

### 2.3 Dismissal policy

| Input | Rule |
|---|---|
| Press inside an overlay | `Captured` (topmost under the point), unless a modal below it blocks |
| Press outside, modal open | `BlockedByModal` (nothing closes; the app owns scrim behavior) |
| Press outside, no modal | Topmost overlay decides: declines → `Ignored`, chain stays open. Accepts → walk up through ancestors that also accept and close the **outermost** of that chain with `OutsidePress` (descendants: `AncestorClosed`). One ambient press dismisses a menu hierarchy, never a sticky palette |
| `Escape` | Topmost overlay **only**. Declines → `Ignored` (Escape never skips the stack). Accepts → closes with its descendants (`Escape` / `AncestorClosed`). One level per press: closing a submenu leaves its parent open |
| Scroll | Routed like a press but never dismisses (`DeliveredToOverlay` / `BlockedByModal` / `Ignored`) |

`close` is idempotent; unknown handles close nothing. `dispose_owner`
closes every overlay of an owner node (each cascades), reporting
`OwnerRemoved` for the directly owned entries.

### 2.4 Focus (`FocusControl`)

Two-method adapter; the tree's focus scope implements it. Open with
`autofocus: Some(n)` saves the current focus and moves to `n`. Close
restores the saved focus **only if focus is still on `n`** — a user who
tabbed away is never yanked back. Cascaded closes restore deepest-first,
so a child correctly unwinds to the focus its parent had set. Keyboard
focus *trapping* inside modals belongs to the tree's focus scope (it owns
Tab order); the service reports `modal_blocker()` / `CapturedByModal` so
the scope knows what to trap against.

### 2.5 API surface (`aqua-ui/src/overlay.rs`)

`OverlayService::new/set_viewport/open/close/dispose_owner/reposition/
is_open/topmost/stack/bounds/side_used/owner_of/overlay_at/
modal_blocker/render_plan/handle_pointer_down/route_scroll/handle_key/
handle_input`, `place_overlay`, `FocusControl`, `OverlayRequest`,
`OverlayId` (generational via `aqua_core::Arena`), `RenderEntry`,
`PointerOutcome/ScrollOutcome/KeyOutcome/InputOutcome`, `CloseReason`,
`ClosedOverlay`, `OverlayKind/Modality/DismissPolicy/Side/Align/
Placement/PlacedOverlay/OverlayError`.

## 3. Motion model

### 3.1 Clock

Time is caller-supplied `Seconds` (`f64`) — never read from a real clock.
`f64` (not integer milliseconds) because 1 ms already quantizes a fast
update cadence and sub-millisecond tween times must stay representable.
`Duration` conveniences exist on both types (`sample_duration`,
`set_target_duration`, `advance_duration`, `next_deadline_duration`,
…) and convert losslessly via `Duration::as_secs_f64`. Non-finite inputs
collapse to the last sampled time; callers must pass finite values.
Monotonic: earlier timestamps are treated as the last time.

Springs integrate at fixed 1 ms substeps (stability) plus one exact
fractional remainder substep, so the sample always lands on the exact
elapsed time — no 1 ms quantization of the presentation value (regression
test: one 1.9 ms jump follows the same substep path as 1 ms + 1.9 ms).

### 3.2 One model, three kinds

`MotionKind::{Tween { duration_secs, easing }, Spring(params), Instant}`.
`MotionValue::{Scalar, Pair, Color(Oklch)}` is the closed animated-value
vocabulary. Tweens blend with `MotionValue::blend`; colors delegate to
`Oklch::lerp` — shortest hue path, neutral endpoints inherit the chromatic
hue (no arbitrary hue swings). Springs integrate per channel with the hue
unwrapped to the shortest path at retarget (channel basis: hue in turns,
so one settle epsilon serves every channel).

### 3.3 Retarget / completion contract

- `set_target` samples the displayed value first: mid-motion reversals
  continue from the visible position, never jump; springs additionally
  preserve velocity. Variant mismatch is `Err(VariantMismatch)`, no panic.
- Completion callbacks bind to the *next target set* (the universal
  `on_complete`-then-`set_target` pattern observes the intended motion).
  A callback bound to a superseded target is **dropped silently** — a
  stale completion can never remove a reopened component. A bound target
  that genuinely lands exactly at a retarget instant still fires
  `Completed`. Explicit `cancel` fires `Cancelled` and freezes; `finish`
  snaps and fires `Completed`. Each callback fires at most once.
- Reduced motion (`Full`/`Reduce`, live-switchable on both `Animated`
  and `MotionController`) collapses in-flight and future motion to
  `Instant` snaps. Product-level fade-vs-move distinctions are separate
  `Animated` values, not a second system.
- Owner disposal (`dispose_owner`) removes properties and drops callbacks
  **silently** — the owner is gone, nothing may observe them. (Confirmed
  per review: silent disposal is the contract.)
- Tween end comparison carries a relative 1e-9 epsilon: f64 addition can
  place an exact end hit (`0.05 + 0.1`) an ulp on either side.

### 3.4 Scheduler wiring (facade owner: this is your interface)

```rust
// Every frame (or on wake):
if let Some(deadline_secs) = motion.next_deadline(now_secs) {
    platform.request_animation_deadline(window, deadline_secs);
    // ... produce frame ...
    for _done in motion.advance(now_secs) { /* completions also fire via callbacks */ }
}
// `next_deadline_duration(now_duration)` is available for Duration timelines.
```

`next_deadline` returns `Some(now)` while anything is unsettled (frame
ASAP), `None` when everything rests — demand-driven, no fixed tick, no
60 Hz assumption. Sampling a property never writes the tree: the owner
commits sampled values during its own update.

### 3.5 API surface (`aqua-ui/src/motion.rs`)

`Animated::new/on_complete/value/target/kind/is_settled/set_target/
set_reduced_motion/sample/sample_duration/set_target_duration/cancel/
finish/next_deadline/tween_end`, `MotionController::new/reduced_motion/
set_reduced_motion/len/is_empty/spawn/set_target/sample/peek/
on_complete/cancel/finish/is_settled/owner_of/next_deadline/
next_deadline_duration/advance/advance_duration/sample_duration/
set_target_duration/remove/dispose_owner`, `MotionHandle` (generational),
`MotionValue/MotionKind/Easing/SpringParams/ReducedMotion/Completion/
MotionError/Seconds/SPRING_MAX_DURATION_SECS`.

## 4. Review responses (applied)

- **f64 seconds clock**: done (`Seconds` + `Duration` conveniences +
  sub-millisecond regression test). No animation-locked tick anywhere.
- **Exact-elapsed springs**: fixed 1 ms substeps + fractional remainder;
  jump-vs-split-path regression test included.
- **Silent owner disposal**: kept, documented in §§2.3/3.3.
- **Facade wiring**: the services expose everything the facade needs
  (§3.4 snippet, `render_plan` + `handle_input`); actual event-loop
  wiring is the facade owner's work — flagged in §8.
- **Tree reconciler**: not my surface (core owner).

## 5. Tests

21 overlay unit tests (placement/flip/shift/oversize/centered/
degenerate; stack/plan/idempotent close; nested close both directions;
stale parent; Escape topmost-only; nested outside-click chain; capture vs
dismiss; sticky survival; modal press+scroll blocking; focus
save/restore/no-clobber/no-autofocus-disturb; owner disposal;
input-router; reposition; viewport resize) + 22 motion unit tests
(tween midpoint/end/easing; retarget continuity; completion-once;
supersede-silence; cancel/finish; variant mismatch; zero-duration;
reduce now + mid-flight; spring converge/retarget/color-settle;
controller deadlines/completions/disposal/unknown-handles/reduce
propagation; sub-ms Duration tween; exact-elapsed spring) + 1
integration test (`tests/overlay_motion_integration.rs`: animated menu
open → mid-flight outside dismiss → focus restore → silent motion
disposal, stepped on a 120 Hz frame loop via `next_deadline`).

Run: `cargo test -p aqua-ui` (currently blocked by §9 — use the harness
below until core lands a fix). Clippy clean for these modules; `cargo
fmt --check` clean.

## 6. Precision and stability limits (honest)

- Spring settling uses absolute channel epsilons (1e-3 pos, 5e-2/s vel;
  hue in turns); extreme stiffness/damping are clamped (1..5000,
  0..200). Unsettleable springs snap at 8 s.
- `Oklch::lerp` (core) owns hue-path correctness for tweens; springs take
  the linear per-channel path after shortest-path unwrap.
- Modal focus *trap* (Tab cycling) is not implemented here — it needs the
  tree focus scope (core owner, §8).

## 7. What I created / edited (absolute paths)

- Created `crates/aqua-ui/Cargo.toml` (minimal: `aqua-core` only) — later
  superseded by the core owner's version (added `aqua-text`/`taffy`/new
  description); not re-edited.
- Created `crates/aqua-ui/src/overlay.rs`, `crates/aqua-ui/src/motion.rs`
  (owned).
- Created `crates/aqua-ui/src/overlay_host.rs` (owned, §10).
- Created `crates/aqua-ui/tests/overlay_motion_integration.rs` (owned).
- Created this doc (owned).
- Sole permitted peer-file edits: appended `pub mod overlay;` /
  `pub mod motion;` / `pub mod overlay_host;` to core-owned
  `crates/aqua-ui/src/lib.rs` (one line each, as each file landed).
- Did NOT touch `tree.rs`/`layout.rs`/`style.rs`/`events.rs`/
  `element.rs`/`binding.rs`, `Cargo.toml`s, or the contract (core owner
  publishes).

## 8. Interface decisions needing parent relay / contract publication

1. **Core owner**: `pub mod overlay/motion` lines are in `lib.rs`; please
   keep them when restructuring. Needed from the tree (proposed hooks,
   none invented here): committed anchor rects in window-local logical
   units for `reposition`; `FocusControl` impl over the focus scope
   (incl. Tab trap using `modal_blocker`); committing `render_plan`
   entries to the window-level host; per-node `dispose_owner` calls for
   both services on node removal; `ui!` builder names when `element`
   lands (no dependency taken here).
2. **Facade owner**: wire `handle_input` + `render_plan` (overlays) and
   the §3.4 deadline loop (motion) into the event loop; feed
   `Scheduler::request_animation_deadline` from `next_deadline_duration`.
   No facade edits made (not my files).
3. **Platform owner**: `next_deadline_duration: Option<Duration>` is the
   proposed feed type — confirm it matches `request_animation_deadline`.
4. **Contract**: propose adding the §3.4 deadline-feed convention and the
   §2.3 dismissal matrix to `docs/IMPLEMENTATION_CONTRACT.md`
   (core owner publishes; code above is the source of truth).

## 9. Gaps and blockers (no silent success)

- **Blocker (updated)**: `cargo check/test -p aqua-ui` fails with 8
  errors, ALL in peer-owned files: `binding.rs` (5: missing
  `dispose_bound` method, `HashSet`/`HashMap` mismatches, closure arity)
  and `element.rs` (3, incl. the orphaned `flat_children`/`push_flat`
  methods sitting outside any `impl` block — they need wrapping in
  `impl Element`). Zero errors in `overlay.rs`/`motion.rs`/
  `overlay_host.rs`. Not my files — I did not touch them.
- **Verification**: 89 tests pass in an isolated harness crate
  (`/tmp/opencode/host-verify`, path-deps on the shared `aqua-core` +
  `aqua-text`, byte-identical module copies of `tree`/`events`/`layout`/
  `style`/`overlay`/`motion`/`overlay_host`): all pre-existing unit tests
  plus 8 new host integration tests. Re-run `cargo test -p aqua-ui` once
  the peer errors clear.
- Reported core defect (needs core fix, worked around):
  `UiTree::insert_spec` panics inside taffy's `remove_child` for the
  fresh detached child. The host mounts via `insert`+`attach` instead
  (the right primitive for fresh popups anyway); please fix or document
  `insert_spec`'s contract.
- Native (out-of-window) popups, focus trap, tree-driven anchor updates,
  and scene commit of `render_plan` are explicitly unwired (peer-owned).

## 10. OverlayHost (`aqua-ui/src/overlay_host.rs`)

The single bridge between `OverlayService` and the shared `UiTree`. The
facade consumes the host, never the service directly.

- **Mounting**: popups mount as absolute-positioned last children of the
  window root (top layer) via `insert`+`attach` construction, with insets
  from the service placement. Same-tree invariant: popup nodes live in
  the one shared `UiTree`, so `NodeId`s never alias. Popup content roots
  must declare `Px` width/height (`HostError::ContentSizeNotFixed`
  otherwise — content-sized popups are future work, no silent fallback).
- **Anchors**: `visible_rect` = committed owner rect minus ancestor
  scroll offsets. `reanchor_popup`/`reanchor_all` (call after
  `compute_layout`/scroll; `handle_scroll` re-anchors automatically after
  a consumed scroll) and `set_viewport` keep service and tree in sync.
- **Input**: `handle_pointer_down` (service policy → tree press dispatch
  for captured/ignored, mount cleanup on dismissal), `handle_scroll`
  (overlay-first, modal blocks, else tree scroll), `handle_key`
  (topmost-modal Tab trap cycles the modal subtree; Escape dismisses via
  the service), `handle_input` (bool consumed), `overlay_at_point` (hit →
  root-child → overlay attribution for the facade).
- **Lifecycle**: close-first (service restores while nodes live), then
  `tree.remove`, then host-saved focus restore (deepest-first cascade;
  only when nothing live holds focus). `sync_lifecycle` prunes
  owner-gone overlays (`OwnerRemoved`), popup-root-gone overlays, and
  stale mount entries. The host additionally records the focus it moves
  (`saved_focus` per overlay) because `focus_root` targets do not exist
  until after mount — the service can only save around pre-existing
  autofocus nodes.
- **Motion**: the host owns the `MotionController`: `animate_popup`
  (owner = overlay's owner; NOT auto-disposed on close — fade-outs may
  outlive the popup; call `dispose_owner_motion`), `advance_motion`,
  `motion_deadline[_duration]` for the platform scheduler feed.
- **Tests** (8, real `UiTree` + `NaiveMeasurer` + `compute_layout`):
  scroller-child menu mounts at root and escapes clip (hit works past the
  scroller clip box); modal press/scroll blocking; modal Tab trap cycling
  + Escape focus restore; outside-click dismissal with mount cleanup;
  owner removal pruning via `sync_lifecycle`; scroll routing +
  re-anchoring; motion deadline/disposal silence; error paths leave no
  partial state (unknown owner, content-sized root).
