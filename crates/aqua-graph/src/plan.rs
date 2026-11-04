use aqua_core::{Oklch, geometry::Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Extent {
    pub width: u32,
    pub height: u32,
}

impl Extent {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub fn texels(self) -> u64 {
        self.width as u64 * self.height as u64
    }

    pub fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub fn bounds(self) -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            w: self.width as f32,
            h: self.height as f32,
        }
    }

    pub fn half(self) -> Extent {
        Extent {
            width: self.width.div_ceil(2).max(1),
            height: self.height.div_ceil(2).max(1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TransientId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextureHandle(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureRef {
    Surface,
    Transient(TransientId),
    Persistent(TextureHandle),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureFormat {
    Rgba16Float,
    Rgba8Unorm,
    Bgra8Unorm,
    R8Unorm,
}

impl TextureFormat {
    pub fn bytes_per_texel(self) -> u64 {
        match self {
            TextureFormat::Rgba16Float => 8,
            TextureFormat::Rgba8Unorm | TextureFormat::Bgra8Unorm => 4,
            TextureFormat::R8Unorm => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CustomShaderId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShaderId {
    Quad,
    LayerOver,
    Shadow,
    Glyph,
    Image,
    BlurDown,
    BlurUp,
    ColorMatrix,
    Custom(CustomShaderId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpecConstant {
    pub index: u32,
    pub value: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientStop {
    pub offset: f32,
    pub color: Oklch,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gradient {
    pub start: [f32; 2],
    pub end: [f32; 2],
    pub stops: [GradientStop; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuadInstance {
    pub bounds: Rect,
    pub radius: f32,
    pub border_width: f32,
    pub fill: Oklch,
    pub border_color: Oklch,
    pub clip: Option<Rect>,
    pub gradient: Option<Gradient>,
    pub opacity: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowInstance {
    pub bounds: Rect,
    pub radius: f32,
    pub offset: [f32; 2],
    pub sigma: f32,
    pub spread: f32,
    pub color: Oklch,
    pub clip: Option<Rect>,
    pub opacity: f32,
}

impl ShadowInstance {
    pub fn extent(&self) -> f32 {
        self.spread.max(0.0) + 3.0 * self.sigma.max(0.0)
    }

    pub fn covered(&self) -> Rect {
        let reach = self.extent();
        Rect {
            x: self.bounds.x + self.offset[0] - reach,
            y: self.bounds.y + self.offset[1] - reach,
            w: self.bounds.w + 2.0 * reach,
            h: self.bounds.h + 2.0 * reach,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphInstance {
    pub bounds: Rect,
    pub texels: Rect,
    pub color: Oklch,
    pub clip: Option<Rect>,
    pub opacity: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageInstance {
    pub source: TextureRef,
    pub bounds: Rect,
    pub texels: Rect,
    pub radius: f32,
    pub opacity: f32,
    pub clip: Option<Rect>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LoadOp {
    Load,
    Clear(Oklch),
    DontCare,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResourceState {
    Undefined,
    Present,
    RenderTarget,
    ShaderRead,
    CopySource,
    CopyDest,
    UnorderedAccess,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Barrier {
    Transition {
        resource: TextureRef,
        from: ResourceState,
        to: ResourceState,
    },
    Alias {
        before: TransientId,
        after: TransientId,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct CustomPassSpec {
    pub name: String,
    pub hlsl: String,
    pub entry: String,
    pub bounds: Rect,
    pub params: Vec<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilterParams {
    pub region: Rect,
    pub values: [f32; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub enum PassKind {
    Instanced {
        shader: ShaderId,
        instances: Vec<QuadInstance>,
    },
    Shadows {
        instances: Vec<ShadowInstance>,
    },
    Glyphs {
        atlas: TextureHandle,
        instances: Vec<GlyphInstance>,
    },
    Images {
        instances: Vec<ImageInstance>,
    },
    Fullscreen {
        shader: ShaderId,
        opacity: f32,
    },
    Filter {
        shader: ShaderId,
        params: FilterParams,
    },
    Compute {
        shader: ShaderId,
        groups: [u32; 3],
    },
    Copy {
        source_origin: [u32; 2],
        dest_origin: [u32; 2],
        size: Extent,
    },
    Custom(CustomPassSpec),
}

impl PassKind {
    pub fn draw_count(&self) -> usize {
        match self {
            PassKind::Instanced { instances, .. } => instances.len(),
            PassKind::Shadows { instances } => instances.len(),
            PassKind::Glyphs { instances, .. } => instances.len(),
            PassKind::Images { instances } => instances.len(),
            PassKind::Fullscreen { .. }
            | PassKind::Filter { .. }
            | PassKind::Compute { .. }
            | PassKind::Copy { .. }
            | PassKind::Custom(_) => 0,
        }
    }

    pub fn read_state(&self) -> ResourceState {
        match self {
            PassKind::Copy { .. } => ResourceState::CopySource,
            _ => ResourceState::ShaderRead,
        }
    }

    pub fn write_state(&self) -> ResourceState {
        match self {
            PassKind::Compute { .. } => ResourceState::UnorderedAccess,
            PassKind::Copy { .. } => ResourceState::CopyDest,
            _ => ResourceState::RenderTarget,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pass {
    pub kind: PassKind,
    pub reads: Vec<TextureRef>,
    pub writes: Vec<TextureRef>,
    pub load: LoadOp,
    pub constants: Vec<SpecConstant>,
    pub barriers: Vec<Barrier>,
}

impl Pass {
    pub fn new(kind: PassKind, reads: Vec<TextureRef>, writes: Vec<TextureRef>) -> Self {
        Self {
            kind,
            reads,
            writes,
            load: LoadOp::Load,
            constants: Vec::new(),
            barriers: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransientTexture {
    pub id: TransientId,
    pub extent: Extent,
    pub format: TextureFormat,
    pub heap_offset: u64,
}

impl TransientTexture {
    pub fn bytes(&self) -> u64 {
        self.extent.texels() * self.format.bytes_per_texel()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FramePlan {
    pub extent: Extent,
    pub clear: Oklch,
    pub textures: Vec<TransientTexture>,
    pub passes: Vec<Pass>,
    pub heap_bytes: u64,
    pub final_barriers: Vec<Barrier>,
}

impl FramePlan {
    pub fn instance_count(&self) -> usize {
        self.passes.iter().map(|pass| pass.kind.draw_count()).sum()
    }

    pub fn texture(&self, id: TransientId) -> Option<&TransientTexture> {
        self.textures.iter().find(|texture| texture.id == id)
    }

    pub fn barrier_count(&self) -> usize {
        self.final_barriers.len()
            + self
                .passes
                .iter()
                .map(|pass| pass.barriers.len())
                .sum::<usize>()
    }
}
