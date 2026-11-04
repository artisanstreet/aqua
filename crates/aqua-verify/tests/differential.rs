use aqua_gpu_ref::conformance::{compare, Session, Tolerance};
use aqua_gpu_ref::RefBackend;
use aqua_verify::differential::{scenes, variant_case, PlanVariant, VARIANTS};

const SEED: u64 = 0x5EED_D1FF;
const COUNT: usize = 160;

#[test]
fn every_plan_variant_renders_the_same_pixels_on_the_oracle() {
    let mut reference = RefBackend::new();
    let mut session = Session::new(&mut reference);
    let mut compared = 0;
    let mut failures = Vec::new();
    for scene in scenes(SEED, COUNT) {
        let baseline = variant_case(&scene, PlanVariant::Baseline).unwrap();
        let expected = session.render_case(&baseline).unwrap();
        for variant in VARIANTS.into_iter().skip(1) {
            let case = variant_case(&scene, variant).unwrap();
            let actual = session.render_case(&case).unwrap();
            let comparison = compare(&baseline.plan, &expected, &actual, Tolerance::EXACT);
            compared += 1;
            if !comparison.passed() {
                failures.push(format!(
                    "{}: interior {}, soft {}, edge {}",
                    case.name, comparison.interior_max, comparison.soft_max, comparison.edge_max
                ));
            }
        }
    }
    assert_eq!(compared, COUNT * (VARIANTS.len() - 1));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_oracle_is_deterministic_across_sessions() {
    let scenes = scenes(SEED ^ 1, 60);
    let render_all = || {
        let mut reference = RefBackend::new();
        let mut session = Session::new(&mut reference);
        scenes
            .iter()
            .map(|scene| {
                let case = variant_case(scene, PlanVariant::Baseline).unwrap();
                aqua_verify::fingerprint::of_bytes(&session.render_case(&case).unwrap().rgba8)
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(render_all(), render_all());
}
