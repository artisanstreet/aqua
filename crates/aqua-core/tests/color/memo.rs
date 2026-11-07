use super::{decode_srgb, Memo};
use crate::Oklch;
use std::cell::Cell;

#[test]
fn exact_conversion_survives_collisions_and_preserves_alpha() {
    for index in 0..20000u32 {
        let color = [(index * 17) as u8, (index >> 3) as u8, (index * 59) as u8];
        let expected = decode_srgb(color);
        for alpha in [0.0, -0.0, 0.3, 1.0, 2.0, f32::NAN] {
            let actual = Oklch::from_srgb8(color[0], color[1], color[2], alpha);
            assert_eq!(
                [actual.l.to_bits(), actual.c.to_bits(), actual.h.to_bits()],
                expected.map(f32::to_bits)
            );
            assert_eq!(actual.alpha.to_bits(), alpha.clamp(0.0, 1.0).to_bits());
        }
    }
}

#[test]
fn repeated_values_are_computed_once_and_clear_discards_them() {
    let mut memo = Memo::default();
    let calls = Cell::new(0);
    for _ in 0..10000 {
        assert_eq!(
            memo.lookup(7u32, u64::from, || {
                calls.set(calls.get() + 1);
                42u32
            }),
            42
        );
    }
    assert_eq!(calls.get(), 1);
    assert_eq!(memo.slots(), 256);
    memo.clear();
    assert_eq!(memo.slots(), 0);
    assert_eq!(memo.lookup(7, u64::from, || 43), 43);
}

#[test]
fn high_volume_growth_is_bounded_and_never_returns_a_colliding_value() {
    let mut memo = Memo::default();
    for key in 0..200000u32 {
        let result = memo.lookup(
            key,
            |key| u64::from(key.wrapping_mul(0x9e3779b9)) << 32,
            || key.rotate_left(11),
        );
        assert_eq!(result, key.rotate_left(11));
    }
    assert_eq!(memo.slots(), 65536);
}
