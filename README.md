# Aqua — Rust GUI framework

Aqua is a Rust GUI framework with one shared codebase and platform adapters
(Windows first). Product requirements and architecture live in
[`GUI_FRAMEWORK_DESIGN.md`](GUI_FRAMEWORK_DESIGN.md); the build plan in
[`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md). This file is the
implementation ledger: what runs today, how to run it, and what is next.

## Run it

```sh
cargo run -p aqua --example todo                 # canonical App, real window
cargo run -p aqua --example todo -- --headless   # canonical App, headless pump (CI-safe)
cargo run -p aqua --example collections          # List/Table demo, real window
cargo run -p aqua --example collections -- --headless  # List/Table demo, headless pump
```

To build a Windows Todo executable from WSL using Artisan Editor's Nix
cross-compilation shell:

```sh
bash scripts/build-windows.sh
```

Output: `dist/windows/aqua-todo.exe`. See
[`docs/WINDOWS_BUILD.md`](docs/WINDOWS_BUILD.md) for prerequisites and the
inherited x86-64-v3 CPU requirement.

Time-bounded smoke (no user input; validates idle behavior):

```sh
AQUA_SMOKE_MS=2000 cargo run -p aqua --example showcase
# frame-budget mode for active animation: AQUA_SMOKE_FRAMES=120 ...
```

On WSLg hosts without `DISPLAY` in the environment:

```sh
DISPLAY=:0 WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/$(id -u) \
  AQUA_SMOKE_MS=2000 cargo run -p aqua --example showcase
```

Validated 2026-10-25 (Linux/WSLg, native run): window opens, GPU presents,
clipboard backend System, smoke exits 0 —
`wakeups=8 requested=6 produced=3 presented_frames=3 render_errors=0`
(idle app sleeps; no loop). Windows 11 native run also exits 0
(`wakeups=5 requested=4 produced=2 presented=2`, System clipboard).

```sh
CARGO_BUILD_JOBS=2 cargo test -p aqua            # facade suite (13 lib + integration)
cargo check --workspace --all-targets            # full shared-workspace gate
cargo check -p aqua --target x86_64-pc-windows-gnu --all-targets
```

Getting started: [`docs/GETTING_STARTED.md`](docs/GETTING_STARTED.md).
Contracts: [`docs/APPLICATION.md`](docs/APPLICATION.md),
[`docs/AUTHORING.md`](docs/AUTHORING.md),
[`docs/IMPLEMENTATION_CONTRACT.md`](docs/IMPLEMENTATION_CONTRACT.md),
[`docs/PLATFORM.md`](docs/PLATFORM.md), [`docs/SCHEDULER.md`](docs/SCHEDULER.md),
[`docs/REACTIVITY.md`](docs/REACTIVITY.md), [`docs/TEXT.md`](docs/TEXT.md),
[`docs/UI_MACRO.md`](docs/UI_MACRO.md), [`docs/STYLING.md`](docs/STYLING.md),
[`docs/STYLE_CONTRACT.md`](docs/STYLE_CONTRACT.md),
[`docs/COLLECTIONS.md`](docs/COLLECTIONS.md),
[`docs/GPU_EXTENSIONS.md`](docs/GPU_EXTENSIONS.md),
[`docs/OVERLAYS_AND_MOTION.md`](docs/OVERLAYS_AND_MOTION.md),
[`docs/PERFORMANCE.md`](docs/PERFORMANCE.md), [`docs/VALIDATION.md`](docs/VALIDATION.md).

## Architecture (one shared codebase)

| Crate | Owns |
|---|---|
| `aqua` (facade) | Event loop, `AquaConfig`, canonical `App` builder + `Context`/`IntoView`, `Aqua::*()` authoring factories, `Application` low-level endpoint, GPU completion worker, shared fonts/runs handles, `ui!` re-export, showcase |
| `aqua-core` | IDs, geometry, color, scene vocabulary, input events, semantics |
| `aqua-reactive` | Explicit-dependency graph, scopes, batching, flush boundary |
| `aqua-text` | Shaping, editing model, shared fonts + run registry |
| `aqua-render` | wgpu 2D renderer, scene store, surface, media + viewport passes |
| `aqua-platform` | winit host, scheduler, materials, clipboard/dialogs/prefs, AccessKit bridge |
| `aqua-ui` | Tree, layout, overlays, motion, element and collection builders, virtualized row realization |
| `aqua-macros` | `ui!` parser + codegen |

`aqua::ui` ≡ `aqua_ui::element::ui` (`column`/`row` constants included);
`ui!` expands to builder calls against it.
`UiApp` (`crates/aqua/src/ui_app.rs`, overlay-agent owned) is the
high-level host over `View`/`UiTree`. New code uses the canonical root
builder (`docs/APPLICATION.md`) and the `Aqua::*()` factories
(`docs/AUTHORING.md`): `Aqua::App().config(Aqua::Config().window(
Aqua::Window().title(..).size(..).min_size(..).theme(..).material(..))).run(todo_app)`
with `fn todo_app(cx: &Context) -> impl IntoView` returning
`cx.view(explicit deps, render)`; `Aqua::run_ui` / `run_ui_reactive` and the
`Application` trait remain as the low-level escape. The `todo` example
(`crates/aqua/examples/todo.rs`, builder-first, no `ui!`) opens a window by default
(`cargo run -p aqua --example todo`); `-- --headless` runs its deterministic
input and state validation without a display. The `collections` example
(`crates/aqua/examples/collections.rs`) demos a 100k-row virtualized `List`
and a sortable `Table` with `fr` tracks through the same factories.
The showcase drives the low-level `Application` escape directly to exercise the
full runtime surface (event loop, externals, IME, a11y).

## Feature matrix (honest status)

| Capability | Status |
|---|---|
| Window open + winit 0.30 event loop (Windows-first, Linux running) | Implemented, native + WSLg runs green |
| Demand-driven scheduling, fps target, idle sleep | Implemented (`Scheduler`; validated idle at fps default) |
| Canonical input routing (modifiers, key text + IME suppression, real cancel, file drops) | Implemented |
| Reactive state (explicit signals, keyed scopes, host-driven flush boundary) | Implemented |
| List/Table, shared columns and opt-in virtualization | Implemented; variable-height windows, anchors, focus pinning and eviction tested |
| Text editing + shaped runs (shared fonts/registry, caret/selection, IME) | Implemented; pixels render via shared `TextBackend` |
| Media (decode → cache → GPU upload → `External` every frame) | Implemented in showcase |
| Viewport3D (depth-tested cube, custom renderer, resize/device-recreate) | Implemented in showcase |
| Toggle pulse via canonical `MotionController` + scheduler deadlines | Implemented |
| Filter popover (placement, light dismissal, focus restore) | Implemented |
| GPU-completion-backed backpressure (worker thread, blocking wait, proxy wake) | Implemented |
| OS prefs (reduced motion/contrast/transparency, theme), material fallback | Implemented |
| Native clipboard (system w/ explicit fallback), native dialogs (gated) | Implemented |
| Accessibility bridge (invisible-first attach, waker, snapshot push, action drain) | Implemented (mechanics; screen-reader run pending) |
| `UiApp` high-level host + builder Todo | Implemented; headless input lifecycle and native Windows GPU smoke pass |
| Screen-reader behavior, native material appearance, richer editing gestures | Pending; native smoke verifies API execution and frame submission, not appearance or assistive-technology behavior |

No performance-superiority claim is made; no comparative measurements exist.

## For contributors

- One shared workspace; targeted commands (`-p <owned-crate>`), one `target/` lock.
- No duplicate engines: consume peer contracts, fix integration, relay gaps to the parent.
- `ui!` layout values: `layout: column` / `layout: row` resolve via
  `aqua::ui::{column, row}` (qualified import) or `"column"`/`"row"`
  strings / `Axis`. The bare names are NOT in the prelude: glob-imported
  lowercase consts would hijack user bindings (`Some(row)` → E0308).
- Styling is canonical `aqua::Style` arrays (`styles: [BASE, override]`,
  later wins per field); see [`docs/STYLING.md`](docs/STYLING.md). Shared
  pieces can be `const` (`Oklch::new` is const); fixed arrays have a known
  length and may contain constant or runtime values. `LayoutStyle`/`Paint` remain as the
  lower-level resolved types.
- Tests live in each crate + `crates/aqua/tests/` (test-agent owned); fix compile defects in owned files, relay others with file:line.

## Validation and remaining work

Run `AQUA_REQUIRE_GPU=1 bash scripts/validate.sh` for the strict checks.
The final results and native smoke evidence are in
[`docs/VALIDATION.md`](docs/VALIDATION.md).

This is an initial implementation. UI host traversals remain linear when
building a frame; richer selection gestures, clipboard shortcuts, and
native screen-reader validation remain work. See
[`docs/UI_APP.md`](docs/UI_APP.md) and
[`docs/PERFORMANCE.md`](docs/PERFORMANCE.md) for the precise limits.
