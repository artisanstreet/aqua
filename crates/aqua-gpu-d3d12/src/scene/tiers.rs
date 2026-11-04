use windows::Win32::Graphics::Direct3D12::ID3D12Device;

use super::barriers::BarrierMode;
use super::bindless::Binding;
use crate::hal::detect;
use crate::hal::tier::{Tier, Tiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TierLimit {
    pub binding: Tier,
    pub enhanced_barriers: bool,
}

impl TierLimit {
    pub const MAX: TierLimit = TierLimit {
        binding: Tier::Bindless,
        enhanced_barriers: true,
    };
    pub const BASELINE: TierLimit = TierLimit {
        binding: Tier::Baseline,
        enhanced_barriers: false,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneOptions {
    pub limit: TierLimit,
    pub transient_ceiling: Option<u64>,
    pub timestamps: bool,
}

impl Default for SceneOptions {
    fn default() -> Self {
        Self {
            limit: TierLimit::MAX,
            transient_ceiling: None,
            timestamps: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Active {
    pub binding: Binding,
    pub sync: BarrierMode,
    pub tiers: Tiers,
}

pub fn resolve(device: &ID3D12Device, limit: TierLimit) -> Active {
    let hardware = detect(device);
    let best = hardware.tiers(Tier::Bindless);
    let binding = match limit.binding {
        Tier::Bindless => best.binding,
        Tier::Baseline => Tier::Baseline,
    };
    let enhanced = best.enhanced_barriers && limit.enhanced_barriers;
    Active {
        binding: Binding::of(binding),
        sync: match enhanced {
            true => BarrierMode::Enhanced,
            false => BarrierMode::Queue,
        },
        tiers: Tiers {
            binding,
            enhanced_barriers: enhanced,
            fence_barriers: false,
            batched_copies: false,
        },
    }
}
