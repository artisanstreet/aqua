use aqua_gpu_d3d12::placement::{Planned, place};

const KB64: u64 = 64 * 1024;

fn planned(offset: u64, bytes: u64) -> Planned {
    Planned { offset, bytes }
}

#[test]
fn aliased_textures_keep_sharing_an_offset_when_real_sizes_grow() {
    let result = place(
        &[planned(0, KB64), planned(0, KB64), planned(KB64, KB64)],
        &[2 * KB64, 2 * KB64, 2 * KB64],
        KB64,
    );
    assert_eq!(result.offsets, vec![0, 0, 2 * KB64]);
    assert_eq!(result.total, 4 * KB64);
}

#[test]
fn stacked_textures_never_overlap_after_growth() {
    let planned_set = [
        planned(0, KB64),
        planned(KB64, KB64),
        planned(2 * KB64, KB64),
    ];
    let real = [3 * KB64, KB64, 2 * KB64];
    let result = place(&planned_set, &real, KB64);
    for a in 0..3 {
        for b in a + 1..3 {
            let disjoint = result.offsets[a] + real[a] <= result.offsets[b]
                || result.offsets[b] + real[b] <= result.offsets[a];
            assert!(disjoint, "{a} {b} {:?}", result.offsets);
        }
    }
}

#[test]
fn empty_plan_needs_no_heap() {
    assert_eq!(place(&[], &[], KB64).total, 0);
}
