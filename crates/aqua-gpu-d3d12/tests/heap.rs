#![cfg(windows)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

use aqua_gpu_d3d12::scene::{AdapterPreference, OffscreenBackend, SurfaceFormat, TierLimit};
use aqua_gpu_ref::conformance::{cases, Case, Session};
use aqua_protocol::{FrameInputs, GpuBackend};

struct Counting;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
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
    for _ in 0..8 {
        session.render(&case.plan).unwrap();
    }
    surface
}

struct Budget {
    case: &'static str,
    allocations: u64,
    bytes: u64,
}

const BUDGETS: [Budget; 3] = [
    Budget {
        case: "isolated_group_opacity",
        allocations: 23,
        bytes: 3768,
    },
    Budget {
        case: "glyph_run_one_to_one",
        allocations: 11,
        bytes: 1868,
    },
    Budget {
        case: "image_unscaled",
        allocations: 14,
        bytes: 1644,
    },
];

#[test]
fn steady_state_frames_stay_inside_their_allocation_budget() {
    let mut over: Vec<String> = Vec::new();
    for preference in [AdapterPreference::Warp, AdapterPreference::HighPerformance] {
        let Ok(mut backend) =
            OffscreenBackend::with_tiers(preference, SurfaceFormat::Rgba16Float, TierLimit::MAX)
        else {
            continue;
        };
        for budget in &BUDGETS {
            let case = named(budget.case);
            let surface = settle(&mut backend, &case);
            let inputs = FrameInputs { surface };
            let mut worst = (0, 0);
            for _ in 0..16 {
                let (result, allocations, bytes) = counted(|| backend.execute(&case.plan, &inputs));
                result.unwrap();
                worst = (worst.0.max(allocations), worst.1.max(bytes));
            }
            eprintln!(
                "{preference:?} {}: {} allocations, {} bytes per steady frame",
                budget.case, worst.0, worst.1
            );
            if worst.0 > budget.allocations || worst.1 > budget.bytes {
                over.push(format!(
                    "{preference:?} {}: {} allocations / {} bytes exceed the budget of {} / {}",
                    budget.case, worst.0, worst.1, budget.allocations, budget.bytes
                ));
            }
        }
    }
    assert!(over.is_empty(), "{}", over.join("\n"));
}
