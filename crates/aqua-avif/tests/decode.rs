use aqua_avif::{decode, dimensions, AvifError, Rgba};

const QUAD: &[u8] = include_bytes!("fixtures/quad.avif");
const QUAD_ALPHA: &[u8] = include_bytes!("fixtures/quad-alpha.avif");
const QUAD_DEEP: &[u8] = include_bytes!("fixtures/quad-444.avif");

fn pixel(image: &Rgba, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * image.width + x) * 4) as usize;
    image.pixels[at..at + 4].try_into().unwrap()
}

fn close(actual: [u8; 4], expected: [u8; 4]) -> bool {
    actual
        .iter()
        .zip(expected)
        .all(|(actual, expected)| actual.abs_diff(expected) <= 6)
}

fn assert_quadrants(image: &Rgba, alpha: u8) {
    assert_eq!((image.width, image.height), (64, 48));
    for (x, y, color) in [
        (16, 12, [220, 40, 30]),
        (48, 12, [30, 180, 60]),
        (16, 36, [40, 70, 210]),
        (48, 36, [240, 240, 240]),
    ] {
        let expected = [color[0], color[1], color[2], alpha];
        let actual = pixel(image, x, y);
        assert!(
            close(actual, expected),
            "{actual:?} != {expected:?} at {x},{y}"
        );
    }
}

#[test]
fn subsampled_eight_bit_colors_survive_the_matrix() {
    assert_quadrants(&decode(QUAD, u64::MAX).unwrap(), 255);
}

#[test]
fn ten_bit_full_chroma_is_narrowed_to_eight_bits() {
    assert_quadrants(&decode(QUAD_DEEP, u64::MAX).unwrap(), 255);
}

#[test]
fn the_alpha_item_becomes_straight_alpha() {
    assert_quadrants(&decode(QUAD_ALPHA, u64::MAX).unwrap(), 128);
}

#[test]
fn the_pixel_cap_refuses_before_decoding() {
    assert_eq!(dimensions(QUAD), Ok((64, 48)));
    assert_eq!(
        decode(QUAD, 64 * 48 - 1),
        Err(AvifError::TooManyPixels(64 * 48))
    );
}

#[test]
fn truncated_files_fail_without_panicking() {
    for length in [0, 16, QUAD.len() / 2, QUAD.len() - 8] {
        assert!(decode(&QUAD[..length], u64::MAX).is_err());
    }
}
