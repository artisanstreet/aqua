use std::sync::Arc;

use aqua_graph::{
    Blend, CustomShaderId, Extent, FramePlan, LoadOp, PassKind, ShaderId, TextureRef,
};
use ash::vk;

use super::binding::{Layouts, Uniform};
use super::frames::Frame;
use super::persistent::PersistentTextures;
use super::pipeline::{Kind, PipelineStats, Pipelines};
use super::transients::Transients;
use super::Error;
use crate::hal::budget::Trim;
use crate::hal::{Access, Device, Image, ImageDesc, ImageRef};
use crate::ledger::Kind as LedgerKind;
use crate::lifetimes::Precision;
use crate::memo::ColorMemo;
use crate::pack::{pack_quad_with, pack_shadow_with};
use crate::sprite::{pack_glyph_with, pack_image, pack_slug_with, SPRITE_STRIDE};

pub const CONSTANT_FLOATS: usize = 36;
const CONSTANT_BYTES: u64 = (CONSTANT_FLOATS * 4) as u64;
const COMPUTE_PARAMETER_BYTES: u64 = 16;
const MAX_INPUTS: usize = 8;
const ROW_BOX_BLUR: CustomShaderId = CustomShaderId(1);
const INSTANCE_ALIGNMENT: u64 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneOptions {
    pub transient_ceiling: Option<u64>,
    pub timestamps: bool,
}

impl Default for SceneOptions {
    fn default() -> Self {
        Self {
            transient_ceiling: None,
            timestamps: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneStats {
    pub passes: u32,
    pub instances: u32,
    pub draws: u32,
    pub barriers: u32,
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

fn pass_label(kind: &PassKind) -> &'static str {
    match kind {
        PassKind::SrgbaSolids { .. } => "srgba_solids",
        PassKind::Solids { .. } => "solids",
        PassKind::Instanced { .. } => "quads",
        PassKind::Shadows { .. } => "shadows",
        PassKind::Glyphs { .. } => "glyphs",
        PassKind::Images { .. } => "images",
        PassKind::Slug { .. } => "slug",
        PassKind::Fullscreen { .. } => "fullscreen",
        PassKind::Filter { .. } => "filter",
        PassKind::Compute { .. } => "compute",
        PassKind::Copy { .. } => "copy",
        PassKind::Custom(_) => "custom",
    }
}

fn full(extent: Extent) -> vk::Rect2D {
    vk::Rect2D {
        offset: vk::Offset2D { x: 0, y: 0 },
        extent: vk::Extent2D {
            width: extent.width,
            height: extent.height,
        },
    }
}

fn edges(rect: vk::Rect2D) -> (i32, i32, i32, i32) {
    (
        rect.offset.x,
        rect.offset.y,
        rect.offset.x + rect.extent.width as i32,
        rect.offset.y + rect.extent.height as i32,
    )
}

fn from_edges(left: i32, top: i32, right: i32, bottom: i32) -> vk::Rect2D {
    vk::Rect2D {
        offset: vk::Offset2D { x: left, y: top },
        extent: vk::Extent2D {
            width: (right - left).max(0) as u32,
            height: (bottom - top).max(0) as u32,
        },
    }
}

fn confine(rect: vk::Rect2D, within: Option<vk::Rect2D>) -> vk::Rect2D {
    match within {
        None => rect,
        Some(bound) => {
            let (left, top, right, bottom) = edges(rect);
            let (bound_left, bound_top, bound_right, bound_bottom) = edges(bound);
            let left = left.max(bound_left);
            let top = top.max(bound_top);
            from_edges(
                left,
                top,
                right.min(bound_right).max(left),
                bottom.min(bound_bottom).max(top),
            )
        }
    }
}

fn is_empty(rect: vk::Rect2D) -> bool {
    rect.extent.width == 0 || rect.extent.height == 0
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

struct Draw {
    pipeline: vk::Pipeline,
    uniform: u64,
    sources: [vk::ImageView; 2],
    vertices: Vertices,
    instances: u32,
}

fn resolve<'a>(
    reference: TextureRef,
    target: ImageRef<'a>,
    transients: &'a Transients,
    textures: &'a PersistentTextures,
) -> Result<ImageRef<'a>, Error> {
    match reference {
        TextureRef::Surface => Ok(target),
        TextureRef::Transient(id) => transients
            .get(id)
            .map(|texture| texture.image.as_ref())
            .ok_or(Error::Unsupported("unknown transient")),
        TextureRef::Persistent(handle) => textures
            .get(handle)
            .map(|entry| entry.image.as_ref())
            .ok_or(Error::UnknownTexture(handle)),
    }
}

fn transitions<'a>(
    sampled: impl IntoIterator<Item = ImageRef<'a>>,
    to: Access,
) -> Vec<vk::ImageMemoryBarrier2<'static>> {
    sampled
        .into_iter()
        .filter_map(|image| image.transition(to))
        .collect()
}

struct Shared {
    device: Arc<Device>,
    layouts: Layouts,
    dummy: Image,
}

pub struct SceneRenderer {
    shared: Shared,
    pipelines: Pipelines,
    transients: Transients,
    scratch: Scratch,
    uniform_slot: u64,
}

impl SceneRenderer {
    pub fn new(
        device: &Arc<Device>,
        cache: Option<&[u8]>,
        options: SceneOptions,
    ) -> Result<Self, Error> {
        let dummy = Image::new(
            device,
            ImageDesc {
                extent: Extent {
                    width: 1,
                    height: 1,
                },
                format: vk::Format::R8_UNORM,
                usage: vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
                label: "unbound_source",
                kind: LedgerKind::Texture,
                bytes: 1,
            },
        )?;
        let alignment = device.info().uniform_alignment;
        Ok(Self {
            shared: Shared {
                device: Arc::clone(device),
                layouts: Layouts::new(device)?,
                dummy,
            },
            pipelines: Pipelines::new(device, cache)?,
            transients: Transients::new(options.transient_ceiling),
            scratch: Scratch::default(),
            uniform_slot: CONSTANT_BYTES.next_multiple_of(alignment) + alignment,
        })
    }

    pub fn warm(&mut self, format: vk::Format) -> Result<(), Error> {
        self.pipelines.warm(format, &self.shared.layouts)
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

    pub fn pipeline_cache(&self) -> Result<Vec<u8>, Error> {
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
        let draws: u64 = plan
            .passes
            .iter()
            .map(|pass| match &pass.kind {
                PassKind::Images { instances } => instances.len() as u64 + 1,
                _ => 1,
            })
            .sum::<u64>()
            + 1;
        let bounds: u64 = plan
            .passes
            .iter()
            .map(|pass| match &pass.kind {
                PassKind::SrgbaSolids { bounds, .. } => bounds.len() as u64 * 16 + 16,
                _ => 0,
            })
            .sum();
        self.scratch.bytes.len() as u64 + INSTANCE_ALIGNMENT + bounds + draws * self.uniform_slot
    }

    pub fn record(
        &mut self,
        frame: &mut Frame<'_>,
        plan: &FramePlan,
        target: ImageRef<'_>,
        textures: &PersistentTextures,
        repaint: Option<vk::Rect2D>,
    ) -> Result<SceneStats, Error> {
        assert_eq!(
            self.scratch.uploads.len(),
            plan.passes.len(),
            "SceneRenderer::record needs SceneRenderer::prepare for the same plan first"
        );
        let precision = match target.format == vk::Format::B8G8R8A8_UNORM {
            true => Precision::EightBit,
            false => Precision::Planned,
        };
        self.transients
            .ensure(&self.shared.device, plan, precision)?;
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
            let rect = confine(full(plan.extent), repaint);
            self.shared.render(
                frame,
                target,
                &[],
                rect,
                Some((plan.clear.to_gpu(), rect)),
                &[],
            )?;
        }
        for (index, pass) in plan.passes.iter().enumerate() {
            frame.mark(pass_label(&pass.kind));
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
                frame,
                plan,
                index,
                write,
                repaint.filter(|_| write == TextureRef::Surface),
            )?;
        }
        stats.barriers = frame.barriers();
        aqua_protocol::probe::count(
            aqua_protocol::probe::Metric::UploadBytes,
            stats.upload_bytes,
        );
        Ok(stats)
    }
}

impl Shared {
    fn uniform(&self, frame: &mut Frame<'_>, values: &[f32]) -> Uniform {
        let bytes: &[u8] = bytemuck::cast_slice(values);
        Uniform {
            offset: frame
                .stream
                .push(self.device.info().uniform_alignment, bytes),
            range: bytes.len() as u64,
        }
    }

    fn render(
        &self,
        frame: &mut Frame<'_>,
        out: ImageRef<'_>,
        sampled: &[ImageRef<'_>],
        scissor: vk::Rect2D,
        clear: Option<([f32; 4], vk::Rect2D)>,
        draws: &[Draw],
    ) -> Result<u32, Error> {
        let mut barriers = transitions(
            sampled
                .iter()
                .copied()
                .chain(std::iter::once(self.dummy.as_ref())),
            Access::Sampled,
        );
        barriers.extend(out.transition(Access::Attachment));
        frame.barrier(&barriers);
        let device = &self.device.raw;
        let command = frame.command();
        let attachments = [vk::RenderingAttachmentInfo::default()
            .image_view(out.view)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::STORE)];
        let rendering = vk::RenderingInfo::default()
            .render_area(full(out.extent))
            .layer_count(1)
            .color_attachments(&attachments);
        unsafe {
            device.cmd_begin_rendering(command, &rendering);
            device.cmd_set_viewport(
                command,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: out.extent.height as f32,
                    width: out.extent.width as f32,
                    height: -(out.extent.height as f32),
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            device.cmd_set_scissor(command, 0, &[scissor]);
        }
        if let Some((color, rect)) = clear.filter(|(_, rect)| !is_empty(*rect)) {
            let attachment = vk::ClearAttachment {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                color_attachment: 0,
                clear_value: vk::ClearValue {
                    color: vk::ClearColorValue { float32: color },
                },
            };
            let rect = vk::ClearRect {
                rect,
                base_array_layer: 0,
                layer_count: 1,
            };
            unsafe { device.cmd_clear_attachments(command, &[attachment], &[rect]) };
        }
        let drawn = match is_empty(scissor) {
            true => Ok(()),
            false => draws.iter().try_for_each(|draw| self.draw(frame, draw)),
        };
        unsafe { device.cmd_end_rendering(command) };
        drawn.map(|()| draws.len() as u32)
    }

    fn draw(&self, frame: &mut Frame<'_>, draw: &Draw) -> Result<(), Error> {
        self.layouts.bind_draw(
            frame,
            &Uniform {
                offset: draw.uniform,
                range: CONSTANT_BYTES,
            },
            draw.sources,
        )?;
        let device = &self.device.raw;
        let command = frame.command();
        let ring = frame.ring();
        unsafe {
            device.cmd_bind_pipeline(command, vk::PipelineBindPoint::GRAPHICS, draw.pipeline);
            match draw.vertices {
                Vertices::None => {}
                Vertices::One(offset) => {
                    device.cmd_bind_vertex_buffers(command, 0, &[ring], &[offset])
                }
                Vertices::Two(first, second) => {
                    device.cmd_bind_vertex_buffers(command, 0, &[ring, ring], &[first, second])
                }
            }
            device.cmd_draw(command, 4, draw.instances, 0, 0);
        }
        Ok(())
    }
}

struct Context<'a> {
    shared: &'a Shared,
    transients: &'a Transients,
    textures: &'a PersistentTextures,
    target: ImageRef<'a>,
    uploads: &'a [Upload],
    base: u64,
}

impl<'a> Context<'a> {
    fn resolve(&self, reference: TextureRef) -> Result<ImageRef<'a>, Error> {
        resolve(reference, self.target, self.transients, self.textures)
    }
}

fn record_pass(
    context: &Context<'_>,
    pipelines: &mut Pipelines,
    frame: &mut Frame<'_>,
    plan: &FramePlan,
    index: usize,
    write: TextureRef,
    bound: Option<vk::Rect2D>,
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
    if reads.iter().any(|read| read.same_image(&out)) {
        return Err(Error::Unsupported("a pass that samples its own target"));
    }
    let extent = out.extent;
    let source_extent = reads.first().map_or(extent, |read| read.extent);
    let dummy = shared.dummy.as_ref().view;
    let sources = [
        reads.first().map_or(dummy, |read| read.view),
        reads.get(1).map_or(dummy, |read| read.view),
    ];
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
                let mut barriers = transitions([source], Access::TransferSource);
                barriers.extend(out.transition(Access::TransferDestination));
                frame.barrier(&barriers);
                let layers = vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                };
                let region = vk::ImageCopy {
                    src_subresource: layers,
                    src_offset: vk::Offset3D {
                        x: source_origin[0] as i32,
                        y: source_origin[1] as i32,
                        z: 0,
                    },
                    dst_subresource: layers,
                    dst_offset: vk::Offset3D {
                        x: dest_origin[0] as i32,
                        y: dest_origin[1] as i32,
                        z: 0,
                    },
                    extent: vk::Extent3D {
                        width,
                        height,
                        depth: 1,
                    },
                };
                unsafe {
                    shared.device.raw.cmd_copy_image(
                        frame.command(),
                        source.image,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        out.image,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &[region],
                    )
                };
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
            let pipeline = pipelines.get(Kind::Copy, out.format, &shared.layouts)?;
            let offset = [
                source_origin[0] as f32 - dest_origin[0] as f32,
                source_origin[1] as f32 - dest_origin[1] as f32,
                0.0,
                0.0,
            ];
            let uniform = shared.uniform(
                frame,
                &constants(extent, 1.0, offset, [0.0; 4], source_extent, None),
            );
            let scissor = confine(
                from_edges(
                    dest_origin[0] as i32,
                    dest_origin[1] as i32,
                    right as i32,
                    bottom as i32,
                ),
                bound,
            );
            let draw = Draw {
                pipeline,
                uniform: uniform.offset,
                sources,
                vertices: Vertices::None,
                instances: 1,
            };
            shared.render(frame, out, &reads, scissor, None, &[draw])
        }
        PassKind::Compute {
            shader: ShaderId::Custom(id),
            groups,
            params,
        } if *id == ROW_BOX_BLUR => {
            if !matches!(write, TextureRef::Transient(_)) {
                return Err(Error::Unsupported("compute output must be a transient"));
            }
            if out.format != vk::Format::R16G16B16A16_SFLOAT {
                return Err(Error::Unsupported("compute output must be rgba16f"));
            }
            let pipeline = pipelines.compute(&shared.layouts)?;
            let uniform = shared.uniform(frame, params);
            let mut barriers = transitions(
                reads
                    .iter()
                    .copied()
                    .chain(std::iter::once(shared.dummy.as_ref())),
                Access::Sampled,
            );
            barriers.extend(out.transition(Access::Storage));
            frame.barrier(&barriers);
            shared.layouts.bind_compute(
                frame,
                &Uniform {
                    offset: uniform.offset,
                    range: COMPUTE_PARAMETER_BYTES,
                },
                sources[0],
                out.view,
            )?;
            unsafe {
                shared.device.raw.cmd_bind_pipeline(
                    frame.command(),
                    vk::PipelineBindPoint::COMPUTE,
                    pipeline,
                );
                shared
                    .device
                    .raw
                    .cmd_dispatch(frame.command(), groups[0], groups[1], groups[2]);
            }
            Ok(1)
        }
        PassKind::Compute { .. } => Err(Error::Unsupported(
            "compute shaders other than the row box blur",
        )),
        PassKind::Custom(_) => Err(Error::Unsupported("custom passes")),
        kind => {
            if matches!(write, TextureRef::Persistent(_)) {
                return Err(Error::Unsupported("write to a persistent texture"));
            }
            let scissor = confine(full(extent), bound);
            let clear = match pass.load {
                LoadOp::Clear(color) => Some((color.to_gpu(), scissor)),
                LoadOp::Load | LoadOp::DontCare => None,
            };
            let (draws, sampled) = draws(
                context, pipelines, frame, plan, index, kind, out, &reads, sources,
            )?;
            shared.render(frame, out, &sampled, scissor, clear, &draws)
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
    out: ImageRef<'a>,
    reads: &[ImageRef<'a>],
    sources: [vk::ImageView; 2],
) -> Result<(Vec<Draw>, Vec<ImageRef<'a>>), Error> {
    let shared = context.shared;
    let base = context.base;
    let extent = out.extent;
    let format = out.format;
    let source_extent = reads.first().map_or(extent, |read| read.extent);
    let standard = constants(extent, 1.0, [0.0; 4], [0.0; 4], source_extent, None);
    let tone = plan.text_tone;
    let mut sampled: Vec<ImageRef<'a>> = reads.to_vec();
    let single = |pipeline, uniform: Uniform, vertices, instances| Draw {
        pipeline,
        uniform: uniform.offset,
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
        ) => {
            let pipeline = pipelines.get(Kind::Quad, format, &shared.layouts)?;
            let uniform = shared.uniform(frame, &standard);
            vec![single(
                pipeline,
                uniform,
                Vertices::One(base + offset),
                count,
            )]
        }
        (PassKind::SrgbaSolids { bounds, .. }, Upload::Srgba { offset, count }) => {
            let pipeline = pipelines.get(Kind::Srgba, format, &shared.layouts)?;
            let packed: Vec<[f32; 4]> = bounds
                .iter()
                .map(|rect| [rect.x, rect.y, rect.w, rect.h])
                .collect();
            let geometry = frame
                .stream
                .push(INSTANCE_ALIGNMENT, bytemuck::cast_slice(&packed));
            let uniform = shared.uniform(frame, &standard);
            vec![single(
                pipeline,
                uniform,
                Vertices::Two(geometry, base + offset),
                count,
            )]
        }
        (PassKind::Solids { .. }, Upload::Solids { offset, count }) => {
            let pipeline = pipelines.get(Kind::Solid, format, &shared.layouts)?;
            let uniform = shared.uniform(frame, &standard);
            vec![single(
                pipeline,
                uniform,
                Vertices::One(base + offset),
                count,
            )]
        }
        (PassKind::Shadows { .. }, Upload::Shadows { offset, count }) => {
            let pipeline = pipelines.get(Kind::Shadow, format, &shared.layouts)?;
            let uniform = shared.uniform(frame, &standard);
            vec![single(
                pipeline,
                uniform,
                Vertices::One(base + offset),
                count,
            )]
        }
        (
            PassKind::Fullscreen {
                shader: ShaderId::LayerOver,
                opacity,
            },
            _,
        ) => {
            let pipeline = pipelines.get(Kind::Layer, format, &shared.layouts)?;
            let uniform = shared.uniform(
                frame,
                &constants(extent, *opacity, [0.0; 4], [0.0; 4], source_extent, None),
            );
            vec![single(pipeline, uniform, Vertices::None, 1)]
        }
        (PassKind::Filter { shader, params }, _) => {
            let kind = match shader {
                ShaderId::BlurDown => Kind::BlurDown,
                ShaderId::BlurUp => Kind::BlurUp,
                ShaderId::ColorMatrix => Kind::ColorMatrix,
                _ => return Err(Error::Unsupported("filter shader")),
            };
            let pipeline = pipelines.get(kind, format, &shared.layouts)?;
            let region = [
                params.region.x,
                params.region.y,
                params.region.w,
                params.region.h,
            ];
            let matrix = (*shader == ShaderId::ColorMatrix).then_some(&params.matrix.0);
            let uniform = shared.uniform(
                frame,
                &constants(extent, 1.0, region, params.values, source_extent, matrix),
            );
            vec![single(pipeline, uniform, Vertices::None, 1)]
        }
        (PassKind::Glyphs { .. }, Upload::Sprites { offset, count })
        | (PassKind::Slug { .. }, Upload::Sprites { offset, count }) => {
            let (pipeline_kind, tone) = match kind {
                PassKind::Glyphs { coverage, .. } => (Kind::Glyph, coverage.tone(tone)),
                _ => (Kind::Slug, tone),
            };
            let pipeline = pipelines.get(pipeline_kind, format, &shared.layouts)?;
            let uniform = shared.uniform(
                frame,
                &constants(
                    extent,
                    1.0,
                    tone.ratios(),
                    [tone.contrast(), 0.0, 0.0, 0.0],
                    source_extent,
                    None,
                ),
            );
            vec![single(
                pipeline,
                uniform,
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
                let pipeline = pipelines.get(pipeline_kind, format, &shared.layouts)?;
                let source = context.resolve(first.source)?;
                let mask = first
                    .mask
                    .map(|mask| context.resolve(mask.source))
                    .transpose()?;
                if source.same_image(&out) || mask.is_some_and(|mask| mask.same_image(&out)) {
                    return Err(Error::Unsupported("a pass that samples its own target"));
                }
                let uniform = shared.uniform(
                    frame,
                    &constants(extent, 1.0, [0.0; 4], [0.0; 4], source.extent, None),
                );
                sampled.push(source);
                sampled.extend(mask);
                draws.push(Draw {
                    pipeline,
                    uniform: uniform.offset,
                    sources: [
                        source.view,
                        mask.map_or(shared.dummy.as_ref().view, |mask| mask.view),
                    ],
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
    Ok((draws, sampled))
}
