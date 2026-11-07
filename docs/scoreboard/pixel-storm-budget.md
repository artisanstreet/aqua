# Pixel Storm: the path to a 25–50 ms frame

The target is a steady-state presented frame for 1,024,000 animated UI boxes. Every box remains a real node, every frame evaluates the original integer colour recurrence, and the current geometry, DPI coverage, clipping and frame-completion callback semantics remain. Cold mount is measured separately. The target has not been demonstrated.

The present implementation repeatedly rebuilds representations of otherwise unchanged state. Removing that work requires a path from a colour change to an existing GPU instance. Small improvements to reconciliation cannot deliver the target on their own.

## Measurements

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

An independent native Windows test uses bounded direct-mapped caches, retains them across three changing frames and checks every output against the existing conversion functions with exact float equality. The reverse cache uses the renderer's four-word colour-key hash. Both measured phases include output-vector allocation. The cache is experimental test code; production conversion and renderer policy are unchanged.

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

## Implementation sequence

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

## Budget and final proof

Use 50 ms as the first hard ceiling; 25 ms is a subsequent target. An initial engineering budget is 8 ms for recurrence and colour preparation, 12 ms for retained evaluation and paint patches, 10 ms for packing/upload/submission, and 15 ms for GPU execution and presentation, leaving 5 ms headroom. These are budgets to test, not measured predictions. The serial frame-completion callback currently gates the next update; thread overlap cannot be counted as a benefit without measuring the same callback semantics.

Verify on the real Windows benchmark with unchanged colour recurrence, logical dimensions, DPI and presentation settings. Report warm present-to-present p50 and p99, all frame counts, allocations, uploaded bytes, CPU/GPU phases and startup separately. Alternate immutable control/candidate runs on a quiet machine. Preserve visual checks and the existing GPUI result. Headless CPU experiments establish where to work; only the complete native benchmark establishes the target.

## Retained artifacts and validation

Raw measurements, exact executable hashes, the small prototype patch and validation logs are in `docs/scoreboard/pixel-storm-budget/`. The phase harness uses an equivalent fixed-viewport grid; it does not run the actual application's reactive closure or native frame loop. The original project checkout is unchanged. Production-source experiments were restored in the research worktree after measurement; only the profiler, behavioral test and findings are retained.

The small prototype passes all 318 non-ignored Linux UI tests, including the new identity/layout/fill regression. Both immutable Windows phase executables pass that regression and the ignored million-box report. The final Windows colour experiment checks both conversion directions for every value across all nine cache/frame combinations. The final default harness test passes with two reports ignored. Changed-file rustfmt and `git diff --check` pass. Strict Linux clippy is blocked in the unchanged dependency `aqua-reactive/src/read_check.rs:8`: the release `Render(Vec<NodeId>)` payload is unused and `-D warnings` rejects its existing dead-code warning. That dependency is unchanged.
