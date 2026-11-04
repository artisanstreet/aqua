#![cfg(windows)]

#[path = "../../aqua-gpu-d3d12/tests/hal/window.rs"]
mod test_window;

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant as StdInstant};

use aqua_core::id::RawId;
use aqua_core::{DisplayItem, NodeId, Oklch, Rect, Size};
use aqua_gpu_d3d12::hal::selection::AdapterPreference;
use aqua_gpu_d3d12::hal::DeviceOptions;
use aqua_gpu_d3d12::window::{WindowBackend, WindowOptions};
use aqua_motion::{AppMotion, Binding, Easing, MotionKind, MotionValue, Property};
use aqua_protocol::{Feedback, FrameId, Instant, RenderCommand, RenderEvent, SceneDiff, WindowId};
use aqua_runtime::RenderSide;
use aqua_transport::{
    spawn_render_thread_with, GeometryLowering, GpuRenderHost, HostConfig, Inspector,
    InspectorServer, MonotonicClock, RenderHost,
};
use test_window::TestWindow;

type Inner = GpuRenderHost<WindowBackend, GeometryLowering, MonotonicClock>;

struct Tap {
    inner: Inner,
    executes: Arc<Mutex<Vec<StdInstant>>>,
    waits: Arc<AtomicU64>,
}

impl RenderHost for Tap {
    fn present_time(&mut self) -> Instant {
        self.inner.present_time()
    }

    fn wait_frame(&mut self) {
        self.waits.fetch_add(1, Ordering::SeqCst);
        self.inner.wait_frame();
    }

    fn execute(&mut self, side: &mut RenderSide, frame: FrameId) -> Vec<Feedback> {
        self.executes.lock().unwrap().push(StdInstant::now());
        self.inner.execute(side, frame)
    }
}

fn color(hex: &str) -> Oklch {
    Oklch::from_hex(hex).unwrap()
}

fn scene_node() -> NodeId {
    NodeId::from_raw(RawId {
        index: 0,
        generation: 0,
    })
}

fn rect(fill: Oklch) -> DisplayItem {
    DisplayItem::Rect {
        bounds: Rect {
            x: 10.0,
            y: 10.0,
            w: 200.0,
            h: 100.0,
        },
        radius: 8.0,
        fill,
        border_width: 0.0,
        border_color: fill,
        shadow: None,
    }
}

fn adapter() -> AdapterPreference {
    AdapterPreference::HighPerformance
}

fn query(addr: std::net::SocketAddr, command: &str) -> String {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    writeln!(stream, "{command}").unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    line
}

fn field(json: &str, key: &str) -> u64 {
    let marker = format!("\"{key}\":");
    let start = json.find(&marker).expect(key) + marker.len();
    json[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap()
}

#[test]
fn render_thread_paces_on_the_waitable_animates_and_goes_idle() {
    let window = TestWindow::open(340, 240);
    let (width, height) = window.client();
    let origin = StdInstant::now();
    let inspector = Inspector::new();
    let server = InspectorServer::bind(inspector.clone(), "127.0.0.1:0".parse().unwrap()).unwrap();
    let executes = Arc::new(Mutex::new(Vec::new()));
    let waits = Arc::new(AtomicU64::new(0));
    let hwnd = window.raw() as u64;

    let build_inspector = inspector.clone();
    let build_executes = Arc::clone(&executes);
    let build_waits = Arc::clone(&waits);
    let mut thread = spawn_render_thread_with(
        1024,
        move || {
            let backend = WindowBackend::new(WindowOptions {
                device: DeviceOptions::default().with_preference(adapter()),
            })
            .expect("window backend");
            Tap {
                inner: GpuRenderHost::new(
                    backend,
                    GeometryLowering,
                    MonotonicClock::from_origin(origin),
                    HostConfig::native(WindowId::new(1).unwrap(), hwnd, color("#101820")),
                )
                .with_inspector(build_inspector),
                executes: build_executes,
                waits: build_waits,
            }
        },
        Arc::new(|| {}),
    );

    let red = color("#ff2020");
    let blue = color("#2030ff");
    let now = || origin.elapsed().as_secs_f64();
    let mut app = AppMotion::default();
    let track = app.spawn(
        None,
        Some(Binding {
            node: scene_node(),
            property: Property::Fill,
        }),
        MotionValue::Color(red),
        MotionKind::Tween {
            duration_secs: 0.5,
            easing: Easing::EaseInOut,
        },
        now(),
    );
    let mut commands = vec![
        RenderCommand::Viewport {
            size: Size::new(width as f32, height as f32),
            scale: 1.0,
        },
        RenderCommand::Scene(SceneDiff {
            upserts: vec![(scene_node(), rect(red))],
            order: Some(vec![scene_node()]),
        }),
    ];
    commands.extend(app.drain_commands().into_iter().map(RenderCommand::Motion));
    assert!(thread.send_all(commands));

    std::thread::sleep(Duration::from_millis(300));
    let started = StdInstant::now();
    app.set_target(track, MotionValue::Color(blue), now())
        .unwrap();
    assert!(thread.send_all(app.drain_commands().into_iter().map(RenderCommand::Motion)));

    let mut settled = false;
    while started.elapsed() < Duration::from_secs(5) && !settled {
        window.pump();
        if let Some(event) = thread.wait_event(Duration::from_millis(20)) {
            if let RenderEvent::Motion(event) = event {
                app.handle(event);
                settled = app.is_settled(track);
            }
        }
        for event in thread.poll() {
            if let RenderEvent::Motion(event) = event {
                app.handle(event);
                settled |= app.is_settled(track);
            }
        }
    }
    assert!(settled, "the track must settle through the render thread");
    let animation = started.elapsed();
    assert!(animation >= Duration::from_millis(450), "{animation:?}");

    let stats = query(server.local_addr(), "stats");
    let presented_at_settle = field(&stats, "frames_presented");
    eprintln!("stats at settle: {stats}");
    assert!(presented_at_settle >= 10, "{stats}");
    assert!(stats.contains("\"unsupported_items\":0"), "{stats}");

    let stamps: Vec<StdInstant> = executes.lock().unwrap().clone();
    let animating: Vec<f64> = stamps
        .windows(2)
        .filter(|pair| pair[0] >= started)
        .map(|pair| pair[1].duration_since(pair[0]).as_secs_f64() * 1000.0)
        .collect();
    let mean = animating.iter().sum::<f64>() / animating.len().max(1) as f64;
    let (min, max) = animating
        .iter()
        .fold((f64::MAX, 0.0_f64), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    eprintln!(
        "animation frames: {} mean {mean:.2} ms min {min:.2} max {max:.2}; waitable waits {}",
        animating.len(),
        waits.load(Ordering::SeqCst)
    );
    assert!(
        mean > 3.0,
        "frames must be paced by the waitable, not spin: mean {mean:.2} ms"
    );
    assert!(
        mean < 40.0,
        "frames must keep coming while animating: mean {mean:.2} ms"
    );

    std::thread::sleep(Duration::from_millis(100));
    let idle_start = field(&query(server.local_addr(), "stats"), "frames_presented");
    std::thread::sleep(Duration::from_millis(500));
    let idle_end = field(&query(server.local_addr(), "stats"), "frames_presented");
    assert!(
        idle_end - idle_start <= 1,
        "no frames at rest: {idle_start} -> {idle_end}"
    );

    window.resize(500, 360);
    let (w2, h2) = window.client();
    assert!(thread.send(RenderCommand::Viewport {
        size: Size::new(w2 as f32, h2 as f32),
        scale: 1.0,
    }));
    std::thread::sleep(Duration::from_millis(400));
    window.pump();
    let resized = query(server.local_addr(), "stats");
    eprintln!("after resize: {resized}");
    assert_eq!(field(&resized, "width"), u64::from(w2), "{resized}");
    assert_eq!(field(&resized, "height"), u64::from(h2), "{resized}");
    assert!(field(&resized, "frames_failed") == 0, "{resized}");

    assert!(thread.shutdown().is_some());
}
