use std::collections::HashMap;

use aqua_graph::{Extent, FramePlan, PassKind, TextureFormat, TextureRef, TransientId};
use windows::Win32::Graphics::Direct3D12::*;

use windows::core::Interface;

use super::Error;
use super::barriers::{BarrierMode, initial_layout};
use super::formats::dxgi;
use super::util::*;
use crate::ledger::{Kind, Token, record};
use crate::placement::{Planned, place};
use crate::policy::{HeapDecision, HeapGate};

const HEAP_ALIGNMENT: u64 = D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT as u64;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    id: TransientId,
    extent: Extent,
    format: TextureFormat,
    offset: u64,
    bytes: u64,
    unordered: bool,
}

pub struct Entry {
    pub resource: ID3D12Resource,
    pub extent: Extent,
    pub format: TextureFormat,
    pub state: D3D12_RESOURCE_STATES,
}

struct Retired {
    fence: u64,
    bytes: u64,
    _resources: Vec<Entry>,
    _heap: Option<(ID3D12Heap, Token)>,
}

pub struct Transients {
    device10: Option<ID3D12Device10>,
    gate: HeapGate,
    heap: Option<(ID3D12Heap, u64, Token)>,
    needed: u64,
    keys: Vec<Key>,
    entries: Vec<Entry>,
    index: HashMap<TransientId, usize>,
    retired: Vec<Retired>,
}

fn desc1_of(extent: Extent, format: TextureFormat, unordered: bool) -> D3D12_RESOURCE_DESC1 {
    let desc = desc_of(extent, format, unordered);
    D3D12_RESOURCE_DESC1 {
        Dimension: desc.Dimension,
        Alignment: desc.Alignment,
        Width: desc.Width,
        Height: desc.Height,
        DepthOrArraySize: desc.DepthOrArraySize,
        MipLevels: desc.MipLevels,
        Format: desc.Format,
        SampleDesc: desc.SampleDesc,
        Layout: desc.Layout,
        Flags: desc.Flags,
        SamplerFeedbackMipRegion: D3D12_MIP_REGION::default(),
    }
}

fn desc_of(extent: Extent, format: TextureFormat, unordered: bool) -> D3D12_RESOURCE_DESC {
    texture_desc(
        extent.width,
        extent.height,
        dxgi(format),
        match unordered {
            true => {
                D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET | D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS
            }
            false => D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
        },
    )
}

fn place_keys(device: &ID3D12Device, keys: &[Key]) -> crate::placement::Placement {
    let real: Vec<u64> = keys
        .iter()
        .map(|key| unsafe {
            device.GetResourceAllocationInfo(0, &[desc_of(key.extent, key.format, key.unordered)])
        }
        .SizeInBytes)
        .collect();
    let planned: Vec<Planned> = keys
        .iter()
        .map(|key| Planned {
            offset: key.offset,
            bytes: key.bytes,
        })
        .collect();
    place(&planned, &real, HEAP_ALIGNMENT)
}

fn keys_of(plan: &FramePlan) -> Vec<Key> {
    plan.textures
        .iter()
        .map(|texture| Key {
            id: texture.id,
            extent: texture.extent,
            format: texture.format,
            offset: texture.heap_offset,
            bytes: texture.bytes(),
            unordered: unordered_write(plan, texture.id),
        })
        .collect()
}

fn unordered_write(plan: &FramePlan, id: TransientId) -> bool {
    plan.passes.iter().any(|pass| {
        matches!(pass.kind, PassKind::Compute { .. })
            && pass.writes.contains(&TextureRef::Transient(id))
    })
}

impl Transients {
    pub fn new(
        device: &ID3D12Device,
        sync: BarrierMode,
        ceiling: Option<u64>,
    ) -> Result<Self, Error> {
        let device10 = match sync {
            BarrierMode::Queue => None,
            BarrierMode::Enhanced => Some(
                device
                    .cast::<ID3D12Device10>()
                    .map_err(|_| Error::Unsupported("enhanced barriers need device 10"))?,
            ),
        };
        Ok(Self {
            device10,
            gate: HeapGate::new(ceiling, HEAP_ALIGNMENT),
            heap: None,
            needed: 0,
            keys: Vec::new(),
            entries: Vec::new(),
            index: HashMap::new(),
            retired: Vec::new(),
        })
    }

    pub fn trim(&mut self, frame_fence: u64) -> u64 {
        let freed = self.heap_bytes();
        let entries = std::mem::take(&mut self.entries);
        let heap = self.heap.take().map(|(heap, _, token)| (heap, token));
        if !entries.is_empty() || heap.is_some() {
            self.retired.push(Retired {
                fence: frame_fence,
                bytes: freed,
                _resources: entries,
                _heap: heap,
            });
        }
        self.keys.clear();
        self.index.clear();
        self.needed = 0;
        self.gate.reset();
        freed
    }

    pub fn retired_bytes(&self) -> u64 {
        self.retired.iter().map(|entry| entry.bytes).sum()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn entries_mut(&mut self) -> &mut [Entry] {
        &mut self.entries
    }

    pub fn position(&self, id: TransientId) -> Option<usize> {
        self.index.get(&id).copied()
    }

    pub fn retire(&mut self, completed: u64) {
        self.retired.retain(|entry| entry.fence > completed);
    }

    pub fn heap_bytes(&self) -> u64 {
        self.heap.as_ref().map(|(_, size, _)| *size).unwrap_or(0)
    }

    fn matches(&self, plan: &FramePlan) -> bool {
        self.keys.len() == plan.textures.len()
            && self.keys.iter().zip(&plan.textures).all(|(key, texture)| {
                key.id == texture.id
                    && key.extent == texture.extent
                    && key.format == texture.format
                    && key.offset == texture.heap_offset
                    && key.bytes == texture.bytes()
                    && key.unordered == unordered_write(plan, texture.id)
            })
    }

    fn create(
        &self,
        device: &ID3D12Device,
        heap: &ID3D12Heap,
        key: &Key,
        offset: u64,
    ) -> Result<ID3D12Resource, Error> {
        let clear = D3D12_CLEAR_VALUE {
            Format: dxgi(key.format),
            Anonymous: D3D12_CLEAR_VALUE_0 { Color: [0.0; 4] },
        };
        let mut resource: Option<ID3D12Resource> = None;
        match &self.device10 {
            Some(device10) => unsafe {
                device10.CreatePlacedResource2(
                    heap,
                    offset,
                    &desc1_of(key.extent, key.format, key.unordered),
                    initial_layout(D3D12_RESOURCE_STATE_RENDER_TARGET),
                    Some(&clear),
                    None,
                    &mut resource,
                )?
            },
            None => unsafe {
                device.CreatePlacedResource(
                    heap,
                    offset,
                    &desc_of(key.extent, key.format, key.unordered),
                    D3D12_RESOURCE_STATE_RENDER_TARGET,
                    Some(&clear),
                    &mut resource,
                )?
            },
        }
        let resource = resource.ok_or(Error::NoAdapter)?;
        if super::debug::requested() {
            let name = format!("transient {} offset {offset}", key.id.0);
            let _ = unsafe { resource.SetName(&windows::core::HSTRING::from(name)) };
        }
        Ok(resource)
    }

    pub fn ensure(
        &mut self,
        device: &ID3D12Device,
        plan: &FramePlan,
        frame_fence: u64,
    ) -> Result<bool, Error> {
        let same = self.matches(plan);
        let fresh = match same {
            true => None,
            false => Some(keys_of(plan)),
        };
        let placement = fresh.as_ref().map(|keys| place_keys(device, keys));
        let needed = placement
            .as_ref()
            .map(|placement| placement.total)
            .unwrap_or(self.needed);
        let current = self.heap_bytes();
        let resize = match self.gate.decide(current, needed) {
            HeapDecision::Keep => None,
            HeapDecision::Resize { bytes } => Some(bytes),
            HeapDecision::Reject { needed, ceiling } => {
                return Err(Error::Budget { needed, ceiling });
            }
        };
        if same && resize.is_none() {
            return Ok(false);
        }
        let keys = fresh.unwrap_or_else(|| std::mem::take(&mut self.keys));
        let placement = placement.unwrap_or_else(|| place_keys(device, &keys));
        let old_entries = std::mem::take(&mut self.entries);
        let mut old_heap = None;
        let mut old_bytes = 0;
        if let Some(bytes) = resize {
            let size = bytes.max(placement.total).next_multiple_of(HEAP_ALIGNMENT);
            let new_heap = match size {
                0 => None,
                size => {
                    let desc = D3D12_HEAP_DESC {
                        SizeInBytes: size,
                        Properties: heap_properties(D3D12_HEAP_TYPE_DEFAULT),
                        Alignment: HEAP_ALIGNMENT,
                        Flags: D3D12_HEAP_FLAG_ALLOW_ONLY_RT_DS_TEXTURES,
                    };
                    let mut heap: Option<ID3D12Heap> = None;
                    unsafe { device.CreateHeap(&desc, &mut heap)? };
                    heap.map(|heap| {
                        (
                            heap,
                            size,
                            record("transient_heap", Kind::TransientHeap, size),
                        )
                    })
                }
            };
            let previous = std::mem::replace(&mut self.heap, new_heap);
            old_bytes = previous.as_ref().map(|(_, size, _)| *size).unwrap_or(0);
            old_heap = previous.map(|(heap, _, token)| (heap, token));
        }
        let mut created = Vec::with_capacity(keys.len());
        for (key, offset) in keys.iter().zip(&placement.offsets) {
            let (heap, _, _) = self.heap.as_ref().ok_or(Error::NoAdapter)?;
            created.push(Entry {
                resource: self.create(device, heap, key, *offset)?,
                extent: key.extent,
                format: key.format,
                state: D3D12_RESOURCE_STATE_RENDER_TARGET,
            });
        }
        if !old_entries.is_empty() || old_heap.is_some() {
            self.retired.push(Retired {
                fence: frame_fence,
                bytes: old_bytes,
                _resources: old_entries,
                _heap: old_heap,
            });
        }
        self.index = keys
            .iter()
            .enumerate()
            .map(|(position, key)| (key.id, position))
            .collect();
        self.entries = created;
        self.keys = keys;
        self.needed = placement.total;
        Ok(true)
    }
}
