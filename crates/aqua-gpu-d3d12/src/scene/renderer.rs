use std::collections::{HashMap, HashSet};

use aqua_graph::{
    Barrier, Extent, FramePlan, LoadOp, PassKind, ResourceState, ShaderId, TextureHandle,
    TextureRef, TransientId,
};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::Error;
use super::formats::{dxgi, state};
use super::pipeline::{CONSTANT_DWORDS, Kind, PipelineStats, Pipelines};
use super::sprites;
use super::transients::Transients;
use super::util::*;
use crate::pack::{QUAD_STRIDE, QuadGpu, SHADOW_STRIDE, ShadowGpu, pack_quad, pack_shadow};
use crate::sprite::{SpriteGpu, pack_glyphs, pack_image};

const INITIAL_UPLOAD_BYTES: u64 = 1 << 16;
const INITIAL_DESCRIPTORS: u32 = 8;

pub struct SceneTarget<'a> {
    pub resource: &'a ID3D12Resource,
    pub rtv: D3D12_CPU_DESCRIPTOR_HANDLE,
    pub format: DXGI_FORMAT,
    pub rest_state: D3D12_RESOURCE_STATES,
}

pub struct PersistentView {
    pub resource: ID3D12Resource,
    pub format: DXGI_FORMAT,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneStats {
    pub passes: u32,
    pub instances: u32,
    pub draws: u32,
    pub barriers: u32,
    pub upload_bytes: u64,
}

struct Slot {
    upload: ID3D12Resource,
    mapped: *mut u8,
    capacity: u64,
    srv_heap: ID3D12DescriptorHeap,
    srv_capacity: u32,
}

pub struct SceneRenderer {
    device: ID3D12Device,
    pipelines: Pipelines,
    slots: Vec<Slot>,
    rtv_heap: ID3D12DescriptorHeap,
    rtv_capacity: u32,
    rtv_stride: usize,
    srv_stride: usize,
    transients: Transients,
}

fn create_srv_heap(device: &ID3D12Device, count: u32) -> Result<ID3D12DescriptorHeap, Error> {
    Ok(unsafe {
        device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
            Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
            NumDescriptors: count,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
            NodeMask: 0,
        })?
    })
}

fn create_rtv_heap(device: &ID3D12Device, count: u32) -> Result<ID3D12DescriptorHeap, Error> {
    Ok(unsafe {
        device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
            Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
            NumDescriptors: count,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
            NodeMask: 0,
        })?
    })
}

fn create_slot(device: &ID3D12Device, capacity: u64, descriptors: u32) -> Result<Slot, Error> {
    let upload = create_buffer(
        device,
        D3D12_HEAP_TYPE_UPLOAD,
        capacity,
        D3D12_RESOURCE_STATE_GENERIC_READ,
    )?;
    let mapped = map_whole(&upload)?;
    Ok(Slot {
        upload,
        mapped,
        capacity,
        srv_heap: create_srv_heap(device, descriptors)?,
        srv_capacity: descriptors,
    })
}

fn srv_desc(format: DXGI_FORMAT) -> D3D12_SHADER_RESOURCE_VIEW_DESC {
    D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: format,
        ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
            Texture2D: D3D12_TEX2D_SRV {
                MostDetailedMip: 0,
                MipLevels: 1,
                PlaneSlice: 0,
                ResourceMinLODClamp: 0.0,
            },
        },
    }
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

fn append<T: bytemuck::Pod>(bytes: &mut Vec<u8>, items: &[T]) -> u64 {
    bytes.resize(align16(bytes.len()), 0);
    let offset = bytes.len() as u64;
    bytes.extend_from_slice(bytemuck::cast_slice(items));
    offset
}

fn pack_passes(plan: &FramePlan) -> (Vec<u8>, Vec<Upload>) {
    let mut bytes: Vec<u8> = Vec::new();
    let uploads = plan
        .passes
        .iter()
        .map(|pass| match &pass.kind {
            PassKind::Instanced { instances, .. } if !instances.is_empty() => {
                let packed: Vec<QuadGpu> = instances.iter().map(pack_quad).collect();
                Upload::Quads {
                    offset: append(&mut bytes, &packed),
                    count: packed.len() as u32,
                }
            }
            PassKind::Shadows { instances } if !instances.is_empty() => {
                let packed: Vec<ShadowGpu> = instances.iter().map(pack_shadow).collect();
                Upload::Shadows {
                    offset: append(&mut bytes, &packed),
                    count: packed.len() as u32,
                }
            }
            PassKind::Glyphs { instances, .. } if !instances.is_empty() => {
                let packed: Vec<SpriteGpu> = pack_glyphs(instances);
                Upload::Sprites {
                    offset: append(&mut bytes, &packed),
                    count: packed.len() as u32,
                }
            }
            PassKind::Images { instances } if !instances.is_empty() => {
                let packed: Vec<SpriteGpu> = instances.iter().map(pack_image).collect();
                Upload::Sprites {
                    offset: append(&mut bytes, &packed),
                    count: packed.len() as u32,
                }
            }
            _ => Upload::None,
        })
        .collect();
    (bytes, uploads)
}

struct Bound {
    resource: ID3D12Resource,
    rtv: Option<D3D12_CPU_DESCRIPTOR_HANDLE>,
    format: DXGI_FORMAT,
    extent: Extent,
}

fn constants(
    target: Extent,
    opacity: f32,
    region: [f32; 4],
    values: [f32; 4],
    source: Extent,
) -> [f32; 16] {
    [
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
    ]
}

impl SceneRenderer {
    pub fn new(device: &ID3D12Device, slots: usize) -> Result<Self, Error> {
        Self::with_pipeline_cache(device, slots, None)
    }

    pub fn with_pipeline_cache(
        device: &ID3D12Device,
        slots: usize,
        cache: Option<&[u8]>,
    ) -> Result<Self, Error> {
        let pipelines = Pipelines::create(device, cache)?;
        let slots = (0..slots.max(1))
            .map(|_| create_slot(device, INITIAL_UPLOAD_BYTES, INITIAL_DESCRIPTORS))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            device: device.clone(),
            pipelines,
            slots,
            rtv_heap: create_rtv_heap(device, INITIAL_DESCRIPTORS)?,
            rtv_capacity: INITIAL_DESCRIPTORS,
            rtv_stride: unsafe {
                device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV)
            } as usize,
            srv_stride: unsafe {
                device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
            } as usize,
            transients: Transients::new(),
        })
    }

    pub fn retire(&mut self, completed_fence: u64) {
        self.transients.retire(completed_fence);
    }

    pub fn warm(&mut self, formats: &[DXGI_FORMAT]) -> Result<(), Error> {
        self.pipelines.warm(formats)
    }

    pub fn pipeline_stats(&self) -> PipelineStats {
        self.pipelines.stats()
    }

    pub fn pipeline_cache(&self) -> Option<Vec<u8>> {
        self.pipelines.serialize()
    }

    pub fn transient_heap_bytes(&self) -> u64 {
        self.transients.heap_bytes()
    }

    fn prepare_slot(
        &mut self,
        slot: usize,
        upload_bytes: u64,
        descriptors: u32,
    ) -> Result<(), Error> {
        let current = &self.slots[slot];
        let capacity = match current.capacity < upload_bytes {
            true => upload_bytes.next_power_of_two(),
            false => current.capacity,
        };
        let srv_capacity = match current.srv_capacity < descriptors {
            true => descriptors.next_power_of_two(),
            false => current.srv_capacity,
        };
        if capacity != current.capacity || srv_capacity != current.srv_capacity {
            self.slots[slot] = create_slot(&self.device, capacity, srv_capacity)?;
        }
        if self.rtv_capacity < descriptors {
            self.rtv_capacity = descriptors.next_power_of_two();
            self.rtv_heap = create_rtv_heap(&self.device, self.rtv_capacity)?;
        }
        Ok(())
    }

    fn rtv_at(&self, index: u32) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        let start = unsafe { self.rtv_heap.GetCPUDescriptorHandleForHeapStart() };
        D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: start.ptr + index as usize * self.rtv_stride,
        }
    }

    pub fn record(
        &mut self,
        list: &ID3D12GraphicsCommandList,
        slot: usize,
        frame_fence: u64,
        plan: &FramePlan,
        target: &SceneTarget,
        persistent: &dyn Fn(TextureHandle) -> Option<PersistentView>,
    ) -> Result<SceneStats, Error> {
        let (bytes, uploads) = pack_passes(plan);
        let read_total: u32 = plan.passes.iter().map(|pass| pass.reads.len() as u32).sum();
        let descriptors = (plan.textures.len() as u32 + 1).max(read_total + 1);
        self.prepare_slot(slot, (bytes.len() as u64).max(16), descriptors)?;
        self.transients.ensure(&self.device, plan, frame_fence)?;
        let slot_data = &self.slots[slot];
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), slot_data.mapped, bytes.len()) };
        for (index, entry) in self.transients.entries().iter().enumerate() {
            unsafe {
                self.device.CreateRenderTargetView(
                    &entry.resource,
                    None,
                    self.rtv_at(index as u32 + 1),
                )
            };
        }
        let srv_start = unsafe { slot_data.srv_heap.GetCPUDescriptorHandleForHeapStart() };
        let srv_gpu = unsafe { slot_data.srv_heap.GetGPUDescriptorHandleForHeapStart() };
        let upload_address = unsafe { slot_data.upload.GetGPUVirtualAddress() };
        let heap = slot_data.srv_heap.clone();
        let null_index = descriptors - 1;
        unsafe {
            self.device.CreateShaderResourceView(
                None::<&ID3D12Resource>,
                Some(&srv_desc(DXGI_FORMAT_R8G8B8A8_UNORM)),
                D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: srv_start.ptr + null_index as usize * self.srv_stride,
                },
            )
        };
        let table = |index: u32| D3D12_GPU_DESCRIPTOR_HANDLE {
            ptr: srv_gpu.ptr + index as u64 * self.srv_stride as u64,
        };

        let mut tracked: HashMap<TextureRef, D3D12_RESOURCE_STATES> = HashMap::new();
        tracked.insert(TextureRef::Surface, target.rest_state);
        for texture in &plan.textures {
            if let Some(index) = self.transients.position(texture.id) {
                tracked.insert(
                    TextureRef::Transient(texture.id),
                    self.transients.entries()[index].state,
                );
            }
        }
        let mut activated: HashSet<TransientId> = HashSet::new();
        let mut stats = SceneStats {
            passes: plan.passes.len() as u32,
            instances: plan.instance_count() as u32,
            upload_bytes: bytes.len() as u64,
            ..SceneStats::default()
        };
        let surface_bound = Bound {
            resource: target.resource.clone(),
            rtv: Some(target.rtv),
            format: target.format,
            extent: plan.extent,
        };
        let bind = |reference: TextureRef, this: &SceneRenderer| -> Result<Bound, Error> {
            match reference {
                TextureRef::Surface => Ok(Bound {
                    resource: surface_bound.resource.clone(),
                    rtv: surface_bound.rtv,
                    format: surface_bound.format,
                    extent: surface_bound.extent,
                }),
                TextureRef::Transient(id) => {
                    let index = this
                        .transients
                        .position(id)
                        .ok_or(Error::Unsupported("unknown transient"))?;
                    let entry = &this.transients.entries()[index];
                    Ok(Bound {
                        resource: entry.resource.clone(),
                        rtv: Some(this.rtv_at(index as u32 + 1)),
                        format: dxgi(entry.format),
                        extent: entry.extent,
                    })
                }
                TextureRef::Persistent(handle) => {
                    let view = persistent(handle)
                        .ok_or(Error::Unsupported("persistent texture is not resident"))?;
                    Ok(Bound {
                        resource: view.resource,
                        rtv: None,
                        format: view.format,
                        extent: Extent::new(0, 0),
                    })
                }
            }
        };

        unsafe {
            list.SetGraphicsRootSignature(&self.pipelines.root_signature);
            list.SetDescriptorHeaps(&[Some(heap)]);
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
        }
        let first_surface_use = plan.passes.iter().find(|pass| {
            pass.reads.contains(&TextureRef::Surface) || pass.writes.contains(&TextureRef::Surface)
        });
        let clears_surface_first = first_surface_use.is_some_and(|pass| {
            pass.writes.first() == Some(&TextureRef::Surface)
                && matches!(pass.load, LoadOp::Clear(_))
        });
        if !clears_surface_first {
            unsafe {
                if target.rest_state != D3D12_RESOURCE_STATE_RENDER_TARGET {
                    list.ResourceBarrier(&[transition(
                        target.resource,
                        target.rest_state,
                        D3D12_RESOURCE_STATE_RENDER_TARGET,
                    )]);
                    stats.barriers += 1;
                }
                list.ClearRenderTargetView(target.rtv, &plan.clear.to_gpu(), None);
            }
            tracked.insert(TextureRef::Surface, D3D12_RESOURCE_STATE_RENDER_TARGET);
        }
        let mut next_descriptor = 0u32;
        for (pass_index, pass) in plan.passes.iter().enumerate() {
            let mut barriers: Vec<D3D12_RESOURCE_BARRIER> = Vec::new();
            for reference in pass.reads.iter().chain(&pass.writes) {
                if let TextureRef::Transient(id) = reference {
                    if activated.insert(*id) {
                        let bound = bind(*reference, self)?;
                        barriers.push(aliasing(&bound.resource));
                    }
                }
            }
            let copy_via_shader = match (&pass.kind, pass.reads.first(), pass.writes.first()) {
                (PassKind::Copy { .. }, Some(read), Some(write)) => {
                    bind(*read, self)?.format != bind(*write, self)?.format
                }
                _ => false,
            };
            for barrier in &pass.barriers {
                match barrier {
                    Barrier::Alias { after, .. } => {
                        let bound = bind(TextureRef::Transient(*after), self)?;
                        barriers.push(aliasing(&bound.resource));
                    }
                    Barrier::Transition { resource, to, .. } => {
                        let to = match (copy_via_shader, to) {
                            (true, ResourceState::CopySource) => &ResourceState::ShaderRead,
                            (true, ResourceState::CopyDest) => &ResourceState::RenderTarget,
                            _ => to,
                        };
                        let Some(after) = state(*to) else {
                            continue;
                        };
                        let bound = bind(*resource, self)?;
                        let before = tracked
                            .get(resource)
                            .copied()
                            .unwrap_or(D3D12_RESOURCE_STATE_COMMON);
                        if before != after {
                            barriers.push(transition(&bound.resource, before, after));
                            tracked.insert(*resource, after);
                        }
                    }
                }
            }
            if !barriers.is_empty() {
                stats.barriers += barriers.len() as u32;
                unsafe { list.ResourceBarrier(&barriers) };
            }
            let Some(&write) = pass.writes.first() else {
                continue;
            };
            let write_bound = bind(write, self)?;
            let read_bounds: Vec<Bound> = pass
                .reads
                .iter()
                .map(|reference| bind(*reference, self))
                .collect::<Result<_, _>>()?;
            let first_descriptor = next_descriptor;
            for read in &read_bounds {
                let handle = D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: srv_start.ptr + next_descriptor as usize * self.srv_stride,
                };
                unsafe {
                    self.device.CreateShaderResourceView(
                        &read.resource,
                        Some(&srv_desc(read.format)),
                        handle,
                    )
                };
                next_descriptor += 1;
            }
            let bound_table = match read_bounds.is_empty() {
                true => table(null_index),
                false => table(first_descriptor),
            };
            let extent = write_bound.extent;
            match &pass.kind {
                PassKind::Copy {
                    source_origin,
                    dest_origin,
                    size,
                } if copy_via_shader => {
                    let source = read_bounds
                        .first()
                        .ok_or(Error::Unsupported("copy without a source"))?;
                    let rtv = write_bound
                        .rtv
                        .ok_or(Error::Unsupported("write to a persistent texture"))?;
                    let right = (dest_origin[0] + size.width)
                        .min(extent.width)
                        .min(dest_origin[0] + source.extent.width.saturating_sub(source_origin[0]));
                    let bottom = (dest_origin[1] + size.height).min(extent.height).min(
                        dest_origin[1] + source.extent.height.saturating_sub(source_origin[1]),
                    );
                    if right > dest_origin[0] && bottom > dest_origin[1] {
                        unsafe {
                            list.OMSetRenderTargets(1, Some(&rtv), false, None);
                            list.RSSetViewports(&[D3D12_VIEWPORT {
                                TopLeftX: 0.0,
                                TopLeftY: 0.0,
                                Width: extent.width as f32,
                                Height: extent.height as f32,
                                MinDepth: 0.0,
                                MaxDepth: 1.0,
                            }]);
                            list.RSSetScissorRects(&[RECT {
                                left: dest_origin[0] as i32,
                                top: dest_origin[1] as i32,
                                right: right as i32,
                                bottom: bottom as i32,
                            }]);
                            list.SetPipelineState(
                                &self.pipelines.get(Kind::Copy, write_bound.format)?,
                            );
                            let offset = [
                                source_origin[0] as f32 - dest_origin[0] as f32,
                                source_origin[1] as f32 - dest_origin[1] as f32,
                                0.0,
                                0.0,
                            ];
                            let values = constants(extent, 1.0, offset, [0.0; 4], source.extent);
                            list.SetGraphicsRoot32BitConstants(
                                0,
                                CONSTANT_DWORDS,
                                values.as_ptr() as *const _,
                                0,
                            );
                            list.SetGraphicsRootDescriptorTable(1, bound_table);
                            list.DrawInstanced(4, 1, 0, 0);
                        }
                        stats.draws += 1;
                    }
                    continue;
                }
                PassKind::Copy {
                    source_origin,
                    dest_origin,
                    size,
                } => {
                    let source = read_bounds
                        .first()
                        .ok_or(Error::Unsupported("copy without a source"))?;
                    let width = size
                        .width
                        .min(source.extent.width.saturating_sub(source_origin[0]));
                    let height = size
                        .height
                        .min(source.extent.height.saturating_sub(source_origin[1]));
                    let width = width.min(extent.width.saturating_sub(dest_origin[0]));
                    let height = height.min(extent.height.saturating_sub(dest_origin[1]));
                    if width > 0 && height > 0 {
                        let region = D3D12_BOX {
                            left: source_origin[0],
                            top: source_origin[1],
                            front: 0,
                            right: source_origin[0] + width,
                            bottom: source_origin[1] + height,
                            back: 1,
                        };
                        unsafe {
                            list.CopyTextureRegion(
                                &texture_location(&write_bound.resource),
                                dest_origin[0],
                                dest_origin[1],
                                0,
                                &texture_location(&source.resource),
                                Some(&region),
                            )
                        };
                        stats.draws += 1;
                    }
                    continue;
                }
                PassKind::Compute { .. } | PassKind::Custom(_) => {
                    return Err(Error::Unsupported("compute and custom passes"));
                }
                _ => {}
            }
            let rtv = write_bound
                .rtv
                .ok_or(Error::Unsupported("write to a persistent texture"))?;
            unsafe {
                list.OMSetRenderTargets(1, Some(&rtv), false, None);
                list.RSSetViewports(&[D3D12_VIEWPORT {
                    TopLeftX: 0.0,
                    TopLeftY: 0.0,
                    Width: extent.width as f32,
                    Height: extent.height as f32,
                    MinDepth: 0.0,
                    MaxDepth: 1.0,
                }]);
                list.RSSetScissorRects(&[RECT {
                    left: 0,
                    top: 0,
                    right: extent.width as i32,
                    bottom: extent.height as i32,
                }]);
                if let LoadOp::Clear(color) = pass.load {
                    list.ClearRenderTargetView(rtv, &color.to_gpu(), None);
                }
                list.SetGraphicsRootDescriptorTable(1, bound_table);
            }
            let source_extent = read_bounds
                .first()
                .map(|bound| bound.extent)
                .unwrap_or(extent);
            match (&pass.kind, uploads[pass_index]) {
                (
                    PassKind::Instanced {
                        shader: ShaderId::Quad,
                        ..
                    },
                    Upload::Quads { offset, count },
                ) => unsafe {
                    list.SetPipelineState(&self.pipelines.get(Kind::Quad, write_bound.format)?);
                    let values = constants(extent, 1.0, [0.0; 4], [0.0; 4], source_extent);
                    list.SetGraphicsRoot32BitConstants(
                        0,
                        CONSTANT_DWORDS,
                        values.as_ptr() as *const _,
                        0,
                    );
                    list.IASetVertexBuffers(
                        0,
                        Some(&[D3D12_VERTEX_BUFFER_VIEW {
                            BufferLocation: upload_address + offset,
                            SizeInBytes: count * QUAD_STRIDE as u32,
                            StrideInBytes: QUAD_STRIDE as u32,
                        }]),
                    );
                    list.DrawInstanced(4, count, 0, 0);
                    stats.draws += 1;
                },
                (PassKind::Shadows { .. }, Upload::Shadows { offset, count }) => unsafe {
                    list.SetPipelineState(&self.pipelines.get(Kind::Shadow, write_bound.format)?);
                    let values = constants(extent, 1.0, [0.0; 4], [0.0; 4], source_extent);
                    list.SetGraphicsRoot32BitConstants(
                        0,
                        CONSTANT_DWORDS,
                        values.as_ptr() as *const _,
                        0,
                    );
                    list.IASetVertexBuffers(
                        0,
                        Some(&[D3D12_VERTEX_BUFFER_VIEW {
                            BufferLocation: upload_address + offset,
                            SizeInBytes: count * SHADOW_STRIDE as u32,
                            StrideInBytes: SHADOW_STRIDE as u32,
                        }]),
                    );
                    list.DrawInstanced(4, count, 0, 0);
                    stats.draws += 1;
                },
                (
                    PassKind::Fullscreen {
                        shader: ShaderId::LayerOver,
                        opacity,
                    },
                    _,
                ) => unsafe {
                    list.SetPipelineState(&self.pipelines.get(Kind::Layer, write_bound.format)?);
                    let values = constants(extent, *opacity, [0.0; 4], [0.0; 4], source_extent);
                    list.SetGraphicsRoot32BitConstants(
                        0,
                        CONSTANT_DWORDS,
                        values.as_ptr() as *const _,
                        0,
                    );
                    list.DrawInstanced(4, 1, 0, 0);
                    stats.draws += 1;
                },
                (PassKind::Filter { shader, params }, _) => {
                    let kind = match shader {
                        ShaderId::BlurDown => Kind::BlurDown,
                        ShaderId::BlurUp => Kind::BlurUp,
                        _ => return Err(Error::Unsupported("filter shader")),
                    };
                    unsafe {
                        list.SetPipelineState(&self.pipelines.get(kind, write_bound.format)?);
                        let region = [
                            params.region.x,
                            params.region.y,
                            params.region.w,
                            params.region.h,
                        ];
                        let values = constants(extent, 1.0, region, params.values, source_extent);
                        list.SetGraphicsRoot32BitConstants(
                            0,
                            CONSTANT_DWORDS,
                            values.as_ptr() as *const _,
                            0,
                        );
                        list.DrawInstanced(4, 1, 0, 0);
                    }
                    stats.draws += 1;
                }
                (PassKind::Glyphs { .. }, Upload::Sprites { offset, count }) => unsafe {
                    list.SetPipelineState(&self.pipelines.get(Kind::Glyph, write_bound.format)?);
                    let values = constants(extent, 1.0, [0.0; 4], [0.0; 4], source_extent);
                    list.SetGraphicsRoot32BitConstants(
                        0,
                        CONSTANT_DWORDS,
                        values.as_ptr() as *const _,
                        0,
                    );
                    sprites::draw(list, upload_address + offset, 0..count);
                    stats.draws += 1;
                },
                (PassKind::Images { instances }, Upload::Sprites { offset, .. }) => unsafe {
                    list.SetPipelineState(&self.pipelines.get(Kind::Image, write_bound.format)?);
                    let values = constants(extent, 1.0, [0.0; 4], [0.0; 4], source_extent);
                    list.SetGraphicsRoot32BitConstants(
                        0,
                        CONSTANT_DWORDS,
                        values.as_ptr() as *const _,
                        0,
                    );
                    for (source, range) in sprites::image_runs(instances) {
                        let position = pass.reads.iter().position(|read| *read == source);
                        list.SetGraphicsRootDescriptorTable(
                            1,
                            table(first_descriptor + position.unwrap_or(0) as u32),
                        );
                        sprites::draw(list, upload_address + offset, range);
                        stats.draws += 1;
                    }
                },
                (
                    PassKind::Instanced { .. }
                    | PassKind::Shadows { .. }
                    | PassKind::Glyphs { .. }
                    | PassKind::Images { .. },
                    Upload::None,
                ) => {}
                _ => return Err(Error::Unsupported("pass kind")),
            }
        }
        let mut closing: Vec<D3D12_RESOURCE_BARRIER> = Vec::new();
        for barrier in &plan.final_barriers {
            if let Barrier::Transition { resource, to, .. } = barrier {
                if let Some(after) = state(*to) {
                    let bound = bind(*resource, self)?;
                    let before = tracked
                        .get(resource)
                        .copied()
                        .unwrap_or(D3D12_RESOURCE_STATE_COMMON);
                    if before != after {
                        closing.push(transition(&bound.resource, before, after));
                        tracked.insert(*resource, after);
                    }
                }
            }
        }
        let surface_now = tracked
            .get(&TextureRef::Surface)
            .copied()
            .unwrap_or(target.rest_state);
        if surface_now != target.rest_state {
            closing.push(transition(target.resource, surface_now, target.rest_state));
            tracked.insert(TextureRef::Surface, target.rest_state);
        }
        if !closing.is_empty() {
            stats.barriers += closing.len() as u32;
            unsafe { list.ResourceBarrier(&closing) };
        }
        for (reference, final_state) in &tracked {
            if let TextureRef::Transient(id) = reference {
                if let Some(index) = self.transients.position(*id) {
                    self.transients.entries_mut()[index].state = *final_state;
                }
            }
        }
        Ok(stats)
    }
}
