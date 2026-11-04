#![allow(dead_code)]

use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant as WallInstant};

use aqua::render::host::{FrameTarget, GpuFrames, RenderStats};
use aqua::render::link::{ExternalUpdate, InitFactory};
use aqua::render::lower::Lowering;
use aqua::UiApp;
use aqua::{Aqua, AquaConfig};
use aqua_core::{InputEvent, Point, PointerButton, PointerEvent, PointerKind, ScrollDelta};
use aqua_gpu_d3d12::scene::{AdapterPreference, OffscreenBackend, SurfaceFormat};
use aqua_gpu_ref::RefBackend;
use aqua_graph::Extent;
use aqua_protocol::geometry::{AquaWindowId, LogicalSize};
use aqua_protocol::window::WindowEvent;
use aqua_protocol::{
    Feedback, FrameId, GpuBackend, GpuError, GpuReadback, SurfaceId, SurfaceTarget,
};
use aqua_runtime::RenderSide;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_verify::fingerprint;

use super::ui::{editor_like, observed};

pub const WIDTH: u32 = 640;
pub const HEIGHT: u32 = 480;

pub fn extent() -> Extent {
    Extent {
        width: WIDTH,
        height: HEIGHT,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Reference,
    D3d12(AdapterPreference),
}

pub const KINDS: [Kind; 2] = [Kind::Reference, Kind::D3d12(AdapterPreference::Warp)];

#[derive(Default)]
pub struct Captured {
    pub hashes: Vec<u64>,
}

pub struct Tap<B: GpuBackend<Error = GpuError> + GpuReadback + 'static> {
    pub inner: GpuFrames<B>,
    pub sink: Arc<Mutex<Captured>>,
}

impl<B: GpuBackend<Error = GpuError> + GpuReadback + 'static> FrameTarget for Tap<B> {
    fn paced(&self) -> bool {
        true
    }

    fn wait_frame(&mut self) {
        self.inner.wait_frame();
    }

    fn present(&mut self, side: &mut RenderSide, frame: FrameId) -> Vec<Feedback> {
        let feedback = self.inner.present(side, frame);
        let surface = self.inner.surface();
        if let Ok(read) = self.inner.backend_mut().read_surface(surface) {
            self.sink
                .lock()
                .unwrap()
                .hashes
                .push(fingerprint::of_bytes(&read.rgba8));
        }
        feedback
    }
}

pub fn target(
    kind: Kind,
    lowering: Lowering,
    externals: Receiver<ExternalUpdate>,
    stats: Arc<RenderStats>,
    sink: Arc<Mutex<Captured>>,
) -> Result<Box<dyn FrameTarget>, String> {
    match kind {
        Kind::Reference => {
            let mut backend = RefBackend::new();
            let surface = backend
                .create_surface(SurfaceTarget::Offscreen(extent()))
                .map_err(|e| e.to_string())?;
            Ok(Box::new(Tap {
                inner: GpuFrames::new(backend, surface, extent(), lowering, externals, stats),
                sink,
            }))
        }
        Kind::D3d12(preference) => {
            let mut backend = OffscreenBackend::with_format(preference, SurfaceFormat::Bgra8Unorm)
                .map_err(|e| e.to_string())?;
            let surface: SurfaceId = backend
                .create_surface(SurfaceTarget::Offscreen(extent()))
                .map_err(|e| e.to_string())?;
            Ok(Box::new(Tap {
                inner: GpuFrames::new(backend, surface, extent(), lowering, externals, stats),
                sink,
            }))
        }
    }
}

fn factory(
    kind: Kind,
    fonts: SharedFonts,
    runs: SharedRuns,
    sink: Arc<Mutex<Captured>>,
) -> InitFactory {
    Box::new(move |context| {
        Box::new(move || {
            target(
                kind,
                Lowering::new(fonts, runs),
                context.externals,
                context.stats,
                sink,
            )
        })
    })
}

pub struct Host {
    pub aqua: Aqua,
    pub app: UiApp,
    pub id: AquaWindowId,
    pub sink: Arc<Mutex<Captured>>,
}

pub fn open(kind: Kind) -> Option<Host> {
    let mut aqua = Aqua::new(AquaConfig::default()).unwrap();
    let mut app = UiApp::new(aqua.fonts().clone(), aqua.runs().clone());
    editor_like(&mut app, WIDTH as f32, HEIGHT as f32);
    let id = aqua.add_headless_window(observed(WIDTH as f32, HEIGHT as f32));
    assert!(app.record_session(id));
    let sink = Arc::new(Mutex::new(Captured::default()));
    let attached = aqua.attach_render(
        id,
        factory(kind, aqua.fonts().clone(), aqua.runs().clone(), sink.clone()),
        Duration::from_millis(2),
    );
    assert!(attached);
    aqua.dispatch_window_event(
        &mut app,
        id,
        &WindowEvent::Resized(id, LogicalSize::new(f64::from(WIDTH), f64::from(HEIGHT))),
    );
    let mut host = Host {
        aqua,
        app,
        id,
        sink,
    };
    host.settle(Duration::from_secs(20)).then_some(host)
}

impl Host {
    pub fn pump(&mut self) {
        let now = self.aqua.clock();
        self.aqua.pump_inbox(&mut self.app);
        self.aqua.draw_ready(&mut self.app, now);
        self.aqua.drain_render_events(&mut self.app, self.id);
    }

    pub fn settle(&mut self, limit: Duration) -> bool {
        let deadline = WallInstant::now() + limit;
        let mut last = (u64::MAX, WallInstant::now());
        while WallInstant::now() < deadline {
            self.pump();
            let presented = self.aqua.presented_frames(self.id).unwrap_or(0);
            if presented != last.0 {
                last = (presented, WallInstant::now());
            }
            let quiet = last.1.elapsed() > Duration::from_millis(250);
            let idle = self.aqua.next_wake(self.aqua.clock()).is_none();
            if presented > 0 && quiet && idle {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    pub fn input(&mut self, event: InputEvent) {
        self.aqua.dispatch_input(&mut self.app, self.id, &event);
    }

    pub fn wheel(&mut self, y: f32) {
        self.input(InputEvent::Scroll {
            at: Point::new(300.0, 200.0),
            delta: ScrollDelta { x: 0.0, y },
        });
    }

    pub fn click(&mut self, at: Point) {
        let pointer = |buttons| PointerEvent {
            contact: 0,
            kind: PointerKind::Mouse,
            position: at,
            buttons,
        };
        self.input(InputEvent::PointerMove(pointer(0)));
        self.input(InputEvent::PointerDown(pointer(1), PointerButton::Primary));
        self.input(InputEvent::PointerUp(pointer(0), PointerButton::Primary));
    }

    pub fn last_hash(&self) -> Option<u64> {
        self.sink.lock().unwrap().hashes.last().copied()
    }
}

pub fn scripted_session(host: &mut Host) {
    let steps: [(f32, Option<Point>); 6] = [
        (120.0, None),
        (240.0, None),
        (-120.0, Some(Point::new(300.0, 200.0))),
        (480.0, None),
        (-120.0, Some(Point::new(40.0, 40.0))),
        (960.0, None),
    ];
    for (delta, click) in steps {
        host.wheel(delta);
        if let Some(at) = click {
            host.click(at);
        }
        assert!(host.settle(Duration::from_secs(20)), "host never settled");
    }
}
