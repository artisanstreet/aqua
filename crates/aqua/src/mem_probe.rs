use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const SITE_MIN_BYTES: usize = 4096;

pub static SEEN_BYTES: AtomicU64 = AtomicU64::new(0);
pub static NESTED_BYTES: AtomicU64 = AtomicU64::new(0);
pub static TRACKED_BYTES: AtomicU64 = AtomicU64::new(0);

static CHAINS: Mutex<Option<HashMap<u32, String>>> = Mutex::new(None);

#[cfg(windows)]
fn chain_hash() -> u32 {
    use windows::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;
    let mut frames = [std::ptr::null_mut::<std::ffi::c_void>(); 24];
    let mut hash = 0u32;
    unsafe { RtlCaptureStackBackTrace(1, &mut frames, Some(&mut hash)) };
    hash
}

#[cfg(not(windows))]
fn chain_hash() -> u32 {
    0
}

static SITES: Mutex<Option<HashMap<usize, (usize, String)>>> = Mutex::new(None);

thread_local! {
    static IN_PROBE: Cell<bool> = const { Cell::new(false) };
}

fn is_infrastructure(frame: &str) -> bool {
    [
        "std::",
        "core::",
        "alloc::",
        "hashbrown::",
        "<alloc::",
        "<std::",
        "<core::",
        "<hashbrown::",
        "aqua::mem_probe",
        "<aqua::mem_probe",
        "__rust",
        "rust_",
        "backtrace",
        "_start",
        "main",
        "BaseThreadInitThunk",
    ]
    .iter()
    .any(|prefix| frame.starts_with(prefix))
}

fn site_of(trace: &str) -> String {
    let frames = trace
        .lines()
        .filter_map(|line| line.trim().split_once(": ").map(|(_, name)| name.trim()))
        .filter(|name| !name.starts_with("at "))
        .collect::<Vec<_>>();
    let normalized = |name: &str| match name.strip_prefix('<') {
        Some(inner) => inner.split([' ', '>']).next().unwrap_or(inner).to_owned(),
        None => name.split('<').next().unwrap_or(name).to_owned(),
    };
    frames
        .iter()
        .filter(|name| !is_infrastructure(name))
        .map(|name| normalized(name))
        .find(|name| name.contains("::"))
        .map_or_else(
            || "unknown".to_owned(),
            |name| name.split("::").take(2).collect::<Vec<_>>().join("::"),
        )
}

fn record(ptr: *mut u8, size: usize) {
    if size < SITE_MIN_BYTES {
        return;
    }
    SEEN_BYTES.fetch_add(size as u64, Ordering::Relaxed);
    let skip = IN_PROBE.with(|flag| flag.replace(true));
    if skip {
        NESTED_BYTES.fetch_add(size as u64, Ordering::Relaxed);
        return;
    }
    let hash = chain_hash();
    let cached = CHAINS
        .lock()
        .ok()
        .and_then(|chains| chains.as_ref().and_then(|map| map.get(&hash).cloned()));
    let site = match cached {
        Some(site) => site,
        None => {
            let site = site_of(&std::backtrace::Backtrace::force_capture().to_string());
            if let Ok(mut chains) = CHAINS.lock() {
                chains
                    .get_or_insert_with(HashMap::new)
                    .insert(hash, site.clone());
            }
            site
        }
    };
    if let Ok(mut sites) = SITES.lock() {
        sites
            .get_or_insert_with(HashMap::new)
            .insert(ptr as usize, (size, site));
        TRACKED_BYTES.fetch_add(size as u64, Ordering::Relaxed);
    }
    IN_PROBE.with(|flag| flag.set(false));
}

fn forget(ptr: *mut u8, size: usize) {
    if size < SITE_MIN_BYTES {
        return;
    }
    let skip = IN_PROBE.with(|flag| flag.replace(true));
    if skip {
        return;
    }
    if let Ok(mut sites) = SITES.lock() {
        if let Some(map) = sites.as_mut() {
            map.remove(&(ptr as usize));
        }
    }
    IN_PROBE.with(|flag| flag.set(false));
}

pub fn site_report() -> String {
    let sites = SITES.lock().ok();
    let mut totals: HashMap<&str, (usize, usize)> = HashMap::new();
    let guard = sites.as_ref().and_then(|sites| sites.as_ref());
    if let Some(map) = guard {
        for (size, site) in map.values() {
            let entry = totals.entry(site.as_str()).or_default();
            entry.0 += size;
            entry.1 += 1;
        }
    }
    let mut rows = totals.into_iter().collect::<Vec<_>>();
    rows.sort_by_key(|(_, (bytes, _))| std::cmp::Reverse(*bytes));
    let header = format!(
        "seen={} KiB nested={} KiB tracked_total={} KiB sites={}",
        SEEN_BYTES.load(Ordering::Relaxed) / 1024,
        NESTED_BYTES.load(Ordering::Relaxed) / 1024,
        TRACKED_BYTES.load(Ordering::Relaxed) / 1024,
        rows.len()
    );
    std::iter::once(header)
        .chain(rows.iter()
        .take(24)
        .map(|(site, (bytes, count))| format!("{:>9} KiB {:>6}x {site}", bytes / 1024, count)))
        .collect::<Vec<_>>()
        .join("\n")
}

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
            record(p, layout.size());
            let now =
                LIVE.fetch_add(layout.size() as u64, Ordering::Relaxed) + layout.size() as u64;
            PEAK.fetch_max(now, Ordering::Relaxed);
            BUCKETS[bucket(layout.size())].fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        forget(ptr, layout.size());
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size() as u64, Ordering::Relaxed);
        BUCKETS[bucket(layout.size())].fetch_sub(layout.size() as u64, Ordering::Relaxed);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        forget(ptr, layout.size());
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            record(p, new_size);
            LIVE.fetch_sub(layout.size() as u64, Ordering::Relaxed);
            BUCKETS[bucket(layout.size())].fetch_sub(layout.size() as u64, Ordering::Relaxed);
            let now = LIVE.fetch_add(new_size as u64, Ordering::Relaxed) + new_size as u64;
            PEAK.fetch_max(now, Ordering::Relaxed);
            BUCKETS[bucket(new_size)].fetch_add(new_size as u64, Ordering::Relaxed);
        }
        p
    }
}

#[cfg(feature = "probe-allocator")]
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessHeap {
    pub allocated: u64,
    pub committed: u64,
}

#[cfg(windows)]
pub fn process_heap() -> Option<ProcessHeap> {
    use windows::Win32::System::Memory::{GetProcessHeap, HeapSummary, HEAP_SUMMARY};
    let heap = unsafe { GetProcessHeap() }.ok()?;
    let mut summary = HEAP_SUMMARY {
        cb: std::mem::size_of::<HEAP_SUMMARY>() as u32,
        ..HEAP_SUMMARY::default()
    };
    let read = unsafe { HeapSummary(heap, 0, &mut summary) }.as_bool();
    read.then_some(ProcessHeap {
        allocated: summary.cbAllocated as u64,
        committed: summary.cbCommitted as u64,
    })
}

#[cfg(not(windows))]
pub fn process_heap() -> Option<ProcessHeap> {
    None
}

pub fn memory_report_enabled() -> bool {
    std::env::var_os("AQUA_MEMORY_REPORT").is_some_and(|value| value == "1")
}
