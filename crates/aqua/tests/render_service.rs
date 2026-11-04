use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicU32, AtomicU64};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant as WallInstant};

use aqua::render::host::{Clock, FrameTarget, RenderStats, Repaint, TargetInit};
use aqua::render::link::{CompositionSink, ExternalUpdate, InitFactory, RenderLink};
use aqua::render::service::RenderService;
use aqua_core::{DisplayItem, Oklch, Rect, Size};
use aqua_protocol::{Feedback, FrameId, Instant, RenderEvent};
use aqua_runtime::RenderSide;
use aqua_scene::Scene;

const INTERVAL: Duration = Duration::from_millis(4);

#[derive(Default)]
struct Probe {
    presents: AtomicU32,
    invalidations: AtomicU32,
    frames: Mutex<Vec<FrameId>>,
    script: Mutex<Vec<Repaint>>,
    thread: AtomicU64,
}

struct Fake {
    probe: Arc<Probe>,
    _externals: std::sync::mpsc::Receiver<ExternalUpdate>,
}

impl FrameTarget for Fake {
    fn wait_frame(&mut self) {}

    fn present(&mut self, _side: &mut RenderSide, frame: FrameId) -> Vec<Feedback> {
        self.probe.presents.fetch_add(1, Relaxed);
        self.probe.frames.lock().unwrap().push(frame);
        self.probe.thread.store(
            {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                std::thread::current().id().hash(&mut hasher);
                hasher.finish()
            },
            Relaxed,
        );
        vec![Feedback::Presented(frame)]
    }

    fn invalidate(&mut self) {
        self.probe.invalidations.fetch_add(1, Relaxed);
    }

    fn repaint(&self) -> Repaint {
        let mut script = self.probe.script.lock().unwrap();
        match script.is_empty() {
            true => Repaint::Settled,
            false => script.remove(0),
        }
    }
}

fn clock() -> Clock {
    let origin = WallInstant::now();
    Arc::new(move || Instant::from_nanos(origin.elapsed().as_nanos() as u64))
}

fn factory(probe: Arc<Probe>) -> InitFactory {
    Box::new(move |context| {
        let externals = context.externals;
        let init: TargetInit = Box::new(move || {
            Ok(Box::new(Fake {
                probe,
                _externals: externals,
            }) as Box<dyn FrameTarget>)
        });
        init
    })
}

fn link(service: &RenderService, probe: &Arc<Probe>) -> RenderLink {
    let composition: CompositionSink = Arc::new(|_| {});
    let wake: aqua_transport::Notify = Arc::new(|| {});
    let mut link = RenderLink::spawn(
        service,
        factory(probe.clone()),
        composition,
        clock(),
        INTERVAL,
        wake,
    );
    link.set_viewport(Size { w: 100.0, h: 100.0 }, 1.0);
    link
}

fn scene_with(color: &str) -> Scene {
    let mut scene = Scene::new();
    scene.insert(DisplayItem::Rect {
        bounds: Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        },
        radius: 0.0,
        fill: Oklch::from_hex(color).unwrap(),
        border_width: 0.0,
        border_color: Oklch::TRANSPARENT,
        shadow: None,
    });
    scene
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = WallInstant::now() + Duration::from_secs(5);
    while !done() {
        assert!(
            WallInstant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn presented_events(link: &mut RenderLink) -> usize {
    link.poll()
        .iter()
        .filter(|event| matches!(event, RenderEvent::Presented(_)))
        .count()
}

fn stats_of(link: &RenderLink) -> &Arc<RenderStats> {
    link.stats()
}

#[test]
fn windows_share_one_thread_and_idle_windows_do_nothing() {
    let service = RenderService::new().unwrap();
    let (a, b) = (Arc::new(Probe::default()), Arc::new(Probe::default()));
    let mut first = link(&service, &a);
    let mut second = link(&service, &b);
    let mut scene_a = scene_with("#ff0000");
    let mut scene_b = scene_with("#00ff00");
    first.push_scene(&mut scene_a);
    second.push_scene(&mut scene_b);
    wait_until("both windows to present", || {
        a.presents.load(Relaxed) >= 1 && b.presents.load(Relaxed) >= 1
    });
    assert_eq!(a.thread.load(Relaxed), b.thread.load(Relaxed));
    std::thread::sleep(Duration::from_millis(80));
    let settled = (a.presents.load(Relaxed), b.presents.load(Relaxed));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        (a.presents.load(Relaxed), b.presents.load(Relaxed)),
        settled,
        "idle windows kept presenting"
    );
    let _ = (presented_events(&mut first), presented_events(&mut second));
}

#[test]
fn pending_content_is_repainted_until_it_settles() {
    let service = RenderService::new().unwrap();
    let probe = Arc::new(Probe::default());
    *probe.script.lock().unwrap() = vec![
        Repaint::Poll(Duration::from_millis(3)),
        Repaint::Poll(Duration::from_millis(3)),
        Repaint::Poll(Duration::from_millis(3)),
    ];
    let mut link = link(&service, &probe);
    link.push_scene(&mut scene_with("#336699"));
    wait_until("four presents", || probe.presents.load(Relaxed) >= 4);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(probe.presents.load(Relaxed), 4);
    assert_eq!(stats_of(&link).forced.load(Relaxed), 3);
    assert_eq!(
        probe.invalidations.load(Relaxed),
        3,
        "items that were pending need a full repaint"
    );
    let frames = probe.frames.lock().unwrap().clone();
    assert!(frames.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn continuous_animation_keeps_presenting_until_it_stops() {
    let service = RenderService::new().unwrap();
    let probe = Arc::new(Probe::default());
    *probe.script.lock().unwrap() = vec![Repaint::NextFrame; 12];
    let mut link = link(&service, &probe);
    link.push_scene(&mut scene_with("#336699"));
    wait_until("thirteen presents", || probe.presents.load(Relaxed) >= 13);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(probe.presents.load(Relaxed), 13);
    assert_eq!(presented_events(&mut link), 1);
}

#[test]
fn an_arriving_image_forces_one_full_repaint() {
    let service = RenderService::new().unwrap();
    let probe = Arc::new(Probe::default());
    let mut link = link(&service, &probe);
    link.push_scene(&mut scene_with("#336699"));
    wait_until("the first present", || probe.presents.load(Relaxed) >= 1);
    std::thread::sleep(Duration::from_millis(50));
    let before = probe.presents.load(Relaxed);
    link.external(ExternalUpdate::Image {
        layer: 1,
        width: 1,
        height: 1,
        rgba: Arc::from(vec![255u8; 4]),
    });
    wait_until("the repaint", || probe.presents.load(Relaxed) > before);
    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(probe.presents.load(Relaxed), before + 1);
    assert_eq!(probe.invalidations.load(Relaxed), 1);
    assert_eq!(
        presented_events(&mut link),
        1,
        "forced frames are not reported as new frames"
    );
}

#[test]
fn a_trim_request_runs_without_a_scene_change() {
    let service = RenderService::new().unwrap();
    let probe = Arc::new(Probe::default());
    let mut link = link(&service, &probe);
    link.push_scene(&mut scene_with("#336699"));
    wait_until("the first present", || probe.presents.load(Relaxed) >= 1);
    std::thread::sleep(Duration::from_millis(50));
    let before = probe.presents.load(Relaxed);
    link.request_trim();
    wait_until("the trim frame", || probe.presents.load(Relaxed) > before);
    wait_until("the trim flag to clear", || {
        !stats_of(&link).trim.load(Relaxed)
    });
    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(probe.presents.load(Relaxed), before + 1);
}

#[test]
fn closing_one_window_leaves_the_others_running() {
    let service = RenderService::new().unwrap();
    let (a, b) = (Arc::new(Probe::default()), Arc::new(Probe::default()));
    let first = link(&service, &a);
    let mut second = link(&service, &b);
    drop(first);
    second.push_scene(&mut scene_with("#336699"));
    wait_until("the survivor to present", || b.presents.load(Relaxed) >= 1);
    let mut third_scene = scene_with("#996633");
    second.push_scene(&mut third_scene);
    let after_close = b.presents.load(Relaxed);
    wait_until("another frame", || b.presents.load(Relaxed) > after_close);
    let closed_at = a.presents.load(Relaxed);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(a.presents.load(Relaxed), closed_at);
}
