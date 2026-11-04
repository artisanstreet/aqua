#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FeatureLevel {
    L11_0,
    L11_1,
    L12_0,
    L12_1,
    L12_2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ShaderModel {
    pub major: u8,
    pub minor: u8,
}

impl ShaderModel {
    pub const SM_6_0: ShaderModel = ShaderModel { major: 6, minor: 0 };
    pub const SM_6_6: ShaderModel = ShaderModel { major: 6, minor: 6 };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BindingTier {
    Tier1,
    Tier2,
    Tier3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HardwareFeatures {
    pub feature_level: FeatureLevel,
    pub shader_model: ShaderModel,
    pub binding_tier: BindingTier,
    pub enhanced_barriers: bool,
    pub fence_barriers: bool,
    pub batched_copies: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Baseline,
    Bindless,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tiers {
    pub binding: Tier,
    pub enhanced_barriers: bool,
    pub fence_barriers: bool,
    pub batched_copies: bool,
}

impl HardwareFeatures {
    pub fn meets_baseline(&self) -> bool {
        self.feature_level >= FeatureLevel::L11_0 && self.shader_model >= ShaderModel::SM_6_0
    }

    pub fn tiers(&self, cap: Tier) -> Tiers {
        let bindless_capable =
            self.shader_model >= ShaderModel::SM_6_6 && self.binding_tier >= BindingTier::Tier3;
        let allow_better = cap >= Tier::Bindless;
        Tiers {
            binding: match bindless_capable && allow_better {
                true => Tier::Bindless,
                false => Tier::Baseline,
            },
            enhanced_barriers: self.enhanced_barriers && allow_better,
            fence_barriers: self.fence_barriers && allow_better,
            batched_copies: self.batched_copies && allow_better,
        }
    }
}
