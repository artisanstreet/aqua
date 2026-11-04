use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aqua_core::{DisplayItem, Oklch};
use aqua_motion::Easing;
use aqua_protocol::{
    Feedback, FrameId, Instant, Machine, PixelSize, RenderEvent, RuntimeInput, ShellEvent, WindowId,
};
use aqua_runtime::{RenderSide, Runtime};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_transport::{spawn_render_thread, RenderHost};
use aqua_ui::element::ui::Box as UiBox;
use aqua_ui::motion::{Animate, Motion};
use aqua_ui::style::{px, Axis, Style};

const QUICK: Motion = Motion::tween(0.1, Easing::Linear);

struct TickHost {
    ticks: Arc<AtomicUsize>,
}

impl RenderHost for TickHost {
    fn present_time(&mut self) -> Instant {
        Instant::from_nanos(self.ticks.load(Ordering::SeqCst) as u64 * 16_666_667)
    }

    fn wait_frame(&mut self) {
        self.ticks.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(1));
    }

    fn execute(&mut self, side: &mut RenderSide, frame: FrameId) -> Vec<Feedback> {
        side.scene_mut().drain_changes();
        vec![Feedback::Presented(frame)]
    }
}

fn color(hex: &str) -> Oklch {
    Oklch::from_hex(hex).unwrap()
}

fn window() -> WindowId {
    WindowId::new(1).unwrap()
}

fn frame(runtime: &mut Runtime, secs: f64) -> aqua_protocol::RuntimeOutput {
    runtime.step(RuntimeInput::Shell(ShellEvent::Frame {
        window: window(),
        presentation: Instant::from_secs_f64(secs),
    }))
}

fn fills(side: &RenderSide) -> Vec<Oklch> {
    side.scene()
        .ordered()
        .filter_map(|(_, item)| match item {
            DisplayItem::Rect { fill, .. } if fill.alpha > 0.0 => Some(*fill),
            _ => None,
        })
        .collect()
}

#[test]
fn runtime_motion_commands_cross_the_ring_and_settle_back_into_the_runtime() {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    let (on, set_on) = runtime.scope().signal(false);
    runtime
        .mount_reactive((on,), |(on,)| {
            UiBox::new()
                .style(
                    Style::new()
                        .axis(Axis::Column)
                        .width(px(120.0))
                        .height(px(80.0)),
                )
                .child(
                    UiBox::new()
                        .style(Style::new().width(px(60.0)).height(px(30.0)))
                        .fill(if on {
                            color("#0000ff")
                        } else {
                            color("#ff0000")
                        })
                        .animate(Animate::new().fill(QUICK))
                        .build(),
                )
                .build()
        })
        .unwrap();
    runtime.step(RuntimeInput::Shell(ShellEvent::Resized {
        window: window(),
        size: PixelSize {
            width: 120,
            height: 80,
        },
        scale: 1.0,
    }));

    let ticks = Arc::new(AtomicUsize::new(0));
    let mut thread = spawn_render_thread(
        256,
        TickHost {
            ticks: Arc::clone(&ticks),
        },
        Arc::new(|| {}),
    );

    let first = frame(&mut runtime, 0.0);
    assert!(thread.send_all(first.render));
    set_on.set(true);
    let second = frame(&mut runtime, 0.001);
    assert!(thread.send_all(second.render));

    let mut settled = 0;
    for _ in 0..400 {
        match thread.wait_event(Duration::from_millis(5)) {
            Some(event @ RenderEvent::Motion(_)) => {
                settled += 1;
                runtime.step(RuntimeInput::Render(event));
            }
            Some(_) => {}
            None if settled > 0 => break,
            None => {}
        }
    }
    assert_eq!(settled, 1);
    let side = thread.shutdown().unwrap();
    assert_eq!(fills(&side), vec![color("#0000ff")]);
    assert!(side.tracks().is_idle());
}
