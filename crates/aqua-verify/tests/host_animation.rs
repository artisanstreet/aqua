#![cfg(windows)]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant as WallInstant};

use aqua::render::host::{Clock, DamageSlot, FrameTarget, GpuFrames};
use aqua::render::link::{CompositionSink, InitFactory, RenderLink};
use aqua::render::lower::Lowering;
use aqua::render::service::RenderService;
use aqua_core::geometry::{CornerRadii, Insets};
use aqua_core::scene::{DisplayItem, GpuiQuadPulse};
use aqua_core::{Oklch, Rect, Size};
use aqua_gpu_ref::RefBackend;
use aqua_graph::Extent;
use aqua_protocol::{GpuBackend, RenderEvent, SurfaceTarget};
use aqua_scene::Scene;
use aqua_text::{SharedFonts, SharedRuns};

const EXTENT: Extent = Extent {
    width: 400,
    height: 300,
};

fn bounding(rects: &[Rect]) -> Option<Rect> {
    rects.iter().copied().reduce(Rect::union)
}

fn factory() -> InitFactory {
    Box::new(|context| {
        Box::new(move || {
            let runs = SharedRuns::default();
            let lowering = Lowering::new(SharedFonts::default(), runs.clone());
            let mut backend = RefBackend::new();
            let surface = backend
                .create_surface(SurfaceTarget::Offscreen(EXTENT))
                .map_err(|error| error.to_string())?;
            let slot: DamageSlot = Rc::new(RefCell::new(None));
            Ok(Box::new(
                GpuFrames::new(
                    backend,
                    surface,
                    EXTENT,
                    lowering,
                    context.externals,
                    context.stats,
                )
                .with_damage(slot, runs)
                .with_repaint_probe(|_, _, rects| bounding(rects)),
            ) as Box<dyn FrameTarget>)
        })
    })
}

fn quad(bounds: Rect) -> DisplayItem {
    DisplayItem::Quad {
        bounds,
        corners: CornerRadii::default(),
        fill: Oklch::from_hex("#3366cc").unwrap(),
        border_widths: Insets::uniform(0.0),
        border_color: Oklch::from_hex("#000000").unwrap(),
        dashed: false,
    }
}

fn grid_with_one_pulse() -> Scene {
    let mut scene = Scene::new();
    (0..20).for_each(|column| {
        (0..10).for_each(|row| {
            scene.insert(quad(Rect {
                x: 4.0 + column as f32 * 19.0,
                y: 4.0 + row as f32 * 19.0,
                w: 16.0,
                h: 16.0,
            }));
        })
    });
    scene.insert(DisplayItem::PushGpuiQuadPulse(GpuiQuadPulse {
        start_ms: 0,
        period_ms: 400,
        min_opacity: 0.2,
    }));
    scene.insert(quad(Rect {
        x: 300.0,
        y: 220.0,
        w: 60.0,
        h: 40.0,
    }));
    scene.insert(DisplayItem::PopGpuiQuadPulse);
    scene
}

#[test]
fn a_pulse_repaints_only_its_footprint_and_costs_the_app_no_wakes() {
    let service = RenderService::new().unwrap();
    let origin = WallInstant::now();
    let clock: Clock =
        Arc::new(move || aqua_protocol::Instant::from_secs_f64(origin.elapsed().as_secs_f64()));
    let composition: CompositionSink = Arc::new(|_| {});
    let mut link = RenderLink::spawn(
        &service,
        factory(),
        composition,
        clock,
        Duration::from_millis(2),
        Arc::new(|| {}),
    );
    let mut scene = grid_with_one_pulse();
    link.set_viewport(Size::new(EXTENT.width as f32, EXTENT.height as f32), 1.0);
    link.push_scene(&mut scene);
    let warmup = WallInstant::now();
    let mut events = Vec::new();
    while warmup.elapsed() < Duration::from_millis(300) {
        events.extend(link.poll());
        std::thread::sleep(Duration::from_millis(5));
    }
    let stats = link.stats().clone();
    let frames = stats.frames.load(Ordering::Relaxed);
    let partial = stats.partial_frames.load(Ordering::Relaxed);
    let culled = stats.culled_commands.load(Ordering::Relaxed);
    let scenes = stats.scenes_applied.load(Ordering::Relaxed);
    events.clear();
    let window = WallInstant::now();
    while window.elapsed() < Duration::from_millis(1000) {
        events.extend(link.poll());
        std::thread::sleep(Duration::from_millis(5));
    }
    let frames = stats.frames.load(Ordering::Relaxed) - frames;
    let partial = stats.partial_frames.load(Ordering::Relaxed) - partial;
    let culled = stats.culled_commands.load(Ordering::Relaxed) - culled;
    assert!(frames >= 20, "only {frames} frames in a second");
    assert!(
        partial + 2 >= frames,
        "{partial} of {frames} frames were partial"
    );
    assert!(
        culled >= partial * 150,
        "{culled} commands culled over {partial} partial frames"
    );
    assert_eq!(
        stats.scenes_applied.load(Ordering::Relaxed),
        scenes,
        "the app sent scenes while the pulse ran"
    );
    let presented = events
        .iter()
        .filter(|event| matches!(event, RenderEvent::Presented(_)))
        .count();
    assert_eq!(
        presented, 0,
        "repaint frames woke the app {presented} times"
    );
}
