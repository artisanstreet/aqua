use aqua::render::lower::{Lowering, RecoverySignal};
use aqua::ui::Text;
use aqua::UiApp;
use aqua_core::geometry::{CornerRadii, Insets, Rect};
use aqua_core::scene::{DisplayItem, ShadowLayer};
use aqua_core::{Oklch, Size};
use aqua_gpu_ref::RefBackend;
use aqua_graph::{Extent, FramePlan};
use aqua_protocol::{
    Feedback, FrameInputs, FrameReport, GpuBackend, GpuCapabilities, GpuError, GpuReadback,
    Instant, PresentHint, PresentTimes, RecoveryReport, RenderCommand, RenderInput, SceneDiff,
    SurfaceId, SurfacePixels, SurfaceTarget, UploadBatch,
};
use aqua_runtime::{emit_scene_diff, RenderSide};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_transport::{GpuRenderHost, HostConfig, Inspector, RenderHost, SharedClock};

struct Flaky {
    inner: RefBackend,
    fail_next: bool,
    waits: Arc<Mutex<Vec<PresentHint>>>,
}

impl Flaky {
    fn new() -> Self {
        Self {
            inner: RefBackend::new(),
            fail_next: false,
            waits: Arc::default(),
        }
    }
}

impl GpuBackend for Flaky {
    type Error = GpuError;

    fn capabilities(&self) -> &GpuCapabilities {
        self.inner.capabilities()
    }

    fn create_surface(&mut self, target: SurfaceTarget) -> Result<SurfaceId, GpuError> {
        self.inner.create_surface(target)
    }

    fn upload(&mut self, batch: UploadBatch) -> Result<(), GpuError> {
        self.inner.upload(batch)
    }

    fn execute(&mut self, plan: &FramePlan, inputs: &FrameInputs) -> Result<FrameReport, GpuError> {
        match std::mem::take(&mut self.fail_next) {
            true => Err(GpuError::DeviceFailure("injected loss".into())),
            false => self.inner.execute(plan, inputs),
        }
    }

    fn present(&mut self, surface: SurfaceId, hint: PresentHint) -> Result<PresentTimes, GpuError> {
        self.inner.present(surface, hint)
    }

    fn recover(&mut self) -> Result<RecoveryReport, GpuError> {
        self.inner = RefBackend::new();
        self.inner.recover()
    }

    fn resize_surface(&mut self, surface: SurfaceId, extent: Extent) -> Result<(), GpuError> {
        self.inner.resize_surface(surface, extent)
    }

    fn destroy_surface(&mut self, surface: SurfaceId) -> Result<(), GpuError> {
        self.inner.destroy_surface(surface)
    }

    fn wait_for_present_slot(
        &mut self,
        _surface: SurfaceId,
        hint: PresentHint,
    ) -> Result<(), GpuError> {
        self.waits.lock().unwrap().push(hint);
        Ok(())
    }
}

#[test]
fn frame_mode_is_selected_before_acquiring_the_slot() {
    let (diff, fonts, runs, _app) = workload();
    let mut side = RenderSide::new();
    let frame = start(&mut side, diff);
    let mut backend = Flaky::new();
    let waits = backend.waits.clone();
    let extent = Extent {
        width: 160,
        height: 64,
    };
    let surface = backend
        .create_surface(SurfaceTarget::Offscreen(extent))
        .unwrap();
    let (_sender, receiver) = std::sync::mpsc::channel();
    let stats = Arc::new(RenderStats::default());
    stats.immediate.store(true, Ordering::Relaxed);
    let mut frames = GpuFrames::new(
        backend,
        surface,
        extent,
        Lowering::new(fonts, runs),
        receiver,
        stats.clone(),
    );
    frames.wait_frame();
    assert_eq!(*waits.lock().unwrap(), [PresentHint::Immediate]);
    assert_eq!(
        frames.present(&mut side, frame),
        [Feedback::Presented(frame)]
    );
    stats.immediate.store(false, Ordering::Relaxed);
    frames.wait_frame();
    assert_eq!(
        *waits.lock().unwrap(),
        [PresentHint::Immediate, PresentHint::Vsync]
    );
}

impl GpuReadback for Flaky {
    fn read_surface(&mut self, surface: SurfaceId) -> Result<SurfacePixels, GpuError> {
        self.inner.read_surface(surface)
    }
}

type Host = GpuRenderHost<Flaky, Lowering, SharedClock>;

const SIZE: Size = Size { w: 160.0, h: 64.0 };

fn workload() -> (SceneDiff, SharedFonts, SharedRuns, UiApp) {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut app = UiApp::new(fonts.clone(), runs.clone());
    app.mount(|_| Text::new("Host path").build()).unwrap();
    app.set_viewport(SIZE.w, SIZE.h, 1.0);
    app.build_frame().unwrap();
    let extra = [
        DisplayItem::Quad {
            bounds: Rect {
                x: 4.0,
                y: 40.0,
                w: 60.0,
                h: 20.0,
            },
            corners: CornerRadii {
                top_left: 8.0,
                top_right: 0.0,
                bottom_right: 0.0,
                bottom_left: 0.0,
            },
            fill: Oklch::from_srgb8(0, 0, 255, 1.0),
            border_widths: Insets::uniform(1.0),
            border_color: Oklch::WHITE,
            dashed: true,
        },
        DisplayItem::Shadow {
            bounds: Rect {
                x: 80.0,
                y: 36.0,
                w: 60.0,
                h: 24.0,
            },
            corners: CornerRadii::uniform(4.0),
            layer: ShadowLayer {
                dx: 0.0,
                dy: 0.0,
                blur: 6.0,
                spread: 0.0,
                inset: true,
                color: Oklch::BLACK,
            },
        },
    ];
    extra.into_iter().for_each(|item| {
        app.scene_mut().insert(item);
    });
    let diff = emit_scene_diff(app.scene_mut());
    (diff, fonts, runs, app)
}

fn host(fonts: SharedFonts, runs: SharedRuns, signal: RecoverySignal) -> (Host, Inspector) {
    let inspector = Inspector::new();
    let lowering = Lowering::new(fonts, runs).with_recovery(signal);
    let host = GpuRenderHost::new(
        Flaky::new(),
        lowering,
        SharedClock::default(),
        HostConfig::offscreen(Oklch::BLACK),
    )
    .with_inspector(inspector.clone());
    (host, inspector)
}

fn start(side: &mut RenderSide, diff: SceneDiff) -> u64 {
    let output = side.step(RenderInput {
        commands: vec![
            RenderCommand::Viewport {
                size: SIZE,
                scale: 1.0,
            },
            RenderCommand::Scene(diff),
        ],
        feedback: Vec::new(),
        present_at: Instant::ZERO,
    });
    output.frame.expect("a frame is due")
}

fn glyph_pixels(host: &mut Host) -> usize {
    let surface = host.surface().unwrap();
    let pixels = host.backend_mut().read_surface(surface).unwrap();
    pixels
        .rgba8
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[3] > 32)
        .count()
}

#[test]
fn the_editor_class_workload_presents_with_nothing_unsupported_and_reports_its_counters() {
    let (diff, fonts, runs, _app) = workload();
    let (mut host, inspector) = host(fonts, runs, RecoverySignal::default());
    let mut side = RenderSide::new();
    let frame = start(&mut side, diff);
    assert_eq!(
        host.execute(&mut side, frame),
        vec![Feedback::Presented(frame)]
    );
    let snapshot = inspector.snapshot();
    let record = snapshot.last.expect("a frame record");
    assert_eq!(record.unsupported_items, 0);
    assert!(record.glyphs > 0 && record.commands > 3);
    assert!(glyph_pixels(&mut host) > 200);
    assert_eq!(host.counters().presented, 1);
    assert_eq!(host.counters().failed, 0);
}

#[test]
fn a_lost_device_clears_the_atlas_so_the_next_frame_uploads_it_again() {
    let (diff, fonts, runs, _app) = workload();
    let (mut host, _) = host(fonts, runs, RecoverySignal::default());
    let mut side = RenderSide::new();
    let frame = start(&mut side, diff);
    host.execute(&mut side, frame);
    let steady = host.execute(&mut side, frame + 1);
    assert_eq!(steady, vec![Feedback::Presented(frame + 1)]);
    let before = glyph_pixels(&mut host);

    host.backend_mut().fail_next = true;
    let lost = host.execute(&mut side, frame + 2);
    assert_eq!(lost, vec![Feedback::DeviceLost]);
    let recovered = host.execute(&mut side, frame + 3);
    assert!(recovered.contains(&Feedback::DeviceRecovered));
    assert!(recovered.contains(&Feedback::Presented(frame + 3)));
    assert_eq!(glyph_pixels(&mut host), before);
}

#[test]
fn an_explicit_trim_request_releases_unused_atlas_pages_between_frames() {
    let (diff, fonts, runs, _app) = workload();
    let (mut host, inspector) = host(fonts, runs, RecoverySignal::default());
    let mut side = RenderSide::new();
    let frame = start(&mut side, diff);
    host.execute(&mut side, frame);
    let blank = SceneDiff {
        fills: Vec::new(),
        upserts: Vec::new(),
        order: Some(Vec::new()),
    };
    let cleared = side.step(RenderInput {
        commands: vec![RenderCommand::Scene(blank)],
        feedback: Vec::new(),
        present_at: Instant::ZERO,
    });
    host.trim_handle().request();
    host.execute(&mut side, cleared.frame.unwrap_or(frame + 1));
    let trimmed = inspector.snapshot().last.unwrap().trimmed_entries;
    assert!(trimmed > 0, "{trimmed}");
}
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use aqua::render::host::{FrameTarget, GpuFrames, RenderStats};
