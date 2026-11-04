# Aqua architecture: pure core, native adapters

Last updated: 2026-11-04.

Implementation update (2026-11-03): the scene/protocol/runtime extractions,
explicit time and analytic springs have landed, together with text, native
Windows rasterization and full Editor application qualification in
[`docs/EDITOR_READINESS.md`](docs/EDITOR_READINESS.md).
The native host backend and render transport replacements below
remain the target architecture. The historical current-state descriptions
are not the latest feature ledger.

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

Implemented shape (`aqua-graph`, `aqua-protocol`): authors build a `RenderGraph` (or feed `PaintCommand`s to `plan_paint`) and `RenderGraph::plan(PlanLimits)` returns a validated `FramePlan`. The planner culls instances outside the target or clip, culls passes that never reach the surface, hoists independent draws into earlier compatible batches, folds group opacity into instance alpha when children are disjoint, allocates transients by lifetime with first-fit aliasing, sets load ops (a transient's first write clears or discards) and derives transitions plus aliasing barriers. `validate` replays the barrier state machine, so a backend can trust the plan. Backdrop and content blur are graph nodes (`Copy`, a `BlurDown`/`BlurUp` pyramid of `Filter` passes, an `Images` composite) whose effective strength `offset * 2^levels` equals the radius, so animating the radius does not step. `GpuCapabilities::plan_limits()` feeds the planner. Layers (group opacity that cannot fold, rounded clips, content effects) are sized to their pixel-aligned bounds and drawn in layer-local coordinates (section 14.8); a layer that covers its whole target keeps the `LayerOver` composite, any other composites as an `Images` instance. A plan whose first surface use is not a clearing render pass (nothing writes the surface, or a backdrop read comes first) starts with an explicit clear pass: an empty `Instanced` pass carrying `LoadOp::Clear(plan.clear)`, so a backend never has to clear on its own.

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
| Content blur, color matrix, custom effects, blend modes | Graph nodes, same mechanism as backdrop blur (`PushContentBlur`, `PushColorMatrix`, `PushCustomEffect`); displacement is not built | unsupported |
| Rounded clips | `PushRoundedClip`: children render into a bounded layer that composites with a rounded clip (`ImageInstance::clip` plus `clip_radius`) | rounded-rect clips only through a stencil-less workaround |
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
- **Shell surface (2026-11-04):** touch and pen arrive as `WM_POINTER*` and are translated by the pure `aqua_shell_win32::pointer::translate` into `PointerDown/Move/Up/Cancel` with a stable contact id (pointer id plus one; mouse stays 0) and per-contact button masks (pen barrel is `Secondary`, eraser tip `Extra(5)`); mouse stays on `WM_MOUSE*`, and pointer types other than touch and pen fall through to `DefWindowProc`. File drag and drop is an OLE `IDropTarget` registered per window and reported as `ShellEvent::FileDrag(Enter | Over | Leave | Drop)` with paths and logical positions. `aqua_shell_win32::set_message_hook` installs a `MessageHook`: `pre_translate` runs on every message before `TranslateMessage` (the TSF keystroke manager hook point; returning true consumes the message) and `window_message` runs first inside the window procedure and can answer a message itself. `WindowOptions::alpha_surface` creates the window with `WS_EX_NOREDIRECTIONBITMAP`, and `Window::attach_composition_surface` binds a composition swap chain (created with `DXGI_ALPHA_MODE_PREMULTIPLIED` through `CreateSwapChainForComposition`) to a DirectComposition visual, which is what lets Mica, Mica Alt and Acrylic show through; the GDI `present_bgra` path draws nothing on such a window. `WM_DPICHANGED` emits `ScaleChanged` first, then moves the window to the suggested rectangle (which emits `Resized`). The pure chrome hit test and geometry (`RectI`, `DragRects`, `HitInput`, `ht`), preference state (`PrefState`, `SystemPrefs`) and the material decision (`DwmSystemBackdrop`, `decide`) live in `aqua-protocol`; `aqua-platform` re-exports them and the shell no longer depends on `aqua-platform`.
- **`aqua-ime-tsf`** implements Text Services Framework directly. TSF loads on first text focus, not at startup. It is a pull-model `ITextStoreACP` over a windowed UTF-16 mirror of the focused text (`ImeCommand::Enable/Update` carry the window, selection and caret rectangle). TSF edits are translated into `ImeEvent`s (composition start/update/commit/cancel with byte cursors, plus `Replace` and `Select` for edits outside the composition); candidate windows are positioned from the caret rectangle in DPI-scaled screen coordinates; `ImeCommand::Reconvert` drives the system `ITfFnReconversion`. Composition that TSF wraps around programmatic edits is folded into a single insertion.
- **`aqua-a11y-uia`** implements UI Automation through AccessKit's Windows adapter (section 14.2). The host calls `UiaAdapter::apply(&AccessTreeUpdate)` with each `Output.access` and drains `UiaAdapter::drain_actions()` into `Input::Access`; the waker passed to `attach` must wake the host loop. It attaches only when a client connects. The accessibility tree is also the inspector's tree.
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

DirectWrite is only loaded by tests. The look decision is closed in section 14.1.

### 8.1 Implementation (2026-11-04)

`aqua-text` no longer depends on cosmic-text, glyphon or wgpu. Layout is split into pure stages:

| Stage | Module | Notes |
|---|---|---|
| Discovery | `aqua-fonts-system` | fontdb index over system fonts; faces are memory-mapped once and shared as `FaceBytes` (the only `unsafe` in the text stack). Fallback is cmap-coverage driven per grapheme: explicit families, emoji families, platform script families, then a lazily cached scan. |
| Itemization and shaping | `shape.rs` | Spans split at font, bidi level and script boundaries; HarfRust per run with the span's features (ligatures off when tracking is set); `ShapedParagraph` holds clusters, runs and glyphs and is the tier-1 cache. |
| Line breaking | `lines.rs` | UAX 14 break opportunities from `icu_segmenter` (computed lazily per paragraph), greedy fit over clusters, hanging blanks, `Word`/`Glyph`/`WordOrGlyph`, tab stops, UAX 9 L1/L2 reordering per line, alignment and justification. This is tier 2: changing width, wrap or alignment never re-shapes (`TextLayout::reshape_count` is the exact counter). |
| Rasterization | `raster.rs`, `strike.rs` | skrifa outlines, exact-area coverage through zeno, 4 horizontal subpixel positions, y snapped, sizes quantized to 1/64 px, synthetic italic skew, fake bold (zeno stroke unioned with the fill, em/24 wide) when the requested weight is 600 or more and the face is lighter and not variable, COLR v0/v1 color glyphs (solid, linear, radial approximated; sweep approximated by its first stop), and CBDT/sbix bitmap strikes (PNG and premultiplied BGRA; nearest strike, area-averaged down or bilinear up to the requested size, placed from the strike bearings). |
| Atlas | `residency.rs` | `aqua-text` has no atlas of its own. `GlyphResolver` interns a face (font, weight, stretch, italic) to the `u64` in `aqua_media::atlas::GlyphKey`, rasterizes through `Rasterizer` on a miss and hands the bitmap to `aqua_media::Residency::glyph_any`, which routes masks to the 8-bit atlas and color glyphs to the RGBA atlas. `GlyphResolver::lower` turns a `LaidOut` into `GlyphInstance`s (masks) and `ImageInstance`s (color glyphs) ready for `glyph_passes`. |
| Tone | `aqua_graph::tone` (re-exported as `aqua_text::tone`) | The Microsoft grayscale contrast and gamma model as a pure function of coverage and text luminance (`ToneCurve::native()` is gamma 1.8, grayscale contrast 1.0). It lives in `aqua-graph` so `aqua-gpu-ref` and the D3D12 glyph shader share it without depending on the text stack. The pipeline blends in linear light while DirectWrite blends in sRGB, so `ToneCurve::linear_blend_alpha` also remaps the toned coverage so that a linear blend against the contrasting backdrop (black under light text, white under dark text) lands where DirectWrite's sRGB-space blend does. The HLSL (`glyph.hlsl`, constants in the root constants `region` and `values`) and the oracle implement the same function. |

Oracle results on the development machine (`aqua-oracle-dwrite`, Segoe UI, run natively on Windows):

- Shaping: glyph ids and advances equal DirectWrite's for five strings at 13, 16, 26, 32 and 48 px (worst advance delta 0.0000 px).
- Coverage: unhinted per-glyph coverage versus `CreateGlyphRunAnalysis` with grid fitting disabled has a mean absolute error of 2.9 to 7.3 levels (of 255) and an ink ratio of 0.98 to 1.01 at 13 to 32 px.
- Blending: with the tone curve, total ink versus `IDWriteBitmapRenderTarget` grayscale output (system parameters gamma 1.8, enhanced contrast 0.5, grayscale enhanced contrast 1.0) is within 0.94 to 1.05 (worst case 0.9425 at 13 px) for black, white, gray and colored text on light and dark backgrounds; without it white-on-black text is 20% light and black-on-white 13% heavy. A parameter sweep over gamma 1.0 to 2.2 and contrast 0 to 2.5 puts the optimum at gamma 1.9 and contrast 1.0 with mean error 11.039, against 11.046 for the system values, so the native parameters are kept.
- Speed (release): 2,000 wrapped paragraphs of about 100 characters shape and break in 56 ms cold and re-wrap in 19 to 24 ms (Linux host 56/24, Windows 66/19). On Windows with 172 system faces, discovery takes 16 ms and the first layout 31 ms (font table parsing for the first face); the second layout of a different string takes 0.04 ms.
- Memory (Windows, debug build): discovery adds 0.12 MB of private bytes (faces are mapped, not copied); shaping and laying out 401 wrapped lines with 21,600 glyphs adds 6.5 MB, and rasterizing the 61 distinct glyph keys that text needs adds 0.3 MB.
- Through D3D12 (`aqua-oracle-dwrite/tests/d3d12_glyphs.rs`, Windows, Segoe UI, three strings at 13, 16, 24 and 32 px, four foreground/background pairs, glyphs resolved through `Residency`, uploaded, drawn by the D3D12 glyph shader on WARP and the RX 9070 XT, read back and re-encoded to sRGB): total ink versus `IDWriteBitmapRenderTarget` grayscale is 0.88 to 1.05 (black on white 1.02 to 1.05, white on black 0.96 to 1.00, navy on cream 1.00 to 1.04, gray 200 on 30 0.88 to 0.96); mean per-channel error 9 to 18 for the high-contrast pairs (position and shape differences, not blend), 2.4 for gray on dark. D3D12 and `aqua-gpu-ref` agree within 1/255 on both adapters. Without `linear_blend_alpha`, plain linear blending of the toned coverage gives 0.63 of DirectWrite's ink for black on white.
- Fallback index: `FallbackIndex` (`fallback.rs`) stores the cmap coverage of each scanned face as merged codepoint ranges keyed by file path, face index, length and modification time; a stale record is replaced, never trusted. `FontManager::use_fallback_index(persisted)` enables it, `fallback_index_snapshot()` returns the bytes to write when it changed, and `aqua-fonts-system` supplies `read_cache_file`, `write_cache_file` (atomic rename) and `default_fallback_index_path` (`%LOCALAPPDATA%\aqua\font-fallback.index`). The host loads it at start and writes the snapshot when idle. A corrupt or foreign file decodes to an empty index.
- Prewarm: `FontManager::prewarm_jobs(&[TextStyle])` resolves the primary and fallback faces under the lock and returns `PrewarmJob`s that own their `FaceBytes`; `PrewarmJob::build` (the 31 ms per face of table parsing, shaper data and the ASCII glyph map) runs on any worker without the lock and `install_prewarmed` takes the lock for a map insert, discarding entries built before a font-set change. The host owns the worker thread, because `aqua-text` spawns none.
- Open in the text stack: Slug outlines for scale animation are not built; CBDT/sbix strikes are tested against a synthetic sbix font and the placement math only (no CBDT/sbix font is installed on the test machine); 1-, 2- and 4-bit EBDT mask strikes are not decoded; fake bold does not widen advances; strikes ignore the subpixel bin (four identical atlas entries per color glyph). `aqua-render` still carries glyphon and cosmic-text because `GlyphonProvider` is its only `TextProvider`; the replacement is the lowering above (`GlyphResolver::lower` plus `glyph_passes`/`image_pass`) in the host, after which `crates/aqua-render` and the two workspace dependencies are deleted together. `ShapedRun` no longer exposes a glyphon type.

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

### 10.5 Authoring motion

Today authors drive `MotionController` by hand. The builder API gains declarative motion: styles say what the value is, motion says how it gets there. When `render` reruns and a node's resolved style value differs, the runtime calls `set_target` on that node's track for that property.

```rust
const CARD: Motion = Motion::spring(SpringParams::SNAPPY);
const FADE: Motion = Motion::tween(0.12, Easing::EaseOut);
const ROW: MotionToken = MotionToken::new("row");

Box::new()
    .styles([ROW_STYLE, ROW_BASE, row_style])
    .animate(Animate::new().fill(CARD).border_color(CARD).frame(ROW))
    .presence(Presence::new().enter(FADE).exit(FADE))
    .build()
    .with_key(t.id)
```

Every animated property is listed explicitly; nothing animates by default and nothing animates on first mount. Motion values are `const` tokens in `aqua-style` (`Motion`, `MotionToken`, `MotionRef`, `MotionTheme`), so a theme can retune all motion in one place, and `MotionRuntime::set_theme` retargets in-flight token-referenced tracks live (literal motions are unaffected).

Implemented: `Animate` (opacity, fill, border_color, text_color, translate, frame) and `Presence` (enter, exit) live in `aqua-style/src/motion.rs` and ride on `Paint::motion`; `aqua-ui/src/motion/runtime.rs` (`MotionRuntime`, owning an `AppMotion`) diffs resolved values after layout and emits `set_target`; a node's first sight never animates, and `enter` applies only to nodes that arrive after the first reconcile. `aqua-layout/src/edge.rs` (`EdgeAnimator`) gives each moved edge its own track and none to stationary edges. Exit: `UiTree::sync_children` keeps a keyed node with an exit motion in the tree as an absolutely positioned, hit-test-excluded node at its last frame; `MotionRuntime::handle` removes it when every track it owns settles (the opacity track is always retargeted to 0, so a `Settled` event always arrives). A returning key reuses the node and retargets opacity from the presented state. Presented values exist only on the render side (`TrackSet`); `MotionRuntime` exposes targets and track handles, never sampled values, which is the app-side read rule.

The debug-mode read check is `aqua_reactive::read_check`: `render_scope(&deps, render)` panics in debug builds on any `ReadSignal::get/try_get` of a signal outside the declared tuple, naming its creation site (`#[track_caller]` on every signal constructor); `unchecked` exempts a region. `aqua-runtime::mount_reactive` and `ReactivePopup` wrap `render` in it. Not yet wired: the host does not yet call `MotionRuntime::reconcile`/`handle`, enter/exit do not yet apply to overlay popups, and presented values are not yet applied to scene property trees by the host.

## 11. Memory

Measured on 2026-11-04 (see 11.1). The first-draft table was an estimate and underestimated the driver by an order of magnitude; the revised budget below is what the gates enforce.

| Component | Private bytes, discrete GPU | Design choice |
|---|---|---|
| D3D12 device (driver private) | 43 MiB measured, vendor-fixed | one device, debug layer off; not reducible by Aqua |
| First `Present` (driver swapchain state) | 40 MiB measured, vendor-fixed (WARP: 0.1) | one swapchain; not reducible by Aqua |
| Aqua queue, rings, descriptor heaps, pipeline library | 10 MiB (52.6 for device plus renderer, less the 43 MiB bare device) | few PSOs, one descriptor heap, reused command allocators |
| One 2560x1440 BGRA8 surface | 15 MiB | double-buffered swapchain adds one more per extra buffer |
| First-frame and warm-up caches (pipelines, upload ring, transient heap) | 9 MiB | transient heap 3.3 MiB for the editor workload, aliased |
| Framework heap (renderer side, measured) | 0.4 MiB | arenas, struct-of-arrays, zero steady-state growth |
| Steady-state growth over 600 frames | 0 MiB (gate: 1 MiB) | no per-frame allocation |
| Fonts | ~0 | memory-mapped, shared |
| Glyph atlas, CPU side | ~0 | upload ring, no CPU mirror |

Budget (enforced by `aqua-verify`): the offscreen renderer path on a discrete adapter, 2560x1440 BGRA8, the editor workload, adds at most 96 MiB of private bytes over the bare process (measured 72 to 78), a framework heap of at most 4 MiB, steady-state growth of at most 1 MiB over 600 frames, and at most 48 MiB of local video memory (measured 31.6). The windowed editor target is the offscreen figure (34 MiB Aqua-controlled: renderer 10, surface 15, caches 9) plus a second swapchain buffer (15), the application heap (about 4) and slack, 57 MiB, on top of the 83 MiB driver floor Aqua cannot influence (device 43, first `Present` 40): 140 MiB private on a discrete GPU. This is not half of the GPUI Editor: the GPUI Editor measures 74 MiB private (debug build, the only one prebuilt; section 12.3), so on a discrete GPU the D3D12 driver alone costs more than that whole process. The old claim of "half of 25 MB" is withdrawn; the budget that remains meaningful is the Aqua-controlled share, zero steady-state growth, and VRAM. The windowed figure is gated once the host runs the Editor (the comparison script reports it). Transients are aliased and sized to effect bounds (layers still allocate full-surface RGBA16F, 29 MiB at 1440p, and need bounds-sizing); the atlas is an 8-bit texture grown in pages with eviction. The adapter responds to video memory budget changes and frees transient heaps when minimized.

Implemented residency (`aqua-media`, verified against `aqua-gpu-ref` on WARP and a hardware adapter): one `Atlas<K>` per texel format (8-bit mask for glyphs and single-colour paths, RGBA8 for images, SVG and gradient paths). Shared pages grow 256, 512, 1024 up to a page cap; items larger than a quarter of the maximum edge get a dedicated exact-size page. Allocation is shelf packing without a CPU mirror. When a budget or the page cap is hit, the least recently used page that holds nothing used in the current frame is reset (shared) or released (dedicated), so a page can never be overwritten while an earlier draw of the same frame still reads it; if every page is in use the atlas grows past budget and counts it. `trim_idle` releases pages idle for N frames (minimize, memory pressure), and the D3D12 side frees the matching `ID3D12Resource` behind the frame fence. Large uploads are split into row bands and streamed under a per-frame byte budget. Paths and SVG are rasterized on the CPU (tiny-skia, resvg) into the atlas, not rendered by a GPU path pipeline; Slug-style rendering for scaled text and animated paths is not built. After device recovery the host must call `Residency::clear` between frames (it drops every page, pending update, streaming queue and retained vector source, and bumps `Residency::generation`), and reset any `PageTable`; the next frame re-creates and re-uploads whatever it draws, so a recovered device never sees a release or write for a page it does not have. Glyphs go through `Residency::glyph_any`, which keeps the 8-bit mask and the RGBA color-glyph atlases apart.

Pure crates get an exact heap gate: `aqua_verify::heap::CountingAllocator` counts allocations, reallocations and bytes per thread plus live and peak bytes process-wide, and `HeapGate` fails a test on any excess. Gates in force (`aqua-verify/tests/heap_gates.rs`): an idle `Runtime::build_frame` performs zero heap events at 100, 1000 and 4000 rows and shapes, measures and emits nothing; an idle `RenderSide::step` with 10 and 5000 scene items performs zero heap events and requests no wake; `emit_scene_diff` of an unchanged 2000-item scene allocates nothing; a cold mount shapes each text run exactly once. The earlier `aqua` perf_scene failure (12 allocations per idle frame) does not reproduce on the extracted runtime.

## 12. Verification

- **Conformance suite:** the same `FramePlan`s run on every `GpuBackend`; output is diffed against `aqua-gpu-ref` with per-channel tolerances. Every tier of the D3D12 adapter runs it.
- **Differential fuzzing** of fast paths against reference implementations from one scene generator.
- **Replay determinism:** recorded `Input` logs replay to identical scene and plan hashes.
- **Exact counters:** allocations, instances, glyphs, layout nodes, passes per scene.
- **Text oracle:** DirectWrite pixel diffs (section 8).
- **Motion checks:** no one-frame jumps, no unmount above zero opacity, no motion on first mount, settle within declared bounds.

### 12.1 Reference semantics (`aqua-gpu-ref`)

`aqua-gpu-ref` defines what every `FramePlan` op means. Backends match it within the tolerances below. Pixel output is premultiplied linear-light RGBA, quantized to 8 bits per channel with round-to-nearest.

| Op | Reference definition |
|---|---|
| Frame start | The surface holds `plan.clear` (premultiplied). A plan that never writes the surface therefore yields the clear colour. |
| `LoadOp` | `Clear(c)` fills the whole target before the pass; `Load` and `DontCare` keep the contents, a never-written transient is transparent. |
| Quad | Signed-distance coverage `saturate(0.5 - d)` for the rounded rect, the same for the clip rect, border blended over the fill with `saturate(0.5 - inner)`, then `x opacity`. |
| Gradient | `t` is the projection of the pixel centre on `start -> end` (absolute pixel space), mapped through the two stop offsets and clamped. Colours interpolate in Cartesian Oklab (straight alpha), then premultiply. Backends may dither by at most half an 8-bit step. |
| Shadow | Exact Gaussian-blurred rounded rect: Gaussian-weighted integral over y of the closed-form erf x-extent of the rounded rect at that row. Spread grows the rect and the corner radius. `sigma < 0.05` is a hard signed-distance edge. |
| Glyph | Hard pixel-centre inclusion in `bounds`, nearest texel of the R8 atlas inside `texels`, multiplied by clip coverage and opacity. |
| Image | Quad coverage of `bounds` and `radius` times the coverage of `clip` rounded by `clip_radius` (ignored without a clip), bilinear sampling clamped to the `texels` sub-rectangle. Persistent RGBA8 data is premultiplied linear. `Blend::Over` composites `src * k` over the target; `Blend::Replace` yields `src * k + dst * (1 - k)` (premultiplied lerp), with `k = coverage * opacity`. Backdrop blur composites with `Replace`, so a translucent backdrop is replaced, not stacked. |
| `LayerOver` | `src * opacity` composited over the target. |
| Blur down / up | Dual Kawase. Down: 4x centre plus four diagonal taps at `0.5 * offset * (source/dest ratio, at least 1)`, divided by 8. Up: four axis taps at `offset`, four diagonal taps at `offset / 2` with weight 2, divided by 12. All taps are bilinear and clamped to the pass region. |
| Copy | Texel copy between `source_origin` and `dest_origin` of `size`. |
| ColorMatrix | Per pixel: unpremultiply (alpha 0 gives rgb 0), `out = clamp(M * [r g b a 1])` with the 4x5 row-major matrix applied to straight color, premultiply. Source and target extents must match. Implemented by the safe twin `aqua_kernels::color_matrix::reference`. |
| Custom | The reference runs a CPU kernel registered under `CustomPassSpec::name` (`Kernels::register_custom`); the standard set holds `aqua.invert`. A name with no kernel is `Unsupported`. Pass reads are inputs, the single write is the output; the HLSL contract is in section 14.9. |
| Compute | The reference runs a CPU kernel registered under the `ShaderId::Custom` id (`Kernels::register_compute`); the standard set holds id 1, a row box blur over 64-wide tiles (`groups = [ceil(w/64), h, 1]`, `params[0]` is the tap radius), so a partial dispatch leaves the rest of the target untouched. |
| Feature gating | A backend that reports `Features::color_matrix`, `compute` or `custom_passes` false is not asked to run plans that need them: the suite and the contract checks skip those cases (`conformance::requirements`) and report `SKIP: not offered by the backend`. Every other op is mandatory. |

### 12.2 Conformance suite

- `aqua_gpu_ref::conformance::run_suite(reference, backend, config)` runs hand-written cases (every op, clips, nested and sibling groups, degenerate geometry, odd extents) plus seeded fuzz scenes from one generator, then the backend contract checks (unknown surface, extent mismatch, invalid plan, determinism, surface reuse without residue, `recover`).
- The case list covers rounded clips (fractional, nested, under an outer clip, inside groups), bounded layers away from the origin and past the surface edge, backdrop replace on translucent surfaces, a blur-radius sweep across every octave boundary, color matrix, custom and compute passes, and explicit clear plans.
- Each pixel is classified `Interior`, `Soft` (shadow and blur footprints) or `Edge` (within `edge_band` pixels of any shape or clip boundary) and diffed per channel against the limit of its class. Defaults: interior 4, soft 8, edge 24 of 255 (measured on a Radeon RX 9070 XT and WARP: worst 1, 4 and 4).
- `tolerate_unsupported` turns `Unsupported` errors into reported skips; it exists only for backends that are on their way out (the wgpu adapter).
- The D3D12 adapter runs the suite on every adapter preference through `aqua-verify/tests/d3d12_conformance.rs`, built with `aqua-win-build -p aqua-verify --test d3d12_conformance` and executed on Windows.
- The enforcement tests of section 4 live in `aqua-verify` (`tests/enforcement.rs`): `cargo metadata` purity over the Linux and Windows target graphs, `forbid(unsafe_code)`, and a source lint for clocks, threads, filesystem and stdio in pure crates. The lint ignores comments, strings and `#[cfg(test)]` items.

### 12.3 Verification harness and results (2026-11-04)

Everything lives in `crates/aqua-verify`; Linux runs the pure parts, Windows runs everything that touches Direct3D (`aqua-win-build -p aqua-verify --test NAME`, then run the `.exe`).

- **Exact counters and heap gates:** `src/heap.rs` (`CountingAllocator`, `Counts`, `HeapGate`, `measure`), `src/wake.rs` (`WakeTally` over `RenderWake`), `tests/heap_gates.rs`. Passing on Linux.
- **Frame-time distribution:** `src/stats.rs` (`Distribution` with p50, p90, p99, p99.9, max; `jank` counts frames over a budget and the longest run). `FrameReport.gpu_time` is now populated by the D3D12 adapter (timestamps), so the memory test reports both.
- **Private memory and VRAM budgets:** `src/budget.rs` (`MemoryBudget`, `RENDERER_OFFSCREEN`), `src/process.rs` (private bytes, working set, DXGI local usage), `tests/memory_budget.rs`.
- **Differential fuzzing across tiers:** `src/differential.rs` plans each fuzz scene under four planner variants (baseline, bindless, 4 KiB heap alignment, 256-texel texture limit); `tests/differential.rs` requires the oracle to render all four bit-identically (160 scenes, passing), and `tests/d3d12_differential.rs` runs 120 scenes x 4 variants on WARP and the RX 9070 XT across the Baseline, Bindless and Bindless+enhanced-barrier tiers against the oracle, tiers against each other (interior 1, soft 2, edge 2) and hardware against WARP.
- **Motion jump sweep:** `tests/motion_sweep.rs` retargets springs and tweens at random times, then requires the presented value to be independent of the sampling rate (30, 60, 144, 240 Hz against a 1440 Hz reference), no one-step jump in a 1 kHz trace, and exact settling on the target. Passing.
- **Hot-path profile:** `tests/profile.rs` (`--ignored`, release).
- **Editor comparison:** `tools/compare-editors.sh` drives `win-app` (window-targeted, posted messages only) against the GPUI Editor and the Aqua Editor and prints one table; the new `win-drive.ps1` steps `cpu`, `gpumem`, `wakes`, `lat` and `scrolllat` sample process CPU time, private and working-set memory, dedicated GPU memory, thread context switches per second, and the round-trip latency of `SendMessageTimeout(WM_NULL)` while idle and while a wheel burst is posted (a UI-thread stall distribution that is comparable between the two apps).

Results on the RX 9070 XT, WARP and this machine:

| Measurement | Result |
|---|---|
| Idle `Runtime::build_frame`, 100 to 4000 rows | 0 heap events, 0 runs shaped, 0 layout nodes measured |
| Idle `RenderSide::step`, 10 to 5000 items | 0 heap events, 0 wakes requested over 120 steps |
| Editor workload at 2560x1440 (16 passes, 4155 instances), RX 9070 XT, release | CPU submit p50 0.12 ms, p99 0.51 ms, max 0.84 ms; GPU p50 0.091 ms, p99 0.19 ms, max 0.27 ms; 0 of 600 frames over 16.7 ms |
| Same workload on WARP (software) | CPU p50 16.0 ms, p99 18.0 ms; 95 of 600 over 16.7 ms |
| Private memory, RX 9070 XT (added over bare process) | device and renderer +52.6 MiB (of which bare device 43), surface +15.0, first frame +4.4, warm-up +4.3, steady state +0.0; total 76 MiB; VRAM 31.6 MiB; renderer heap 0.36 MiB |
| Private memory, WARP | 37 MiB total; +3.8 MiB growth over 600 steady frames (WARP's own caches, not Aqua) |
| Differential, oracle against all planner variants | 160 scenes x 3 variants bit-identical |
| Differential, D3D12 tiers against each other and hardware against WARP | agree within interior 1, soft 2, edge 2 (WARP versus hardware default tolerance) |
| Differential, D3D12 against the oracle at this commit | fails: the oracle gained rounded clips, backdrop replace and continuous blur; the D3D12 passes do not implement them yet (72 of 480 runs per tier on each of the three tiers and both adapters, soft up to 109, edge up to 189, interior 1). The fixed-seed conformance suite fails for the same cases. The fuzz test is the gate that closes when they land |
| Motion sweep | 40 schedules x 4 rates, 60 jump traces, 40 settle traces: pass |
| GPUI Editor (debug build, 3 runs, median) | idle CPU 7.2% of one core, 5071 thread context switches per second, private 74 MiB, working set 77.5 MiB, dedicated GPU 53.6 MiB, 36 threads; UI-thread stall during a 600-wheel burst p50 0.06 ms, p99 2.4 ms, max 2.9 ms, none over 16.7 ms; 4.2 ms CPU per wheel message |
| Aqua Editor, same script | not measurable: the prebuilt port still runs on the wgpu stack and exits at startup (`adapter lacks required rendering features: CLIP_DISTANCES`); there is no native-stack host to launch. The script is ready and reports the Aqua column once the executable runs |

Driver overhead included in the budget: the device floor (43 MiB) and the first-present cost (40 MiB) are measured on the RX 9070 XT; both are vendor-fixed. The GPUI figures come from a debug build; a release GPUI build was not available and the user's running Editor was not touched, so the 25 MB figure in the old budget could not be re-measured.

Still not measured or not built: replay determinism over the host's `Runtime` (the runtime does not implement `Machine` yet; `Recorder`, `replay` and `first_divergence` exist in `aqua-protocol`, and `aqua_verify::fingerprint` provides the scene and plan hashes), the DirectWrite oracle for Bindless-tier text, integrated GPU and cross-adapter present, and windowed (swapchain) memory.

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

1. **Text look (decided 2026-11-04):** one consistent rasterizer on every platform: unhinted skrifa outlines, grayscale coverage at 4 horizontal subpixel positions with y snapped, no LCD subpixel rendering. The Windows native look is reproduced at blend time by the Microsoft gamma and contrast model keyed on text luminance (`aqua_text::tone::ToneCurve::native`, gamma 1.8, grayscale contrast 1.0). Evidence (section 8.1): unhinted coverage agrees with DirectWrite to a few percent, shaping agrees exactly, and the tone curve brings blended ink within about 6% of DirectWrite's own grayscale output, where the plain coverage was off by 13 to 22%. DirectWrite grid fitting is deliberately not matched (it moves stems vertically by up to a pixel at 13 px); ClearType color fringing is not offered. DirectWrite stays test-only.
2. **UIA: decided, AccessKit's Windows adapter** (`accesskit_windows` 0.35 `SubclassingAdapter`, no winit). Evidence, 2026-11-04: the runtime already emits an AccessKit-shaped snapshot (roles, bounds, text runs, selection) and `aqua-a11y-uia` feeds it through unchanged; a real UIA client (`IUIAutomation`, `FindAll` over the descendants, the Invoke pattern) round-trips against it on Windows in `crates/aqua-a11y-uia/tests/uia_windows.rs`: control types, names, bounding rectangles, Invoke arriving as `A11yAction::Press`, and live relabeling after an update. Hand-written UIA would have to reimplement the element, fragment, fragment-root, text, value, toggle, expand-collapse, range-value, selection and grid providers plus the event queue, with no behavior the adapter lacks. The adapter costs nothing until a client connects: attaching installs one window subclass, `request_initial_tree` only flips the bridge to connected and wakes the host, and no tree is built while dormant. Two constraints follow from it: attach before the window is first shown (`UiaAdapter::attach` returns `WindowAlreadyVisible` instead of panicking), and a `Role::Label` node gets its name from `value`, so the snapshot copies the label into the value for text nodes.
3. **Render thread ownership:** decided 2026-11-04: a pure state machine driven by the host. `aqua_runtime::RenderSide::step(RenderInput) -> RenderOutput` owns the presented scene and a `aqua_motion::TrackSet`, reads no clock and spawns no thread. The host (`aqua-transport::spawn_render_thread` with a `RenderHost` supplying the clock, the frame wait and GPU execution) owns the thread and moves `RenderCommand`/`RenderEvent` over a safe SPSC ring. App side: `aqua_motion::AppMotion` (targets, policy, owners, completions; never samples) plus `aqua_runtime::emit_scene_diff` (drains the app `Scene` into a `SceneDiff`). Completion callbacks fire on the app side when a `Settled { track, generation }` event with the current generation arrives. Motion times are `f64` seconds on the host clock (`Instant::as_secs_f64`); presentation time arrives as `RenderInput::present_at`. Round 2 (2026-11-04): `aqua_transport::GpuRenderHost<B, L, C>` is the `RenderHost` over any `GpuBackend<Error = GpuError>` (the D3D12 `WindowBackend`, `OffscreenBackend`, `RefBackend`). It lowers the render-side scene to `PaintCommand`s through a `Lowering` (`GeometryLowering`: quads, shadows, clips, opacity, content and backdrop blur, translation transforms; text, images and vectors are counted as `unsupported_items` until a richer lowering is plugged in), plans with the backend's `plan_limits`, waits for the frame slot once per frame (`wait_for_frame_slot`, the swapchain's waitable object on D3D12), executes and presents. `spawn_render_thread_with` builds the host on the render thread, because COM-backed backends are not `Send`; the loop now waits on the frame slot before sampling tracks, so `present_at` is the clock plus one frame interval measured after the wait, and `RenderWake::At` blocks on the command ring instead of spinning. A device failure is reported as `Feedback::DeviceLost`; the next frame calls `recover`, recreates the surface and reports `DeviceRecovered`, which makes `RenderSide` resubmit. `RenderSide` now submits a frame when the viewport changes as well as when the scene revision does. The inspector (`aqua_transport::InspectorServer`) is a loopback-only TCP socket speaking one JSON line per request: `stats` (frames presented and failed, device losses, scene items, tracks, last frame record with passes, instances, barriers, CPU and GPU time, present queue time, unsupported items), `tree` (the accessibility-tree JSON the host publishes) and `watch` (a line per published change); it refuses non-loopback addresses.

Popups and scroll glide are routed through tracks. `OverlayHost` owns the single app-side `AppMotion` (`tracks_mut`); `animate_popup(overlay, PopupMotion { property, from, to, kind }, now)` registers an opacity or translate animation, the runtime wraps that popup's portal subtree in a `PushOpacity`/`PushTransform` group (`SceneKey::PortalStart`/`PortalEnd`), binds a track to the group's scene node once it exists and emits `Spawn` and `SetTarget` after the scene diff; closing the popup removes the track. Every scroll node's content is wrapped in an identity `PushTransform` inside its clip (`SceneKey::ScrollContentStart`/`ScrollContentEnd`). A wheel notch moves the logical scroll offset to its final, clamped value at once (layout, virtualization and hit-testing see the target state) and queues the applied distance; the runtime then emits `MotionCommand::Shift`, which moves the presented translation by that distance and retargets to zero with the carried velocity, so the render side glides the content with a critically damped spring (`glide::kind`, 0.25 s) and the app side does no work while it glides. `Track::retarget` and `Track::shift` now advance the track to the retarget time before launching the new segment, which removes a position error of up to one frame of travel when a retarget lands between samples. `MotionController` remains for author-driven values that need app-side sampling. Remaining: damage-limited presentation, glyph, image and vector lowering, and moving author-driven `MotionController` values onto `AppMotion`.
4. **Other platforms:** decided: Windows only (2026-11-04); out of scope until Windows ships. Vulkan, Metal and non-Windows shells are not built because they cannot be tested here; the port stays graph-level so they can be added later.
5. **`aqua-kernels` scope (decided 2026-11-04):** the crate has no dependencies, denies `unsafe_op_in_unsafe_fn`, and every module that uses `unsafe` exports a safe `reference` twin plus a dispatching `apply`. Kernels: `color_matrix` (4x5 matrix over premultiplied f32 pixels, SSE2 on x86_64), `over` (premultiplied span composite with opacity, SSE2) and `diff` (max absolute byte difference for conformance compares and damage detection, SSE2). The fast paths are bit-identical to their twins (same operation order, no fused multiply-add), proven by differential tests in `crates/aqua-kernels/tests`, and other architectures fall back to the twin. The oracle (`aqua-gpu-ref`) calls only twins. `aqua-verify/tests/kernels.rs` enforces: no dependencies, the deny attribute, a twin and a differential test for every unsafe module, and twin-only use by the oracle. Hot paths are admitted when a measurement shows the twin costing frame time; none beyond these three is justified yet. **Profile evidence (2026-11-04, `aqua-verify/tests/profile.rs`, release, Windows x86_64):** the three SSE2 kernels do not earn their `unsafe` on the target: `over` over 3.69 million RGBA32F pixels takes 3.06 ms in the safe twin and 3.02 ms in the SSE2 path (Linux: 4.6 versus 3.3 ms), and `diff` over 14.7 MB takes 0.24 ms versus 0.28 ms (the twin auto-vectorizes). Runtime hot paths measured are all in safe code and mostly in external crates: 1900 glyph rasterizations (skrifa, 5 sizes x 95 glyphs x 4 subpixel offsets) cost 10.9 ms including a 4 ms first-face load (5.7 microseconds per glyph), and 2000 wrapped paragraphs shape cold in 37 ms and re-wrap in 18 ms. The only hot path found is the oracle's shadow integration: at 2560x1440, 7 shadows of sigma 16 take 50.7 s of CPU while 148 quads take 51 ms, 4001 glyphs 75 ms and a backdrop blur 60 ms, so shadows are 99.6% of an oracle frame (a numeric y-integral of two f64 `erf` calls per step, about 48 steps per pixel). Decision: no kernel is admitted for the runtime; the rule stays as written (a kernel needs a measured speedup of at least 1.5x over its twin on the Windows target), `color_matrix`, `over` and `diff` stay only because they exist and are twin-tested, and the first fix for the oracle's cost is algorithmic and safe (hoist the per-row sample weights and half-widths out of the per-pixel loop, bound the footprint), with a batched `erf`-sum kernel admitted only if that is still above 1 s per 1440p shadow frame.
6. **D3D12 device core decisions (2026-11-04, `aqua-gpu-d3d12::hal`):** the Bindless tier requires Shader Model 6.6 and Resource Binding Tier 3; enhanced barriers follow `D3D12_OPTIONS12`; fence barriers and batched copy lists stay off until the `windows` bindings expose their feature queries. A device that fails the baseline (feature level 11_0, SM 6.0) is skipped and the chain ends at WARP. After a device loss the same adapter is re-selected by LUID; a second loss within 10 s falls back to WARP. Recovery consumes the old core so the removed device is released before the replacement is created, which WARP requires. The first `Present` on the RX 9070 XT adds about 40 MB of driver-private commit (WARP: 0.1 MB), so the section 11 budget has to be measured with the real driver in place.
7. **D3D12 scene pipelines and shaders (2026-11-04, `aqua-shaders`, `aqua-gpu-d3d12::scene`):** HLSL under `crates/aqua-shaders/hlsl` is compiled by `build.rs` to signed DXIL, Shader Model 6.0, with `dxc` found through `AQUA_DXC`, `PATH` or the Windows SDK (`dxc.exe` runs from WSL through `wslpath`); the build fails without it, and the adapter never compiles shaders at run time. The plan carries barriers, load ops and aliasing; the executor translates them against a per-resource tracked state (the plan's `from` and `Undefined` are ignored, so an aliased transient is moved from wherever it last was), emits an aliasing barrier at every transient's first use in a frame, and re-places the planner's heap offsets with real allocation sizes (`placement::place`) because D3D12 tile padding makes RGBA16F textures larger than `texels * 8`. Offscreen surfaces default to RGBA16F; BGRA8 UNORM and sRGB targets are supported by passing the render-target view format in `SceneTarget`, and a `Copy` pass whose source and destination formats differ (backdrop snapshots from an 8-bit window surface) is executed as a draw instead of `CopyTextureRegion`. Pipelines are keyed by (kind, target format), prebuilt through `warm`, and loaded from a serialized `ID3D12PipelineLibrary` that the host persists (a corrupt or foreign library is ignored). Not covered: `ColorMatrix`, `Custom` and `Compute` passes (the graph IR and the reference now define them, see items 8 to 10; the adapter reports them unsupported), and the Bindless/enhanced-barrier tiers.

8. **D3D12 window backend (2026-11-04, `aqua-gpu-d3d12::window`):** `WindowBackend` owns a `hal::Core`, one `Swapchain` per `SurfaceTarget::Native`, one `SceneRenderer` and one `PersistentTextures`, and implements `GpuBackend` plus `GpuReadback` (reads the unpresented back buffer). Frame order: `wait_for_frame_slot` (the waitable), `execute` (`Core::begin_frame`, upload batches, scene record, `end_frame` signals the timeline), `present` or `present_with_damage`. Swapchains are `FLIP_SEQUENTIAL`, not `FLIP_DISCARD`: DXGI returns `DXGI_ERROR_INVALID_CALL` from `Present1` for any non-empty dirty rect on a `FLIP_DISCARD` chain (measured on the RX 9070 XT and WARP), and sequential flip keeps the buffer contents the age tracking relies on. Damage: `window::damage::DamageTracker` keeps the last-presented frame per buffer and a short history; `repaint_region` returns what a buffer needs repainted (its age worth of damage plus this frame's), `Damage::Full` for unknown buffers, after resize, after an occluded present or when history ran out. An empty damage slice means full. The executor still redraws the whole surface every frame, so today the tracker feeds only `Present1` dirty rects (this frame's damage) and the `repaint_region` query; scissoring the plan to the repaint region is not built. Recovery (`recover`): wait idle, drop swapchains, renderer and textures, `Core::recover` (or a fresh WARP core if an earlier recovery failed halfway), rebuild renderer (reusing the serialized pipeline library when the old device still serializes it), recreate every swapchain on its HWND, drop pending uploads, then call the recovery listener (`RecoveryNotice { generation, software }`); the host must call `Residency::clear` there because persistent textures are not mirrored. Failed calls probe `Core::check_removed` and latch `device_status() == Lost`; the host reacts by calling `recover`. Not built: frame GPU timestamps (`FrameReport::gpu_time` stays `None`), scissored repaint, Bindless and enhanced-barrier paths, DirectComposition surfaces.
8. **Runtime as a `Machine` (decided 2026-11-04, `aqua-runtime::Runtime`):** `Runtime` implements `aqua_protocol::Machine` over `RuntimeInput`/`RuntimeOutput`; the legacy `Input`/`Output` are deleted. A step is infallible: build errors go to `build_error()` and `take_diagnostics()`. The runtime binds the first window id it sees and addresses every `ShellCommand` to it; `open_window`, `submit` and `request_dialog` queue commands that the next step emits. Per step it emits: `RequestFrame` for input, resize, due wakes and recovery; `SetCursor` and `SetDragRegions` on change; `Viewport` then `Scene(SceneDiff)` after a frame (first frame is the full scene plus order, idle frames emit nothing); IME `Enable`, `Update` and `Disable` from the focused editor's revision, selection, composition and caret area (document windowed by `aqua_ui::events::ime_surroundings`); `ime_area` on change; and `access` as a full `AccessTreeUpdate`, only while an assistive client is attached (`Input::AccessClient { connected }`, an input so replay logs stay complete) and only when the tree epoch, focus or scale changed. Clipboard shortcuts become `ClipboardWrite` and `ClipboardRead { request }`, and the `ShellEvent::Clipboard` answer pastes text; image and file clipboard payloads are not carried by the shell protocol yet. File drag and drop follows `FileDropEvent` with the last pointer position. A `Tick` at or after the last `wake_at` requests an `ANIMATION` frame. `Recorder` plus `replay` and `first_divergence` reproduce every output byte for byte (`crates/aqua-runtime/tests/runtime.rs`).

### 13.1 Status after round 1 (audit, 2026-11-04)

1 done. 2 partial (runtime exists; host not on RuntimeInput/RuntimeOutput). 3 partial (aqua-scene and aqua-graph split; aqua-render still holds the wgpu path). 4 done for D3D12 (89 cases + 8 contract checks pass on RX 9070 XT and WARP, float/BGRA8/sRGB; Bindless and enhanced-barrier tiers not exercised). 5 partial (aqua-motion exists; overlays and scroll glide still sample app side). 6 partial (scene renderer passes conformance offscreen; windowed backend, hal::Core wiring and recovery missing; wgpu not deleted). 7 partial (adapters exist and are tested standalone; unwired; winit still in aqua-platform and aqua). 8 partial (aqua-transport and RenderSide tested with a fake host; not the running loop). 9 partial (own text stack done; glyphon and cosmic-text remain in aqua-render). 10 missing. Missing crates: aqua-style, aqua-layout, aqua-kernels. The Editor port has not run on the native stack. Open: section 14 item 5.
8. **Bounded layers and per-pass origin (2026-11-04, `aqua-graph::paint`):** a layer's transient is sized to the pixel-aligned union of its children clipped to the clip and target (a 1440p group of 200x40 px costs 64 KB instead of 29 MB). The origin lives in the planner, not in the pass: children are emitted in layer-local coordinates (bounds, clips, gradient endpoints and shadow footprints are translated by the layer origin), so passes, backends and the reference need no origin field and stay source compatible. The composite is an `Images` instance whose `bounds` sit at the layer origin in the parent's coordinates and whose `texels` are the whole layer, which samples exactly at texel centers. Rounded clips use the same mechanism with `clip` set to the exact fractional rect and `clip_radius` to the corner radius, so the rounded edge is analytic and independent of pixel alignment.
9. **Custom pass contract and blur continuity (2026-11-04):** a `Custom` pass is a full-target pixel shader. HLSL entry signature: `float4 entry(CustomInput input) : SV_Target` with `struct CustomInput { float4 position : SV_Position; float2 uv : TEXCOORD0; }`, inputs `Texture2D<float4> input0..input7` at `t0..t7` in `reads` order, parameters `cbuffer Parameters : register(b0) { float4 parameters[N]; }` packed from `CustomPassSpec::params`, one `Load` per pixel addressed by `position.xy`; `bounds` is in the output's pixel space. The adapter compiles and caches it by `name`; until it does, it reports `custom_passes` false. Blur is continuous in the radius: below 2 the pyramid is one level with `offset = radius / 2`; above, the lower depth `floor(log2 radius)` is used alone until 75% of the octave, then crossfaded into the next depth (`BlurDepth::Mixed`, two pyramids lerped by two `Replace` image instances) so that at `radius = 2^k` the output equals the single pyramid of depth `k` with offset 1 from both sides. The crossfade costs a second pyramid only in the top quarter of each octave.
10. **Adapter obligations from the graph IR (2026-11-04):** `ImageInstance::blend` and `clip_radius`, the explicit clear pass and `Features::color_matrix` are new contract items. `aqua-gpu-d3d12` must implement `Replace` blending and the rounded clip coverage in the image shader, and may keep `ColorMatrix`, `Custom` and `Compute` unsupported (it reports them false) until its shaders exist; the conformance cases for the first two fail on the D3D12 adapter until then.
11. **D3D12 tiers, transients and timing (2026-11-04, `aqua-gpu-d3d12::scene`, `aqua-shaders`):** `SceneOptions { limit: TierLimit, transient_ceiling, timestamps }` selects tiers; the default is the best tier the device offers (`hal::detect` then `scene::tiers::resolve`). **Bindless:** every pixel shader is also compiled as `<entry>_bl` (Shader Model 6.6, `-D BINDLESS`) with the texture read through `ResourceDescriptorHeap[texture_index]`; the index travels in root constant dword 14 and the root signature is the 16 constants plus the static sampler with `CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED`, so passes and image runs set one constant instead of a descriptor table. `SetDescriptorHeaps` must run before `SetGraphicsRootSignature` for such a signature (the RX 9070 XT returns garbage otherwise, WARP does not care). **Enhanced barriers:** transients are created with `CreatePlacedResource2` and an initial layout; surface and transient transitions become texture barriers derived from the tracked legacy state (`barriers::access_of`), a transient's first use is an activation barrier (`LayoutBefore` undefined, discard) instead of an aliasing barrier, and persistent atlas pages keep legacy barriers because the upload path owns their state. **Fence barriers: not implemented.** Neither `windows` 0.62 nor the 10.0.26100 SDK headers expose them, so the tier is reported off and nothing in the adapter depends on it; revisit when the bindings ship. **Transient heap:** `policy::HeapGate` sizes the heap to the plan's re-placed total plus 25% growth slack, keeps it while the plan needs more than half of it, shrinks it after 240 frames below half, and rejects a plan above `transient_ceiling` with `Error::Budget` instead of allocating; `SceneRenderer::trim(&Trim, fence)` releases the heap behind the frame fence and shrinks upload and descriptor slots back to their initial size (`hal::budget::trim_for` decides when). **Timestamps:** two `EndQuery` stamps bracket `SceneRenderer::record` per slot, resolved into a readback buffer and read in `retire` once the slot fence passed; `gpu_time()` is the latest finished frame (one to three frames old), taken from a throwaway direct queue's `GetTimestampFrequency`. The window backend must copy `renderer.gpu_time()` into `FrameReport`. Verified with `tests/tiers.rs` (every tier combination against the Baseline tier's results on WARP, the RX 9070 XT and the minimum-power adapter, plus BGRA8 on the top tier), `tests/transients.rs` and `tests/heap.rs` (exact allocation counts per steady frame: 23, 11 and 14 for the group, glyph and image cases).
12. **Text blending on a linear pipeline (decided 2026-11-04, `aqua-graph::tone`, `aqua-shaders/hlsl/glyph.hlsl`, `aqua-text::residency`):** the glyph pass applies `ToneCurve::native().linear_blend_alpha(coverage, luminance)` to every mask sample, where `luminance` is the sRGB-encoded luminance of the instance color (unpremultiplied first) and the curve parameters travel in the pass constants (`region` = the four ratios, `values.x` = contrast), so the curve is a data input and not recompiled. Mask instances from vector paths pass through the same function. The remap assumes the backdrop is the opposite extreme of the text; against a mid-tone backdrop the result is between the plain linear blend and DirectWrite's (gray 200 on 30 measures 0.88 to 0.96 of DirectWrite's ink). Alternative rejected: blending glyphs in encoded space, which needs a backdrop read per glyph. One glyph identity everywhere: `GlyphKey.font` is an interned face index owned by the `GlyphResolver`, sizes are quantized to 1/64 px before they enter any key, and `aqua-text` keeps no atlas.
13. **Host composition root (2026-11-04, `crates/aqua`):** aqua is the only event loop. `native::looping::Host` implements the `aqua-shell-win32` `Handler` and drives `Aqua`, which steps the single-window `aqua_runtime::Runtime` Machine with time as an input (`Tick`, `Frame { window, presentation }`) and executes every `RuntimeOutput` in `apply_output` (shell, IME, access tree, render commands, `wake_at`, clipboard feedback). The `Application` trait remains as the compatibility layer for the Editor port, extended with `step_input`, `take_output`, `on_dialog`, `on_motion_event` and `motion_commands`. winit is gone from the runtime; an `Inbox` (mutex queue of `AquaUserEvent` plus the shell `Waker`) replaces the event-loop proxy. Window creation order: hidden shell window, UIA attach, `TsfIme::from_hwnd`, show, render link. One render thread per window via `aqua_transport::spawn_render_thread`; the D3D12 `WindowBackend` is not `Send`, so `RenderLink` ships an `InitFactory` and the backend is built lazily on the render thread (`render::host::DeferredHost` over `GpuFrames`). Pacing: the host waits on the waitable swapchain slot only when the previous iteration presented (`presented_last`); otherwise it falls back to a 60 Hz sleep so an idle or failing present cannot busy-loop. `Presented` and motion events return to the Machine as `GpuEvent::Presented` and `RenderEvent`. Lowering of the display-item scene to `PaintCommand`s lives in `crates/aqua/src/render/lower.rs` (rects, quads, shadows, clips, opacity, blur, scale and translate transforms, glyphs through `GlyphResolver` and `Residency`, retained images and vectors); images reach the render thread over an `ExternalUpdate` side channel. A full command ring drops a command, so `RenderLink` keeps a backlog and retries. Not supported and reported as `WindowError::Unsupported` or counted in `LowerReport.skipped`: snapshot transitions, text raster mode, taskbar badge, materials and transparent windows (DirectComposition), rounded clip radius, inset shadows, dashed borders, general transforms, GPUI items. Not built: a device shared across windows, a persisted pipeline cache, `FrameReport::gpu_time` and the trim path in the host, caption hover forwarding, image and file clipboard payloads.
