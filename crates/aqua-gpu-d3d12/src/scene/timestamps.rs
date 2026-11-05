use std::time::Duration;

use windows::Win32::Graphics::Direct3D12::*;

use super::Error;
use super::util::{create_buffer, map_whole};
use crate::ledger::{Kind, Token, record};
use crate::policy::ticks_to_duration;

const STAMPS_PER_SLOT: u32 = 64;
const STAMP_BYTES: u64 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassTime {
    pub label: &'static str,
    pub duration: Duration,
}

pub struct Timestamps {
    heap: ID3D12QueryHeap,
    readback: ID3D12Resource,
    mapped: *const u64,
    frequency: u64,
    pending: Vec<Option<Resolved>>,
    labels: Vec<Vec<&'static str>>,
    latest: Option<Duration>,
    latest_passes: Vec<PassTime>,
    _tokens: [Token; 2],
}

#[derive(Clone, Copy)]
struct Resolved {
    fence: u64,
    count: u32,
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
        let readback_bytes = count as u64 * STAMP_BYTES;
        Ok(Some(Self {
            _tokens: [
                record("timestamp_query_heap", Kind::QueryHeap, readback_bytes),
                record("timestamp_readback", Kind::ReadbackBuffer, readback_bytes),
            ],
            heap,
            readback,
            mapped,
            frequency,
            pending: vec![None; slots],
            labels: (0..slots)
                .map(|_| Vec::with_capacity(STAMPS_PER_SLOT as usize))
                .collect(),
            latest: None,
            latest_passes: Vec::with_capacity(STAMPS_PER_SLOT as usize),
        }))
    }

    pub fn begin(&mut self, list: &ID3D12GraphicsCommandList, slot: usize) {
        self.labels[slot].clear();
        unsafe {
            list.EndQuery(
                &self.heap,
                D3D12_QUERY_TYPE_TIMESTAMP,
                slot as u32 * STAMPS_PER_SLOT,
            )
        };
    }

    pub fn mark(&mut self, list: &ID3D12GraphicsCommandList, slot: usize, label: &'static str) {
        let used = self.labels[slot].len() as u32 + 1;
        if used + 1 >= STAMPS_PER_SLOT {
            return;
        }
        self.labels[slot].push(label);
        unsafe {
            list.EndQuery(
                &self.heap,
                D3D12_QUERY_TYPE_TIMESTAMP,
                slot as u32 * STAMPS_PER_SLOT + used,
            )
        };
    }

    pub fn end(&mut self, list: &ID3D12GraphicsCommandList, slot: usize, fence: u64) {
        let first = slot as u32 * STAMPS_PER_SLOT;
        let count = self.labels[slot].len() as u32 + 2;
        unsafe {
            list.EndQuery(&self.heap, D3D12_QUERY_TYPE_TIMESTAMP, first + count - 1);
            list.ResolveQueryData(
                &self.heap,
                D3D12_QUERY_TYPE_TIMESTAMP,
                first,
                count,
                &self.readback,
                first as u64 * STAMP_BYTES,
            );
        }
        self.pending[slot] = Some(Resolved { fence, count });
    }

    pub fn poll(&mut self, completed: u64) {
        let frequency = self.frequency;
        for slot in 0..self.pending.len() {
            let Some(resolved) = self.pending[slot].filter(|resolved| resolved.fence <= completed)
            else {
                continue;
            };
            self.pending[slot] = None;
            let first = slot * STAMPS_PER_SLOT as usize;
            let read =
                |index: usize| unsafe { std::ptr::read_volatile(self.mapped.add(first + index)) };
            let last = resolved.count as usize - 1;
            let Some(duration) = ticks_to_duration(read(0), read(last), frequency) else {
                continue;
            };
            self.latest = Some(duration);
            self.latest_passes.clear();
            for (index, label) in self.labels[slot].iter().enumerate() {
                if let Some(duration) = ticks_to_duration(read(index), read(index + 1), frequency) {
                    self.latest_passes.push(PassTime { label, duration });
                }
            }
        }
    }

    pub fn latest(&self) -> Option<Duration> {
        self.latest
    }

    pub fn latest_passes(&self) -> &[PassTime] {
        &self.latest_passes
    }
}
