#![cfg(windows)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use aqua::render::host::{FrameTarget, GpuFrames, RenderStats};
use aqua::render::lower::Lowering;
use aqua_core::{DisplayItem, Oklch, Rect};
use aqua_gpu_d3d11::scene::{AdapterPreference, OffscreenBackend, SurfaceFormat};
use aqua_graph::Extent;
use aqua_protocol::probe::{self, Event};
use aqua_protocol::{Instant, RenderCommand, RenderInput, SurfaceTarget};
use aqua_runtime::{emit_scene_diff, RenderSide};
use aqua_scene::Scene;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_verify::heap::{measure, CountingAllocator};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const EXTENT: Extent = Extent {
    width: 640,
    height: 480,
};
const WARMUP: usize = 30;
const FRAMES: usize = 100;

static SPANS: AtomicU64 = AtomicU64::new(0);
static COUNTS: AtomicU64 = AtomicU64::new(0);

fn counting_sink(event: Event) {
    match event {
        Event::Enter(_) => SPANS.fetch_add(1, Ordering::Relaxed),
        Event::Exit(_) => 0,
        Event::Count(..) => COUNTS.fetch_add(1, Ordering::Relaxed),
    };
}

fn scene() -> Scene {
    let mut scene = Scene::new();
    for index in 0..40 {
        scene.insert(DisplayItem::Rect {
            bounds: Rect {
                x: index as f32 * 12.0,
                y: 20.0,
                w: 10.0,
                h: 200.0,
            },
            radius: 3.0,
            fill: Oklch::from_hex("#336699").unwrap(),
            border_width: 0.0,
            border_color: Oklch::from_hex("#000000").unwrap(),
            shadow: None,
        });
    }
    scene
}

fn worst_events(frames: &mut GpuFrames<OffscreenBackend>, side: &mut RenderSide) -> u64 {
    let frame = side
        .step(RenderInput {
            commands: Vec::new(),
            feedback: Vec::new(),
            present_at: Instant::ZERO,
        })
        .frame;
    assert!(frame.is_none());
    let id = 1;
    (0..WARMUP).for_each(|_| {
        frames.present(side, id);
    });
    (0..FRAMES)
        .map(|_| measure(|| frames.present(side, id)).1.heap_events())
        .max()
        .unwrap()
}

#[test]
fn an_active_probe_sink_adds_no_allocations_to_steady_gpu_host_frames() {
    let mut backend =
        OffscreenBackend::with_format(AdapterPreference::Warp, SurfaceFormat::Bgra8Unorm)
            .unwrap_or_else(|error| panic!("no WARP adapter: {error}"));
    let surface = {
        use aqua_protocol::GpuBackend;
        backend
            .create_surface(SurfaceTarget::Offscreen(EXTENT))
            .unwrap()
    };
    let (_updates, externals) = std::sync::mpsc::channel();
    let mut frames = GpuFrames::new(
        backend,
        surface,
        EXTENT,
        Lowering::new(SharedFonts::new(), SharedRuns::new()),
        externals,
        Arc::new(RenderStats::default()),
    );
    let mut side = RenderSide::new();
    let mut app_scene = scene();
    side.step(RenderInput {
        commands: vec![
            RenderCommand::Viewport {
                size: aqua_core::Size {
                    w: EXTENT.width as f32,
                    h: EXTENT.height as f32,
                },
                scale: 1.0,
            },
            RenderCommand::Scene(emit_scene_diff(&mut app_scene)),
        ],
        feedback: Vec::new(),
        present_at: Instant::ZERO,
    });

    assert!(!probe::active());
    let without = worst_events(&mut frames, &mut side);
    assert!(probe::install(counting_sink));
    let spans = SPANS.load(Ordering::Relaxed);
    let counts = COUNTS.load(Ordering::Relaxed);
    let with = worst_events(&mut frames, &mut side);
    assert!(
        SPANS.load(Ordering::Relaxed) > spans && COUNTS.load(Ordering::Relaxed) > counts,
        "the sink saw no spans or counts, so the gate measured nothing"
    );
    eprintln!("steady gpu host frame heap events: without probe {without}, with probe {with}");
    assert!(
        with <= without,
        "the probe adds {} heap events to a steady frame",
        with - without
    );
}
