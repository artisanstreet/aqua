use std::ffi::c_void;
use std::mem::size_of;

use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D12::*;

use super::tier::{BindingTier, FeatureLevel, HardwareFeatures, ShaderModel};

fn query<T>(device: &ID3D12Device, feature: D3D12_FEATURE, data: &mut T) -> bool {
    unsafe {
        device.CheckFeatureSupport(
            feature,
            data as *mut T as *mut c_void,
            size_of::<T>() as u32,
        )
    }
    .is_ok()
}

fn feature_level(device: &ID3D12Device) -> FeatureLevel {
    let requested = [
        D3D_FEATURE_LEVEL_12_2,
        D3D_FEATURE_LEVEL_12_1,
        D3D_FEATURE_LEVEL_12_0,
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
    ];
    let mut data = D3D12_FEATURE_DATA_FEATURE_LEVELS {
        NumFeatureLevels: requested.len() as u32,
        pFeatureLevelsRequested: requested.as_ptr(),
        MaxSupportedFeatureLevel: D3D_FEATURE_LEVEL_11_0,
    };
    query(device, D3D12_FEATURE_FEATURE_LEVELS, &mut data);
    match data.MaxSupportedFeatureLevel {
        level if level == D3D_FEATURE_LEVEL_12_2 => FeatureLevel::L12_2,
        level if level == D3D_FEATURE_LEVEL_12_1 => FeatureLevel::L12_1,
        level if level == D3D_FEATURE_LEVEL_12_0 => FeatureLevel::L12_0,
        level if level == D3D_FEATURE_LEVEL_11_1 => FeatureLevel::L11_1,
        _ => FeatureLevel::L11_0,
    }
}

fn shader_model(device: &ID3D12Device) -> ShaderModel {
    [
        0x6a, 0x69, 0x68, 0x67, 0x66, 0x65, 0x64, 0x63, 0x62, 0x61, 0x60,
    ]
    .into_iter()
    .find_map(|code| {
        let mut data = D3D12_FEATURE_DATA_SHADER_MODEL {
            HighestShaderModel: D3D_SHADER_MODEL(code),
        };
        query(device, D3D12_FEATURE_SHADER_MODEL, &mut data).then_some(ShaderModel {
            major: (data.HighestShaderModel.0 >> 4) as u8,
            minor: (data.HighestShaderModel.0 & 0xf) as u8,
        })
    })
    .unwrap_or(ShaderModel { major: 5, minor: 1 })
}

fn binding_tier(device: &ID3D12Device) -> BindingTier {
    let mut data = D3D12_FEATURE_DATA_D3D12_OPTIONS::default();
    query(device, D3D12_FEATURE_D3D12_OPTIONS, &mut data);
    match data.ResourceBindingTier {
        tier if tier == D3D12_RESOURCE_BINDING_TIER_3 => BindingTier::Tier3,
        tier if tier == D3D12_RESOURCE_BINDING_TIER_2 => BindingTier::Tier2,
        _ => BindingTier::Tier1,
    }
}

fn enhanced_barriers(device: &ID3D12Device) -> bool {
    let mut data = D3D12_FEATURE_DATA_D3D12_OPTIONS12::default();
    query(device, D3D12_FEATURE_D3D12_OPTIONS12, &mut data)
        && data.EnhancedBarriersSupported.as_bool()
}

pub fn detect(device: &ID3D12Device) -> HardwareFeatures {
    HardwareFeatures {
        feature_level: feature_level(device),
        shader_model: shader_model(device),
        binding_tier: binding_tier(device),
        enhanced_barriers: enhanced_barriers(device),
        fence_barriers: false,
        batched_copies: false,
    }
}
