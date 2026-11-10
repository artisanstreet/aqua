use std::time::Duration;

use windows::core::Interface;
use windows::Win32::Foundation::S_OK;
use windows::Win32::Graphics::Direct3D11::*;

use super::Error;
use crate::ledger::{record, Kind, Token};

const STAMPS_PER_SLOT: usize = 64;
const SLOTS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassTime {
    pub label: &'static str,
    pub duration: Duration,
}

struct Slot {
    disjoint: ID3D11Query,
    stamps: Vec<ID3D11Query>,
    labels: Vec<&'static str>,
    pending: bool,
}

pub struct Timestamps {
    slots: Vec<Slot>,
    next: usize,
    recording: Option<usize>,
    latest: Option<Duration>,
    latest_passes: Vec<PassTime>,
    _token: Token,
}

fn query(device: &ID3D11Device, kind: D3D11_QUERY) -> Result<ID3D11Query, Error> {
    let mut created = None;
    unsafe {
        device.CreateQuery(
            &D3D11_QUERY_DESC {
                Query: kind,
                MiscFlags: 0,
            },
            Some(&mut created),
        )?
    };
    created.ok_or(Error::NoAdapter)
}

fn ready<T: Default>(context: &ID3D11DeviceContext1, query: &ID3D11Query) -> Option<T> {
    let mut data = T::default();
    let code = unsafe {
        (Interface::vtable(context).base__.GetData)(
            Interface::as_raw(context),
            Interface::as_raw(query),
            &mut data as *mut T as *mut _,
            std::mem::size_of::<T>() as u32,
            D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
        )
    };
    (code == S_OK).then_some(data)
}

fn ticks(start: u64, end: u64, frequency: u64) -> Option<Duration> {
    match (frequency, end.checked_sub(start)) {
        (0, _) | (_, None) => None,
        (frequency, Some(ticks)) => Some(Duration::from_nanos(
            (ticks as u128 * 1_000_000_000 / frequency as u128).min(u64::MAX as u128) as u64,
        )),
    }
}

impl Timestamps {
    pub fn new(device: &ID3D11Device) -> Result<Self, Error> {
        let slots = (0..SLOTS)
            .map(|_| {
                Ok(Slot {
                    disjoint: query(device, D3D11_QUERY_TIMESTAMP_DISJOINT)?,
                    stamps: (0..STAMPS_PER_SLOT)
                        .map(|_| query(device, D3D11_QUERY_TIMESTAMP))
                        .collect::<Result<_, Error>>()?,
                    labels: Vec::with_capacity(STAMPS_PER_SLOT),
                    pending: false,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(Self {
            slots,
            next: 0,
            recording: None,
            latest: None,
            latest_passes: Vec::with_capacity(STAMPS_PER_SLOT),
            _token: record(
                "timestamp_queries",
                Kind::Query,
                (SLOTS * (STAMPS_PER_SLOT + 1) * 8) as u64,
            ),
        })
    }

    pub fn poll(&mut self, context: &ID3D11DeviceContext1) {
        for index in 0..self.slots.len() {
            let slot = &self.slots[index];
            if !slot.pending {
                continue;
            }
            let Some(disjoint) =
                ready::<D3D11_QUERY_DATA_TIMESTAMP_DISJOINT>(context, &slot.disjoint)
            else {
                continue;
            };
            let count = slot.labels.len() + 2;
            let mut stamps = [0u64; STAMPS_PER_SLOT];
            let complete =
                (0..count).all(|stamp| match ready::<u64>(context, &slot.stamps[stamp]) {
                    Some(value) => {
                        stamps[stamp] = value;
                        true
                    }
                    None => false,
                });
            if !complete {
                continue;
            }
            self.slots[index].pending = false;
            if disjoint.Disjoint.as_bool() {
                continue;
            }
            let frequency = disjoint.Frequency;
            let Some(total) = ticks(stamps[0], stamps[count - 1], frequency) else {
                continue;
            };
            self.latest = Some(total);
            self.latest_passes.clear();
            for (stamp, label) in self.slots[index].labels.iter().enumerate() {
                if let Some(duration) = ticks(stamps[stamp + 1], stamps[stamp + 2], frequency) {
                    self.latest_passes.push(PassTime { label, duration });
                }
            }
        }
    }

    pub fn begin(&mut self, context: &ID3D11DeviceContext1) {
        let index = self.next;
        if self.slots[index].pending {
            self.recording = None;
            return;
        }
        self.next = (self.next + 1) % self.slots.len();
        let slot = &mut self.slots[index];
        slot.labels.clear();
        unsafe {
            context.Begin(&slot.disjoint);
            context.End(&slot.stamps[0]);
        }
        self.recording = Some(index);
    }

    pub fn mark(&mut self, context: &ID3D11DeviceContext1, label: &'static str) {
        let Some(index) = self.recording else {
            return;
        };
        let slot = &mut self.slots[index];
        let used = slot.labels.len() + 1;
        if used + 1 >= STAMPS_PER_SLOT {
            return;
        }
        slot.labels.push(label);
        unsafe { context.End(&slot.stamps[used]) };
    }

    pub fn end(&mut self, context: &ID3D11DeviceContext1) {
        let Some(index) = self.recording.take() else {
            return;
        };
        let slot = &mut self.slots[index];
        let last = slot.labels.len() + 1;
        unsafe {
            context.End(&slot.stamps[last]);
            context.End(&slot.disjoint);
        }
        slot.pending = true;
    }

    pub fn latest(&self) -> Option<Duration> {
        self.latest
    }

    pub fn latest_passes(&self) -> &[PassTime] {
        &self.latest_passes
    }
}
