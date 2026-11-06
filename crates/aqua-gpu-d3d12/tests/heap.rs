#![cfg(windows)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use aqua_gpu_d3d12::scene::{AdapterPreference, OffscreenBackend, SurfaceFormat, TierLimit};
use aqua_gpu_ref::conformance::{cases, Case, Session};
use aqua_protocol::{FrameInputs, GpuBackend};

struct Counting;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static TRACING: AtomicBool = AtomicBool::new(false);
static SITES: Mutex<Vec<String>> = Mutex::new(Vec::new());

thread_local! {
    static INSIDE: Cell<bool> = const { Cell::new(false) };
}

fn record_site() {
    if !TRACING.load(Ordering::Relaxed) {
        return;
    }
    INSIDE.with(|inside| {
        if inside.replace(true) {
            return;
        }
        let trace = std::backtrace::Backtrace::force_capture().to_string();
        let frames: Vec<&str> = trace
            .lines()
            .filter(|line| {
                line.trim_start().starts_with(|c: char| c.is_ascii_digit())
                    && !line.contains("heap::")
                    && !line.contains("std::")
                    && !line.contains("core::")
                    && !line.contains("alloc::")
            })
            .take(10)
            .collect();
        SITES.lock().unwrap().push(frames.join(" <- "));
        inside.set(false);
    });
}
static BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        record_site();
        BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        record_site();
        BYTES.fetch_add(size as u64, Ordering::Relaxed);
        unsafe { System.realloc(pointer, layout, size) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

fn counted<T>(work: impl FnOnce() -> T) -> (T, u64, u64) {
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let bytes = BYTES.load(Ordering::Relaxed);
    let value = work();
    (
        value,
        ALLOCATIONS.load(Ordering::Relaxed) - allocations,
        BYTES.load(Ordering::Relaxed) - bytes,
    )
}

fn named(name: &str) -> Case {
    cases()
        .into_iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("conformance case {name} is missing"))
}

fn settle(backend: &mut OffscreenBackend, case: &Case) -> aqua_protocol::SurfaceId {
    let mut session = Session::new(backend);
    session.render_case(case).unwrap();
    let surface = session.surface(case.plan.extent).unwrap();
    for _ in 0..(aqua_gpu_d3d12::policy::SHRINK_AFTER_FRAMES + 8) {
        session.render(&case.plan).unwrap();
    }
    surface
}

const CASES: [&str; 14] = [
    "isolated_group_opacity",
    "glyph_run_one_to_one",
    "image_unscaled",
    "backdrop_blur_large_radius",
    "content_blur_crossfade_radius",
    "rounded_clip_basic",
    "color_matrix_grayscale_over_stripes",
    "color_matrix_offsets_and_alpha",
    "custom_invert_full_amount",
    "custom_invert_half_amount_fractional_bounds",
    "compute_row_box_blur",
    "slug_rotated_and_skewed",
    "quad_rotated_rounded_bordered",
    "image_rotated_rounded",
];

#[test]
fn steady_state_frames_stay_inside_their_allocation_budget() {
    let mut over: Vec<String> = Vec::new();
    let combinations = [
        (AdapterPreference::Warp, TierLimit::MAX),
        (AdapterPreference::Warp, TierLimit::BASELINE),
        (AdapterPreference::HighPerformance, TierLimit::MAX),
        (AdapterPreference::HighPerformance, TierLimit::BASELINE),
    ];
    for (preference, limit) in combinations {
        let Ok(mut backend) =
            OffscreenBackend::with_tiers(preference, SurfaceFormat::Rgba16Float, limit)
        else {
            continue;
        };
        for name in CASES {
            let case = named(name);
            let surface = settle(&mut backend, &case);
            let inputs = FrameInputs { surface };
            let mut worst = (0, 0);
            if std::env::var("AQUA_HEAP_TRACE").is_ok() {
                SITES.lock().unwrap().clear();
                TRACING.store(true, Ordering::Relaxed);
                backend.execute(&case.plan, &inputs).unwrap();
                TRACING.store(false, Ordering::Relaxed);
                for site in SITES.lock().unwrap().iter() {
                    eprintln!("{name} site: {site}");
                }
            }
            for round in 0..16 {
                if round == 5 && std::env::var("AQUA_HEAP_TRACE").is_ok() {
                    SITES.lock().unwrap().clear();
                    TRACING.store(true, Ordering::Relaxed);
                }
                let (result, allocations, bytes) = counted(|| backend.execute(&case.plan, &inputs));
                result.unwrap();
                if allocations != 0 && std::env::var("AQUA_HEAP_TRACE").is_ok() {
                    eprintln!("{name} frame {round} allocated {allocations}");
                }
                if round == 5 && TRACING.swap(false, Ordering::Relaxed) {
                    for site in SITES.lock().unwrap().iter() {
                        eprintln!("{name} site: {site}");
                    }
                }
                worst = (worst.0.max(allocations), worst.1.max(bytes));
            }
            eprintln!(
                "{preference:?} {limit:?} {name}: {} allocations, {} bytes per steady frame",
                worst.0, worst.1
            );
            if worst != (0, 0) {
                over.push(format!(
                    "{preference:?} {limit:?} {name}: {} allocations / {} bytes in a steady frame, the gate is 0",
                    worst.0, worst.1
                ));
            }
        }
    }
    assert!(over.is_empty(), "{}", over.join("\n"));
}
