use std::time::Duration;

use aqua_core::geometry::Rect;
use aqua_gpu_vulkan::window::Alpha;
use aqua_shell_wayland::testing::DamageWatch;

use super::support::{
    self, bgr, close, drive, extent_of, mapped, painted, quad, shown, srgb, Flow, RenderThread,
    ScreenRect,
};

fn inside(damage: &ScreenRect, planned: ScreenRect) -> bool {
    damage.x >= planned.x - 1
        && damage.y >= planned.y - 1
        && damage.x + damage.width <= planned.x + planned.width + 1
        && damage.y + damage.height <= planned.y + planned.height + 1
}

fn on_screen(window: ScreenRect, rect: Rect) -> ScreenRect {
    ScreenRect {
        x: window.x + rect.x as i32,
        y: window.y + rect.y as i32,
        width: rect.w as i32,
        height: rect.h as i32,
    }
}

#[test]
fn partial_present_damages_only_the_changed_rectangle() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    if !gpu.run(|backend| backend.incremental_present()) {
        eprintln!("SKIP: the device lacks VK_KHR_incremental_present");
        return;
    }
    let app = "com.aqua.test.vulkan.partial";
    let window = support::window(&compositor, app, (240.0, 160.0), (40, 30));
    let extent = extent_of(window);
    let surface = gpu.attach(window, Alpha::Opaque);
    let paper = srgb(235, 235, 230);
    let first = Rect {
        x: 40.0,
        y: 30.0,
        w: 50.0,
        h: 20.0,
    };
    let second = Rect {
        x: 150.0,
        y: 100.0,
        w: 30.0,
        h: 40.0,
    };
    let (red, blue) = (srgb(210, 30, 30), srgb(30, 60, 210));
    gpu.frame(surface, support::solid(extent, paper));
    (0..8).for_each(|_| {
        gpu.damaged(surface, support::solid(extent, paper), vec![first]);
    });
    let warmed = gpu.run(|backend| backend.stats());
    let probe = compositor.clone();
    let mut step = 0;
    let mut watch: Option<DamageWatch> = None;
    drive(Duration::from_secs(15), move |_| {
        let Some(placed) = mapped(&probe, app).filter(|_| shown(&gpu, surface) >= 1) else {
            return Flow::Wait(Duration::from_millis(10));
        };
        match step {
            0 => {
                watch = Some(DamageWatch::start(&probe).expect("screencopy").0);
                gpu.damaged(
                    surface,
                    painted(extent, paper, &[quad(first, red, 0.0)]),
                    vec![first],
                );
                step = 1;
            }
            1 | 2 => {
                let (capture, damage) = watch
                    .as_mut()
                    .expect("watching")
                    .next(Duration::from_secs(2))
                    .expect("the partial frame damages the output");
                let planned = on_screen(placed, if step == 1 { first } else { second });
                assert!(
                    !damage.is_empty() && damage.iter().all(|rect| inside(rect, planned)),
                    "frame {step}: compositor damage {damage:?} stays within {planned:?}"
                );
                let at = |rect: Rect| {
                    bgr(
                        &capture,
                        placed.x + (rect.x + rect.w / 2.0) as i32,
                        placed.y + (rect.y + rect.h / 2.0) as i32,
                    )
                };
                let paper_seen = bgr(&capture, placed.x + 5, placed.y + 5);
                assert!(close(at(first), [210, 30, 30], 1), "{:?}", at(first));
                match step {
                    1 => {
                        assert!(close(at(second), paper_seen, 0));
                        gpu.damaged(
                            surface,
                            painted(
                                extent,
                                paper,
                                &[quad(first, red, 0.0), quad(second, blue, 0.0)],
                            ),
                            vec![second],
                        );
                        step = 2;
                    }
                    _ => {
                        assert!(close(at(second), [30, 60, 210], 1), "{:?}", at(second));
                        let stats = gpu.run(|backend| backend.stats());
                        assert_eq!(
                            stats.incremental_presents - warmed.incremental_presents,
                            2,
                            "{stats:?}"
                        );
                        assert_eq!(
                            stats.scissored_frames - warmed.scissored_frames,
                            2,
                            "both damaged frames were recorded in a scissor: {stats:?}"
                        );
                        assert_eq!(
                            stats.last_repaint_area,
                            140 * 110,
                            "the repaint is the bounds of what changed since the image was last shown: {stats:?}"
                        );
                        gpu.assert_clean();
                        return Flow::Done;
                    }
                }
            }
            _ => unreachable!(),
        }
        Flow::Wait(Duration::from_millis(5))
    });
}
