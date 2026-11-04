use aqua_core::geometry::Rect;
use aqua_core::Oklch;
use aqua_gpu_d3d12::hal::budget::{Trim, Visibility};
use aqua_gpu_d3d12::hal::selection::AdapterPreference;
use aqua_gpu_d3d12::window::MemoryBreakdown;
use aqua_graph::{plan_paint, Extent, FramePlan, PaintCommand, Quad};

use crate::backend::{backend, frame, open, scene};
use crate::test_window::TestWindow;

fn blurred(extent: Extent) -> FramePlan {
    let quad = PaintCommand::Quad(Quad {
        bounds: Rect {
            x: 20.0,
            y: 20.0,
            w: 200.0,
            h: 120.0,
        },
        radius: 8.0,
        border_width: 0.0,
        fill: Oklch::from_hex("#3b82f6").unwrap(),
        border_color: Oklch::from_hex("#000000").unwrap(),
    });
    let commands = [
        PaintCommand::PushContentBlur {
            bounds: Rect {
                x: 10.0,
                y: 10.0,
                w: 260.0,
                h: 180.0,
            },
            radius: 6.0,
        },
        quad,
        PaintCommand::PopContentBlur,
    ];
    plan_paint(extent, Oklch::from_hex("#20242c").unwrap(), &commands).unwrap()
}

fn everything() -> Trim {
    Trim {
        shrink_upload_ring: true,
        free_transients: true,
        evict_idle_resources: true,
    }
}

fn breakdown_and_trim(preference: AdapterPreference, label: &str) {
    let mut backend = backend(preference);
    let window = TestWindow::open(640, 400);
    let before = backend.memory_breakdown().unwrap();
    assert_eq!(before.swapchain_bytes, 0);
    let (id, extent) = open(&mut backend, &window);
    let opened = backend.memory_breakdown().unwrap();
    let expected_swapchain = extent.width as u64 * extent.height as u64 * 4 * 2;
    assert!(
        opened.swapchain_bytes >= expected_swapchain,
        "[{label}] swapchain {} below {expected_swapchain}",
        opened.swapchain_bytes
    );
    let plan = blurred(extent);
    for _ in 0..4 {
        frame(&mut backend, &window, id, &plan);
    }
    let warm: MemoryBreakdown = backend.memory_breakdown().unwrap();
    eprintln!("[{label}] before {before:?}");
    eprintln!("[{label}] opened {opened:?}");
    eprintln!(
        "[{label}] warm {warm:?} attributed {} unattributed {}",
        warm.attributed(),
        warm.unattributed()
    );
    assert!(warm.transient_heap_bytes > 0, "[{label}] no transient heap");
    assert!(warm.frame_upload_bytes > 0 && warm.scene_upload_bytes > 0);

    let freed = backend.trim(everything()).unwrap();
    let trimmed = backend.memory_breakdown().unwrap();
    assert_eq!(freed, warm.transient_heap_bytes);
    assert_eq!(trimmed.transient_heap_bytes, 0);
    assert_eq!(trimmed.retired_transient_bytes, 0);
    assert_eq!(trimmed.swapchain_bytes, warm.swapchain_bytes);

    frame(&mut backend, &window, id, &plan);
    let regrown = backend.memory_breakdown().unwrap();
    assert!(regrown.transient_heap_bytes > 0);
    assert_eq!(backend.stats().frames, 5);

    let applied = backend.set_visibility(Visibility::Hidden).unwrap();
    assert_eq!(applied, everything());
    assert_eq!(backend.memory_breakdown().unwrap().transient_heap_bytes, 0);
    backend.set_visibility(Visibility::Visible).unwrap();
    frame(&mut backend, &window, id, &scene(extent, 0.0));
}

#[test]
fn warp_memory_is_attributed_and_trim_releases_transients() {
    breakdown_and_trim(AdapterPreference::Warp, "warp");
}

#[test]
fn hardware_memory_is_attributed_and_trim_releases_transients() {
    breakdown_and_trim(AdapterPreference::HighPerformance, "hardware");
}

fn stages_at_editor_size(preference: AdapterPreference, label: &str) {
    let mut backend = backend(preference);
    let window = TestWindow::open(2560, 1440);
    let (id, extent) = open(&mut backend, &window);
    let plan = blurred(extent);
    let created = backend.memory_breakdown().unwrap();
    frame(&mut backend, &window, id, &plan);
    let first = backend.memory_breakdown().unwrap();
    for _ in 0..120 {
        frame(&mut backend, &window, id, &plan);
    }
    let steady = backend.memory_breakdown().unwrap();
    let mib = |bytes: u64| bytes as f64 / 1048576.0;
    eprintln!(
        "[{label}] {}x{}: surface open {:.1} MiB local (swapchain {:.1}), first present +{:.1} (transients {:.1}), steady +{:.1}, unattributed {:.1}",
        extent.width,
        extent.height,
        mib(created.local_usage),
        mib(created.swapchain_bytes),
        mib(first.local_usage.saturating_sub(created.local_usage)),
        mib(first.transient_heap_bytes),
        mib(steady.local_usage.saturating_sub(first.local_usage)),
        mib(steady.unattributed()),
    );
    assert!(
        steady.local_usage <= first.local_usage + (1 << 20),
        "[{label}] video memory grew from {} to {} over 120 frames",
        first.local_usage,
        steady.local_usage
    );
    assert_eq!(steady.transient_heap_bytes, first.transient_heap_bytes);
}

#[test]
fn warp_editor_size_video_memory_is_steady() {
    stages_at_editor_size(AdapterPreference::Warp, "warp");
}

#[test]
fn hardware_editor_size_video_memory_is_steady() {
    stages_at_editor_size(AdapterPreference::HighPerformance, "hardware");
}
