use aqua_gpu_d3d11::hal::selection::AdapterPreference;
use aqua_gpu_d3d11::window::WindowBackend;

use crate::backend::options;

#[test]
fn deferred_startup_drains_in_bounded_steps_and_leaves_no_pending_work() {
    crate::support::hold_software();
    WindowBackend::begin_early(options(AdapterPreference::Warp), || None);
    let mut backend = WindowBackend::take_early()
        .expect("early backend was started")
        .expect("early backend builds on WARP");
    assert!(!backend.startup_pending());
    let before = backend.pipeline_stats().expect("pipelines exist");

    backend.finish_startup();
    assert!(backend.startup_pending());
    let steps =
        std::iter::from_fn(|| backend.startup_pending().then(|| backend.startup_step())).count();

    let after = backend.pipeline_stats().expect("pipelines exist");
    assert!(steps > 0);
    assert!(after.compiled + after.loaded > before.compiled + before.loaded);
    assert_eq!(after.late, 0);
    assert!(!backend.startup_pending());
}
