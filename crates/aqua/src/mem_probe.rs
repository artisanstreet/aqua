use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Probe;

pub static LIVE: AtomicU64 = AtomicU64::new(0);
pub static PEAK: AtomicU64 = AtomicU64::new(0);
pub static BUCKETS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

fn bucket(size: usize) -> usize {
    match size {
        0..=256 => 0,
        257..=4096 => 1,
        4097..=65536 => 2,
        65537..=1048576 => 3,
        1048577..=16777216 => 4,
        _ => 5,
    }
}

unsafe impl GlobalAlloc for Probe {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let now =
                LIVE.fetch_add(layout.size() as u64, Ordering::Relaxed) + layout.size() as u64;
            PEAK.fetch_max(now, Ordering::Relaxed);
            BUCKETS[bucket(layout.size())].fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size() as u64, Ordering::Relaxed);
        BUCKETS[bucket(layout.size())].fetch_sub(layout.size() as u64, Ordering::Relaxed);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            LIVE.fetch_sub(layout.size() as u64, Ordering::Relaxed);
            BUCKETS[bucket(layout.size())].fetch_sub(layout.size() as u64, Ordering::Relaxed);
            let now = LIVE.fetch_add(new_size as u64, Ordering::Relaxed) + new_size as u64;
            PEAK.fetch_max(now, Ordering::Relaxed);
            BUCKETS[bucket(new_size)].fetch_add(new_size as u64, Ordering::Relaxed);
        }
        p
    }
}

#[global_allocator]
static ALLOC: Probe = Probe;

pub fn summary() -> String {
    let b: Vec<u64> = BUCKETS
        .iter()
        .map(|b| b.load(Ordering::Relaxed) / 1024)
        .collect();
    format!(
        "live_kb={} peak_kb={} buckets_kb(<=256,4k,64k,1m,16m,>)={:?}",
        LIVE.load(Ordering::Relaxed) / 1024,
        PEAK.load(Ordering::Relaxed) / 1024,
        b
    )
}
