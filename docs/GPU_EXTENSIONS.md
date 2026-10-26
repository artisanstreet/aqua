# Aqua GPU extensions: media + programmable viewport contract

Owner: media3d agent. Files: `crates/aqua-render/src/media.rs`,
`crates/aqua-render/src/viewport.rs`. Agreed versions: **wgpu 30**,
`image 0.25` (codecs: `png`, `bmp`, `jpeg`, `gif`), all per
`docs/IMPLEMENTATION_CONTRACT.md` §0. The renderer owns composition
(`Renderer::register_external`); this document is the extension-side
contract.

## 1. Minimal API (concrete, as landed)

### Media (`aqua_render::media`)

```rust
// Decode (mature codec, header limits first):
decode_bytes(&bytes) -> Result<DecodedImage, MediaError>
// errors: Empty | UnsupportedFormat | Decode(msg≤256) | DimensionsTooLarge(w,h)
//         | TooManyPixels(n) — header-checked before any pixel allocation
let img: DecodedImage; // straight-alpha sRGB8 RGBA, row-major
img.width(); img.height(); img.pixels(); img.pixel_at(x, y) -> Option<[u8;4]>
img.to_premultiplied_linear_rgba() -> Vec<u8>   // GPU upload (Linear8)
img.to_premultiplied_linear_f16() -> Vec<u8>     // GPU upload (Float16)
img.premultiplied_linear_f32() -> Vec<[f32;4]>   // CPU checks

// CPU cache (bounded decoded bytes, generational ImageId):
let mut cache = MediaCache::new(MediaBudget { max_decoded_bytes, max_images });
let id: ImageId = cache.decode_and_insert(&bytes)?;
let id: ImageId = cache.insert(img)?;
cache.get(id) -> Option<&DecodedImage>; cache.remove(id) -> bool
// eviction is LRU; evicted/removed handles go stale and fail closed.

// Precision + GPU cache (bounded GPU bytes/textures):
ImagePrecision::Linear8   // Rgba8Unorm premultiplied-linear, 4 B/px (default)
ImagePrecision::Float16    // Rgba16Float premultiplied-linear, 8 B/px (no banding)
let mut gpu = GpuImageCache::new(GpuMediaBudget { max_gpu_bytes, max_textures });
let up: &UploadedImage = gpu.upload(&device, &queue, id, &img)?; // Linear8
let up: &UploadedImage = gpu.upload_precise(&device, &queue, id, &img, ImagePrecision::Float16)?;
gpu.view(id) -> Option<&UploadedImage>; gpu.release(id) -> bool
// LRU eviction frees immediately; re-show = re-upload from retained DecodedImage.

// Scene feed (renderer samples the registered view during draw):
// Canonical helpers: aqua_core::scene::{media_layer, viewport_layer,
// LAYER_TAG_MEDIA, LAYER_TAG_VIEWPORT, LAYER_TAG_MASK, LAYER_MAX_GENERATION,
// decode_layer, LayerKind} — re-exported from media/viewport modules.
// Full 32-bit index + 30-bit generation; assert fail-closed above 0x3fffffff.
// Stores retire slots at the bound (MediaCache::kill_slot) so generations
// never wrap in normal use.
let layer: u64 = media_layer(id);   // 0x40.. tag | (generation<<32 | index)
let item: DisplayItem = external_item(layer, bounds, opacity, rounded_clip);
renderer.register_external(layer, view, w, h); // every frame shown
```

### Viewport (`aqua_render::viewport`)

```rust
// Handles + layers (canonical, see media section above):
let id: ViewportId = issue_viewport_id(); // unique, never reused (no ABA)
let layer: u64 = viewport_layer(id);      // 0x80.. tag, disjoint from media

// Import normal form (no loader here; see §5 glTF table):
let mesh = MeshData::from_positions_indices(positions, indices)
    .with_colors(colors).with_normals(n).with_uvs(uvs); // each must match len
mesh.validate()?; mesh.aabb(); mesh.triangle_count();
MeshData::quad(z, color); MeshData::cube(); MeshData::grid(n, y, color);

// Camera (plain arrays, no extra dependency):
Camera::from_matrix(m); Camera::perspective(eye, target, up, fov_y, aspect, near, far);
Camera::ortho_box(l, r, b, t, n, f); // helpers: mat_mul, transform_point, look_at

// App renderer (arbitrary render AND compute in one frame):
trait ViewportRenderer: Send + Sync {
    fn name(&self) -> &str;
    fn render(&mut self, ctx: &mut ViewportFrameCtx) -> Result<(), ViewportError>;
    fn on_resize(&mut self, w, h);  // size-dependent state
    fn on_device_recreated(&mut self, device, queue); // rebuild ALL GPU objects
}
// ctx: { device, queue, encoder, color_view, depth_view, width, height }
// TARGET CONVENTION: color_view holds PREMULTIPLIED linear working-space
// color (MeshRenderer upholds it; custom renderers must premultiply).

// Built-in sample engine (depth-tested; proves the host runs real 3D):
let mut mr = MeshRenderer::new(&device, color_fmt, depth_fmt, &mesh, camera, clear)?;
mr.set_camera_data(cam); mr.set_mesh(&device, &mesh)?; // bounded re-upload
impl ViewportRenderer for MeshRenderer // render() = depth-tested indexed draw

// Compute + readback:
let buf: wgpu::Buffer = run_storage_compute(&device, &queue, wgsl, entry, [x,y,z], init)?;
let bytes: Vec<u8> = read_buffer_sync(&device, &queue, &buf, size)?;
let rgba: Vec<u8> = read_texture_rgba8_sync(&device, &queue, &tex, w, h)?;

// Host pass (owns color+depth targets; canonical scene feed):
let mut pass = ViewportPass::new(device, queue, &ViewportDesc { width, height, scale,
    color_format, depth_format }, Box::new(renderer))?;
pass.set_layout(bounds, opacity, rounded_clip); // logical units
pass.render_offscreen()?;     // must return Ok: errors propagate, never skipped
renderer.register_external(pass.layer(), pass.color_view(), w, h); // every frame
scene.insert(pass.item());    // true painter order: UI above 3D works first frame
pass.resize(w, h, scale)?;               // window/scale change, same epoch
pass.recreate_device(device, queue)?;    // device loss: rebuilds all, epoch+=1
pass.epoch(); pass.frames_rendered(); pass.renderer_name();

// Input + scheduling (data contracts, platform-owned translation):
translate_pointer(window_pos, bounds) -> Option<local>; // None = outside
ViewportInput { focused, capture: CaptureMode::{None,Soft,Hard}, pointer_local, relative_delta }
FrameDemand::{OnDemand, Continuous};
let mut sim = FixedStepper::new(step, max_steps);
let n: u32 = sim.advance(elapsed); // spiral-of-death guard, sim.dropped counts
ViewportCapabilities::probe(&device) // { depth, compute, timestamps, max_texture_dim }
```

## 2. Device/queue sharing boundaries

- The app lends `Device`/`Queue` (cheap `Arc` clones). Viewport/media code
  never creates a second queue and never blocks the queue in production
  paths (`read_buffer_sync` is test/feedback-only; per-frame readback is a
  documented stall).
- Framework-owned textures (`ViewportPass` color/depth, `GpuImageCache`
  textures) are never aliased into app pipelines. Apps sample viewport
  output only through `color_view()` clones registered per frame.
- `poll()` is called only inside blocking test/feedback helpers, never
  during `draw`/`encode`.

## 3. Formats, color, alpha (binding)

| Stage | Format | Alpha |
|---|---|---|
| `DecodedImage` (CPU) | straight sRGB8 RGBA | straight |
| GPU upload, `Linear8` | `Rgba8Unorm` premultiplied-linear | **premultiplied** |
| GPU upload, `Float16` | `Rgba16Float` premultiplied-linear | **premultiplied** |
| `ViewportPass` color target | caller format (default `Rgba8Unorm`) | **premultiplied** |
| Renderer base + compose | `Rgba8Unorm` working space | premultiplied-over |

Why premultiplied: `compose.wgsl` returns
`vec4(rgb * opacity, a * opacity)` under `ONE, ONE_MINUS_SRC_ALPHA`
blending — it never multiplies sampled RGB by sampled alpha. A
straight-alpha upload emits full-bright RGB at partial alpha (halos on
every translucent texel). Pinned by
`gpu_half_alpha_over_blue_composes_linear` (half red over blue reads
`[~128, 0, ~128, 255]`, i.e. linear `[0.5, 0, 0.5, 1]`) and
`gpu_transparent_texel_contributes_no_rgb` (transparent texels add nothing).

Precision: 8-bit linear steps band on dark gradients (documented, accepted
default). `Float16` preserves sRGB precision through the linear space at 2x
bytes, counted honestly in `GpuMediaBudget`. `Rgba8UnormSrgb` upload was
considered and rejected: hardware sRGB decode of premultiplied-encoded
values is nonlinear (`decode(p*a) != decode(p)*a`), so it corrupts
translucency — the correct high-precision path is linear Float16.

## 4. Lifetime rules

- `MediaCache`: LRU over all entries (decoded bytes are reproducible);
  eviction/ removal bumps generations (stale fails closed).
- `GpuImageCache`: LRU frees textures immediately; `resident_bytes` counts
  padded upload bytes; single texture over budget is refused, never
  half-kept.
- `ViewportPass`: `resize` recreates targets (same device/epoch, renderer
  notified); `recreate_device` rebuilds pipelines + targets (epoch+1,
  renderer rebuilds everything in `on_device_recreated`); drop frees.
- `register_external` views must stay alive through `draw`; re-register
  every frame shown (the renderer only samples).

## 5. Import paths

Still images: `image 0.25` with `png`, `bmp`, `jpeg`, `gif` features.
`webp`/`tiff`/`qoi`/`exr` stay disabled until a consumer needs them (edit
`crates/aqua-render/Cargo.toml` + this doc). BMP note: 32-bit `BI_RGB`
carries no alpha channel (4th byte is padding, decoded opaque per spec);
real BMP alpha needs `BITFIELDS` masks, out of the enabled set. Limits are header-first:
`MAX_IMAGE_DIM` 8192/side, `MAX_IMAGE_PIXELS` 64 MP — hostile headers fail
without a large allocation; malformed bodies fail as `Decode`, never panic.

Meshes: construct `MeshData` from buffers. The glTF 2.0 mapping the future
`gltf`-crate loader implements:

| glTF 2.0 source | `MeshData` field |
|---|---|
| `POSITION` accessor (`VEC3` float) | `positions` (required) |
| `NORMAL` (`VEC3` float) | `normals` |
| `TEXCOORD_0` (`VEC2` float) | `uvs` |
| `COLOR_0` (`VEC3`/`VEC4` float/ubyte) | `colors` (to linear straight-alpha float) |
| `indices` (`SCALAR` u16/u32) | `indices` (u16 widened) |
| interleaved `bufferView` stride | de-interleave on import |
| skins, morph targets, sparse accessors | rejected as `InvalidMesh` |

Fixture `crates/aqua/examples/assets/sample_cube.gltf` decodes to exactly
`MeshData::cube()` (8 corners, 36 indices); it documents the mapping until
the loader lands. There is deliberately no partial loader: an API that
promises models but only uploads shader snippets would lie.

## 6. Unsupported (explicit, never silent)

- `MediaError::UnsupportedFormat`: webp/tiff/qoi/exr/avif (enable per §5),
  animated GIF (first frame only via `image`; playback is deferred work).
- `ViewportError::Unsupported`: MSAA targets (`samples != 1`).
- Fullscreen post-processing belongs in a separate explicit pass, never in
  the primary viewport primitive (no blanket `CustomPass` impl: it would run
  after built-ins, ignore ancestor clips, and paint over the top layer).
- `CaptureMode::Hard` where the platform cannot confine/hide: the platform
  adapter reports `Unsupported`; the viewport keeps working uncaptured.
- `read_buffer_sync` per-frame in production (documented stall; use
  fixed-step readback deadlines instead).
- No scene graph, physics, audio, or asset manager lives here: the
  viewport is an engine-*hosting* boundary. Accessibility/IME inside custom
  viewports needs explicit semantic integration from the app/renderer.

## 7. Test contract (binding for these modules' GPU tests)

Only a genuine `wgpu::RequestAdapterError::NotFound` skips a GPU test. Any
other adapter error, any device/request failure, any pipeline/validation
error FAILS — never labeled "no adapter". `AQUA_REQUIRE_GPU=1` turns a
skip into a panic (CI runs strict against lavapipe; use `--nocapture` to
verify every GPU test actually ran). This replaces the flagged `Err(any)
=> None` helper pattern.
