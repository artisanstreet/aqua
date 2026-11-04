use aqua_gpu_ref::RefBackend;
use aqua_gpu_ref::conformance::{SuiteConfig, run_suite};
use aqua_gpu_wgpu::WgpuBackend;
use aqua_protocol::GpuError;

fn backend() -> Option<WgpuBackend> {
    match WgpuBackend::headless() {
        Ok(backend) => Some(backend),
        Err(GpuError::DeviceFailure(detail))
            if detail.contains("no usable GPU adapter")
                && std::env::var("AQUA_REQUIRE_GPU").ok().as_deref() != Some("1") =>
        {
            eprintln!("SKIP-NO-ADAPTER: {detail}");
            None
        }
        Err(error) => panic!("wgpu adapter setup failed: {error}"),
    }
}

#[test]
fn wgpu_adapter_matches_cpu_reference_on_every_supported_case() {
    let Some(mut backend) = backend() else {
        return;
    };
    let config = SuiteConfig {
        tolerate_unsupported: true,
        fuzz_count: 0,
        ..SuiteConfig::default()
    };
    let report = run_suite(&mut RefBackend::new(), &mut backend, &config);
    eprintln!("{report}");
    assert!(report.passed(), "{}", report.failures().join("\n"));
}
