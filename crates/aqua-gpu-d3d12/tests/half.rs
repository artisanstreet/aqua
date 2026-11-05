use aqua_gpu_d3d12::half::{f16_to_f32, rgba16f_rows_to_rgba8, unorm8};

#[test]
fn decodes_normals_subnormals_and_specials() {
    assert_eq!(f16_to_f32(0x3c00), 1.0);
    assert_eq!(f16_to_f32(0xc000), -2.0);
    assert_eq!(f16_to_f32(0x3800), 0.5);
    assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
    assert_eq!(f16_to_f32(0x7c00), f32::INFINITY);
    assert!(f16_to_f32(0x7e00).is_nan());
}

#[test]
fn quantizes_with_clamping_and_rounding() {
    assert_eq!(unorm8(-1.0), 0);
    assert_eq!(unorm8(2.0), 255);
    assert_eq!(unorm8(0.5), 128);
}

#[test]
fn row_pitch_padding_is_skipped() {
    let one = 0x3c00u16.to_le_bytes();
    let zero = 0u16.to_le_bytes();
    let texel = [one, zero, zero, one].concat();
    let mut source = Vec::new();
    for _ in 0..2 {
        source.extend_from_slice(&texel);
        source.extend_from_slice(&[0xff; 24]);
    }
    let pixels = rgba16f_rows_to_rgba8(&source, 32, 1, 2);
    assert_eq!(pixels, vec![255, 0, 0, 255, 255, 0, 0, 255]);
}

#[test]
fn bgra_rows_swap_to_rgba() {
    use aqua_gpu_d3d12::half::bgra8_rows_to_rgba8;
    let source = [10u8, 20, 30, 40, 0xff, 0xff, 0xff, 0xff];
    assert_eq!(
        bgra8_rows_to_rgba8(&source, 8, 2, 1),
        vec![30, 20, 10, 40, 255, 255, 255, 255]
    );
}
