use aqua_gpu_d3d12::hal::budget::*;

fn memory(usage: u64) -> MemoryBudget {
    MemoryBudget {
        budget: 1000,
        usage,
    }
}

#[test]
fn pressure_rises_with_usage_over_budget() {
    assert_eq!(pressure(memory(500)), Pressure::None);
    assert_eq!(pressure(memory(800)), Pressure::Soft);
    assert_eq!(pressure(memory(960)), Pressure::Hard);
    assert_eq!(
        pressure(MemoryBudget {
            budget: 0,
            usage: 10
        }),
        Pressure::None
    );
}

#[test]
fn hidden_windows_release_everything_regardless_of_pressure() {
    let trim = trim_for(Pressure::None, Visibility::Hidden);
    assert!(trim.shrink_upload_ring && trim.free_transients && trim.evict_idle_resources);
}

#[test]
fn visible_windows_only_evict_under_hard_pressure() {
    assert_eq!(
        trim_for(Pressure::None, Visibility::Visible),
        Trim::default()
    );
    assert!(!trim_for(Pressure::Soft, Visibility::Visible).evict_idle_resources);
    assert!(trim_for(Pressure::Hard, Visibility::Visible).evict_idle_resources);
}
