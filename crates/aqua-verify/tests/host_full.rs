#![cfg(windows)]

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant as WallInstant};

use aqua::render::host::{FrameTarget, GpuFrames};
use aqua::render::link::InitFactory;
use aqua::render::lower::Lowering;
use aqua::UiApp;
use aqua::{Aqua, AquaConfig};
use aqua_core::{InputEvent, Point, PointerButton, PointerEvent, PointerKind, ScrollDelta};
use aqua_gpu_d3d11::scene::{AdapterPreference, OffscreenBackend, SurfaceFormat};
use aqua_gpu_ref::RefBackend;
use aqua_graph::Extent;
use aqua_protocol::geometry::{AquaWindowId, LogicalSize};
use aqua_protocol::window::WindowEvent;
use aqua_protocol::{
    Feedback, FrameId, GpuBackend, GpuError, GpuReadback, SurfacePixels, SurfaceTarget,
};
use aqua_runtime::RenderSide;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_verify::fingerprint;
use aqua_verify::heap::{measure, CountingAllocator};

#[path = "support/ui.rs"]
mod ui;

use ui::{editor_like, observed, ROW};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;
const NOTCH_UNITS: f32 = 16.0;
const PROBE_COLUMN: usize = 600;
const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Reference,
    D3d11(AdapterPreference),
}

#[derive(Default)]
struct Captured {
    frames: Vec<Capture>,
}

#[derive(Clone)]
struct Capture {
    hash: u64,
    column: Vec<[u8; 4]>,
    pixels: Option<SurfacePixels>,
}

struct Tap<B: GpuBackend<Error = GpuError> + GpuReadback + 'static> {
    inner: GpuFrames<B>,
    sink: Arc<Mutex<Captured>>,
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
            let width = read.extent.width as usize;
            let column = (0..read.extent.height as usize)
                .map(|y| {
                    let at = (y * width + PROBE_COLUMN) * 4;
                    [
                        read.rgba8[at],
                        read.rgba8[at + 1],
                        read.rgba8[at + 2],
                        read.rgba8[at + 3],
                    ]
                })
                .collect();
            self.sink.lock().unwrap().frames.push(Capture {
                hash: fingerprint::of_bytes(&read.rgba8),
                column,
                pixels: Some(read),
            });
        }
        feedback
    }
}

fn factory(
    kind: Kind,
    fonts: SharedFonts,
    runs: SharedRuns,
    sink: Arc<Mutex<Captured>>,
) -> InitFactory {
    let extent = Extent {
        width: WIDTH,
        height: HEIGHT,
    };
    Box::new(move |context| {
        Box::new(move || {
            let lowering = Lowering::new(fonts, runs);
            match kind {
                Kind::Reference => {
                    let mut backend = RefBackend::new();
                    let surface = backend
                        .create_surface(SurfaceTarget::Offscreen(extent))
                        .map_err(|e| e.to_string())?;
                    Ok(Box::new(Tap {
                        inner: GpuFrames::new(
                            backend,
                            surface,
                            extent,
                            lowering,
                            context.externals,
                            context.stats,
                        ),
                        sink,
                    }) as Box<dyn FrameTarget>)
                }
                Kind::D3d11(preference) => {
                    let mut backend =
                        OffscreenBackend::with_format(preference, SurfaceFormat::Bgra8Unorm)
                            .map_err(|e| e.to_string())?;
                    let surface = backend
                        .create_surface(SurfaceTarget::Offscreen(extent))
                        .map_err(|e| e.to_string())?;
                    Ok(Box::new(Tap {
                        inner: GpuFrames::new(
                            backend,
                            surface,
                            extent,
                            lowering,
                            context.externals,
                            context.stats,
                        ),
                        sink,
                    }) as Box<dyn FrameTarget>)
                }
            }
        })
    })
}

struct Host {
    aqua: Aqua,
    app: UiApp,
    id: AquaWindowId,
    sink: Arc<Mutex<Captured>>,
}

fn open(kind: Kind) -> Option<Host> {
    let mut aqua = Aqua::new(AquaConfig::default()).unwrap();
    let mut app = UiApp::new(aqua.fonts().clone(), aqua.runs().clone());
    editor_like(&mut app, WIDTH as f32, HEIGHT as f32);
    let id = aqua.add_headless_window(observed(WIDTH as f32, HEIGHT as f32));
    let sink = Arc::new(Mutex::new(Captured::default()));
    let attached = aqua.attach_render(
        id,
        factory(
            kind,
            aqua.fonts().clone(),
            aqua.runs().clone(),
            sink.clone(),
        ),
        FRAME_INTERVAL,
    );
    assert!(attached);
    aqua.dispatch_window_event(
        &mut app,
        id,
        &WindowEvent::Resized(id, LogicalSize::new(WIDTH as f64, HEIGHT as f64)),
    );
    let mut host = Host {
        aqua,
        app,
        id,
        sink,
    };
    if !host.settle(Duration::from_secs(20)) {
        return None;
    }
    Some(host)
}

impl Host {
    fn pump(&mut self) {
        let now = self.aqua.clock();
        self.aqua.pump_inbox(&mut self.app);
        self.aqua.draw_ready(&mut self.app, now);
        self.aqua.drain_render_events(&mut self.app, self.id);
    }

    fn presented(&self) -> u64 {
        self.aqua.presented_frames(self.id).unwrap_or(0)
    }

    fn settle(&mut self, limit: Duration) -> bool {
        let deadline = WallInstant::now() + limit;
        let mut last = (u64::MAX, WallInstant::now());
        while WallInstant::now() < deadline {
            self.pump();
            let presented = self.presented();
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

    fn input(&mut self, event: InputEvent) {
        self.aqua.dispatch_input(&mut self.app, self.id, &event);
    }

    fn wheel(&mut self, y: f32) {
        self.input(InputEvent::Scroll {
            at: Point::new(300.0, 200.0),
            delta: ScrollDelta { x: 0.0, y },
        });
    }

    fn click(&mut self, at: Point) {
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

    fn frames(&self) -> usize {
        self.sink.lock().unwrap().frames.len()
    }

    fn last(&self) -> Capture {
        self.sink.lock().unwrap().frames.last().cloned().unwrap()
    }

    fn since(&self, from: usize) -> Vec<Capture> {
        self.sink.lock().unwrap().frames[from..].to_vec()
    }
}

fn script(host: &mut Host) -> Vec<u64> {
    let mut finals = Vec::new();
    let steps: [(f32, Option<Point>); 8] = [
        (120.0, None),
        (240.0, None),
        (-120.0, Some(Point::new(300.0, 200.0))),
        (480.0, None),
        (-120.0, Some(Point::new(40.0, 40.0))),
        (240.0, None),
        (960.0, None),
        (-480.0, Some(Point::new(500.0, 300.0))),
    ];
    for (delta, click) in steps {
        host.wheel(delta);
        if let Some(at) = click {
            host.click(at);
        }
        assert!(host.settle(Duration::from_secs(20)), "host never settled");
        finals.push(host.last().hash);
    }
    finals
}

fn adapters() -> Vec<Kind> {
    [
        Kind::Reference,
        Kind::D3d11(AdapterPreference::Warp),
        Kind::D3d11(AdapterPreference::HighPerformance),
        Kind::D3d11(AdapterPreference::MinimumPower),
    ]
    .into()
}

fn open_or_skip(kind: Kind) -> Option<Host> {
    match (open(kind), kind) {
        (Some(host), _) => Some(host),
        (None, _) => panic!("{kind:?} must present"),
    }
}

#[test]
fn the_full_host_replays_a_scripted_session_to_identical_frames_on_every_backend() {
    for kind in adapters() {
        let Some(mut first) = open_or_skip(kind) else {
            continue;
        };
        let a = script(&mut first);
        let Some(mut second) = open_or_skip(kind) else {
            continue;
        };
        let b = script(&mut second);
        assert_eq!(a, b, "{kind:?} diverged between two replays");
        eprintln!("host replay {kind:?}: {} settled frames identical", a.len());
    }
}

fn disagreement(a: &SurfacePixels, b: &SurfacePixels) -> (u8, f64) {
    let worst = a
        .rgba8
        .iter()
        .zip(&b.rgba8)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0);
    let off = a
        .rgba8
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba8.as_chunks::<4>().0)
        .filter(|(x, y)| x.iter().zip(y.iter()).any(|(p, q)| p.abs_diff(*q) > 24))
        .count();
    (worst, off as f64 / (a.rgba8.len() / 4) as f64)
}

#[test]
fn the_full_host_on_d3d11_matches_the_reference_host_after_each_scripted_step() {
    let mut reference = open(Kind::Reference).expect("reference host");
    let mut others: Vec<(Kind, Host)> = adapters()
        .into_iter()
        .skip(1)
        .filter_map(|kind| open_or_skip(kind).map(|host| (kind, host)))
        .collect();
    for (delta, click) in [
        (120.0, None),
        (600.0, Some(Point::new(300.0, 200.0))),
        (-240.0, None),
    ] {
        reference.wheel(delta);
        if let Some(at) = click {
            reference.click(at);
        }
        assert!(reference.settle(Duration::from_secs(20)));
        let expected = reference.last().pixels.unwrap();
        for (kind, host) in &mut others {
            host.wheel(delta);
            if let Some(at) = click {
                host.click(at);
            }
            assert!(host.settle(Duration::from_secs(20)));
            let actual = host.last().pixels.unwrap();
            let (worst, fraction) = disagreement(&expected, &actual);
            eprintln!("host vs reference {kind:?}: worst {worst}, off fraction {fraction:.5}");
            assert!(
                fraction < 0.01,
                "{kind:?}: {:.3}% of pixels differ by more than 24",
                fraction * 100.0
            );
        }
    }
}

fn transition(column: &[[u8; 4]]) -> Option<usize> {
    let first = column.first()?;
    column.iter().position(|pixel| pixel != first)
}

fn glide_steps(frames: &[Capture]) -> Vec<f32> {
    let positions: Vec<f32> = frames
        .iter()
        .filter_map(|frame| transition(&frame.column).map(|y| y as f32))
        .collect();
    positions
        .windows(2)
        .map(|pair| (pair[0] - pair[1]).rem_euclid(ROW))
        .collect()
}

#[test]
fn scroll_glide_on_the_full_host_advances_monotonically_without_jumps() {
    for kind in adapters() {
        let Some(mut host) = open_or_skip(kind) else {
            continue;
        };
        let from = host.frames();
        eprintln!(
            "prefs reduce_motion {:?}; before transition {:?}",
            host.aqua.preferences().reduce_motion,
            transition(&host.last().column)
        );
        let counted_from = host
            .aqua
            .render_stats(host.id)
            .unwrap()
            .frames
            .load(Ordering::Relaxed);
        host.wheel(NOTCH_UNITS);
        assert!(host.settle(Duration::from_secs(20)));
        let frames = host.since(from);
        let measured = host.since(from - 1);
        let counted = host
            .aqua
            .render_stats(host.id)
            .unwrap()
            .frames
            .load(Ordering::Relaxed)
            - counted_from;
        assert_eq!(
            counted,
            frames.len() as u64,
            "{kind:?}: frame counters disagree"
        );
        let paced = kind != Kind::Reference;
        assert!(
            !paced || (5..=30).contains(&frames.len()),
            "{kind:?}: one notch took {} frames",
            frames.len()
        );
        eprintln!("after transition {:?}", transition(&host.last().column));
        let steps: Vec<f32> = glide_steps(&measured)
            .into_iter()
            .filter(|step| step.abs() > 0.0)
            .collect();
        let total: f32 = steps.iter().sum();
        assert!(
            (total - aqua_runtime::glide::NOTCH_PIXELS).abs() <= 3.0,
            "{kind:?}: one notch moved {total} px: {steps:?}"
        );
        eprintln!(
            "glide {kind:?}: {} presented frames, {} moving steps, total {total:.1} px, steps {steps:?}",
            frames.len(),
            steps.len()
        );
        assert!(
            !paced || steps.len() >= 4,
            "{kind:?}: the glide was not animated: {steps:?}"
        );
        let largest = steps.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            !paced || largest <= total.abs() * 0.6,
            "{kind:?}: a single frame jumped {largest} of {total}: {steps:?}"
        );
    }
}

#[test]
fn an_idle_full_host_presents_nothing_and_allocates_nothing_on_the_app_thread() {
    for kind in adapters() {
        let Some(mut host) = open_or_skip(kind) else {
            continue;
        };
        host.wheel(120.0);
        assert!(host.settle(Duration::from_secs(20)));
        let stats = host.aqua.render_stats(host.id).unwrap();
        let presented = host.presented();
        let waits = stats.waits.load(Ordering::Relaxed);
        let mut events = 0;
        let started = WallInstant::now();
        while started.elapsed() < Duration::from_millis(1500) {
            let (_, counts) = measure(|| {
                let now = host.aqua.clock();
                host.aqua.draw_ready(&mut host.app, now);
                host.aqua.drain_render_events(&mut host.app, host.id);
            });
            events += counts.heap_events();
            host.pump();
            std::thread::sleep(Duration::from_millis(5));
        }
        let idle_waits = stats.waits.load(Ordering::Relaxed) - waits;
        eprintln!(
            "idle {kind:?}: presented +{}, render waits +{idle_waits}, app-thread heap events {events}",
            host.presented() - presented
        );
        assert_eq!(host.presented(), presented, "{kind:?} presented while idle");
        assert!(
            idle_waits <= 4,
            "{kind:?}: the render thread woke {idle_waits} times while idle"
        );
        assert!(host.aqua.next_wake(host.aqua.clock()).is_none());
        assert_eq!(
            events, 0,
            "{kind:?}: the idle app thread allocated {events} times"
        );
    }
}

#[test]
fn an_input_produces_exactly_one_scene_submission_and_bounded_frames() {
    let mut host = open(Kind::Reference).expect("reference host");
    let link_before = host
        .aqua
        .render_stats(host.id)
        .unwrap()
        .frames
        .load(Ordering::Relaxed);
    let frames_before = host.frames();
    host.click(Point::new(300.0, 200.0));
    assert!(host.settle(Duration::from_secs(20)));
    let produced = host.frames() - frames_before;
    let counted = host
        .aqua
        .render_stats(host.id)
        .unwrap()
        .frames
        .load(Ordering::Relaxed)
        - link_before;
    eprintln!("click produced {produced} frames ({counted} counted by the host)");
    assert!(produced <= 3, "a click presented {produced} frames");
    assert_eq!(produced as u64, counted);
}
