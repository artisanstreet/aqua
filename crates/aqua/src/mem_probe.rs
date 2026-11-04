use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};

const HEADER: usize = 16;
const SLOTS: usize = 32768;
const DEPTH: usize = 28;

struct Slot {
    key: AtomicU32,
    live: AtomicI64,
    allocations: AtomicU64,
    frames: [AtomicUsize; DEPTH],
}

static TABLE: [Slot; SLOTS] = [const {
    Slot {
        key: AtomicU32::new(0),
        live: AtomicI64::new(0),
        allocations: AtomicU64::new(0),
        frames: [const { AtomicUsize::new(0) }; DEPTH],
    }
}; SLOTS];

static PROBE_LIVE: AtomicI64 = AtomicI64::new(0);

thread_local! {
    static IN_PROBE: Cell<bool> = const { Cell::new(false) };
}

#[cfg(windows)]
fn capture() -> ([usize; DEPTH], u32) {
    use windows::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;
    let mut frames = [std::ptr::null_mut::<std::ffi::c_void>(); DEPTH];
    let mut hash = 0u32;
    unsafe { RtlCaptureStackBackTrace(1, &mut frames, Some(&mut hash)) };
    (frames.map(|frame| frame as usize), hash.max(1))
}

#[cfg(not(windows))]
fn capture() -> ([usize; DEPTH], u32) {
    ([0; DEPTH], 1)
}

#[cfg(windows)]
fn module_base() -> usize {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    unsafe { GetModuleHandleW(None) }.map_or(0, |module| module.0 as usize)
}

#[cfg(not(windows))]
fn module_base() -> usize {
    0
}

fn slot_for(key: u32) -> Option<(&'static Slot, bool)> {
    (0..SLOTS).find_map(|probe| {
        let slot = &TABLE[(key as usize).wrapping_add(probe) % SLOTS];
        match slot
            .key
            .compare_exchange(0, key, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => Some((slot, true)),
            Err(existing) if existing == key => Some((slot, false)),
            Err(_) => None,
        }
    })
}

fn site_key() -> u32 {
    let nested = IN_PROBE.with(|flag| flag.replace(true));
    if nested {
        return 0;
    }
    let (frames, key) = capture();
    if let Some((slot, fresh)) = slot_for(key) {
        if fresh {
            frames
                .iter()
                .zip(slot.frames.iter())
                .for_each(|(frame, cell)| cell.store(*frame, Ordering::Relaxed));
        }
    }
    IN_PROBE.with(|flag| flag.set(false));
    key
}

fn account(key: u32, size: usize, sign: i64) {
    if key == 0 {
        PROBE_LIVE.fetch_add(sign * size as i64, Ordering::Relaxed);
        return;
    }
    if let Some((slot, _)) = slot_for(key) {
        slot.live.fetch_add(sign * size as i64, Ordering::Relaxed);
        if sign > 0 {
            slot.allocations.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub fn site_report() -> String {
    let base = module_base();
    let mut rows = TABLE
        .iter()
        .filter(|slot| slot.live.load(Ordering::Relaxed) > 0)
        .map(|slot| {
            let frames = slot
                .frames
                .iter()
                .map(|frame| frame.load(Ordering::Relaxed))
                .filter(|frame| *frame != 0)
                .map(|frame| format!("{:x}", frame.wrapping_sub(base)))
                .collect::<Vec<_>>()
                .join(",");
            (
                slot.live.load(Ordering::Relaxed),
                slot.allocations.load(Ordering::Relaxed),
                frames,
            )
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|(live, _, _)| std::cmp::Reverse(*live));
    let header = format!(
        "aqua-heap-chains probe_live={} live_total={} base={base:x}",
        PROBE_LIVE.load(Ordering::Relaxed),
        LIVE.load(Ordering::Relaxed)
    );
    std::iter::once(header)
        .chain(
            rows.into_iter().map(|(live, count, frames)| {
                format!("chain live={live} allocs={count} rel={frames}")
            }),
        )
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn dump_chains() {
    if let Some(path) = std::env::var_os("AQUA_HEAP_CHAINS_FILE") {
        let _ = std::fs::write(path, site_report());
    }
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

fn base_layout(layout: Layout) -> Layout {
    let align = layout.align().max(HEADER);
    unsafe { Layout::from_size_align_unchecked(layout.size() + align, align) }
}

fn offset(layout: Layout) -> usize {
    layout.align().max(HEADER)
}

unsafe fn finish_alloc(base: *mut u8, layout: Layout) -> *mut u8 {
    if base.is_null() {
        return base;
    }
    let key = site_key();
    let user = unsafe { base.add(offset(layout)) };
    unsafe {
        user.sub(HEADER).cast::<u32>().write(key);
        user.sub(HEADER)
            .cast::<u64>()
            .add(1)
            .write(layout.size() as u64);
    }
    account(key, layout.size(), 1);
    let now = LIVE.fetch_add(layout.size() as u64, Ordering::Relaxed) + layout.size() as u64;
    PEAK.fetch_max(now, Ordering::Relaxed);
    BUCKETS[bucket(layout.size())].fetch_add(layout.size() as u64, Ordering::Relaxed);
    user
}

unsafe fn release(ptr: *mut u8, layout: Layout) -> *mut u8 {
    let key = unsafe { ptr.sub(HEADER).cast::<u32>().read() };
    account(key, layout.size(), -1);
    LIVE.fetch_sub(layout.size() as u64, Ordering::Relaxed);
    BUCKETS[bucket(layout.size())].fetch_sub(layout.size() as u64, Ordering::Relaxed);
    unsafe { ptr.sub(offset(layout)) }
}

unsafe impl GlobalAlloc for Probe {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let base = unsafe { System.alloc(base_layout(layout)) };
        unsafe { finish_alloc(base, layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let base = unsafe { System.alloc_zeroed(base_layout(layout)) };
        unsafe { finish_alloc(base, layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let base = unsafe { release(ptr, layout) };
        unsafe { System.dealloc(base, base_layout(layout)) };
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        let fresh = unsafe { self.alloc(new_layout) };
        if !fresh.is_null() {
            unsafe {
                std::ptr::copy_nonoverlapping(ptr, fresh, layout.size().min(new_size));
                self.dealloc(ptr, layout);
            }
        }
        fresh
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

#[cfg(windows)]
pub fn return_memory_to_os() {
    use windows::Win32::System::Memory::{
        GetProcessHeap, HeapCompact, HeapOptimizeResources, HeapSetInformation, HEAP_FLAGS,
    };
    #[repr(C)]
    struct HeapOptimizeResourcesInformation {
        version: u32,
        flags: u32,
    }
    let before = process_heap();
    let started = std::time::Instant::now();
    if let Ok(heap) = unsafe { GetProcessHeap() } {
        unsafe { HeapCompact(heap, HEAP_FLAGS(0)) };
        let optimize = HeapOptimizeResourcesInformation {
            version: 1,
            flags: 0,
        };
        let result = unsafe {
            HeapSetInformation(
                Some(heap),
                HeapOptimizeResources,
                Some(&optimize as *const _ as *const std::ffi::c_void),
                std::mem::size_of::<HeapOptimizeResourcesInformation>(),
            )
        };
        if memory_report_enabled() {
            eprintln!("aqua-trim: HeapOptimizeResources {result:?}");
        }
    }
    trim_working_set();
    if memory_report_enabled() {
        eprintln!(
            "aqua-trim: heap compact {:.2} ms before={before:?} after={:?}",
            started.elapsed().as_secs_f64() * 1e3,
            process_heap()
        );
    }
}

#[cfg(windows)]
fn trim_working_set() {
    use windows::Win32::System::Memory::{
        SetProcessWorkingSetSizeEx, SETPROCESSWORKINGSETSIZEEX_FLAGS,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;
    let _ = unsafe {
        SetProcessWorkingSetSizeEx(
            GetCurrentProcess(),
            usize::MAX,
            usize::MAX,
            SETPROCESSWORKINGSETSIZEEX_FLAGS(0),
        )
    };
}

#[cfg(not(windows))]
pub fn return_memory_to_os() {}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrivateRegion {
    pub base: usize,
    pub size: usize,
    pub allocation_base: usize,
    pub protection: u32,
}

#[derive(Clone, Debug, Default)]
pub struct AddressSpaceReport {
    pub private_committed: usize,
    pub regions: Vec<PrivateRegion>,
    pub heaps: Vec<ProcessHeap>,
}

#[cfg(windows)]
pub fn address_space() -> AddressSpaceReport {
    use windows::Win32::System::Memory::{
        GetProcessHeaps, HeapSummary, VirtualQuery, HEAP_SUMMARY, MEMORY_BASIC_INFORMATION,
        MEM_COMMIT, MEM_PRIVATE,
    };
    let mut regions = Vec::new();
    let mut cursor = 0usize;
    loop {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        let written = unsafe {
            VirtualQuery(
                Some(cursor as *const std::ffi::c_void),
                &mut info,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if written == 0 {
            break;
        }
        if info.State == MEM_COMMIT && info.Type == MEM_PRIVATE {
            regions.push(PrivateRegion {
                base: info.BaseAddress as usize,
                size: info.RegionSize,
                allocation_base: info.AllocationBase as usize,
                protection: info.Protect.0,
            });
        }
        match (info.BaseAddress as usize).checked_add(info.RegionSize) {
            Some(next) if next > cursor => cursor = next,
            _ => break,
        }
    }
    let mut handles = vec![windows::Win32::Foundation::HANDLE::default(); 256];
    let count = unsafe { GetProcessHeaps(&mut handles) } as usize;
    let heaps = handles
        .iter()
        .take(count.min(256))
        .filter_map(|heap| {
            let mut summary = HEAP_SUMMARY {
                cb: std::mem::size_of::<HEAP_SUMMARY>() as u32,
                ..HEAP_SUMMARY::default()
            };
            unsafe { HeapSummary(*heap, 0, &mut summary) }
                .as_bool()
                .then_some(ProcessHeap {
                    allocated: summary.cbAllocated as u64,
                    committed: summary.cbCommitted as u64,
                })
        })
        .collect();
    AddressSpaceReport {
        private_committed: regions.iter().map(|region| region.size).sum(),
        regions,
        heaps,
    }
}

#[cfg(not(windows))]
pub fn address_space() -> AddressSpaceReport {
    AddressSpaceReport::default()
}

pub fn address_space_summary() -> String {
    let report = address_space();
    let mut by_allocation: std::collections::HashMap<usize, (usize, usize, u32)> =
        std::collections::HashMap::new();
    report.regions.iter().for_each(|region| {
        let entry =
            by_allocation
                .entry(region.allocation_base)
                .or_insert((0, 0, region.protection));
        entry.0 += region.size;
        entry.1 += 1;
    });
    let mut groups = by_allocation.into_iter().collect::<Vec<_>>();
    groups.sort_by_key(|(_, (size, _, _))| std::cmp::Reverse(*size));
    let small = groups
        .iter()
        .filter(|(_, (size, _, _))| *size < 1 << 20)
        .fold((0usize, 0usize), |(bytes, count), (_, (size, _, _))| {
            (bytes + size, count + 1)
        });
    let heap_total = report
        .heaps
        .iter()
        .fold(ProcessHeap::default(), |sum, heap| ProcessHeap {
            allocated: sum.allocated + heap.allocated,
            committed: sum.committed + heap.committed,
        });
    let per_heap = report
        .heaps
        .iter()
        .enumerate()
        .filter(|(_, heap)| heap.committed >= 256 * 1024)
        .map(|(index, heap)| {
            format!(
                "#{index} {:.1}/{:.1}",
                heap.allocated as f64 / 1048576.0,
                heap.committed as f64 / 1048576.0
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut lines = vec![format!("aqua-heaps (allocated/committed MiB): {per_heap}")];
    lines.push(format!(
        "aqua-address-space: private_committed={:.1} MiB regions={} allocations={} heaps={} heap_allocated={:.1} MiB heap_committed={:.1} MiB under_1mib_allocations={} ({:.1} MiB)",
        report.private_committed as f64 / 1048576.0,
        report.regions.len(),
        groups.len(),
        report.heaps.len(),
        heap_total.allocated as f64 / 1048576.0,
        heap_total.committed as f64 / 1048576.0,
        small.1,
        small.0 as f64 / 1048576.0,
    ));
    lines.extend(
        groups
            .iter()
            .take(24)
            .map(|(base, (size, count, protect))| {
                format!(
                    "  allocation {base:#x} {:.2} MiB in {count} regions protect={protect:#x}",
                    *size as f64 / 1048576.0
                )
            }),
    );
    lines.join("\n")
}
