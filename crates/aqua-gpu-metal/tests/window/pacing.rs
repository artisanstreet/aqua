use std::time::{Duration, Instant};

use aqua_gpu_metal::window::Alpha;
use aqua_protocol::PresentHint;

use super::support::{
    self, drive, extent_of, frame, shown, solid, srgb, Flow, RenderThread, LIMIT,
};

const VM_TIMER_SLACK: Duration = Duration::from_millis(40);

pub fn display_interval() {
    if !support::appkit() {
        return;
    }
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    let window = support::window("metal vsync", (200.0, 120.0), (40, 80));
    let plan = solid(extent_of(window), srgb(90, 90, 120));
    let surface = gpu.attach(window, Alpha::Opaque);
    drive(LIMIT, move |_, _| {
        gpu.frame(surface, plan.clone());
        if shown(&gpu, surface) < 3 {
            return Flow::Wait(Duration::from_millis(10));
        }
        let interval = window
            .refresh_interval()
            .expect("the screen reports its refresh interval");
        let plan = plan.clone();
        let report = gpu.run(move |backend| {
            (0..120).for_each(|_| {
                frame(backend, surface, &plan, PresentHint::Vsync);
            });
            backend.pacing(surface).expect("a live surface")
        });
        println!("refresh {interval:?}, {report:?}");
        assert!(
            report.interval.abs_diff(interval) <= Duration::from_micros(1),
            "the pacer uses the screen's interval {interval:?}: {report:?}"
        );
        if report.frames == 0 && report.discarded > 0 {
            assert!(
                std::env::var_os("AQUA_REQUIRE_PRESENT_TIMES").is_none_or(|value| value != "1"),
                "AQUA_REQUIRE_PRESENT_TIMES=1 but every drawable came back without a presented time: {report:?}"
            );
            println!(
                "SKIP presentation_feedback_reports_the_display_interval: the display reports no presented times, so every present counts as discarded ({} of them)",
                report.discarded
            );
            gpu.assert_clean();
            return Flow::Done;
        }
        assert!(report.frames >= 100, "{report:?}");
        assert!(
            report.mean.abs_diff(interval) < interval / 10,
            "presented times follow the display interval {interval:?}: {report:?}"
        );
        gpu.assert_clean();
        Flow::Done
    });
}

pub fn hidden_window_bounded() {
    if !support::appkit() {
        return;
    }
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    let seen = support::window("metal seen", (200.0, 120.0), (20, 80));
    let hidden = support::window("metal hidden", (200.0, 120.0), (400, 80));
    let seen_plan = solid(extent_of(seen), srgb(40, 140, 90));
    let hidden_plan = solid(extent_of(hidden), srgb(140, 40, 90));
    let seen_surface = gpu.attach(seen, Alpha::Opaque);
    let hidden_surface = gpu.attach(hidden, Alpha::Opaque);
    let mut ordered_out = false;
    drive(LIMIT, move |_, _| {
        gpu.frame(seen_surface, seen_plan.clone());
        if !ordered_out {
            gpu.frame(hidden_surface, hidden_plan.clone());
            if shown(&gpu, seen_surface) < 2 || shown(&gpu, hidden_surface) < 2 {
                return Flow::Wait(Duration::from_millis(10));
            }
            hidden.hide();
            ordered_out = true;
            return Flow::Wait(Duration::from_millis(50));
        }
        let interval = seen.refresh_interval().expect("a screen");
        let (seen_plan, hidden_plan) = (seen_plan.clone(), hidden_plan.clone());
        let (hidden_costs, settled, slot) = gpu.run(move |backend| {
            let mut hidden_costs = Vec::new();
            let mut settled = None;
            for iteration in 0..120 {
                if iteration == 30 {
                    settled = Some(Instant::now());
                }
                frame(backend, seen_surface, &seen_plan, PresentHint::Vsync);
                let cost = frame(backend, hidden_surface, &hidden_plan, PresentHint::Vsync);
                hidden_costs.push((cost, backend.phases().slot_wait));
            }
            let settled = settled.expect("thirty iterations ran").elapsed();
            (hidden_costs, settled, backend.frame_slot(hidden_surface))
        });
        println!(
            "hidden slot {slot:?}, first costs {:?}",
            &hidden_costs[..12]
        );
        assert!(
            hidden_costs
                .iter()
                .all(|(cost, _)| *cost <= interval * 2 + VM_TIMER_SLACK),
            "every hidden-window frame is bounded by two refresh intervals: {hidden_costs:?}"
        );
        assert!(
            hidden_costs[10..]
                .iter()
                .all(|(_, slot_wait)| *slot_wait < interval / 4),
            "after the first bounded wait the hidden window never waits again: {:?}",
            &hidden_costs[10..]
        );
        let per_frame = settled / 90;
        assert!(
            per_frame.abs_diff(interval) < interval / 10,
            "the visible window keeps the display pace beside the hidden one: {per_frame:?} per frame"
        );
        gpu.assert_clean();
        Flow::Done
    });
}
