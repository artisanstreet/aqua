#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fmt;
use std::ops::Sub;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub allocations: u64,
    pub reallocations: u64,
    pub deallocations: u64,
    pub bytes: u64,
}

impl Counts {
    pub const ZERO: Counts = Counts {
        allocations: 0,
        reallocations: 0,
        deallocations: 0,
        bytes: 0,
    };

    pub fn heap_events(self) -> u64 {
        self.allocations + self.reallocations
    }
}

impl Sub for Counts {
    type Output = Counts;

    fn sub(self, rhs: Counts) -> Counts {
        Counts {
            allocations: self.allocations - rhs.allocations,
            reallocations: self.reallocations - rhs.reallocations,
            deallocations: self.deallocations - rhs.deallocations,
            bytes: self.bytes - rhs.bytes,
        }
    }
}

thread_local! {
    static LOCAL: Cell<Counts> = const { Cell::new(Counts::ZERO) };
}

static LIVE: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);

fn local(update: impl FnOnce(&mut Counts)) {
    let _ = LOCAL.try_with(|cell| {
        let mut counts = cell.get();
        update(&mut counts);
        cell.set(counts);
    });
}

fn grow(bytes: u64) {
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

fn shrink(bytes: u64) {
    LIVE.fetch_sub(bytes, Ordering::Relaxed);
}

pub struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            local(|counts| {
                counts.allocations += 1;
                counts.bytes += layout.size() as u64;
            });
            grow(layout.size() as u64);
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            local(|counts| {
                counts.allocations += 1;
                counts.bytes += layout.size() as u64;
            });
            grow(layout.size() as u64);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        local(|counts| counts.deallocations += 1);
        shrink(layout.size() as u64);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            local(|counts| {
                counts.reallocations += 1;
                counts.bytes += new_size as u64;
            });
            shrink(layout.size() as u64);
            grow(new_size as u64);
        }
        moved
    }
}

pub fn thread_counts() -> Counts {
    LOCAL.with(Cell::get)
}

pub fn live_bytes() -> u64 {
    LIVE.load(Ordering::Relaxed)
}

pub fn peak_bytes() -> u64 {
    PEAK.load(Ordering::Relaxed)
}

pub fn reset_peak() {
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
}

pub fn measure<R>(work: impl FnOnce() -> R) -> (R, Counts) {
    let before = thread_counts();
    let result = work();
    (result, thread_counts() - before)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeapGate {
    pub max_heap_events: u64,
    pub max_bytes: u64,
}

impl HeapGate {
    pub const ZERO: HeapGate = HeapGate {
        max_heap_events: 0,
        max_bytes: 0,
    };

    pub fn check(self, counts: Counts) -> Result<(), GateFailure> {
        match (
            counts.heap_events() <= self.max_heap_events,
            counts.bytes <= self.max_bytes,
        ) {
            (true, true) => Ok(()),
            _ => Err(GateFailure { gate: self, counts }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GateFailure {
    pub gate: HeapGate,
    pub counts: Counts,
}

impl fmt::Display for GateFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "heap gate exceeded: {} allocations and reallocations (limit {}), {} bytes (limit {})",
            self.counts.heap_events(),
            self.gate.max_heap_events,
            self.counts.bytes,
            self.gate.max_bytes
        )
    }
}

impl std::error::Error for GateFailure {}
