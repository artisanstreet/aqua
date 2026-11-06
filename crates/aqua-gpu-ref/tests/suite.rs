use aqua_gpu_ref::conformance::{self, run_suite, SuiteConfig, Tolerance};
use aqua_gpu_ref::RefBackend;

#[test]
fn reference_passes_its_own_suite_exactly() {
    let mut a = RefBackend::new();
    let mut b = RefBackend::new();
    let config = SuiteConfig {
        tolerance: Tolerance::EXACT,
        fuzz_count: 300,
        ..SuiteConfig::default()
    };
    let report = run_suite(&mut a, &mut b, &config);
    assert!(report.passed(), "{}", report.failures().join("\n"));
    assert!(report.cases.len() >= 300 + conformance::cases().len());
}
