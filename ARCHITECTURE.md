# Aqua architecture: pure core, native adapters

Last updated: 2026-10-29.

This document supersedes the foundation choices in `GUI_FRAMEWORK_DESIGN.md` (winit and wgpu) and records the target architecture. It is a design, not an implementation ledger: nothing below the "Current state" sections exists yet. Numbers marked "estimate" are unmeasured.

## 1. Goals

In priority order:

1. **Performance:** frame time, input latency, memory, idle cost, battery.
2. **Customizability on the level of a browser engine:** styles, effects, custom passes and themes without forking the framework.
3. **Text quality:** shaping, rasterization and editing that hold up next to native and Chromium text.
4. **Verifiability:** every subsystem deterministic, inspectable and testable without a window or a GPU.

Aqua is developed by agents. Engineering effort is not a constraint and verbosity is not a cost. API design buys explicitness, determinism and checkability instead of brevity.

## 2. Context: Rune

[anomalyco/rune](https://github.com/anomalyco/rune) is a Rust UI framework with similar goals. Its `PLAN.md` and `research/` folder (30 notes) were reviewed on 2026-10-29. Its motive is the author repeatedly hitting GPUI's architectural limits (no backdrop/glass, animation on the main thread, wrong window behavior for HUDs, thin text, no springs, a frozen upstream), plus code written and verified by agents. Its flagship test is recreating OpenCode Desktop.

Adopted from Rune:

- Animation runs on a render thread from analytic spring tracks; app code never runs per animation frame.
- A slow, obviously correct CPU reference renderer as the oracle for the fast GPU path, with differential tests.
- Text quality as a gated risk: pixel diffs against the native rasterizer.
- Perf measured by exact counts (allocations, instances, glyphs, layout nodes), so gates are bisectable.
- Pyramid blur with a fractional radius, closed-form shadows.

Where Aqua deliberately differs:

- **State.** Rune rebuilds view functions and diffs a keyed tree. Aqua keeps declared dependencies as data (`aqua-reactive`), which is the faster direction: invalidation cost follows what changed, not the view size.
- **Foundation.** Rune starts on winit and wgpu and drops to native only where measured. Aqua goes native from the start (sections 5 and 6).
- **Platform order.** Rune is Mac-centered. Aqua is Windows-first.

## 3. Principles

1. **Ports and adapters.** Pure crates define every boundary as traits and plain data (ports). Adapters implement ports and contain all OS and GPU code. Dependencies point inward only.
2. **Sans-IO core.** The pure runtime is a state machine: `step(Input) -> Output`. It reads no clock, spawns no threads and touches no files. Time, events and GPU results are inputs. Draw plans, window commands and wake deadlines are outputs.
3. **The pure side plans, adapters execute.** The core never says "call D3D12". It emits a `FramePlan` that is complete enough for a dumb backend to run.
4. **All `unsafe` lives in adapters,** except an audited `aqua-kernels` crate (section 4) where every kernel has a safe reference implementation.
5. **Capabilities are explicit.** Adapters report what they support. Unsupported features return errors, never silent no-ops. This continues the existing `EffectCapabilities` rule.
6. **Tiers change speed, never output.** A user on a weak laptop sees the same pixels as a developer on a flagship GPU, within the reference renderer's tolerances.

## 4. Crate structure

```
crates/
  pure/
    aqua-core          ids, generational arena, geometry, Oklch color, units
    aqua-reactive      declared-dependency graph, scopes, batching, disposal
    aqua-style         Style, tokens, cascade, themes, motion tokens
    aqua-ui            element builders, node tree, keyed reconciliation, events, overlays, collections
    aqua-layout        Taffy integration over the node arena, sum-tree height index
    aqua-motion        motion targets and policy (app side), analytic tracks (render side)
    aqua-text          shaping, bidi, line breaking, editing, glyph rasterization, atlas packing
    aqua-media         image decode and cache bookkeeping
    aqua-scene         display list, property trees, damage
    aqua-graph         render graph IR and planner: culling, ordering, aliasing, barriers
    aqua-protocol      every port trait and message type
    aqua-runtime       sans-IO frame pipeline composing everything above
    aqua-kernels       audited unsafe/SIMD kernels, each with a safe reference twin
  adapters/
    aqua-gpu-d3d12     GpuBackend on Direct3D 12
    aqua-gpu-ref       GpuBackend on the CPU: the oracle, headless tests, CI
    aqua-shell-win32   windows, input, DPI, chrome, clipboard, dialogs, prefs, materials
    aqua-ime-tsf       Text Services Framework
    aqua-a11y-uia      UI Automation
    aqua-fonts-system  system font enumeration, fallback, memory-mapped font files
    aqua-shaders       build-time HLSL compilation to DXIL
    aqua-transport     in-process SPSC ring between app side and render thread, inspector socket
    aqua-oracle-dwrite DirectWrite, test-only, text pixel diffs
  host/
    aqua               public API, composition root, the only real event loop
```

Crate names keep the existing `aqua-*` names where the role is unchanged.

### Enforcement

- A workspace test reads `cargo metadata` and fails if any `pure/*` crate depends on an adapter or on `windows`, `ash`, `objc2`, `winit` or `wgpu`.
- `#![forbid(unsafe_code)]` on every pure crate except `aqua-kernels`.
- Pure crates do not call `std::time::Instant::now`, `std::thread`, `std::fs` or print to stdout/stderr. A lint test greps for them.
- Adapters expose nothing public beyond their port implementations and a constructor.

## 5. The runtime loop

```rust
pub struct Runtime { }

impl Runtime {
    pub fn step(&mut self, input: Input) -> Output;
}

pub enum Input {
    Shell(ShellEvent),
    Ime(ImeEvent),
    Access(AccessAction),
    Gpu(GpuEvent),
    Render(RenderEvent),
    Tick(Instant),
}

pub struct Output {
    pub shell: Vec<ShellCommand>,
    pub ime: Vec<ImeCommand>,
    pub access: Option<AccessTreeUpdate>,
    pub render: Vec<RenderCommand>,
    pub wake_at: Option<Instant>,
}
```

`Instant` is a plain value type in `aqua-protocol`, not `std::time::Instant`.

Consequences:

- Recording inputs gives deterministic replay. Bug reports are input logs.
- Time control (pause, step, slow motion, seek) is a test wrapper around `step`.
- The headless mode and every integration test drive `step` directly with the reference GPU adapter and a scripted shell. There is no parallel fake pipeline.

### Threads

Two pure state machines, driven by the host:

- **App side:** reactive flush, structural rebuilds, layout at the target state, text shaping, scene emission, input routing. Sends `RenderCommand`s (scene diffs, property-tree updates, motion targets, uploads).
- **Render side:** owns the presented scene and every motion track, samples tracks at the display's target presentation time, plans and submits frames. Sends `RenderEvent`s back (settled tracks, presented-frame ids, GPU timings, device loss).

Both are pure crates. The host owns the actual threads and moves messages over `aqua-transport`. While one value animates, the app side does no work.

## 6. GPU port and rendering

### 6.1 The port

The planner is the hard part, so it stays pure and testable. The adapter receives a finished plan.

```rust
pub trait GpuBackend {
    type Error: std::error::Error;

    fn capabilities(&self) -> &GpuCapabilities;
    fn create_surface(&mut self, target: SurfaceTarget) -> Result<SurfaceId, Self::Error>;
    fn upload(&mut self, batch: UploadBatch) -> Result<(), Self::Error>;
    fn execute(&mut self, plan: &FramePlan, inputs: &FrameInputs) -> Result<FrameReport, Self::Error>;
    fn present(&mut self, surface: SurfaceId, hint: PresentHint) -> Result<PresentTimes, Self::Error>;
    fn recover(&mut self) -> Result<RecoveryReport, Self::Error>;
}
```

`FramePlan` is plain data:

- Ordered passes, each a `PassKind`: `Instanced { shader, instances }`, `Fullscreen`, `Compute`, `Copy`, `Custom`.
- Transient textures with size, format and a heap offset assigned by the planner (aliasing).
- Declared usage per pass (`reads`, `writes`). The planner derives barriers; the backend only translates them.
- Bindless texture handles by index, so no backend binding types leak inward.
- `ShaderId` plus specialization constants. Blobs come from `aqua-shaders` per backend.

The planner gets alignment, heap limits and feature tiers from `GpuCapabilities`. `FrameReport` returns timings, counters and device status as `Input::Gpu`.

### 6.2 Rendering methods

| Area | Method | Replaces |
|---|---|---|
| Common primitives | One instanced SDF "uber-quad": rounded and squircle rects, borders, gradients in Oklab with dithering, per-side soft clip edges | `rect.wgsl` per-rect SDF without gradients or dithering |
| Text | 8-bit glyph atlas with 4× horizontal subpixel positioning; Slug curve rendering for large, scaling or rotating text and icons | glyphon |
| Complex vectors | Sparse-strip or tiled path rendering for arbitrary paths | not supported today ("rounded-rect clips only") |
| Shadows | Closed-form erf blurred rounded rect in the fragment shader | `smoothstep` approximation |
| Backdrop blur | Downsample pyramid (dual Kawase or Jimenez) with a fractional radius, scissored to the element's bounds, as graph nodes | 9-tap Gaussian with integer downsample and step thresholds, which steps when the radius animates |
| Group opacity | Folded into instance alpha when children don't overlap; an aliased offscreen layer only when they do | always an isolated intermediate surface |
| Content blur, color matrix, displacement, blend modes | Graph nodes, same mechanism as backdrop blur | unsupported |
| Damage | Per-swapchain-buffer age tracking with dirty rects passed to `Present1` | a retained full-window base texture |
| Custom effects | `CustomPassSpec` as data (HLSL source, inputs, outputs, bounds), compiled and cached by the adapter | `CustomPass` trait receiving wgpu types |

### 6.3 The D3D12 adapter

- `windows` crate bindings, Agility SDK runtime shipped with the app.
- DXGI flip-model swapchain with a waitable frame-latency object and tearing allowed. Frames start as late as possible against the waitable, which is the main input-latency lever on Windows.
- DirectComposition for per-pixel-alpha windows, popovers sized to content, and system backdrop materials.
- Placed resources in aliased heaps for render-graph transients.
- Shaders precompiled by DXC at build time. Pipeline state objects prebuilt, cached on disk and warmed at startup. The uber-shader keeps the PSO count, and the driver memory that comes with it, small.
- Device-removed recovery: the retained scene is enough to rebuild every GPU resource, so `recover` rebuilds and the app continues.

### 6.4 Hardware baseline and tiers

Most users run integrated GPUs or older discrete cards. The baseline must be fast there.

**Baseline (must run everywhere):** D3D12 feature level 11_0/12_0, Shader Model 6.0, legacy resource barriers. **WARP** (Microsoft's software rasterizer) as the last fallback for VMs, Remote Desktop and broken drivers.

| Feature | Baseline | Better tier |
|---|---|---|
| Binding | Small descriptor tables per pass, glyph atlas as a texture array | SM 6.6 descriptor heap indexing (bindless) |
| Barriers | Legacy transitions | Enhanced barriers |
| Uploads | Ordinary copy commands | Batched copies (`CopyBufferRegions`) |
| Sync | Queue-level fences | Fence barriers inside command lists |

What decides performance on integrated GPUs is memory bandwidth, not API features: damage-limited redraw, blur at reduced resolution, avoiding offscreen layers, zero work at rest, two-buffer swapchains.

### 6.5 DirectX update, October 2026

The Agility SDK 1.620 retail and 1.722 preview were announced on 2026-10-28. The raytracing work (clustered geometry, partitioned top-level acceleration structures, indirect builds) is in the preview and irrelevant to Aqua. Relevant from 1.620 retail, all as adapter-internal tiers:

- **Fence barriers tier 1:** signal and wait on fences inside a command list.
- **Batched asynchronous command list APIs:** independent copies that can overlap, clears without views and inside render passes. A fallback layer emulates them on older hardware.
- **DirectX dump files, driver-reported messages, extended PIX markers:** for device-removed debugging.
- **`CheckFeatureSupport1`:** capability queries clamped to an SDK version, a fit for `GpuCapabilities`.

On AMD, most require RX 7000 or 9000. Shader Model 6.10 goes retail in Agility SDK 1.621, due early December 2026.

### 6.6 Test hardware

Development machine: Radeon RX 9070 XT and Ryzen 7 9800X3D. The 9800X3D has a 2-CU RDNA 2 integrated GPU, usually set to "Auto" in the BIOS and therefore off while a discrete card is installed. Enabling it ("Integrated Graphics" set to Enabled or Forces, under Advanced, NB Configuration or AMD CBS → NBIO → GFX Configuration depending on the board) gives a pessimistic floor: it is weaker than typical laptop iGPUs but has laptop-like memory bandwidth. Select it with `EnumAdapterByGpuPreference` (minimum power) or by LUID. Presenting from it to a monitor on the 9070 XT adds a cross-adapter copy, which is fine for correctness and tier tests but skews latency. Measure with a monitor on the motherboard output or with offscreen benchmarks.

## 7. Shell, input, IME, accessibility

- **`aqua-shell-win32`** replaces winit. Win32 windows with per-monitor DPI v2, raw input, non-activating panels, custom chrome and drag regions (today's `chrome.rs`), system backdrop materials (Mica, Acrylic), clipboard, dialogs and OS preferences.
- **`aqua-ime-tsf`** implements Text Services Framework directly. TSF loads on first text focus, not at startup.
- **`aqua-a11y-uia`** implements UI Automation (directly or through AccessKit's Windows adapter, decided by measurement). It attaches only when a client connects. The accessibility tree is also the inspector's tree.
- **Shell file dialogs** pull many shell components and third-party shell extensions into the process, and they never unload. They run in a short-lived helper process.

## 8. Text

Text is a top priority. The pipeline is pure except font discovery.

| Stage | Choice |
|---|---|
| Discovery | `aqua-fonts-system`: system fonts memory-mapped, never copied into the heap; fallback scanned lazily with an on-disk index |
| Shaping | HarfRust (HarfBuzz port) |
| Paragraph layout | Parley or equivalent: bidi, line breaking, rich text, inline boxes, with a two-tier cache (shaped runs separate from wrap points) so resizing never re-shapes |
| Rasterization | skrifa outlines, luminance-based glyph dilation tuned against the native rasterizer, 8-bit atlas, 4× horizontal subpixel positions |
| Motion | y snapped to pixels and x to ¼ px while moving; Slug outlines for text under scale animation so glyphs never re-rasterize per frame |
| Editing | rope or sum-tree storage, today's `aqua-text` editor model on top |
| Gate | `aqua-oracle-dwrite` renders the same strings through DirectWrite; CI diffs per string at 1× and 2× |

DirectWrite is only loaded by tests. Matching DirectWrite pixel-for-pixel versus one consistent look on every platform stays an open decision (section 13).

## 9. Application API

### 9.1 Current state (from code, 2026-10-29)

The README is stale. The canonical API in `crates/aqua/src/app.rs` and `crates/aqua/examples/todo.rs`:

```rust
fn todo_app(cx: &Context) -> impl IntoView {
    let glass = cx.material().is_glass();
    let (deps, render) = todo_view(cx.scope(), Vec::new(), glass);
    cx.view(deps, render)
}

fn main() -> Result<(), aqua::Error> {
    Aqua::App()
        .config(
            Aqua::Config().window(
                Aqua::Window()
                    .title("Aqua — Tasks")
                    .size(px(760.0), px(560.0))
                    .min_size(px(480.0), px(360.0))
                    .theme(Theme::Dark)
                    .material(Material::Glass)
                    .titlebar(Titlebar::Overlay),
            ),
        )
        .run(todo_app)
}
```

- **State:** `cx.scope().signal(initial)` returns `(ReadSignal<T>, WriteSignal<T>)`. There is no implicit tracking; every `get()` is a snapshot.
- **Views:** `cx.view(deps, render)` takes a `DepTuple` (1 to 8 `ReadSignal`s) and `render: FnMut(D::Values) -> Element`. A dependency change reruns `render` and keyed reconciliation preserves node identity, focus and edits. Targeted prop updates go through bindings (`.bind((read, write))`) without a rebuild.
- **Elements:** builders on the `Aqua` struct: `Aqua::Box()`, `Text`, `Button`, `Checkbox`, `Slider`, `Dropdown`, `Tabs`, `Scroll`, `Canvas`, `Media`, `Viewport3D`, `Fragment`, `Dynamic`, `List`, `Table`, `Column`. `.styles([BASE, variant])` (later wins per field), `.child`, `.children`, `.build()`, `.with_key(id)`.
- **Styles:** `const` values from `Aqua::Style()` with Oklch colors.
- **Host:** `UiApp` runs the frame pipeline: flush → structural drain → overlay lifecycle → layout → re-anchor → text runs → emit → caret/selection/IME → reconcile → motion advance.

`aqua-macros` (the `ui!` macro, 1,748 lines with tests) still exists and `todo.rs` still calls it "compatible". It is slated for removal.

### 9.2 How the API maps onto the new structure

The public surface stays in the `aqua` host crate and keeps its shape: `Aqua::App().config(..).run(setup)`, `Context`, `IntoView`, the `Aqua::*()` builders. What changes is underneath:

| Today | Becomes |
|---|---|
| `App::run` resolves config, constructs `Aqua` (winit + wgpu), runs the loop | `App::run` picks adapters per target, constructs the pure `Runtime`, runs the host loop |
| `App::prepare` + `run_headless_one_frame` | `Runtime` driven by `step` with `aqua-gpu-ref` and a scripted shell |
| `UiApp::build_frame` | `aqua-runtime`'s app-side frame step |
| `UiApp` reads `Instant::now()` (`t0`, deadline conversion) and prints with `eprintln!` | time arrives as `Input::Tick`; errors return as `Output` diagnostics |
| `Context::material()` startup snapshot | same, resolved by the shell adapter's capabilities |
| Hand-written headless pumps (`todo.rs` `headless_demo`) | sequences of `Input::Shell(PointerDown ..)` through `step` |

### 9.3 Reactivity

The explicit-dependency model stays and gets one addition: a **debug-mode read check**. While `render` runs, the runtime records every `ReadSignal::get()`. Any signal read inside `render` that is not in the declared `DepTuple` is a hard error with the signal's creation site. Reads inside handlers (`on_activate`, `on_submit`) are not render reads and stay unchecked. This turns a forgotten dependency, the one failure mode of explicit deps, into an immediate test failure.

Later, for performance: declared dependencies are static data, so the runtime can derive per-node dirty classes (style-only, paint-only, layout, structure) and skip work below the changed level. `ReadSignal`/`WriteSignal` currently wrap `Rc<RefCell<..>>`; moving cells into an arena indexed by `NodeId` removes per-signal allocation and matches the rest of the data-oriented design.

## 10. Motion

### 10.1 Current state

`aqua-ui/src/motion.rs`:

- `Animated`: one property moving toward one target. `MotionKind` is `Tween { duration_secs, easing }`, `Spring(SpringParams { stiffness, damping })` or `Instant`.
- `MotionValue`: `Scalar`, `Pair`, `Color(Oklch)` with shortest-path hue.
- Retargets sample the displayed value first; springs keep velocity.
- Completion callbacks bind to a specific target; superseded completions are dropped and never fire.
- Reduced motion collapses in-flight and future motion to `Instant`.
- `MotionController`: generational arena of properties with owners (`NodeId`), `next_deadline`, `advance(now)`, `dispose_owner`.
- Time is always caller-supplied. It owns no clock.
- The controller lives in `OverlayHost` and advances inside `UiApp::build_frame` on the app thread. Authors reach it through `ui.host_mut().motion_mut()`.

Its semantics (no jumps on retarget, exact completion delivery, owner disposal, reduced motion, caller-supplied time) are already right for the new architecture. What changes is where it runs and how springs are evaluated.

### 10.2 Analytic springs

Springs are integrated with semi-implicit Euler at 1 ms substeps (up to 9,000 per sample on a clock jump) and channels are `Vec<f32>` (allocations per property). Replace with closed-form tracks:

A track is a segment `{ t0, x0, v0, target, params }`. With mass 1, `ω = sqrt(stiffness)` and `ζ = damping / (2ω)`. For the underdamped case, with `A = x0 − target`, `ω_d = ω·sqrt(1 − ζ²)` and `B = (v0 + ζωA) / ω_d`:

```
x(t) = target + e^(−ζω(t−t0)) · (A·cos(ω_d(t−t0)) + B·sin(ω_d(t−t0)))
```

The critically damped and overdamped cases have their own closed forms. Benefits:

- Exact position and velocity at any time, at constant cost, with no substep loop.
- Retargeting samples `(x, v)` at the presentation time and starts a new segment, so velocity carries over exactly as today.
- The settle time is computable, so the render side knows when a track ends without polling.
- The same formula can run in a shader for opt-in GPU-evaluated tracks.

`SpringParams` keeps stiffness and damping and adds a `duration`/`bounce` constructor (`ω = 2π / duration`, `ζ = 1 − bounce`). Channels become fixed `[f32; 4]` arrays: zero allocations per track.

### 10.3 Split across threads

`aqua-motion` splits into two halves:

- **App side (`MotionController`):** spawn, set target, policy, owners, completion registration. It does not sample. `set_target` emits a `RenderCommand::Motion` with the target, kind and the target's generation.
- **Render side (`TrackSet`):** owns segments, samples them at the display's target presentation time, writes presented values into property-tree nodes (transform, opacity, clip, colors, edges), and emits `RenderEvent::Settled { handle, generation }`.

Completion callbacks (`Box<dyn FnMut(Completion)>`) cannot cross threads, so they stay on the app side and fire when the matching `Settled` event arrives. A `Settled` event with a stale generation is dropped, which preserves today's "a superseded completion never fires" rule exactly.

### 10.4 Layout animation and presence

Layout runs once, at the target state. When a node's frame changes, the render side animates each edge that moved with its own track; stationary edges get no track and cannot jiggle. Shapes, borders and shadows are drawn at the presented size, so there is no scale distortion.

Presence: removing a keyed node with an exit motion moves it to an exiting state. It leaves layout flow, plays its exit and is disposed only when its tracks settle. A key that returns mid-exit revives from its presented state. `dispose_owner` keeps its silent-drop semantics and runs at that point.

### 10.5 Authoring motion (proposal, not compiled)

Today authors drive `MotionController` by hand. The builder API gains declarative motion: styles say what the value is, motion says how it gets there. When `render` reruns and a node's resolved style value differs, the runtime calls `set_target` on that node's track for that property.

```rust
const CARD: Motion = Aqua::Motion().spring(SpringParams::SNAPPY);
const FADE: Motion = Aqua::Motion().tween(0.12, Easing::EaseOut);

Aqua::Box()
    .styles([ROW, ROW_BASE, row_style])
    .animate(
        Aqua::Animate()
            .background(CARD)
            .border_color(CARD)
            .frame(CARD),
    )
    .presence(Aqua::Presence().enter(FADE).exit(FADE))
    .build()
    .with_key(t.id)
```

Every animated property is listed explicitly; nothing animates by default and nothing animates on first mount. Motion values are `const` tokens in `aqua-style`, so a theme can retune all motion in one place, and token changes retarget in-flight tracks live.

## 11. Memory

Estimates for an editor-class app on D3D12, unmeasured:

| Component | Private working set | Design choice |
|---|---|---|
| D3D12 runtime + driver private | several MB, vendor-dependent | few PSOs, one descriptor heap, reused command allocators, debug layer off |
| Framework heap | ~1–5 MB | arenas, struct-of-arrays, per-frame arena, zero steady-state growth |
| Fonts | ~0 | memory-mapped, shared |
| Glyph atlas, CPU side | ~0 | upload ring, no CPU mirror |
| Executable code | ~0 | image pages are shareable |

Target: roughly half of Artisan Editor's 25 MB private working set (estimate). GPU memory is dominated by the swapchain: about 14.7 MB per 2560×1440 BGRA8 buffer, two buffers minimum. Transients are aliased and sized to effect bounds; the atlas is an 8-bit texture grown in pages with eviction. The adapter responds to video memory budget changes and frees transient heaps when minimized.

Pure crates get an exact heap gate: a counting allocator in tests reports allocations and bytes per scene, and CI fails on regressions.

## 12. Verification

- **Conformance suite:** the same `FramePlan`s run on every `GpuBackend`; output is diffed against `aqua-gpu-ref` with per-channel tolerances. Every tier of the D3D12 adapter runs it.
- **Differential fuzzing** of fast paths against reference implementations from one scene generator.
- **Replay determinism:** recorded `Input` logs replay to identical scene and plan hashes.
- **Exact counters:** allocations, instances, glyphs, layout nodes, passes per scene.
- **Text oracle:** DirectWrite pixel diffs (section 8).
- **Motion checks:** no one-frame jumps, no unmount above zero opacity, no motion on first mount, settle within declared bounds.

## 13. Migration order

Each step keeps the app running.

1. Remove `aqua-macros` and the `ui!` references in docs and examples.
2. Create `aqua-protocol` and `aqua-runtime`. Move `UiApp::build_frame` and the scheduler into the runtime; make time an input.
3. Wrap today's wgpu renderer as a temporary `GpuBackend` adapter. Split `aqua-render` into `aqua-scene`, `aqua-graph` and the adapter.
4. Build `aqua-gpu-ref` and the conformance suite against the wgpu adapter.
5. Replace springs with analytic tracks and fixed-size channels.
6. Build `aqua-gpu-d3d12` until it passes conformance on all tiers, then delete the wgpu adapter.
7. Build `aqua-shell-win32`, `aqua-ime-tsf` and `aqua-a11y-uia`; delete winit.
8. Move the render side onto its own thread with `aqua-transport`; split `aqua-motion`.
9. Replace cosmic-text and glyphon with HarfRust, Parley-class layout and skrifa rasterization; add the DirectWrite oracle.
10. Declarative motion, presence and the debug-mode read check.

## 14. Open decisions

1. **Text look:** match DirectWrite/ClearType on Windows, or one consistent rasterizer on every platform.
2. **UIA:** hand-written or AccessKit's Windows adapter.
3. **Render thread ownership:** a pure state machine driven by the host (preferred) or an adapter-owned thread.
4. **Other platforms:** out of scope until Windows ships. Vulkan, Metal and non-Windows shells are not built because they cannot be tested here; the port stays graph-level so they can be added later.
5. **`aqua-kernels` scope:** which hot paths justify `unsafe` and SIMD.
