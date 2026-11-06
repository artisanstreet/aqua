use aqua_graph::{Blend, Extent, FramePlan, LoadOp, PassKind, ShaderId, TextureRef};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::custom::{scissor_of, ComputePipelines, CustomPipelines, MAX_INPUTS};
use super::persistent::PersistentTextures;
use super::pipeline::{Kind, PipelineStats, Pipelines, Program, CONSTANT_FLOATS};
use super::timestamps::{PassTime, Timestamps};
use super::transients::Transients;
use super::Error;
use crate::hal::budget::Trim;
use crate::ledger::{record, Kind as LedgerKind, Token};
use crate::lifetimes::Precision;
use crate::memo::ColorMemo;
use crate::pack::{pack_quad_with, pack_shadow_with, QUAD_STRIDE, SHADOW_STRIDE};
use crate::sprite::{pack_glyph_with, pack_image, pack_slug_with, SPRITE_STRIDE};

const INITIAL_INSTANCE_BYTES: u64 = 1 << 16;
const PARAMETER_BYTES: u64 = 256;
const COMPUTE_PARAMETER_BYTES: u64 = 16;

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

pub struct SceneTarget<'a> {
    pub texture: &'a ID3D11Texture2D,
    pub rtv: &'a ID3D11RenderTargetView,
    pub srv: Option<&'a ID3D11ShaderResourceView>,
    pub format: DXGI_FORMAT,
    pub extent: Extent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneStats {
    pub passes: u32,
    pub instances: u32,
    pub draws: u32,
    pub barriers: u32,
    pub upload_bytes: u64,
}

struct DynamicBuffer {
    buffer: ID3D11Buffer,
    capacity: u64,
    _token: Token,
}

fn dynamic_buffer(
    device: &ID3D11Device,
    capacity: u64,
    bind: D3D11_BIND_FLAG,
    label: &'static str,
) -> Result<DynamicBuffer, Error> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: capacity as u32,
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: bind.0 as u32,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let mut buffer = None;
    unsafe { device.CreateBuffer(&desc, None, Some(&mut buffer))? };
    Ok(DynamicBuffer {
        buffer: buffer.ok_or(Error::NoAdapter)?,
        capacity,
        _token: record(label, LedgerKind::DynamicBuffer, capacity),
    })
}

fn write_buffer(
    context: &ID3D11DeviceContext1,
    buffer: &ID3D11Buffer,
    bytes: &[u8],
    zero_to: usize,
) -> Result<(), Error> {
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        context.Map(buffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))?;
        let target = mapped.pData as *mut u8;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
        if zero_to > bytes.len() {
            std::ptr::write_bytes(target.add(bytes.len()), 0, zero_to - bytes.len());
        }
        context.Unmap(buffer, 0);
    }
    Ok(())
}

struct SurfaceMirror {
    extent: Extent,
    format: DXGI_FORMAT,
    texture: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    _token: Token,
}

#[derive(Clone, Copy)]
enum Upload {
    None,
    Quads { offset: u64, count: u32 },
    Shadows { offset: u64, count: u32 },
    Sprites { offset: u64, count: u32 },
}

fn align16(value: usize) -> usize {
    value.div_ceil(16) * 16
}

fn append_all<T: bytemuck::Pod>(bytes: &mut Vec<u8>, items: impl Iterator<Item = T>) -> (u64, u32) {
    bytes.resize(align16(bytes.len()), 0);
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

#[derive(Clone)]
struct Bound {
    texture: ID3D11Texture2D,
    rtv: Option<ID3D11RenderTargetView>,
    srv: Option<ID3D11ShaderResourceView>,
    uav: Option<ID3D11UnorderedAccessView>,
    format: DXGI_FORMAT,
    extent: Extent,
}

fn pass_label(kind: &PassKind) -> &'static str {
    match kind {
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

fn confine(rect: RECT, within: Option<RECT>) -> RECT {
    match within {
        None => rect,
        Some(bound) => RECT {
            left: rect.left.max(bound.left),
            top: rect.top.max(bound.top),
            right: rect.right.min(bound.right).max(rect.left.max(bound.left)),
            bottom: rect.bottom.min(bound.bottom).max(rect.top.max(bound.top)),
        },
    }
}

fn full(extent: Extent) -> RECT {
    RECT {
        left: 0,
        top: 0,
        right: extent.width as i32,
        bottom: extent.height as i32,
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

fn clear(
    context: &ID3D11DeviceContext1,
    rtv: &ID3D11RenderTargetView,
    color: [f32; 4],
    rect: RECT,
    extent: Extent,
) {
    let empty = rect.right <= rect.left || rect.bottom <= rect.top;
    match (rect == full(extent), empty) {
        (true, _) => unsafe { context.ClearRenderTargetView(rtv, &color) },
        (false, true) => {}
        (false, false) => unsafe { context.ClearView(rtv, &color, Some(&[rect])) },
    }
}

fn viewport(context: &ID3D11DeviceContext1, extent: Extent) {
    unsafe {
        context.RSSetViewports(Some(&[D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: extent.width as f32,
            Height: extent.height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        }]))
    };
}

pub struct SceneRenderer {
    device: ID3D11Device,
    pipelines: Pipelines,
    custom: CustomPipelines,
    compute: ComputePipelines,
    transients: Transients,
    timestamps: Option<Timestamps>,
    instances: DynamicBuffer,
    constants: DynamicBuffer,
    parameters: DynamicBuffer,
    compute_parameters: DynamicBuffer,
    last_constants: Option<[f32; CONSTANT_FLOATS]>,
    scratch: Scratch,
    mirror: Option<SurfaceMirror>,
    shrink_instances: bool,
}

impl SceneRenderer {
    pub fn new(
        device: &ID3D11Device,
        cache: Option<&[u8]>,
        options: SceneOptions,
    ) -> Result<Self, Error> {
        let stages = crate::stages::StageClock::from_env();
        let pipelines = Pipelines::create(device, cache)?;
        stages.mark("      pipelines_create");
        let timestamps = match options.timestamps {
            true => Some(Timestamps::new(device)?),
            false => None,
        };
        let renderer = Self {
            device: device.clone(),
            pipelines,
            custom: CustomPipelines::new(device),
            compute: ComputePipelines::new(device),
            transients: Transients::new(options.transient_ceiling),
            timestamps,
            instances: dynamic_buffer(
                device,
                INITIAL_INSTANCE_BYTES,
                D3D11_BIND_VERTEX_BUFFER,
                "instance_buffer",
            )?,
            constants: dynamic_buffer(
                device,
                (CONSTANT_FLOATS * 4) as u64,
                D3D11_BIND_CONSTANT_BUFFER,
                "constant_buffer",
            )?,
            parameters: dynamic_buffer(
                device,
                PARAMETER_BYTES,
                D3D11_BIND_CONSTANT_BUFFER,
                "constant_buffer",
            )?,
            compute_parameters: dynamic_buffer(
                device,
                COMPUTE_PARAMETER_BYTES,
                D3D11_BIND_CONSTANT_BUFFER,
                "constant_buffer",
            )?,
            last_constants: None,
            scratch: Scratch::default(),
            mirror: None,
            shrink_instances: false,
        };
        stages.mark("      renderer_buffers");
        Ok(renderer)
    }

    pub fn validate(&self, plan: &FramePlan) -> Result<(), aqua_graph::PlanError> {
        aqua_graph::validate(plan)
    }

    pub fn register_compute(
        &mut self,
        id: aqua_graph::CustomShaderId,
        hlsl: &str,
        entry: &str,
    ) -> Result<(), Error> {
        self.compute.register(id, hlsl, entry)
    }

    pub fn warm(&mut self) -> Result<(), Error> {
        self.pipelines.warm()
    }

    pub fn warm_first_frame(&mut self) -> Result<(), Error> {
        self.pipelines.warm_first_frame()
    }

    pub fn close_startup(&mut self) {
        self.pipelines.close_startup();
    }

    pub fn deferred_pipelines(&self) -> Vec<Kind> {
        self.pipelines.deferred()
    }

    pub fn warm_one(&mut self, kind: Kind) -> Result<(), Error> {
        self.pipelines.warm_one(kind)
    }

    pub fn late_pipelines(&self) -> &[Kind] {
        self.pipelines.late()
    }

    pub fn pipeline_stats(&self) -> PipelineStats {
        self.pipelines.stats()
    }

    pub fn pipeline_cache(&self) -> Option<Vec<u8>> {
        self.pipelines.serialize()
    }

    pub fn gpu_time(&self) -> Option<std::time::Duration> {
        self.timestamps.as_ref().and_then(Timestamps::latest)
    }

    pub fn gpu_pass_times(&self) -> &[PassTime] {
        self.timestamps
            .as_ref()
            .map(Timestamps::latest_passes)
            .unwrap_or(&[])
    }

    pub fn transient_bytes(&self) -> u64 {
        self.transients.bytes()
    }

    pub fn transient_textures(&self) -> usize {
        self.transients.textures()
    }

    pub fn instance_bytes(&self) -> u64 {
        self.instances.capacity
    }

    pub fn constant_bytes(&self) -> u64 {
        self.constants.capacity + self.parameters.capacity + self.compute_parameters.capacity
    }

    pub fn mirror_bytes(&self) -> u64 {
        self.mirror
            .as_ref()
            .map_or(0, |mirror| mirror.extent.texels() * 4)
    }

    pub fn trim(&mut self, trim: &Trim) -> u64 {
        if trim.shrink_upload_ring {
            self.shrink_instances = true;
            self.mirror = None;
        }
        match trim.free_transients {
            true => self.transients.trim(),
            false => 0,
        }
    }

    pub fn reserve(&mut self, plan: &FramePlan, precision: Precision) -> Result<bool, Error> {
        self.transients.ensure(&self.device, plan, precision)
    }

    fn prepare_instances(&mut self, needed: u64) -> Result<(), Error> {
        let shrink = std::mem::take(&mut self.shrink_instances);
        let wanted = needed.next_power_of_two().max(INITIAL_INSTANCE_BYTES);
        if self.instances.capacity < needed || (shrink && wanted != self.instances.capacity) {
            self.instances = dynamic_buffer(
                &self.device,
                wanted,
                D3D11_BIND_VERTEX_BUFFER,
                "instance_buffer",
            )?;
        }
        Ok(())
    }

    fn write_constants(
        &mut self,
        context: &ID3D11DeviceContext1,
        values: [f32; CONSTANT_FLOATS],
    ) -> Result<(), Error> {
        if self.last_constants == Some(values) {
            return Ok(());
        }
        write_buffer(
            context,
            &self.constants.buffer,
            bytemuck::cast_slice(&values),
            0,
        )?;
        self.last_constants = Some(values);
        Ok(())
    }

    fn bind(
        &self,
        reference: TextureRef,
        target: &SceneTarget,
        surface: Extent,
        textures: &PersistentTextures,
    ) -> Result<Bound, Error> {
        match reference {
            TextureRef::Surface => Ok(Bound {
                texture: target.texture.clone(),
                rtv: Some(target.rtv.clone()),
                srv: target.srv.cloned(),
                uav: None,
                format: target.format,
                extent: surface,
            }),
            TextureRef::Transient(id) => {
                let texture = self
                    .transients
                    .get(id)
                    .ok_or(Error::Unsupported("unknown transient"))?;
                Ok(Bound {
                    texture: texture.texture.clone(),
                    rtv: Some(texture.rtv.clone()),
                    srv: Some(texture.srv.clone()),
                    uav: texture.uav.clone(),
                    format: texture.format,
                    extent: texture.extent,
                })
            }
            TextureRef::Persistent(handle) => {
                let entry = textures
                    .get(handle)
                    .ok_or(Error::Unsupported("persistent texture is not resident"))?;
                Ok(Bound {
                    texture: entry.texture.clone(),
                    rtv: None,
                    srv: Some(entry.srv.clone()),
                    uav: None,
                    format: super::formats::dxgi(entry.format),
                    extent: entry.extent,
                })
            }
        }
    }

    fn surface_view(
        &mut self,
        context: &ID3D11DeviceContext1,
        target: &SceneTarget,
        extent: Extent,
    ) -> Result<ID3D11ShaderResourceView, Error> {
        if let Some(srv) = target.srv {
            return Ok(srv.clone());
        }
        let stale = self
            .mirror
            .as_ref()
            .is_none_or(|mirror| mirror.extent != extent || mirror.format != target.format);
        if stale {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: extent.width.max(1),
                Height: extent.height.max(1),
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_TYPELESS,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut texture = None;
            unsafe {
                self.device
                    .CreateTexture2D(&desc, None, Some(&mut texture))?
            };
            let texture: ID3D11Texture2D = texture.ok_or(Error::NoAdapter)?;
            let view = D3D11_SHADER_RESOURCE_VIEW_DESC {
                Format: target.format,
                ViewDimension: windows::Win32::Graphics::Direct3D::D3D11_SRV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_SRV {
                        MostDetailedMip: 0,
                        MipLevels: 1,
                    },
                },
            };
            let mut srv = None;
            unsafe {
                self.device
                    .CreateShaderResourceView(&texture, Some(&view), Some(&mut srv))?
            };
            self.mirror = Some(SurfaceMirror {
                extent,
                format: target.format,
                srv: srv.ok_or(Error::NoAdapter)?,
                texture,
                _token: record("surface_mirror", LedgerKind::Texture, extent.texels() * 4),
            });
        }
        let mirror = self.mirror.as_ref().ok_or(Error::NoAdapter)?;
        unsafe { context.CopyResource(&mirror.texture, target.texture) };
        Ok(mirror.srv.clone())
    }

    fn use_program(&self, context: &ID3D11DeviceContext1, program: &Program) {
        unsafe {
            context.IASetInputLayout(program.layout.as_ref());
            context.VSSetShader(&program.vertex, None);
            context.PSSetShader(&program.pixel, None);
            context.OMSetBlendState(self.pipelines.states.blend(program.blend), None, u32::MAX);
        }
    }

    fn instances_at(&self, context: &ID3D11DeviceContext1, offset: u64, stride: usize) {
        let buffers = [Some(self.instances.buffer.clone())];
        let strides = [stride as u32];
        let offsets = [offset as u32];
        unsafe {
            context.IASetVertexBuffers(
                0,
                1,
                Some(buffers.as_ptr()),
                Some(strides.as_ptr()),
                Some(offsets.as_ptr()),
            )
        };
    }

    fn reset_bindings(&self, context: &ID3D11DeviceContext1) {
        let views: [Option<ID3D11ShaderResourceView>; MAX_INPUTS] = Default::default();
        unsafe {
            context.PSSetShaderResources(0, Some(&views));
            context.OMSetRenderTargets(None, None::<&ID3D11DepthStencilView>);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &mut self,
        context: &ID3D11DeviceContext1,
        plan: &FramePlan,
        target: &SceneTarget,
        textures: &PersistentTextures,
        repaint: Option<RECT>,
    ) -> Result<SceneStats, Error> {
        let mut scratch = std::mem::take(&mut self.scratch);
        let outcome = self.record_with(context, plan, target, textures, repaint, &mut scratch);
        self.scratch = scratch;
        self.reset_bindings(context);
        if let Some(timestamps) = &mut self.timestamps {
            timestamps.end(context);
        }
        outcome
    }

    fn record_with(
        &mut self,
        context: &ID3D11DeviceContext1,
        plan: &FramePlan,
        target: &SceneTarget,
        textures: &PersistentTextures,
        repaint: Option<RECT>,
        scratch: &mut Scratch,
    ) -> Result<SceneStats, Error> {
        scratch.pack(plan);
        self.prepare_instances((scratch.bytes.len() as u64).max(16))?;
        if !scratch.bytes.is_empty() {
            write_buffer(context, &self.instances.buffer, &scratch.bytes, 0)?;
        }
        let precision = match target.format == DXGI_FORMAT_B8G8R8A8_UNORM {
            true => Precision::EightBit,
            false => Precision::Planned,
        };
        self.transients.ensure(&self.device, plan, precision)?;
        if let Some(timestamps) = &mut self.timestamps {
            timestamps.poll(context);
            timestamps.begin(context);
        }
        let mut stats = SceneStats {
            passes: plan.passes.len() as u32,
            instances: plan.instance_count() as u32,
            upload_bytes: scratch.bytes.len() as u64,
            ..SceneStats::default()
        };
        unsafe {
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            context.RSSetState(&self.pipelines.states.rasterizer);
            context.PSSetSamplers(0, Some(&[Some(self.pipelines.states.sampler.clone())]));
            context.VSSetConstantBuffers(0, Some(&[Some(self.constants.buffer.clone())]));
            context.PSSetConstantBuffers(0, Some(&[Some(self.constants.buffer.clone())]));
        }
        let first_surface_use = plan.passes.iter().find(|pass| {
            pass.reads.contains(&TextureRef::Surface) || pass.writes.contains(&TextureRef::Surface)
        });
        let clears_surface_first = first_surface_use.is_some_and(|pass| {
            pass.writes.first() == Some(&TextureRef::Surface)
                && matches!(pass.load, LoadOp::Clear(_))
        });
        if !clears_surface_first {
            clear(
                context,
                target.rtv,
                plan.clear.to_gpu(),
                confine(full(plan.extent), repaint),
                plan.extent,
            );
        }
        let empty: [Option<ID3D11ShaderResourceView>; MAX_INPUTS] = Default::default();
        for (pass_index, pass) in plan.passes.iter().enumerate() {
            if let Some(timestamps) = &mut self.timestamps {
                timestamps.mark(context, pass_label(&pass.kind));
            }
            let Some(&write) = pass.writes.first() else {
                continue;
            };
            let out = self.bind(write, target, target.extent, textures)?;
            let extent = out.extent;
            let scissor_bound = repaint.filter(|_| write == TextureRef::Surface);
            let mut views: [Option<ID3D11ShaderResourceView>; MAX_INPUTS] = Default::default();
            let mut source_extent = None;
            let mut source_format = None;
            let mut source_texture = None;
            for (slot, reference) in pass.reads.iter().take(MAX_INPUTS).enumerate() {
                let bound = self.bind(*reference, target, target.extent, textures)?;
                source_extent.get_or_insert(bound.extent);
                source_format.get_or_insert(bound.format);
                if slot == 0 {
                    source_texture = Some(bound.texture.clone());
                }
                views[slot] = bound.srv;
            }
            let needs_surface_view = pass.reads.contains(&TextureRef::Surface)
                && target.srv.is_none()
                && match &pass.kind {
                    PassKind::Copy { .. } => source_format != Some(out.format),
                    _ => true,
                };
            unsafe {
                context.PSSetShaderResources(0, Some(&empty));
                context.OMSetRenderTargets(None, None::<&ID3D11DepthStencilView>);
            }
            if needs_surface_view {
                let view = self.surface_view(context, target, target.extent)?;
                for (slot, reference) in pass.reads.iter().take(MAX_INPUTS).enumerate() {
                    if *reference == TextureRef::Surface {
                        views[slot] = Some(view.clone());
                    }
                }
            }
            let source_extent = source_extent.unwrap_or(extent);
            match &pass.kind {
                PassKind::Copy {
                    source_origin,
                    dest_origin,
                    size,
                } => {
                    let source_texture =
                        source_texture.ok_or(Error::Unsupported("copy without a source"))?;
                    match source_format == Some(out.format) {
                        true => {
                            let width = size
                                .width
                                .min(source_extent.width.saturating_sub(source_origin[0]))
                                .min(extent.width.saturating_sub(dest_origin[0]));
                            let height = size
                                .height
                                .min(source_extent.height.saturating_sub(source_origin[1]))
                                .min(extent.height.saturating_sub(dest_origin[1]));
                            if width > 0 && height > 0 {
                                let region = D3D11_BOX {
                                    left: source_origin[0],
                                    top: source_origin[1],
                                    front: 0,
                                    right: source_origin[0] + width,
                                    bottom: source_origin[1] + height,
                                    back: 1,
                                };
                                unsafe {
                                    context.CopySubresourceRegion(
                                        &out.texture,
                                        0,
                                        dest_origin[0],
                                        dest_origin[1],
                                        0,
                                        &source_texture,
                                        0,
                                        Some(&region),
                                    )
                                };
                                stats.draws += 1;
                            }
                        }
                        false => {
                            let rtv = out
                                .rtv
                                .clone()
                                .ok_or(Error::Unsupported("write to a persistent texture"))?;
                            let right = (dest_origin[0] + size.width).min(extent.width).min(
                                dest_origin[0]
                                    + source_extent.width.saturating_sub(source_origin[0]),
                            );
                            let bottom = (dest_origin[1] + size.height).min(extent.height).min(
                                dest_origin[1]
                                    + source_extent.height.saturating_sub(source_origin[1]),
                            );
                            if right > dest_origin[0] && bottom > dest_origin[1] {
                                let program = self.pipelines.get(Kind::Copy)?;
                                let offset = [
                                    source_origin[0] as f32 - dest_origin[0] as f32,
                                    source_origin[1] as f32 - dest_origin[1] as f32,
                                    0.0,
                                    0.0,
                                ];
                                self.write_constants(
                                    context,
                                    constants(extent, 1.0, offset, [0.0; 4], source_extent, None),
                                )?;
                                self.use_program(context, &program);
                                unsafe {
                                    context.OMSetRenderTargets(
                                        Some(&[Some(rtv)]),
                                        None::<&ID3D11DepthStencilView>,
                                    );
                                    viewport(context, extent);
                                    context.RSSetScissorRects(Some(&[confine(
                                        RECT {
                                            left: dest_origin[0] as i32,
                                            top: dest_origin[1] as i32,
                                            right: right as i32,
                                            bottom: bottom as i32,
                                        },
                                        scissor_bound,
                                    )]));
                                    context.PSSetShaderResources(0, Some(&views[..1]));
                                    context.Draw(4, 0);
                                }
                                stats.draws += 1;
                            }
                        }
                    }
                    continue;
                }
                PassKind::Compute {
                    shader: ShaderId::Custom(id),
                    groups,
                    params,
                } => {
                    let uav = match write {
                        TextureRef::Transient(_) => out.uav.clone(),
                        _ => None,
                    }
                    .ok_or(Error::Unsupported("compute output must be a transient"))?;
                    let shader = self.compute.get(*id)?;
                    write_buffer(
                        context,
                        &self.compute_parameters.buffer,
                        bytemuck::cast_slice(params),
                        0,
                    )?;
                    let inputs = pass.reads.len().min(MAX_INPUTS);
                    unsafe {
                        context.CSSetShader(&shader, None);
                        context.CSSetConstantBuffers(
                            0,
                            Some(&[Some(self.compute_parameters.buffer.clone())]),
                        );
                        context.CSSetShaderResources(0, Some(&views[..inputs.max(1)]));
                        let bound = [Some(uav)];
                        context.CSSetUnorderedAccessViews(0, 1, Some(bound.as_ptr()), None);
                        context.Dispatch(groups[0], groups[1], groups[2]);
                        let unbound: [Option<ID3D11UnorderedAccessView>; 1] = [None];
                        context.CSSetUnorderedAccessViews(0, 1, Some(unbound.as_ptr()), None);
                        context.CSSetShaderResources(0, Some(&empty[..inputs.max(1)]));
                    }
                    stats.draws += 1;
                    continue;
                }
                PassKind::Compute { .. } => {
                    return Err(Error::Unsupported("compute shaders other than custom ids"));
                }
                PassKind::Custom(spec) => {
                    let rtv = out
                        .rtv
                        .clone()
                        .ok_or(Error::Unsupported("write to a persistent texture"))?;
                    let bounds = confine(scissor_of(spec.bounds, extent), scissor_bound);
                    let vertex = self.custom.vertex()?;
                    let pixel = self.custom.pixel(spec)?;
                    unsafe {
                        context.OMSetRenderTargets(
                            Some(&[Some(rtv.clone())]),
                            None::<&ID3D11DepthStencilView>,
                        )
                    };
                    if let LoadOp::Clear(color) = pass.load {
                        unsafe { context.ClearRenderTargetView(&rtv, &color.to_gpu()) };
                    }
                    if bounds.right > bounds.left && bounds.bottom > bounds.top {
                        let bytes: &[u8] = bytemuck::cast_slice(&spec.params);
                        let size = (bytes.len() as u64)
                            .next_multiple_of(16)
                            .max(PARAMETER_BYTES);
                        if size > self.parameters.capacity {
                            self.parameters = dynamic_buffer(
                                &self.device,
                                size.next_power_of_two(),
                                D3D11_BIND_CONSTANT_BUFFER,
                                "constant_buffer",
                            )?;
                        }
                        write_buffer(
                            context,
                            &self.parameters.buffer,
                            bytes,
                            self.parameters.capacity as usize,
                        )?;
                        unsafe {
                            viewport(context, extent);
                            context.RSSetScissorRects(Some(&[bounds]));
                            context.IASetInputLayout(None);
                            context.VSSetShader(&vertex, None);
                            context.PSSetShader(&pixel, None);
                            context.OMSetBlendState(&self.pipelines.states.opaque, None, u32::MAX);
                            context.PSSetConstantBuffers(
                                0,
                                Some(&[Some(self.parameters.buffer.clone())]),
                            );
                            context.PSSetShaderResources(0, Some(&views));
                            context.Draw(4, 0);
                            context.PSSetConstantBuffers(
                                0,
                                Some(&[Some(self.constants.buffer.clone())]),
                            );
                        }
                        stats.draws += 1;
                    }
                    continue;
                }
                _ => {}
            }
            let rtv = out
                .rtv
                .clone()
                .ok_or(Error::Unsupported("write to a persistent texture"))?;
            let scissor = confine(full(extent), scissor_bound);
            unsafe {
                context.OMSetRenderTargets(
                    Some(&[Some(rtv.clone())]),
                    None::<&ID3D11DepthStencilView>,
                );
                viewport(context, extent);
                context.RSSetScissorRects(Some(&[scissor]));
            }
            if let LoadOp::Clear(color) = pass.load {
                clear(context, &rtv, color.to_gpu(), scissor, extent);
            }
            let reads = pass.reads.len().min(MAX_INPUTS);
            if reads > 0 {
                unsafe { context.PSSetShaderResources(0, Some(&views[..reads])) };
            }
            let tone = plan.text_tone;
            match (&pass.kind, scratch.uploads[pass_index]) {
                (
                    PassKind::Instanced {
                        shader: ShaderId::Quad,
                        ..
                    },
                    Upload::Quads { offset, count },
                ) => {
                    let program = self.pipelines.get(Kind::Quad)?;
                    self.write_constants(
                        context,
                        constants(extent, 1.0, [0.0; 4], [0.0; 4], source_extent, None),
                    )?;
                    self.use_program(context, &program);
                    self.instances_at(context, offset, QUAD_STRIDE);
                    unsafe { context.DrawInstanced(4, count, 0, 0) };
                    stats.draws += 1;
                }
                (PassKind::Shadows { .. }, Upload::Shadows { offset, count }) => {
                    let program = self.pipelines.get(Kind::Shadow)?;
                    self.write_constants(
                        context,
                        constants(extent, 1.0, [0.0; 4], [0.0; 4], source_extent, None),
                    )?;
                    self.use_program(context, &program);
                    self.instances_at(context, offset, SHADOW_STRIDE);
                    unsafe { context.DrawInstanced(4, count, 0, 0) };
                    stats.draws += 1;
                }
                (
                    PassKind::Fullscreen {
                        shader: ShaderId::LayerOver,
                        opacity,
                    },
                    _,
                ) => {
                    let program = self.pipelines.get(Kind::Layer)?;
                    self.write_constants(
                        context,
                        constants(extent, *opacity, [0.0; 4], [0.0; 4], source_extent, None),
                    )?;
                    self.use_program(context, &program);
                    unsafe { context.Draw(4, 0) };
                    stats.draws += 1;
                }
                (PassKind::Filter { shader, params }, _) => {
                    let kind = match shader {
                        ShaderId::BlurDown => Kind::BlurDown,
                        ShaderId::BlurUp => Kind::BlurUp,
                        ShaderId::ColorMatrix => Kind::ColorMatrix,
                        _ => return Err(Error::Unsupported("filter shader")),
                    };
                    let program = self.pipelines.get(kind)?;
                    let region = [
                        params.region.x,
                        params.region.y,
                        params.region.w,
                        params.region.h,
                    ];
                    let matrix = (*shader == ShaderId::ColorMatrix).then_some(&params.matrix.0);
                    self.write_constants(
                        context,
                        constants(extent, 1.0, region, params.values, source_extent, matrix),
                    )?;
                    self.use_program(context, &program);
                    unsafe { context.Draw(4, 0) };
                    stats.draws += 1;
                }
                (PassKind::Glyphs { .. }, Upload::Sprites { offset, count })
                | (PassKind::Slug { .. }, Upload::Sprites { offset, count }) => {
                    let kind = match pass.kind {
                        PassKind::Glyphs { .. } => Kind::Glyph,
                        _ => Kind::Slug,
                    };
                    let program = self.pipelines.get(kind)?;
                    self.write_constants(
                        context,
                        constants(
                            extent,
                            1.0,
                            tone.ratios(),
                            [tone.contrast(), 0.0, 0.0, 0.0],
                            source_extent,
                            None,
                        ),
                    )?;
                    self.use_program(context, &program);
                    self.instances_at(context, offset, SPRITE_STRIDE);
                    unsafe { context.DrawInstanced(4, count, 0, 0) };
                    stats.draws += 1;
                }
                (PassKind::Images { instances }, Upload::Sprites { offset, .. }) => {
                    self.write_constants(
                        context,
                        constants(extent, 1.0, [0.0; 4], [0.0; 4], source_extent, None),
                    )?;
                    let mut start = 0usize;
                    while start < instances.len() {
                        let first = &instances[start];
                        let end = instances[start..]
                            .iter()
                            .position(|next| {
                                next.source != first.source || next.blend != first.blend
                            })
                            .map_or(instances.len(), |run| start + run);
                        let kind = match first.blend {
                            Blend::Over => Kind::Image,
                            Blend::Replace => Kind::ImageReplace,
                        };
                        let program = self.pipelines.get(kind)?;
                        self.use_program(context, &program);
                        let position = pass
                            .reads
                            .iter()
                            .position(|read| *read == first.source)
                            .unwrap_or(0)
                            .min(MAX_INPUTS - 1);
                        unsafe {
                            context.PSSetShaderResources(0, Some(&views[position..position + 1]))
                        };
                        self.instances_at(
                            context,
                            offset + start as u64 * SPRITE_STRIDE as u64,
                            SPRITE_STRIDE,
                        );
                        unsafe { context.DrawInstanced(4, (end - start) as u32, 0, 0) };
                        stats.draws += 1;
                        start = end;
                    }
                }
                (
                    PassKind::Instanced { .. }
                    | PassKind::Shadows { .. }
                    | PassKind::Glyphs { .. }
                    | PassKind::Slug { .. }
                    | PassKind::Images { .. },
                    Upload::None,
                ) => {}
                _ => return Err(Error::Unsupported("pass kind")),
            }
        }
        Ok(stats)
    }
}
