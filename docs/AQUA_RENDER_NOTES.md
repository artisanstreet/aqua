# aqua-render: interface notes for core / text / media3d / platform / facade

Render agent cannot `agent_message` peers directly from its thread
(`has no agent` errors); parent, please relay. All paths absolute under the
shared workspace `crates/aqua-render/`.

STATUS 2026-10-25: implemented, `cargo check -p aqua-render --all-targets`
clean, clippy 0 warnings, **23 unit + 5 headless GPU tests pass** on
lavapipe (llvmpipe LLVM 20.1.2 Vulkan). Core-vocabulary migration DONE
(re-export, duplicates deleted). text.rs ownership TRANSFERRED to text
agent — render no longer edits it.

STATUS checkpoint 2: partial raster + fast path + colorspace landed.
Mirror-verified (shared blocked on 3 peer-owned errors, see report):
28 lib unit tests + 12/12 headless GPU tests pass. Local-change cost
flat across scene sizes (lavapipe, debug build): n=10/1000/10000 →
prepare 17/12/16us, upload 128B, raster 392px. 10k mount in
milliseconds (bounded damage, no coalescer stall).

## 1. Agreed versions (proposed, no objections heard yet)

| crate | version | basis |
|---|---|---|
| `wgpu` | `30.0.x` (Cargo resolves 30.0.1) | latest stable; all backends incl. Vulkan lavapipe headless |
| `glyphon` | `0.12.0` | depends on `cosmic-text 0.19` + `wgpu 30.0.0` per crates.io |
| `cosmic-text` | `0.19.0` | pinned by glyphon 0.12 |
| `bytemuck` | `1` (+ `derive`) | instance-buffer casts |

Text/media agents: if you need different versions, post here; renderer
adopts whatever the three of us agree. Do NOT introduce a second text
rasterizer version in another crate.

## 2. Canonical types must move to aqua-core (parent review)

`src/color.rs::AuthoredColor{l,c,h_deg,alpha}`,
`src/scene.rs::NodeId{index,generation}`, `Rect{x,y,w,h}`,
`DisplayItem`, `TextRun{id,revision}` are **provisional render-local
proposals**. Core owner: please adopt these exact layouts (or publish
alternatives) as canonical `aqua-core` types. Renderer will then re-export
them with `From` adapters and keep only GPU-internal linear-light math.
No permanent alternate color/geometry/scene vocabulary will remain.

Hex-parser hazard (fixed here, core parser needs it too): `from_hex` must
reject non-ASCII **before** any byte slicing (`#ééé` with len 6 panics on
`&h[i..i+2]`). Regression tests added.

## 3. Renderer public API (notify parent early — this is it)

```rust
Renderer::headless(width, height) -> Result<Renderer, RenderError>
// software Vulkan adapter, offscreen texture target; for tests.

Renderer::new(device, queue, format) -> Renderer
renderer.set_text_provider(Box<dyn TextProvider>) / .with_text_provider(..)
// facade wiring for the shared text backend (text-agent owned trait).
renderer.set_size(w, h, scale)
renderer.prepare(&mut Scene, &DamageTracker-output) -> FramePlan
renderer.draw(&FramePlan, &Scene, &TextureView)
renderer.draw_offscreen(&FramePlan, &Scene) -> Texture  // readback path
renderer.register_external(layer: u64, view: TextureView, size)  // media/viewport feed frames
renderer.add_custom_pass(Box<dyn CustomPass>)
renderer.diagnostics() -> &RenderDiagnostics
renderer.present(frame: wgpu::SurfaceTexture)  // wgpu 30 presents via queue
SurfaceFacade::{with_adapter(device, adapter, surface, w, h, prefer_format,
  prefer_present_mode), resize, acquire_and_draw, request_present}
FramePlan::{is_clean, damage, nodes_total, nodes_updated}
```

Submit-boundary rule (text-encoder hazard, verified by readback): the text
provider submits its own encoder, so `Texts` and `Group` ops are submit
boundaries — main encoder finishes+submits, text flushes, fresh encoder
continues. Painter order via submission order. When text lands
same-encoder rendering (`render_areas(&mut encoder)`), boundaries collapse
to inline passes; call sites are `draw()` Texts arm + `draw_group_isolated`.
Blit/group-scratch buffers are fresh per call, never shared-mid-encoder.

## 4. Extension contracts (media.rs / viewport.rs owners)

- Do NOT edit your files; implement against these traits in `src/lib.rs`:
  - `register_external(view)` for per-frame texture feed (image/video/3D),
    referenced by `DisplayItem::External.layer`. ON DEMAND ONLY: every
    call dirties the layer bounds for the next prepare even with an
    unchanged scene (content swaps without it never composite). Call
    `unregister_external(layer)` on removal; additionally the structural
    walk prunes layers unreferenced by the committed scene (views/dims/
    bounds/pending-dirty), so removed layers free without facade calls —
    observe `diagnostics.external_layers_live`. A layer registered with
    no scene reference in the same commit cycle is dropped (re-register).
  - `CustomPass::encode(..)` returns `Result<(), RenderError>` (failures
    abort explicitly, base keeps last good pixels). Only NEWLY QUEUED
    passes invalidate one frame; persistence never forces idle frames —
    the renderer owns no window clock, callers request frames on demand.
  - ALPHA CONTRACT (measured, binding): external layers arrive
    PREMULTIPLIED-linear. The compose path does NOT premultiply them
    (`premultiplied` flag = 1); straight input through it renders too
    bright at half alpha, double-premultiply too dark — both caught
    numerically (`gpu_half_alpha_over_blue_composes_linear`,
    `external_premultiplied_composites_exact`). A straight-alpha source
    must normalize on upload (media does); no silent conversion exists.
  - Compose uniform ABI (you duplicate it — keep in sync): 8 floats / 32B
    (viewport@0, opacity@8, clip_radius@12, output_gamma@16,
    premultiplied@20, pad@24). 28B buffers fail validation against the
    shader; pinned by `renderer::tests::compose_uniform_abi_32b`.
  - Gamma rule (measured on Vulkan lavapipe): hardware encodes on sRGB
    render-target writes, so Srgb blits use gamma 0 and Unorm blits
    gamma 1. Manual-on-hardware = double gamma, caught numerically by
    `gamma_single_encode_midgray`. Metal/DX12 assumed same, unmeasured.
  - `CustomPass::encode(..)` now returns `Result<(), RenderError>`:
    failures abort the frame explicitly (nothing submitted, base keeps
    last good pixels). Update impls (one-line + Ok). Do NOT render
    viewports as blanket last-stage passes over the UI: compose through
    `External` layers in painter order with layout clip.
  - `TextProvider` trait (glyphon default in `src/text.rs`); text agent may
    replace shaping, renderer keeps raster cache.
- Unsupported effects are explicit (`EffectCapabilities::unsupported()`),
  never silent no-ops.

## 5. Performance honesty

No >Chrome claims. Diagnostics expose bytes uploaded, passes, pool stats,
nodes updated vs total. Partial-update contract tested
(`trivial_update_yields_single_change`, dirty-queue cost).
