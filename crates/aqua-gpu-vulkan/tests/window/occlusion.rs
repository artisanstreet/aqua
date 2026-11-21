use std::time::{Duration, Instant};

use aqua_gpu_vulkan::window::pacing::Slot;
use aqua_gpu_vulkan::window::Alpha;
use aqua_protocol::PresentHint;

use super::support::{self, drive, extent_of, frame, mapped, shown, srgb, Flow, RenderThread};

const REFRESH: Duration = Duration::from_micros(16_667);

#[test]
fn vsync_presents_follow_the_output_refresh() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    let app = "com.aqua.test.vulkan.vsync";
    let window = support::window(&compositor, app, (200.0, 120.0), (40, 30));
    let plan = support::solid(extent_of(window), srgb(90, 90, 120));
    let surface = gpu.attach(window, Alpha::Opaque);
    gpu.frame(surface, plan.clone());
    let probe = compositor.clone();
    drive(Duration::from_secs(20), move |_| {
        if mapped(&probe, app).is_none() || shown(&gpu, surface) == 0 {
            return Flow::Wait(Duration::from_millis(10));
        }
        let plan = plan.clone();
        let report = gpu.run(move |backend| {
            (0..120).for_each(|_| {
                frame(backend, surface, &plan, PresentHint::Vsync, None);
            });
            backend.pacing(surface).expect("a live surface")
        });
        assert!(report.frames >= 110, "{report:?}");
        assert!(
            report.mean.abs_diff(REFRESH) < REFRESH / 10,
            "presentation feedback follows 60 Hz: {report:?}"
        );
        gpu.assert_clean();
        Flow::Done
    });
}

#[test]
fn a_hidden_window_never_blocks_the_other_window_on_the_shared_render_thread() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    let (seen_app, hidden_app) = ("com.aqua.test.vulkan.seen", "com.aqua.test.vulkan.hidden");
    let seen = support::window(&compositor, seen_app, (200.0, 120.0), (20, 20));
    let hidden = support::window(&compositor, hidden_app, (200.0, 120.0), (400, 20));
    let seen_plan = support::solid(extent_of(seen), srgb(40, 140, 90));
    let hidden_plan = support::solid(extent_of(hidden), srgb(140, 40, 90));
    let seen_surface = gpu.attach(seen, Alpha::Opaque);
    let hidden_surface = gpu.attach(hidden, Alpha::Opaque);
    gpu.frame(seen_surface, seen_plan.clone());
    gpu.frame(hidden_surface, hidden_plan.clone());
    let probe = compositor.clone();
    let mut moved = false;
    drive(Duration::from_secs(30), move |_| {
        let mapped_both =
            mapped(&probe, seen_app).is_some() && mapped(&probe, hidden_app).is_some();
        if !mapped_both || shown(&gpu, seen_surface) == 0 || shown(&gpu, hidden_surface) == 0 {
            return Flow::Wait(Duration::from_millis(10));
        }
        if !moved {
            probe
                .command(&format!(
                    "[app_id=\"{hidden_app}\"] move container to workspace 2"
                ))
                .expect("sway hides the window on another workspace");
            moved = true;
            return Flow::Wait(Duration::from_millis(50));
        }
        let (seen_plan, hidden_plan) = (seen_plan.clone(), hidden_plan.clone());
        let (hidden_costs, settled, slot) = gpu.run(move |backend| {
            let interval = backend
                .refresh_interval(hidden_surface)
                .expect("a live surface");
            let mut hidden_costs = Vec::new();
            let mut settled = None;
            for iteration in 0..120 {
                if iteration == 30 {
                    settled = Some(Instant::now());
                }
                frame(backend, seen_surface, &seen_plan, PresentHint::Vsync, None);
                let cost = frame(
                    backend,
                    hidden_surface,
                    &hidden_plan,
                    PresentHint::Vsync,
                    None,
                );
                hidden_costs.push((cost, interval, backend.phases().slot_wait));
            }
            let settled = settled.expect("thirty iterations ran").elapsed();
            (hidden_costs, settled, backend.frame_slot(hidden_surface))
        });
        assert_eq!(slot, Some(Slot::Starved), "the hidden window starved");
        assert!(
            hidden_costs
                .iter()
                .all(|(cost, interval, _)| *cost <= *interval * 2 + Duration::from_millis(10)),
            "every hidden-window frame is bounded by two refresh intervals: {hidden_costs:?}"
        );
        assert!(
            hidden_costs[10..]
                .iter()
                .all(|(_, interval, slot_wait)| *slot_wait < *interval / 4),
            "once starved, the hidden window's frame slot never waits for the compositor: {:?}",
            &hidden_costs[10..]
        );
        let per_frame = settled / 90;
        assert!(
            per_frame.abs_diff(REFRESH) < REFRESH / 10,
            "the visible window keeps the 60 Hz pace beside the hidden one: {per_frame:?} per frame"
        );
        gpu.assert_clean();
        Flow::Done
    });
}
