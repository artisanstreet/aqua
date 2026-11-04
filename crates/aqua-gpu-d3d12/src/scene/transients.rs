use std::collections::HashMap;

use aqua_graph::{Extent, FramePlan, TextureFormat, TransientId};
use windows::Win32::Graphics::Direct3D12::*;

use super::Error;
use super::formats::dxgi;
use super::util::*;
use crate::placement::{Planned, place};

const HEAP_ALIGNMENT: u64 = D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT as u64;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    id: TransientId,
    extent: Extent,
    format: TextureFormat,
    offset: u64,
    bytes: u64,
}

pub struct Entry {
    pub resource: ID3D12Resource,
    pub extent: Extent,
    pub format: TextureFormat,
    pub state: D3D12_RESOURCE_STATES,
}

struct Retired {
    fence: u64,
    _resources: Vec<Entry>,
    _heap: Option<ID3D12Heap>,
}

pub struct Transients {
    heap: Option<(ID3D12Heap, u64)>,
    keys: Vec<Key>,
    entries: Vec<Entry>,
    index: HashMap<TransientId, usize>,
    retired: Vec<Retired>,
}

fn desc_of(extent: Extent, format: TextureFormat) -> D3D12_RESOURCE_DESC {
    texture_desc(
        extent.width,
        extent.height,
        dxgi(format),
        D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
    )
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
        })
        .collect()
}

impl Transients {
    pub fn new() -> Self {
        Self {
            heap: None,
            keys: Vec::new(),
            entries: Vec::new(),
            index: HashMap::new(),
            retired: Vec::new(),
        }
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
        self.heap.as_ref().map(|(_, size)| *size).unwrap_or(0)
    }

    pub fn ensure(
        &mut self,
        device: &ID3D12Device,
        plan: &FramePlan,
        frame_fence: u64,
    ) -> Result<(), Error> {
        let keys = keys_of(plan);
        if keys == self.keys {
            return Ok(());
        }
        let old_entries = std::mem::take(&mut self.entries);
        let real: Vec<u64> = keys
            .iter()
            .map(|key| unsafe { device.GetResourceAllocationInfo(0, &[desc_of(key.extent, key.format)]) }.SizeInBytes)
            .collect();
        let planned: Vec<Planned> = keys
            .iter()
            .map(|key| Planned {
                offset: key.offset,
                bytes: key.bytes,
            })
            .collect();
        let placement = place(&planned, &real, HEAP_ALIGNMENT);
        let mut old_heap = None;
        let grow = self
            .heap
            .as_ref()
            .map(|(_, size)| *size < placement.total)
            .unwrap_or(placement.total > 0);
        if grow {
            let size = placement.total.next_multiple_of(HEAP_ALIGNMENT);
            let desc = D3D12_HEAP_DESC {
                SizeInBytes: size,
                Properties: heap_properties(D3D12_HEAP_TYPE_DEFAULT),
                Alignment: HEAP_ALIGNMENT,
                Flags: D3D12_HEAP_FLAG_ALLOW_ONLY_RT_DS_TEXTURES,
            };
            let mut heap: Option<ID3D12Heap> = None;
            unsafe { device.CreateHeap(&desc, &mut heap)? };
            old_heap = self.heap.take().map(|(heap, _)| heap);
            self.heap = heap.map(|heap| (heap, size));
        }
        let mut created = Vec::with_capacity(keys.len());
        for (key, offset) in keys.iter().zip(&placement.offsets) {
            let clear = D3D12_CLEAR_VALUE {
                Format: dxgi(key.format),
                Anonymous: D3D12_CLEAR_VALUE_0 { Color: [0.0; 4] },
            };
            let mut resource: Option<ID3D12Resource> = None;
            let (heap, _) = self.heap.as_ref().ok_or(Error::NoAdapter)?;
            unsafe {
                device.CreatePlacedResource(
                    heap,
                    *offset,
                    &desc_of(key.extent, key.format),
                    D3D12_RESOURCE_STATE_RENDER_TARGET,
                    Some(&clear),
                    &mut resource,
                )?
            };
            created.push(Entry {
                resource: resource.ok_or(Error::NoAdapter)?,
                extent: key.extent,
                format: key.format,
                state: D3D12_RESOURCE_STATE_RENDER_TARGET,
            });
        }
        if !old_entries.is_empty() || old_heap.is_some() {
            self.retired.push(Retired {
                fence: frame_fence,
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
        Ok(())
    }
}
