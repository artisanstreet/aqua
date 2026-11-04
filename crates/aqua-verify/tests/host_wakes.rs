#![cfg(windows)]

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant as WallInstant};

use aqua::render::host::{Clock, FrameTarget, GpuFrames};
use aqua::render::link::{CompositionSink, InitFactory, RenderLink};
use aqua::render::lower::Lowering;
use aqua::render::service::RenderService;
use aqua_core::geometry::{CornerRadii, Insets};
use aqua_core::scene::{DisplayItem, GpuiQuadPulse};
use aqua_core::{Oklch, Rect, Size};
use aqua_graph::Extent;
use aqua_gpu_ref::RefBackend;
use aqua_protocol::{GpuBackend, SurfaceTarget};
use aqua_scene::Scene;
use aqua_text::{SharedFonts, SharedRuns};

const EXTENT: Extent = Extent {
    width: 320,
    height: 200,
};

fn factory() -> InitFactory {
    Box::new(|context| {
        Box::new(move || {
            let lowering = Lowering::new(SharedFonts::default(), SharedRuns::default());
            let mut backend = RefBackend::new();
            let surface = backend
                .create_surface(SurfaceTarget::Offscreen(EXTENT))
                .map_err(|error| error.to_string())?;
            Ok(Box::new(GpuFrames::new(
                backend,
                surface,
                EXTENT,
                lowering,
                context.externals,
                context.stats,
            )) as Box<dyn FrameTarget>)
        })
    })
}

fn quad() -> DisplayItem {
    DisplayItem::Quad {
        bounds: Rect::from_edges(20.0, 20.0, 220.0, 120.0),
        corners: CornerRadii::default(),
        fill: Oklch::from_hex("#3366cc").unwrap(),
        border_widths: Insets::uniform(0.0),
        border_color: Oklch::from_hex("#000000").unwrap(),
        dashed: false,
    }
}

fn frames_over(link: &mut RenderLink, window: Duration) -> u64 {
    let before = link.stats().frames.load(Ordering::Relaxed);
    let started = WallInstant::now();
    while started.elapsed() < window {
        link.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    link.stats().frames.load(Ordering::Relaxed) - before
}

#[test]
fn a_pulsing_quad_keeps_the_render_thread_producing_frames_until_the_pulse_ends() {
    let service = RenderService::new().unwrap();
    let origin = WallInstant::now();
    let clock: Clock = Arc::new(move || aqua_protocol::Instant::from_secs_f64(origin.elapsed().as_secs_f64()));
    let composition: CompositionSink = Arc::new(|_| {});
    let mut link = RenderLink::spawn(
        &service,
        factory(),
        composition,
        clock,
        Duration::from_millis(2),
        Arc::new(|| {}),
    );
    let mut scene = Scene::new();
    let ids: Vec<_> = [
        DisplayItem::PushGpuiQuadPulse(GpuiQuadPulse {
            start_ms: 0,
            period_ms: 400,
            min_opacity: 0.2,
        }),
        quad(),
        DisplayItem::PopGpuiQuadPulse,
    ]
    .into_iter()
    .map(|item| scene.insert(item))
    .collect();
    link.set_viewport(Size::new(EXTENT.width as f32, EXTENT.height as f32), 1.0);
    link.push_scene(&mut scene);
    let pulsing = frames_over(&mut link, Duration::from_millis(1000));
    assert!(pulsing >= 10, "a pulse presented only {pulsing} frames in a second");

    scene.remove(ids[0]);
    scene.remove(ids[2]);
    link.push_scene(&mut scene);
    frames_over(&mut link, Duration::from_millis(300));
    let settled = frames_over(&mut link, Duration::from_millis(600));
    assert_eq!(settled, 0, "frames kept coming after the pulse ended");
}
