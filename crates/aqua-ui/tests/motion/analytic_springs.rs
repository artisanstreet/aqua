use aqua_core::Oklch;
use aqua_ui::motion::{
    Animated, MotionKind, MotionValue, ReducedMotion, Seconds, SpringParams, SpringSegment,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::f32::consts::TAU;

struct CountingAllocator;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

fn record_allocation() {
    let tracking = TRACKING.try_with(Cell::get).unwrap_or(false);
    if tracking {
        let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record_allocation();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn count_allocations(run: impl FnOnce()) -> usize {
    ALLOCATIONS.with(|count| count.set(0));
    TRACKING.with(|flag| flag.set(true));
    run();
    TRACKING.with(|flag| flag.set(false));
    ALLOCATIONS.with(Cell::get)
}

fn rk4_trajectory(
    params: SpringParams,
    x0: f64,
    v0: f64,
    target: f64,
    step: f64,
    checkpoints: &[f64],
) -> Vec<(f64, f64)> {
    let (k, c) = (f64::from(params.stiffness), f64::from(params.damping));
    let accel = |x: f64, v: f64| k * (target - x) - c * v;
    let mut state = (x0, v0);
    let mut t = 0.0;
    checkpoints
        .iter()
        .map(|&checkpoint| {
            while t + step * 0.5 < checkpoint {
                let (x, v) = state;
                let (k1x, k1v) = (v, accel(x, v));
                let (k2x, k2v) = (
                    v + 0.5 * step * k1v,
                    accel(x + 0.5 * step * k1x, v + 0.5 * step * k1v),
                );
                let (k3x, k3v) = (
                    v + 0.5 * step * k2v,
                    accel(x + 0.5 * step * k2x, v + 0.5 * step * k2v),
                );
                let (k4x, k4v) = (v + step * k3v, accel(x + step * k3x, v + step * k3v));
                state = (
                    x + step / 6.0 * (k1x + 2.0 * k2x + 2.0 * k3x + k4x),
                    v + step / 6.0 * (k1v + 2.0 * k2v + 2.0 * k3v + k4v),
                );
                t += step;
            }
            state
        })
        .collect()
}

fn assert_matches_numerical(params: SpringParams) {
    let segment = SpringSegment {
        t0: 0.0,
        x0: -0.5,
        v0: 3.0,
        target: 1.0,
    };
    let checkpoints: Vec<f64> = (1..=150).map(|i| f64::from(i) * 0.01).collect();
    let reference = rk4_trajectory(params, -0.5, 3.0, 1.0, 1e-5, &checkpoints);
    for (&t, &(x_ref, v_ref)) in checkpoints.iter().zip(&reference) {
        let (x, v) = segment.sample(params, t);
        assert!(
            (f64::from(x) - x_ref).abs() < 1e-4,
            "{params:?} t={t} x={x} reference={x_ref}"
        );
        assert!(
            (f64::from(v) - v_ref).abs() < 1e-4 * v_ref.abs().max(1.0),
            "{params:?} t={t} v={v} reference={v_ref}"
        );
    }
}

#[test]
fn underdamped_solution_matches_rk4() {
    assert_matches_numerical(SpringParams::new(400.0, 8.0));
    assert_matches_numerical(SpringParams::SNAPPY);
}

#[test]
fn critically_damped_solution_matches_rk4() {
    assert_matches_numerical(SpringParams::new(100.0, 20.0));
}

#[test]
fn overdamped_solution_matches_rk4() {
    assert_matches_numerical(SpringParams::new(100.0, 45.0));
}

fn spring(initial: MotionValue, params: SpringParams) -> Animated {
    Animated::new(
        initial,
        MotionKind::Spring(params),
        ReducedMotion::Full,
        0.0,
    )
}

#[test]
fn retarget_mid_flight_is_c0_and_c1_continuous() {
    let params = SpringParams::new(300.0, 12.0);
    let mut anim = spring(MotionValue::Pair(0.0, 10.0), params);
    anim.set_target(MotionValue::Pair(1.0, -4.0), 0.0).unwrap();
    anim.sample(0.083);
    let now: Seconds = 0.137;
    let before: Vec<(f32, f32)> = anim
        .spring_segments()
        .iter()
        .map(|segment| segment.sample(params, now))
        .collect();
    assert!(before.iter().any(|&(_, v)| v.abs() > 1.0));

    anim.set_target(MotionValue::Pair(-2.0, 6.0), now).unwrap();
    let segments = anim.spring_segments();
    assert_eq!(segments.len(), 2);
    for (segment, &(x, v)) in segments.iter().zip(&before) {
        assert_eq!(segment.t0, now);
        assert!(
            (segment.x0 - x).abs() < 1e-6,
            "x0={} before={x}",
            segment.x0
        );
        assert_eq!(segment.v0, v);
        let (x_after, v_after) = segment.sample(params, now);
        assert!((x_after - x).abs() < 1e-6);
        assert!((v_after - v).abs() < 1e-5);
    }
    let MotionValue::Pair(px, py) = anim.sample(now) else {
        panic!("pair")
    };
    assert!((px - before[0].0).abs() < 1e-6 && (py - before[1].0).abs() < 1e-6);
}

#[test]
fn clock_jump_matches_incremental_sampling() {
    let params = SpringParams::new(100.0, 0.5);
    let mut jumped = spring(MotionValue::Scalar(0.0), params);
    jumped.set_target(MotionValue::Scalar(1.0), 0.0).unwrap();
    let mut stepped = spring(MotionValue::Scalar(0.0), params);
    stepped.set_target(MotionValue::Scalar(1.0), 0.0).unwrap();

    let end: Seconds = 7.9;
    (1..790).for_each(|i| {
        stepped.sample(f64::from(i) * 0.01);
    });
    let a = jumped.sample(end);
    let b = stepped.sample(end);
    assert!(!jumped.is_settled(), "test spring must still be in flight");
    let (MotionValue::Scalar(a), MotionValue::Scalar(b)) = (a, b) else {
        panic!("scalar")
    };
    let (expected, _) = SpringSegment {
        t0: 0.0,
        x0: 0.0,
        v0: 0.0,
        target: 1.0,
    }
    .sample(params, end);
    assert!((a - b).abs() < 1e-6, "jumped={a} stepped={b}");
    assert!(
        (a - expected).abs() < 1e-6,
        "jumped={a} closed form={expected}"
    );
}

#[test]
fn from_duration_bounce_converts_to_stiffness_and_damping() {
    let params = SpringParams::from_duration_bounce(0.5, 0.2);
    let omega = TAU / 0.5;
    assert!((params.stiffness - omega * omega).abs() < 1e-3);
    assert!((params.damping - 2.0 * 0.8 * omega).abs() < 1e-4);

    let critical = SpringParams::from_duration_bounce(1.0, -3.0);
    assert!((critical.damping - 2.0 * TAU).abs() < 1e-4);

    let bouncy = SpringParams::from_duration_bounce(1.0, 4.0);
    assert!((bouncy.damping - 2.0 * 0.05 * TAU).abs() < 1e-4);
    assert!(bouncy.damping > 0.0);

    let degenerate = SpringParams::from_duration_bounce(0.0, 0.0);
    assert!(degenerate.stiffness.is_finite() && degenerate.stiffness > 0.0);
    assert!(degenerate.damping.is_finite() && degenerate.damping > 0.0);
}

#[test]
fn color_spring_samples_and_retargets_without_allocating() {
    let palette = [
        MotionValue::Color(Oklch::oklch(0.5, 0.05, 20.0, 1.0)),
        MotionValue::Color(Oklch::oklch(0.7, 0.12, 340.0, 0.6)),
        MotionValue::Color(Oklch::oklch(0.3, 0.08, 160.0, 0.9)),
    ];
    let mut anim = spring(palette[0], SpringParams::SNAPPY);
    let allocations = count_allocations(|| {
        (0..1000u32).for_each(|i| {
            let now = f64::from(i) * 0.004;
            if i % 10 == 0 {
                anim.set_target(palette[(i as usize / 10) % palette.len()], now)
                    .unwrap();
            }
            anim.sample(now);
        });
    });
    assert_eq!(allocations, 0);
}
