use aqua_gpu_d3d12::hal::selection::AdapterPreference;
use aqua_graph::Extent;
use aqua_protocol::{DeviceStatus, FrameInputs, GpuBackend};

use crate::backend::{backend, frame, matches_reference, open, scene};
use crate::test_window::TestWindow;

fn two_windows_share_one_device(preference: AdapterPreference, label: &str) {
    let mut backend = backend(preference);
    let left = TestWindow::open(480, 320);
    let right = TestWindow::open(360, 240);
    let (left_id, left_extent) = open(&mut backend, &left);
    let (right_id, right_extent) = open(&mut backend, &right);
    assert_ne!(left_id, right_id);
    assert_ne!(left_extent, right_extent);
    let generation = backend.generation();
    for index in 0..30 {
        let shade = index as f32 / 30.0;
        frame(&mut backend, &left, left_id, &scene(left_extent, shade));
        frame(
            &mut backend,
            &right,
            right_id,
            &scene(right_extent, 1.0 - shade),
        );
    }
    matches_reference(
        &mut backend,
        left_id,
        &scene(left_extent, 0.3),
        &format!("{label}-left"),
    );
    matches_reference(
        &mut backend,
        right_id,
        &scene(right_extent, 0.7),
        &format!("{label}-right"),
    );
    assert_eq!(backend.generation(), generation);
    assert_eq!(backend.stats().presents, 62);
    assert!(backend.buffer_age(left_id).is_some_and(|age| age > 0));
    assert!(backend.buffer_age(right_id).is_some_and(|age| age > 0));
}

#[test]
fn hardware_windows_share_one_device_queue_and_renderer() {
    two_windows_share_one_device(AdapterPreference::HighPerformance, "hardware-shared");
}

#[test]
fn warp_windows_share_one_device_queue_and_renderer() {
    two_windows_share_one_device(AdapterPreference::Warp, "warp-shared");
}

#[test]
fn closing_or_resizing_one_window_leaves_the_other_untouched() {
    let mut backend = backend(AdapterPreference::Warp);
    let first = TestWindow::open(400, 300);
    let second = TestWindow::open(400, 300);
    let (first_id, first_extent) = open(&mut backend, &first);
    let (second_id, second_extent) = open(&mut backend, &second);
    for _ in 0..4 {
        frame(&mut backend, &first, first_id, &scene(first_extent, 0.2));
        frame(&mut backend, &second, second_id, &scene(second_extent, 0.4));
    }
    let age = backend.buffer_age(second_id);
    first.resize(560, 360);
    let (width, height) = first.client();
    let resized = Extent { width, height };
    backend.resize_surface(first_id, resized).unwrap();
    assert_eq!(backend.buffer_age(first_id), Some(0));
    assert_eq!(backend.buffer_age(second_id), age);
    frame(&mut backend, &first, first_id, &scene(resized, 0.5));
    backend.destroy_surface(first_id).unwrap();
    assert!(
        backend
            .execute(&scene(first_extent, 0.0), &FrameInputs::new(first_id))
            .is_err()
    );
    for index in 0..6 {
        frame(
            &mut backend,
            &second,
            second_id,
            &scene(second_extent, index as f32 / 6.0),
        );
    }
    matches_reference(
        &mut backend,
        second_id,
        &scene(second_extent, 0.9),
        "survivor",
    );
    assert_eq!(backend.device_status(), DeviceStatus::Healthy);
}

#[test]
fn device_loss_rebuilds_every_window_on_the_new_device() {
    let mut backend = backend(AdapterPreference::HighPerformance);
    let first = TestWindow::open(400, 300);
    let second = TestWindow::open(320, 220);
    let (first_id, first_extent) = open(&mut backend, &first);
    let (second_id, second_extent) = open(&mut backend, &second);
    for _ in 0..4 {
        frame(&mut backend, &first, first_id, &scene(first_extent, 0.2));
        frame(&mut backend, &second, second_id, &scene(second_extent, 0.4));
    }
    backend.inject_device_removal().unwrap();
    assert!(
        backend
            .execute(&scene(first_extent, 0.1), &FrameInputs::new(first_id))
            .is_err()
    );
    let report = backend.recover().unwrap();
    assert_eq!(report.rebuilt_surfaces, 2);
    frame(&mut backend, &first, first_id, &scene(first_extent, 0.6));
    frame(&mut backend, &second, second_id, &scene(second_extent, 0.7));
    matches_reference(
        &mut backend,
        first_id,
        &scene(first_extent, 0.8),
        "after-loss-first",
    );
    matches_reference(
        &mut backend,
        second_id,
        &scene(second_extent, 0.9),
        "after-loss-second",
    );
}
