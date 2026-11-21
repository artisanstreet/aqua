#![cfg(target_os = "linux")]

mod support;

use aqua_gpu_vulkan::scene::{AdapterPreference, SurfaceFormat};
use aqua_protocol::{FeatureTier, GpuBackend};

#[test]
fn capabilities_report_baseline_tier_and_software_on_lavapipe() {
    let Some(backend) = support::open(AdapterPreference::Software, SurfaceFormat::Rgba16Float)
    else {
        return;
    };
    let capabilities = backend.capabilities();
    assert_eq!(capabilities.backend, "vulkan");
    assert!(
        capabilities.adapter.contains("llvmpipe"),
        "{}",
        capabilities.adapter
    );
    assert_eq!(capabilities.tier, FeatureTier::Baseline);
    assert!(capabilities.features.software);
    assert!(capabilities.features.compute && capabilities.features.color_matrix);
    assert!(!capabilities.features.custom_passes);
    assert!(!capabilities.features.window_surfaces);
    assert!(capabilities.max_texture_extent >= 16384);
}
