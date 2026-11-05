use std::collections::{HashMap, HashSet};

use aqua_graph::{
    Barrier, Blend, Extent, FramePlan, LoadOp, PassKind, ResourceState, ShaderId, TextureHandle,
    TextureRef, TransientId,
};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::Error;
use super::barriers::Emitter;
use super::compute::{self, ComputePipelines};
use super::custom::{self, CustomDraw, CustomPipelines};
use super::formats::{dxgi, state_for};
use super::pipeline::{Kind, PipelineStats, Pipelines};
use super::sprites;
use super::tiers::{Active, SceneOptions, resolve};
use super::timestamps::{PassTime, Timestamps};
use super::transients::Transients;
use super::util::*;
use crate::ledger::{self, Token};
use crate::pack::{QUAD_STRIDE, SHADOW_STRIDE, pack_quad, pack_shadow};
use crate::sprite::{glyph_packer, pack_image, pack_slug};

const INITIAL_UPLOAD_BYTES: u64 = 1 << 16;
const INITIAL_DESCRIPTORS: u32 = 8;

pub struct SceneTarget<'a> {
    pub resource: &'a ID3D12Resource,
    pub rtv: D3D12_CPU_DESCRIPTOR_HANDLE,
    pub format: DXGI_FORMAT,
    pub rest_state: D3D12_RESOURCE_STATES,
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
    _tokens: [Token; 2],
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

fn ensure_custom<'a>(
    slot: &'a mut Option<CustomPipelines>,
    device: &ID3D12Device,
) -> Result<&'a mut CustomPipelines, Error> {
    if slot.is_none() {
        *slot = Some(CustomPipelines::create(device)?);
    }
    slot.as_mut().ok_or(Error::Unsupported("custom pipelines"))
}

fn ensure_compute<'a>(
    slot: &'a mut Option<ComputePipelines>,
    device: &ID3D12Device,
) -> Result<&'a mut ComputePipelines, Error> {
    if slot.is_none() {
        *slot = Some(ComputePipelines::create(device)?);
    }
    slot.as_mut().ok_or(Error::Unsupported("compute pipelines"))
}

pub struct SceneRenderer {
    device: ID3D12Device,
    pipelines: Pipelines,
    custom: Option<CustomPipelines>,
    compute: Option<ComputePipelines>,
    emitter: Emitter,
    scratch: Scratch,
    slots: Vec<Slot>,
    rtv_heap: ID3D12DescriptorHeap,
    rtv_capacity: u32,
    rtv_stride: usize,
    srv_stride: usize,
    transients: Transients,
    active: Active,
    timestamps: Option<Timestamps>,
    trim_slots: Vec<bool>,
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
    let srv_stride =
        unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) }
            as u64;
    Ok(Slot {
        _tokens: [
            ledger::record("scene_upload", ledger::Kind::UploadBuffer, capacity),
            ledger::record(
                "scene_srv_heap",
                ledger::Kind::DescriptorHeap,
                descriptors as u64 * srv_stride,
            ),
        ],
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
    Parameters { offset: u64 },
}

fn pass_descriptors(pass: &aqua_graph::Pass) -> u32 {
    match &pass.kind {
        PassKind::Custom(_) => custom::MAX_INPUTS,
        PassKind::Compute { .. } => compute::DESCRIPTORS,
        _ => pass.reads.len() as u32,
    }
}

fn align16(value: usize) -> usize {
    value.div_ceil(16) * 16
}

fn append_all<T: bytemuck::Pod>(bytes: &mut Vec<u8>, items: impl Iterator<Item = T>) -> (u64, u32) {
    bytes.resize(align16(bytes.len()), 0);
    let offset = bytes.len() as u64;
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
    tracked: HashMap<TextureRef, D3D12_RESOURCE_STATES>,
    activated: HashSet<TransientId>,
    reads: Vec<Bound>,
}

impl Scratch {
    fn pack(&mut self, plan: &FramePlan) {
        self.bytes.clear();
        self.uploads.clear();
        let bytes = &mut self.bytes;
        for pass in &plan.passes {
            let upload = match &pass.kind {
                PassKind::Instanced { instances, .. } if !instances.is_empty() => {
                    let (offset, count) = append_all(bytes, instances.iter().map(pack_quad));
                    Upload::Quads { offset, count }
                }
                PassKind::Shadows { instances } if !instances.is_empty() => {
                    let (offset, count) = append_all(bytes, instances.iter().map(pack_shadow));
                    Upload::Shadows { offset, count }
                }
                PassKind::Glyphs { instances, .. } if !instances.is_empty() => {
                    let (offset, count) = append_all(bytes, instances.iter().map(glyph_packer()));
                    Upload::Sprites { offset, count }
                }
                PassKind::Slug { instances, .. } if !instances.is_empty() => {
                    let (offset, count) = append_all(bytes, instances.iter().map(pack_slug));
                    Upload::Sprites { offset, count }
                }
                PassKind::Custom(spec) => Upload::Parameters {
                    offset: custom::pack_parameters(bytes, &spec.params),
                },
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
        Self::with_options(device, slots, cache, SceneOptions::default())
    }

    pub fn with_options(
        device: &ID3D12Device,
        slots: usize,
        cache: Option<&[u8]>,
        options: SceneOptions,
    ) -> Result<Self, Error> {
        let stages = crate::stages::StageClock::from_env();
        let active = resolve(device, options.limit);
        let pipelines = Pipelines::create(device, cache, active.binding)?;
        stages.mark("      pipelines_create");
        let timestamps = match options.timestamps {
            true => Timestamps::new(device, slots.max(1))?,
            false => None,
        };
        let slot_count = slots.max(1);
        let slots = (0..slot_count)
            .map(|_| create_slot(device, INITIAL_UPLOAD_BYTES, INITIAL_DESCRIPTORS))
            .collect::<Result<Vec<_>, _>>()?;
        stages.mark("      timestamps_slots");
        Ok(Self {
            device: device.clone(),
            pipelines,
            emitter: Emitter::new(active.sync),
            scratch: Scratch::default(),
            custom: None,
            compute: None,
            slots,
            rtv_heap: create_rtv_heap(device, INITIAL_DESCRIPTORS)?,
            rtv_capacity: INITIAL_DESCRIPTORS,
            rtv_stride: unsafe {
                device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV)
            } as usize,
            srv_stride: unsafe {
                device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
            } as usize,
            transients: Transients::new(device, active.sync, options.transient_ceiling)?,
            active,
            timestamps,
            trim_slots: vec![false; slot_count],
        })
    }

    pub fn retire(&mut self, completed_fence: u64) {
        self.transients.retire(completed_fence);
        if let Some(timestamps) = &mut self.timestamps {
            timestamps.poll(completed_fence);
        }
    }

    pub fn tiers(&self) -> crate::hal::tier::Tiers {
        self.active.tiers
    }

    pub fn gpu_pass_times(&self) -> &[PassTime] {
        self.timestamps
            .as_ref()
            .map(Timestamps::latest_passes)
            .unwrap_or(&[])
    }

    pub fn gpu_time(&self) -> Option<std::time::Duration> {
        self.timestamps.as_ref().and_then(Timestamps::latest)
    }

    pub fn slot_upload_bytes(&self) -> u64 {
        self.slots.iter().map(|slot| slot.capacity).sum()
    }

    pub fn retired_transient_bytes(&self) -> u64 {
        self.transients.retired_bytes()
    }

    pub fn trim(&mut self, trim: &crate::hal::budget::Trim, frame_fence: u64) -> u64 {
        if trim.shrink_upload_ring {
            self.trim_slots
                .iter_mut()
                .for_each(|pending| *pending = true);
        }
        match trim.free_transients {
            true => self.transients.trim(frame_fence),
            false => 0,
        }
    }

    pub fn reserve(&mut self, plan: &FramePlan, frame_fence: u64) -> Result<bool, Error> {
        self.transients.ensure(&self.device, plan, frame_fence)
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
        ensure_compute(&mut self.compute, &self.device)?.register(id, hlsl, entry)
    }

    pub fn warm(&mut self, formats: &[DXGI_FORMAT]) -> Result<(), Error> {
        self.pipelines.warm(formats)
    }

    pub fn warm_first_frame(&mut self, formats: &[DXGI_FORMAT]) -> Result<(), Error> {
        self.pipelines.warm_first_frame(formats)
    }

    pub fn close_startup(&mut self) {
        self.pipelines.close_startup();
    }

    pub fn deferred_pipelines(&self, formats: &[DXGI_FORMAT]) -> Vec<(Kind, DXGI_FORMAT)> {
        self.pipelines.deferred(formats)
    }

    pub fn warm_one(&mut self, kind: Kind, format: DXGI_FORMAT) -> Result<(), Error> {
        self.pipelines.warm_one(kind, format)
    }

    pub fn late_pipelines(&self) -> &[(Kind, i32)] {
        self.pipelines.late()
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
        let shrink = std::mem::take(&mut self.trim_slots[slot]);
        let current = &self.slots[slot];
        let wanted = upload_bytes.next_power_of_two().max(INITIAL_UPLOAD_BYTES);
        let wanted_descriptors = descriptors.next_power_of_two().max(INITIAL_DESCRIPTORS);
        let capacity = match (current.capacity < upload_bytes, shrink) {
            (true, _) => wanted,
            (false, true) => wanted,
            (false, false) => current.capacity,
        };
        let srv_capacity = match (current.srv_capacity < descriptors, shrink) {
            (true, _) => wanted_descriptors,
            (false, true) => wanted_descriptors,
            (false, false) => current.srv_capacity,
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
        self.record_region(list, slot, frame_fence, plan, target, persistent, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_region(
        &mut self,
        list: &ID3D12GraphicsCommandList,
        slot: usize,
        frame_fence: u64,
        plan: &FramePlan,
        target: &SceneTarget,
        persistent: &dyn Fn(TextureHandle) -> Option<PersistentView>,
        repaint: Option<RECT>,
    ) -> Result<SceneStats, Error> {
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.pack(plan);
        let Scratch {
            bytes,
            uploads,
            tracked,
            activated,
            reads,
        } = &mut scratch;
        let read_total: u32 = plan.passes.iter().map(pass_descriptors).sum();
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
        let srv_stride = self.srv_stride;
        let table = move |index: u32| D3D12_GPU_DESCRIPTOR_HANDLE {
            ptr: srv_gpu.ptr + index as u64 * srv_stride as u64,
        };

        tracked.clear();
        tracked.insert(TextureRef::Surface, target.rest_state);
        for texture in &plan.textures {
            if let Some(index) = self.transients.position(texture.id) {
                tracked.insert(
                    TextureRef::Transient(texture.id),
                    self.transients.entries()[index].state,
                );
            }
        }
        activated.clear();
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
            list.SetDescriptorHeaps(&[Some(heap)]);
            list.SetGraphicsRootSignature(&self.pipelines.root_signature);
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
        }
        if let Some(timestamps) = &mut self.timestamps {
            timestamps.begin(list, slot);
        }
        self.emitter.attach(list)?;
        let emitter = &self.emitter;
        let first_surface_use = plan.passes.iter().find(|pass| {
            pass.reads.contains(&TextureRef::Surface) || pass.writes.contains(&TextureRef::Surface)
        });
        let clears_surface_first = first_surface_use.is_some_and(|pass| {
            pass.writes.first() == Some(&TextureRef::Surface)
                && matches!(pass.load, LoadOp::Clear(_))
        });
        if !clears_surface_first {
            if target.rest_state != D3D12_RESOURCE_STATE_RENDER_TARGET {
                let mut batch = emitter.batch();
                batch.transition(
                    target.resource,
                    target.rest_state,
                    D3D12_RESOURCE_STATE_RENDER_TARGET,
                );
                stats.barriers += emitter.submit(list, batch);
            }
            unsafe {
                list.ClearRenderTargetView(
                    target.rtv,
                    &plan.clear.to_gpu(),
                    repaint.as_ref().map(std::slice::from_ref),
                )
            };
            tracked.insert(TextureRef::Surface, D3D12_RESOURCE_STATE_RENDER_TARGET);
        }
        let mut next_descriptor = 0u32;
        for (pass_index, pass) in plan.passes.iter().enumerate() {
            if let Some(timestamps) = &mut self.timestamps {
                timestamps.mark(list, slot, pass_label(&pass.kind));
            }
            let mut barriers = emitter.batch();
            let rewrites_unordered = matches!(pass.kind, PassKind::Compute { .. })
                && pass.writes.first().is_some_and(|write| {
                    tracked.get(write) == Some(&D3D12_RESOURCE_STATE_UNORDERED_ACCESS)
                });
            for reference in pass.reads.iter().chain(&pass.writes) {
                if let TextureRef::Transient(id) = reference {
                    if activated.insert(*id) {
                        let bound = bind(*reference, self)?;
                        let current = tracked
                            .get(reference)
                            .copied()
                            .unwrap_or(D3D12_RESOURCE_STATE_COMMON);
                        if let Some(after) = barriers.activate(&bound.resource, current) {
                            tracked.insert(*reference, after);
                        }
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
                        let reference = TextureRef::Transient(*after);
                        let current = tracked
                            .get(&reference)
                            .copied()
                            .unwrap_or(D3D12_RESOURCE_STATE_COMMON);
                        if let Some(state) = barriers.activate(&bound.resource, current) {
                            tracked.insert(reference, state);
                        }
                    }
                    Barrier::Transition { resource, to, .. } => {
                        let to = match (copy_via_shader, to) {
                            (true, ResourceState::CopySource) => &ResourceState::ShaderRead,
                            (true, ResourceState::CopyDest) => &ResourceState::RenderTarget,
                            _ => to,
                        };
                        let Some(after) = state_for(resource, *to) else {
                            continue;
                        };
                        let bound = bind(*resource, self)?;
                        let before = tracked
                            .get(resource)
                            .copied()
                            .unwrap_or(D3D12_RESOURCE_STATE_COMMON);
                        if before != after {
                            match resource {
                                TextureRef::Persistent(_) => {
                                    barriers.transition_legacy(&bound.resource, before, after)
                                }
                                _ => barriers.transition(&bound.resource, before, after),
                            }
                            tracked.insert(*resource, after);
                        }
                    }
                }
            }
            let Some(&write) = pass.writes.first() else {
                stats.barriers += emitter.submit(list, barriers);
                continue;
            };
            let write_bound = bind(write, self)?;
            if rewrites_unordered {
                barriers.unordered(&write_bound.resource);
            }
            stats.barriers += emitter.submit(list, barriers);
            reads.clear();
            for reference in &pass.reads {
                reads.push(bind(*reference, self)?);
            }
            let read_bounds = &*reads;
            let first_descriptor = next_descriptor;
            for read in read_bounds {
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
            if matches!(pass.kind, PassKind::Custom(_) | PassKind::Compute { .. }) {
                for _ in read_bounds.len() as u32..custom::MAX_INPUTS {
                    unsafe {
                        self.device.CreateShaderResourceView(
                            None::<&ID3D12Resource>,
                            Some(&srv_desc(DXGI_FORMAT_R8G8B8A8_UNORM)),
                            D3D12_CPU_DESCRIPTOR_HANDLE {
                                ptr: srv_start.ptr + next_descriptor as usize * self.srv_stride,
                            },
                        )
                    };
                    next_descriptor += 1;
                }
            }
            let bound_index = match read_bounds.is_empty() {
                true => null_index,
                false => first_descriptor,
            };
            let bound_table = table(bound_index);
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
                            list.RSSetScissorRects(&[confine(
                                RECT {
                                    left: dest_origin[0] as i32,
                                    top: dest_origin[1] as i32,
                                    right: right as i32,
                                    bottom: bottom as i32,
                                },
                                repaint.filter(|_| write == TextureRef::Surface),
                            )]);
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
                            self.active
                                .binding
                                .set_constants(list, &values, bound_index);
                            self.active.binding.bind_table(list, bound_table);
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
                PassKind::Compute {
                    shader: ShaderId::Custom(id),
                    groups,
                    params,
                } => {
                    if !matches!(write, TextureRef::Transient(_)) {
                        return Err(Error::Unsupported("compute output must be a transient"));
                    }
                    let output_index = next_descriptor;
                    next_descriptor += 1;
                    unsafe {
                        self.device.CreateUnorderedAccessView(
                            &write_bound.resource,
                            None::<&ID3D12Resource>,
                            Some(&compute::uav_desc(write_bound.format)),
                            D3D12_CPU_DESCRIPTOR_HANDLE {
                                ptr: srv_start.ptr + output_index as usize * self.srv_stride,
                            },
                        )
                    };
                    let compute = ensure_compute(&mut self.compute, &self.device)?;
                    let state = compute.get(*id)?;
                    compute::dispatch(
                        list,
                        compute,
                        &state,
                        params,
                        table(first_descriptor),
                        table(output_index),
                        *groups,
                    );
                    stats.draws += 1;
                    continue;
                }
                PassKind::Compute { .. } => {
                    return Err(Error::Unsupported("compute shaders other than custom ids"));
                }
                PassKind::Custom(spec) => {
                    let Upload::Parameters { offset } = uploads[pass_index] else {
                        return Err(Error::Unsupported("custom pass without parameters"));
                    };
                    let rtv = write_bound
                        .rtv
                        .ok_or(Error::Unsupported("write to a persistent texture"))?;
                    let custom = ensure_custom(&mut self.custom, &self.device)?;
                    let state = custom.get(spec, write_bound.format)?;
                    let bounds = confine(
                        custom::scissor_of(spec.bounds, extent),
                        repaint.filter(|_| write == TextureRef::Surface),
                    );
                    unsafe {
                        if let LoadOp::Clear(color) = pass.load {
                            list.ClearRenderTargetView(rtv, &color.to_gpu(), None);
                        }
                    }
                    let drawn = custom::draw(
                        list,
                        &*custom,
                        &state,
                        &CustomDraw {
                            rtv,
                            extent,
                            parameters: upload_address + offset,
                            inputs: table(first_descriptor),
                            bounds,
                        },
                    );
                    unsafe { list.SetGraphicsRootSignature(&self.pipelines.root_signature) };
                    stats.draws += drawn as u32;
                    continue;
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
                let scissor = confine(
                    RECT {
                        left: 0,
                        top: 0,
                        right: extent.width as i32,
                        bottom: extent.height as i32,
                    },
                    repaint.filter(|_| write == TextureRef::Surface),
                );
                list.RSSetScissorRects(&[scissor]);
                if let LoadOp::Clear(color) = pass.load {
                    list.ClearRenderTargetView(rtv, &color.to_gpu(), Some(&[scissor]));
                }
                self.active.binding.bind_table(list, bound_table);
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
                    self.active
                        .binding
                        .set_constants(list, &values, bound_index);
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
                    self.active
                        .binding
                        .set_constants(list, &values, bound_index);
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
                    self.active
                        .binding
                        .set_constants(list, &values, bound_index);
                    list.DrawInstanced(4, 1, 0, 0);
                    stats.draws += 1;
                },
                (PassKind::Filter { shader, params }, _) => {
                    let kind = match shader {
                        ShaderId::BlurDown => Kind::BlurDown,
                        ShaderId::BlurUp => Kind::BlurUp,
                        ShaderId::ColorMatrix => Kind::ColorMatrix,
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
                        self.active
                            .binding
                            .set_constants(list, &values, bound_index);
                        if *shader == ShaderId::ColorMatrix {
                            self.active.binding.set_matrix(list, &params.matrix.0);
                        }
                        list.DrawInstanced(4, 1, 0, 0);
                    }
                    stats.draws += 1;
                }
                (PassKind::Glyphs { .. }, Upload::Sprites { offset, count }) => unsafe {
                    list.SetPipelineState(&self.pipelines.get(Kind::Glyph, write_bound.format)?);
                    let tone = plan.text_tone;
                    let values = constants(
                        extent,
                        1.0,
                        tone.ratios(),
                        [tone.contrast(), 0.0, 0.0, 0.0],
                        source_extent,
                    );
                    self.active
                        .binding
                        .set_constants(list, &values, bound_index);
                    sprites::draw(list, upload_address + offset, 0..count);
                    stats.draws += 1;
                },
                (PassKind::Slug { .. }, Upload::Sprites { offset, count }) => unsafe {
                    list.SetPipelineState(&self.pipelines.get(Kind::Slug, write_bound.format)?);
                    let tone = plan.text_tone;
                    let values = constants(
                        extent,
                        1.0,
                        tone.ratios(),
                        [tone.contrast(), 0.0, 0.0, 0.0],
                        source_extent,
                    );
                    self.active
                        .binding
                        .set_constants(list, &values, bound_index);
                    sprites::draw(list, upload_address + offset, 0..count);
                    stats.draws += 1;
                },
                (PassKind::Images { instances }, Upload::Sprites { offset, .. }) => unsafe {
                    let values = constants(extent, 1.0, [0.0; 4], [0.0; 4], source_extent);
                    self.active
                        .binding
                        .set_constants(list, &values, bound_index);
                    for (source, blend, range) in sprites::image_runs(instances) {
                        let kind = match blend {
                            Blend::Over => Kind::Image,
                            Blend::Replace => Kind::ImageReplace,
                        };
                        list.SetPipelineState(&self.pipelines.get(kind, write_bound.format)?);
                        let position = pass.reads.iter().position(|read| *read == source);
                        let descriptor = first_descriptor + position.unwrap_or(0) as u32;
                        self.active
                            .binding
                            .select(list, table(descriptor), descriptor);
                        sprites::draw(list, upload_address + offset, range);
                        stats.draws += 1;
                    }
                },
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
        let mut closing = emitter.batch();
        for barrier in &plan.final_barriers {
            if let Barrier::Transition { resource, to, .. } = barrier {
                if let Some(after) = state_for(resource, *to) {
                    let bound = bind(*resource, self)?;
                    let before = tracked
                        .get(resource)
                        .copied()
                        .unwrap_or(D3D12_RESOURCE_STATE_COMMON);
                    if before != after {
                        match resource {
                            TextureRef::Persistent(_) => {
                                closing.transition_legacy(&bound.resource, before, after)
                            }
                            _ => closing.transition(&bound.resource, before, after),
                        }
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
            closing.transition(target.resource, surface_now, target.rest_state);
            tracked.insert(TextureRef::Surface, target.rest_state);
        }
        stats.barriers += emitter.submit(list, closing);
        reads.clear();
        for (reference, final_state) in tracked.iter() {
            if let TextureRef::Transient(id) = reference {
                if let Some(index) = self.transients.position(*id) {
                    self.transients.entries_mut()[index].state = *final_state;
                }
            }
        }
        if let Some(timestamps) = &mut self.timestamps {
            timestamps.end(list, slot, frame_fence);
        }
        self.scratch = scratch;
        Ok(stats)
    }
}
