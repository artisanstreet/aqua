use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aqua_core::id::RawId;
use aqua_core::{DisplayItem, NodeId, Oklch, Rect};
use aqua_motion::{AppMotion, Binding, Easing, MotionEvent, MotionKind, MotionValue, Property};
use aqua_protocol::{Feedback, FrameId, Instant, RenderCommand, RenderEvent, SceneDiff};
use aqua_runtime::RenderSide;
use aqua_transport::{spawn_render_thread, RenderHost};

struct FakeHost {
    ticks: Arc<AtomicUsize>,
    frames: Arc<Mutex<Vec<FrameId>>>,
}

impl RenderHost for FakeHost {
    fn present_time(&mut self) -> Instant {
        Instant::from_nanos(self.ticks.load(Ordering::SeqCst) as u64 * 16_666_667)
    }

    fn wait_frame(&mut self) {
        self.ticks.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(1));
    }

    fn execute(&mut self, side: &mut RenderSide, frame: FrameId) -> Vec<Feedback> {
        side.scene_mut().drain_changes();
        self.frames.lock().unwrap().push(frame);
        vec![Feedback::Presented(frame)]
    }
}

fn item(fill: Oklch) -> DisplayItem {
    DisplayItem::Rect {
        bounds: Rect {
            x: 0.0,
            y: 0.0,
            w: 4.0,
            h: 4.0,
        },
        radius: 0.0,
        fill,
        border_width: 0.0,
        border_color: fill,
        shadow: None,
    }
}

#[test]
fn render_thread_animates_alone_and_reports_back_only_the_settle() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let frames = Arc::new(Mutex::new(Vec::new()));
    let woken = Arc::new(AtomicUsize::new(0));
    let wake_counter = Arc::clone(&woken);
    let mut thread = spawn_render_thread(
        256,
        FakeHost {
            ticks: Arc::clone(&ticks),
            frames: Arc::clone(&frames),
        },
        Arc::new(move || {
            wake_counter.fetch_add(1, Ordering::SeqCst);
        }),
    );

    let node = NodeId::from_raw(RawId {
        index: 0,
        generation: 0,
    });
    let red = Oklch::from_hex("#ff0000").unwrap();
    let blue = Oklch::from_hex("#0000ff").unwrap();
    let mut app = AppMotion::default();
    let track = app.spawn(
        None,
        Some(Binding {
            node,
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
    assert!(thread.send(RenderCommand::Scene(SceneDiff {
        fills: Vec::new(),
        upserts: vec![(node, item(red))],
        order: Some(vec![node]),
    })));
    assert!(thread.send_all(app.drain_commands().into_iter().map(RenderCommand::Motion)));

    let mut presented = 0;
    let mut settled = Vec::new();
    while settled.is_empty() {
        match thread.wait_event(Duration::from_secs(10)) {
            Some(RenderEvent::Motion(event)) => {
                app.handle(event);
                settled.push(event);
            }
            Some(RenderEvent::Presented(_)) => presented += 1,
            Some(_) => {}
            None => panic!("render thread never settled the track"),
        }
    }
    assert!(matches!(
        settled[0],
        MotionEvent::Settled { generation: 1, .. }
    ));
    assert!(app.is_settled(track));
    let side = thread.shutdown().expect("render thread joins");
    let shown = match side.presented(node).unwrap() {
        DisplayItem::Rect { fill, .. } => *fill,
        other => panic!("{other:?}"),
    };
    assert_eq!(shown, blue);
    assert!(frames.lock().unwrap().len() > 3);
    assert!(presented > 0);
    assert!(woken.load(Ordering::SeqCst) >= 1);
}
