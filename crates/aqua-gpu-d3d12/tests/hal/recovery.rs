use std::time::Duration;

use aqua_gpu_d3d12::hal::recovery::*;
use aqua_gpu_d3d12::hal::selection::Adapter;

#[test]
fn dxgi_codes_map_to_loss_causes() {
    assert_eq!(classify(0x887A0005u32 as i32), Some(LossCause::Removed));
    assert_eq!(classify(0x887A0006u32 as i32), Some(LossCause::Hung));
    assert_eq!(classify(0x887A0007u32 as i32), Some(LossCause::Reset));
    assert_eq!(
        classify(0x887A0020u32 as i32),
        Some(LossCause::DriverInternal)
    );
    assert_eq!(classify(0x887A0001u32 as i32), Some(LossCause::InvalidCall));
    assert_eq!(classify(0x80070057u32 as i32), None);
}

#[test]
fn repeated_hardware_losses_fall_back_to_warp() {
    let mut history = LossHistory::new(Duration::from_secs(10), 2);
    assert_eq!(
        history.record(Duration::from_secs(1), Adapter::Hardware),
        RecoveryStep::Reselect
    );
    assert_eq!(
        history.record(Duration::from_secs(5), Adapter::Hardware),
        RecoveryStep::FallBackToWarp
    );
}

#[test]
fn losses_outside_the_window_are_forgotten() {
    let mut history = LossHistory::new(Duration::from_secs(10), 2);
    history.record(Duration::from_secs(1), Adapter::Hardware);
    assert_eq!(
        history.record(Duration::from_secs(60), Adapter::Hardware),
        RecoveryStep::Reselect
    );
}

#[test]
fn software_adapter_never_falls_back_further() {
    let mut history = LossHistory::new(Duration::from_secs(10), 1);
    assert_eq!(
        history.record(Duration::from_secs(1), Adapter::Software),
        RecoveryStep::Reselect
    );
}
