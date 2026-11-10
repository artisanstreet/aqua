# Pixel Storm: the path to a 25–50 ms frame

The original declaration-rebuilding benchmark has not reached 25–50 ms. The earlier retained-mode result below uses a different application update API and does not establish a speedup for the original workload or a fair speedup over GPUI.

## Original-workload follow-up

The follow-up keeps `pixel_storm --size 1280x800` in its default rebuilding mode. The application still evaluates the original colour recurrence, calls the Oklch API, constructs every box declaration and submits the new tree for reconciliation. The example source is unchanged throughout this follow-up, SHA-256 `b549d8dda2775e63e018e58bf472ab794f2345dc387189656dbd3c65e51b9f4d`.

A native pilot of that mode, before the request to protect the game screen, records 301.62 ms median and 325.89 ms p99 across 33 intervals in its one complete warm window. That is a single diagnostic baseline, not a paired speedup result. An allocation-counted original-mode run records a median 809 allocations, 7,200 reallocations and 286,849,648 allocated bytes within app phases. Declaration construction accounts for 802 allocations and 262,272,008 bytes of that volume. Counts include positive reallocation growth, not peak live memory or measured physical copy traffic.

Subsequent work opens no Windows benchmark windows. Builds use two jobs at `nice 19` and idle I/O priority in WSL. CPU component diagnostics also run at low priority. Windows execution and presented-frame validation remain deferred while the game screen is protected.

### Granular diagnostics

The new ignored `element-children` construction report uses the same million-cell integer recurrence, row-dependent fold and existing builder/style/Oklch APIs. It counts all constructed cells and rows, allocations, reallocations, allocated growth, deallocations, moved reallocation addresses and the largest allocation. Counters are thread-local and confined to the test executable. The report drops its previous declaration tree between samples and excludes retained runtime/scene state; it is an isolated construction diagnostic, not an end-to-end benchmark.

`moved_capacity_bytes` records the preserved capacity of allocations whose addresses changed. It is not a physical memory-copy counter: an allocator can move mappings without copying their pages. Individual allocation stacks and exact copy volume are not yet recorded.

Linux user-space CPU sampling identifies style application, box construction, colour-cache lookups and libc copy instructions as investigation targets. Disassembly shows SIMD copy instructions already in use. Generic user-space hardware counters are retained, but they do not establish Windows scheduler behavior. In particular, the recorded `context-switches:u=0` must not be interpreted as zero context switches. The available perf build lacks libdw/libunwind support, so full stack attribution is unavailable; selected leaf addresses were resolved separately using ELF section offsets and `addr2line`.

### Rejected candidates

All trial logs and source identities are retained in `pixel-storm-budget/original-workload/`.

| Candidate | Observation | Decision |
|---|---|---|
| Bound lazy child-vector capacity by a useful iterator upper bound | Allocation volume fell from 262,272,000 to 163,968,000 bytes; construction was slower in all three repeats | Reverted |
| Force style/application inlining | No material repeatable construction-time improvement | Reverted |
| Force box finishing to inline as well | No convincing improvement in the pilot | Removed |
| Keep builder paint compact through construction | Slower in its exploratory control/candidate pair | Removed |

These experiments do not establish that fewer allocated bytes or more inlining automatically makes the original workload faster. Execution conditions varied during background work; timings are compared only within each recorded experiment.

### Exact channel-transfer candidate

The remaining candidate changes the implementation of the existing `srgb8_to_linear` function. A lazily initialized table holds the transfer function's 256 possible channel results, calculated with the existing scalar expression on the running platform. The table payload is 1,024 bytes. Callers, the Oklch interface, colour recurrence, box declarations and application source remain unchanged. First use computes the table; warm calls perform three lookups. No approximation or benchmark-specific palette is introduced.

An independent regression checks every possible byte in every channel against the previous scalar expression with exact float-bit equality. A separate direct-API diagnostic converts 1,024,000 RGB values, retains all outputs and produces the same output digest in both executables.

| Direct transfer diagnostic | Control median ms | Candidate median ms |
|---|---:|---:|
| Repeat 1, five samples | 27.073 | 4.039 |
| Repeat 2, five samples | 23.305 | 4.307 |
| Repeat 3, five samples | 23.381 | 4.102 |

This is a transfer-function result, not Pixel Storm frame time. The existing full-colour memo already bypasses many transfer calls, so the application benefit can be much smaller and has not been established. The 25–50 ms original-workload target remains unproven.

The channel-table candidate passes 710 tests across Core, Style, UI, Runtime, Graph, Transport and the reference renderer, with zero failures and seven ignored reports/doc tests. Its Windows Core colour test executable cross-compiles successfully. Windows test execution and native frame timings have not been performed during the screen-protected follow-up.

## Earlier opt-in retained workload

The retained native-sRGB path demonstrates the 25–50 ms target for typical steady-state presented frames at 1,024,000 animated UI boxes. Three final native Windows runs recorded window medians of 27.59–28.65 ms and p99 of 30.03–35.76 ms over 2,120 warm frame intervals. The largest warm interval was 50.77 ms. This is measured throughput on a shared desktop, not a hard latency guarantee or a cold-start result.

Every box remains a real node, every frame evaluates the original integer colour recurrence, and the current geometry, DPI coverage, clipping and frame-completion callback semantics remain. The default example retains its original descriptor-rebuilding mode. Run `pixel_storm --size 1280x800 --retained` to select the new bulk colour-update path; `--retained-oklch` selects the comparable retained Oklch path.

## Implemented path

`FillGroup` binds a validated set of existing UI handles to a reusable colour vector. Its Oklch and native `Srgba8` APIs reject foreign trees, wrong counts, duplicate or stale handles before applying a batch. A colour batch advances the tree revision once. Changes to node declarations, layout, order, clipping, effects or unsupported paint use the structural path.

Colour patches pass through source scene reconciliation and the render scene without reconstructing elements, emitting the UI tree, scanning text bindings or repeating layout. Dense generation-checked slot maps replace per-cell hash lookups. Pooled patch buffers move between stages and return to their owner after consumption. Scene snapshots still receive the latest fills when caret or selection changes require them.

The render host caches a flat-solid draw plan and its node-to-colour mapping. Plain native-sRGB rectangles retain their bounds in GPU vertex buffers and upload a separate four-byte normalized colour stream. The warm colour upload is 4,096,000 bytes for this scene, compared with 163,840,000 bytes for complete 160-byte quad records. Bounds require another 16,384,000 bytes when established; D3D12 retains them per existing fenced frame slot. Geometry, viewport, DPI and eligibility changes rebuild the plan. Mixed Oklch/native solids use the compact general solid pipeline; complex scenes retain normal lowering.

The ordinary authoring path also retains plain bindings, stores common fills inline, shares immutable layouts, and validates stable paint refreshes before mutation. `Element` shrank from 1,208 to 160 bytes. Existing Oklch conversion uses a bounded adaptive exact cache and a batched API; native-sRGB fills avoid the perceptual colour round trip altogether. No colour recurrence or node is skipped.

## Native presented-frame validation, 2026-11-07

The frozen final executable is built from `9059e95a1149d7370bc5559f67a7e2e38b5a655b`, SHA-256 `2ea1748bdb0cd1d35fe087a26cef98b803dc99cc512104cf3b924e35670dcca2`. The target is `x86_64-pc-windows-gnu`, release, default features, D3D11. Native D3D12 correctness is checked separately; these are not D3D12 performance results.

The client is 1280×800 logical pixels, 1600×1000 physical pixels at DPI 120. The adapter is AMD Radeon RX 9070 XT, desktop 2560×1440 at 165 Hz, Ultimate Performance power plan, unchanged default Vsync. Every run uses the same executable, statistics off, `HWND_BOTTOM` and `SWP_NOACTIVATE`. No run raised the benchmark or took keyboard focus. Other desktop activity continued, and machine load varied.

The order was Oklch control 1, native candidate 1, candidate 2, control 2, control 3, candidate 3, then control 2 repeat. Control 2 produced no complete ten-second window and is retained as incomplete, with no established cause. Its replacement is reported separately. Windows 1 and 2, after the first ten seconds following first presentation, define the warm comparison. These are individual window percentiles; their median is not a pooled percentile.

| Run | Warm window | Intervals | p50 ms | p99 ms | Maximum ms |
|---|---:|---:|---:|---:|---:|
| Native candidate 1 | 1 | 360 | 27.66 | 30.03 | 33.43 |
| Native candidate 1 | 2 | 357 | 27.59 | 33.98 | 39.58 |
| Native candidate 2 | 1 | 342 | 28.65 | 35.76 | 42.43 |
| Native candidate 2 | 2 | 354 | 28.08 | 31.67 | 38.92 |
| Native candidate 3 | 1 | 354 | 27.94 | 31.59 | 50.77 |
| Native candidate 3 | 2 | 353 | 28.04 | 32.09 | 33.73 |
| Oklch control 1 | 1 | 128 | 78.70 | 85.96 | 90.91 |
| Oklch control 1 | 2 | 127 | 78.85 | 90.79 | 91.66 |
| Oklch control 3 | 1 | 127 | 78.85 | 85.29 | 86.57 |
| Oklch control 3 | 2 | 127 | 78.86 | 80.79 | 84.59 |
| Oklch control 2 repeat | 1 | 181 | 54.94 | 65.03 | 65.51 |
| Oklch control 2 repeat | 2 | 182 | 54.72 | 59.55 | 63.59 |

The median of the six native window medians is 27.99 ms. Candidate first-present times are 13,554–13,624 ms, and final private bytes are 2,921–2,934 MiB. Cold mounting remains expensive. The first ten-second windows contain roughly one-second outliers, so the warm result must not be presented as a whole-run maximum. The Oklch controls also varied substantially; this is not evidence for a fixed speedup ratio.

An earlier run of the identical final executable recorded 42.50–42.60 ms medians and 48.97–54.35 ms p99. Before hot-field grouping, three repeated runs of the preceding colour-stream executable measured about 48.5 ms medians, while its earlier pilot measured 33–34 ms. All are retained. The final target is demonstrated across repeated runs, but desktop conditions materially affect latency. Padding arena entries to cache-line boundaries increased memory without a measured median improvement and was rejected.

### Instrumented costs and memory

A separate release build at `1ad4d76` enables `profile-alloc` and `AQUA_PROFILE=1`; heap counters explicitly report `true`. Its timings are excluded from the primary comparison because instrumentation and per-frame logging change throughput. The table uses the last 100 recorded frames for each role. Phases can nest and must not be summed as independent wall time. The phase named `layout` includes draining UI commands; actual tree layout is skipped on eligible colour updates.

| Recorded work | Median ms |
|---|---:|
| App frame wall time | 22.065 |
| Reactive evaluation | 5.770 |
| UI command/layout phase | 13.070 |
| Scene build/patch phase | 8.880 |
| Render frame wall time | 17.935 |
| Applying render input | 11.505 |
| Updating cached plan | 1.385 |
| GPU recording | 1.500 |
| Presentation call | 0.100 |
| Render wait phase | 3.360 |
| GPU timestamp | 1.017 |

Every sampled render frame records exactly one pass, 1,024,000 instances and 4,096,000 uploaded bytes. Within measured phases, the app records a median six allocations and 1,800 allocated bytes; the renderer records three allocations, 18 reallocations and 4,194,470 allocated bytes. The remaining roughly 4 MiB allocation comes from rebuilding the dirty queue after it is drained. Patch buffers are pooled, but the complete frame is not allocation-free. Counts include profiling overhead within these phases and do not account for allocations outside recorded spans.

A separate uninstrumented resource probe of the frozen executable reports 52.4 MiB process GPU dedicated memory and 3,034 MiB final private bytes. CPU time grows by 2,281 ms over 4,443 ms wall time, about 51.3% of one core. This short probe includes GPU-memory queries and is not a matched throughput run. Its colder first-present time is 21,441 ms. Neither native geometry residency nor these measurements establishes performance or memory for arbitrary complex scenes.

## Correctness and evidence

The settled full million-box frame-zero capture matches the original rebuilding mode over the complete 1600×1000 client: maximum channel difference one byte, mean absolute channel difference 0.002023/255, and no pixel differs by more than two bytes. An earlier capture during the helper's final resize differed substantially; both that failed comparison and the settled replacement are retained. No image scaling, cropping or tolerance relaxation was used to obtain the passing result.

Both native conformance suites pass: D3D11 hardware and software, float and UNORM surfaces; D3D12 hardware and software, table and bindless paths. Solid cases cover Oklch, native sRGB and mixed fills at scales 1, 1.25, 1.5 and 2. A multi-frame contract changes colours while retaining bounds, replaces geometry, and returns to previous geometry to exercise buffer ownership and slot reuse. Interior output remains exact in the new solid cases; edge differences are at most two bytes with existing tolerances.

The final portable Core, UI, Scene, Runtime, Graph, Transport and reference-renderer suites pass: 715 tests, zero failures, five ignored reports/doc tests. They cover transactional updates, stale handles, allocation reuse, scene invalidation, alpha, caret/selection rebuilding and motion restoration. All 72 changed Rust files pass rustfmt; workspace-wide formatting is blocked by existing vendored Taffy formatting. Strict Linux clippy remains blocked by the pre-existing unused payload in `aqua-reactive/src/read_check.rs:8` under `-D warnings`.

The final retained implementation artifacts are in `pixel-storm-budget/retained/`, including raw trial logs, executable/source identities, all reported windows, the incomplete control, prior measurements, native conformance, portable validation and visual comparison. The original GPUI benchmark in `pixel-storm.md` is unchanged and is not a matched comparison with this new API path. The original user checkout and its uncommitted work are untouched.

## Baseline investigation

The new ignored `aqua-ui` integration test `pixel_storm_phase_report` separates the integer recurrence, canonical colour conversion, element construction, lowering, retained refresh, command draining, cached layout, scene emission and direct paint mutation. It also verifies that a colour-only refresh preserves node identities, geometry and the layout revision while updating every emitted fill.

These are headless CPU measurements. They do not include the application runtime's scene reconciliation, transport, render lowering, graph planning, GPU execution or presentation. The additional lowering and direct-write cases are independent diagnostics, not additional stages to sum into a frame time. Temporary vectors, diagnostic allocation history and the shared machine affect the numbers. They are suitable for identifying expensive work, not predicting the final displayed frame time.

Linux release, three samples at 1280×800, median milliseconds on the unchanged framework at e5836e8:

| Work | Median ms |
|---|---:|
| Original integer recurrence, including an RGB output vector | 5.00 |
| sRGB bytes to Oklch | 78.33 |
| Oklch to GPU colours, without the renderer's colour memo | 27.97 |
| Element construction from computed colours | 375.02 |
| Standalone descriptor lowering | 724.92 |
| `View::set_content`, including its lowering and binding walk | 1201.71 |
| Applying queued commands | 473.30 |
| Unchanged layout | 0.001 |
| Emitting the UI scene | 297.84 |
| Direct `UiTree::set_paint` mutation | 92.32 |
| Scene emission after that direct mutation | 261.46 |

Even the direct mutation diagnostic plus emission is hundreds of milliseconds before renderer work. Bypassing the diff while keeping the rest of the current path is insufficient. The colour round trip also exceeds the whole target by itself on this build. Neither observation is a hardware limit: both describe current software work.

Native Windows release CPU check, the same million-box workload with three samples:

| Work | Unchanged framework median ms | Small local prototype median ms |
|---|---:|---:|
| Integer recurrence | 6.69 | 6.82 |
| sRGB bytes to Oklch | 301.47 | 307.10 |
| Uncached Oklch to GPU colours | 195.33 | 198.12 |
| Element construction | 508.45 | 495.18 |
| `View::set_content` | 2771.86 | 2345.77 |
| Applying queued commands | 815.32 | 844.08 |
| Scene emission | 330.50 | 323.93 |
| Direct paint writes | 217.25 | 149.04 |

The prototype reuses allocated box paint, avoids keyed tables for stable unkeyed children, skips an empty binding-disposal sweep and short-circuits colour ancestry lookup for an empty interaction store. The latter does not help this workload after mount: plain nodes still receive interaction entries. All 318 non-ignored UI tests pass with the prototype. These changes save some CPU work but leave seconds of framework work. The prototype is retained as a reconstructable patch, not integrated or offered as the 25–50 ms solution. This is one control/prototype pair with three internal samples each, not a statistically established frame-time comparison. An earlier candidate run overlapped a build and is explicitly excluded.

### Exact colour-cache experiment

An independent native Windows test uses bounded direct-mapped caches, retains them across three changing frames and checks every output against the existing conversion functions with exact float equality. The reverse cache uses the renderer's four-word colour-key hash. Both measured phases include output-vector allocation. At this research stage, the cache was experimental test code and production conversion and renderer policy were unchanged.

| Slots per cache | Forward conversion median ms | Reverse conversion median ms | Sum of phase medians ms |
|---|---:|---:|---:|
| 256 | 135.64 | 91.56 | 227.20 |
| 4,096 | 34.88 | 25.69 | 60.57 |
| 65,536 | 14.98 | 11.98 | 26.96 |

The largest caches miss on about 33,000–35,000 of 1,024,000 values in each frame. No colour recurrence, colour value or node is omitted. This shows that the existing Oklch API can be much cheaper for this workload without an approximation. It depends on repeated colours; it does not establish performance for a million distinct colours. Two fixed large caches would also consume several MiB, so ordinary applications need a bounded adaptive policy and memory measurements rather than unconditional large tables.

Adding the separately measured integer phase gives about 34 ms of CPU colour preparation with the large exact caches. That leaves about 16 ms for all remaining work under a 50 ms ceiling. It cannot support a 25 ms end-to-end claim on this measured path. Keeping native RGB through ordinary solid fills is the stronger route to the lower target.

## Confirmed sources of work

1. `Element` is 1,208 bytes, `NodeSpec` 824 bytes, `NodeRecord` 464 bytes and `Paint` 504 bytes in this release build. A million-element return value alone occupies over a gigabyte before lowering and retained storage. Reusing child descriptors removed duplicate lowering, but still constructs one complete descriptor tree each refresh.
2. `View::set_content` calls `lower_all`, updates the tree and then recursively calls `bind_new`. `lower_specs_in` materializes vectors even for single-node results. `UiTree::sync_children` builds keyed and positional reconciliation structures for unchanged unkeyed lists, including the empty child lists at leaves.
3. `apply_bindings` runs even when a node has no bindings. It clears input and canvas state, marks node paint through `get_mut`, then queues `InteractionCommand::Bind`. That command allocates a boxed layout, a boxed paint and a boxed command. `bind_new` separately boxes a typography command. That is four boxed allocations per ordinary node per refresh, independent of colour math. It also causes repeated interaction-state processing after refresh.
4. `BoxContent::set_paint` replaces its boxed paint allocation. `prune_dead` constructs a live-node hash set even when there are no owned binding scopes or collections to prune.
5. Every content change goes through scene emission and key reconstruction. `Runtime::emit_tree_keyed` builds per-node operation counters in a hash map. `Runtime::reconcile` reconstructs and checks order and looks up scene handles. Scene differences carry complete display items; the render side looks up handles again. The draw path lowers and plans the scene again even when all geometry is unchanged.
6. Both native GPU backends use a 160-byte `QuadGpu`. A complete million-box instance stream is 163.84 MB even though only its colours change. The current layout carries border, gradient, transform and clipping data along with the fill. The D3D11 renderer has a bounded colour memo, so the standalone uncached reverse-conversion measurement is not a measurement of its actual packing cost.

Relevant sources: `aqua-ui/src/binding.rs` (`set_content`, `bind_new`, `apply_bindings`, `prune_dead`), `aqua-ui/src/element.rs` (`lower_one`, `lower_specs_in`), `aqua-ui/src/tree.rs` (`sync_children`, `set_paint`, `emit_scene`), `aqua-runtime/src/runtime.rs` (`emit_tree_keyed`, `reconcile`), `aqua-runtime/src/render.rs` (`apply_scene`), `aqua-graph/src/paint.rs` (`plan_paint_with`), and both GPU backends' `src/pack.rs`.

## Original implementation sequence

### 1. Retain binding and interaction declarations

Record whether bindings, typography ownership, input handlers, semantics and control declarations changed. A fill-only refresh must not clear or reinstall them, queue interaction commands or traverse unrelated nodes for disposal. Preserve disposal and rebinding on actual declaration or structure changes. Keep this information in compact node metadata, rather than restoring a hash-map entry and reactive scope for every plain node.

Acceptance: a warmed colour-only refresh performs zero binding-command allocations and preserves subscriptions, input behavior and inherited interaction colours. Transitions from plain to reactive and back, handler replacement, typography changes, removal, portals and exit transitions must retain their current behavior.

### 2. Make refresh data compact and transactional

Separate static layout and structural declarations from dynamic paint values. Use compact common-box descriptors and shared layout identities; move uncommon widget properties out of the common representation. Reuse descriptor storage and reconcile unchanged unkeyed topology positionally. Avoid constructing a second complete `NodeSpec` tree on a stable refresh.

The authoring path must still validate before committing mutations. A later invalid descriptor cannot leave earlier children partially updated. Key changes, kind changes, reorders, fragments and managed collections require the existing structural path. A benchmark-specific grid node, texture replacement or skipped recurrence would not satisfy the target.

Acceptance: warm per-frame element/descriptor work is allocation-free or bounded by changed structure, with measured bytes touched per box. Preserve uncontrolled editing, typography, keys and tree ownership. Optimizing only the retained setter leaves the 1,208-byte element construction cost intact.

### 3. Compile stable scenes into updateable paint slots

Retain the source-node-to-instance mapping and draw plan after the initial structural build. Carry typed colour patches to stable slots instead of re-emitting display items, rebuilding scene keys, sorting a full dirty list, lowering shapes and parsing/replanning the graph. Batch contiguous patches for dense updates; retain sparse updates for normal applications.

Slots need generation and plan-version checks so a stale patch cannot modify a replacement node or an obsolete plan. Changes to geometry, draw order, clip/effect context, borders, resource use or draw eligibility must invalidate the affected plan and use the structural path. Zero-alpha culling and colour-dependent visibility need explicit handling; assuming every fill can be repacked into any existing plan would be incorrect.

Acceptance: a colour-only frame performs no layout, shape lowering or graph planning. Compare patched output with a fresh full build, including opacity changes, removal and slot reuse, scrolling, clipping, transforms and effect groups. Test both sparse and million-value updates.

### 4. Remove unnecessary colour round trips

The integer recurrence is inexpensive. Its call to `Oklch::from_srgb8` is not: it includes transfer functions, three cube roots and polar conversion, followed later by inverse conversion for GPU packing. Preserve encoded RGB input in a general colour representation and convert when a perceptual operation actually requires it. Oklch-authored colours, interpolation and colour effects retain their existing semantics.

Keeping the exact current Oklch-returning API on every cell means its work must instead be accelerated through an exact bounded cache or verified vectorized/batched conversion. Merely caching GPU output does not remove the application's forward conversion. Measure hit rates on changing colours and ordinary application workloads; do not assume the benchmark's repeated colours imply the same benefit elsewhere.

The cache experiment establishes a concrete API-preserving option for this scene. A general native-RGB representation is an API/design change from the current Oklch-authored style model; it must preserve the encoded premultiplied blending and Oklab-gradient rules in architecture section 6.8. It must not reinterpret existing Oklch values or silently change colour math.

Acceptance: colour evaluation and packing fit their explicit budget, and fresh/reference rendering preserves colour output and blending. No enlarged pixel tolerance to conceal an approximation.

### 5. Retain geometry on the GPU and upload colour streams

Split persistent bounds, clip and shape data from dynamic colour attributes. An opaque sRGB fill can use a four-byte normalized colour attribute; a float colour stream uses 16 bytes. At 1,024,000 boxes these are 4.096 or 16.384 MB per update, instead of 163.84 MB of complete quad records. Retain the general quad pipeline for shapes requiring the other attributes.

Use existing frame-slot fences or per-slot dynamic buffers to prevent overwriting in-flight data. Rebuild static geometry on structural, layout, DPI, device-loss or relevant effect changes. Keep painter order, coverage and clipping unchanged on D3D11 and D3D12.

Acceptance: record actual uploaded bytes, CPU submission time and GPU timestamp time. A million instances must batch into a bounded number of draws while remaining a million independently addressable nodes.

## Original budget and proof requirements

Use 50 ms as the first hard ceiling; 25 ms is a subsequent target. An initial engineering budget is 8 ms for recurrence and colour preparation, 12 ms for retained evaluation and paint patches, 10 ms for packing/upload/submission, and 15 ms for GPU execution and presentation, leaving 5 ms headroom. These are budgets to test, not measured predictions. The serial frame-completion callback currently gates the next update; thread overlap cannot be counted as a benefit without measuring the same callback semantics.

Verify on the real Windows benchmark with unchanged colour recurrence, logical dimensions, DPI and presentation settings. Report warm present-to-present p50 and p99, all frame counts, allocations, uploaded bytes, CPU/GPU phases and startup separately. Alternate immutable control/candidate runs on a quiet machine. Preserve visual checks and the existing GPUI result. Headless CPU experiments establish where to work; only the complete native benchmark establishes the target.

## Retained artifacts and validation

Raw baseline measurements, exact executable hashes, the small prototype patch and validation logs are in `docs/scoreboard/pixel-storm-budget/`. The phase harness uses an equivalent fixed-viewport grid; it does not run the actual application's reactive closure or native frame loop. The original project checkout is unchanged. Production-source experiments were restored at the end of the initial investigation. The retained implementation described above was subsequently developed and measured in a separate worktree.

The small prototype passes all 318 non-ignored Linux UI tests, including the new identity/layout/fill regression. Both immutable Windows phase executables pass that regression and the ignored million-box report. The final Windows colour experiment checks both conversion directions for every value across all nine cache/frame combinations. The final default harness test passes with two reports ignored. Changed-file rustfmt and `git diff --check` pass. Strict Linux clippy is blocked in the unchanged dependency `aqua-reactive/src/read_check.rs:8`: the release `Render(Vec<NodeId>)` payload is unused and `-D warnings` rejects its existing dead-code warning. That dependency is unchanged.
