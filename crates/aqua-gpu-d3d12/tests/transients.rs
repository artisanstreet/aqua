#![cfg(windows)]

use aqua_gpu_d3d12::hal::budget::Trim;
use aqua_gpu_d3d12::scene::{
    AdapterPreference, OffscreenBackend, SceneOptions, SurfaceFormat, TierLimit,
};
use aqua_gpu_ref::conformance::{cases, Case, Session};

fn layered_case() -> Case {
    cases()
        .into_iter()
        .filter(|case| !case.plan.textures.is_empty())
        .max_by_key(|case| case.plan.textures.len())
        .expect("the suite has a case with transients")
}

fn backends() -> Vec<(AdapterPreference, OffscreenBackend)> {
    [AdapterPreference::Warp, AdapterPreference::HighPerformance]
        .into_iter()
        .filter_map(|preference| {
            OffscreenBackend::with_tiers(preference, SurfaceFormat::Rgba16Float, TierLimit::MAX)
                .ok()
                .map(|backend| (preference, backend))
        })
        .collect()
}

#[test]
fn trimming_releases_the_transient_heap_and_the_next_frame_still_matches() {
    let case = layered_case();
    for (preference, mut backend) in backends() {
        let first = Session::new(&mut backend).render_case(&case).unwrap();
        let held = backend.transient_bytes();
        assert!(held > 0, "{preference:?}: the case should hold transients");
        let freed = backend
            .trim(&Trim {
                shrink_upload_ring: true,
                free_transients: true,
                evict_idle_resources: false,
            })
            .unwrap();
        assert_eq!(freed, held, "{preference:?}");
        assert_eq!(backend.transient_bytes(), 0, "{preference:?}");
        assert_eq!(backend.retired_transient_bytes(), 0, "{preference:?}");
        let again = Session::new(&mut backend).render_case(&case).unwrap();
        assert_eq!(
            again.rgba8, first.rgba8,
            "{preference:?}: frame after trim differs from the frame before it"
        );
        assert_eq!(backend.transient_bytes(), held, "{preference:?}");
    }
}

#[test]
fn steady_frames_neither_grow_nor_churn_the_transient_heap() {
    let case = layered_case();
    for (preference, mut backend) in backends() {
        let mut session = Session::new(&mut backend);
        session.render_case(&case).unwrap();
        let settled = session.backend().transient_bytes();
        for _ in 0..24 {
            session.render(&case.plan).unwrap();
            assert_eq!(
                session.backend().transient_bytes(),
                settled,
                "{preference:?}"
            );
        }
        assert_eq!(
            session.backend().retired_transient_bytes(),
            0,
            "{preference:?}"
        );
    }
}

#[test]
fn the_heap_is_no_larger_than_the_plan_with_its_slack() {
    let case = layered_case();
    for (preference, mut backend) in backends() {
        Session::new(&mut backend).render_case(&case).unwrap();
        let held = backend.transient_bytes();
        let planned = case.plan.heap_bytes;
        assert!(
            held <= planned * 2 + 4 * 65536,
            "{preference:?}: {held} bytes held for a {planned} byte plan"
        );
    }
}

#[test]
fn a_ceiling_below_the_plan_rejects_the_frame_instead_of_allocating() {
    let case = layered_case();
    for preference in [AdapterPreference::Warp, AdapterPreference::HighPerformance] {
        let Ok(mut backend) = OffscreenBackend::with_options(
            preference,
            SurfaceFormat::Rgba16Float,
            None,
            SceneOptions {
                transient_ceiling: Some(65536),
                ..SceneOptions::default()
            },
        ) else {
            continue;
        };
        let error = Session::new(&mut backend)
            .render_case(&case)
            .expect_err("the plan cannot fit under the ceiling");
        assert!(error.to_string().contains("ceiling"), "{error}");
        assert_eq!(backend.transient_bytes(), 0);
    }
}

#[test]
fn gpu_timestamps_report_a_frame_time_once_the_queue_catches_up() {
    let case = layered_case();
    for (preference, mut backend) in backends() {
        let mut session = Session::new(&mut backend);
        session.render_case(&case).unwrap();
        for _ in 0..6 {
            session.render(&case.plan).unwrap();
        }
        let measured = session.backend().gpu_time();
        eprintln!("{preference:?} gpu_time {measured:?}");
        let time = measured.unwrap_or_else(|| panic!("{preference:?}: no GPU time"));
        assert!(time > std::time::Duration::ZERO, "{preference:?}");
        assert!(time < std::time::Duration::from_secs(2), "{preference:?}");
    }
}
