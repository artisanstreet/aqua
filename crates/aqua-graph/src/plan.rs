use aqua_core::{geometry::Rect, Oklch, Srgba8};

use crate::linear::Linear2;
use crate::slug::SlugInstance;
use crate::tone::ToneCurve;

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
    Rgba32Float,
    Rgba8Unorm,
    Bgra8Unorm,
    R8Unorm,
}

impl TextureFormat {
    pub fn bytes_per_texel(self) -> u64 {
        match self {
            TextureFormat::Rgba32Float => 16,
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
    pub linear: Linear2,
}

impl QuadInstance {
    pub fn covered(&self) -> Rect {
        self.linear.cover(self.bounds)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SolidColor {
    Oklch(Oklch),
    Srgba(Srgba8),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SolidInstance {
    pub bounds: Rect,
    pub fill: SolidColor,
}

impl SolidInstance {
    pub fn quad(self) -> QuadInstance {
        QuadInstance {
            bounds: self.bounds,
            radius: 0.0,
            border_width: 0.0,
            fill: match self.fill {
                SolidColor::Oklch(fill) => fill,
                SolidColor::Srgba(fill) => fill.to_oklch(),
            },
            border_color: Oklch::TRANSPARENT,
            clip: None,
            gradient: None,
            opacity: 1.0,
            linear: Linear2::IDENTITY,
        }
    }
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
    pub cast: ShadowCast,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadowCast {
    Whole,
    Outside,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Coverage {
    #[default]
    Text,
    Exact,
}

impl Coverage {
    pub fn tone(self, text: ToneCurve) -> ToneCurve {
        match self {
            Self::Text => text,
            Self::Exact => ToneCurve::EXACT,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Blend {
    #[default]
    Over,
    Replace,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageMask {
    pub source: TextureRef,
    pub offset: [f32; 2],
    pub scale: [f32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageInstance {
    pub source: TextureRef,
    pub bounds: Rect,
    pub texels: Rect,
    pub radius: f32,
    pub opacity: f32,
    pub clip: Option<Rect>,
    pub clip_radius: f32,
    pub blend: Blend,
    pub linear: Linear2,
    pub mask: Option<ImageMask>,
}

impl ImageInstance {
    pub fn covered(&self) -> Rect {
        self.linear.cover(self.bounds)
    }
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
    pub isolate_backdrop: bool,
    pub reference: Option<aqua_core::shader::ReferenceShader>,
    pub name: String,
    pub hlsl: String,
    pub entry: String,
    pub bounds: Rect,
    pub params: Vec<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorMatrix(pub [f32; 20]);

impl ColorMatrix {
    pub const IDENTITY: ColorMatrix = ColorMatrix([
        1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        1.0, 0.0,
    ]);

    pub fn saturation(amount: f32) -> Self {
        let (r, g, b) = (0.2126, 0.7152, 0.0722);
        let inverse = 1.0 - amount;
        ColorMatrix([
            r * inverse + amount,
            g * inverse,
            b * inverse,
            0.0,
            0.0,
            r * inverse,
            g * inverse + amount,
            b * inverse,
            0.0,
            0.0,
            r * inverse,
            g * inverse,
            b * inverse + amount,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
        ])
    }

    pub fn is_finite(&self) -> bool {
        self.0.iter().all(|value| value.is_finite())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilterParams {
    pub region: Rect,
    pub values: [f32; 4],
    pub matrix: ColorMatrix,
}

impl FilterParams {
    pub fn blur(region: Rect, values: [f32; 4]) -> Self {
        Self {
            region,
            values,
            matrix: ColorMatrix::IDENTITY,
        }
    }

    pub fn color_matrix(region: Rect, matrix: ColorMatrix) -> Self {
        Self {
            region,
            values: [0.0; 4],
            matrix,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PassKind {
    SrgbaSolids {
        bounds: std::sync::Arc<[Rect]>,
        fills: Vec<Srgba8>,
    },
    Solids {
        instances: Vec<SolidInstance>,
    },
    Instanced {
        shader: ShaderId,
        instances: Vec<QuadInstance>,
    },
    Shadows {
        instances: Vec<ShadowInstance>,
    },
    Glyphs {
        atlas: TextureHandle,
        coverage: Coverage,
        instances: Vec<GlyphInstance>,
    },
    Images {
        instances: Vec<ImageInstance>,
    },
    Slug {
        store: TextureHandle,
        instances: Vec<SlugInstance>,
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
        params: [f32; 4],
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
            PassKind::SrgbaSolids { fills, .. } => fills.len(),
            PassKind::Solids { instances } => instances.len(),
            PassKind::Instanced { instances, .. } => instances.len(),
            PassKind::Shadows { instances } => instances.len(),
            PassKind::Glyphs { instances, .. } => instances.len(),
            PassKind::Images { instances } => instances.len(),
            PassKind::Slug { instances, .. } => instances.len(),
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
    pub fn clear(target: TextureRef, color: Oklch) -> Self {
        Self {
            kind: PassKind::Instanced {
                shader: ShaderId::Quad,
                instances: Vec::new(),
            },
            reads: Vec::new(),
            writes: vec![target],
            load: LoadOp::Clear(color),
            constants: Vec::new(),
            barriers: Vec::new(),
        }
    }

    pub fn is_clear(&self) -> bool {
        matches!(
            &self.kind,
            PassKind::Instanced { instances, .. } if instances.is_empty()
        ) && matches!(self.load, LoadOp::Clear(_))
    }

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
    pub text_tone: ToneCurve,
}

impl FramePlan {
    pub fn repaint_bounds(&self, region: Rect) -> Rect {
        let frame = self.extent.bounds();
        let sources: Vec<Rect> = self
            .passes
            .iter()
            .filter(|pass| pass.reads.contains(&TextureRef::Surface))
            .map(|pass| match &pass.kind {
                PassKind::Filter { params, .. } => params.region,
                PassKind::Copy {
                    source_origin,
                    size,
                    ..
                } => Rect {
                    x: source_origin[0] as f32,
                    y: source_origin[1] as f32,
                    w: size.width as f32,
                    h: size.height as f32,
                },
                _ => frame,
            })
            .map(|source| {
                Rect::from_edges(
                    source.x.floor(),
                    source.y.floor(),
                    source.right().ceil(),
                    source.bottom().ceil(),
                )
                .intersect(frame)
            })
            .collect();
        let mut region = region.intersect(frame);
        loop {
            let before = region;
            for source in &sources {
                if region.overlaps(*source) {
                    region = region.union(*source);
                }
            }
            if region == before {
                return region;
            }
        }
    }

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
