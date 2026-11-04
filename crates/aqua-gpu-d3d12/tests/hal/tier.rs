use aqua_gpu_d3d12::hal::tier::*;

fn features(shader_model: ShaderModel, binding: BindingTier, enhanced: bool) -> HardwareFeatures {
    HardwareFeatures {
        feature_level: FeatureLevel::L12_0,
        shader_model,
        binding_tier: binding,
        enhanced_barriers: enhanced,
        fence_barriers: false,
        batched_copies: false,
    }
}

#[test]
fn bindless_needs_sm66_and_binding_tier_three() {
    let sm66 = ShaderModel { major: 6, minor: 6 };
    assert_eq!(
        features(sm66, BindingTier::Tier3, false)
            .tiers(Tier::Bindless)
            .binding,
        Tier::Bindless
    );
    assert_eq!(
        features(sm66, BindingTier::Tier2, false)
            .tiers(Tier::Bindless)
            .binding,
        Tier::Baseline
    );
    assert_eq!(
        features(ShaderModel::SM_6_0, BindingTier::Tier3, false)
            .tiers(Tier::Bindless)
            .binding,
        Tier::Baseline
    );
}

#[test]
fn a_cap_forces_every_better_tier_off() {
    let sm66 = ShaderModel { major: 6, minor: 6 };
    let hardware = features(sm66, BindingTier::Tier3, true);
    assert!(hardware.tiers(Tier::Bindless).enhanced_barriers);
    let capped = hardware.tiers(Tier::Baseline);
    assert_eq!(capped.binding, Tier::Baseline);
    assert!(!capped.enhanced_barriers);
}

#[test]
fn baseline_requires_feature_level_11_and_sm60() {
    let mut hardware = features(
        ShaderModel { major: 5, minor: 1 },
        BindingTier::Tier1,
        false,
    );
    assert!(!hardware.meets_baseline());
    hardware.shader_model = ShaderModel::SM_6_0;
    assert!(hardware.meets_baseline());
}
