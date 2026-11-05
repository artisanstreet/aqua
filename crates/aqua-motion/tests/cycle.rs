use aqua_motion::{Cycle, CycleSet};

const PULSE: Cycle = Cycle::Pulse {
    period: 1.6,
    floor: 0.4,
};

#[test]
fn pulse_swings_between_full_and_floor_over_one_period() {
    assert!((PULSE.value(0.0) - 1.0).abs() < 1e-6);
    assert!((PULSE.value(0.8) - 0.4).abs() < 1e-5);
    assert!((PULSE.value(1.6) - 1.0).abs() < 1e-5);
    assert!((PULSE.value(0.4) - 0.7).abs() < 1e-5);
}

#[test]
fn pulse_is_continuous_across_its_period_boundary() {
    let before = PULSE.value(1.6 - 1e-6);
    let after = PULSE.value(1.6 + 1e-6);
    assert!((before - after).abs() < 1e-4);
}

#[test]
fn pulse_values_stay_inside_the_floor_and_one() {
    (0..4000).for_each(|step| {
        let value = PULSE.value(f64::from(step) * 0.0137);
        assert!((0.4 - 1e-5..=1.0 + 1e-6).contains(&value), "{value}");
    });
}

#[test]
fn sweep_waits_for_its_delay_then_runs_zero_to_one_and_repeats() {
    let sweep = Cycle::Sweep {
        delay: 1.0,
        duration: 2.0,
    };
    assert_eq!(sweep.value(0.0), 0.0);
    assert_eq!(sweep.value(0.99), 0.0);
    assert!((sweep.value(2.0) - 0.5).abs() < 1e-6);
    assert!(sweep.value(2.999) > 0.999);
    assert_eq!(sweep.value(3.0), 0.0);
    assert!((sweep.value(5.0) - 0.5).abs() < 1e-6);
}

#[test]
fn degenerate_cycles_are_inert() {
    assert_eq!(
        Cycle::Pulse {
            period: 0.0,
            floor: 0.2
        }
        .value(3.0),
        1.0
    );
    assert_eq!(
        Cycle::Sweep {
            delay: 1.0,
            duration: 0.0
        }
        .value(3.0),
        0.0
    );
}

#[test]
fn value_is_independent_of_how_often_it_is_sampled() {
    let sweep = Cycle::Sweep {
        delay: 0.25,
        duration: 1.5,
    };
    let mut set = CycleSet::<u64>::new();
    let dense: Vec<f32> = (0..=700)
        .map(|step| {
            set.begin_frame();
            let value = set.sample(7, sweep, f64::from(step) * 0.01);
            set.end_frame();
            value
        })
        .collect();
    let mut sparse = CycleSet::<u64>::new();
    for step in (0..=700).step_by(35) {
        sparse.begin_frame();
        let value = sparse.sample(7, sweep, f64::from(step) * 0.01);
        sparse.end_frame();
        assert_eq!(value, dense[step as usize]);
    }
}

#[test]
fn anchors_on_first_sight_and_forgets_items_that_disappear() {
    let mut set = CycleSet::<u32>::new();
    set.begin_frame();
    assert!((set.sample(1, PULSE, 10.0) - 1.0).abs() < 1e-6);
    set.end_frame();
    set.begin_frame();
    assert!((set.sample(1, PULSE, 10.8) - 0.4).abs() < 1e-5);
    set.end_frame();
    assert!(set.active());
    set.begin_frame();
    set.end_frame();
    assert!(!set.active());
    set.begin_frame();
    assert!((set.sample(1, PULSE, 20.0) - 1.0).abs() < 1e-6);
}

#[test]
fn a_changed_cycle_restarts_from_its_anchor() {
    let mut set = CycleSet::<u32>::new();
    set.begin_frame();
    set.sample(1, PULSE, 0.0);
    let faster = Cycle::Pulse {
        period: 0.8,
        floor: 0.4,
    };
    assert!((set.sample(1, faster, 3.0) - 1.0).abs() < 1e-6);
}

#[test]
fn a_sweep_holds_only_through_its_delay() {
    let sweep = Cycle::Sweep {
        delay: 1.5,
        duration: 3.0,
    };
    assert_eq!(sweep.holds_for(0.0), Some(1.5));
    assert!((sweep.holds_for(1.0).unwrap() - 0.5).abs() < 1e-9);
    assert_eq!(sweep.holds_for(1.5), None);
    assert_eq!(sweep.holds_for(4.0), None);
    assert!((sweep.holds_for(4.5 + 0.25).unwrap() - 1.25).abs() < 1e-9);
    assert_eq!(PULSE.holds_for(0.3), None);
    let without_delay = Cycle::Sweep {
        delay: 0.0,
        duration: 2.0,
    };
    assert_eq!(without_delay.holds_for(0.5), None);
}

#[test]
fn a_cycle_set_reports_the_hold_relative_to_its_anchor() {
    let sweep = Cycle::Sweep {
        delay: 2.0,
        duration: 1.0,
    };
    let mut set: CycleSet<u8> = CycleSet::new();
    set.sample(1, sweep, 10.0);
    assert_eq!(set.sample(1, sweep, 10.5), 0.0);
    assert!((set.holds_for(1, 10.5).unwrap() - 1.5).abs() < 1e-9);
    assert_eq!(set.holds_for(1, 12.5), None);
    assert_eq!(set.holds_for(2, 10.5), None);
}
