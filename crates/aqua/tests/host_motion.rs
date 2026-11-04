#![cfg(windows)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant as WallInstant};

use aqua::render::host::{FrameTarget, GpuFrames};
use aqua::render::link::{ExternalUpdate, InitFactory};
use aqua::render::lower::Lowering;
use aqua::render::transition::{Settings, Sweep};
use aqua::UiApp;
use aqua::{Aqua, AquaConfig};
use aqua_core::{Oklch, Point};
use aqua_gpu_d3d12::scene::{AdapterPreference, OffscreenBackend, SurfaceFormat};
use aqua_graph::Extent;
use aqua_protocol::geometry::{AquaWindowId, LogicalSize};
use aqua_protocol::window::{ObservedWindowState, WindowEvent};
use aqua_protocol::{
    Feedback, FrameId, GpuBackend, GpuError, GpuReadback, SurfaceTarget,
};
use aqua_reactive::WriteSignal;
use aqua_runtime::{PopupOptions, RenderSide};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::Box as UiBox;
use aqua_ui::motion::{Animate, Motion, Presence};
use aqua_ui::style::{px, Axis, Style};
use aqua_motion::Easing;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;
const SLIDE: f32 = 80.0;
const ROW_Y: usize = 140;
const WIDE_Y: usize = 220;
const POPUP_X: usize = 340;
const MOTION: Motion = Motion::tween(0.4, Easing::Linear);

struct Probes {
    row: Vec<[u8; 4]>,
    wide: Vec<[u8; 4]>,
    popup: Vec<[u8; 4]>,
    near: [u8; 4],
    far: [u8; 4],
}

impl Probes {
    fn left(&self, line: &[[u8; 4]]) -> Option<usize> {
        line.iter().position(|pixel| pixel[3] > 8)
    }

    fn right(&self, line: &[[u8; 4]]) -> Option<usize> {
        line.iter().rposition(|pixel| pixel[3] > 8)
    }
}

struct Tap<B: GpuBackend<Error = GpuError> + GpuReadback + 'static> {
    inner: GpuFrames<B>,
    sink: Arc<Mutex<Vec<Probes>>>,
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
            let height = read.extent.height as usize;
            let pixel = |x: usize, y: usize| {
                let at = (y * width + x) * 4;
                [
                    read.rgba8[at],
                    read.rgba8[at + 1],
                    read.rgba8[at + 2],
                    read.rgba8[at + 3],
                ]
            };
            let probes = Probes {
                row: (0..480).map(|x| pixel(x, ROW_Y)).collect(),
                wide: (0..480).map(|x| pixel(x, WIDE_Y)).collect(),
                popup: (0..height).map(|y| pixel(POPUP_X, y)).collect(),
                near: pixel(510, 4),
                far: pixel(width - 4, height - 4),
            };
            self.sink.lock().unwrap().push(probes);
        }
        feedback
    }

    fn invalidate(&mut self) {
        self.inner.invalidate();
    }

    fn repaint(&self) -> aqua::render::host::Repaint {
        self.inner.repaint()
    }
}

fn factory(
    preference: AdapterPreference,
    fonts: SharedFonts,
    runs: SharedRuns,
    sink: Arc<Mutex<Vec<Probes>>>,
) -> InitFactory {
    let extent = Extent {
        width: WIDTH,
        height: HEIGHT,
    };
    Box::new(move |context| {
        Box::new(move || {
            let mut backend = OffscreenBackend::with_format(preference, SurfaceFormat::Bgra8Unorm)
                .map_err(|e| e.to_string())?;
            let surface = backend
                .create_surface(SurfaceTarget::Offscreen(extent))
                .map_err(|e| e.to_string())?;
            Ok(Box::new(Tap {
                inner: GpuFrames::new(
                    backend,
                    surface,
                    extent,
                    Lowering::new(fonts, runs),
                    context.externals,
                    context.stats,
                ),
                sink,
            }) as Box<dyn FrameTarget>)
        })
    })
}

struct Signals {
    rows: WriteSignal<Vec<u32>>,
    wide: WriteSignal<bool>,
    ground: WriteSignal<bool>,
}

struct Host {
    aqua: Aqua,
    app: UiApp,
    id: AquaWindowId,
    sink: Arc<Mutex<Vec<Probes>>>,
    signals: Signals,
}

fn red() -> Oklch {
    Oklch::from_hex("#ff0000").unwrap()
}

fn observed() -> ObservedWindowState {
    ObservedWindowState {
        id: AquaWindowId::new(1).unwrap(),
        title: "host-motion".into(),
        inner_size: LogicalSize::new(f64::from(WIDTH), f64::from(HEIGHT)),
        scale_factor: 1.0,
        focused: true,
        visible: true,
        occluded: false,
        maximized: false,
    }
}

fn mount(app: &mut UiApp) -> Signals {
    let (rows, set_rows) = app.scope().signal(vec![1u32, 2, 3]);
    let (wide, set_wide) = app.scope().signal(false);
    let (ground, set_ground) = app.scope().signal(false);
    app.mount_reactive((rows, wide, ground), |(rows, wide, ground)| {
        let backdrop = match ground {
            true => "#0000ff",
            false => "#ff0000",
        };
        UiBox::new()
            .style(
                Style::new()
                    .axis(Axis::Column)
                    .width(px(WIDTH as f32))
                    .height(px(HEIGHT as f32)),
            )
            .child(
                UiBox::new()
                    .style(
                        Style::new()
                            .axis(Axis::Column)
                            .width(px(WIDTH as f32))
                            .height(px(200.0))
                            .flex_shrink(0.0),
                    )
                    .children(rows.iter().map(|id| {
                        UiBox::new()
                            .style(Style::new().width(px(200.0)).height(px(40.0)).flex_shrink(0.0))
                            .fill(red())
                            .presence(Presence::new().enter(MOTION).exit(MOTION).slide(SLIDE, 0.0))
                            .build()
                            .with_key(*id)
                    }))
                    .build(),
            )
            .child(
                UiBox::new()
                    .style(
                        Style::new()
                            .width(px(if wide { 300.0 } else { 100.0 }))
                            .height(px(40.0))
                            .flex_shrink(0.0),
                    )
                    .fill(red())
                    .animate(Animate::new().frame(MOTION))
                    .build(),
            )
            .child(
                UiBox::new()
                    .style(
                        Style::new()
                            .width(px(140.0))
                            .height(px(HEIGHT as f32))
                            .absolute_at(Some(500.0), Some(0.0), None, None),
                    )
                    .fill(Oklch::from_hex(backdrop).unwrap())
                    .build(),
            )
            .build()
    })
    .unwrap();
    Signals {
        rows: set_rows,
        wide: set_wide,
        ground: set_ground,
    }
}

fn open(preference: AdapterPreference) -> Host {
    let mut aqua = Aqua::new(AquaConfig::default()).unwrap();
    let mut app = UiApp::new(aqua.fonts().clone(), aqua.runs().clone());
    let signals = mount(&mut app);
    let id = aqua.add_headless_window(observed());
    let sink = Arc::new(Mutex::new(Vec::new()));
    assert!(aqua.attach_render(
        id,
        factory(
            preference,
            aqua.fonts().clone(),
            aqua.runs().clone(),
            sink.clone()
        ),
        Duration::from_millis(2),
    ));
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
        signals,
    };
    assert!(host.settle(Duration::from_secs(20)), "host never presented");
    host
}

impl Host {
    fn pump(&mut self) {
        let now = self.aqua.clock();
        self.aqua.pump_inbox(&mut self.app);
        self.aqua.draw_ready(&mut self.app, now);
        self.aqua.drain_render_events(&mut self.app, self.id);
    }

    fn settle(&mut self, limit: Duration) -> bool {
        let deadline = WallInstant::now() + limit;
        let mut last = (u64::MAX, WallInstant::now());
        while WallInstant::now() < deadline {
            self.pump();
            let presented = self.frames() as u64;
            if presented != last.0 {
                last = (presented, WallInstant::now());
            }
            let quiet = last.1.elapsed() > Duration::from_millis(300);
            let idle = self.aqua.next_wake(self.aqua.clock()).is_none();
            if presented > 0 && quiet && idle {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    fn poke(&mut self) {
        let id = self.id;
        self.aqua.dispatch_window_event(
            &mut self.app,
            id,
            &WindowEvent::Resized(id, LogicalSize::new(f64::from(WIDTH), f64::from(HEIGHT))),
        );
    }

    fn frames(&self) -> usize {
        self.sink.lock().unwrap().len()
    }

    fn series<T>(&self, from: usize, read: impl Fn(&Probes) -> T) -> Vec<T> {
        self.sink.lock().unwrap()[from..].iter().map(read).collect()
    }
}

fn adapters() -> [AdapterPreference; 2] {
    [AdapterPreference::Warp, AdapterPreference::HighPerformance]
}

fn distinct(values: &[usize]) -> usize {
    let mut seen: Vec<usize> = values.to_vec();
    seen.dedup();
    seen.len()
}

fn assert_glide(label: &str, positions: &[usize], from: usize, to: usize) {
    assert_eq!(positions.last().copied(), Some(to), "{label}: {positions:?}");
    assert!(
        distinct(positions) >= 5,
        "{label}: only {} distinct positions: {positions:?}",
        distinct(positions)
    );
    let direction = (to as i64 - from as i64).signum();
    assert!(
        positions
            .windows(2)
            .all(|pair| (pair[1] as i64 - pair[0] as i64) * direction >= 0),
        "{label}: not monotonic: {positions:?}"
    );
    let total = (to as i64 - from as i64).unsigned_abs() as usize;
    let largest = positions
        .windows(2)
        .map(|pair| pair[1].abs_diff(pair[0]))
        .max()
        .unwrap_or(0);
    assert!(
        largest * 100 <= total * 60,
        "{label}: a single frame moved {largest} of {total}: {positions:?}"
    );
}

#[test]
fn keyed_presence_slides_in_and_out_across_many_presented_frames() {
    for preference in adapters() {
        let mut host = open(preference);
        let label = format!("{preference:?}");
        let from = host.frames();
        host.signals.rows.set(vec![1, 2, 3, 4]);
        host.poke();
        assert!(host.settle(Duration::from_secs(20)));
        let lefts: Vec<usize> = host
            .series(from, |p| p.left(&p.row))
            .into_iter()
            .flatten()
            .collect();
        eprintln!("{label} slide in: {lefts:?}");
        assert_glide(&format!("{label} enter"), &lefts, SLIDE as usize, 0);

        let from = host.frames();
        host.signals.rows.set(vec![1, 2, 3]);
        host.poke();
        assert!(host.settle(Duration::from_secs(20)));
        let series = host.series(from, |p| p.left(&p.row));
        let lefts: Vec<usize> = series.iter().copied().flatten().collect();
        eprintln!("{label} slide out: {lefts:?}");
        assert!(
            lefts.windows(2).all(|pair| pair[1] >= pair[0]),
            "{label}: exit not monotonic {lefts:?}"
        );
        assert!(distinct(&lefts) >= 4, "{label}: {lefts:?}");
        assert_eq!(series.last().copied().flatten(), None, "{label}: row stayed");
        assert!(host.app.tree().exiting_nodes().is_empty());
    }
}

#[test]
fn only_the_moving_edge_animates_and_the_stationary_edge_never_jiggles() {
    for preference in adapters() {
        let mut host = open(preference);
        let label = format!("{preference:?}");
        let from = host.frames();
        host.signals.wide.set(true);
        host.poke();
        assert!(host.settle(Duration::from_secs(20)));
        let rights: Vec<usize> = host
            .series(from, |p| p.right(&p.wide))
            .into_iter()
            .flatten()
            .collect();
        let lefts: Vec<usize> = host
            .series(from, |p| p.left(&p.wide))
            .into_iter()
            .flatten()
            .collect();
        eprintln!("{label} right edge: {rights:?}");
        assert_glide(&format!("{label} edge"), &rights, 99, 299);
        assert!(lefts.iter().all(|left| *left == 0), "{label}: left edge moved {lefts:?}");
    }
}

#[test]
fn a_popup_slides_in_and_out_through_the_host() {
    for preference in adapters() {
        let mut host = open(preference);
        let label = format!("{preference:?}");
        let owner = host.app.tree().root().unwrap();
        let from = host.frames();
        let popup = host
            .app
            .open_popup(
                owner,
                UiBox::new()
                    .style(Style::new().width(px(80.0)).height(px(40.0)))
                    .fill(red())
                    .build(),
                PopupOptions::menu()
                    .at(Point::new(POPUP_X as f32 - 10.0, 300.0))
                    .presence(Presence::new().enter(MOTION).exit(MOTION).slide(0.0, 40.0)),
            )
            .unwrap();
        host.poke();
        assert!(host.settle(Duration::from_secs(20)));
        let tops: Vec<usize> = host
            .series(from, |p| p.popup.iter().skip(250).position(|pixel| pixel[3] > 8))
            .into_iter()
            .flatten()
            .collect();
        eprintln!("{label} popup in: {tops:?}");
        assert_glide(&format!("{label} popup enter"), &tops, 90, 50);

        let from = host.frames();
        assert!(host.app.close_popup(popup));
        host.poke();
        assert!(host.settle(Duration::from_secs(20)));
        let series = host.series(from, |p| p.popup.iter().skip(250).position(|pixel| pixel[3] > 8));
        let tops: Vec<usize> = series.iter().copied().flatten().collect();
        eprintln!("{label} popup out: {tops:?}");
        assert!(tops.windows(2).all(|pair| pair[1] >= pair[0]), "{label}: {tops:?}");
        assert!(distinct(&tops) >= 4, "{label}: {tops:?}");
        assert_eq!(series.last().copied().flatten(), None, "{label}");
    }
}

#[test]
fn a_snapshot_transition_crossfades_from_the_old_frame_to_the_new_one_through_the_host() {
    for preference in adapters() {
        let mut host = open(preference);
        let label = format!("{preference:?}");
        let before = host.sink.lock().unwrap().last().unwrap().far;
        assert!(before[0] > 200 && before[2] < 40, "{label}: {before:?}");
        let from = host.frames();
        host.aqua
            .send_external(
                host.id,
                ExternalUpdate::Transition(Settings {
                    duration: Duration::from_millis(400),
                    band: 0.4,
                    tint: [1.0, 1.0, 1.0, 0.0],
                    sweep: Sweep::Soft,
                }),
            )
            .unwrap();
        host.signals.ground.set(true);
        host.poke();
        assert!(host.settle(Duration::from_secs(20)));
        let pairs = host.series(from, |p| (p.near, p.far));
        let is_red = |c: &[u8; 4]| c[0] > 200 && c[2] < 40;
        let is_blue = |c: &[u8; 4]| c[2] > 200 && c[0] < 40;
        let first = pairs.first().unwrap();
        assert!(is_red(&first.0) && is_red(&first.1), "{label}: first frame {first:?}");
        let between = pairs
            .iter()
            .filter(|(near, far)| is_blue(near) && is_red(far))
            .count();
        eprintln!("{label} sweep: {between} frames with the near edge new and the far edge old of {}", pairs.len());
        assert!(between >= 2, "{label}: no diagonal sweep was presented ({between})");
        let last = pairs.last().unwrap();
        assert!(is_blue(&last.0) && is_blue(&last.1), "{label}: {last:?}");
        assert!(host.aqua.next_wake(host.aqua.clock()).is_none());
    }
}
