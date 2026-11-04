use std::time::Duration;

use windows::Win32::Graphics::Direct3D12::*;

use super::util::{create_buffer, map_whole};
use super::Error;
use crate::policy::ticks_to_duration;

const STAMPS_PER_SLOT: u32 = 2;
const STAMP_BYTES: u64 = 8;

pub struct Timestamps {
    heap: ID3D12QueryHeap,
    readback: ID3D12Resource,
    mapped: *const u64,
    frequency: u64,
    pending: Vec<Option<u64>>,
    latest: Option<Duration>,
}

fn frequency_of(device: &ID3D12Device) -> Option<u64> {
    let queue: ID3D12CommandQueue = unsafe {
        device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
            Priority: 0,
            Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
            NodeMask: 0,
        })
    }
    .ok()?;
    unsafe { queue.GetTimestampFrequency() }
        .ok()
        .filter(|frequency| *frequency > 0)
}

impl Timestamps {
    pub fn new(device: &ID3D12Device, slots: usize) -> Result<Option<Self>, Error> {
        let Some(frequency) = frequency_of(device) else {
            return Ok(None);
        };
        let count = slots as u32 * STAMPS_PER_SLOT;
        let mut heap: Option<ID3D12QueryHeap> = None;
        let created = unsafe {
            device.CreateQueryHeap(
                &D3D12_QUERY_HEAP_DESC {
                    Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP,
                    Count: count,
                    NodeMask: 0,
                },
                &mut heap,
            )
        };
        let Some(heap) = created.ok().and(heap) else {
            return Ok(None);
        };
        let readback = create_buffer(
            device,
            D3D12_HEAP_TYPE_READBACK,
            count as u64 * STAMP_BYTES,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        let mapped = map_whole(&readback)? as *const u64;
        Ok(Some(Self {
            heap,
            readback,
            mapped,
            frequency,
            pending: vec![None; slots],
            latest: None,
        }))
    }

    pub fn begin(&self, list: &ID3D12GraphicsCommandList, slot: usize) {
        unsafe {
            list.EndQuery(
                &self.heap,
                D3D12_QUERY_TYPE_TIMESTAMP,
                slot as u32 * STAMPS_PER_SLOT,
            )
        };
    }

    pub fn end(&mut self, list: &ID3D12GraphicsCommandList, slot: usize, fence: u64) {
        let first = slot as u32 * STAMPS_PER_SLOT;
        unsafe {
            list.EndQuery(&self.heap, D3D12_QUERY_TYPE_TIMESTAMP, first + 1);
            list.ResolveQueryData(
                &self.heap,
                D3D12_QUERY_TYPE_TIMESTAMP,
                first,
                STAMPS_PER_SLOT,
                &self.readback,
                first as u64 * STAMP_BYTES,
            );
        }
        self.pending[slot] = Some(fence);
    }

    pub fn poll(&mut self, completed: u64) {
        let frequency = self.frequency;
        for slot in 0..self.pending.len() {
            if self.pending[slot].is_some_and(|fence| fence <= completed) {
                self.pending[slot] = None;
                let first = slot * STAMPS_PER_SLOT as usize;
                let (start, end) = unsafe {
                    (
                        std::ptr::read_volatile(self.mapped.add(first)),
                        std::ptr::read_volatile(self.mapped.add(first + 1)),
                    )
                };
                if let Some(duration) = ticks_to_duration(start, end, frequency) {
                    self.latest = Some(duration);
                }
            }
        }
    }

    pub fn latest(&self) -> Option<Duration> {
        self.latest
    }
}
