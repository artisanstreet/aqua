#[test]
fn snapshot_requests_are_applied_before_flushing_the_changed_scene() {
    let (mut aqua, mut app, id) = headless();
    let pixels = Arc::new(Mutex::new(None));
    assert!(aqua.attach_render(
        id,
        reference_factory(
            Extent { width: 32, height: 32 },
            aqua_text::SharedFonts::new(),
            aqua_text::SharedRuns::new(),
            pixels,
        ),
        Duration::from_millis(16),
    ));
    let other = AquaWindowId::new(99).unwrap();
    let transition = aqua_core::scene::SnapshotTransition {
        duration: Duration::from_millis(700),
        band: 0.18,
        shimmer: aqua_core::Oklch::WHITE,
    };
    aqua.window_requests.push_back(WindowRequest::SetTitle(id, "next".into()));
    aqua.window_requests.push_back(WindowRequest::BeginSnapshotTransition(other, transition));
    aqua.window_requests.push_back(WindowRequest::BeginSnapshotTransition(id, transition));

    aqua.flush_output(&mut app, id);

    assert_eq!(aqua.window_requests.len(), 2);
    assert!(matches!(aqua.window_requests[0], WindowRequest::SetTitle(window, _) if window == id));
    assert!(matches!(aqua.window_requests[1], WindowRequest::BeginSnapshotTransition(window, _) if window == other));
}
