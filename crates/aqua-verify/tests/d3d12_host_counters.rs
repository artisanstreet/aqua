#![cfg(windows)]

use aqua_gpu_d3d12::scene::{AdapterPreference, OffscreenBackend, SurfaceFormat};
use aqua_gpu_ref::conformance::assets::standard_uploads;
use aqua_gpu_ref::conformance::cases::hex;
use aqua_graph::{plan_paint, Extent};
use aqua_protocol::{FrameInputs, GpuBackend, SurfaceTarget};
use aqua_verify::heap::{measure, CountingAllocator};
use aqua_verify::workload::editor_workload;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const SURFACE: Extent = Extent {
    width: 1280,
    height: 720,
};
const WARMUP: usize = 30;
const FRAMES: usize = 200;
const STEADY_FRAME_HEAP_EVENTS: u64 = 0;

fn steady_frame_heap_events(preference: AdapterPreference) -> Option<(u64, u64)> {
    let mut backend = match OffscreenBackend::with_format(preference, SurfaceFormat::Bgra8Unorm) {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("SKIP-NO-ADAPTER {preference:?}: {error}");
            return None;
        }
    };
    let surface = backend
        .create_surface(SurfaceTarget::Offscreen(SURFACE))
        .unwrap();
    backend.upload(standard_uploads()).unwrap();
    let plan = plan_paint(SURFACE, hex("#000000", 1.0), &editor_workload(SURFACE)).unwrap();
    let inputs = FrameInputs { surface };
    (0..WARMUP).for_each(|_| {
        backend.execute(&plan, &inputs).unwrap();
    });
    let events: Vec<u64> = (0..FRAMES)
        .map(|_| {
            measure(|| backend.execute(&plan, &inputs).unwrap())
                .1
                .heap_events()
        })
        .collect();
    let worst = events.iter().copied().max().unwrap();
    let least = events.iter().copied().min().unwrap();
    eprintln!("d3d12 {preference:?}: steady frame heap events min {least} max {worst}");
    Some((least, worst))
}

#[test]
fn hardware_steady_frames_stay_within_the_heap_event_gate() {
    if let Some((_, worst)) = steady_frame_heap_events(AdapterPreference::HighPerformance) {
        assert!(
            worst == STEADY_FRAME_HEAP_EVENTS,
            "{worst} heap events in a steady frame"
        );
    }
}

#[test]
fn warp_steady_frames_stay_within_the_heap_event_gate() {
    if let Some((_, worst)) = steady_frame_heap_events(AdapterPreference::Warp) {
        assert!(
            worst == STEADY_FRAME_HEAP_EVENTS,
            "{worst} heap events in a steady frame"
        );
    }
}
