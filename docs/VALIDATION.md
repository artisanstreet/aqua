# Aqua validation: acceptance checklist, environment, and evidence

Owner: verification agent. This is the honest ledger for the Aqua
conformance suite. Status words are load-bearing:

- **tested**: an automated check ran and passed on this host; command + count
  recorded below.
- **compilation-only**: `cargo check`/`clippy` pass; no runtime behavior
  asserted.
- **blocked**: cannot run yet; reason + owner named.
- **untested-native**: Windows/macOS native runtime not exercised on this
  Linux host (cross-check compiles only).
- **missing**: capability absent; no claim made.

Nothing here claims an unimplemented feature works. Last updated: 2026-10-26.

## Current result: factories, List/Table, and opt-in virtualization (2026-10-26)

Strict canonical validation: **11 passed, 0 failed, 0 skipped**. This includes
all workspace tests, strict GPU paths, formatting, clippy with warnings denied,
and the complete Windows GNU cross-check. Evidence:
[`collections-validation.txt`](../artifacts/collections-validation.txt).
The macro facade feature suite also passes (21 tests; one doc example ignored).

Canonical authoring uses the existing concrete `Aqua` struct's factories,
`.children()` for arrays/vectors/iterators, and shared const-capable styles.
List/Table mount every row by default; `.virtualize()` enables measured
windowing, stable keys, reading anchors, isolated focus pinning, and disposal.
Table headers stay outside vertical scrolling and share horizontal scrolling,
including when virtualization is off. Header/row/cell styles use normal styles
and a shared track plan. Invalid source/configuration inputs fail explicitly.

Coverage includes **20 public collection conformance tests**, **18 View runtime
tests**, **17 builder/sizing tests**, and **7 facade factory tests**. Additional
regressions cover logical parent links on reconciliation and focus restoration
after subtree removal. The real Todo headless lifecycle passes and its factory
migration preview is byte-identical to the prior UI (SHA-256
`dd295dd43f7e4f28ab4b3822ce1529a04dc7c5e18644b33a60c714c537a77946`).

The Collections demo supplies 100,000 list items and 100,000 table rows without
truncation. Offscreen preview:
[`aqua-collections-final.png`](../artifacts/aqua-collections-final.png).

Both release executables build through Artisan Editor's Nix Windows shell and
run natively with exit 0, DX12/PreMultiplied/Bgra8UnormSrgb, system clipboard,
and zero render errors. The repeat Collections smoke records 6 wakeups,
2 produced frames and 1 presentation over 1.8 seconds. Evidence:
[`collections-native-smoke.log`](../artifacts/collections-native-smoke.log).
An earlier unchanged-binary run had a non-reproduced event burst (source not
captured); its log is retained separately. No speculative runtime change was
made. Native screenshots, screen-reader operation, and comparative performance
are not claimed by these smoke tests. See `PERFORMANCE.md` for cost boundaries.

## Previous result: App builder and native overlay chrome (2026-10-26)

Strict `AQUA_REQUIRE_GPU=1 CARGO_BUILD_JOBS=2 bash scripts/validate.sh`:
**11 passed, 0 failed, 0 skipped**, including GPU tests, workspace clippy,
formatting, and the Windows GNU check. Log:
`artifacts/overlay-validation.txt`.

The canonical `App::new().config(Config::new().window(...)).run(todo_app)`
path is implemented, including static/reactive mounting, early config
errors, effective reduced motion, and scope disposal. Todo uses
`Titlebar::Overlay`: client content under native Windows caption buttons.
`Native` and `Hidden` modes and caption-button enablement are configurable.

Native Windows `scripts/windows-chrome-smoke.ps1` passed with actual
process exit 0 at **120 DPI (125%)**:

- Configured 760×560 logical client is exactly **950×700 physical pixels**;
  configured 480×360 minimum is **600×450**.
- Client origin equals outer top-left: no separate native titlebar inset.
- Native hit testing returns caption drag 2, body 1, resize edge 10, and
  DWM caption buttons 8/9/20.
- Maximize fills the **2560×1380 monitor work area**; restore, minimize,
  restore, and close succeed.

Log: `artifacts/overlay-native-smoke.log`. A separate 1.8-second native
render smoke exits 0 with **one presented frame, zero render errors**,
system clipboard, Acrylic applied, and **DX12 / PreMultiplied /
Bgra8UnormSrgb**. Log: `artifacts/overlay-render-smoke.log`.

An initial native loader failure from comctl32 v6 static imports was fixed
by replacing those imports with a user32 procedure chain. State ownership
survives later subclasses and reentrant destruction; GUI-thread access is
enforced. No downstream application manifest/build script is required for
the chrome adapter. Initial sizing and min/max tracking compensate for the
removed caption inset.

The native checks prove operation, not appearance or screen-reader
behavior. `artifacts/aqua-overlay-layout.png` is the opaque offscreen
layout, without simulated desktop glass or fake native controls. Overlay
currently has a Windows adapter; macOS/Linux report unsupported and retain
their native frame. RTL, auto-hide taskbar behavior, and native AT remain
unverified. Earlier evidence below is historical.

## Previous result: glass Tasks demo (2026-10-26)

Strict `AQUA_REQUIRE_GPU=1 CARGO_BUILD_JOBS=2 bash scripts/validate.sh`:
**11 passed, 0 failed, 0 skipped** after the glass/presentation changes.
Log: `artifacts/glass-validation.txt`. The added GPU regression verifies
encoded-premultiplied presentation on Unorm and sRGB targets, including
half-alpha color and fully transparent pixels. The headless Todo lifecycle
still passes after removal of the eyebrow and secondary copy.

Native Windows smoke (real `.NET Process.ExitCode`): **exit 0**, Acrylic
applied, **DX12 / PreMultiplied / Bgra8UnormSrgb**, one presented frame,
zero rendering errors. The interactive build was opened afterward.
Log: `artifacts/glass-native-smoke.log`. No native visual or AT claim is
made; the layout PNG uses the opaque fallback, not a simulated desktop blur.

## Previous result: typed style builders (2026-10-26)

Strict `AQUA_REQUIRE_GPU=1 CARGO_BUILD_JOBS=2 bash scripts/validate.sh`:
**11 passed, 0 failed, 0 skipped**, after the style-builder integration.
This includes workspace formatting, all package tests, real headless GPU
checks, workspace Clippy with `-D warnings`, and the whole-workspace
Windows GNU cross-check (`--all-targets`). Log: `artifacts/style-validation.txt`.
The Windows result is compilation evidence; no new native Windows run was
performed for this migration.

Actual package pass counts from the final logs:

| Package | Passed |
|---|---:|
| `aqua-core` | 31 |
| `aqua-reactive` | 39 |
| `aqua-platform` | 50 |
| `aqua-text` | 56 |
| `aqua-ui` | 167 |
| `aqua-render` | 74 |
| `aqua` | 101 |

Additional checks: the Todo headless lifecycle (type → Add → toggle →
Remove) passes, and its 760×720 real Vulkan/llvmpipe preview is pixel-identical
to the pre-migration render (0 changed pixels, maximum channel difference 0).

Style coverage includes const construction; per-property merge and explicit
clearing; actual repeated builder calls and direct-setter ordering; macro
parity; inherited typography at mount and refresh; keyed identity, focus and
input retention; generic primitive paint without wrappers; border-only
surfaces; and caret/selection opacity. See `STYLING.md` for supported units
and the explicit error for unequal corner radii. The existing native and
performance evidence below remains historical and is not a new claim.

## Historical canonical result (2026-10-25)


Strict `AQUA_REQUIRE_GPU=1 bash scripts/validate.sh`:
**11 passed, 0 failed, 0 skipped.** All sources frozen. Plus macro
`--features aqua-integration`: 21 pass (3 extra unique). Canonical package
counts from actual `test result` log regex: core 30, reactive 39, platform
50, text 56, macros 18, UI 121 (120 lib + 1 integration), render 73
(58 lib + 15 headless), aqua 85 (13 lib + 11 reactive_integration + 15
damage_scheduler + 6 keyed_rows + 12 keyed_tree + 6 facade_pump + 14
overlay_focus + 3 offscreen + 5 ui_app) = **472**, plus 3 feature tests =
**475 unique executed passes**; macro doc 1 ignored. (An earlier 95/85
mixup was the parent's arithmetic error, corrected: actuals sum to 85
for `-p aqua`, 472/475 overall.)

Known limits (not pass/fail):
- Native high-level Todo run (ui! → UiApp → GPU, Win11): EXIT 0,
  produced 2 / presented 1 / deferred 0 / errors 0 — counters only, no
  appearance or AT claims.
- `artifacts/aqua-ui.png` (480×320, 228 colors, 756 glyph px, real `ui!`
  Todo card via shared `TextBackend`) is the honest offscreen artifact. The
  earlier CopyFromScreen `windows-showcase.png` is ALL BLACK (failed
  capture) and is NEVER visual evidence.
- Performance: renderer prepare fast-path only (23/28/27us); UiApp host
  linear traversals remain an optimization gap (see `docs/PERFORMANCE.md`
  §5). No end-to-end, cache-perfect, or Chrome-parity claims.
- README todo-CLI default + outdated Windows-GPU pending row: parent-fixed
  docs-only.
- Historical blockers (all resolved, kept for the record): aqua-ui 38→22→1
  compile errors, render viewport delimiter + E0308 + E0061 churn, facade
  runtime fixture errors, GPU cubic-coalescer hang (bounded-coalescer fix
  landed, strict headless 15/15 green), backpressure busy-wake (rule 1
  landed), reorder edit-state clobber (core fix landed).

## 1. Environment / toolchain report (measured 2026-10-25)

| Item | Observed |
|---|---|
| `rustc` / `cargo` | 1.98.1 |
| Host target | `x86_64-unknown-linux-gnu` (installed) |
| Windows target | `x86_64-pc-windows-gnu` **installed** — cross-check available |
| `DISPLAY` / `WAYLAND_DISPLAY` | unset in agent shell; sockets exist: `/tmp/.X11-unix/X0`, `/run/user/1001/wayland-0` (+ `.lock`) → WSLg smoke possible with `DISPLAY=:0 WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1001` |
| Vulkan ICDs | `lvp_icd.json` present (SoftwareVulkan lavapipe) + intel/nouveau/radeon/virtio/gfxstream; `vulkaninfo` not installed |
| Workspace members | 7 crates (`aqua`, `aqua-core`, `aqua-reactive`, `aqua-ui`, `aqua-text`, `aqua-render`, `aqua-platform`); `aqua-macros` removed 2026-10-29 |
| Disk / RAM | 455 GB free disk, 15 GB RAM; shared `target/` 3.9 GB — sufficient; do not duplicate the workspace for isolated copies |
| Agreed versions | `wgpu 30` / `glyphon 0.12` / `cosmic-text 0.19` / `winit 0.30` (stable, NOT 0.31 beta) / `taffy 0.15` — see `docs/IMPLEMENTATION_CONTRACT.md` |

## 2. Acceptance checklist (from `GUI_FRAMEWORK_DESIGN.md` + `IMPLEMENTATION_PLAN.md`)

| # | Requirement | Check | Status |
|---|---|---|---|
| 1 | Explicit reactive updates, no implicit subscription | `reactive_integration.rs::derived_observes_only_declared_inputs` | **tested** (final canonical green) |
| 2 | Disposal cascades, stale handles fail closed | `reactive_integration.rs::dispose_cascade…`, `stale_writes_fail_closed…` | **tested** |
| 3 | Coherent batches (one effect run, latest values) | `reactive_integration.rs::batch_coalesces…` | **tested** |
| 4 | Glitch-free diamond propagation | `reactive_integration.rs::diamond_propagates_glitch_free` | **tested** |
| 5 | Dynamic `select` re-subscription | `reactive_integration.rs::select_resubscribes…` | **tested** |
| 6 | Manual flush + scheduler wake callback | `reactive_integration.rs::manual_flush…` | **tested** |
| 7 | Retained identity keyed rows, reorder-safe | `keyed_rows.rs` (6) + `keyed_tree.rs` (12, real `UiTree::sync_children`) | **tested** (reorder-clobber fixed by core) |
| 8 | Text editing Unicode (grapheme/ZWJ/IME/undo) | peer `aqua-text` suite (56) + `keyed_tree.rs` IME/input tests + `ui_app.rs` bound field | **tested** |
| 9 | Focus save/restore, overlay lifecycle | `overlay_focus.rs` (14) | **tested** |
| 10 | Incremental damage (old+new footprints, full on structural/scale) | `damage_scheduler.rs` (scene + damage) | **tested** |
| 11 | Scheduler idle sleep / runtime target change | `damage_scheduler.rs` + platform suite (50) | **tested** |
| 12 | No busy-spin under backpressure | `damage_scheduler.rs::backpressure_wakeup_sleeps_until_present` | **tested** (rule 1 landed) |
| 13 | Texture residency capped in bytes (live+cached) | `damage_scheduler.rs` (pool tests) | **tested** |
| 14 | Offscreen composition + pixel checks + upload metrics | `offscreen.rs` (3, lavapipe) + render `headless` (15) | **tested** (strict; AdapterNotFound-only skip) |
| 15 | Facade headless pump (dispatch → schedule → scene) | `facade_pump.rs` (6) | **tested** |
| 16 | WSLg/native GUI smoke (finite frames) | Win11 .NET runs (platform + showcase + Todo): EXIT 0, counters logged | **tested** (counters only; no visuals) |
| 17 | Offscreen PNG artifact + pixel checks | `artifacts/aqua-ui.png` via `ui_app.rs` (shared backend, 756 glyph px) | **tested** (CopyFromScreen capture labeled failed, never evidence) |
| 18 | Windows native runtime | MinGW builds + .NET runs, EXIT 0 | **tested** (runtime counters; appearance/AT unclaimed) |
| 19 | Clip/group nesting, image/media, Viewport3D custom passes | render/media suites (73 total w/ headless) | **tested** per final canonical (see §3) |
| 20 | `UiApp` tree/macro/binding | `ui_app.rs` (5: mount, bind, refresh, dispose, artifact) + `real_mount`/`ui_facade` macro suites | **tested** |

## 3. Evidence log (commands actually run, 2026-10-25)

All from the shared workspace root, `CARGO_BUILD_JOBS=2` (ten agents share
one `target/` lock):

```
cargo test -p aqua-core
  28 lib passed, 0 failed
cargo test -p aqua-reactive
  4 + 7 + 8 + 7 integration + 1 doc = 27 passed, 0 failed
  (batching / disposal / effects / propagation suites)
  Phase 2 FROZEN (37 tests per parent incl. resource/worker examples).
  Claimed benchmark (0 alloc/update, 48–58ns/affected node) NOT cited here:
  validate first via `cargo run -p aqua-reactive --release --example scaling`
  on this host with hardware label when the build lock is free.
cargo test -p aqua-platform
  44 lib passed, 0 failed
  (scheduler pacing/merging/deadlines/backpressure/multiwindow/runtime-target-change;
   event/key/IME/coordinate translation; materials; clipboard/IME-area/DnD/host)
  FROZEN 2026-10-25: platform owner reports 50 tests, Linux +
  Windows-GNU --all-targets zero warnings (parent to relay build log).
  Core FROZEN: 28 tests valid. My test files' event/window constructors
  (PointerEvent/KeyEvent/WindowEvent/ObservedWindowState fields, variants)
  verified public against frozen sources — no private-fields fix needed.
cargo test -p aqua-text
  34 unit + 3 integration + 1 doc = 38 passed, 0 failed
  (shaping, bidi, emoji/ZWJ/extend, multiline, wrap, selection, IME, undo, cache)
cargo test -p aqua-render -p aqua-text
  BLOCKED 2026-10-25: render lib does not parse — unclosed delimiter at
  crates/aqua-render/src/viewport.rs:2082 (impl opened line 1186); render
  owner mid-edit. Baseline (parent-observed): 23 unit + 5 GPU passed.
cargo check -p aqua-ui
  22 errors remaining (was 38): taffy-0.15 API misuse (AUTO/Align* consts,
  to_lp/to_lpa), aqua-core import paths (RawId, NodeId privacy, Point
  Default), Default-derivation failures. Core/UI owner fixing.
  Parent-observed PASS 2026-10-25: 81 unit + 1 integration (incl. keyed
  focus/caret, paint-only layout-skip, button cancel) — labeled precisely:
  at that check the lib did NOT yet export element/binding, so the 81 are
  NOT proof the builder works. Macro agent now enabling + reactive dep.
cargo test -p aqua-macros
  passed, 0 failed
cargo check -p aqua-platform --target x86_64-pc-windows-gnu
  PASS (Windows cross-check compiles; native runtime NOT run here)
cargo fmt --check
  FAIL: crates/aqua/examples/showcase.rs needs formatting (facade owner)
cargo clippy -p aqua-core -p aqua-reactive -p aqua-platform -p aqua-text -p aqua-macros
  FAIL: aqua-core lib has 33 clippy errors (float excessive precision,
  `oklch` constructor naming) — core owner
scripts/validate.sh (2026-10-25): 6 passed, 3 failed, 1 skipped
  (fails = fmt/showcase, clippy/core, render parse; skip = -p aqua on UI build)
  Review defects fixed 2026-10-25: CI uses `shell: bash` + portable
  multi-crate test invocation; aqua-ui included in canonical tests; compile
  errors are FAIL (only missing GPU hardware may SKIP); render/facade tests
  run with `-- --nocapture` and match the specific `no usable GPU adapter`
  string with `AQUA_REQUIRE_GPU=1` strict mode; clippy is
  `--workspace --all-targets`; `--offscreen-png` removed until a PNG
  producer exists; Windows check is whole-workspace `--all-targets`.
  Native Windows note: parent-owned runs only (no independent native build
  here). SMOKE RAN 2026-10-25 on host Windows 11 build 26300 via
  PowerShell: `/mnt/c/Users/sander/AppData/Local/Temp/aqua-smoke/
  platform.out.log` (5 lines, read verbatim) + empty err log, shell exit 0:
  `a11y attached`; prefs Off/Off/Off/theme Unknown via WindowsSystem,
  material Mica; `material capability … supported: true, active: Mica`
  with source note "verify visually: needs transparent window + alpha
  surface"; `dark titlebar applied`; `budget reached (3 frames)`.
  PROVES: windowing opens, native adapter applies DWM Mica + dark titlebar
  calls without error, finite-frame budget exits clean. DOES NOT prove:
  the example performs NO GPU drawing (explicit source comment), so no
  pixels, no screen-reader verification, no material *visual* verification.
  GPU status: 12 root tests running, compose 28-vs-32 ABI fail (graph owner
  fixing); media/viewport depth+compute CPU 19 pass, 5 composition blocked
  until fix. UI actual bindings still pending.
cargo test -p aqua  (verification suite: 6 files, 30 tests)
  BLOCKED: aqua lib depends on aqua-ui. Runs the moment UI compiles; no
  test edits needed. UI-independent files additionally hold peer-level
  coverage (items 11–13).
  2026-10-25 refresh (`cargo check -p aqua --all-targets
  --message-format=short`): all 8 lib crates compile EXCEPT facade
  `aqua/src/runtime.rs` — 4 errors, all facade-owned, relayed to facade:
  E0425 `observed` not in scope at 692:29, 693:31, 695:23 (lib), plus
  E0614 `i32` deref at 1482:64 (unit fixture, cf. parent-reported 1420).
  UI stray delimiter gone; real builders/binding compile. Re-run root
  tests once facade fixes; no test edits planned (maintain, not zero).
  Native re-run: .NET Process confirms actual NATIVE_EXIT=0 (not shell 0).
  FULL NATIVE GPU RUN 2026-10-25 (parent-built MinGW + pthread showcase
  exe, Win11 .NET Process 2 s, AQUA_SMOKE_MS=2000): NATIVE_EXIT=0; actual
  GPU surface/draw 2 frames; requested 4, coalesced 2, wakeups 5
  (+duplicate exit report 6); deferred 0; render_errors 0; system
  clipboard backend; material None.
  HIGH-LEVEL TODO NATIVE RUN 2026-10-25 (ui! → UiApp → GPU, Windows GNU
  exe, .NET Process, AQUA_SMOKE_MS=2000): EXIT 0; wakeups 5, requested 4,
  produced 2, presented_frames 1, coalesced 2, deferred 0, render_errors
  0; system clipboard; second frame clean-skipped. No appearance/AT
  claims — counters only.
  Renderer FINAL: 58 lib + 15 headless; external-map prune on structural
  removal with test (ExternalFrame unregister / autocleanup landed). PROVES: full-framework native GPU
  window run succeeds end-to-end (exit/errors/present counters valid).
  SCREENSHOT FAILED: .NET CopyFromScreen scoped via GetWindowRect saved
  artifacts/windows-showcase.png ALL BLACK including title bar (viewed by
  parent) — capture/session limitation, NOT visual evidence. Do not claim
  app appearance validated. Replaced by planned UiApp offscreen PNG
  (ui! → UiApp/UiTree → real text backend → readback → PNG); UiApp owner
  still typing (`ui_app.rs` 19 pub fns, not yet exported in
  `aqua/src/lib.rs`), facade lib currently broken (6 errors incl.
  `ensure_completion_worker`), so the artifact test stays pending.
  2026-10-25 later: facade lib errors FIXED by facade owner; UI at 112
  tests / 111 pass — `rebind_same_key_switches_source` FAILS (macro owner
  fixing); root nest/leak/cancel/overlay-host tests PASS; core
  insert_spec regression PASS. My own E0382 (`parent` used after
  `dispose()` in reactive_integration.rs) FIXED via pre-dispose clone
  probe — but the file on disk contained an unexpected `parent_clone`
  identifier I did not write (possible cross-edit; flagged to parent).
  My suite first full run 2026-10-25: 68 passed, 0 failed across all 7
  binaries (reactive 13, damage 15, keyed_rows 6, facade 6, offscreen 3 on
  llvmpipe, overlay 14, aqua lib 11) — including two of my own test bugs
  found and fixed (residency eviction sequence; frames_presented now
  counts surface presents, offscreen draws assert 0). Strict-skip fix
  2026-10-25: `offscreen.rs::headless_or_skip` skipped on ANY init error —
  now returns `None` ONLY for `RenderError::AdapterNotFound` with
  `AQUA_REQUIRE_GPU!=1` (explicit `SKIP-NO-ADAPTER` marker), panics
  otherwise; scripts map the marker to honest SKIP (never a pass) and to
  FAIL under `AQUA_REQUIRE_GPU=1`. The UiApp PNG artifact test will reuse
  this exact helper contract when `UiApp` exports. New
  `keyed_tree.rs` (12 real-UiTree tests) first run 9/12; 3 failures all
  my caret-position assumptions (fresh editors: caret at 0), fixed via
  explicit move_to_end; rerun BLOCKED by facade lib breakage (5 errors:
  Loop<> missing fields, wgpu PollType::Wait variant — facade mid-edit).
  Earlier render E0308 resolved.
  `crates/aqua/tests/ui_app.rs` LANDED and green 5/5 on first run (mount,
  two-way bind, keyed refresh, dispose, offscreen). UPGRADED 2026-10-25:
  artifact test is now a real `ui!` Todo card (title, keyed rows, bound
  editable field, Button) with `GlyphonProvider` attached via
  `set_text_provider` sharing the UiApp's `TextBackend{fonts, runs}`;
  asserts glyph pixels vs transparent background (>500); PNG iff
  `AQUA_PNG_OUT` set (target `artifacts/aqua-ui.png`). FINAL exact counts
  from actual runs 2026-10-25 (no manual arithmetic): lib 13 +
  reactive_integration 11 + damage_scheduler 15 + keyed_rows 6 +
  keyed_tree 12 + facade_pump 6 + overlay_focus 14 + offscreen 3 +
  ui_app 5 = 85 passed, 0 failed (llvmpipe). NOTE: actuals sum to 85, not
  95 — no higher total is claimed. Clippy `-D warnings` clean on all
  `-p aqua` tests (fixed `chunks_exact` → `as_chunks` in ui_app.rs);
  rustfmt clean on all owned test files. keyed_tree 12/12 after core
  owner's reorder-clobber fix + my space-expectation fix. FROZEN: no
  further verification edits except parent-directed fixes.
  Compile verification of my fix PENDING: `aqua-render` lib now fails at
  `renderer.rs:2122:23` E0308 (`[f32; 2]` vs float, pass-2 rewrite) —
  render-owned, blocking the whole `-p aqua` build.
  GPU HANG recorded as NOT PASS: GPU test killed after N=10000, 100% CPU,
  4 min — cubic damage coalescer suspected; render owner's old isolated
  test also hung 8 min. Fix (bounded coalescer + compose uniform 32)
  pending with renderer owner. No GPU result may be cited until rerun.
  UiApp black-box tests PENDING: `ui_app.rs`/`todo.rs` transfer to overlay
  agent announced but not yet landed (no `UiApp` in `aqua-ui/src/` as of
  this check; `overlay_host.rs` present). New `ui_app` test file lands once
  the module + real macro Todo exist; existing tests adapt to API then.
```

## 4. Defects filed (repro + owner, fixes belong to owner)

1. **Platform `next_wakeup` busy-wake under backpressure** — FIXED by
   platform owner 2026-10-25 (`next_wakeup` rule 1: backpressured windows
   contribute no wakeup; animation-only fuses gate+deadline). Peer suite
   green. Locked by `damage_scheduler.rs::backpressure_wakeup_sleeps_until_present`.
   Collateral: peer's own `pacing_enforced…` test initially failed (produced
   twice without `mark_presented`, hit default cap 2); platform owner has
   since fixed (suite green).
2. **`aqua-ui` 22 compile errors** (tree/layout vs taffy-0.15 + core imports)
   — owner: core/UI agent. Repro: `cargo check -p aqua-ui`. Blocks all
   `-p aqua` tests.
3. **`aqua-render` unclosed delimiter** (`viewport.rs:2082`) — owner: render/
   media3d agent. Repro: `cargo check -p aqua-render`. Mid-edit breakage.
4. **Facade has no `aqua-text` dependency** — editable-`Text` binding cannot
   be tested through the facade; request: facade owner adds `aqua-text`
   (+ `aqua-macros`) dep so `crates/aqua/tests/text_editing.rs` can land.
   Unicode editing itself is tested at peer level (item 8).
5. **`cargo fmt --check` fails on `crates/aqua/examples/showcase.rs`**
   (facade owner; repro in `scripts/validate.sh`).
6. **Clippy: 33 errors in `aqua-core`** (float precision, `oklch`
   constructor naming) — core owner; repro in `scripts/validate.sh`.

## 5. Gaps / explicitly not claimed

- No FPS or Chrome-comparison numbers anywhere (forbidden without matched
  workloads; see `docs/PERFORMANCE.md`).
- Trace metrics recorded are counters/bytes/reasons from diagnostics, never
  invented benchmarks.
- Windows: `cargo check --target x86_64-pc-windows-gnu` cross-check only;
  native runtime never executed here.
- `docs/PLATFORM.md` notes 25 platform tests; this ledger counts what it
  ran itself (§3), not peer-quoted numbers.

## 6. Canonical commands

- Full validation: `scripts/validate.sh` (Linux) / `scripts/validate.ps1`
  (Windows). No push/publish; CI (`.github/workflows/ci.yml`) runs
  fmt + clippy + tests, GPU tests serial with headless-skip on no-adapter.
- Targeted: `CARGO_BUILD_JOBS=2 cargo test -p <owned-crate>`. No
  full-workspace builds without reason (shared lock).
