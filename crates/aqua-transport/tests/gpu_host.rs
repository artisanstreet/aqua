use std::sync::{Arc, Mutex};

use aqua_core::id::RawId;
use aqua_core::{DisplayItem, NodeId, Oklch, Rect, Size};
use aqua_gpu_ref::RefBackend;
use aqua_graph::FramePlan;
use aqua_motion::{AppMotion, Binding, Easing, MotionKind, MotionValue, Property};
use aqua_protocol::{
    Feedback, FrameInputs, GpuBackend, GpuCapabilities, GpuError, GpuReadback, Instant,
    PresentHint, PresentTimes, RecoveryReport, RenderCommand, RenderInput, SceneDiff, SurfaceId,
    SurfacePixels, SurfaceTarget, UploadBatch,
};
use aqua_runtime::RenderSide;
use aqua_transport::{
    lower_items, GeometryLowering, GpuRenderHost, HostConfig, RenderHost, SharedClock,
};

fn color(hex: &str) -> Oklch {
    Oklch::from_hex(hex).unwrap()
}

fn rect_item(x: f32, fill: Oklch) -> DisplayItem {
    DisplayItem::Rect {
        bounds: Rect {
            x,
            y: 0.0,
            w: 16.0,
            h: 16.0,
        },
        radius: 0.0,
        fill,
        border_width: 0.0,
        border_color: fill,
        shadow: None,
    }
}

fn node(index: u32) -> NodeId {
    NodeId::from_raw(RawId {
        index,
        generation: 0,
    })
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<&'static str>>>);

impl Log {
    fn push(&self, entry: &'static str) {
        self.0.lock().unwrap().push(entry);
    }

    fn take(&self) -> Vec<&'static str> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

struct Probe {
    inner: RefBackend,
    log: Log,
    fail_next_execute: bool,
    hints: Vec<PresentHint>,
    wait_hints: Vec<PresentHint>,
}

impl Probe {
    fn new(log: Log) -> Self {
        Self {
            inner: RefBackend::new(),
            log,
            fail_next_execute: false,
            hints: Vec::new(),
            wait_hints: Vec::new(),
        }
    }
}

impl GpuBackend for Probe {
    type Error = GpuError;

    fn capabilities(&self) -> &GpuCapabilities {
        self.inner.capabilities()
    }

    fn create_surface(&mut self, target: SurfaceTarget) -> Result<SurfaceId, GpuError> {
        self.log.push("create");
        self.inner.create_surface(target)
    }

    fn upload(&mut self, batch: UploadBatch) -> Result<(), GpuError> {
        self.inner.upload(batch)
    }

    fn execute(
        &mut self,
        plan: &FramePlan,
        inputs: &FrameInputs,
    ) -> Result<aqua_protocol::FrameReport, GpuError> {
        self.log.push("execute");
        match std::mem::take(&mut self.fail_next_execute) {
            true => Err(GpuError::DeviceFailure("removed".into())),
            false => self.inner.execute(plan, inputs),
        }
    }

    fn present(&mut self, surface: SurfaceId, hint: PresentHint) -> Result<PresentTimes, GpuError> {
        self.log.push("present");
        self.hints.push(hint);
        self.inner.present(surface, hint)
    }

    fn recover(&mut self) -> Result<RecoveryReport, GpuError> {
        self.log.push("recover");
        self.inner.recover()
    }

    fn wait_for_frame_slot_with_hint(
        &mut self,
        surface: SurfaceId,
        hint: PresentHint,
    ) -> Result<(), GpuError> {
        self.wait_hints.push(hint);
        self.wait_for_frame_slot(surface)
    }

    fn wait_for_frame_slot(&mut self, _surface: SurfaceId) -> Result<(), GpuError> {
        self.log.push("wait");
        Ok(())
    }


}

impl GpuReadback for Probe {
    fn read_surface(&mut self, surface: SurfaceId) -> Result<SurfacePixels, GpuError> {
        self.inner.read_surface(surface)
    }
}

fn host(log: &Log, clock: SharedClock) -> GpuRenderHost<Probe, GeometryLowering, SharedClock> {
    GpuRenderHost::new(
        Probe::new(log.clone()),
        GeometryLowering,
        clock,
        HostConfig::offscreen(color("#000000")),
    )
}

fn input(commands: Vec<RenderCommand>, secs: f64) -> RenderInput {
    RenderInput {
        commands,
        feedback: Vec::new(),
        present_at: Instant::from_secs_f64(secs),
    }
}

fn setup(side: &mut RenderSide, items: Vec<(NodeId, DisplayItem)>) {
    let order = items.iter().map(|(id, _)| *id).collect();
    let output = side.step(input(
        vec![
            RenderCommand::Viewport {
                size: Size::new(32.0, 16.0),
                scale: 1.0,
            },
            RenderCommand::Scene(SceneDiff {
                fills: Vec::new(),
                srgba_fills: Vec::new(),
                upserts: items,
                order: Some(order),
            }),
        ],
        0.0,
    ));
    assert!(output.frame.is_some());
}

fn pixel(
    host: &mut GpuRenderHost<Probe, GeometryLowering, SharedClock>,
    x: usize,
    y: usize,
) -> [u8; 4] {
    let surface = host.surface().unwrap();
    let pixels = host.backend_mut().read_surface(surface).unwrap();
    let at = (y * pixels.extent.width as usize + x) * 4;
    pixels.rgba8[at..at + 4].try_into().unwrap()
}

#[test]
fn a_frame_waits_for_the_slot_once_then_executes_and_presents() {
    let log = Log::default();
    let mut host = host(&log, SharedClock::default());
    let mut side = RenderSide::new();
    setup(&mut side, vec![(node(0), rect_item(0.0, color("#ff0000")))]);
    let feedback = host.execute(&mut side, 1);
    assert_eq!(feedback, vec![Feedback::Presented(1)]);
    assert_eq!(log.take(), vec!["create", "wait", "execute", "present"]);

    host.wait_frame();
    host.wait_frame();
    assert_eq!(log.take(), vec!["wait"]);
    host.execute(&mut side, 2);
    assert_eq!(log.take(), vec!["execute", "present"]);
    assert_eq!(host.counters().plans_built, 1);
    assert_eq!(host.counters().plans_reused, 1);
}

#[test]
fn slot_waits_follow_the_current_presentation_hint() {
    let log = Log::default();
    let mut host = host(&log, SharedClock::default());
    let mut side = RenderSide::new();
    setup(&mut side, vec![(node(0), rect_item(0.0, color("#ff0000")))]);
    host.set_present(PresentHint::Immediate);
    host.execute(&mut side, 1);
    assert_eq!(
        log.take(),
        vec!["create", "wait", "execute", "present"]
    );
    host.set_present(PresentHint::Vsync);
    host.wait_frame();
    assert_eq!(log.take(), vec!["wait"]);
    host.execute(&mut side, 2);
    assert_eq!(log.take(), vec!["execute", "present"]);
    host.set_present(PresentHint::Immediate);
    host.wait_frame();
    assert_eq!(log.take(), vec!["wait"]);
    assert_eq!(host.backend().wait_hints, vec![PresentHint::Immediate, PresentHint::Vsync, PresentHint::Immediate]);
}

#[test]
fn uncapped_present_time_uses_the_current_clock() {
    let clock = SharedClock::default();
    let mut host = host(&Log::default(), clock.clone());
    for nanos in [0, 1_000_000, 1_000_000_000] {
        let now = Instant::from_nanos(nanos);
        clock.set(now);
        assert_eq!(host.present_time(), now);
    }
}

#[test]
fn track_driven_fill_reaches_the_pixels_through_the_host() {
    let log = Log::default();
    let mut host = host(&log, SharedClock::default());
    let mut side = RenderSide::new();
    let red = color("#ff0000");
    let blue = color("#0000ff");
    setup(&mut side, vec![(node(0), rect_item(0.0, red))]);
    host.execute(&mut side, 1);
    assert_eq!(pixel(&mut host, 4, 4)[0], 255);

    let mut app = AppMotion::default();
    let track = app.spawn(
        None,
        Some(Binding {
            node: side.scene().ordered().next().unwrap().0,
            property: Property::Fill,
        }),
        MotionValue::Color(red),
        MotionKind::Tween {
            duration_secs: 0.2,
            easing: Easing::Linear,
        },
        0.0,
    );
    app.set_target(track, MotionValue::Color(blue), 0.0)
        .unwrap();
    let commands = app
        .drain_commands()
        .into_iter()
        .map(RenderCommand::Motion)
        .collect();
    let first = side.step(input(commands, 0.0));
    host.execute(&mut side, first.frame.unwrap_or(2));
    let early = pixel(&mut host, 4, 4);

    let mid = side.step(input(Vec::new(), 0.1));
    host.execute(&mut side, mid.frame.unwrap());
    let halfway = pixel(&mut host, 4, 4);

    let end = side.step(input(Vec::new(), 0.25));
    host.execute(&mut side, end.frame.unwrap());
    let last = pixel(&mut host, 4, 4);

    assert!(early[0] > 200 && early[2] < 60, "starts red: {early:?}");
    assert!(halfway[0] > 20 && halfway[2] > 20, "blends: {halfway:?}");
    assert!(last[0] < 40 && last[2] > 200, "ends blue: {last:?}");
}

#[test]
fn device_failure_reports_loss_then_recovers_and_repaints() {
    let log = Log::default();
    let mut host = host(&log, SharedClock::default());
    let mut side = RenderSide::new();
    setup(&mut side, vec![(node(0), rect_item(0.0, color("#00ff00")))]);
    host.backend_mut().fail_next_execute = true;
    let lost = host.execute(&mut side, 1);
    assert_eq!(lost, vec![Feedback::DeviceLost]);
    log.take();

    let recovered = host.execute(&mut side, 2);
    assert_eq!(
        recovered,
        vec![Feedback::DeviceRecovered, Feedback::Presented(2)]
    );
    assert_eq!(
        log.take(),
        vec!["recover", "create", "wait", "execute", "present"]
    );
    assert_eq!(pixel(&mut host, 4, 4)[1], 255);
}

#[test]
fn lowering_applies_translation_scale_and_balances_groups() {
    let items = [
        DisplayItem::PushTransform(aqua_core::Transform2D::translate(10.0, 5.0)),
        rect_item(2.0, color("#ff0000")),
        DisplayItem::PopTransform,
        DisplayItem::PushOpacity(0.5),
        DisplayItem::PopClip,
        DisplayItem::PushClip {
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                w: 4.0,
                h: 4.0,
            },
            radius: 0.0,
        },
        DisplayItem::Image {
            bounds: Rect::ZERO,
            layer: 0,
            fit: aqua_core::image::ImageFit::Fill,
            position: aqua_core::Point::ZERO,
            source: Rect::ZERO,
            opacity: 1.0,
            rounded_clip: 0.0,
        },
    ];
    let lowered = lower_items(items.iter(), 2.0);
    assert_eq!(lowered.unsupported, 1);
    match &lowered.commands[0] {
        aqua_graph::PaintCommand::Quad(quad) => {
            assert_eq!((quad.bounds.x, quad.bounds.y), (24.0, 10.0));
            assert_eq!((quad.bounds.w, quad.bounds.h), (32.0, 32.0));
        }
        other => panic!("{other:?}"),
    }
    let tail: Vec<_> = lowered.commands[1..]
        .iter()
        .map(std::mem::discriminant)
        .collect();
    use aqua_graph::PaintCommand as P;
    assert_eq!(
        tail,
        [
            P::PushOpacity(0.0),
            P::PushClip(Rect::ZERO),
            P::PopClip,
            P::PopOpacity,
        ]
        .iter()
        .map(std::mem::discriminant)
        .collect::<Vec<_>>()
    );
}

#[test]
fn retained_solids_patch_pixels_and_rebuild_after_geometry_changes() {
    let log = Log::default();
    let mut host = host(&log, SharedClock::default());
    let mut side = RenderSide::new();
    setup(
        &mut side,
        vec![(node(0), solid_rect(0.0, color("#ff0000")))],
    );
    host.execute(&mut side, 1);
    assert!(host.retains_solid_geometry(&side));
    assert_eq!(pixel(&mut host, 4, 4), [255, 0, 0, 255]);
    side.step(input(
        vec![RenderCommand::Scene(SceneDiff {
            fills: vec![aqua_core::FillPatch {
                node: node(0),
                fill: color("#00ff00"),
            }],
            ..SceneDiff::default()
        })],
        1.0,
    ));
    host.execute(&mut side, 2);
    assert!(host.retains_solid_geometry(&side));
    assert_eq!(pixel(&mut host, 4, 4), [0, 255, 0, 255]);
    side.step(input(
        vec![RenderCommand::Scene(SceneDiff {
            upserts: vec![(node(0), solid_rect(16.0, color("#0000ff")))],
            ..SceneDiff::default()
        })],
        2.0,
    ));
    assert!(!host.retains_solid_geometry(&side));
    host.execute(&mut side, 3);
    assert_eq!(pixel(&mut host, 20, 4), [0, 0, 255, 255]);
    assert_eq!(pixel(&mut host, 4, 4), [0, 0, 0, 255]);
}

fn solid_rect(x: f32, fill: Oklch) -> DisplayItem {
    let mut item = rect_item(x, fill);
    if let DisplayItem::Rect { border_color, .. } = &mut item {
        *border_color = Oklch::TRANSPARENT;
    }
    item
}

#[test]
fn native_batches_update_cached_solid_pixels_with_alpha() {
    use aqua_core::{Srgba8, SrgbaFillBatch, SrgbaFillPatch};
    let log = Log::default();
    let mut host = host(&log, SharedClock::default());
    let mut side = RenderSide::new();
    let bounds = Rect {
        x: 0.0,
        y: 0.0,
        w: 16.0,
        h: 16.0,
    };
    setup(
        &mut side,
        vec![(
            node(0),
            DisplayItem::SrgbaRect {
                bounds,
                fill: Srgba8::new(255, 0, 0, 255),
            },
        )],
    );
    host.execute(&mut side, 1);
    assert!(host.retains_solid_geometry(&side));
    assert_eq!(pixel(&mut host, 4, 4), [255, 0, 0, 255]);
    let mut batch = SrgbaFillBatch::default();
    batch.push(SrgbaFillPatch {
        node: node(0),
        fill: Srgba8::new(0, 255, 0, 102),
    });
    side.step(input(
        vec![RenderCommand::Scene(SceneDiff {
            srgba_fills: vec![batch],
            ..SceneDiff::default()
        })],
        0.1,
    ));
    host.execute(&mut side, 2);
    assert!(host.retains_solid_geometry(&side));
    assert_eq!(pixel(&mut host, 4, 4), [0, 102, 0, 255]);
}

#[test]
fn presentation_is_uncapped_until_monitor_sync_is_explicitly_selected() {
    let mut host = host(&Log::default(), SharedClock::default());
    let mut side = RenderSide::new();
    setup(&mut side, vec![(node(0), rect_item(0.0, color("#ff0000")))]);
    assert_eq!(host.execute(&mut side, 1), vec![Feedback::Presented(1)]);
    host.set_present(PresentHint::Vsync);
    assert_eq!(host.execute(&mut side, 2), vec![Feedback::Presented(2)]);
    host.set_present(PresentHint::Immediate);
    assert_eq!(host.execute(&mut side, 3), vec![Feedback::Presented(3)]);
    assert_eq!(host.backend().wait_hints, host.backend().hints);
    assert_eq!(
        host.backend().hints,
        [
            PresentHint::Immediate,
            PresentHint::Vsync,
            PresentHint::Immediate
        ]
    );
}
