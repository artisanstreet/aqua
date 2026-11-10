# Aqua performance: what we measure, what we refuse to claim

## Persistent update contract (2026-11-10)

Property bindings carry source identities and versions into typed node
commands. The runtime emits dirty scene fragments and patches existing scene
items directly. Owned structural regions can replace their scene spans
without emitting unrelated subtrees. Text inspection and shaping follow
changed fragments. Selection descriptors and viewport-cap text use indexes;
collection region layout uses a dirty worklist.

`crates/aqua-runtime/tests/runtime/incremental.rs` compares incremental results with
full layout/scene reconstruction and checks sparse update work with thousands
of unrelated nodes. It also exercises conditional disposal, semantics-only
updates, indexed queries and a 20,000-record virtual conversation. These are
operation-count and behavior contracts, not elapsed-time comparisons.

Explicit fixed-size layout boundaries limit internal recomputation. General
content-sized layout retains genuine ancestor/sibling dependencies. Scene
order vectors and collection key/height snapshots still require linear work
when their topology changes. Legacy arbitrary prepaint callbacks require
full scene emission; global viewport, scale and font invalidation can also
require full work. No constant-cost claim applies to those operations.

Tracing is optional and bounded. It records source versions, property
writes, invalidation, layout commits, text inspection/shaping and scene
patches/rebuilds without creating a permanent history per update.

## Runtime update (2026-11-03)

`Runtime::frame_work()` exposes text nodes inspected, source bytes compared,
runs shaped, base scene operations emitted, scene items reconciled and base
scene reuse. The 1,001-text transcript suite in
`crates/aqua-runtime/tests/frame-work.rs` checks that unchanged frames inspect
zero text nodes, compare zero source bytes, shape zero runs and emit zero base
scene operations. Moving a selection reuses the base scene. Streaming a source
reshapes its changed run and patches its retained scene fragment.

Font loads invalidate metrics and runs. Scene insertion/removal/reordering
preserves surviving node identities. Source-backed selection copies unmounted
rows without realizing their UI. The combined 20,000-message consumer workload
checks bounded realization, source copy, rich streaming updates and reading
anchor preservation after variable-height changes.

These are behavioral and operation-count gates. They do not measure frame-time
distributions or establish a performance advantage over GPUI or Chromium. The
renderer-only timings and implementation costs below are historical evidence;
the old claim that every frame clones/hashes all text no longer applies to
unchanged or selection-only frames.

Owner: verification agent. The design (`GUI_FRAMEWORK_DESIGN.md` §§
"Performance architecture", "Data locality", "Demand-driven scheduling")
requires honest, inspectable efficiency evidence — not benchmark victories.
This file defines the allowed metrics, how to capture them, and the current
numbers. There are no FPS claims and no Chrome comparisons here; per the
design, those require matched workloads (idle, caret blink, local hover,
dense-text scrolling, streaming text, resize, overlapping backdrop blur,
animated 3D viewport) at matched resolution/content/scale — none of which
have been built yet.

## 1. Allowed metrics (all exist in code today)

| Metric | Source | Meaning |
|---|---|---|
| `frames_produced` / `frames_presented` / `frames_skipped_idle` | `RenderDiagnostics` | Rasterized frames, surface presents, and clean plans skipped are separate counters |
| `nodes_total_last` vs `nodes_updated_last` | `RenderDiagnostics` | Scene size vs changed nodes per frame — partial-update proof is the RATIO, not a time |
| `bytes_uploaded_last` / `_total` | `RenderDiagnostics` | Exact owned instance-buffer upload bytes; glyphon atlas traffic is unknown and excluded |
| `upload_ratio_last(scene_bytes)` | `RenderDiagnostics` | `bytes_uploaded_last / scene_bytes` as float |
| `passes_last`, `intermediate_bytes_last` | `RenderDiagnostics` | Draw/effect passes encoded; scratch bytes actually resident |
| `pool_hits/misses/evictions`, `live/cached/allocated_bytes` | `TexturePoolModel` | Residency in BYTES (live + cached + pending-release); budget caps all residents, never just live |
| `requested/produced/coalesced/deferred_backpressure` | `FrameStats` | Demand accounting; backpressure defers instead of queueing |
| Frame-request reasons | Scheduler `FrameReason` | Reasons for requested work; a full trace inspector and missed-deadline analysis remain future tooling |
| `pending_change_count`, revision bumps | `Scene` | One node change = one pending entry, one revision bump; no-op writes free |
| Allocations / bytes traversed / cache-miss profiles | profilers (future) | Per design §"Data locality"; no numbers claimed yet |

Forbidden: invented FPS, aggregate utilization percentages presented as
per-frame timings, any "faster than X" without the matched workloads above.

## 2. Memory categories (contract)

- The intermediate texture pool caps its live and cached resident bytes
  together (`TexturePoolModel::allocated_bytes = live + cached`). This is
  a scratch-pool budget, not a process-wide memory limit. Base/surface
  targets, glyphon atlases, CPU structures, and separately budgeted media
  caches must be accounted for in addition. Pending GPU releases do not
  have a separate measured byte counter.
- A 3840×2160 RGBA8 target alone is about 31.6 MiB before driver overhead.
- CPU arenas keep a sparse high-water mark (`Arena::capacity_slots`);
  removal reuses slots with generation bumps — no dense-compaction or
  constant-time claims.

## 3. How to capture (reproducible)

```sh
# Unit/bound proofs (no GPU):
CARGO_BUILD_JOBS=2 cargo test -p aqua-render resource  # byte-cap, eviction, count-cap
CARGO_BUILD_JOBS=2 cargo test -p aqua-render damage    # damage coverage invariants
CARGO_BUILD_JOBS=2 cargo test -p aqua-render scene     # single-change drain, noop-free
# Offscreen composition + upload metrics (needs lavapipe; skips honestly):
CARGO_BUILD_JOBS=2 cargo test -p aqua-render --test headless
CARGO_BUILD_JOBS=2 cargo test -p aqua --test offscreen  # blocked until aqua-ui compiles
# Scheduler demand accounting:
CARGO_BUILD_JOBS=2 cargo test -p aqua-platform scheduler
# Canonical end-to-end:
scripts/validate.sh
```

GPU tests run serially (`--test-threads=1` in CI) — software Vulkan has one
device; parallel adapters requests flake. On machines with no adapter the
suites print `SKIP … (no adapter)` and pass; that is reported as
**untested-GPU**, never as a pass with pixels.

## 4. Current numbers (measured 2026-10-25, this host)

- Scheduler: 44 platform lib tests green — idle sleeps (`next_wakeup None`),
  pacing gates enforced then released by runtime target change, backpressure
  defers with counter (no queue growth), animation-only waits for deadline.
- Pool/damage/scene unit proofs: render 53 + text-side suites green;
  re-run via `scripts/validate.sh`.
- Renderer prepare (parent-observed, lavapipe, strict headless 12/12):
  23/28/27us across 10/1000/10000 nodes, 128 B upload, 392 px raster,
  draw 447/465/535us. **Renderer fast-path proof ONLY.**
- Offscreen pixels (verification-owned, this host, llvmpipe):
  `artifacts/aqua-ui.png` (480×320, valid PNG): 228 distinct colors,
  1465 non-transparent px, 756 glyph px from a real `ui!` Todo card
  (title, keyed rows, bound field, Button) shaped through the shared
  `TextBackend`. Replaces the failed all-black CopyFromScreen capture;
  that file is labeled failed-capture, never evidence.

## 5. Honesty boundary (binding)

- The prepare numbers above EXCLUDE `UiApp` and describe the historical
  renderer path. The persistent-update contract above supersedes the former
  all-text/all-scene traversal on every content frame. Those old timings do
  not measure the current sparse-update path.
- DO NOT claim: end-to-end constant-cost frames, cache-perfect updates,
  or Chrome parity. No matched-workload comparison exists.
- Still legit: idle windows are demand-driven (scheduler `next_wakeup`
  `None`, clean plans skip presentation); partial scene uploads are
  measured per frame. Those are scheduling/partial-update facts, not
  speed claims.
- No frame-time distributions, allocation profiles, or matched-workload
  comparisons exist yet. Capturing them is future work, not a claim.


## Collection realization (2026-10-26)

`List` and `Table` mount all rows by default. `.virtualize()` opts into
viewport-driven row realization, measured variable heights, overscan, and
isolated focus pinning. A Fenwick index provides offset lookup and height
updates in O(log n); row factories and retained nodes follow the resident
window. Snapshot creation, validation, and structural key-index rebuilds
still visit the dataset. Virtualization does not remove the application's
data or turn snapshot construction into constant work.

The public conformance suite checks 100,000-row lists, 20,000-row tables,
factory/node bounds, large scroll jumps, resize, stable reading anchors,
retained editing state, live subscriptions, eviction cleanup, shared columns,
and clean frames with no new row/header factory calls. The `collections`
example supplies 100,000 rows to each collection without truncation.
These are behavioral bounds, not comparative renderer benchmarks.

Native Windows repeat smoke (1.8 seconds, unchanged release binary):
Collections exited 0 with 6 wakeups, 2 produced frames, 1 presentation,
zero render errors. An earlier run recorded a burst of 660 produced frames
with the same binary; it did not reproduce, and the event source was not
captured. It is not evidence of an established framework idle loop or a
basis for claiming universal idle performance. No speculative runtime change
was made in response.
