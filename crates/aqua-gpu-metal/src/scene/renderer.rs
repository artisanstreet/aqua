use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;

use aqua_graph::{
    Blend, CustomShaderId, Extent, FramePlan, LoadOp, PassKind, ShaderId, TextureRef,
};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLBlitCommandEncoder, MTLCommandBuffer, MTLCommandEncoder, MTLComputeCommandEncoder,
    MTLDevice, MTLOrigin, MTLPixelFormat, MTLPrimitiveType, MTLRenderCommandEncoder,
    MTLSamplerAddressMode, MTLSamplerDescriptor, MTLSamplerMinMagFilter, MTLSamplerMipFilter,
    MTLSamplerState, MTLScissorRect, MTLSize, MTLTextureUsage, MTLViewport,
};

use super::custom::compiler::RuntimeCompiler;
use super::custom::{parameters, Custom, CUSTOM_INPUTS};
use super::frames::Frame;
use super::map::{slot, Slot, INSTANCES, SECOND_INSTANCES};
use super::persistent::PersistentTextures;
use super::pipeline::{ComputePipeline, Kind, PipelineStats, Pipelines, RenderPipeline};
use super::transients::Transients;
use super::Error;
use crate::hal::budget::Trim;
use crate::hal::{Device, Error as DeviceError, Texture, TextureDesc, View};
use crate::ledger::Kind as LedgerKind;
use crate::lifetimes::Precision;
use crate::memo::ColorMemo;
use crate::pack::{pack_quad_with, pack_shadow_with};
use crate::sprite::{pack_glyph_with, pack_image, pack_slug_with, SPRITE_STRIDE};

pub const CONSTANT_FLOATS: usize = 36;
const BOX_BLUR_THREADS: MTLSize = MTLSize {
    width: 64,
    height: 1,
    depth: 1,
};
const MAX_INPUTS: usize = CUSTOM_INPUTS;
const SET_BYTES_LIMIT: usize = 4096;
const ROW_BOX_BLUR: CustomShaderId = CustomShaderId(1);
const INSTANCE_ALIGNMENT: u64 = 16;
const FRAME_BINDING: u32 = 0;
const SOURCE_BINDING: u32 = 16;
const SECOND_SOURCE_BINDING: u32 = 17;
const SAMPLER_BINDING: u32 = 32;
const OUTPUT_BINDING: u32 = 48;

const fn buffer_slot(binding: u32) -> usize {
    match slot(binding) {
        Some(Slot::Buffer(index)) => index as usize,
        _ => panic!("invariant: the binding is a Metal buffer slot"),
    }
}

const fn texture_slot(binding: u32) -> usize {
    match slot(binding) {
        Some(Slot::Texture(index)) => index as usize,
        _ => panic!("invariant: the binding is a Metal texture slot"),
    }
}

const fn sampler_slot(binding: u32) -> usize {
    match slot(binding) {
        Some(Slot::Sampler(index)) => index as usize,
        _ => panic!("invariant: the binding is a Metal sampler slot"),
    }
}

const FRAME_SLOT: usize = buffer_slot(FRAME_BINDING);
const SOURCE_SLOT: usize = texture_slot(SOURCE_BINDING);
const SECOND_SOURCE_SLOT: usize = texture_slot(SECOND_SOURCE_BINDING);
const SAMPLER_SLOT: usize = sampler_slot(SAMPLER_BINDING);
const OUTPUT_SLOT: usize = texture_slot(OUTPUT_BINDING);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneOptions {
    pub transient_ceiling: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneStats {
    pub passes: u32,
    pub instances: u32,
    pub draws: u32,
    pub upload_bytes: u64,
}

#[derive(Clone, Copy)]
enum Upload {
    None,
    Quads { offset: u64, count: u32 },
    Solids { offset: u64, count: u32 },
    Srgba { offset: u64, count: u32 },
    Shadows { offset: u64, count: u32 },
    Sprites { offset: u64, count: u32 },
}

fn append_all<T: bytemuck::Pod>(bytes: &mut Vec<u8>, items: impl Iterator<Item = T>) -> (u64, u32) {
    bytes.resize(
        (bytes.len() as u64).next_multiple_of(INSTANCE_ALIGNMENT) as usize,
        0,
    );
    let offset = bytes.len() as u64;
    bytes.reserve(items.size_hint().0 * std::mem::size_of::<T>());
    let mut count = 0;
    for item in items {
        bytes.extend_from_slice(bytemuck::bytes_of(&item));
        count += 1;
    }
    (offset, count)
}

#[derive(Default)]
struct Scratch {
    bytes: Vec<u8>,
    uploads: Vec<Upload>,
    colors: ColorMemo,
}

impl Scratch {
    fn pack(&mut self, plan: &FramePlan) {
        self.bytes.clear();
        self.uploads.clear();
        let bytes = &mut self.bytes;
        let colors = &mut self.colors;
        for pass in &plan.passes {
            let upload = match &pass.kind {
                PassKind::SrgbaSolids { fills, .. } if !fills.is_empty() => {
                    let (offset, count) =
                        append_all(bytes, fills.iter().map(|fill| u32::from_le_bytes(fill.0)));
                    Upload::Srgba { offset, count }
                }
                PassKind::Solids { instances } if !instances.is_empty() => {
                    let (offset, count) =
                        append_all(bytes, instances.iter().map(crate::pack::pack_solid));
                    Upload::Solids { offset, count }
                }
                PassKind::Instanced { instances, .. } if !instances.is_empty() => {
                    let (offset, count) = append_all(
                        bytes,
                        instances.iter().map(|quad| pack_quad_with(quad, colors)),
                    );
                    Upload::Quads { offset, count }
                }
                PassKind::Shadows { instances } if !instances.is_empty() => {
                    let (offset, count) = append_all(
                        bytes,
                        instances
                            .iter()
                            .map(|shadow| pack_shadow_with(shadow, colors)),
                    );
                    Upload::Shadows { offset, count }
                }
                PassKind::Glyphs { instances, .. } if !instances.is_empty() => {
                    let (offset, count) = append_all(
                        bytes,
                        instances.iter().map(|glyph| pack_glyph_with(glyph, colors)),
                    );
                    Upload::Sprites { offset, count }
                }
                PassKind::Slug { instances, .. } if !instances.is_empty() => {
                    let (offset, count) = append_all(
                        bytes,
                        instances.iter().map(|slug| pack_slug_with(slug, colors)),
                    );
                    Upload::Sprites { offset, count }
                }
                PassKind::Images { instances } if !instances.is_empty() => {
                    let (offset, count) = append_all(bytes, instances.iter().map(pack_image));
                    Upload::Sprites { offset, count }
                }
                _ => Upload::None,
            };
            self.uploads.push(upload);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

impl Rect {
    fn full(extent: Extent) -> Self {
        Self {
            left: 0,
            top: 0,
            right: extent.width,
            bottom: extent.height,
        }
    }

    fn bounded(bounds: aqua_core::geometry::Rect, extent: Extent) -> Self {
        let clamp = |value: f32, limit: u32| (value.max(0.0) as u64).min(limit as u64) as u32;
        let left = clamp(bounds.x.floor(), extent.width);
        let top = clamp(bounds.y.floor(), extent.height);
        Self {
            left,
            top,
            right: clamp((bounds.x + bounds.w).ceil(), extent.width).max(left),
            bottom: clamp((bounds.y + bounds.h).ceil(), extent.height).max(top),
        }
    }

    fn is_empty(self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    fn scissor(self) -> MTLScissorRect {
        MTLScissorRect {
            x: self.left as usize,
            y: self.top as usize,
            width: (self.right - self.left) as usize,
            height: (self.bottom - self.top) as usize,
        }
    }
}

fn constants(
    target: Extent,
    opacity: f32,
    region: [f32; 4],
    values: [f32; 4],
    source: Extent,
    matrix: Option<&[f32; 20]>,
) -> [f32; CONSTANT_FLOATS] {
    let mut out = [0.0f32; CONSTANT_FLOATS];
    out[..16].copy_from_slice(&[
        target.width as f32,
        target.height as f32,
        opacity,
        0.0,
        region[0],
        region[1],
        region[2],
        region[3],
        values[0],
        values[1],
        values[2],
        values[3],
        source.width as f32,
        source.height as f32,
        0.0,
        0.0,
    ]);
    if let Some(matrix) = matrix {
        out[16..].copy_from_slice(matrix);
    }
    out
}

#[derive(Clone, Copy)]
enum Vertices {
    None,
    One(u64),
    Two(u64, u64),
}

struct Draw<'a> {
    pipeline: RenderPipeline,
    constants: [f32; CONSTANT_FLOATS],
    sources: [View<'a>; 2],
    vertices: Vertices,
    instances: u32,
}

fn resolve<'a>(
    reference: TextureRef,
    target: View<'a>,
    transients: &'a Transients,
    textures: &'a PersistentTextures,
) -> Result<View<'a>, Error> {
    match reference {
        TextureRef::Surface => Ok(target),
        TextureRef::Transient(id) => transients
            .get(id)
            .map(|texture| texture.texture.view())
            .ok_or(Error::Unsupported("unknown transient")),
        TextureRef::Persistent(handle) => textures
            .get(handle)
            .map(|entry| entry.texture.view())
            .ok_or(Error::UnknownTexture(handle)),
    }
}

fn bytes_of<T>(value: &T) -> (NonNull<c_void>, usize) {
    (NonNull::from(value).cast(), std::mem::size_of::<T>())
}

struct Shared {
    device: Arc<Device>,
    dummy: Texture,
    sampler: Retained<ProtocolObject<dyn MTLSamplerState>>,
}

pub struct SceneRenderer {
    shared: Shared,
    pipelines: Pipelines,
    custom: Custom,
    transients: Transients,
    scratch: Scratch,
    dummy_cleared: bool,
}

fn linear_clamp(device: &Device) -> Result<Retained<ProtocolObject<dyn MTLSamplerState>>, Error> {
    let descriptor = MTLSamplerDescriptor::new();
    descriptor.setMinFilter(MTLSamplerMinMagFilter::Linear);
    descriptor.setMagFilter(MTLSamplerMinMagFilter::Linear);
    descriptor.setMipFilter(MTLSamplerMipFilter::NotMipmapped);
    descriptor.setSAddressMode(MTLSamplerAddressMode::ClampToEdge);
    descriptor.setTAddressMode(MTLSamplerAddressMode::ClampToEdge);
    descriptor.setRAddressMode(MTLSamplerAddressMode::ClampToEdge);
    device
        .raw
        .newSamplerStateWithDescriptor(&descriptor)
        .ok_or(Error::Device(DeviceError::Unavailable {
            call: "newSamplerStateWithDescriptor",
        }))
}

impl SceneRenderer {
    pub fn new(
        device: &Arc<Device>,
        cache: Option<&[u8]>,
        options: SceneOptions,
        compiler: RuntimeCompiler,
    ) -> Result<Self, Error> {
        let dummy = Texture::new(
            device,
            TextureDesc {
                extent: Extent {
                    width: 1,
                    height: 1,
                },
                format: MTLPixelFormat::RGBA8Unorm,
                usage: MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget,
                label: "unbound_source",
                kind: LedgerKind::Texture,
                bytes: 4,
            },
        )?;
        Ok(Self {
            shared: Shared {
                device: Arc::clone(device),
                dummy,
                sampler: linear_clamp(device)?,
            },
            pipelines: Pipelines::new(device, cache),
            custom: Custom::new(device, compiler),
            transients: Transients::new(options.transient_ceiling),
            scratch: Scratch::default(),
            dummy_cleared: false,
        })
    }

    pub fn warm(&mut self, format: MTLPixelFormat) -> Result<(), Error> {
        self.pipelines.warm(format)
    }

    pub fn custom_passes(&self) -> bool {
        self.custom.available()
    }

    pub fn register_compute(
        &mut self,
        id: CustomShaderId,
        hlsl: &str,
        entry: &str,
    ) -> Result<(), Error> {
        self.custom.register(id, hlsl, entry)
    }

    pub fn close_startup(&mut self) {
        self.pipelines.close_startup();
    }

    pub fn late_pipelines(&self) -> &[Kind] {
        self.pipelines.late()
    }

    pub fn pipeline_stats(&self) -> PipelineStats {
        self.pipelines.stats()
    }

    pub fn pipeline_cache(&self) -> Vec<u8> {
        self.pipelines.serialize()
    }

    pub fn transient_bytes(&self) -> u64 {
        self.transients.bytes()
    }

    pub fn transient_textures(&self) -> usize {
        self.transients.textures()
    }

    pub fn trim(&mut self, trim: &Trim) -> u64 {
        if trim.shrink_upload_ring {
            self.scratch = Scratch::default();
        }
        match trim.free_transients {
            true => self.transients.trim(),
            false => 0,
        }
    }

    pub fn prepare(&mut self, plan: &FramePlan) -> u64 {
        self.scratch.pack(plan);
        let side_data: u64 = plan
            .passes
            .iter()
            .map(|pass| match &pass.kind {
                PassKind::SrgbaSolids { bounds, .. } => {
                    bounds.len() as u64 * 16 + INSTANCE_ALIGNMENT
                }
                _ => 0,
            })
            .sum();
        self.scratch.bytes.len() as u64 + INSTANCE_ALIGNMENT + side_data
    }

    pub fn record(
        &mut self,
        frame: &mut Frame<'_>,
        plan: &FramePlan,
        target: View<'_>,
        textures: &PersistentTextures,
    ) -> Result<SceneStats, Error> {
        assert_eq!(
            self.scratch.uploads.len(),
            plan.passes.len(),
            "invariant: SceneRenderer::record follows SceneRenderer::prepare for the same plan"
        );
        let precision = match target.format == MTLPixelFormat::BGRA8Unorm {
            true => Precision::EightBit,
            false => Precision::Planned,
        };
        self.transients
            .ensure(&self.shared.device, plan, precision)?;
        if !self.dummy_cleared {
            frame.clear(self.shared.dummy.view())?;
            self.dummy_cleared = true;
        }
        let base = frame.stream.push(INSTANCE_ALIGNMENT, &self.scratch.bytes);
        let mut stats = SceneStats {
            passes: plan.passes.len() as u32,
            instances: plan.instance_count() as u32,
            upload_bytes: self.scratch.bytes.len() as u64,
            ..SceneStats::default()
        };
        let first_surface_use = plan.passes.iter().find(|pass| {
            pass.reads.contains(&TextureRef::Surface) || pass.writes.contains(&TextureRef::Surface)
        });
        let clears_surface_first = first_surface_use.is_some_and(|pass| {
            pass.writes.first() == Some(&TextureRef::Surface)
                && matches!(pass.load, LoadOp::Clear(_))
        });
        if !clears_surface_first {
            self.shared.render(
                frame,
                target,
                Some(plan.clear.to_gpu()),
                Rect::full(plan.extent),
                &[],
            )?;
        }
        for (index, pass) in plan.passes.iter().enumerate() {
            let Some(&write) = pass.writes.first() else {
                continue;
            };
            let context = Context {
                shared: &self.shared,
                transients: &self.transients,
                textures,
                target,
                uploads: &self.scratch.uploads,
                base,
            };
            stats.draws += record_pass(
                &context,
                &mut self.pipelines,
                &mut self.custom,
                frame,
                plan,
                index,
                write,
            )?;
        }
        aqua_protocol::probe::count(
            aqua_protocol::probe::Metric::UploadBytes,
            stats.upload_bytes,
        );
        Ok(stats)
    }
}

impl Shared {
    fn render(
        &self,
        frame: &Frame<'_>,
        out: View<'_>,
        clear: Option<[f32; 4]>,
        scissor: Rect,
        draws: &[Draw<'_>],
    ) -> Result<u32, Error> {
        self.encode(frame, out, clear, scissor, |encoder| {
            draws
                .iter()
                .for_each(|draw| self.draw(encoder, frame, draw))
        })?;
        Ok(draws.len() as u32)
    }

    #[allow(clippy::too_many_arguments)]
    fn custom(
        &self,
        frame: &Frame<'_>,
        out: View<'_>,
        clear: Option<[f32; 4]>,
        scissor: Rect,
        pipeline: &RenderPipeline,
        parameters: &[f32],
        inputs: &[View<'_>; CUSTOM_INPUTS],
    ) -> Result<u32, Error> {
        let bytes: &[u8] = bytemuck::cast_slice(parameters);
        if bytes.len() > SET_BYTES_LIMIT {
            return Err(Error::Unsupported(
                "custom pass parameters above 4096 bytes",
            ));
        }
        self.encode(frame, out, clear, scissor, |encoder| {
            encoder.setRenderPipelineState(pipeline);
            unsafe {
                encoder.setFragmentBytes_length_atIndex(
                    NonNull::from(bytes).cast(),
                    bytes.len(),
                    FRAME_SLOT,
                );
                for (slot, input) in inputs.iter().enumerate() {
                    encoder.setFragmentTexture_atIndex(Some(input.raw), SOURCE_SLOT + slot);
                }
                encoder.setFragmentSamplerState_atIndex(Some(&self.sampler), SAMPLER_SLOT);
                encoder.drawPrimitives_vertexStart_vertexCount(
                    MTLPrimitiveType::TriangleStrip,
                    0,
                    4,
                );
            }
        })?;
        Ok(u32::from(!scissor.is_empty()))
    }

    fn encode(
        &self,
        frame: &Frame<'_>,
        out: View<'_>,
        clear: Option<[f32; 4]>,
        scissor: Rect,
        record: impl FnOnce(&ProtocolObject<dyn MTLRenderCommandEncoder>),
    ) -> Result<(), Error> {
        let encoder = frame.render_pass(out, clear)?;
        encoder.setViewport(MTLViewport {
            originX: 0.0,
            originY: 0.0,
            width: out.extent.width as f64,
            height: out.extent.height as f64,
            znear: 0.0,
            zfar: 1.0,
        });
        if !scissor.is_empty() {
            encoder.setScissorRect(scissor.scissor());
            record(&encoder);
        }
        encoder.endEncoding();
        Ok(())
    }

    fn draw(
        &self,
        encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
        frame: &Frame<'_>,
        draw: &Draw<'_>,
    ) {
        let (constants, length) = bytes_of(&draw.constants);
        let ring = frame.ring();
        encoder.setRenderPipelineState(&draw.pipeline);
        unsafe {
            encoder.setVertexBytes_length_atIndex(constants, length, FRAME_SLOT);
            encoder.setFragmentBytes_length_atIndex(constants, length, FRAME_SLOT);
            encoder.setFragmentTexture_atIndex(Some(draw.sources[0].raw), SOURCE_SLOT);
            encoder.setFragmentTexture_atIndex(Some(draw.sources[1].raw), SECOND_SOURCE_SLOT);
            encoder.setFragmentSamplerState_atIndex(Some(&self.sampler), SAMPLER_SLOT);
            match draw.vertices {
                Vertices::None => {}
                Vertices::One(offset) => {
                    encoder.setVertexBuffer_offset_atIndex(
                        Some(ring),
                        offset as usize,
                        INSTANCES as usize,
                    );
                }
                Vertices::Two(first, second) => {
                    encoder.setVertexBuffer_offset_atIndex(
                        Some(ring),
                        first as usize,
                        INSTANCES as usize,
                    );
                    encoder.setVertexBuffer_offset_atIndex(
                        Some(ring),
                        second as usize,
                        SECOND_INSTANCES as usize,
                    );
                }
            }
            encoder.drawPrimitives_vertexStart_vertexCount_instanceCount(
                MTLPrimitiveType::TriangleStrip,
                0,
                4,
                draw.instances as usize,
            );
        }
    }

    fn copy(
        &self,
        frame: &Frame<'_>,
        source: View<'_>,
        source_origin: [u32; 2],
        out: View<'_>,
        dest_origin: [u32; 2],
        size: Extent,
    ) -> Result<(), Error> {
        let blit = frame.blit()?;
        unsafe {
            blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                source.raw,
                0,
                0,
                MTLOrigin {
                    x: source_origin[0] as usize,
                    y: source_origin[1] as usize,
                    z: 0,
                },
                MTLSize {
                    width: size.width as usize,
                    height: size.height as usize,
                    depth: 1,
                },
                out.raw,
                0,
                0,
                MTLOrigin {
                    x: dest_origin[0] as usize,
                    y: dest_origin[1] as usize,
                    z: 0,
                },
            )
        };
        blit.endEncoding();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch(
        &self,
        frame: &Frame<'_>,
        pipeline: &ComputePipeline,
        params: [f32; 4],
        inputs: &[View<'_>],
        out: View<'_>,
        groups: [u32; 3],
        threads: MTLSize,
    ) -> Result<(), Error> {
        let encoder = frame
            .command()
            .computeCommandEncoder()
            .ok_or(Error::Device(DeviceError::Unavailable {
                call: "computeCommandEncoder",
            }))?;
        let (parameters, length) = bytes_of(&params);
        encoder.setComputePipelineState(pipeline);
        unsafe {
            encoder.setBytes_length_atIndex(parameters, length, FRAME_SLOT);
            for (slot, input) in inputs.iter().enumerate() {
                encoder.setTexture_atIndex(Some(input.raw), SOURCE_SLOT + slot);
            }
            encoder.setTexture_atIndex(Some(out.raw), OUTPUT_SLOT);
        }
        encoder.dispatchThreadgroups_threadsPerThreadgroup(
            MTLSize {
                width: groups[0] as usize,
                height: groups[1] as usize,
                depth: groups[2] as usize,
            },
            threads,
        );
        encoder.endEncoding();
        Ok(())
    }
}

struct Context<'a> {
    shared: &'a Shared,
    transients: &'a Transients,
    textures: &'a PersistentTextures,
    target: View<'a>,
    uploads: &'a [Upload],
    base: u64,
}

impl<'a> Context<'a> {
    fn resolve(&self, reference: TextureRef) -> Result<View<'a>, Error> {
        resolve(reference, self.target, self.transients, self.textures)
    }
}

fn record_pass(
    context: &Context<'_>,
    pipelines: &mut Pipelines,
    custom: &mut Custom,
    frame: &mut Frame<'_>,
    plan: &FramePlan,
    index: usize,
    write: TextureRef,
) -> Result<u32, Error> {
    let pass = &plan.passes[index];
    let shared = context.shared;
    let out = context.resolve(write)?;
    let reads = pass
        .reads
        .iter()
        .take(MAX_INPUTS)
        .map(|reference| context.resolve(*reference))
        .collect::<Result<Vec<_>, _>>()?;
    if reads.iter().any(|read| read.same_texture(&out)) {
        return Err(Error::Unsupported("a pass that samples its own target"));
    }
    let extent = out.extent;
    let source_extent = reads.first().map_or(extent, |read| read.extent);
    let dummy = shared.dummy.view();
    let sources = [
        reads.first().copied().unwrap_or(dummy),
        reads.get(1).copied().unwrap_or(dummy),
    ];
    let inputs: [View<'_>; CUSTOM_INPUTS] =
        std::array::from_fn(|slot| reads.get(slot).copied().unwrap_or(dummy));
    match &pass.kind {
        PassKind::Copy {
            source_origin,
            dest_origin,
            size,
        } => {
            let source = *reads
                .first()
                .ok_or(Error::Unsupported("copy without a source"))?;
            if source.format == out.format {
                let width = size
                    .width
                    .min(source_extent.width.saturating_sub(source_origin[0]))
                    .min(extent.width.saturating_sub(dest_origin[0]));
                let height = size
                    .height
                    .min(source_extent.height.saturating_sub(source_origin[1]))
                    .min(extent.height.saturating_sub(dest_origin[1]));
                if width == 0 || height == 0 {
                    return Ok(0);
                }
                shared.copy(
                    frame,
                    source,
                    *source_origin,
                    out,
                    *dest_origin,
                    Extent { width, height },
                )?;
                return Ok(1);
            }
            if matches!(write, TextureRef::Persistent(_)) {
                return Err(Error::Unsupported("write to a persistent texture"));
            }
            let right = (dest_origin[0] + size.width)
                .min(extent.width)
                .min(dest_origin[0] + source_extent.width.saturating_sub(source_origin[0]));
            let bottom = (dest_origin[1] + size.height)
                .min(extent.height)
                .min(dest_origin[1] + source_extent.height.saturating_sub(source_origin[1]));
            if right <= dest_origin[0] || bottom <= dest_origin[1] {
                return Ok(0);
            }
            let pipeline = pipelines.get(Kind::Copy, out.format)?;
            let offset = [
                source_origin[0] as f32 - dest_origin[0] as f32,
                source_origin[1] as f32 - dest_origin[1] as f32,
                0.0,
                0.0,
            ];
            let draw = Draw {
                pipeline,
                constants: constants(extent, 1.0, offset, [0.0; 4], source_extent, None),
                sources,
                vertices: Vertices::None,
                instances: 1,
            };
            let scissor = Rect {
                left: dest_origin[0],
                top: dest_origin[1],
                right,
                bottom,
            };
            shared.render(frame, out, None, scissor, &[draw])
        }
        PassKind::Compute {
            shader: ShaderId::Custom(id),
            groups,
            params,
        } => {
            if !matches!(write, TextureRef::Transient(_)) {
                return Err(Error::Unsupported("compute output must be a transient"));
            }
            if out.format != MTLPixelFormat::RGBA16Float {
                return Err(Error::Unsupported("compute output must be rgba16f"));
            }
            match (custom.registered(*id)?, *id == ROW_BOX_BLUR) {
                (Some(registered), _) => shared.dispatch(
                    frame,
                    &registered.pipeline,
                    *params,
                    &inputs,
                    out,
                    *groups,
                    registered.threads,
                )?,
                (None, true) => shared.dispatch(
                    frame,
                    &pipelines.compute()?,
                    *params,
                    &sources[..1],
                    out,
                    *groups,
                    BOX_BLUR_THREADS,
                )?,
                (None, false) => return Err(Error::Unsupported("unregistered compute shader")),
            }
            Ok(1)
        }
        PassKind::Compute { .. } => {
            Err(Error::Unsupported("compute shaders other than custom ids"))
        }
        PassKind::Custom(spec) => {
            if matches!(write, TextureRef::Persistent(_)) {
                return Err(Error::Unsupported("write to a persistent texture"));
            }
            let pipeline = custom.graphics(spec, out.format)?;
            let clear = match pass.load {
                LoadOp::Clear(color) => Some(color.to_gpu()),
                LoadOp::Load | LoadOp::DontCare => None,
            };
            shared.custom(
                frame,
                out,
                clear,
                Rect::bounded(spec.bounds, extent),
                &pipeline,
                &parameters(&spec.params),
                &inputs,
            )
        }
        kind => {
            if matches!(write, TextureRef::Persistent(_)) {
                return Err(Error::Unsupported("write to a persistent texture"));
            }
            let clear = match pass.load {
                LoadOp::Clear(color) => Some(color.to_gpu()),
                LoadOp::Load | LoadOp::DontCare => None,
            };
            let draws = draws(
                context, pipelines, frame, plan, index, kind, out, &reads, sources,
            )?;
            shared.render(frame, out, clear, Rect::full(extent), &draws)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draws<'a>(
    context: &Context<'a>,
    pipelines: &mut Pipelines,
    frame: &mut Frame<'_>,
    plan: &FramePlan,
    index: usize,
    kind: &PassKind,
    out: View<'a>,
    reads: &[View<'a>],
    sources: [View<'a>; 2],
) -> Result<Vec<Draw<'a>>, Error> {
    let base = context.base;
    let extent = out.extent;
    let format = out.format;
    let source_extent = reads.first().map_or(extent, |read| read.extent);
    let standard = constants(extent, 1.0, [0.0; 4], [0.0; 4], source_extent, None);
    let tone = plan.text_tone;
    let single = |pipeline, constants, vertices, instances| Draw {
        pipeline,
        constants,
        sources,
        vertices,
        instances,
    };
    let draws = match (kind, context.uploads[index]) {
        (
            PassKind::Instanced {
                shader: ShaderId::Quad,
                ..
            },
            Upload::Quads { offset, count },
        ) => vec![single(
            pipelines.get(Kind::Quad, format)?,
            standard,
            Vertices::One(base + offset),
            count,
        )],
        (PassKind::SrgbaSolids { bounds, .. }, Upload::Srgba { offset, count }) => {
            let pipeline = pipelines.get(Kind::Srgba, format)?;
            let packed: Vec<[f32; 4]> = bounds
                .iter()
                .map(|rect| [rect.x, rect.y, rect.w, rect.h])
                .collect();
            let geometry = frame
                .stream
                .push(INSTANCE_ALIGNMENT, bytemuck::cast_slice(&packed));
            vec![single(
                pipeline,
                standard,
                Vertices::Two(geometry, base + offset),
                count,
            )]
        }
        (PassKind::Solids { .. }, Upload::Solids { offset, count }) => vec![single(
            pipelines.get(Kind::Solid, format)?,
            standard,
            Vertices::One(base + offset),
            count,
        )],
        (PassKind::Shadows { .. }, Upload::Shadows { offset, count }) => vec![single(
            pipelines.get(Kind::Shadow, format)?,
            standard,
            Vertices::One(base + offset),
            count,
        )],
        (
            PassKind::Fullscreen {
                shader: ShaderId::LayerOver,
                opacity,
            },
            _,
        ) => vec![single(
            pipelines.get(Kind::Layer, format)?,
            constants(extent, *opacity, [0.0; 4], [0.0; 4], source_extent, None),
            Vertices::None,
            1,
        )],
        (PassKind::Filter { shader, params }, _) => {
            let kind = match shader {
                ShaderId::BlurDown => Kind::BlurDown,
                ShaderId::BlurUp => Kind::BlurUp,
                ShaderId::ColorMatrix => Kind::ColorMatrix,
                _ => return Err(Error::Unsupported("filter shader")),
            };
            let region = [
                params.region.x,
                params.region.y,
                params.region.w,
                params.region.h,
            ];
            let matrix = (*shader == ShaderId::ColorMatrix).then_some(&params.matrix.0);
            vec![single(
                pipelines.get(kind, format)?,
                constants(extent, 1.0, region, params.values, source_extent, matrix),
                Vertices::None,
                1,
            )]
        }
        (PassKind::Glyphs { .. }, Upload::Sprites { offset, count })
        | (PassKind::Slug { .. }, Upload::Sprites { offset, count }) => {
            let (pipeline_kind, tone) = match kind {
                PassKind::Glyphs { coverage, .. } => (Kind::Glyph, coverage.tone(tone)),
                _ => (Kind::Slug, tone),
            };
            vec![single(
                pipelines.get(pipeline_kind, format)?,
                constants(
                    extent,
                    1.0,
                    tone.ratios(),
                    [tone.contrast(), 0.0, 0.0, 0.0],
                    source_extent,
                    None,
                ),
                Vertices::One(base + offset),
                count,
            )]
        }
        (PassKind::Images { instances }, Upload::Sprites { offset, .. }) => {
            let mut draws = Vec::new();
            let mut start = 0usize;
            while start < instances.len() {
                let first = &instances[start];
                let end = instances[start..]
                    .iter()
                    .position(|next| {
                        next.source != first.source
                            || next.blend != first.blend
                            || next.mask.map(|mask| mask.source)
                                != first.mask.map(|mask| mask.source)
                    })
                    .map_or(instances.len(), |run| start + run);
                let pipeline_kind = match first.blend {
                    Blend::Over => Kind::Image,
                    Blend::Replace => Kind::ImageReplace,
                };
                let pipeline = pipelines.get(pipeline_kind, format)?;
                let source = context.resolve(first.source)?;
                let mask = first
                    .mask
                    .map(|mask| context.resolve(mask.source))
                    .transpose()?;
                if source.same_texture(&out) || mask.is_some_and(|mask| mask.same_texture(&out)) {
                    return Err(Error::Unsupported("a pass that samples its own target"));
                }
                draws.push(Draw {
                    pipeline,
                    constants: constants(extent, 1.0, [0.0; 4], [0.0; 4], source.extent, None),
                    sources: [source, mask.unwrap_or(context.shared.dummy.view())],
                    vertices: Vertices::One(base + offset + start as u64 * SPRITE_STRIDE as u64),
                    instances: (end - start) as u32,
                });
                start = end;
            }
            draws
        }
        (
            PassKind::Instanced { .. }
            | PassKind::Solids { .. }
            | PassKind::SrgbaSolids { .. }
            | PassKind::Shadows { .. }
            | PassKind::Glyphs { .. }
            | PassKind::Slug { .. }
            | PassKind::Images { .. },
            Upload::None,
        ) => Vec::new(),
        _ => return Err(Error::Unsupported("pass kind")),
    };
    Ok(draws)
}
