//! Shared wgpu 2D renderer: pipelines, retained instance mirror,
//! segmented composition (backdrop blur, isolated opacity groups, external
//! layers, custom passes), bounded scratch pool, diagnostics.
//!
//! Frame model (honest partial-update contract):
//! - The facade schedules a frame only when damage is non-clean, an animation
//!   / media deadline fires, or the platform demands presentation. Clean idle
//!   frames are SKIPPED, never presented.
//! - A presented frame re-encodes commands for visible ops (one rect draw
//!   call + effect passes) but instance-buffer UPLOADS are range-diffed
//!   against the retained mirror: a trivial update uploads bytes for the
//!   changed range only. `diagnostics()` reports both sides.

use aqua_core::Oklch;
use std::collections::{HashMap, HashSet};

use crate::damage::{DamageSet, DamageTracker, InvalidationReason};
use crate::effects::{BlurParams, CustomPass};
use crate::resource::{RenderDiagnostics, ResourceBudget, TextureDesc};
use crate::scene::{DisplayItem, Rect, Scene};
use crate::text::{
    resolve_scene_text, GlyphonProvider, ShapedRun, TextBackend, TextFrame, TextProvider,
};

#[derive(Debug)]
pub enum RenderError {
    AdapterNotFound(String),
    DeviceFailed(String),
    SurfaceFailed(String),
    BudgetExceeded(&'static str),
    TextureTooLarge(u32, u32),
    Gpu(String),
    Vector(crate::vector::VectorError),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::AdapterNotFound(s) => write!(f, "no usable GPU adapter: {s}"),
            RenderError::DeviceFailed(s) => write!(f, "device request failed: {s}"),
            RenderError::SurfaceFailed(s) => write!(f, "surface failed: {s}"),
            RenderError::BudgetExceeded(s) => write!(f, "resource budget exceeded: {s}"),
            RenderError::TextureTooLarge(w, h) => write!(f, "target too large: {w}x{h}"),
            RenderError::Gpu(s) => write!(f, "gpu error: {s}"),
            RenderError::Vector(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for RenderError {}

// ---------------------------------------------------------------------------
// GPU instance layouts (must match shaders/rect.wgsl, shaders/compose.wgsl)
// ---------------------------------------------------------------------------

/// 8 x vec4<f32> = 128 bytes. Field order matches rect.wgsl VsIn 1..8.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct RectInstance {
    rect: [f32; 4],
    rb_pack: [f32; 4], // radius, border_width, 0, 0
    fill: [f32; 4],
    border: [f32; 4],
    shadow: [f32; 4], // ox, oy, blur, enabled
    shadow_color: [f32; 4],
    clip: [f32; 4], // x, y, w, h; w < 0 disables
    misc: [f32; 4], // clip_radius, opacity, 0, 0
}

/// 3 x vec4<f32> = 48 bytes. Matches compose.wgsl VsIn 0,1,3.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct ComposeInstance {
    dst: [f32; 4],
    src: [f32; 4], // uv-space rect
    clip: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct RectUniforms {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BlurUniforms {
    texel: [f32; 2],
    direction: [f32; 2],
    sigma: f32,
    step: f32,
    _pad: [f32; 2],
}

/// Compose uniform ABI (BINDING CONTRACT — media/viewport owners: any
/// hand-rolled compose bind group must match this EXACTLY):
/// 8 floats / 32 bytes, offsets viewport@0, opacity@8, clip_radius@12,
/// output_gamma@16, premultiplied@20, pad@24. A 7-float / 28-byte buffer
/// fails validation against the shader. See `compose_uniform_abi_32b`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ComposeUniforms {
    viewport: [f32; 2],
    opacity: f32,
    clip_radius: f32,
    output_gamma: f32,
    premultiplied: f32,
    _pad: [f32; 2],
}

// ---------------------------------------------------------------------------
// Frame plan
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct GroupOp {
    text_maps: Vec<aqua_core::scene::TextColorMap>,
    filter: Option<(Rect, f32)>,
    clips: Vec<(Rect, f32)>,
    bounds: Rect,
    opacity: f32,
    instances: Vec<RectInstance>,
    commands: Vec<GroupCommand>,
}

#[derive(Debug)]
enum GroupCommand {
    Rects { start: u32, count: u32 },
    Text(ShapedRun),
    Group(GroupOp),
    Surface(SurfaceOp),
}

/// Cached per-node paint context from the last slow-path walk: physical
/// clip + radius and whether the node sits inside an opacity group.
/// Fast-path value updates reuse it; structural changes rebuild it.
#[derive(Clone, Copy, Debug)]
struct NodeCtx {
    clip: Rect, // physical px; w < 0 = unclipped
    clip_r: f32,
    clip_index: u32,
    in_group: bool,
}

#[derive(Debug)]
enum Op {
    Rects {
        start: u32,
        count: u32,
        /// Physical-px union of the run's quads: partial repaint skips runs
        /// that don't intersect damage.
        bounds: Rect,
    },
    Blur {
        bounds: Rect,
        params: BlurParams,
    }, // physical px
    Group(GroupOp),
    Surface(SurfaceOp),
    /// Text runs in painter position. Drawn via the text provider into
    /// whatever target is current (base or group scratch).
    Texts(Vec<ShapedRun>),
}

#[derive(Debug)]
struct SurfaceOp {
    pixel_scale: f32,
    bounds: Rect,
    source: SurfaceSource,
    opacity: f32,
    clips: Vec<(Rect, f32)>,
    placement: Option<(aqua_core::image::ImageFit, aqua_core::Point, Rect)>,
}

#[derive(Debug)]
enum SurfaceSource {
    External(u64),
    Vector(aqua_core::vector::VectorSource),
}

/// CPU-side plan for one presented frame.
#[derive(Debug, Default)]
pub struct FramePlan {
    validation_error: Option<&'static str>,
    ops: Vec<Op>,
    instances: Vec<RectInstance>,
    damage: DamageSet,
    pub nodes_total: usize,
    pub nodes_updated: usize,
    pub full_upload: bool,
    /// Nodes traversed to build this plan (fast path: only changed nodes;
    /// slow path: whole scene). Benchmark-grade counter, not an estimate.
    pub nodes_traversed: usize,
    /// Raster area in physical px (sum of damage regions; full frame when
    /// `damage.full`). Exact, not estimated.
    pub raster_area_px: u64,
    /// True when the frame repainted only damage regions over the retained
    /// base instead of a full clear + redraw.
    pub partial: bool,
    /// True when live instances already sit in the renderer's mirror (fast
    /// path): `instances` is empty and uploads come from dirty slot ranges.
    pub(crate) mirror_current: bool,
}

impl FramePlan {
    pub fn is_clean(&self) -> bool {
        self.damage.is_clean()
    }

    /// Damage accumulated for this plan (conservative: `full` means repaint).
    pub fn damage(&self) -> &DamageSet {
        &self.damage
    }
}

// ---------------------------------------------------------------------------
// Scratch texture pool (real wgpu textures, bounded)
// ---------------------------------------------------------------------------

struct PoolEntry {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    w: u32,
    h: u32,
    bytes: u64,
    in_use: bool,
}

struct ScratchPool {
    entries: Vec<PoolEntry>,
    budget: ResourceBudget,
    pub evictions: u64,
    pub hits: u64,
    pub misses: u64,
}

impl ScratchPool {
    fn new(budget: ResourceBudget) -> Self {
        Self {
            entries: Vec::new(),
            budget,
            evictions: 0,
            misses: 0,
            hits: 0,
        }
    }

    fn used_bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.bytes).sum()
    }

    /// Acquire a scratch texture view of exactly (w,h). Returns entry index.
    /// Same accounting as [`TexturePoolModel`]: every resident entry counts
    /// against the byte budget, free or not; admission evicts free entries
    /// first and fails instead of exceeding the cap.
    fn acquire(
        &mut self,
        device: &wgpu::Device,
        w: u32,
        h: u32,
        format: wgpu::TextureFormat,
    ) -> Result<usize, RenderError> {
        let w = w.max(1);
        let h = h.max(1);
        let bytes = TextureDesc::rgba8(w, h)
            .checked_bytes()
            .ok_or(RenderError::TextureTooLarge(w, h))?;
        if bytes > self.budget.max_intermediate_bytes {
            return Err(RenderError::BudgetExceeded("single scratch exceeds budget"));
        }
        if let Some(i) = self
            .entries
            .iter()
            .position(|e| !e.in_use && e.w == w && e.h == h)
        {
            self.entries[i].in_use = true;
            self.hits += 1;
            return Ok(i);
        }
        self.misses += 1;
        // Evict free entries until the new texture fits.
        while self.used_bytes().checked_add(bytes).is_none_or(|total| {
            total > self.budget.max_intermediate_bytes
                || self.entries.len() >= self.budget.max_pooled_textures
        }) {
            match self.entries.iter().position(|e| !e.in_use) {
                Some(i) => {
                    self.entries.swap_remove(i);
                    self.evictions += 1;
                }
                None => return Err(RenderError::BudgetExceeded("scratch pool exhausted")),
            }
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("aqua-scratch"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.entries.push(PoolEntry {
            texture,
            view,
            w,
            h,
            bytes,
            in_use: true,
        });
        Ok(self.entries.len() - 1)
    }

    fn release(&mut self, index: usize) {
        if let Some(e) = self.entries.get_mut(index) {
            e.in_use = false;
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

const MAX_TARGET_DIM: u32 = 8192;
const MAX_UPLOAD_RANGES: usize = 8;
/// Cap on cached compose-pipeline format variants (base + surfaces).
const MAX_COMPOSE_PIPES: usize = 4;

struct SnapshotState {
    previous: usize,
    output: usize,
    settings: crate::effects::SnapshotTransition,
    elapsed: std::time::Duration,
}

pub struct Renderer {
    effect_compositor: crate::effects::compositor::EffectCompositor,
    snapshot: Option<SnapshotState>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat, // base working format (linear, non-sRGB)
    width: u32,
    height: u32,
    scale: f32,

    rect_pipeline: wgpu::RenderPipeline,
    /// Same rect shader with REPLACE blending: scissored region clears for
    /// partial repaint (premultiplied-over of transparent cannot clear).
    clear_pipeline: wgpu::RenderPipeline,
    blur_pipeline: wgpu::RenderPipeline,
    compose_pipes: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    rect_uniform_buf: wgpu::Buffer,
    rect_uniform_bind: wgpu::BindGroup,
    rect_bind_layout: wgpu::BindGroupLayout,
    clip_buffer: wgpu::Buffer,
    clip_capacity: usize,
    clip_nodes: Vec<[f32; 8]>,
    clip_dirty: bool,
    blur_bind_layout: wgpu::BindGroupLayout,
    compose_bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    quad_vb: wgpu::Buffer,
    quad_ib: wgpu::Buffer,

    instance_buf: wgpu::Buffer,
    instance_capacity: usize,
    mirror: Vec<RectInstance>,
    /// Retained base holds last frame's pixels (partial repaint over Load).
    /// False after resize/scale change until the next full repaint.
    base_valid: bool,
    /// Node index -> top-level instance slot, with per-node clip context.
    /// Built on slow-path walks; enables O(changed) fast-path updates.
    slots_ready: bool,
    slot_of: HashMap<u32, usize>,
    ctx_of: HashMap<u32, NodeCtx>,
    /// True when the committed scene contains blur/group/external/text/
    /// custom content: fast-path value updates can't cover those, and
    /// partial repaint falls back to full (documented, correct).
    has_complex: bool,
    /// Dirty instance slots from the fast path (uploads skip the diff).
    dirty_ranges: Vec<(usize, usize)>,

    base: Option<(wgpu::Texture, wgpu::TextureView)>,
    pool: ScratchPool,
    vectors: crate::vector::VectorCache,
    external: HashMap<u64, wgpu::TextureView>,
    external_size: HashMap<u64, (u32, u32)>,
    /// Layers with new content since the last prepare. Registration IS the
    /// invalidation signal: hosts must register only on demand (new frame),
    /// and every registration dirties the layer's bounds even when the
    /// scene is unchanged — otherwise new pixels would never composite.
    external_dirty: Vec<u64>,
    /// Last walked logical bounds per layer (for content damage without
    /// scene changes).
    layer_bounds: HashMap<u64, Rect>,
    /// Custom passes added since the last prepare. Only the transition
    /// invalidates (see prepare); passes persist across frames and the
    /// renderer never schedules frames by itself.
    unconsumed_passes: usize,
    custom_passes: Vec<Box<dyn CustomPass>>,

    text: Box<dyn TextProvider>,
    tracker: DamageTracker,
    last_bounds: HashMap<u32, Rect>,
    /// Last measured logical ink rect per text run id (for old-footprint
    /// damage when text moves/changes/is removed).
    text_ink: HashMap<u64, Rect>,
    diagnostics: RenderDiagnostics,
}

impl Renderer {
    // -- construction -------------------------------------------------------

    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        Self::new_with_text(device, queue, format, TextBackend::with_system_fonts())
    }

    /// Construction against the SHARED text backend (facade path: one font
    /// database and one run registry for the process). Prefer this over
    /// [`Renderer::new`], which falls back to a private backend.
    pub fn new_with_text(
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
        backend: TextBackend,
    ) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("aqua-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let rect_bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("aqua-rect-bind"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let rect_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aqua-rect-uniform"),
            size: std::mem::size_of::<RectUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let clip_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aqua-clip-tree"),
            size: 32,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let rect_uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("aqua-rect-bind-group"),
            layout: &rect_bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: clip_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: rect_uniform_buf.as_entire_binding(),
                },
            ],
        });

        let blend_premult = Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        });

        let rect_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("aqua-rect"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/rect.wgsl").into()),
        });
        let rect_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("aqua-rect-layout"),
            bind_group_layouts: &[Some(&rect_bind_layout)],
            immediate_size: 0,
        });
        let rect_attrs = wgpu::vertex_attr_array![
            0 => Float32x2,
        ];
        // Separate array so instance-attribute offsets restart at 0.
        let rect_instance_attrs = wgpu::vertex_attr_array![
            1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
            5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4,
        ];
        let rect_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("aqua-rect-pipeline"),
            layout: Some(&rect_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &rect_shader,
                entry_point: Some("vs_main"),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 8,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &rect_attrs,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<RectInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &rect_instance_attrs,
                    }),
                ],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &rect_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: blend_premult,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // Region-clear pipeline: identical vertex/fragment layout to the
        // rect pipeline, but REPLACE blending so a transparent quad actually
        // clears retained pixels (premultiplied-over of zero is a no-op).
        let blend_replace = Some(wgpu::BlendState::REPLACE);
        let clear_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("aqua-clear-pipeline"),
            layout: Some(&rect_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &rect_shader,
                entry_point: Some("vs_main"),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 8,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &rect_attrs,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<RectInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &rect_instance_attrs,
                    }),
                ],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &rect_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: blend_replace,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let blur_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("aqua-blur"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/blur.wgsl").into()),
        });
        let blur_bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("aqua-blur-bind"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let blur_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("aqua-blur-pipeline"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("aqua-blur-layout"),
                    bind_group_layouts: &[Some(&blur_bind_layout)],
                    immediate_size: 0,
                }),
            ),
            vertex: wgpu::VertexState {
                module: &blur_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &blur_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None, // overwrite scratch
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // Compose bind layout (format-independent; shared by all
        // per-format compose pipelines).
        let compose_bind_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("aqua-compose-bind"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        // Compose pipelines are per-target-format (base vs surface formats
        // may differ); seed with the base working format.
        let mut compose_pipes = HashMap::new();
        compose_pipes.insert(
            format,
            make_compose_pipeline(&device, &compose_bind_layout, format),
        );

        // Unit quad corners + indices, shared by rect and compose passes.
        let quad_vb = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aqua-quad-vb"),
            size: 8 * 4,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let quad_ib = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aqua-quad-ib"),
            size: 6 * 4,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Filled in set_size (queue available there); write now via queue.
        queue.write_buffer(
            &quad_vb,
            0,
            bytemuck::cast_slice(&[0f32, 0., 1., 0., 1., 1., 0., 1.]),
        );
        queue.write_buffer(&quad_ib, 0, bytemuck::cast_slice(&[0u32, 1, 2, 0, 2, 3]));

        let instance_capacity = 1024;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aqua-instances"),
            size: (instance_capacity * std::mem::size_of::<RectInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let text: Box<dyn TextProvider> =
            Box::new(GlyphonProvider::new(&device, &queue, format, backend));

        Self {
            effect_compositor: crate::effects::compositor::EffectCompositor::new(&device, format),
            snapshot: None,
            device,
            queue,
            format,
            width: 0,
            height: 0,
            scale: 1.0,
            rect_pipeline,
            clear_pipeline,
            blur_pipeline,
            compose_pipes,
            rect_uniform_buf,
            rect_uniform_bind,
            rect_bind_layout,
            clip_buffer,
            clip_capacity: 1,
            clip_nodes: Vec::new(),
            clip_dirty: true,
            blur_bind_layout,
            compose_bind_layout,
            sampler,
            quad_vb,
            quad_ib,
            instance_buf,
            instance_capacity,
            mirror: Vec::new(),
            base_valid: false,
            slots_ready: false,
            slot_of: HashMap::new(),
            ctx_of: HashMap::new(),
            has_complex: false,
            dirty_ranges: Vec::new(),
            base: None,
            pool: ScratchPool::new(ResourceBudget::default()),
            vectors: crate::vector::VectorCache::default(),
            external: HashMap::new(),
            external_size: HashMap::new(),
            external_dirty: Vec::new(),
            layer_bounds: HashMap::new(),
            unconsumed_passes: 0,
            custom_passes: Vec::new(),
            text,
            tracker: DamageTracker::new(1.0),
            last_bounds: HashMap::new(),
            text_ink: HashMap::new(),
            diagnostics: RenderDiagnostics::default(),
        }
    }

    /// Headless offscreen renderer on the best available adapter (software
    /// Vulkan lavapipe qualifies). Used by tests and headless hosts.
    pub async fn headless(width: u32, height: u32) -> Result<Self, RenderError> {
        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_desc.backends = wgpu::Backends::from_env().unwrap_or_else(wgpu::Backends::all);
        let instance = wgpu::Instance::new(instance_desc);
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|e| RenderError::AdapterNotFound(format!("{e:?}")))?;
        let info = adapter.get_info();
        eprintln!(
            "aqua-render headless adapter: {} ({:?})",
            info.name, info.backend
        );
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("aqua-headless"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|e| RenderError::DeviceFailed(format!("{e:?}")))?;
        let mut r = Self::new(device, queue, wgpu::TextureFormat::Rgba8Unorm);
        r.set_size(width, height, 1.0)?;
        Ok(r)
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    // -- sizing -------------------------------------------------------------

    pub fn set_size(&mut self, width: u32, height: u32, scale: f32) -> Result<(), RenderError> {
        let (w, h) = (width.max(1), height.max(1));
        if w > MAX_TARGET_DIM || h > MAX_TARGET_DIM {
            return Err(RenderError::TextureTooLarge(w, h));
        }
        let scale = scale.max(0.01);
        if self.width == w && self.height == h && self.scale == scale && self.base.is_some() {
            return Ok(());
        }
        self.cancel_snapshot_transition();
        self.width = w;
        self.height = h;
        self.scale = scale.max(0.01);
        self.tracker.set_scale(self.scale);
        self.tracker.set_frame_size(w as f32, h as f32);
        self.tracker
            .invalidate_full(InvalidationReason::StructuralChanged);
        // Physical-pixel state is stale: retained base, instance mirror and
        // slot map all rebuild on the next full repaint.
        self.base_valid = false;
        self.slots_ready = false;
        self.slot_of.clear();
        self.ctx_of.clear();
        self.has_complex = false;
        self.dirty_ranges.clear();
        self.mirror.clear();
        self.queue.write_buffer(
            &self.rect_uniform_buf,
            0,
            bytemuck::cast_slice(&[RectUniforms {
                viewport: [w as f32, h as f32],
                _pad: [0.0; 2],
            }]),
        );
        let base = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("aqua-base"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = base.create_view(&wgpu::TextureViewDescriptor::default());
        self.base = Some((base, view));
        self.pool.clear();
        Ok(())
    }

    // -- extension contracts --------------------------------------------------

    /// Feed an external frame (media.rs / viewport.rs owners call this per
    /// frame, ON DEMAND ONLY: each call invalidates the layer's bounds for
    /// the next prepare, even with an unchanged scene). Views must stay
    /// alive through `draw`; the renderer only samples them.
    pub fn register_external(&mut self, layer: u64, view: wgpu::TextureView, w: u32, h: u32) {
        self.external.insert(layer, view);
        self.external_size.insert(layer, (w, h));
        if !self.external_dirty.contains(&layer) {
            self.external_dirty.push(layer);
        }
    }

    pub fn unregister_external(&mut self, layer: u64) -> bool {
        let had = self.external.remove(&layer).is_some();
        self.external_size.remove(&layer);
        let bounds = self.layer_bounds.remove(&layer);
        self.external_dirty.retain(|&l| l != layer);
        if had {
            match bounds {
                Some(bounds) => self.tracker.record_rect_change(bounds, Some(bounds)),
                None => self
                    .tracker
                    .invalidate_full(InvalidationReason::ContentChanged),
            }
        }
        had
    }

    pub fn add_custom_pass(&mut self, pass: Box<dyn CustomPass>) {
        self.custom_passes.push(pass);
        self.unconsumed_passes += 1;
    }

    pub fn vector_stats(&self) -> crate::vector::VectorStats {
        self.vectors.stats()
    }

    pub fn set_vector_budget(&mut self, budget: crate::vector::VectorBudget) {
        self.vectors.set_budget(budget);
        self.base_valid = false;
        self.tracker
            .invalidate_full(InvalidationReason::StructuralChanged);
    }

    pub fn diagnostics(&self) -> &RenderDiagnostics {
        &self.diagnostics
    }

    /// Swap the text provider (facade wiring for the shared text backend).
    /// The provider must be built for the same device/format; see
    /// [`GlyphonProvider::new`].
    pub fn set_text_raster_mode(
        &mut self,
        mode: aqua_core::scene::TextRasterMode,
    ) -> Result<(), RenderError> {
        self.text
            .set_raster_mode(&self.device, &self.queue, self.format, mode)
            .map_err(RenderError::Gpu)?;
        self.base_valid = false;
        self.tracker
            .invalidate_full(InvalidationReason::ContentChanged);
        Ok(())
    }

    pub fn set_text_provider(&mut self, provider: Box<dyn TextProvider>) {
        self.text = provider;
    }

    /// Builder-style provider swap.
    pub fn with_text_provider(mut self, provider: Box<dyn TextProvider>) -> Self {
        self.text = provider;
        self
    }

    // -- prepare ---------------------------------------------------------------

    /// Drain scene changes, record damage, build the op list and diff the
    /// instance mirror. Returns a plan; clean plans carry empty damage and
    /// the caller should skip presentation.
    pub fn prepare(&mut self, scene: &mut Scene) -> FramePlan {
        let mut plan = FramePlan {
            validation_error: None,
            nodes_total: scene.len(),
            ..Default::default()
        };

        let changes = scene.drain_changes();
        let removed = scene.drain_removed();
        let structure_changed = scene.take_structure_changed();
        plan.nodes_updated = changes.len() + removed.len();

        // Damage from removals: repaint old effect-inclusive footprints.
        for item in &removed {
            match item {
                DisplayItem::Text { run, .. } => {
                    if let Some(old_ink) = self.text_ink.remove(&run.id) {
                        self.tracker.record_rect_change(old_ink, None);
                    } else {
                        self.tracker.record_change(item, None);
                    }
                }
                _ => match item_bounds(item) {
                    Some(b) => self.tracker.record_rect_change(b, None),
                    None => self.tracker.record_change(item, None),
                },
            }
        }
        // Damage from changes, with old-footprint coverage for moves.
        // Rect/External/Blur footprints are effect-inclusive (shadow union,
        // blur padding) by item_bounds; text uses measured ink.
        for (id, item) in &changes {
            match item {
                DisplayItem::Text { at, run, .. } => {
                    match self.text.measure_ink(run.id) {
                        Some(ink) => {
                            let new_ink = Rect {
                                x: at[0] + ink.x,
                                y: at[1] + ink.y,
                                w: ink.w,
                                h: ink.h,
                            };
                            let old_ink = self.text_ink.insert(run.id, new_ink);
                            self.tracker.record_text_change(old_ink, new_ink);
                        }
                        None => {
                            // Unknown run (never minted or already removed):
                            // conservative full invalidation, plus repaint
                            // any previously recorded ink for this id.
                            if let Some(old_ink) = self.text_ink.remove(&run.id) {
                                self.tracker.record_rect_change(old_ink, None);
                            }
                            self.tracker.record_change(item, None);
                        }
                    }
                    self.last_bounds.remove(&id.raw().index);
                }
                DisplayItem::PushClip { .. }
                | DisplayItem::PushOpacity(_)
                | DisplayItem::PopClip
                | DisplayItem::PushTextColorMap(_)
                | DisplayItem::PopTextColorMap
                | DisplayItem::PushContentBlur { .. }
                | DisplayItem::PopContentBlur
                | DisplayItem::PushTransform(_)
                | DisplayItem::PopTransform
                | DisplayItem::PopOpacity => {
                    // Structural change: explicit full-frame invalidation.
                    self.tracker.record_change(item, None);
                    self.last_bounds.remove(&id.raw().index);
                }
                _ => {
                    let old = self.last_bounds.get(&id.raw().index).copied();
                    match item_bounds(item) {
                        Some(new_footprint) => {
                            self.tracker.record_rect_change(new_footprint, old);
                            self.last_bounds.insert(id.raw().index, new_footprint);
                        }
                        None => self.tracker.record_change(item, old),
                    }
                }
            }
        }

        // External content invalidation: registration IS the signal. A layer
        // with new pixels dirties its last-walked bounds even when the
        // scene is unchanged (no DisplayItem edit exists for a pure content
        // swap). Unknown layers (never walked) invalidate conservatively.
        // This surfaces as damage, not node counts: content change is not
        // a scene change.
        let dirty_layers: Vec<u64> = std::mem::take(&mut self.external_dirty);
        for layer in dirty_layers {
            match self.layer_bounds.get(&layer) {
                Some(bounds) => self.tracker.record_rect_change(*bounds, None),
                None => self
                    .tracker
                    .invalidate_full(InvalidationReason::ContentChanged),
            }
        }
        // New custom passes invalidate their declared bounds once (the
        // transition, not every frame: passes persist, and the renderer
        // never owns the window clock — callers request frames on demand).
        let pass_start = self
            .custom_passes
            .len()
            .saturating_sub(self.unconsumed_passes);
        let new_pass_bounds: Vec<Rect> = self.custom_passes[pass_start..]
            .iter()
            .map(|p| p.bounds())
            .collect();
        self.unconsumed_passes = 0;
        for bounds in new_pass_bounds {
            self.tracker.record_rect_change(bounds, None);
        }

        // Fast path: plain-Rect value updates patch retained mirror slots
        // in place — O(changed), no walk, no rebuild, no clone. Eligible
        // only with built slots, unchanged structure, no removals, no
        // complex content anywhere, and every change a top-level plain Rect
        // with cached context. Anything else takes the slow path below.
        let fast = self.slots_ready
            && !structure_changed
            && removed.is_empty()
            && !self.has_complex
            && self.custom_passes.is_empty()
            && !changes.is_empty()
            && changes.iter().all(|(id, item)| {
                let idx = id.raw().index;
                matches!(item, DisplayItem::Rect { .. })
                    && matches!(self.slot_of.get(&idx), Some(slot) if *slot < self.mirror.len())
                    && matches!(self.ctx_of.get(&idx), Some(ctx) if !ctx.in_group)
            });
        if fast {
            plan.nodes_traversed = changes.len();
            plan.mirror_current = true;
            self.dirty_ranges.clear();
            let s = self.scale;
            for (id, item) in &changes {
                if let DisplayItem::Rect {
                    bounds,
                    radius,
                    fill,
                    border_width,
                    border_color,
                    shadow,
                } = item
                {
                    let idx = id.raw().index;
                    let ctx = self.ctx_of[&idx];
                    let slot = self.slot_of[&idx];
                    let mut inst = build_rect_instance(
                        bounds,
                        *radius,
                        *fill,
                        *border_width,
                        *border_color,
                        shadow,
                        (ctx.clip, ctx.clip_r),
                        s,
                    );
                    inst.misc[2] = ctx.clip_index as f32;
                    self.mirror[slot] = inst;
                    self.dirty_ranges.push((slot, slot + 1));
                    plan.ops.push(Op::Rects {
                        start: slot as u32,
                        count: 1,
                        bounds: Rect {
                            x: inst.rect[0],
                            y: inst.rect[1],
                            w: inst.rect[2],
                            h: inst.rect[3],
                        },
                    });
                }
            }
            plan.damage = self.tracker.take();
            plan.raster_area_px = damage_area(&plan.damage);
            plan.partial = self.base_valid && !plan.damage.full && !plan.damage.is_clean();
            plan.full_upload = false;
            self.diagnostics.external_layers_live = self.external.len() as u64;
            return plan;
        }

        // Walk painter order, resolving clip/opacity stacks into instances.
        // Rect runs and text runs flush as ordered ops at every composition
        // boundary (blur / group / external), preserving painter order.
        // This slow path also rebuilds slot map + clip contexts + the
        // complex-content flag for future fast paths.
        self.slot_of.clear();
        self.ctx_of.clear();
        self.has_complex = false;
        plan.nodes_traversed = scene.len();
        plan.mirror_current = false;
        let s = self.scale;
        self.clip_nodes.clear();
        self.clip_dirty = true;
        let mut clip_indices = Vec::<u32>::new();
        let mut clip_stack: Vec<(Rect, f32)> = Vec::new(); // physical px
        let mut group_stack: Vec<GroupOp> = Vec::new();
        let mut blur_regions: Vec<(Rect, Rect)> = Vec::new(); // (logical, phys)
        let mut top_start = 0u32;
        let mut pending_texts: Vec<ShapedRun> = Vec::new();
        // Layers referenced by the committed scene this walk. Unreferenced
        // entries are pruned afterwards (memory lifecycle, no per-frame
        // full scan: this IS the structural walk).
        let mut seen_layers: HashSet<u64> = HashSet::new();
        let mut transforms = vec![aqua_core::Transform2D::IDENTITY];
        let mut has_transform = false;
        let mut text_maps = Vec::<aqua_core::scene::TextColorMap>::new();

        for (id, item) in scene.ordered() {
            match item {
                DisplayItem::PushTransform(local) => {
                    has_transform = true;
                    let combined = local.then(*transforms.last().unwrap());
                    if ![
                        combined.a,
                        combined.b,
                        combined.c,
                        combined.d,
                        combined.tx,
                        combined.ty,
                    ]
                    .iter()
                    .all(|v| v.is_finite())
                        || combined.b != 0.0
                        || combined.c != 0.0
                        || combined.a != combined.d
                        || combined.a <= 0.0
                    {
                        plan.validation_error = Some("subtree transform requires finite positive uniform scale and translation");
                    }
                    transforms.push(combined);
                    continue;
                }
                DisplayItem::PopTransform => {
                    if transforms.len() == 1 {
                        plan.validation_error = Some("unbalanced transform stack");
                    } else {
                        transforms.pop();
                    }
                    continue;
                }
                _ => {}
            }
            let transform = *transforms.last().unwrap();
            let transformed = transform_item(item, transform);
            let item = &transformed;
            match item {
                DisplayItem::PushTextColorMap(map) => {
                    has_transform = true;
                    if ![map.start, map.end, map.strength]
                        .iter()
                        .all(|v| v.is_finite())
                        || map.end <= map.start
                        || !(0.0..=1.0).contains(&map.strength)
                        || !map.target.to_linear().iter().all(|v| v.is_finite())
                    {
                        plan.validation_error = Some("invalid text color mapping");
                    }
                    text_maps.push(*map);
                }
                DisplayItem::PopTextColorMap => {
                    if text_maps.pop().is_none() {
                        plan.validation_error = Some("unbalanced text color map");
                    }
                }

                DisplayItem::PushTransform(_) | DisplayItem::PopTransform => unreachable!(),
                DisplayItem::Rect {
                    bounds,
                    radius,
                    fill,
                    border_width,
                    border_color,
                    shadow,
                } => {
                    let phys = Rect {
                        x: bounds.x * s,
                        y: bounds.y * s,
                        w: bounds.w * s,
                        h: bounds.h * s,
                    };
                    let (clip, clip_r) = clip_stack.last().copied().unwrap_or((
                        Rect {
                            x: 0.0,
                            y: 0.0,
                            w: -1.0,
                            h: 0.0,
                        },
                        0.0,
                    ));
                    let (sox, soy, _, _) = shadow
                        .map(|(x, y, b, _)| (x * s, y * s, b * s, 1.0))
                        .unwrap_or((0.0, 0.0, 0.0, 0.0));
                    let mut inst = build_rect_instance(
                        bounds,
                        *radius,
                        *fill,
                        *border_width,
                        *border_color,
                        shadow,
                        (clip, clip_r),
                        s,
                    );
                    inst.misc[2] = clip_indices.last().copied().unwrap_or(0) as f32;
                    // NOTE: shader centers the shadow at instance-quad center
                    // + shadow.xy, so store the offset relative to the quad
                    // center (quad covers rect + shadow footprint).
                    {
                        let qc = [
                            inst.rect[0] + inst.rect[2] * 0.5,
                            inst.rect[1] + inst.rect[3] * 0.5,
                        ];
                        let rc = [phys.x + phys.w * 0.5, phys.y + phys.h * 0.5];
                        inst.shadow[0] = rc[0] + sox - qc[0];
                        inst.shadow[1] = rc[1] + soy - qc[1];
                    }
                    emit_rect_instance(&mut plan, &mut group_stack, s, inst);
                    // Slot map + clip context for future fast paths (only
                    // top-level rects get upload slots; grouped nodes are
                    // marked so value updates never take the fast path).
                    let idx = id.raw().index;
                    if group_stack.is_empty() {
                        self.slot_of.insert(idx, plan.instances.len() - 1);
                        self.ctx_of.insert(
                            idx,
                            NodeCtx {
                                clip,
                                clip_r,
                                clip_index: clip_indices.last().copied().unwrap_or(0),
                                in_group: false,
                            },
                        );
                    } else {
                        self.ctx_of.insert(
                            idx,
                            NodeCtx {
                                clip,
                                clip_r,
                                clip_index: clip_indices.last().copied().unwrap_or(0),
                                in_group: true,
                            },
                        );
                    }
                }
                DisplayItem::PushClip { bounds, radius } => {
                    let parent = clip_indices.last().copied().unwrap_or(0);
                    self.clip_nodes.push([
                        bounds.x * s,
                        bounds.y * s,
                        bounds.w * s,
                        bounds.h * s,
                        *radius * s,
                        parent as f32,
                        0.0,
                        0.0,
                    ]);
                    clip_indices.push(self.clip_nodes.len() as u32);
                    clip_stack.push((
                        Rect {
                            x: bounds.x * s,
                            y: bounds.y * s,
                            w: bounds.w * s,
                            h: bounds.h * s,
                        },
                        *radius * s,
                    ));
                }
                DisplayItem::PopClip => {
                    clip_indices.pop();
                    clip_stack.pop();
                }
                DisplayItem::PushOpacity(o) => {
                    group_stack.push(GroupOp {
                        text_maps: Vec::new(),
                        filter: None,
                        clips: clip_stack.clone(),
                        bounds: Rect {
                            x: 0.0,
                            y: 0.0,
                            w: 0.0,
                            h: 0.0,
                        },
                        opacity: *o,
                        instances: Vec::new(),
                        commands: Vec::new(),
                    });
                }
                DisplayItem::PushContentBlur { bounds, radius } => {
                    if !radius.is_finite() || *radius < 0.0 {
                        plan.validation_error =
                            Some("content blur requires finite nonnegative radius");
                    }
                    group_stack.push(GroupOp {
                        text_maps: Vec::new(),
                        filter: Some((*bounds, *radius)),
                        clips: clip_stack.clone(),
                        bounds: *bounds,
                        opacity: 1.0,
                        instances: Vec::new(),
                        commands: Vec::new(),
                    });
                }
                DisplayItem::PopOpacity | DisplayItem::PopContentBlur => {
                    if let Some(g) = group_stack.pop() {
                        // True group isolation: bounds in physical px.
                        if matches!(item, DisplayItem::PopContentBlur) != g.filter.is_some() {
                            plan.validation_error = Some("mismatched content group stack");
                        }
                        let logical = match g.filter {
                            Some((bounds, radius)) => crate::scene::inflate(
                                bounds,
                                BlurParams::for_radius(radius * s).padding() / s,
                            ),
                            None => g.bounds,
                        };
                        let phys = Rect {
                            x: logical.x * s,
                            y: logical.y * s,
                            w: logical.w * s,
                            h: logical.h * s,
                        };
                        let op = Op::Group(GroupOp { bounds: phys, ..g });
                        if let Some(parent) = group_stack.last_mut() {
                            let bounds = Rect {
                                x: phys.x / s,
                                y: phys.y / s,
                                w: phys.w / s,
                                h: phys.h / s,
                            };
                            parent.bounds = match parent.commands.is_empty() {
                                true => bounds,
                                false => parent.bounds.union(bounds),
                            };
                            if let Op::Group(inner) = op {
                                parent.commands.push(GroupCommand::Group(inner));
                            }
                        } else {
                            // Close pending top-level runs first (order!).
                            flush_rect_run(&mut plan.ops, &plan.instances, &mut top_start);
                            flush_texts(&mut plan.ops, &mut pending_texts);
                            plan.ops.push(op);
                            top_start = plan.instances.len() as u32;
                        }
                    }
                }
                DisplayItem::BackdropBlur { bounds, radius } => {
                    flush_rect_run(&mut plan.ops, &plan.instances, &mut top_start);
                    flush_texts(&mut plan.ops, &mut pending_texts);
                    let params = BlurParams::for_radius(*radius * s);
                    let pad = params.padding();
                    let phys = crate::scene::inflate(
                        Rect {
                            x: bounds.x * s,
                            y: bounds.y * s,
                            w: bounds.w * s,
                            h: bounds.h * s,
                        },
                        pad,
                    );
                    blur_regions.push((*bounds, phys));
                    plan.ops.push(Op::Blur {
                        bounds: phys,
                        params,
                    });
                    top_start = plan.instances.len() as u32;
                }
                DisplayItem::Text {
                    at,
                    run,
                    color,
                    opacity,
                } => {
                    // Painter-order clip in logical px (clip_stack is
                    // physical). w < 0 means "no clip" by stack convention.
                    let clip_logical = clip_stack.last().and_then(|(c, _)| {
                        if c.w < 0.0 {
                            None
                        } else {
                            Some(Rect {
                                x: c.x / s,
                                y: c.y / s,
                                w: c.w / s,
                                h: c.h / s,
                            })
                        }
                    });
                    let text_opacity = (color.alpha * opacity).clamp(0.0, 1.0);
                    let mut shaped =
                        resolve_scene_text(run, *at, color.with_alpha(1.0), 1.0, clip_logical, s);
                    shaped.scale = transform.a;
                    let bounds = Rect {
                        x: 0.0,
                        y: 0.0,
                        w: self.width as f32,
                        h: self.height as f32,
                    };
                    let isolated = match text_opacity < 1.0
                        || !text_maps.is_empty()
                        || clip_stack.len() > 1
                        || clip_stack.iter().any(|(_, radius)| *radius > 0.0)
                    {
                        true => Some(GroupOp {
                            text_maps: text_maps.clone(),
                            filter: None,
                            clips: clip_stack.clone(),
                            bounds,
                            opacity: text_opacity,
                            instances: Vec::new(),
                            commands: vec![GroupCommand::Text(shaped.clone())],
                        }),
                        false => None,
                    };
                    if let Some(g) = group_stack.last_mut() {
                        let logical = Rect {
                            x: 0.0,
                            y: 0.0,
                            w: bounds.w / s,
                            h: bounds.h / s,
                        };
                        g.bounds = match g.commands.is_empty() {
                            true => logical,
                            false => g.bounds.union(logical),
                        };
                        g.commands.push(match isolated {
                            Some(group) => GroupCommand::Group(group),
                            None => GroupCommand::Text(shaped),
                        });
                    } else {
                        match isolated {
                            Some(group) => {
                                flush_rect_run(&mut plan.ops, &plan.instances, &mut top_start);
                                flush_texts(&mut plan.ops, &mut pending_texts);
                                plan.ops.push(Op::Group(group));
                            }
                            None => pending_texts.push(shaped),
                        }
                    }
                }
                DisplayItem::Image {
                    bounds,
                    layer,
                    fit,
                    position,
                    source,
                    opacity,
                    rounded_clip,
                } => {
                    self.layer_bounds.insert(*layer, *bounds);
                    seen_layers.insert(*layer);
                    let physical = Rect {
                        x: bounds.x * s,
                        y: bounds.y * s,
                        w: bounds.w * s,
                        h: bounds.h * s,
                    };
                    let mut clips = clip_stack.clone();
                    clips.push((physical, *rounded_clip * s));
                    let op = SurfaceOp {
                        pixel_scale: s * transform.a,
                        bounds: physical,
                        source: SurfaceSource::External(*layer),
                        opacity: *opacity,
                        clips,
                        placement: Some((*fit, *position, *source)),
                    };
                    emit_surface(
                        &mut plan,
                        &mut group_stack,
                        op,
                        s,
                        &mut top_start,
                        &mut pending_texts,
                    );
                }
                DisplayItem::Vector {
                    bounds,
                    source,
                    opacity,
                } => {
                    let op = SurfaceOp {
                        pixel_scale: s * transform.a,
                        bounds: Rect {
                            x: bounds.x * s,
                            y: bounds.y * s,
                            w: bounds.w * s,
                            h: bounds.h * s,
                        },
                        source: SurfaceSource::Vector(source.clone()),
                        opacity: *opacity,
                        clips: clip_stack.clone(),
                        placement: None,
                    };
                    emit_surface(
                        &mut plan,
                        &mut group_stack,
                        op,
                        s,
                        &mut top_start,
                        &mut pending_texts,
                    );
                }
                DisplayItem::External {
                    bounds,
                    layer,
                    opacity,
                    rounded_clip,
                } => {
                    self.layer_bounds.insert(*layer, *bounds);
                    seen_layers.insert(*layer);
                    let physical = Rect {
                        x: bounds.x * s,
                        y: bounds.y * s,
                        w: bounds.w * s,
                        h: bounds.h * s,
                    };
                    let mut clips = clip_stack.clone();
                    if *rounded_clip > 0.0 {
                        clips.push((physical, *rounded_clip * s));
                    }
                    let op = SurfaceOp {
                        pixel_scale: s * transform.a,
                        bounds: physical,
                        source: SurfaceSource::External(*layer),
                        opacity: *opacity,
                        clips,
                        placement: None,
                    };
                    emit_surface(
                        &mut plan,
                        &mut group_stack,
                        op,
                        s,
                        &mut top_start,
                        &mut pending_texts,
                    );
                }
            }
        }
        flush_rect_run(&mut plan.ops, &plan.instances, &mut top_start);
        flush_texts(&mut plan.ops, &mut pending_texts);
        self.slots_ready = true;
        self.dirty_ranges.clear();
        // Memory lifecycle: retain only layers referenced by the committed
        // scene; release views/dims/bounds/pending-dirty for the rest.
        // Runs on structural walks only (removals take the slow path), so
        // no per-frame full scan. A layer registered without a scene
        // reference in the same commit cycle is dropped: owners feed
        // per-frame and re-register.
        self.external.retain(|k, _| seen_layers.contains(k));
        self.external_size.retain(|k, _| seen_layers.contains(k));
        self.layer_bounds.retain(|k, _| seen_layers.contains(k));
        self.external_dirty.retain(|l| seen_layers.contains(l));
        self.diagnostics.external_layers_live = self.external.len() as u64;
        // Complex content anywhere forces full-repaint fallback (correct,
        // documented): fast value updates and scissored partial repaint
        // cover plain-rect scenes only.
        self.has_complex =
            has_transform || plan.ops.iter().any(|op| !matches!(op, Op::Rects { .. }));
        if !text_maps.is_empty() {
            plan.validation_error = Some("unbalanced text color map");
        }
        if !group_stack.is_empty() {
            plan.validation_error = Some("unbalanced content group stack");
        }
        if transforms.len() != 1 {
            plan.validation_error = Some("unbalanced transform stack");
        }

        // Backdrop dependency: any blur whose region overlaps accumulated
        // damage recomputes, even with unchanged params. Checked BEFORE take.
        for (logical, phys) in &blur_regions {
            if self.tracker.damage_overlaps(*phys) {
                self.tracker.record_backdrop_dependency(*logical);
            }
        }
        plan.damage = self.tracker.take();
        plan.raster_area_px = damage_area(&plan.damage);
        plan.partial = self.base_valid
            && !plan.damage.full
            && !plan.damage.is_clean()
            && !self.has_complex
            && self.custom_passes.is_empty();

        // Diff instance mirror -> bounded upload ranges.
        plan.full_upload = self.mirror.len() != plan.instances.len();
        plan
    }

    // -- draw ------------------------------------------------------------------

    /// Encode and submit the plan into the offscreen base texture.
    /// Text renders into the SAME open encoder (text-agent protocol), so
    /// painter order holds without submit boundaries.
    pub fn draw(&mut self, plan: &FramePlan) -> Result<(), RenderError> {
        if let Some(error) = plan.validation_error {
            return Err(RenderError::Gpu(error.into()));
        }
        let t0 = std::time::Instant::now();
        let (w, h) = (self.width, self.height);
        // Clone the base view up front: draw helpers take &mut self, so no
        // borrow of self.base may be live across those calls.
        let base_view = self
            .base
            .as_ref()
            .map(|(_, v)| v.clone())
            .ok_or_else(|| RenderError::Gpu("no base target; call set_size".into()))?;

        // Clean plan over a valid base: no work at all. (The facade never
        // calls draw on clean plans; direct calls are explicit no-ops.)
        if plan.is_clean() && self.base_valid {
            return Ok(());
        }

        self.text.begin_frame();
        self.upload_clip_nodes()?;
        if plan.mirror_current {
            // Fast path: write dirty slot ranges from the retained mirror.
            self.upload_dirty_ranges()?;
        } else {
            self.upload_instances(&plan.instances, plan.full_upload)?;
        }

        // Base clear: transparent black (native-material windows composite).
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("aqua-frame"),
            });
        // wgpu Device/Queue are cheap Arc clones; cloning avoids borrow
        // conflicts between the op list walk and &mut self methods.
        let device = self.device.clone();
        let queue = self.queue.clone();
        let mut passes: u32;
        if plan.partial && self.base_valid {
            // Partial repaint over the retained base (scissored regions).
            passes = self.partial_repaint(&mut encoder, &base_view, plan)?;
        } else {
            passes = 1;
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("aqua-clear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &base_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.rect_pipeline);
                pass.set_bind_group(0, &self.rect_uniform_bind, &[]);
                pass.set_vertex_buffer(0, self.quad_vb.slice(..));
                pass.set_index_buffer(self.quad_ib.slice(..), wgpu::IndexFormat::Uint32);
                // Zero-instance no-op draw keeps the clear valid for empty scenes.
                pass.set_vertex_buffer(1, self.instance_buf.slice(..));
                pass.draw_indexed(0..6, 0, 0..0);
            }
            for op in &plan.ops {
                match op {
                    Op::Rects { start, count, .. } => {
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("aqua-rects"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &base_view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Load,
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                            multiview_mask: None,
                        });
                        pass.set_pipeline(&self.rect_pipeline);
                        pass.set_bind_group(0, &self.rect_uniform_bind, &[]);
                        pass.set_vertex_buffer(0, self.quad_vb.slice(..));
                        pass.set_index_buffer(self.quad_ib.slice(..), wgpu::IndexFormat::Uint32);
                        pass.set_vertex_buffer(1, self.instance_buf.slice(..));
                        pass.draw_indexed(0..6, 0, *start..(*start + *count));
                        passes += 1;
                    }
                    Op::Blur { bounds, params } => {
                        self.apply_blur(
                            &device,
                            &queue,
                            &mut encoder,
                            &base_view,
                            *bounds,
                            (*params, false),
                        )?;
                        passes += 3; // H + V + composite
                    }
                    Op::Group(g) => {
                        passes += self.draw_group(&device, &queue, &mut encoder, &base_view, g)?;
                    }
                    Op::Surface(surface) => {
                        passes += self.draw_surface(&device, &mut encoder, &base_view, surface)?;
                    }
                    Op::Texts(runs) => {
                        for run in runs.clone() {
                            self.text.queue_run(run);
                        }
                        let stats = self.text.encode_queued(TextFrame {
                            device: &device,
                            queue: &queue,
                            encoder: &mut encoder,
                            target: &base_view,
                            width: self.width,
                            height: self.height,
                        });
                        if stats.raster_failures != 0 {
                            return Err(RenderError::Gpu("text rasterization failed".into()));
                        }
                        self.diagnostics.text_areas_last = stats.areas as u64;
                        self.diagnostics.text_stale_last = stats.stale_skipped;
                        self.diagnostics.text_mismatch_last = stats.revision_mismatches;
                        // stats.atlas_upload_bytes is None by design (unknown):
                        // never added to byte totals, never fabricated.
                        passes += 1;
                    }
                }
            }

            // Custom passes run after built-ins, in registration order.
            // Backdrop readers get a real snapshot copy; writers target base.
            // A pass failure aborts the frame with Err (nothing submitted yet):
            // remaining passes stay registered, snapshots release, the retained
            // base keeps last good pixels. Never silently ignored.
            let mut snapshots: Vec<usize> = Vec::new();
            let mut taken = std::mem::take(&mut self.custom_passes);
            let mut pass_err = None;
            while !taken.is_empty() {
                let pass = taken.remove(0);
                let source_view = if pass.reads_backdrop() {
                    let idx = self.pool.acquire(&device, w, h, self.format)?;
                    let snap_tex = self.pool.entries[idx].texture.clone();
                    let base_tex = self.base.as_ref().unwrap().0.clone();
                    encoder.copy_texture_to_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &base_tex,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::TexelCopyTextureInfo {
                            texture: &snap_tex,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::Extent3d {
                            width: w,
                            height: h,
                            depth_or_array_layers: 1,
                        },
                    );
                    snapshots.push(idx);
                    self.pool.entries[idx].view.clone()
                } else {
                    base_view.clone()
                };
                let r = pass.encode(&mut encoder, &source_view, &base_view, &device, &queue);
                // Damage contract: custom passes declare effect bounds; the
                // facade must invalidate them (see CustomPass::bounds).
                self.custom_passes.push(pass);
                passes += 1;
                if let Err(e) = r {
                    pass_err = Some(e);
                    break;
                }
            }
            self.custom_passes.extend(taken);
            for idx in snapshots {
                self.pool.release(idx);
            }
            if let Some(e) = pass_err {
                return Err(e);
            }
        } // end else (full repaint)

        self.queue.submit([encoder.finish()]);
        // Atlas trim only after the final submit, never between encode and
        // submit (text-agent protocol).
        self.text.trim_atlas();

        // Any completed draw leaves a valid retained base (full repaint by
        // construction; partial repaint preserves the rest).
        self.base_valid = true;
        self.diagnostics.frames_produced += 1;
        self.diagnostics.nodes_total_last = plan.nodes_total;
        self.diagnostics.nodes_updated_last = plan.nodes_updated;
        self.diagnostics.passes_last = passes;
        self.diagnostics.intermediate_bytes_last = self.pool.used_bytes();
        self.diagnostics.pool_hits = self.pool.hits;
        self.diagnostics.pool_misses = self.pool.misses;
        self.diagnostics.pool_evictions = self.pool.evictions;
        self.diagnostics.last_frame_us = t0.elapsed().as_micros() as u64;
        Ok(())
    }

    /// Present a surface frame (wgpu 30 presents via the queue).
    pub fn present(&self, frame: wgpu::SurfaceTexture) {
        self.queue.present(frame);
    }

    /// Record a REAL presentation (facade path only). `draw` increments
    /// `frames_produced` (includes offscreen/readback work);
    /// `frames_presented` counts surfaces actually presented.
    pub fn note_presented(&mut self) {
        self.diagnostics.frames_presented += 1;
    }

    pub fn start_snapshot_transition(
        &mut self,
        settings: crate::effects::SnapshotTransition,
    ) -> Result<(), RenderError> {
        if settings.duration.is_zero()
            || !settings.band.is_finite()
            || settings.band <= 0.0
            || settings.band > 1.0
            || !settings
                .shimmer
                .to_linear()
                .iter()
                .all(|value| value.is_finite())
        {
            return Err(RenderError::Gpu("invalid snapshot transition".into()));
        }
        if !self.base_valid {
            return Err(RenderError::Gpu(
                "snapshot transition requires a rendered frame".into(),
            ));
        }
        self.cancel_snapshot_transition();
        let previous = self
            .pool
            .acquire(&self.device, self.width, self.height, self.format)?;
        let output = match self
            .pool
            .acquire(&self.device, self.width, self.height, self.format)
        {
            Ok(index) => index,
            Err(error) => {
                self.pool.release(previous);
                return Err(error);
            }
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("aqua-transition-snapshot"),
            });
        encoder.copy_texture_to_texture(
            self.base.as_ref().unwrap().0.as_image_copy(),
            self.pool.entries[previous].texture.as_image_copy(),
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        self.snapshot = Some(SnapshotState {
            previous,
            output,
            settings,
            elapsed: std::time::Duration::ZERO,
        });
        Ok(())
    }

    pub fn cancel_snapshot_transition(&mut self) {
        if let Some(snapshot) = self.snapshot.take() {
            self.pool.release(snapshot.previous);
            self.pool.release(snapshot.output);
        }
    }

    pub fn set_snapshot_transition_elapsed(&mut self, elapsed: std::time::Duration) -> bool {
        let complete = self.snapshot.as_mut().is_some_and(|snapshot| {
            snapshot.elapsed = elapsed;
            elapsed >= snapshot.settings.duration
        });
        if complete {
            self.cancel_snapshot_transition();
        }
        self.snapshot.is_some()
    }

    pub fn snapshot_transition_active(&self) -> bool {
        self.snapshot.is_some()
    }

    fn composed_presentation(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let Some(snapshot) = &self.snapshot else {
            return self.base.as_ref().unwrap().clone();
        };
        let previous = &self.pool.entries[snapshot.previous];
        let output = &self.pool.entries[snapshot.output];
        let base = &self.base.as_ref().unwrap().1;
        self.effect_compositor.encode(
            &self.device,
            encoder,
            [base, &previous.view, &output.view],
            crate::effects::compositor::EffectUniforms {
                viewport: [self.width as f32, self.height as f32],
                progress: (snapshot.elapsed.as_secs_f64()
                    / snapshot.settings.duration.as_secs_f64()) as f32,
                band: snapshot.settings.band,
                target: snapshot.settings.shimmer.to_linear(),
                mode: 0,
                padding: [0; 3],
            },
        );
        (output.texture.clone(), output.view.clone())
    }

    pub fn blit_to(
        &mut self,
        target: &wgpu::TextureView,
        target_format: wgpu::TextureFormat,
        output_gamma: f32,
    ) -> Result<(), RenderError> {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("aqua-blit"),
            });
        let (_, base_view) = self.composed_presentation(&mut encoder);
        let device = self.device.clone();
        self.blit(
            &device,
            &mut encoder,
            &base_view,
            target,
            target_format,
            Rect {
                x: 0.0,
                y: 0.0,
                w: self.width as f32,
                h: self.height as f32,
            },
            full_uv(),
            1.0,
            [0.0, 0.0, -1.0, 0.0],
            0.0,
            output_gamma,
            1.0, // base working target is premultiplied
        )?;
        self.queue.submit([encoder.finish()]);
        Ok(())
    }

    /// Offscreen readback: Rgba8Unorm bytes, row-major, no padding.
    /// Must be called after `draw`; returns a full-frame copy.
    pub fn readback(&mut self) -> Result<Vec<u8>, RenderError> {
        let (w, h) = (self.width, self.height);
        let padded_bpr = align256(w * 4);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aqua-readback"),
            size: (padded_bpr * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("aqua-copy"),
            });
        let (base, _) = self.composed_presentation(&mut encoder);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &base,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bpr),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RenderError::Gpu(format!("{e:?}")))?;
        rx.recv()
            .map_err(|_| RenderError::Gpu("readback channel closed".into()))?
            .map_err(|e| RenderError::Gpu(format!("{e:?}")))?;
        // Copy out and strip per-row padding while mapped.
        let out = {
            let mapped = slice
                .get_mapped_range()
                .map_err(|e| RenderError::Gpu(format!("{e:?}")))?;
            let row_bytes = (w * 4) as usize;
            let stride = padded_bpr as usize;
            let mut out = vec![0u8; (w * h * 4) as usize];
            for y in 0..h as usize {
                out[y * row_bytes..(y + 1) * row_bytes]
                    .copy_from_slice(&mapped[y * stride..y * stride + row_bytes]);
            }
            out
        };
        buffer.unmap();
        Ok(out)
    }

    // -- internals -------------------------------------------------------------

    fn ensure_instance_capacity(&mut self, need: usize) -> Result<(), RenderError> {
        let need = need.max(1);
        if need <= self.instance_capacity {
            return Ok(());
        }
        let mut cap = self.instance_capacity;
        while cap < need {
            cap *= 2;
        }
        // Growth budget: refuse absurd scenes instead of OOM.
        if cap > 1_000_000 {
            return Err(RenderError::BudgetExceeded("instance buffer growth cap"));
        }
        self.instance_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aqua-instances"),
            size: (cap * std::mem::size_of::<RectInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_capacity = cap;
        self.mirror.clear(); // force full re-upload
        Ok(())
    }

    /// Fast-path upload: write dirty slot ranges from the retained mirror.
    /// No diff, no clone. Byte-exact for diagnostics.
    fn upload_dirty_ranges(&mut self) -> Result<(), RenderError> {
        if self.mirror.len() > self.instance_capacity {
            // Must never happen on the fast path (no inserts); fail
            // explicitly rather than corrupt slot mapping.
            return Err(RenderError::BudgetExceeded(
                "fast-path mirror exceeds capacity",
            ));
        }
        let mut ranges = std::mem::take(&mut self.dirty_ranges);
        coalesce_ranges(&mut ranges);
        let stride = std::mem::size_of::<RectInstance>() as u64;
        let mut bytes = 0u64;
        if ranges.len() > MAX_UPLOAD_RANGES {
            self.queue
                .write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&self.mirror));
            bytes = (self.mirror.len() * 128) as u64;
        } else {
            for (s, e) in &ranges {
                self.queue.write_buffer(
                    &self.instance_buf,
                    (*s as u64) * stride,
                    bytemuck::cast_slice(&self.mirror[*s..*e]),
                );
                bytes += ((*e - *s) * 128) as u64;
            }
        }
        self.diagnostics.bytes_uploaded_last = bytes;
        self.diagnostics.bytes_uploaded_total += bytes;
        Ok(())
    }

    /// Partial repaint over the retained base: for each damage region, clear
    /// it (REPLACE) and redraw intersecting content in painter order with a
    /// scissor. Returns passes encoded.
    ///
    /// Per-pixel correctness: a pixel's result depends only on items
    /// covering it; effect-inclusive damage guarantees every contributor
    /// intersects the region and redraws. Overlapping transparency is
    /// correct through full painter-order redraw over transparent clear.
    /// Only plain-rect scenes reach here (prepare falls back to full for
    /// blur/groups/external/text/custom effects).
    ///
    /// Draw scope honesty: slow-path plans carry complete painter-ordered
    /// runs, so regions redraw only intersecting runs. Fast-path plans carry
    /// changed slots only (unchanged contributors live in the mirror but
    /// have no ops), so regions redraw the full instance range scissored —
    /// vertex work is O(total), fragment/raster work stays O(region), CPU
    /// traversal stays O(changed). All three are reported (traversal,
    /// upload bytes, raster area); no claim beyond them.
    fn partial_repaint(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        base_view: &wgpu::TextureView,
        plan: &FramePlan,
    ) -> Result<u32, RenderError> {
        let (w, h) = (self.width, self.height);
        let device = self.device.clone();
        let mut passes = 0u32;
        // Fast path: ops are changed-slots only; redraw the whole retained
        // range scissored (contributors included by construction).
        let full_range = plan.mirror_current;
        let mirror_len = self.mirror.len() as u32;
        for region in &plan.damage.damage {
            let Some((sx, sy, sw, sh)) = clamp_scissor(region, w, h) else {
                continue;
            };
            // 1. REPLACE-clear the region to transparent.
            let clear_inst = RectInstance {
                rect: [sx as f32, sy as f32, sw as f32, sh as f32],
                rb_pack: [0.0; 4],
                fill: [0.0; 4],
                border: [0.0; 4],
                shadow: [0.0; 4],
                shadow_color: [0.0; 4],
                clip: [0.0, 0.0, -1.0, 0.0],
                misc: [0.0; 4],
            };
            let clear_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("aqua-region-clear"),
                size: std::mem::size_of::<RectInstance>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue
                .write_buffer(&clear_buf, 0, bytemuck::cast_slice(&[clear_inst]));
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("aqua-region-clear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: base_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.clear_pipeline);
                pass.set_bind_group(0, &self.rect_uniform_bind, &[]);
                pass.set_vertex_buffer(0, self.quad_vb.slice(..));
                pass.set_index_buffer(self.quad_ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.set_vertex_buffer(1, clear_buf.slice(..));
                pass.set_scissor_rect(sx, sy, sw, sh);
                pass.draw_indexed(0..6, 0, 0..1);
            }
            passes += 1;
            // 2. Redraw intersecting runs in painter order (scissored).
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("aqua-region-redraw"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: base_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.rect_pipeline);
                pass.set_bind_group(0, &self.rect_uniform_bind, &[]);
                pass.set_vertex_buffer(0, self.quad_vb.slice(..));
                pass.set_index_buffer(self.quad_ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.set_vertex_buffer(1, self.instance_buf.slice(..));
                pass.set_scissor_rect(sx, sy, sw, sh);
                for op in &plan.ops {
                    // Partial path carries Rects ops only (prepare
                    // guarantees fallback-full for everything else).
                    if full_range {
                        break;
                    }
                    if let Op::Rects {
                        start,
                        count,
                        bounds,
                    } = op
                    {
                        if rects_overlap(*bounds, *region) {
                            pass.draw_indexed(0..6, 0, *start..(*start + *count));
                        }
                    }
                }
                if full_range {
                    // Fast-path ops cover changed slots only; unchanged
                    // contributors live in the mirror without ops, so draw
                    // the retained range scissored (vertex O(total),
                    // fragment O(region), traversal O(changed)).
                    pass.draw_indexed(0..6, 0, 0..mirror_len);
                }
            }
            passes += 1;
        }
        Ok(passes)
    }

    fn upload_instances(
        &mut self,
        instances: &[RectInstance],
        full: bool,
    ) -> Result<(), RenderError> {
        self.ensure_instance_capacity(instances.len())?;
        let stride = std::mem::size_of::<RectInstance>() as u64;
        if self.mirror.len() != instances.len() {
            // Length changed: full upload.
            self.queue
                .write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(instances));
            self.diagnostics.bytes_uploaded_last = (instances.len() * 128) as u64;
            self.mirror = instances.to_vec();
        } else if full {
            self.queue
                .write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(instances));
            self.diagnostics.bytes_uploaded_last = (instances.len() * 128) as u64;
            self.mirror = instances.to_vec();
        } else {
            // Range-diff: coalesce changed slots into <= MAX ranges.
            let mut ranges: Vec<(usize, usize)> = Vec::new();
            for (i, (old, new)) in self.mirror.iter().zip(instances.iter()).enumerate() {
                if old != new {
                    match ranges.last_mut() {
                        Some((_, end)) if *end == i => *end = i + 1,
                        _ => ranges.push((i, i + 1)),
                    }
                }
            }
            let mut bytes = 0u64;
            if ranges.len() > MAX_UPLOAD_RANGES {
                self.queue
                    .write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(instances));
                bytes = (instances.len() * 128) as u64;
            } else {
                for (s, e) in &ranges {
                    self.queue.write_buffer(
                        &self.instance_buf,
                        (*s as u64) * stride,
                        bytemuck::cast_slice(&instances[*s..*e]),
                    );
                    bytes += ((*e - *s) * 128) as u64;
                }
            }
            self.diagnostics.bytes_uploaded_last = bytes;
            self.mirror = instances.to_vec();
        }
        self.diagnostics.bytes_uploaded_total += self.diagnostics.bytes_uploaded_last;
        Ok(())
    }

    fn bind_for(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniform: &wgpu::Buffer,
        view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("aqua-fx-bind"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// Real separable Gaussian backdrop blur over `bounds` (physical px):
    /// base -> scratchH -> scratchV -> composite back onto base.
    fn apply_blur(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        base_view: &wgpu::TextureView,
        bounds: Rect,
        filter: (BlurParams, bool),
    ) -> Result<(), RenderError> {
        let (params, replace) = filter;
        let (w, h) = (self.width, self.height);
        let ds = params.downsample.max(1);
        let (sw, sh) = ((w / ds).max(1), (h / ds).max(1));
        if params.radius_px < 0.5 {
            return Ok(()); // no-op radius: skip passes entirely
        }
        // Working-res kernel; the H pass samples full-res base, so its
        // kernel scales up by ds for the same physical sigma.
        let sigma = params.sigma_working();
        let step = params.step;
        let sigma_h = sigma * ds as f32;
        let step_h = step * ds as f32;
        let ia = self.pool.acquire(device, sw, sh, self.format)?;
        let ib = self.pool.acquire(device, sw, sh, self.format)?;

        let uniform_h = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aqua-blur-u"),
            size: std::mem::size_of::<BlurUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_v = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aqua-blur-u"),
            size: std::mem::size_of::<BlurUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &uniform_h,
            0,
            bytemuck::cast_slice(&[BlurUniforms {
                texel: [1.0 / w as f32, 1.0 / h as f32],
                direction: [1.0, 0.0],
                sigma: sigma_h,
                step: step_h,
                _pad: [0.0; 2],
            }]),
        );
        queue.write_buffer(
            &uniform_v,
            0,
            bytemuck::cast_slice(&[BlurUniforms {
                texel: [1.0 / sw as f32, 1.0 / sh as f32],
                direction: [0.0, 1.0],
                sigma,
                step,
                _pad: [0.0; 2],
            }]),
        );

        // Pass H: base -> A. Pass V: A -> B.
        for (src, dst_idx, uniform) in [
            (base_view.clone(), ia, &uniform_h),
            (self.pool.entries[ia].view.clone(), ib, &uniform_v),
        ] {
            let bind = self.bind_for(device, &self.blur_bind_layout, uniform, &src);
            let dst = self.pool.entries[dst_idx].view.clone();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("aqua-blur-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &dst,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.blur_pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }

        if replace {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("aqua-filter-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: base_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        let src_view = self.pool.entries[ib].view.clone();
        // uv subrect of the scratch corresponding to bounds (clamped).
        let bx0 = (bounds.x / w as f32).clamp(0.0, 1.0);
        let by0 = (bounds.y / h as f32).clamp(0.0, 1.0);
        let bx1 = ((bounds.x + bounds.w) / w as f32).clamp(0.0, 1.0);
        let by1 = ((bounds.y + bounds.h) / h as f32).clamp(0.0, 1.0);
        self.blit(
            device,
            encoder,
            &src_view,
            base_view,
            self.format,
            Rect {
                x: bx0 * w as f32,
                y: by0 * h as f32,
                w: (bx1 - bx0) * w as f32,
                h: (by1 - by0) * h as f32,
            },
            [bx0, by0, bx1 - bx0, by1 - by0],
            1.0,
            [0.0, 0.0, -1.0, 0.0],
            0.0,
            0.0,
            1.0, // blur scratch is premultiplied working space
        )?;

        self.pool.release(ia);
        self.pool.release(ib);
        Ok(())
    }

    fn upload_clip_nodes(&mut self) -> Result<(), RenderError> {
        if !self.clip_dirty {
            return Ok(());
        }
        let count = self.clip_nodes.len().max(1);
        let bytes = (count * 32) as u64;
        if bytes > self.device.limits().max_storage_buffer_binding_size {
            return Err(RenderError::BudgetExceeded("rectangle clip tree"));
        }
        if count > self.clip_capacity {
            self.clip_capacity = count;
            self.clip_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("aqua-clip-tree"),
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.rect_uniform_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("aqua-rect-bind"),
                layout: &self.rect_bind_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.rect_uniform_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.clip_buffer.as_entire_binding(),
                    },
                ],
            });
        }
        match self.clip_nodes.is_empty() {
            true => {
                self.queue
                    .write_buffer(&self.clip_buffer, 0, bytemuck::cast_slice(&[[0.0f32; 8]]))
            }
            false => self.queue.write_buffer(
                &self.clip_buffer,
                0,
                bytemuck::cast_slice(&self.clip_nodes),
            ),
        }
        self.clip_dirty = false;
        Ok(())
    }

    fn draw_surface(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        op: &SurfaceOp,
    ) -> Result<u32, RenderError> {
        let bounds = op.bounds;
        if ![bounds.x, bounds.y, bounds.w, bounds.h, op.opacity]
            .iter()
            .all(|value| value.is_finite())
        {
            return Err(RenderError::Vector(
                crate::vector::VectorError::InvalidGeometry,
            ));
        }
        if bounds.is_empty()
            || bounds
                .intersect(Rect {
                    x: 0.0,
                    y: 0.0,
                    w: self.width as f32,
                    h: self.height as f32,
                })
                .is_empty()
        {
            return Ok(0);
        }
        let view = match &op.source {
            SurfaceSource::External(layer) => match self.external.get(layer).cloned() {
                Some(view) => view,
                None => return Ok(0),
            },
            SurfaceSource::Vector(source) => self
                .vectors
                .view(
                    device,
                    &self.queue,
                    source,
                    bounds.w.ceil() as u32,
                    bounds.h.ceil() as u32,
                )
                .map_err(RenderError::Vector)?,
        };
        let placement = match (&op.source, op.placement) {
            (SurfaceSource::External(layer), Some((fit, position, source))) => {
                let (width, height) = self
                    .external_size
                    .get(layer)
                    .copied()
                    .ok_or_else(|| RenderError::Gpu("image dimensions missing".into()))?;
                aqua_core::image::ImagePlacement::resolve(
                    bounds,
                    aqua_core::Size::new(
                        width as f32 * op.pixel_scale,
                        height as f32 * op.pixel_scale,
                    ),
                    fit,
                    position,
                    source,
                )
                .map_err(|error| RenderError::Gpu(format!("image fit: {error:?}")))?
            }
            _ => aqua_core::image::ImagePlacement {
                destination: bounds,
                source: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                },
            },
        };
        let uv = placement.source;
        self.blit_clipped(
            device,
            encoder,
            &view,
            target,
            self.format,
            placement.destination,
            [uv.x, uv.y, uv.w, uv.h],
            op.opacity,
            [0.0, 0.0, -1.0, 0.0],
            0.0,
            0.0,
            1.0,
            &op.clips,
        )?;
        Ok(1)
    }

    fn draw_group(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        base_view: &wgpu::TextureView,
        group: &GroupOp,
    ) -> Result<u32, RenderError> {
        let (w, h) = (self.width, self.height);
        let scratch = self.pool.acquire(device, w, h, self.format)?;
        let view = self.pool.entries[scratch].view.clone();
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aqua-group-instances"),
            size: ((group.instances.len().max(1)) * std::mem::size_of::<RectInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&buf, 0, bytemuck::cast_slice(&group.instances));
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("aqua-group-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        let mut passes = 2;
        let result = (|| {
            for command in &group.commands {
                match command {
                    GroupCommand::Rects { start, count } => {
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("aqua-group-rects"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Load,
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                            multiview_mask: None,
                        });
                        pass.set_pipeline(&self.rect_pipeline);
                        pass.set_bind_group(0, &self.rect_uniform_bind, &[]);
                        pass.set_vertex_buffer(0, self.quad_vb.slice(..));
                        pass.set_index_buffer(self.quad_ib.slice(..), wgpu::IndexFormat::Uint32);
                        pass.set_vertex_buffer(1, buf.slice(..));
                        pass.draw_indexed(0..6, 0, *start..(*start + *count));
                        passes += 1;
                    }
                    GroupCommand::Text(run) => {
                        self.text.queue_run(run.clone());
                        let stats = self.text.encode_queued(TextFrame {
                            device,
                            queue,
                            encoder,
                            target: &view,
                            width: w,
                            height: h,
                        });
                        if stats.raster_failures != 0 {
                            return Err(RenderError::Gpu("text rasterization failed".into()));
                        }
                        passes += u32::from(stats.areas > 0);
                        self.diagnostics.text_areas_last += stats.areas as u64;
                        self.diagnostics.text_stale_last += stats.stale_skipped;
                        self.diagnostics.text_mismatch_last += stats.revision_mismatches;
                    }
                    GroupCommand::Group(inner) => {
                        passes += self.draw_group(device, queue, encoder, &view, inner)?;
                    }
                    GroupCommand::Surface(surface) => {
                        passes += self.draw_surface(device, encoder, &view, surface)?;
                    }
                }
            }
            Ok::<(), RenderError>(())
        })();
        if let Err(error) = result {
            self.pool.release(scratch);
            return Err(error);
        }
        let mut mapped_scratch = None;
        let mut filtered_view = view.clone();
        for map in &group.text_maps {
            let output = match self.pool.acquire(device, w, h, self.format) {
                Ok(index) => index,
                Err(error) => {
                    if let Some(index) = mapped_scratch {
                        self.pool.release(index);
                    }
                    self.pool.release(scratch);
                    return Err(error);
                }
            };
            let output_view = self.pool.entries[output].view.clone();
            let [r, g, b, _] = map.target.to_linear();
            self.effect_compositor.encode(
                device,
                encoder,
                [&filtered_view, &filtered_view, &output_view],
                crate::effects::compositor::EffectUniforms {
                    viewport: [w as f32, h as f32],
                    progress: map.start * self.scale,
                    band: map.end * self.scale,
                    target: [r, g, b, map.strength],
                    mode: 1,
                    padding: [0; 3],
                },
            );
            if let Some(index) = mapped_scratch.replace(output) {
                self.pool.release(index);
            }
            filtered_view = output_view;
            passes += 1;
        }
        if let Some((_, radius)) = group.filter {
            if let Err(error) = self.apply_blur(
                device,
                queue,
                encoder,
                &view,
                group.bounds,
                (BlurParams::for_radius(radius * self.scale), true),
            ) {
                self.pool.release(scratch);
                return Err(error);
            }
            if radius * self.scale >= 0.5 {
                passes += 4;
            }
        }
        let b = group.bounds.intersect(Rect {
            x: 0.0,
            y: 0.0,
            w: w as f32,
            h: h as f32,
        });
        let (bw, bh) = (w as f32, h as f32);
        let result = self.blit_clipped(
            device,
            encoder,
            &filtered_view,
            base_view,
            self.format,
            Rect {
                x: b.x.max(0.0),
                y: b.y.max(0.0),
                w: b.w.min(bw),
                h: b.h.min(bh),
            },
            [
                (b.x.max(0.0)) / bw,
                (b.y.max(0.0)) / bh,
                b.w.min(bw) / bw,
                b.h.min(bh) / bh,
            ],
            group.opacity,
            [0.0, 0.0, -1.0, 0.0],
            0.0,
            0.0,
            1.0,
            &group.clips,
        );
        if let Some(index) = mapped_scratch {
            self.pool.release(index);
        }
        self.pool.release(scratch);
        result.map(|()| passes)
    }

    /// Compose pipeline for an exact color-target format. The base working
    /// format and the surface format may differ; the pipeline target must
    /// match the view or sampling/blending silently misbehaves.
    fn compose_pipe_for(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> Result<wgpu::RenderPipeline, RenderError> {
        if let Some(pipe) = self.compose_pipes.get(&format) {
            return Ok(pipe.clone());
        }
        if self.compose_pipes.len() >= MAX_COMPOSE_PIPES {
            return Err(RenderError::BudgetExceeded("compose pipeline variants"));
        }
        let pipe = make_compose_pipeline(device, &self.compose_bind_layout, format);
        self.compose_pipes.insert(format, pipe.clone());
        Ok(pipe)
    }

    /// Textured-quad composite onto `target` (Load blend).
    /// Buffer discipline: uniform and instance buffers are FRESH per call,
    /// never a shared buffer rewritten mid-encoder — queued writes execute
    /// before their own submit, so no draw can observe another pass's values.
    /// `clip` is a physical-px rect with the shader's convention
    /// (`w < 0` disables); `clip_radius` rounds it with real edge AA.
    #[allow(clippy::too_many_arguments)]
    fn blit(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        target: &wgpu::TextureView,
        target_format: wgpu::TextureFormat,
        dst: Rect,
        src_uv: [f32; 4],
        opacity: f32,
        clip: [f32; 4],
        clip_radius: f32,
        output_gamma: f32,
        premultiplied: f32,
    ) -> Result<(), RenderError> {
        self.blit_clipped(
            device,
            encoder,
            src,
            target,
            target_format,
            dst,
            src_uv,
            opacity,
            clip,
            clip_radius,
            output_gamma,
            premultiplied,
            &[],
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn blit_clipped(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        target: &wgpu::TextureView,
        target_format: wgpu::TextureFormat,
        dst: Rect,
        src_uv: [f32; 4],
        opacity: f32,
        clip: [f32; 4],
        clip_radius: f32,
        output_gamma: f32,
        premultiplied: f32,
        clips: &[(Rect, f32)],
    ) -> Result<(), RenderError> {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aqua-compose-u"),
            size: std::mem::size_of::<ComposeUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Viewport uniform must match the TARGET size; base and surface share
        // dimensions in the facade (letterbox otherwise).
        self.queue.write_buffer(
            &uniform,
            0,
            bytemuck::cast_slice(&[ComposeUniforms {
                viewport: [self.width as f32, self.height as f32],
                opacity,
                clip_radius,
                output_gamma,
                premultiplied,
                _pad: [clips.len() as f32, 0.0],
            }]),
        );
        let mut clip_data = Vec::<[f32; 8]>::with_capacity(clips.len().max(1));
        for (bounds, radius) in clips {
            clip_data.push([
                bounds.x, bounds.y, bounds.w, bounds.h, *radius, 0.0, 0.0, 0.0,
            ]);
        }
        if clip_data.is_empty() {
            clip_data.push([0.0; 8]);
        }
        let clip_bytes = bytemuck::cast_slice(&clip_data);
        if clip_bytes.len() as u64 > device.limits().max_storage_buffer_binding_size {
            return Err(RenderError::BudgetExceeded("clip stack"));
        }
        let clip_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aqua-compose-clips"),
            size: clip_bytes.len() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&clip_buffer, 0, clip_bytes);
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("aqua-compose-bind"),
            layout: &self.compose_bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: clip_buffer.as_entire_binding(),
                },
            ],
        });
        let inst = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aqua-compose-inst"),
            size: std::mem::size_of::<ComposeInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(
            &inst,
            0,
            bytemuck::cast_slice(&[ComposeInstance {
                dst: [dst.x, dst.y, dst.w, dst.h],
                src: src_uv,
                clip,
            }]),
        );
        let pipe = self.compose_pipe_for(device, target_format)?;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("aqua-blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipe);
            pass.set_bind_group(0, &bind, &[]);
            pass.set_vertex_buffer(0, self.quad_vb.slice(..));
            pass.set_index_buffer(self.quad_ib.slice(..), wgpu::IndexFormat::Uint32);
            pass.set_vertex_buffer(1, inst.slice(..));
            pass.draw_indexed(0..6, 0, 0..1);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn align256(v: u32) -> u32 {
    v.div_ceil(256) * 256
}

fn full_uv() -> [f32; 4] {
    [0.0, 0.0, 1.0, 1.0]
}

/// Exact raster area of a damage set in physical px (full frame when `full`).
fn damage_area(set: &DamageSet) -> u64 {
    if set.full {
        return (set.frame.w.max(0.0) * set.frame.h.max(0.0)) as u64;
    }
    set.damage
        .iter()
        .map(|r| (r.w.max(0.0) * r.h.max(0.0)) as u64)
        .sum()
}

fn rects_overlap(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

/// Clamp a physical damage rect to target pixels for scissoring.
/// `None` when the region covers nothing.
fn clamp_scissor(r: &Rect, w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
    let x0 = r.x.floor().clamp(0.0, w as f32) as u32;
    let y0 = r.y.floor().clamp(0.0, h as f32) as u32;
    let x1 = (r.x + r.w).ceil().clamp(0.0, w as f32) as u32;
    let y1 = (r.y + r.h).ceil().clamp(0.0, h as f32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0, y0, x1 - x0, y1 - y0))
}

/// Sort + merge adjacent dirty slot ranges in place.
fn coalesce_ranges(ranges: &mut Vec<(usize, usize)>) {
    if ranges.len() < 2 {
        return;
    }
    ranges.sort_unstable();
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (s, e) in ranges.drain(..) {
        if let Some(last) = out.last_mut() {
            if last.1 >= s {
                last.1 = last.1.max(e);
                continue;
            }
        }
        out.push((s, e));
    }
    *ranges = out;
}

/// Build one rect instance (physical px) from a scene item and its resolved
/// clip context. Shared by the slow-path walk and fast-path slot patches so
/// both produce bit-identical instances.
#[allow(clippy::too_many_arguments)]
fn build_rect_instance(
    bounds: &Rect,
    radius: f32,
    fill: Oklch,
    border_width: f32,
    border_color: Oklch,
    shadow: &Option<(f32, f32, f32, Oklch)>,
    clip: (Rect, f32),
    s: f32,
) -> RectInstance {
    let phys = Rect {
        x: bounds.x * s,
        y: bounds.y * s,
        w: bounds.w * s,
        h: bounds.h * s,
    };
    let (clip_rect, clip_r) = clip;
    let (sox, soy, sblur, sen) = shadow
        .map(|(x, y, b, _)| (x * s, y * s, b * s, 1.0))
        .unwrap_or((0.0, 0.0, 0.0, 0.0));
    let shadow_color = shadow.map(|(_, _, _, c)| c.to_gpu()).unwrap_or([0.0; 4]);
    // Instance quad covers rect + shadow footprint.
    let pad = sblur * 2.5 + soy.abs().max(sox.abs()) + 2.0;
    let quad = if sen > 0.5 {
        Rect {
            x: (phys.x - pad).min(phys.x + sox - sblur * 2.0),
            y: (phys.y - pad).min(phys.y + soy - sblur * 2.0),
            w: phys.w + 2.0 * pad + sox.abs() + sblur * 4.0,
            h: phys.h + 2.0 * pad + soy.abs() + sblur * 4.0,
        }
    } else {
        phys
    };
    let mut inst = RectInstance {
        rect: [quad.x, quad.y, quad.w, quad.h],
        rb_pack: [radius * s, border_width * s, 0.0, 0.0],
        fill: fill.to_gpu(),
        border: border_color.to_gpu(),
        shadow: [phys.x + sox, phys.y + soy, sblur, sen],
        shadow_color,
        clip: [clip_rect.x, clip_rect.y, clip_rect.w, clip_rect.h],
        misc: [clip_r, 1.0, 0.0, 0.0],
    };
    // NOTE: shader centers the shadow at instance-quad center + shadow.xy,
    // so store the offset relative to the quad center (quad covers rect +
    // shadow footprint).
    let qc = [quad.x + quad.w * 0.5, quad.y + quad.h * 0.5];
    let rc = [phys.x + phys.w * 0.5, phys.y + phys.h * 0.5];
    inst.shadow[0] = rc[0] + sox - qc[0];
    inst.shadow[1] = rc[1] + soy - qc[1];
    inst
}

/// Push a top-level instance into the plan or the enclosing group,
/// maintaining the group's logical union bounds.
// `Vec` (not a slice) is required: groups grow via `push`.
#[allow(clippy::ptr_arg)]
fn emit_surface(
    plan: &mut FramePlan,
    groups: &mut [GroupOp],
    surface: SurfaceOp,
    scale: f32,
    top_start: &mut u32,
    texts: &mut Vec<ShapedRun>,
) {
    match groups.last_mut() {
        Some(group) => {
            let bounds = Rect {
                x: surface.bounds.x / scale,
                y: surface.bounds.y / scale,
                w: surface.bounds.w / scale,
                h: surface.bounds.h / scale,
            };
            group.bounds = match group.commands.is_empty() {
                true => bounds,
                false => group.bounds.union(bounds),
            };
            group.commands.push(GroupCommand::Surface(surface));
        }
        None => {
            flush_rect_run(&mut plan.ops, &plan.instances, top_start);
            flush_texts(&mut plan.ops, texts);
            plan.ops.push(Op::Surface(surface));
            *top_start = plan.instances.len() as u32;
        }
    }
}

fn emit_rect_instance(
    plan: &mut FramePlan,
    group_stack: &mut [GroupOp],
    s: f32,
    inst: RectInstance,
) {
    if let Some(g) = group_stack.last_mut() {
        let b = Rect {
            x: inst.rect[0] / s,
            y: inst.rect[1] / s,
            w: inst.rect[2] / s,
            h: inst.rect[3] / s,
        };
        g.bounds = if g.commands.is_empty() {
            b
        } else {
            g.bounds.union(b)
        };
        let index = g.instances.len() as u32;
        g.instances.push(inst);
        match g.commands.last_mut() {
            Some(GroupCommand::Rects { count, .. }) => *count += 1,
            _ => g.commands.push(GroupCommand::Rects {
                start: index,
                count: 1,
            }),
        }
    } else {
        plan.instances.push(inst);
    }
}

/// Close the pending top-level rect run as one draw call.
/// Close the pending top-level rect run as one draw call, recording the
/// run's physical union bounds for partial-repaint culling.
fn flush_rect_run(ops: &mut Vec<Op>, instances: &[RectInstance], top_start: &mut u32) {
    let start = *top_start as usize;
    let count = instances.len() - start;
    if count > 0 {
        let mut bounds = Rect {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        };
        let mut first = true;
        for inst in &instances[start..] {
            let r = Rect {
                x: inst.rect[0],
                y: inst.rect[1],
                w: inst.rect[2],
                h: inst.rect[3],
            };
            bounds = if first { r } else { bounds.union(r) };
            first = false;
        }
        ops.push(Op::Rects {
            start: *top_start,
            count: count as u32,
            bounds,
        });
    }
    *top_start = instances.len() as u32;
}

/// Close pending top-level texts as one ordered op.
fn flush_texts(ops: &mut Vec<Op>, pending: &mut Vec<ShapedRun>) {
    if !pending.is_empty() {
        ops.push(Op::Texts(std::mem::take(pending)));
    }
}

/// Effect-inclusive logical footprint for damage: core `bounds()` (which
/// already unions shadow offset+blur) plus renderer-side sampling padding
/// for backdrop blur and rounded-clip expansion on external layers.
fn item_bounds(item: &DisplayItem) -> Option<Rect> {
    match item {
        DisplayItem::BackdropBlur { bounds, radius } => {
            Some(crate::scene::inflate(*bounds, *radius * 2.0))
        }
        DisplayItem::External {
            bounds,
            rounded_clip,
            ..
        } => Some(crate::scene::inflate(*bounds, *rounded_clip)),
        _ => item.bounds(),
    }
}

/// Build a compose pipeline for an exact target format (shared layout).
fn make_compose_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("aqua-compose"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/compose.wgsl").into()),
    });
    let attrs = wgpu::vertex_attr_array![
        0 => Float32x2,
    ];
    let instance_attrs = wgpu::vertex_attr_array![
        1 => Float32x4, 2 => Float32x4, 3 => Float32x4,
    ];
    let blend_premult = Some(wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("aqua-compose-pipeline"),
        layout: Some(
            &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("aqua-compose-layout"),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            }),
        ),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[
                Some(wgpu::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attrs,
                }),
                Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<ComposeInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &instance_attrs,
                }),
            ],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: blend_premult,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn transform_item(item: &DisplayItem, transform: aqua_core::Transform2D) -> DisplayItem {
    let bounds = |r: Rect| Rect {
        x: r.x * transform.a + transform.tx,
        y: r.y * transform.d + transform.ty,
        w: r.w * transform.a,
        h: r.h * transform.d,
    };
    let mut result = item.clone();
    match &mut result {
        DisplayItem::PushTextColorMap(map) => {
            map.start = map.start * transform.a + transform.tx;
            map.end = map.end * transform.a + transform.tx;
        }
        DisplayItem::Rect {
            bounds: r,
            radius,
            border_width,
            shadow,
            ..
        } => {
            *r = bounds(*r);
            *radius *= transform.a;
            *border_width *= transform.a;
            if let Some((x, y, blur, _)) = shadow {
                *x *= transform.a;
                *y *= transform.a;
                *blur *= transform.a;
            }
        }
        DisplayItem::PushContentBlur { bounds: r, radius }
        | DisplayItem::PushClip { bounds: r, radius }
        | DisplayItem::BackdropBlur { bounds: r, radius } => {
            *r = bounds(*r);
            *radius *= transform.a;
        }
        DisplayItem::Image {
            bounds: r,
            rounded_clip,
            ..
        }
        | DisplayItem::External {
            bounds: r,
            rounded_clip,
            ..
        } => {
            *r = bounds(*r);
            *rounded_clip *= transform.a;
        }
        DisplayItem::Vector { bounds: r, .. } => *r = bounds(*r),
        DisplayItem::Text { at, .. } => {
            at[0] = at[0] * transform.a + transform.tx;
            at[1] = at[1] * transform.d + transform.ty;
        }
        _ => {}
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compose uniform ABI regression: exactly 8 floats / 32 bytes with the
    /// documented offsets. A 7-float / 28-byte buffer fails validation
    /// against the shader (seen live in media/viewport duplicates).
    #[test]
    fn compose_uniform_abi_32b() {
        assert_eq!(std::mem::size_of::<ComposeUniforms>(), 32);
        let u = ComposeUniforms {
            viewport: [1.0, 2.0],
            opacity: 3.0,
            clip_radius: 4.0,
            output_gamma: 5.0,
            premultiplied: 6.0,
            _pad: [7.0, 8.0],
        };
        let words: &[f32] = bytemuck::cast_slice(std::slice::from_ref(&u));
        assert_eq!(words, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
    }

    #[test]
    fn rect_instance_is_128b() {
        assert_eq!(std::mem::size_of::<RectInstance>(), 128);
    }

    #[test]
    fn blur_uniform_is_32b() {
        assert_eq!(std::mem::size_of::<BlurUniforms>(), 32);
    }
}
