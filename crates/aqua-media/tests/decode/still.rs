use std::io::Cursor;

use aqua_media::decode::still::{dimensions, sniff, MAX_DETAIL_PIXELS};
use aqua_media::decode::{
    decode_bytes, decode_still, DecodedImage, Detail, MediaError, StillFormat,
};

const HEIC: &[u8] = include_bytes!("../fixtures/quad.heic");
const AVIF: &[u8] = include_bytes!("../../../aqua-avif/tests/fixtures/quad.avif");
const QUADRANTS: [[u8; 3]; 4] = [[220, 40, 30], [30, 180, 60], [40, 70, 210], [240, 240, 240]];
const BUDGET: u64 = 256 * 1024 * 1024;

fn quad(width: u32, height: u32) -> image::RgbImage {
    image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb(QUADRANTS[usize::from(x >= width / 2) + 2 * usize::from(y >= height / 2)])
    })
}

fn encoded(image: &image::RgbImage, format: image::ImageFormat) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, format).unwrap();
    bytes.into_inner()
}

fn with_orientation(jpeg: &[u8], orientation: u8) -> Vec<u8> {
    let mut exif = b"Exif\0\0II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
    exif.extend_from_slice(&[orientation, 0, 0, 0, 0, 0, 0, 0]);
    let length = u16::try_from(exif.len() + 2).unwrap().to_be_bytes();
    [&jpeg[..2], &[0xff, 0xe1], &length, &exif, &jpeg[2..]].concat()
}

fn bmp_header(width: u32, height: u32) -> Vec<u8> {
    let mut header = b"BM".to_vec();
    header.extend_from_slice(&[0; 8]);
    header.extend_from_slice(&54u32.to_le_bytes());
    header.extend_from_slice(&40u32.to_le_bytes());
    header.extend_from_slice(&width.to_le_bytes());
    header.extend_from_slice(&height.to_le_bytes());
    header.extend_from_slice(&[1, 0, 24, 0]);
    header.extend_from_slice(&[0; 24]);
    header
}

fn assert_quadrants(image: &DecodedImage, order: [usize; 4], tolerance: u8) {
    let (width, height) = (image.width(), image.height());
    for (index, (x, y)) in [(1, 1), (3, 1), (1, 3), (3, 3)].into_iter().enumerate() {
        let actual = image.pixel_at(width * x / 4, height * y / 4).unwrap();
        let expected = QUADRANTS[order[index]];
        assert!(
            actual[..3]
                .iter()
                .zip(expected)
                .all(|(actual, expected)| actual.abs_diff(expected) <= tolerance),
            "{actual:?} != {expected:?} in quadrant {index}"
        );
    }
}

#[test]
fn every_enabled_format_is_recognized_and_decoded() {
    let jpeg = encoded(&quad(64, 48), image::ImageFormat::Jpeg);
    let tiff = encoded(&quad(64, 48), image::ImageFormat::Tiff);
    let mut ico = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(quad(32, 32))
        .to_rgba8()
        .write_to(&mut ico, image::ImageFormat::Ico)
        .unwrap();
    let ico = ico.into_inner();
    for (bytes, format, extent, tolerance) in [
        (&tiff[..], StillFormat::Tiff, (64, 48), 0),
        (&ico[..], StillFormat::Ico, (32, 32), 0),
        (AVIF, StillFormat::Avif, (64, 48), 6),
        (HEIC, StillFormat::Heic, (64, 48), 8),
        (&jpeg[..], StillFormat::Jpeg, (64, 48), 8),
    ] {
        assert_eq!(sniff(bytes), Some(format));
        let still = decode_still(bytes, Detail::STANDARD, BUDGET).unwrap();
        assert_eq!(still.source, extent, "{format:?}");
        assert_eq!((still.image.width(), still.image.height()), extent);
        assert_quadrants(&still.image, [0, 1, 2, 3], tolerance);
        assert_quadrants(&decode_bytes(bytes).unwrap(), [0, 1, 2, 3], tolerance);
    }
}

#[test]
fn an_exif_rotation_turns_the_pixels_and_the_reported_extent() {
    let upright = encoded(&quad(64, 48), image::ImageFormat::Jpeg);
    let turned = decode_still(&with_orientation(&upright, 6), Detail::STANDARD, BUDGET).unwrap();
    assert_eq!(turned.source, (48, 64));
    assert_quadrants(&turned.image, [2, 0, 3, 1], 8);
}

#[test]
fn detail_bounds_the_long_side_and_never_enlarges() {
    let wide = encoded(&quad(4000, 1000), image::ImageFormat::Png);
    let standard = decode_still(&wide, Detail::STANDARD, BUDGET).unwrap();
    assert_eq!(standard.source, (4000, 1000));
    assert_eq!(
        (standard.image.width(), standard.image.height()),
        (2048, 512)
    );
    assert_quadrants(&standard.image, [0, 1, 2, 3], 0);
    let full = decode_still(&wide, Detail::FULL, BUDGET).unwrap();
    assert_eq!((full.image.width(), full.image.height()), (4000, 1000));
    let small = decode_still(HEIC, Detail::FULL, BUDGET).unwrap();
    assert_eq!((small.image.width(), small.image.height()), (64, 48));
}

#[test]
fn detail_steps_cover_a_displayed_size_with_at_most_double_the_pixels() {
    assert_eq!(Detail::covering(1.0), Detail::SMALLEST);
    assert_eq!(Detail::covering(2048.0), Detail::STANDARD);
    assert_eq!(Detail::covering(2049.0).long_side(), 2896);
    assert_eq!(Detail::covering(1e9), Detail::FULL);
    assert_eq!(Detail::STANDARD.lower().unwrap().long_side(), 1448);
    assert_eq!(Detail::SMALLEST.lower(), None);
    for side in [300.0f32, 700.0, 1500.0, 3000.0, 6000.0] {
        let covering = Detail::covering(side).long_side() as f32;
        assert!(covering >= side && covering * covering <= side * side * 2.05);
    }
    let (width, height) = Detail::FULL.fit((60_000, 2_000));
    assert_eq!((width, height), (8192, 273));
    let (width, height) = Detail::FULL.fit((11_000, 11_000));
    assert!(u64::from(width) * u64::from(height) <= MAX_DETAIL_PIXELS);
    assert_eq!(Detail::STANDARD.fit((640, 480)), (640, 480));
}

#[test]
fn a_decode_that_would_not_fit_the_budget_is_refused_before_resizing() {
    let wide = encoded(&quad(4000, 1000), image::ImageFormat::Png);
    assert_eq!(
        decode_still(&wide, Detail::STANDARD, 2048 * 512 * 4 - 1).unwrap_err(),
        MediaError::OverBudget
    );
}

#[test]
fn sources_beyond_the_caps_are_refused_from_the_header() {
    assert_eq!(
        decode_still(&bmp_header(20_000, 20_000), Detail::SMALLEST, BUDGET).unwrap_err(),
        MediaError::TooManyPixels(400_000_000)
    );
    assert_eq!(
        dimensions(&bmp_header(11_000, 11_000)),
        Ok((11_000, 11_000))
    );
    assert_eq!(
        decode_bytes(&bmp_header(11_000, 11_000)).unwrap_err(),
        MediaError::DimensionsTooLarge(11_000, 11_000)
    );
}

#[test]
fn malformed_files_fail_without_panicking() {
    for bytes in [HEIC, AVIF] {
        for length in [12, bytes.len() / 2] {
            assert!(decode_still(&bytes[..length], Detail::STANDARD, BUDGET).is_err());
        }
    }
    assert_eq!(
        decode_still(b"plain text", Detail::STANDARD, BUDGET).unwrap_err(),
        MediaError::UnsupportedFormat
    );
    assert_eq!(
        decode_still(&[], Detail::STANDARD, BUDGET).unwrap_err(),
        MediaError::Empty
    );
}
