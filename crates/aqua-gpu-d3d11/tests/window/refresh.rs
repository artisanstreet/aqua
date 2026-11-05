use std::time::{Duration, Instant};

use aqua_gpu_d3d11::hal::selection::AdapterPreference;
use aqua_gpu_d3d11::window::timing;
use aqua_protocol::{FrameInputs, GpuBackend, PresentHint};

use crate::backend::{backend, open, scene};
use crate::test_window::TestWindow;

const FRAMES: usize = 180;

fn paced(preference: AdapterPreference, label: &str) {
    let mut backend = backend(preference);
    let _priority = crate::support::prioritized();
    let window = TestWindow::open(480, 320);
    let (id, extent) = open(&mut backend, &window);
    let interval = backend.refresh_interval(id).expect("interval");
    let reported = timing::query().expect("DWM composition timing");
    eprintln!(
        "[{label}] DWM composition interval {:.3} ms, window monitor interval {:.3} ms",
        reported.interval.as_secs_f64() * 1000.0,
        interval.as_secs_f64() * 1000.0
    );
    assert!(
        interval >= Duration::from_micros(2_000) && interval <= Duration::from_millis(40),
        "{interval:?}"
    );
    assert!(reported.until_next <= reported.interval + Duration::from_micros(500));

    for warm in 0..10 {
        backend.wait_for_frame_slot(id).unwrap();
        backend
            .execute(&scene(extent, warm as f32 / 10.0), &FrameInputs::new(id))
            .unwrap();
        backend.present(id, PresentHint::Vsync).unwrap();
        window.pump();
    }
    let started = Instant::now();
    let mut gaps = Vec::with_capacity(FRAMES);
    let mut last = started;
    for index in 0..FRAMES {
        backend.wait_for_frame_slot(id).unwrap();
        backend
            .execute(
                &scene(extent, index as f32 / FRAMES as f32),
                &FrameInputs::new(id),
            )
            .unwrap();
        backend.present(id, PresentHint::Vsync).unwrap();
        window.pump();
        let now = Instant::now();
        gaps.push(now - last);
        last = now;
    }
    let total = started.elapsed();
    let mean = total / FRAMES as u32;
    gaps.sort();
    let median = gaps[gaps.len() / 2];
    let report = backend.pacing(id).unwrap();
    eprintln!(
        "[{label}] refresh {:.3} ms, mean {:.3} ms, median {:.3} ms, worst {:.3} ms, missed {} of {}",
        interval.as_secs_f64() * 1000.0,
        mean.as_secs_f64() * 1000.0,
        median.as_secs_f64() * 1000.0,
        gaps[gaps.len() - 1].as_secs_f64() * 1000.0,
        report.missed,
        report.frames,
    );
    let interval_s = reported.interval.as_secs_f64();
    let ratio = mean.as_secs_f64() / interval.as_secs_f64();
    assert!(
        (0.9..=1.1).contains(&ratio),
        "[{label}] presents average {ratio:.3} of the window monitor's refresh interval"
    );
    let aligned = gaps
        .iter()
        .filter(|gap| {
            let cycles = gap.as_secs_f64() / interval_s;
            cycles >= 0.75 && (cycles - cycles.round()).abs() <= 0.25
        })
        .count();
    let floor = gaps[gaps.len() / 20].as_secs_f64() / interval_s;
    assert!(
        floor >= 0.6,
        "[{label}] presents run ahead of the display: 5th percentile gap is {floor:.2} refresh intervals"
    );
    assert!(
        aligned * 10 >= gaps.len() * 9,
        "[{label}] only {aligned} of {} presents landed on a composition tick",
        gaps.len()
    );
    assert!(report.frames >= FRAMES as u64);
}

#[test]
fn hardware_vsync_presents_follow_the_display_refresh_interval() {
    paced(AdapterPreference::HighPerformance, "hardware-pacing");
}

#[test]
fn warp_vsync_presents_follow_the_display_refresh_interval() {
    paced(AdapterPreference::Warp, "warp-pacing");
}
