//! Programmable 3D viewport (`Viewport3D`): render-to-texture host, custom
//! render + compute execution, resize/device lifetime, input capture, and
//! fixed-step simulation.
//!
//! Ownership: this module (media3d agent) owns the viewport extension. It
//! does NOT own GPU composition ([`crate::Renderer`]), the scene vocabulary
//! (`DisplayItem::External`, owned by `aqua-core`), or windowing/input
//! (owned by `aqua-platform`). Integration uses only public renderer
//! contracts: `register_external` against [`crate::Renderer`].
//!
//! What this module promises (and nothing more):
//! - A wgpu device/queue sharing boundary: the host app lends its
//!   `Device`/`Queue` (cheap `Arc` clones); framework-owned textures are
//!   never aliased into app pipelines and vice versa. See
//!   `docs/GPU_EXTENSIONS.md` for the exact rules.
//! - Arbitrary custom rendering AND compute: [`ViewportRenderer`] receives a
//!   live encoder plus owned color/depth targets; [`run_storage_compute`] and
//!   [`read_buffer_sync`] run arbitrary WGSL compute with CPU readback.
//! - A built-in [`MeshRenderer`] (depth-tested, indexed triangles, camera
//!   uniforms) and [`MeshData`] import buffers with sample builders. This is
//!   an engine-*hosting* boundary, not a game engine: no scene graph,
//!   physics, audio, or asset manager lives here.
//!
//! Mesh import details (positions/normals/UVs/colors + indices), the glTF
//! mapping table, and the `sample_cube.gltf` fixture live in
//! `docs/GPU_EXTENSIONS.md`; no fake loader is provided.
//! - Explicit lifetime: [`ViewportPass::resize`] (window/scale change) and
//!   [`ViewportPass::recreate_device`] (device loss) with epoch counting and
//!   renderer notifications. Input capture and scheduling are explicit data
//!   contracts ([`ViewportInput`], [`FrameDemand`], [`FixedStepper`]).
//!
//! Canonical integration (binding): [`ViewportPass::render_offscreen`]
//! → `register_external(pass.layer(), pass.color_view())` → scene
//! [`ViewportPass::item`] in true painter order, so UI overlays above 3D
//! compose correctly on the very first frame.
//!
//! A blanket [`crate::effects::CustomPass`] impl is deliberately NOT
//! provided: custom passes run after all built-in ops and cannot honor
//! ancestor clips or the top layer, so a viewport blitting there would
//! paint over UI. Fullscreen post-processing belongs in a separate
//! explicit pass, never in the primary viewport primitive.

use std::sync::{
    atomic::{AtomicU32, Ordering},
    Mutex,
};

use aqua_core::geometry::Rect;
use aqua_core::id::{RawId, ViewportId};
use aqua_core::scene::DisplayItem;

// ---------------------------------------------------------------------------
// Errors and capabilities
// ---------------------------------------------------------------------------

/// Explicit viewport failures. Unsupported configurations are typed errors,
/// never silent no-ops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewportError {
    /// Zero or absurd size (`0`, or a side above 16384).
    BadSize(u32, u32),
    /// Non-positive scale factor.
    BadScale,
    /// Only single-sample rendering is implemented.
    Unsupported(&'static str),
    /// Mesh/buffer exceeds the import byte cap or fails validation.
    InvalidMesh(String),
    /// wgpu failure (device loss, OOM, validation).
    Gpu(String),
}

impl std::fmt::Display for ViewportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ViewportError::BadSize(w, h) => write!(f, "viewport: bad size {w}x{h}"),
            ViewportError::BadScale => write!(f, "viewport: non-positive scale"),
            ViewportError::Unsupported(m) => write!(f, "viewport: unsupported: {m}"),
            ViewportError::InvalidMesh(m) => write!(f, "viewport: invalid mesh: {m}"),
            ViewportError::Gpu(m) => write!(f, "viewport: gpu error: {m}"),
        }
    }
}

impl std::error::Error for ViewportError {}

/// Honestly probed device capabilities. Anything `false` here means the app
/// must take a fallback path; the viewport never emulates it silently.
#[derive(Clone, Debug)]
pub struct ViewportCapabilities {
    /// Depth24Plus render targets usable.
    pub depth: bool,
    /// Compute shaders usable (required by wgpu; reported for the ledger).
    pub compute: bool,
    /// Timestamp queries usable.
    pub timestamps: bool,
    /// Maximum 2D texture dimension.
    pub max_texture_dim: u32,
}

impl ViewportCapabilities {
    /// Probe real device features/limits. `depth` is conservatively true:
    /// `Depth24Plus` is universally supported on wgpu 30 backends, and every
    /// viewport creation verifies its depth texture on demand.
    pub fn probe(device: &wgpu::Device) -> Self {
        let features = device.features();
        Self {
            depth: true,
            compute: true,
            timestamps: features.contains(wgpu::Features::TIMESTAMP_QUERY),
            max_texture_dim: device.limits().max_texture_dimension_2d,
        }
    }
}

// ---------------------------------------------------------------------------
// Descriptor, IDs, layer namespace
// ---------------------------------------------------------------------------

/// Viewport creation parameters. Sizes are physical pixels.
#[derive(Clone, Copy, Debug)]
pub struct ViewportDesc {
    /// Width in physical pixels (>= 1).
    pub width: u32,
    /// Height in physical pixels (>= 1).
    pub height: u32,
    /// UI scale (logical px * scale = physical px). Must be positive.
    pub scale: f32,
    /// Color target format. Defaults to the renderer working format.
    pub color_format: wgpu::TextureFormat,
    /// Depth target format.
    pub depth_format: wgpu::TextureFormat,
}

impl Default for ViewportDesc {
    fn default() -> Self {
        Self {
            width: 256,
            height: 256,
            scale: 1.0,
            color_format: wgpu::TextureFormat::Rgba8Unorm,
            depth_format: wgpu::TextureFormat::Depth24Plus,
        }
    }
}

impl ViewportDesc {
    /// Validate without touching the GPU.
    pub fn validate(&self) -> Result<(), ViewportError> {
        if self.width == 0 || self.height == 0 || self.width > 16384 || self.height > 16384 {
            return Err(ViewportError::BadSize(self.width, self.height));
        }
        if !self.scale.is_finite() || self.scale <= 0.0 {
            return Err(ViewportError::BadScale);
        }
        Ok(())
    }
}

static NEXT_VIEWPORT_INDEX: AtomicU32 = AtomicU32::new(0);

/// Mint a fresh [`ViewportId`]. Indices are never reused, so ABA aliasing is
/// impossible by construction; generations stay 0 and stale checks compare
/// the full handle.
pub fn issue_viewport_id() -> ViewportId {
    let index = NEXT_VIEWPORT_INDEX.fetch_add(1, Ordering::Relaxed);
    ViewportId::from_raw(RawId {
        index,
        generation: 0,
    })
}

/// Viewport layer namespace: canonical helpers live in `aqua_core::scene`
/// and are re-exported here so `aqua_render::viewport::…` paths keep
/// working. Full 32-bit indices + 30-bit generations, fail-closed beyond
/// [`LAYER_MAX_GENERATION`] (see [`crate::media`] re-exports).
pub use aqua_core::scene::{
    decode_layer, viewport_layer, LayerKind, LAYER_MAX_GENERATION, LAYER_TAG_MASK,
    LAYER_TAG_VIEWPORT,
};

// ---------------------------------------------------------------------------
// Mesh data: import buffers + sample builders (no fake loader)
// ---------------------------------------------------------------------------

/// Upper bound for one imported mesh (64 MiB of vertex/index bytes).
pub const MAX_MESH_BYTES: usize = 64 * 1024 * 1024;

/// CPU-side triangle mesh: the import normal form. Optional streams let
/// color-only, textured, or lit pipelines each take what they need; the
/// built-in [`MeshRenderer`] uses positions + colors (+ indices).
///
/// | glTF 2.0 source | `MeshData` field | Notes |
/// |---|---|---|
/// | `POSITION` accessor (`VEC3` float) | `positions` | required |
/// | `NORMAL` accessor (`VEC3` float) | `normals` | optional |
/// | `TEXCOORD_0` (`VEC2` float) | `uvs` | optional;NPOT allowed |
/// | `COLOR_0` (`VEC3`/`VEC4` float/ubyte) | `colors` | optional; convert to linear float, straight alpha |
/// | `indices` accessor (`SCALAR` u16/u32) | `indices` | u16 widened to u32 |
/// | interleaved `bufferView` stride | de-interleave on import | keep `byteStride` handling in the loader |
/// | skins, morph targets, sparse accessors | rejected | explicit `InvalidMesh`, never half-imported |
///
/// The future `gltf`-crate loader maps exactly this table; until it lands,
/// construct `MeshData` from generated or hand-built buffers (see builders
/// and `sample_cube.gltf`, whose decoded contents equal `cube()`).
#[derive(Clone, Debug, Default)]
pub struct MeshData {
    /// Vertex positions (model space). Required, non-empty.
    pub positions: Vec<[f32; 3]>,
    /// Per-vertex normals. `None` = unlit pipeline only.
    pub normals: Option<Vec<[f32; 3]>>,
    /// Per-vertex UVs.
    pub uvs: Option<Vec<[f32; 2]>>,
    /// Per-vertex straight-alpha linear colors.
    pub colors: Option<Vec<[f32; 4]>>,
    /// Triangles. Empty = non-indexed (sequential triples).
    pub indices: Vec<u32>,
}

impl MeshData {
    /// Positions + indices; everything else absent.
    pub fn from_positions_indices(positions: Vec<[f32; 3]>, indices: Vec<u32>) -> Self {
        Self {
            positions,
            indices,
            ..Default::default()
        }
    }

    /// Attach colors (must match positions length).
    pub fn with_colors(mut self, colors: Vec<[f32; 4]>) -> Self {
        self.colors = Some(colors);
        self
    }

    /// Attach normals (must match positions length).
    pub fn with_normals(mut self, normals: Vec<[f32; 3]>) -> Self {
        self.normals = Some(normals);
        self
    }

    /// Attach UVs (must match positions length).
    pub fn with_uvs(mut self, uvs: Vec<[f32; 2]>) -> Self {
        self.uvs = Some(uvs);
        self
    }

    /// Validate stream lengths, index range, finiteness, and byte budget.
    pub fn validate(&self) -> Result<(), ViewportError> {
        if self.positions.is_empty() {
            return Err(ViewportError::InvalidMesh("no positions".into()));
        }
        let n = self.positions.len();
        if let Some(v) = &self.normals {
            if v.len() != n {
                return Err(ViewportError::InvalidMesh("normals length".into()));
            }
        }
        if let Some(v) = &self.uvs {
            if v.len() != n {
                return Err(ViewportError::InvalidMesh("uv length".into()));
            }
        }
        if let Some(v) = &self.colors {
            if v.len() != n {
                return Err(ViewportError::InvalidMesh("colors length".into()));
            }
        }
        for i in &self.indices {
            if (*i as usize) >= n {
                return Err(ViewportError::InvalidMesh("index out of range".into()));
            }
        }
        for p in &self.positions {
            if !p.iter().all(|c| c.is_finite()) {
                return Err(ViewportError::InvalidMesh("non-finite position".into()));
            }
        }
        if self.byte_len() > MAX_MESH_BYTES {
            return Err(ViewportError::InvalidMesh("mesh exceeds byte cap".into()));
        }
        Ok(())
    }

    /// Approximate CPU byte size (positions + present streams + indices).
    pub fn byte_len(&self) -> usize {
        let mut bytes = self.positions.len() * 12 + self.indices.len() * 4;
        if let Some(v) = &self.normals {
            bytes += v.len() * 12;
        }
        if let Some(v) = &self.uvs {
            bytes += v.len() * 8;
        }
        if let Some(v) = &self.colors {
            bytes += v.len() * 16;
        }
        bytes
    }

    /// Triangle count (indexed or sequential).
    pub fn triangle_count(&self) -> usize {
        if self.indices.is_empty() {
            self.positions.len() / 3
        } else {
            self.indices.len() / 3
        }
    }

    /// Axis-aligned bounding box `(min, max)`, or `None` when empty/invalid.
    pub fn aabb(&self) -> Option<([f32; 3], [f32; 3])> {
        if self.positions.is_empty() {
            return None;
        }
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for p in &self.positions {
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
        if min.iter().any(|c| !c.is_finite()) {
            return None;
        }
        Some((min, max))
    }

    /// Unit quad in the XY plane at depth `z`, spanning `[-1, 1]^2`.
    pub fn quad(z: f32, color: [f32; 4]) -> Self {
        Self {
            positions: vec![
                [-1.0, -1.0, z],
                [1.0, -1.0, z],
                [1.0, 1.0, z],
                [-1.0, 1.0, z],
            ],
            colors: Some(vec![color; 4]),
            indices: vec![0, 1, 2, 0, 2, 3],
            ..Default::default()
        }
    }

    /// Unit cube spanning `[-1, 1]^3`, 8 vertices, 12 triangles, per-vertex
    /// colors mapped from position. Matches `sample_cube.gltf` contents.
    pub fn cube() -> Self {
        let positions = vec![
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
        ];
        let colors: Vec<[f32; 4]> = positions
            .iter()
            .map(|p| {
                [
                    (p[0] + 1.0) * 0.5,
                    (p[1] + 1.0) * 0.5,
                    (p[2] + 1.0) * 0.5,
                    1.0,
                ]
            })
            .collect();
        Self {
            positions,
            colors: Some(colors),
            indices: vec![
                0, 1, 2, 0, 2, 3, // -z
                4, 6, 5, 4, 7, 6, // +z
                4, 5, 1, 4, 1, 0, // -y
                3, 2, 6, 3, 6, 7, // +y
                1, 5, 6, 1, 6, 2, // +x
                4, 0, 3, 4, 3, 7, // -x
            ],
            ..Default::default()
        }
    }

    /// XZ-plane grid of `n x n` cells at height `y`, for ground-plane tests.
    pub fn grid(n: u32, y: f32, color: [f32; 4]) -> Self {
        let n = n.max(1);
        let mut positions = Vec::new();
        let mut colors = Vec::new();
        let mut indices = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                positions.push([
                    -1.0 + 2.0 * i as f32 / n as f32,
                    y,
                    -1.0 + 2.0 * j as f32 / n as f32,
                ]);
                colors.push(color);
            }
        }
        let row = n + 1;
        for j in 0..n {
            for i in 0..n {
                let a = j * row + i;
                indices.extend([a, a + 1, a + row + 1, a, a + row + 1, a + row]);
            }
        }
        Self {
            positions,
            colors: Some(colors),
            indices,
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Camera math (plain arrays, no extra dependency)
// ---------------------------------------------------------------------------

/// 4x4 row-major matrix, column-vector convention (`out = M * v`).
pub type Mat4 = [[f32; 4]; 4];

/// Identity matrix.
pub fn mat_identity() -> Mat4 {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Matrix product `a * b` (apply `b` first).
pub fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0; 4]; 4];
    for r in 0..4 {
        for c in 0..4 {
            out[r][c] =
                a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c] + a[r][3] * b[3][c];
        }
    }
    out
}

/// Transform a point with perspective divide.
pub fn transform_point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    let v = [p[0], p[1], p[2], 1.0];
    let mut o = [0.0; 4];
    for r in 0..4 {
        o[r] = m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2] + m[r][3] * v[3];
    }
    let w = if o[3].abs() < 1e-9 { 1e-9 } else { o[3] };
    [o[0] / w, o[1] / w, o[2] / w]
}

/// Right-handed perspective for wgpu depth `0..1` (Vulkan-style: `w' = -z`,
/// visible `z_view` in `[-far, -near]` maps to NDC `0..1`).
pub fn perspective_rh(fov_y_rad: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fov_y_rad * 0.5).tan();
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, far / (near - far), near * far / (near - far)],
        [0.0, 0.0, -1.0, 0.0],
    ]
}

/// Orthographic volume for wgpu depth `0..1`.
pub fn orthographic(l: f32, r: f32, b: f32, t: f32, n: f32, f: f32) -> Mat4 {
    [
        [2.0 / (r - l), 0.0, 0.0, -(r + l) / (r - l)],
        [0.0, 2.0 / (t - b), 0.0, -(t + b) / (t - b)],
        [0.0, 0.0, 1.0 / (f - n), -n / (f - n)],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn norm3(v: [f32; 3]) -> [f32; 3] {
    let l = dot3(v, v).sqrt().max(1e-9);
    [v[0] / l, v[1] / l, v[2] / l]
}

/// Right-handed look-at view matrix.
pub fn look_at(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> Mat4 {
    let z = norm3(sub3(eye, target));
    let x = norm3(cross3(up, z));
    let y = cross3(z, x);
    [
        [x[0], x[1], x[2], -dot3(x, eye)],
        [y[0], y[1], y[2], -dot3(y, eye)],
        [z[0], z[1], z[2], -dot3(z, eye)],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Camera: precomputed view-projection matrix plus its parameters.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    /// Combined projection * view matrix.
    pub view_proj: Mat4,
}

impl Camera {
    /// Direct matrix (identity = NDC passthrough; used by depth tests).
    pub fn from_matrix(view_proj: Mat4) -> Self {
        Self { view_proj }
    }

    /// Perspective camera looking from `eye` at `target`.
    pub fn perspective(
        eye: [f32; 3],
        target: [f32; 3],
        up: [f32; 3],
        fov_y_rad: f32,
        aspect: f32,
        near: f32,
        far: f32,
    ) -> Self {
        let vp = mat_mul(
            &perspective_rh(fov_y_rad, aspect, near, far),
            &look_at(eye, target, up),
        );
        Self { view_proj: vp }
    }

    /// Orthographic camera over the given model-space box.
    pub fn ortho_box(l: f32, r: f32, b: f32, t: f32, n: f32, f: f32) -> Self {
        Self {
            view_proj: orthographic(l, r, b, t, n, f),
        }
    }
}

// ---------------------------------------------------------------------------
// Renderer trait + frame context
// ---------------------------------------------------------------------------

/// Per-frame render context handed to app renderers. All views are
/// viewport-owned and valid for this call only; the encoder is the pass's
/// own (single canonical offscreen path).
///
/// Target convention (binding): `color_view` holds PREMULTIPLIED linear
/// working-space color (same as the 2D base). [`MeshRenderer`] upholds it;
/// custom [`ViewportRenderer`]s must premultiply their own output. The
/// scene composite samples it premultiplied-over, so straight-alpha
/// output would fringe on translucency.
pub struct ViewportFrameCtx<'a> {
    /// Shared device (cloned handle; do not create competing queues).
    pub device: &'a wgpu::Device,
    /// Shared queue for uploads and timestamp writes.
    pub queue: &'a wgpu::Queue,
    /// Encoder recording this frame's work.
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// Owned color target (cleared by the pass before `render`).
    pub color_view: &'a wgpu::TextureView,
    /// Owned depth target (cleared to 1.0 before `render`).
    pub depth_view: &'a wgpu::TextureView,
    /// Target size in physical pixels.
    pub width: u32,
    /// Target size in physical pixels.
    pub height: u32,
}

/// Application-provided renderer: arbitrary passes AND compute within one
/// frame. The framework owns scheduling, target lifetime, and composition;
/// the app owns draw calls, pipelines, and persistent GPU objects (which it
/// must rebuild in `on_device_recreated`).
pub trait ViewportRenderer: Send + Sync {
    /// Stable name for diagnostics and the inspector.
    fn name(&self) -> &str;
    /// Record render and/or compute work into `ctx`.
    fn render(&mut self, ctx: &mut ViewportFrameCtx) -> Result<(), ViewportError>;
    /// Viewport pixel size changed; re-derive size-dependent state.
    fn on_resize(&mut self, _width: u32, _height: u32) {}
    /// Device/queue were replaced after loss; rebuild ALL GPU objects.
    fn on_device_recreated(&mut self, _device: &wgpu::Device, _queue: &wgpu::Queue) {}
}

// ---------------------------------------------------------------------------
// Built-in mesh renderer (depth-tested sample engine)
// ---------------------------------------------------------------------------

/// Interleaved vertex: position + straight-alpha linear color (28 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct MeshVertex {
    pos: [f32; 3],
    color: [f32; 4],
}

const MESH_WGSL: &str = r#"
struct Cam { vp: mat4x4<f32> };
@group(0) @binding(0) var<uniform> cam: Cam;
struct In { @location(0) pos: vec3<f32>, @location(1) color: vec4<f32> };
struct Out { @builtin(position) p: vec4<f32>, @location(0) c: vec4<f32> };
@vertex fn vs(i: In) -> Out {
    var o: Out;
    o.p = cam.vp * vec4<f32>(i.pos, 1.0);
    o.c = i.color;
    return o;
}
@fragment fn fs(i: Out) -> @location(0) vec4<f32> {
    // Straight-alpha vertex colors -> premultiplied working space.
    return vec4<f32>(i.c.rgb * i.c.a, i.c.a);
}
"#;

/// Depth-tested indexed-triangle renderer over [`MeshData`].
///
/// This is the sample engine proving the viewport hosts real 3D: apps replace
/// it with their own [`ViewportRenderer`] through the same public contract.
pub struct MeshRenderer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    camera_buf: wgpu::Buffer,
    bind: wgpu::BindGroup,
    vertex_buf: wgpu::Buffer,
    index_buf: Option<wgpu::Buffer>,
    index_count: u32,
    vertex_count: u32,
    mesh: MeshData,
    camera: Camera,
    clear: [f32; 4],
    color_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
}

impl MeshRenderer {
    /// Build pipelines and upload `mesh`. Rejects invalid/over-budget meshes.
    pub fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        mesh: &MeshData,
        camera: Camera,
        clear: [f32; 4],
    ) -> Result<Self, ViewportError> {
        mesh.validate()?;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("aqua-mesh"),
            source: wgpu::ShaderSource::Wgsl(MESH_WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("aqua-mesh-bind"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("aqua-mesh-layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("aqua-mesh-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<MeshVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
                })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let mut r = Self {
            pipeline,
            layout,
            camera_buf: Self::camera_buffer(device),
            bind: Self::dummy_bind(device),
            vertex_buf: Self::empty_buffer(device, "aqua-mesh-vb"),
            index_buf: None,
            index_count: 0,
            vertex_count: 0,
            mesh: MeshData::default(),
            camera,
            clear,
            color_format,
            depth_format,
        };
        r.bind = Self::make_bind(device, &r.layout, &r.camera_buf);
        r.set_mesh(device, mesh)?;
        r.set_camera_data(camera);
        Ok(r)
    }

    /// Replace the mesh (bounded re-upload).
    pub fn set_mesh(
        &mut self,
        device: &wgpu::Device,
        mesh: &MeshData,
    ) -> Result<(), ViewportError> {
        mesh.validate()?;
        let verts: Vec<MeshVertex> = mesh
            .positions
            .iter()
            .enumerate()
            .map(|(i, p)| MeshVertex {
                pos: *p,
                color: mesh.colors.as_ref().map(|c| c[i]).unwrap_or([1.0; 4]),
            })
            .collect();
        // Mapped-at-creation upload: no queue needed, valid on any device.
        let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aqua-mesh-vb"),
            size: (verts.len() * std::mem::size_of::<MeshVertex>()).max(16) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: true,
        });
        {
            let bytes_len = verts.len() * std::mem::size_of::<MeshVertex>();
            let mut view = vertex_buf
                .slice(..)
                .get_mapped_range_mut()
                .map_err(|e| ViewportError::Gpu(format!("map mesh: {e:?}")))?;
            view.slice(..bytes_len)
                .copy_from_slice(bytemuck::cast_slice(&verts));
        }
        vertex_buf.unmap();
        self.vertex_buf = vertex_buf;
        self.vertex_count = verts.len() as u32;
        self.index_buf = if mesh.indices.is_empty() {
            self.index_count = 0;
            None
        } else {
            let ib = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("aqua-mesh-ib"),
                size: (mesh.indices.len() * 4).max(16) as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: true,
            });
            {
                let bytes_len = mesh.indices.len() * 4;
                let mut view = ib
                    .slice(..)
                    .get_mapped_range_mut()
                    .map_err(|e| ViewportError::Gpu(format!("map mesh indices: {e:?}")))?;
                view.slice(..bytes_len)
                    .copy_from_slice(bytemuck::cast_slice(&mesh.indices));
            }
            ib.unmap();
            self.index_count = mesh.indices.len() as u32;
            Some(ib)
        };
        self.mesh = mesh.clone();
        Ok(())
    }

    /// Update the camera uniform (CPU-side copy retained for rebuilds).
    pub fn set_camera_data(&mut self, camera: Camera) {
        self.camera = camera;
        // Upload happens in render_into via the queue (always current).
    }

    /// Rebuild pipelines and buffers on the new device after loss.
    pub fn rebuild(&mut self, device: &wgpu::Device) -> Result<(), ViewportError> {
        let mesh = std::mem::take(&mut self.mesh);
        let camera = self.camera;
        let clear = self.clear;
        let (cf, df) = (self.color_format, self.depth_format);
        *self = Self::new(device, cf, df, &mesh, camera, clear)?;
        Ok(())
    }

    /// Draw into the context's targets (depth-tested, premultiplied out).
    pub fn render_into(&mut self, ctx: &mut ViewportFrameCtx) -> Result<(), ViewportError> {
        ctx.queue.write_buffer(
            &self.camera_buf,
            0,
            bytemuck::cast_slice(&[self.camera.view_proj]),
        );
        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("aqua-mesh-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: ctx.color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: self.clear[0] as f64,
                        g: self.clear[1] as f64,
                        b: self.clear[2] as f64,
                        a: self.clear[3] as f64,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: ctx.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        if let Some(ib) = &self.index_buf {
            pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.index_count, 0, 0..1);
        } else {
            pass.draw(0..self.vertex_count, 0..1);
        }
        Ok(())
    }

    fn camera_buffer(device: &wgpu::Device) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aqua-mesh-cam"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn make_bind(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        camera_buf: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("aqua-mesh-bind-group"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buf.as_entire_binding(),
            }],
        })
    }

    fn dummy_bind(device: &wgpu::Device) -> wgpu::BindGroup {
        let buf = Self::camera_buffer(device);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("aqua-mesh-dummy"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        Self::make_bind(device, &layout, &buf)
    }

    fn empty_buffer(device: &wgpu::Device, label: &'static str) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: 16,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
}

impl ViewportRenderer for MeshRenderer {
    fn name(&self) -> &str {
        "aqua-mesh"
    }

    fn render(&mut self, ctx: &mut ViewportFrameCtx) -> Result<(), ViewportError> {
        self.render_into(ctx)
    }

    fn on_device_recreated(&mut self, device: &wgpu::Device, _queue: &wgpu::Queue) {
        if let Err(e) = self.rebuild(device) {
            eprintln!("MeshRenderer rebuild failed: {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// Compute: arbitrary WGSL compute + CPU readback
// ---------------------------------------------------------------------------

/// Run an arbitrary WGSL compute shader over a caller-sized storage buffer.
///
/// `wgsl` must declare `@group(0) @binding(0) var<storage, read_write> ...`
/// (any element type); `init` seeds the buffer and `workgroups` is the
/// dispatch size. Returns the storage buffer for [`read_buffer_sync`].
/// This is the "arbitrary custom compute" half of the programmable contract:
/// simulation steps, image filters, or tensor prep all enter here.
pub fn run_storage_compute(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    wgsl: &str,
    entry: &str,
    workgroups: [u32; 3],
    init: &[u8],
) -> Result<wgpu::Buffer, ViewportError> {
    if init.is_empty() {
        return Err(ViewportError::InvalidMesh("compute buffer is empty".into()));
    }
    if init.len() as u64 > 256 * 1024 * 1024 {
        return Err(ViewportError::InvalidMesh(
            "compute buffer exceeds 256 MiB".into(),
        ));
    }
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("aqua-compute"),
        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
    });
    let storage = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("aqua-compute-storage"),
        size: init.len() as u64,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    queue.write_buffer(&storage, 0, init);
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("aqua-compute-bind"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("aqua-compute-group"),
        layout: &layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: storage.as_entire_binding(),
        }],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("aqua-compute-layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("aqua-compute-pipeline"),
        layout: Some(&pipeline_layout),
        module: &module,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("aqua-compute-encode"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("aqua-compute-pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(workgroups[0], workgroups[1], workgroups[2]);
    }
    queue.submit([encoder.finish()]);
    Ok(storage)
}

/// Copy `size` bytes from `src` to CPU memory (blocking map + poll).
/// Used by tests and CPU-feedback loops; never call per-frame in production
/// (it stalls the queue) — prefer fixed-step readback deadlines instead.
pub fn read_buffer_sync(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    src: &wgpu::Buffer,
    size: u64,
) -> Result<Vec<u8>, ViewportError> {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("aqua-readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("aqua-readback-encode"),
    });
    encoder.copy_buffer_to_buffer(src, 0, &staging, 0, size);
    queue.submit([encoder.finish()]);
    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| ViewportError::Gpu(format!("{e:?}")))?;
    rx.recv()
        .map_err(|_| ViewportError::Gpu("readback channel closed".into()))?
        .map_err(|e| ViewportError::Gpu(format!("{e:?}")))?;
    let out = slice
        .get_mapped_range()
        .map_err(|e| ViewportError::Gpu(format!("{e:?}")))?
        .to_vec();
    staging.unmap();
    Ok(out)
}

/// Read back a whole texture (`Rgba8*`, full-frame, no padding) to CPU bytes.
pub fn read_texture_rgba8_sync(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, ViewportError> {
    let stride = width.div_ceil(256) * 256 * 4;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("aqua-tex-readback"),
        size: (stride * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("aqua-tex-copy"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride * 4 / 4),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| ViewportError::Gpu(format!("{e:?}")))?;
    rx.recv()
        .map_err(|_| ViewportError::Gpu("readback channel closed".into()))?
        .map_err(|e| ViewportError::Gpu(format!("{e:?}")))?;
    let row = width as usize * 4;
    let out = {
        let mapped = slice
            .get_mapped_range()
            .map_err(|e| ViewportError::Gpu(format!("{e:?}")))?;
        let mut out = vec![0u8; (width * height * 4) as usize];
        for y in 0..height as usize {
            out[y * row..(y + 1) * row]
                .copy_from_slice(&mapped[y * stride as usize..y * stride as usize + row]);
        }
        out
    };
    staging.unmap();
    Ok(out)
}

// ---------------------------------------------------------------------------
// ViewportPass: owned render-to-texture host (canonical scene feed)
// ---------------------------------------------------------------------------

/// A hosted 3D viewport: owns its color + depth textures, runs a
/// [`ViewportRenderer`], and feeds frames into the 2D scene through the
/// canonical [`ViewportPass::render_offscreen`] + `register_external` path
/// (true painter order, clips and overlays honored).
///
/// Lifetime contract (binding):
/// - [`ViewportPass::resize`] on every window/scale change (recreates
///   targets, notifies the renderer, keeps the epoch).
/// - [`ViewportPass::recreate_device`] after device loss (rebuilds every GPU
///   object, bumps [`ViewportPass::epoch`], notifies the renderer).
pub struct ViewportPass {
    device: wgpu::Device,
    queue: wgpu::Queue,
    id: ViewportId,
    epoch: u64,
    width: u32,
    height: u32,
    scale: f32,
    color_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    renderer: Mutex<Box<dyn ViewportRenderer>>,
    renderer_name: String,
    bounds: Rect, // logical
    opacity: f32,
    clip_radius: f32, // logical
    frames_rendered: u64,
}

impl ViewportPass {
    /// Create the pass: validates `desc`, allocates targets, names the layer.
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        desc: &ViewportDesc,
        renderer: Box<dyn ViewportRenderer>,
    ) -> Result<Self, ViewportError> {
        desc.validate()?;
        let renderer_name = renderer.name().to_owned();
        let color = Self::make_color(&device, desc);
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        // Depth needs no queue work; keep the signature symmetric for callers.
        let depth_view = Self::make_depth(&device, desc);
        Ok(Self {
            device,
            queue,
            id: issue_viewport_id(),
            epoch: 0,
            width: desc.width,
            height: desc.height,
            scale: desc.scale,
            color_format: desc.color_format,
            depth_format: desc.depth_format,
            color,
            color_view,
            depth_view,
            renderer: Mutex::new(renderer),
            renderer_name,
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                w: desc.width as f32,
                h: desc.height as f32,
            },
            opacity: 1.0,
            clip_radius: 0.0,
            frames_rendered: 0,
        })
    }

    /// Stable viewport handle (layer namespace + diagnostics).
    pub fn id(&self) -> ViewportId {
        self.id
    }

    /// Device-generation counter; bumps on every [`ViewportPass::recreate_device`].
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Current target size in physical pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// UI scale (logical px * scale = physical px). The scene feeds logical
    /// bounds; this texture is physical — the facade keeps them consistent
    /// by calling [`ViewportPass::resize`] on scale changes.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Scene `layer` key for the eager texture path.
    pub fn layer(&self) -> u64 {
        viewport_layer(self.id)
    }

    /// Scene item referencing this viewport's texture (eager path only;
    /// requires [`ViewportPass::render_offscreen`] before `prepare`).
    pub fn item(&self) -> DisplayItem {
        DisplayItem::External {
            bounds: self.bounds,
            layer: self.layer(),
            opacity: self.opacity,
            rounded_clip: self.clip_radius,
        }
    }

    /// A cloneable view of the owned color texture for `register_external`.
    pub fn color_view(&self) -> wgpu::TextureView {
        self.color_view.clone()
    }

    /// Update layout/opacity/clip (logical units, like the scene).
    pub fn set_layout(&mut self, bounds: Rect, opacity: f32, rounded_clip: f32) {
        self.bounds = bounds;
        self.opacity = opacity.clamp(0.0, 1.0);
        self.clip_radius = rounded_clip.max(0.0);
    }

    /// Recreate targets after a window/scale change. Same device, same
    /// epoch; the renderer is notified.
    pub fn resize(&mut self, width: u32, height: u32, scale: f32) -> Result<(), ViewportError> {
        let desc = ViewportDesc {
            width,
            height,
            scale,
            color_format: self.color_format,
            depth_format: self.depth_format,
        };
        desc.validate()?;
        self.width = width;
        self.height = height;
        self.scale = scale;
        let color = Self::make_color(&self.device, &desc);
        self.color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        self.color = color;
        self.depth_view = Self::make_depth(&self.device, &desc);
        if let Ok(mut r) = self.renderer.lock() {
            r.on_resize(width, height);
        }
        Ok(())
    }

    /// Rebuild every GPU object on a new device after loss. Bumps the epoch;
    /// the renderer rebuilds via `on_device_recreated`.
    pub fn recreate_device(
        &mut self,
        device: wgpu::Device,
        queue: wgpu::Queue,
    ) -> Result<(), ViewportError> {
        let desc = ViewportDesc {
            width: self.width,
            height: self.height,
            scale: self.scale,
            color_format: self.color_format,
            depth_format: self.depth_format,
        };
        desc.validate()?;
        let color = Self::make_color(&device, &desc);
        self.color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        self.color = color;
        self.device = device;
        self.queue = queue;
        self.depth_view = Self::make_depth(&self.device, &desc);
        self.epoch += 1;
        if let Ok(mut r) = self.renderer.lock() {
            r.on_device_recreated(&self.device, &self.queue);
        }
        Ok(())
    }

    /// Frames rendered via [`ViewportPass::render_offscreen`] (diagnostics).
    pub fn frames_rendered(&self) -> u64 {
        self.frames_rendered
    }

    /// Renderer name (diagnostics/inspector).
    pub fn renderer_name(&self) -> &str {
        &self.renderer_name
    }

    /// Render the scene into the owned texture with a private encoder and
    /// submit. Call before `prepare`, then feed [`ViewportPass::color_view`]
    /// to `register_external` and [`ViewportPass::item`] to the scene.
    /// Errors propagate to the caller; nothing is skipped silently.
    pub fn render_offscreen(&mut self) -> Result<(), ViewportError> {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("aqua-viewport-offscreen"),
            });
        self.render_scene(&mut encoder)?;
        self.queue.submit([encoder.finish()]);
        self.frames_rendered += 1;
        Ok(())
    }

    fn render_scene(&self, encoder: &mut wgpu::CommandEncoder) -> Result<(), ViewportError> {
        let mut renderer = self
            .renderer
            .lock()
            .map_err(|_| ViewportError::Gpu("viewport renderer lock poisoned".into()))?;
        let mut ctx = ViewportFrameCtx {
            device: &self.device,
            queue: &self.queue,
            encoder,
            color_view: &self.color_view,
            depth_view: &self.depth_view,
            width: self.width,
            height: self.height,
        };
        // The pass owns clearing: MeshRenderer clears+draws in one pass.
        // Custom renderers receive uncleared targets and own their loads.
        renderer.render(&mut ctx)
    }

    fn make_color(device: &wgpu::Device, desc: &ViewportDesc) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("aqua-viewport-color"),
            size: wgpu::Extent3d {
                width: desc.width.max(1),
                height: desc.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: desc.color_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    fn make_depth(device: &wgpu::Device, desc: &ViewportDesc) -> wgpu::TextureView {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("aqua-viewport-depth"),
                size: wgpu::Extent3d {
                    width: desc.width.max(1),
                    height: desc.height.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: desc.depth_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    }
}

// ---------------------------------------------------------------------------
// Input capture, scheduling, fixed-step simulation
// ---------------------------------------------------------------------------

/// Pointer-capture policy for a focused viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureMode {
    /// No capture; pointer events route by hit-testing as usual.
    None,
    /// Soft capture: the viewport keeps receiving pointer events after
    /// press while the contact is down (framework-level grab).
    Soft,
    /// Hard capture: cursor hidden and confined with relative motion where
    /// the platform supports it. Platforms without support report
    /// `Unsupported` on request; the viewport keeps working uncaptured.
    Hard,
}

/// Focus-scoped input state for one viewport.
#[derive(Clone, Copy, Debug)]
pub struct ViewportInput {
    /// Whether the viewport currently holds keyboard/pointer focus.
    pub focused: bool,
    /// Active capture policy.
    pub capture: CaptureMode,
    /// Last pointer position in viewport-local logical units (if inside).
    pub pointer_local: Option<[f32; 2]>,
    /// Relative motion since the last event (Hard capture only).
    pub relative_delta: Option<[f32; 2]>,
}

impl Default for ViewportInput {
    fn default() -> Self {
        Self {
            focused: false,
            capture: CaptureMode::None,
            pointer_local: None,
            relative_delta: None,
        }
    }
}

/// Map a window-local logical position into viewport-local logical units.
/// Returns `None` when the position falls outside `bounds` (the caller keeps
/// routing those events to the surrounding UI).
pub fn translate_pointer(window_pos: [f32; 2], bounds: Rect) -> Option<[f32; 2]> {
    if window_pos[0] >= bounds.x
        && window_pos[0] < bounds.x + bounds.w
        && window_pos[1] >= bounds.y
        && window_pos[1] < bounds.y + bounds.h
    {
        Some([window_pos[0] - bounds.x, window_pos[1] - bounds.y])
    } else {
        None
    }
}

/// Frame scheduling policy for a viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameDemand {
    /// Render only when scene damage, input, or an explicit deadline demands.
    OnDemand,
    /// Render every framework frame (animation, simulation preview).
    Continuous,
}

/// Fixed-step simulation clock independent of render cadence.
///
/// `advance(elapsed)` returns how many `step`s to run this frame (possibly
/// zero). Past `max_steps` worth of backlog the excess time is dropped and
/// counted in `dropped` (spiral-of-death guard): the simulation slows down
/// instead of freezing the UI, and the inspector sees exactly how much time
/// was discarded.
#[derive(Clone, Debug)]
pub struct FixedStepper {
    step: std::time::Duration,
    acc: std::time::Duration,
    max_steps: u32,
    /// Backlog discards served so far.
    pub dropped: u64,
}

impl FixedStepper {
    /// `step`: fixed simulation dt; `max_steps`: per-frame cap (>= 1).
    pub fn new(step: std::time::Duration, max_steps: u32) -> Self {
        Self {
            step: step.max(std::time::Duration::from_nanos(1)),
            acc: std::time::Duration::ZERO,
            max_steps: max_steps.max(1),
            dropped: 0,
        }
    }

    /// Feed elapsed wall time; returns simulation steps to run now.
    pub fn advance(&mut self, elapsed: std::time::Duration) -> u32 {
        self.acc += elapsed;
        let mut steps = 0;
        while self.acc >= self.step && steps < self.max_steps {
            self.acc -= self.step;
            steps += 1;
        }
        if self.acc >= self.step {
            self.dropped += 1;
            self.acc = std::time::Duration::ZERO;
        }
        steps
    }

    /// Fixed dt (for systems that need it explicitly).
    pub fn step(&self) -> std::time::Duration {
        self.step
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn layer_tags_are_disjoint_and_stable() {
        let a = issue_viewport_id();
        let b = issue_viewport_id();
        assert_ne!(a, b, "issuer never repeats indices");
        let la = viewport_layer(a);
        let lb = viewport_layer(b);
        assert_ne!(la, lb);
        assert_eq!(la & LAYER_TAG_VIEWPORT, LAYER_TAG_VIEWPORT);
        assert_eq!(la & crate::media::LAYER_TAG_MEDIA, 0);
        // Canonical decode round-trips the issued handle.
        let (kind, back) = decode_layer(la).expect("known tag decodes");
        assert_eq!(kind, LayerKind::Viewport);
        assert_eq!(back, a.raw());
    }

    #[test]
    fn layer_roundtrips_max_generation_without_truncation() {
        let raw = RawId {
            index: u32::MAX,
            generation: LAYER_MAX_GENERATION,
        };
        let layer = viewport_layer(ViewportId::from_raw(raw));
        let (kind, back) = decode_layer(layer).expect("max coords decode");
        assert_eq!(kind, LayerKind::Viewport);
        assert_eq!(back, raw);
    }

    #[test]
    #[should_panic(expected = "overflowed layer capacity")]
    fn layer_fails_closed_above_max_generation() {
        let _ = viewport_layer(ViewportId::from_raw(RawId {
            index: 0,
            generation: LAYER_MAX_GENERATION + 1,
        }));
    }

    #[test]
    fn desc_validation_is_explicit() {
        let ok = ViewportDesc::default();
        assert!(ok.validate().is_ok());
        assert!(matches!(
            ViewportDesc { width: 0, ..ok }.validate(),
            Err(ViewportError::BadSize(0, 256))
        ));
        assert!(matches!(
            ViewportDesc { scale: 0.0, ..ok }.validate(),
            Err(ViewportError::BadScale)
        ));
        assert!(matches!(
            ViewportDesc {
                scale: f32::INFINITY,
                ..ok
            }
            .validate(),
            Err(ViewportError::BadScale)
        ));
    }

    #[test]
    fn mesh_builders_validate_and_measure() {
        let cube = MeshData::cube();
        assert!(cube.validate().is_ok());
        assert_eq!(cube.triangle_count(), 12);
        let (min, max) = cube.aabb().unwrap();
        assert_eq!((min, max), ([-1.0; 3], [1.0; 3]));
        let quad = MeshData::quad(0.5, [1.0; 4]);
        assert!(quad.validate().is_ok());
        assert_eq!(quad.triangle_count(), 2);
        let grid = MeshData::grid(4, 0.0, [1.0; 4]);
        assert!(grid.validate().is_ok());
        assert_eq!(grid.triangle_count(), 32);
        // Mismatched streams and bad indices fail, never half-upload.
        let bad = MeshData::from_positions_indices(vec![[0.0; 3]], vec![7]);
        assert!(matches!(bad.validate(), Err(ViewportError::InvalidMesh(_))));
        let bad_len = MeshData::cube().with_colors(vec![[1.0; 4]]);
        assert!(matches!(
            bad_len.validate(),
            Err(ViewportError::InvalidMesh(_))
        ));
        assert!(MeshData::default().validate().is_err());
    }

    #[test]
    fn camera_math_maps_known_points() {
        // Ortho box [-1,1]^2 maps corners to NDC corners.
        let cam = Camera::ortho_box(-1.0, 1.0, -1.0, 1.0, -1.0, 1.0);
        let p = transform_point(&cam.view_proj, [1.0, 1.0, 0.0]);
        assert!((p[0] - 1.0).abs() < 1e-6 && (p[1] - 1.0).abs() < 1e-6);
        // Depth: nearer (smaller z) maps to smaller NDC z under Less.
        let near = transform_point(&cam.view_proj, [0.0, 0.0, 0.1])[2];
        let far = transform_point(&cam.view_proj, [0.0, 0.0, 0.3])[2];
        assert!(near < far);
        // Look-at centers the target; perspective keeps it inside NDC.
        let persp = Camera::perspective(
            [3.0, 2.0, 4.0],
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            60.0_f32.to_radians(),
            1.0,
            0.1,
            100.0,
        );
        let c = transform_point(&persp.view_proj, [0.0, 0.0, 0.0]);
        assert!(c[0].abs() < 1e-5 && c[1].abs() < 1e-5);
        assert!((0.0..1.0).contains(&c[2]), "center in depth range: {c:?}");
    }

    #[test]
    fn pointer_translation_and_stepper() {
        let bounds = Rect {
            x: 10.0,
            y: 20.0,
            w: 100.0,
            h: 50.0,
        };
        assert_eq!(translate_pointer([15.0, 25.0], bounds), Some([5.0, 5.0]));
        assert_eq!(translate_pointer([0.0, 0.0], bounds), None);
        assert_eq!(translate_pointer([110.0, 30.0], bounds), None);

        let step = Duration::from_millis(16);
        let mut s = FixedStepper::new(step, 4);
        assert_eq!(s.advance(Duration::from_millis(100)), 4);
        assert_eq!(s.dropped, 1, "100ms backlog at cap 4 drops");
        assert_eq!(s.advance(Duration::from_millis(8)), 0);
        assert_eq!(s.advance(Duration::from_millis(8)), 1);
    }

    #[test]
    fn capabilities_probe_reports_honestly() {
        let Some((device, _)) = headless_device("capabilities_probe_reports_honestly") else {
            eprintln!("SKIP capabilities_probe_reports_honestly (no adapter)");
            return;
        };
        let caps = ViewportCapabilities::probe(&device);
        assert!(caps.depth && caps.compute);
        assert!(caps.max_texture_dim >= 256);
    }

    // -- GPU-backed tests: strict skip contract (see media.rs: only a
    // genuine `NotFound` skips; everything else fails). --

    fn headless_device(caller: &str) -> Option<(wgpu::Device, wgpu::Queue)> {
        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_desc.backends = wgpu::Backends::all();
        let instance = wgpu::Instance::new(instance_desc);
        let adapter =
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })) {
                Ok(adapter) => adapter,
                Err(wgpu::RequestAdapterError::NotFound { .. }) => {
                    if std::env::var_os("AQUA_REQUIRE_GPU").is_some() {
                        panic!("AQUA_REQUIRE_GPU=1 but no adapter for {caller}");
                    }
                    eprintln!("SKIP {caller} (genuine NotFound: no adapter)");
                    return None;
                }
                Err(e) => panic!("{caller}: adapter request failed, must not skip: {e:?}"),
            };
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .unwrap_or_else(|e| {
                    panic!("{caller}: device request failed, must not skip: {e:?}")
                });
        Some((device, queue))
    }

    fn depth_fixture_mesh() -> MeshData {
        // Far red quad + near green quad: center MUST resolve green.
        let mut far = MeshData::quad(0.3, [1.0, 0.0, 0.0, 1.0]);
        let mut near = MeshData::quad(0.1, [0.0, 1.0, 0.0, 1.0]);
        for p in near.positions.iter_mut() {
            p[0] *= 0.4;
            p[1] *= 0.4;
        }
        let base = far.positions.len() as u32;
        far.positions.extend(near.positions);
        far.colors.as_mut().unwrap().extend(near.colors.unwrap());
        far.indices.extend(near.indices.iter().map(|i| i + base));
        far
    }

    #[test]
    fn depth_tested_mesh_resolves_front_face() {
        let Some((device, queue)) = headless_device("depth_tested_mesh_resolves_front_face") else {
            eprintln!("SKIP depth_tested_mesh_resolves_front_face (no adapter)");
            return;
        };
        let desc = ViewportDesc {
            width: 64,
            height: 64,
            ..Default::default()
        };
        let camera = Camera::ortho_box(-1.0, 1.0, -1.0, 1.0, -1.0, 1.0);
        let mesh = depth_fixture_mesh();
        let mut renderer = MeshRenderer::new(
            &device,
            desc.color_format,
            desc.depth_format,
            &mesh,
            camera,
            [0.0; 4],
        )
        .expect("mesh renderer");
        let color = ViewportPass::make_color(&device, &desc);
        let depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("test-depth"),
                size: wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: desc.depth_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("test-encode"),
        });
        {
            let mut ctx = ViewportFrameCtx {
                device: &device,
                queue: &queue,
                encoder: &mut encoder,
                color_view: &color_view,
                depth_view: &depth,
                width: 64,
                height: 64,
            };
            renderer.render(&mut ctx).expect("render");
        }
        queue.submit([encoder.finish()]);
        let px = read_texture_rgba8_sync(&device, &queue, &color, 64, 64).expect("readback");
        // Center: near green quad wins the depth test.
        let c = &px[(32 * 64 + 32) * 4..][..4];
        assert!(c[1] > 200 && c[0] < 30 && c[3] == 255, "front face: {c:?}");
        // Corner: far red quad (outside the green quad, inside red).
        let r = &px[(48 * 64 + 48) * 4..][..4];
        assert!(r[0] > 200 && r[1] < 30, "back face: {r:?}");
    }

    #[test]
    fn compute_shader_writes_readable_buffer() {
        let Some((device, queue)) = headless_device("compute_shader_writes_readable_buffer") else {
            eprintln!("SKIP compute_shader_writes_readable_buffer (no adapter)");
            return;
        };
        const N: usize = 256;
        let buf = run_storage_compute(
            &device,
            &queue,
            r#"
            @group(0) @binding(0) var<storage, read_write> buf: array<u32>;
            @compute @workgroup_size(64)
            fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
                if (gid.x < 256u) { buf[gid.x] = gid.x * 2u + 1u; }
            }
            "#,
            "main",
            [4, 1, 1],
            &vec![0u8; N * 4],
        )
        .expect("compute");
        let out = read_buffer_sync(&device, &queue, &buf, (N * 4) as u64).expect("readback");
        let words: Vec<u32> = out
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect();
        assert_eq!(words.len(), N);
        for (i, w) in words.iter().enumerate() {
            assert_eq!(*w, i as u32 * 2 + 1, "lane {i}");
        }
    }

    #[test]
    fn viewport_pass_composes_through_real_renderer() {
        let Some((device, queue)) = headless_device("viewport_pass_composes_through_real_renderer")
        else {
            eprintln!("SKIP viewport_pass_composes_through_real_renderer (no adapter)");
            return;
        };
        // Eager path: render_offscreen -> register_external -> External item.
        let desc = ViewportDesc {
            width: 64,
            height: 64,
            ..Default::default()
        };
        let camera = Camera::ortho_box(-1.0, 1.0, -1.0, 1.0, -1.0, 1.0);
        let mesh = MeshData::quad(0.0, [0.0, 0.0, 1.0, 1.0]);
        let mesh_renderer = MeshRenderer::new(
            &device,
            desc.color_format,
            desc.depth_format,
            &mesh,
            camera,
            [0.0; 4],
        )
        .expect("mesh renderer");
        let mut pass = ViewportPass::new(
            device.clone(),
            queue.clone(),
            &desc,
            Box::new(mesh_renderer),
        )
        .expect("viewport pass");
        assert_eq!(pass.epoch(), 0);
        pass.set_layout(
            Rect {
                x: 32.0,
                y: 32.0,
                w: 64.0,
                h: 64.0,
            },
            1.0,
            0.0,
        );
        pass.render_offscreen().expect("offscreen");
        assert_eq!(pass.frames_rendered(), 1);

        let mut renderer = crate::renderer::Renderer::new(
            device.clone(),
            queue.clone(),
            wgpu::TextureFormat::Rgba8Unorm,
        );
        renderer.set_size(128, 128, 1.0).expect("size");
        let layer = pass.layer();
        renderer.register_external(layer, pass.color_view(), 64, 64);
        let mut scene = crate::scene::Scene::new();
        scene.insert(pass.item());
        let plan = renderer.prepare(&mut scene);
        renderer.draw(&plan).expect("draw");
        let px = renderer.readback().expect("readback");
        // Viewport quad covers scene center: blue, opaque, linear.
        let c = &px[(64 * 128 + 64) * 4..][..4];
        assert!(
            c[2] > 200 && c[0] < 30 && c[3] == 255,
            "viewport texel: {c:?}"
        );
        // Outside the viewport rect the base stays transparent.
        assert_eq!(&px[0..4], &[0, 0, 0, 0]);

        // Resize keeps the pass rendering (same device, same epoch).
        pass.resize(32, 32, 1.0).expect("resize");
        assert_eq!(pass.size(), (32, 32));
        assert_eq!(pass.epoch(), 0);
        pass.render_offscreen().expect("offscreen after resize");
        assert_eq!(pass.frames_rendered(), 2);
    }

    #[test]
    fn viewport_under_toolbar_with_rounded_clip_first_frame() {
        let Some((device, queue)) =
            headless_device("viewport_under_toolbar_with_rounded_clip_first_frame")
        else {
            return;
        };
        // Canonical painter order on the VERY FIRST frame: the 3D viewport
        // item comes first, then an opaque toolbar overlapping its bottom
        // edge. The viewport layer carries a rounded clip; the toolbar must
        // win where they overlap and the rounded corner must clip.
        let desc = ViewportDesc {
            width: 64,
            height: 64,
            ..Default::default()
        };
        let camera = Camera::ortho_box(-1.0, 1.0, -1.0, 1.0, -1.0, 1.0);
        let mesh = MeshData::quad(0.0, [0.0, 0.0, 1.0, 1.0]);
        let mesh_renderer = MeshRenderer::new(
            &device,
            desc.color_format,
            desc.depth_format,
            &mesh,
            camera,
            [0.0; 4],
        )
        .expect("mesh renderer");
        let mut pass = ViewportPass::new(
            device.clone(),
            queue.clone(),
            &desc,
            Box::new(mesh_renderer),
        )
        .expect("viewport pass");
        // Viewport covers the full 64x64 scene with a 16px rounded clip.
        pass.set_layout(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 64.0,
                h: 64.0,
            },
            1.0,
            16.0,
        );
        // ONE offscreen render, then a single prepare/draw: first frame.
        pass.render_offscreen().expect("offscreen");
        let mut renderer = crate::renderer::Renderer::new(
            device.clone(),
            queue.clone(),
            wgpu::TextureFormat::Rgba8Unorm,
        );
        renderer.set_size(64, 64, 1.0).expect("size");
        let layer = pass.layer();
        renderer.register_external(layer, pass.color_view(), 64, 64);
        let mut scene = crate::scene::Scene::new();
        scene.insert(pass.item());
        let green = aqua_core::Oklch::from_hex("#00ff00").unwrap();
        scene.insert(DisplayItem::Rect {
            bounds: Rect {
                x: 0.0,
                y: 48.0,
                w: 64.0,
                h: 16.0,
            },
            radius: 0.0,
            fill: green,
            border_width: 0.0,
            border_color: green,
            shadow: None,
        });
        let plan = renderer.prepare(&mut scene);
        renderer.draw(&plan).expect("draw");
        let px = renderer.readback().expect("readback");
        // 3D content visible where nothing covers it: blue, opaque, linear.
        let c = &px[(24 * 64 + 32) * 4..][..4];
        assert!(
            c[2] > 200 && c[1] < 30 && c[3] == 255,
            "viewport texel: {c:?}"
        );
        // Opaque toolbar wins the overlap (painter order, first frame).
        let t = &px[(56 * 64 + 32) * 4..][..4];
        assert!(
            t[1] > 200 && t[2] < 30 && t[3] == 255,
            "toolbar texel: {t:?}"
        );
        // Rounded corner clips the layer: (2,2) is inside the layer bounds
        // but outside the 16px corner radius, so the base shows through.
        let k = &px[(2 * 64 + 2) * 4..][..4];
        assert!(k[3] < 10, "clipped corner must stay clear, got {k:?}");
        // Edge center (not a corner) still shows 3D content.
        let e = &px[(32 * 64 + 2) * 4..][..4];
        assert!(e[2] > 200 && e[3] == 255, "clipped edge texel: {e:?}");
    }
}
