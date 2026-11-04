#![cfg(windows)]

#[allow(dead_code)]
#[path = "../../aqua-gpu-d3d12/tests/hal/window.rs"]
mod test_window;

use aqua::render::lower::{Lowering, RecoverySignal};
use aqua::render::persist::{self, PipelineCacheSaver};
use aqua::ui::Text;
use aqua::UiApp;
use aqua_core::geometry::{CornerRadii, Insets, Rect};
use aqua_core::scene::DisplayItem;
use aqua_core::{Oklch, Size};
use aqua_gpu_d3d12::hal::selection::AdapterPreference;
use aqua_gpu_d3d12::hal::DeviceOptions;
use aqua_gpu_d3d12::window::{TextureRecovery, WindowBackend, WindowOptions};
use aqua_protocol::{Feedback, Instant, RenderCommand, RenderInput, SceneDiff, WindowId};
use aqua_runtime::{emit_scene_diff, RenderSide};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_transport::{GpuRenderHost, HostConfig, Inspector, MonotonicClock, RenderHost};
use test_window::TestWindow;

fn options() -> WindowOptions {
    WindowOptions {
        device: DeviceOptions::default().with_preference(AdapterPreference::HighPerformance),
        ..WindowOptions::default()
    }
}

fn workload(fonts: &SharedFonts, runs: &SharedRuns, size: Size) -> (UiApp, SceneDiff) {
    let mut app = UiApp::new(fonts.clone(), runs.clone());
    app.mount(|_| Text::new("Windowed host").build()).unwrap();
    app.set_viewport(size.w, size.h, 1.0);
    app.build_frame().unwrap();
    [DisplayItem::Quad {
        bounds: Rect {
            x: 8.0,
            y: 60.0,
            w: 120.0,
            h: 40.0,
        },
        corners: CornerRadii {
            top_left: 10.0,
            top_right: 2.0,
            bottom_right: 10.0,
            bottom_left: 2.0,
        },
        fill: Oklch::from_srgb8(30, 90, 200, 1.0),
        border_widths: Insets::uniform(2.0),
        border_color: Oklch::WHITE,
        dashed: true,
    }]
    .into_iter()
    .for_each(|item| {
        app.scene_mut().insert(item);
    });
    let diff = emit_scene_diff(app.scene_mut());
    (app, diff)
}

fn step(side: &mut RenderSide, commands: Vec<RenderCommand>) -> Option<u64> {
    side.step(RenderInput {
        commands,
        feedback: Vec::new(),
        present_at: Instant::ZERO,
    })
    .frame
}

#[test]
fn the_window_backend_renders_recovers_persists_and_reports_gpu_time_through_the_host() {
    let cache_dir = std::env::temp_dir().join(format!("aqua-render-window-{}", std::process::id()));
    std::fs::create_dir_all(&cache_dir).unwrap();
    unsafe { std::env::set_var("LOCALAPPDATA", &cache_dir) };

    let window = TestWindow::open(360, 240);
    let (width, height) = window.client();
    let size = Size::new(width as f32, height as f32);
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let (_app, diff) = workload(&fonts, &runs, size);

    let mut backend =
        WindowBackend::with_pipeline_cache(options(), persist::load_pipeline_cache().as_deref())
            .expect("window backend");
    let signal = RecoverySignal::default();
    let listener = signal.listener();
    let reuploads = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let counted = reuploads.clone();
    backend.set_recovery_listener(Box::new(move |notice| {
        if notice.textures == TextureRecovery::HostMustReupload {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            listener.textures_lost();
        }
    }));
    let inspector = Inspector::new();
    let mut host = GpuRenderHost::new(
        backend,
        Lowering::new(fonts.clone(), runs.clone()).with_recovery(signal),
        MonotonicClock::from_origin(std::time::Instant::now()),
        HostConfig::native(WindowId::new(1).unwrap(), window.raw() as u64, Oklch::BLACK),
    )
    .with_inspector(inspector.clone());
    let mut side = RenderSide::new();
    let frame = step(
        &mut side,
        vec![
            RenderCommand::Viewport { size, scale: 1.0 },
            RenderCommand::Scene(diff),
        ],
    )
    .expect("first frame");

    let mut feedback = host.execute(&mut side, frame);
    assert_eq!(
        feedback,
        vec![Feedback::Presented(frame)],
        "{:?}",
        host.last_error()
    );
    for index in 1..6 {
        window.pump();
        host.wait_frame();
        feedback = host.execute(&mut side, frame + index);
        assert_eq!(feedback, vec![Feedback::Presented(frame + index)]);
    }
    let steady = inspector.snapshot().last.expect("a record");
    assert_eq!(steady.unsupported_items, 0);
    assert!(steady.glyphs > 0);
    assert!(
        steady.gpu_micros.is_some(),
        "timestamps reach the inspector: {steady:?}"
    );
    let textures = host.backend().persistent_stats().unwrap().textures;
    assert!(textures > 0);

    host.backend().inject_device_removal().expect("inject");
    let mut recovered = false;
    for index in 0..8u64 {
        window.pump();
        host.wait_frame();
        let result = host.execute(&mut side, frame + 10 + index);
        recovered |= result.contains(&Feedback::DeviceRecovered);
        if recovered && result.contains(&Feedback::Presented(frame + 10 + index)) {
            break;
        }
    }
    assert!(recovered, "the host recovers the device");
    assert_eq!(reuploads.load(std::sync::atomic::Ordering::SeqCst), 1);
    let after = inspector.snapshot().last.expect("a record");
    assert_eq!(after.unsupported_items, 0);
    assert_eq!(after.glyphs, steady.glyphs);
    assert_eq!(
        host.backend().persistent_stats().unwrap().textures,
        textures
    );

    let mut saver = PipelineCacheSaver::default();
    assert!(saver.save(host.backend().pipeline_cache()));
    assert!(!saver.save(host.backend().pipeline_cache()));
    let reloaded = persist::load_pipeline_cache().expect("the cache file is read back");
    assert!(!reloaded.is_empty());
    WindowBackend::with_pipeline_cache(options(), Some(&reloaded))
        .expect("a backend from the saved cache");

    persist::attach_fallback_index(&fonts);
    let worker = persist::spawn_prewarm(fonts.clone(), persist::prewarm_styles()).expect("worker");
    worker.join().expect("the prewarm worker finishes");
    assert!(persist::save_fallback_index(&fonts) || fonts.lock().fallback_index_len() == 0);
    drop(host);
    let _ = std::fs::remove_dir_all(&cache_dir);
}
